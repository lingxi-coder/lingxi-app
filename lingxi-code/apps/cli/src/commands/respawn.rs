//! `lingxi-cli respawn [id|--all]` — restart background sessions with the
//! current CLI binary.
//!
//! A manual respawn is a durable stop → resume transition.  The existing
//! daemon owns worker/PTY termination and the normal pending-job sweep owns
//! spawning, so this command only rotates the launch kind and reopens the
//! durable job state.  No second supervisor or process protocol is introduced.

use clap::Args;
use std::path::{Path, PathBuf};

pub(crate) const PHASE_CREATING: &str = "creating";
pub(crate) const PHASE_QUEUED: &str = "queued";
pub(crate) const PHASE_LAUNCHING: &str = "launching";
pub(crate) const PHASE_RUNNING: &str = "running";
pub(crate) const PHASE_RESTARTING: &str = "restarting";
pub(crate) const PHASE_DELETING: &str = "deleting";
pub(crate) const CLAIM_LEASE_MS: i64 = 30_000;
pub(crate) const CLAIM_OWNER_DISPATCH: &str = "dispatch";
pub(crate) const CLAIM_OWNER_LAUNCH: &str = "launch";
pub(crate) const CLAIM_OWNER_RESTART: &str = "restart";
pub(crate) const CLAIM_OWNER_DELETE: &str = "delete";
pub(crate) const BG_WORKER_GENERATION_ENV: &str = "LINGXI_BG_WORKER_GENERATION";
pub(crate) const BG_WORKER_CLAIM_TOKEN_ENV: &str = "LINGXI_BG_WORKER_CLAIM_TOKEN";

/// The locked `claude respawn --help` text from 2.1.252.
pub const RESPAWN_HELP: &str = "Usage: claude respawn <id>|--all\n\n  Restart a background session (or all of them) so it picks up the current Claude binary.\n";

/// Bare usage emitted for a missing target or an invalid combination.
pub const RESPAWN_USAGE: &str = "usage: claude respawn <id>|--all";

#[derive(Debug, Clone, Args)]
#[command(disable_help_flag = true)]
pub struct Cli {
    /// Restart every live background session.
    #[arg(long = "all")]
    pub all: bool,

    /// Display help for command.
    #[arg(short = 'h', long = "help")]
    pub help: bool,

    /// Background session id (or an unambiguous prefix).
    #[arg(value_name = "id", allow_hyphen_values = true)]
    pub id: Option<String>,
}

fn valid_short(short: &str) -> bool {
    short.len() == 8
        && short
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn real_job_dir(home: &Path, short: &str) -> bool {
    let path = crate::agents_registry::jobs_dir(home).join(short);
    std::fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_dir())
        .unwrap_or(false)
}

fn jobs(home: &Path) -> Vec<(String, crate::agents_registry::JobState)> {
    crate::agents_registry::read_jobs(&crate::agents_registry::jobs_dir(home))
        .into_iter()
        .filter(|(short, _)| valid_short(short) && real_job_dir(home, short))
        .collect()
}

fn resolve_short(home: &Path, id: &str) -> Result<String, crate::commands::rm::PrefixMatch> {
    let shorts: Vec<String> = jobs(home).into_iter().map(|(short, _)| short).collect();
    match crate::commands::rm::match_prefix(&shorts, id) {
        crate::commands::rm::PrefixMatch::Unique(short) => Ok(short),
        other => Err(other),
    }
}

/// Make an existing launch context resume its transcript rather than replaying
/// the original prompt.  Legacy roster data is migrated through the existing
/// launch loader before the final read/write, when possible.
fn load_launch_spec_for_resume(
    home: &Path,
    short: &str,
) -> Result<crate::background_launch::BackgroundLaunchSpec, String> {
    match crate::background_launch::read_launch_spec(home, short) {
        Ok(spec) => Ok(spec),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            crate::background_launch::load_or_migrate_launch_spec(home, home, short)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "background launch context is missing".to_string())?;
            crate::background_launch::read_launch_spec(home, short)
                .map_err(|error| error.to_string())
        }
        Err(error) => Err(error.to_string()),
    }
}

fn validate_resumable_transcript(
    home: &Path,
    spec: &crate::background_launch::BackgroundLaunchSpec,
) -> Result<(), String> {
    let path = PathBuf::from(&spec.transcript_path);
    let metadata = std::fs::symlink_metadata(&path)
        .map_err(|e| format!("could not inspect transcript {}: {e}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("transcript is not a regular file".to_string());
    }
    if metadata.len() == 0 {
        return Err("transcript contains no resumable conversation".to_string());
    }
    let canonical_path = std::fs::canonicalize(&path)
        .map_err(|e| format!("could not resolve transcript {}: {e}", path.display()))?;
    let projects_root = home.join("projects");
    let canonical_projects = std::fs::canonicalize(&projects_root).map_err(|e| {
        format!(
            "could not resolve transcript root {}: {e}",
            projects_root.display()
        )
    })?;
    let expected_name = format!("{}.jsonl", spec.session_id);
    if !canonical_path.starts_with(&canonical_projects)
        || canonical_path.file_name().and_then(|name| name.to_str()) != Some(&expected_name)
    {
        return Err("transcript path does not match the recorded session".to_string());
    }
    Ok(())
}

pub(crate) fn prepare_resume(home: &Path, short: &str) -> Result<(), String> {
    let mut spec = load_launch_spec_for_resume(home, short)?;
    validate_resumable_transcript(home, &spec)?;
    let mut changed = false;
    if spec.launch == crate::background_launch::BackgroundLaunchKind::Fresh {
        spec.launch = crate::background_launch::BackgroundLaunchKind::Resume;
        changed = true;
    }
    if spec.initial_prompt.is_some() {
        spec.initial_prompt = None;
        changed = true;
    }
    if changed {
        crate::background_launch::write_launch_spec(home, short, &spec)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JobClaim {
    pub generation: String,
    pub token: String,
}

fn claimed_state(expected_state: &str) -> &str {
    if expected_state == "stopped" {
        "working"
    } else {
        expected_state
    }
}

fn now_millis() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => i64::try_from(duration.as_millis()).unwrap_or(i64::MAX),
        Err(_) => 0,
    }
}

pub(crate) fn claim_resume_if_matches(
    home: &Path,
    short: &str,
    expected_state: &str,
    expected_phase: Option<&str>,
    expected_worker_pid: Option<i32>,
    expected_worker_proc_start: Option<&str>,
    expected_generation: Option<&str>,
) -> Result<Option<JobClaim>, String> {
    let claim = JobClaim {
        generation: uuid::Uuid::new_v4().to_string(),
        token: uuid::Uuid::new_v4().to_string(),
    };
    let updated = crate::agents_registry::patch_job_state_if_matches(
        home,
        short,
        crate::agents_registry::JobStateMatch {
            state: expected_state,
            phase: expected_phase,
            worker_pid: expected_worker_pid,
            worker_proc_start: expected_worker_proc_start,
            worker_generation: expected_generation,
            claim_token: None,
            claim_owner: None,
            claim_created_at: None,
            claim_lease_ms: None,
        },
        crate::agents_registry::JobStatePatch {
            state: Some(claimed_state(expected_state)),
            tempo: None,
            cwd: None,
            detail: Some(None),
            worker_pid: Some(None),
            worker_proc_start: Some(None),
            phase: Some(Some(PHASE_RESTARTING)),
            worker_generation: Some(Some(&claim.generation)),
            claim_token: Some(Some(&claim.token)),
            claim_owner: Some(Some(CLAIM_OWNER_RESTART)),
            claim_created_at: Some(Some(now_millis())),
            claim_lease_ms: Some(Some(CLAIM_LEASE_MS)),
        },
    )
    .map_err(|_| "couldn't persist restart state, retry in a moment".to_string())?;
    Ok(updated.then_some(claim))
}

fn activate_claimed_resume(home: &Path, short: &str, claim: &JobClaim) -> Result<bool, String> {
    crate::agents_registry::patch_job_state_if_matches(
        home,
        short,
        crate::agents_registry::JobStateMatch {
            state: "working",
            phase: Some(PHASE_RESTARTING),
            worker_pid: None,
            worker_proc_start: None,
            worker_generation: Some(&claim.generation),
            claim_token: Some(&claim.token),
            claim_owner: Some(CLAIM_OWNER_RESTART),
            claim_created_at: None,
            claim_lease_ms: Some(CLAIM_LEASE_MS),
        },
        crate::agents_registry::JobStatePatch {
            state: Some("working"),
            tempo: None,
            cwd: None,
            detail: Some(None),
            worker_pid: Some(None),
            worker_proc_start: Some(None),
            phase: Some(Some(PHASE_QUEUED)),
            worker_generation: Some(Some(&claim.generation)),
            claim_token: Some(None),
            claim_owner: Some(None),
            claim_created_at: Some(None),
            claim_lease_ms: Some(None),
        },
    )
    .map_err(|_| "couldn't persist restart state, retry in a moment".to_string())
}

pub(crate) fn restore_claimed_resume(
    home: &Path,
    short: &str,
    state: &str,
    phase: Option<&str>,
    worker_pid: Option<i32>,
    worker_proc_start: Option<&str>,
    worker_generation: Option<&str>,
    claim: &JobClaim,
) -> Result<bool, String> {
    crate::agents_registry::patch_job_state_if_matches(
        home,
        short,
        crate::agents_registry::JobStateMatch {
            state: "working",
            phase: Some(PHASE_RESTARTING),
            worker_pid: None,
            worker_proc_start: None,
            worker_generation: Some(&claim.generation),
            claim_token: Some(&claim.token),
            claim_owner: Some(CLAIM_OWNER_RESTART),
            claim_created_at: None,
            claim_lease_ms: Some(CLAIM_LEASE_MS),
        },
        crate::agents_registry::JobStatePatch {
            state: Some(state),
            tempo: None,
            cwd: None,
            detail: Some(None),
            worker_pid: Some(worker_pid),
            worker_proc_start: Some(worker_proc_start),
            phase: Some(phase),
            worker_generation: Some(worker_generation),
            claim_token: Some(None),
            claim_owner: Some(None),
            claim_created_at: Some(None),
            claim_lease_ms: Some(None),
        },
    )
    .map_err(|_| "couldn't persist restart state, retry in a moment".to_string())
}

pub(crate) fn fail_claimed_resume(
    home: &Path,
    short: &str,
    claim: &JobClaim,
    detail: Option<&str>,
) -> Result<bool, String> {
    crate::agents_registry::patch_job_state_if_matches(
        home,
        short,
        crate::agents_registry::JobStateMatch {
            state: "working",
            phase: Some(PHASE_RESTARTING),
            worker_pid: None,
            worker_proc_start: None,
            worker_generation: Some(&claim.generation),
            claim_token: Some(&claim.token),
            claim_owner: Some(CLAIM_OWNER_RESTART),
            claim_created_at: None,
            claim_lease_ms: Some(CLAIM_LEASE_MS),
        },
        crate::agents_registry::JobStatePatch {
            state: Some("failed"),
            tempo: None,
            cwd: None,
            detail: Some(detail),
            worker_pid: Some(None),
            worker_proc_start: Some(None),
            phase: Some(None),
            worker_generation: Some(None),
            claim_token: Some(None),
            claim_owner: Some(None),
            claim_created_at: Some(None),
            claim_lease_ms: Some(None),
        },
    )
    .map_err(|_| "couldn't persist restart state, retry in a moment".to_string())
}

pub(crate) fn queue_prepared_resume(
    home: &Path,
    short: &str,
    claim: &JobClaim,
) -> Result<(), String> {
    if !activate_claimed_resume(home, short, claim)? {
        return Err("session state changed while restart was being queued".to_string());
    }
    Ok(())
}

pub(crate) fn queue_prepared_resume_if_matches(
    home: &Path,
    short: &str,
    expected_state: &str,
    expected_phase: Option<&str>,
    expected_worker_pid: Option<i32>,
    expected_worker_proc_start: Option<&str>,
    expected_generation: Option<&str>,
) -> Result<bool, String> {
    let Some(claim) = claim_resume_if_matches(
        home,
        short,
        expected_state,
        expected_phase,
        expected_worker_pid,
        expected_worker_proc_start,
        expected_generation,
    )?
    else {
        return Ok(false);
    };
    activate_claimed_resume(home, short, &claim)
}

pub(crate) fn queue_resume(home: &Path, short: &str) -> Result<(), String> {
    let Some(job) = jobs(home)
        .into_iter()
        .find(|(candidate, job)| {
            candidate == short && !crate::agents_registry::job_is_terminal(job)
        })
        .map(|(_, job)| job)
    else {
        return Err("background job not found".to_string());
    };
    if !queue_resume_if_matches(
        home,
        short,
        &job.state,
        job.phase.as_deref(),
        job.worker_pid,
        job.worker_proc_start.as_deref(),
        job.worker_generation.as_deref(),
    )? {
        return Err("session state changed while restart was being queued".to_string());
    }
    Ok(())
}

pub(crate) fn queue_resume_if_matches(
    home: &Path,
    short: &str,
    expected_state: &str,
    expected_phase: Option<&str>,
    expected_worker_pid: Option<i32>,
    expected_worker_proc_start: Option<&str>,
    expected_generation: Option<&str>,
) -> Result<bool, String> {
    let Some(claim) = claim_resume_if_matches(
        home,
        short,
        expected_state,
        expected_phase,
        expected_worker_pid,
        expected_worker_proc_start,
        expected_generation,
    )?
    else {
        return Ok(false);
    };
    if let Err(error) = prepare_resume(home, short) {
        let _ = fail_claimed_resume(home, short, &claim, Some(error.as_str()));
        return Err(error);
    }
    activate_claimed_resume(home, short, &claim)
}

#[cfg(test)]
fn read_launch_spec(home: &Path, short: &str) -> crate::background_launch::BackgroundLaunchSpec {
    crate::background_launch::read_launch_spec(home, short).unwrap()
}

/// Stop, switch to resume, and leave a durable `working` job for the daemon's
/// ordinary pending-worker sweep.  The operation is deliberately synchronous:
/// a success line means the old writer was confirmed gone and a new launch is
/// durably queued, not merely that a request file was emitted.
fn respawn_one(
    home: &Path,
    short: &str,
    job: &crate::agents_registry::JobState,
) -> Result<(), String> {
    if !crate::commands::daemon::stop_background_job(home, short, job) {
        return Err("still running — couldn't confirm restart, retry in a moment".to_string());
    }
    let Some(job) = crate::agents_registry::read_job(home, short) else {
        return Err("background job not found".to_string());
    };
    if !queue_resume_if_matches(
        home,
        short,
        &job.state,
        job.phase.as_deref(),
        job.worker_pid,
        job.worker_proc_start.as_deref(),
        job.worker_generation.as_deref(),
    )? {
        return Err("session state changed while restart was being queued".to_string());
    }
    Ok(())
}

fn proven_live_worker_conflicts(
    home: &Path,
    short: &str,
    job: &crate::agents_registry::JobState,
) -> Result<bool, String> {
    use crate::daemon_roster::ProcProbe as _;
    let probe = crate::daemon_roster::SystemProbe;
    let roster = crate::daemon_roster::read_roster(home, 0, false).into_roster();
    if let Some(record) = roster.workers.get(short) {
        let verified = record.proc_start.as_deref().is_some_and(|stored| {
            probe.is_alive(record.pid)
                && probe
                    .start_time(record.pid)
                    .as_deref()
                    .is_some_and(|live| live == stored)
        });
        if verified {
            return Ok(true);
        }
        if probe.is_alive(record.pid) {
            return Err("background worker generation could not be verified".to_string());
        }
    }
    if let Some(pid) = job.worker_pid {
        if !probe.is_alive(pid) {
            return Ok(false);
        }
        let live_start = probe.start_time(pid);
        if job
            .worker_proc_start
            .as_deref()
            .zip(live_start.as_deref())
            .is_some_and(|(expected, live)| expected == live)
        {
            return Ok(true);
        }
        if job.worker_proc_start.is_some() {
            return Ok(false);
        }
        return Err("background worker generation could not be verified".to_string());
    }
    Ok(false)
}

pub(crate) fn queue_resume_for_short_if_safe(
    home: &Path,
    short: &str,
    session_id: &str,
) -> Result<bool, String> {
    let Some(job) = jobs(home)
        .into_iter()
        .find(|(candidate, job)| {
            candidate == short
                && job.session_id.as_deref() == Some(session_id)
                && !crate::agents_registry::job_is_terminal(job)
        })
        .map(|(_, job)| job)
    else {
        return Ok(false);
    };
    if proven_live_worker_conflicts(home, short, &job)? {
        return Err("session is still running in the background".to_string());
    }
    queue_resume_if_matches(
        home,
        short,
        &job.state,
        job.phase.as_deref(),
        job.worker_pid,
        job.worker_proc_start.as_deref(),
        job.worker_generation.as_deref(),
    )
}

fn print_single_error(short: &str, reason: &str) {
    if reason == "still running — couldn't confirm restart, retry in a moment" {
        println!("{short}: {reason}");
    } else {
        eprintln!("{short}: {reason}");
    }
}

/// Run the `respawn` family.
pub async fn run(cli: &Cli) -> i32 {
    if cli.help {
        print!("{RESPAWN_HELP}");
        return crate::exit_codes::SUCCESS;
    }
    if cli.all && cli.id.is_some() {
        eprintln!("{RESPAWN_USAGE}");
        return crate::exit_codes::ARGV_ERROR;
    }
    let home = crate::run::lingxi_home_dir();
    if cli.all {
        let live: Vec<_> = jobs(&home)
            .into_iter()
            .filter(|(_, job)| !crate::agents_registry::job_is_terminal(job))
            .collect();
        if live.is_empty() {
            println!("no live jobs to respawn");
            return crate::exit_codes::SUCCESS;
        }
        let mut all_ok = true;
        let mut queued_any = false;
        for (short, job) in live {
            match respawn_one(&home, &short, &job) {
                Ok(()) => {
                    queued_any = true;
                    println!("respawned {short}");
                }
                Err(reason) => {
                    all_ok = false;
                    print_single_error(&short, &reason);
                }
            }
        }
        // Successful entries must not depend on every sibling succeeding.
        // Start/wake the supervisor even when the aggregate exit is non-zero.
        if queued_any {
            crate::background_dispatch::ensure_daemon_for_control(&home);
        }
        if all_ok {
            tracing::info!(event = "cli_bg_respawn", all = true);
            crate::exit_codes::SUCCESS
        } else {
            crate::exit_codes::RUNTIME_ERROR
        }
    } else {
        let Some(id) = cli.id.as_deref().filter(|id| !id.is_empty()) else {
            eprintln!("{RESPAWN_USAGE}");
            return crate::exit_codes::ARGV_ERROR;
        };
        if id.starts_with('-') {
            eprintln!("unknown option '{id}'");
            eprintln!("{RESPAWN_USAGE}");
            return crate::exit_codes::ARGV_ERROR;
        }
        let short = match resolve_short(&home, id) {
            Ok(short) => short,
            Err(crate::commands::rm::PrefixMatch::None) => {
                eprintln!("No job matching '{id}'");
                return crate::exit_codes::RUNTIME_ERROR;
            }
            Err(crate::commands::rm::PrefixMatch::Ambiguous(matches)) => {
                eprintln!("{}", crate::commands::rm::ambiguous_message(id, &matches));
                return crate::exit_codes::RUNTIME_ERROR;
            }
            Err(crate::commands::rm::PrefixMatch::Unique(_)) => unreachable!(),
        };
        let job = jobs(&home)
            .into_iter()
            .find(|(candidate, _)| candidate == &short)
            .map(|(_, job)| job)
            .unwrap_or_default();
        match respawn_one(&home, &short, &job) {
            Ok(()) => {
                crate::background_dispatch::ensure_daemon_for_control(&home);
                tracing::info!(event = "cli_bg_respawn", short = short.as_str());
                println!("respawned {short}");
                crate::exit_codes::SUCCESS
            }
            Err(reason) => {
                print_single_error(&short, &reason);
                crate::exit_codes::RUNTIME_ERROR
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::background_launch::{
        write_launch_spec, BackgroundLaunchKind, BackgroundLaunchOptions, BackgroundLaunchSpec,
        TerminalSize, LAUNCH_SPEC_VERSION,
    };

    fn seed_launch_spec(
        home: &std::path::Path,
        short: &str,
        session_id: &str,
        cwd: &std::path::Path,
        prompt: &str,
    ) {
        let transcript =
            session::jsonl::path::session_path(home, &cwd.display().to_string(), session_id);
        std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        std::fs::write(
            &transcript,
            "{\"type\":\"user\",\"message\":{\"role\":\"user\"}}\n",
        )
        .unwrap();
        write_launch_spec(
            home,
            short,
            &BackgroundLaunchSpec {
                schema_version: LAUNCH_SPEC_VERSION,
                short: short.to_string(),
                created_at: 0,
                preflight_approved: true,
                launch: BackgroundLaunchKind::Fresh,
                session_id: session_id.to_string(),
                transcript_path: transcript.display().to_string(),
                cwd: cwd.display().to_string(),
                origin_cwd: cwd.display().to_string(),
                worktree_path: None,
                worktree_ownership_token: None,
                initial_prompt: Some(prompt.to_string()),
                handoff: None,
                options: BackgroundLaunchOptions::default(),
                env: std::collections::BTreeMap::new(),
                terminal: TerminalSize::default(),
            },
        )
        .unwrap();
    }

    fn seed_job(home: &std::path::Path, short: &str, session_id: &str) {
        let respawn: Vec<String> = Vec::new();
        let job = crate::agents_registry::JobStateWrite {
            state: "working",
            tempo: Some("active"),
            name: None,
            session_id: Some(session_id),
            cwd: Some("/work"),
            origin_cwd: Some("/work"),
            created_at: Some("2026-07-04T00:00:00.000Z"),
            intent: Some("intent"),
            display_intent: None,
            template: Some("bg"),
            respawn_flags: &respawn,
            in_flight: None,
            backend: Some("daemon"),
            initial_prompt: Some("prompt"),
            detail: None,
            worker_pid: None,
            worker_proc_start: None,
            phase: Some("queued"),
            worker_generation: None,
            claim_token: None,
            claim_owner: None,
            claim_created_at: None,
            claim_lease_ms: None,
        };
        crate::agents_registry::write_job_state(home, short, &job).unwrap();
    }

    #[test]
    fn help_and_usage_are_byte_exact() {
        assert_eq!(
            RESPAWN_HELP,
            "Usage: claude respawn <id>|--all\n\n  Restart a background session (or all of them) so it picks up the current Claude binary.\n"
        );
        assert_eq!(RESPAWN_USAGE, "usage: claude respawn <id>|--all");
    }

    #[test]
    fn resolver_uses_prefix_matching_and_rejects_symlink_dirs() {
        let home = tempfile::tempdir().unwrap();
        let jobs_dir = crate::agents_registry::jobs_dir(home.path());
        for short in ["abcd1234", "abce1234"] {
            let job = jobs_dir.join(short);
            std::fs::create_dir_all(&job).unwrap();
            std::fs::write(
                job.join("state.json"),
                r#"{"state":"working","tempo":"active"}"#,
            )
            .unwrap();
        }
        assert!(matches!(
            resolve_short(home.path(), "abc"),
            Err(crate::commands::rm::PrefixMatch::Ambiguous(_))
        ));
        assert_eq!(resolve_short(home.path(), "abcd").unwrap(), "abcd1234");
    }

    #[test]
    fn prepare_resume_switches_to_resume_and_clears_initial_prompt() {
        let home = tempfile::tempdir().unwrap();
        let short = "abcd1234";
        let session_id = "11111111-1111-1111-1111-111111111111";
        let cwd = home.path().join("work");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();
        seed_launch_spec(home.path(), short, session_id, &cwd, "do the thing");

        prepare_resume(home.path(), short).unwrap();
        let spec = read_launch_spec(home.path(), short);
        assert_eq!(spec.launch, BackgroundLaunchKind::Resume);
        assert_eq!(spec.initial_prompt, None);
    }

    #[test]
    fn prepare_resume_rejects_missing_or_empty_transcript() {
        let home = tempfile::tempdir().unwrap();
        let short = "beef0001";
        let session_id = "22222222-2222-2222-2222-222222222222";
        let cwd = home.path().join("work");
        std::fs::create_dir_all(&cwd).unwrap();
        let transcript =
            session::jsonl::path::session_path(home.path(), &cwd.display().to_string(), session_id);
        std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        std::fs::write(&transcript, "").unwrap();
        write_launch_spec(
            home.path(),
            short,
            &BackgroundLaunchSpec {
                schema_version: LAUNCH_SPEC_VERSION,
                short: short.to_string(),
                created_at: 0,
                preflight_approved: true,
                launch: BackgroundLaunchKind::Fresh,
                session_id: session_id.to_string(),
                transcript_path: transcript.display().to_string(),
                cwd: cwd.display().to_string(),
                origin_cwd: cwd.display().to_string(),
                worktree_path: None,
                worktree_ownership_token: None,
                initial_prompt: Some("do the thing".to_string()),
                handoff: None,
                options: BackgroundLaunchOptions::default(),
                env: std::collections::BTreeMap::new(),
                terminal: TerminalSize::default(),
            },
        )
        .unwrap();

        let err = prepare_resume(home.path(), short).unwrap_err();
        assert!(err.contains("no resumable conversation"));
        let spec = read_launch_spec(home.path(), short);
        assert_eq!(spec.launch, BackgroundLaunchKind::Fresh);
        assert_eq!(spec.initial_prompt.as_deref(), Some("do the thing"));
    }

    #[test]
    fn queue_resume_for_short_if_safe_only_touches_the_exact_job() {
        let home = tempfile::tempdir().unwrap();
        let session_id = "33333333-3333-3333-3333-333333333333";
        let cwd = home.path().join("work");
        std::fs::create_dir_all(&cwd).unwrap();
        seed_job(home.path(), "abcd1234", session_id);
        seed_job(home.path(), "abcd5678", session_id);
        seed_launch_spec(home.path(), "abcd1234", session_id, &cwd, "first prompt");
        seed_launch_spec(home.path(), "abcd5678", session_id, &cwd, "second prompt");

        assert!(queue_resume_for_short_if_safe(home.path(), "abcd1234", session_id).unwrap());

        let first = read_launch_spec(home.path(), "abcd1234");
        let second = read_launch_spec(home.path(), "abcd5678");
        assert_eq!(first.launch, BackgroundLaunchKind::Resume);
        assert_eq!(first.initial_prompt, None);
        assert_eq!(second.launch, BackgroundLaunchKind::Fresh);
        assert_eq!(second.initial_prompt.as_deref(), Some("second prompt"));
    }
}

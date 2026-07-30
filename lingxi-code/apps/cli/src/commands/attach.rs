//! `lingxi-cli attach <job-or-session-id>` — attach to a live background PTY.
//!
//! This module is deliberately the single boundary between CLI surfaces and
//! `bg_attach`'s wire protocol.  Both the public command and the interactive
//! agents view resolve the durable job/roster state here, which prevents them
//! from drifting when the attach transport is upgraded.

use clap::Args;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const STALL_RECONNECT_POLL: Duration = Duration::from_millis(50);
const STALL_RECONNECT_TIMEOUT: Duration =
    Duration::from_millis(crate::bg_attach_stall::RESPAWN_EXIT_WAIT_MS);

/// Attach to a running background job by its short job id or session UUID.
#[derive(Debug, Clone, Args)]
pub struct Cli {
    /// Background job short id or session UUID
    #[arg(value_name = "job-or-session-id")]
    pub target: String,
}

/// The result of resolving and attempting a live attach.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AttachDisposition {
    /// The transport connected and returned normally (including detach).
    Attached,
    /// The selector is not present in either the job store or daemon roster.
    NotFound,
    /// The job/session is known, but no associated worker is alive.
    NotRunning {
        short: String,
        session_id: Option<String>,
    },
    /// A worker is alive, but a safe authenticated attach endpoint is absent.
    LiveEndpointUnavailable {
        short: String,
        session_id: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LiveAttachTarget {
    short: String,
    socket: PathBuf,
    auth: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ResolvedAttachTarget {
    Live(LiveAttachTarget),
    NotRunning {
        short: String,
        session_id: Option<String>,
    },
    LiveEndpointUnavailable {
        short: String,
        session_id: Option<String>,
    },
}

/// Run the public `attach` command.
pub async fn run(cli: &Cli) -> i32 {
    let home = crate::run::lingxi_home_dir();
    match attach_target(&home, &cli.target) {
        Ok(AttachDisposition::Attached) => crate::exit_codes::SUCCESS,
        Ok(AttachDisposition::NotFound) => {
            eprintln!(
                "lingxi-cli attach: background job or session `{}` was not found",
                cli.target
            );
            crate::exit_codes::RUNTIME_ERROR
        }
        Ok(AttachDisposition::NotRunning { short, session_id }) => {
            let display = session_id.as_deref().unwrap_or(&short);
            eprintln!("lingxi-cli attach: `{display}` is not running (job {short})");
            crate::exit_codes::RUNTIME_ERROR
        }
        Ok(AttachDisposition::LiveEndpointUnavailable { short, session_id }) => {
            let display = session_id.as_deref().unwrap_or(&short);
            eprintln!(
                "lingxi-cli attach: `{display}` is still running, but its live attach endpoint is unavailable (job {short})"
            );
            crate::exit_codes::RUNTIME_ERROR
        }
        Err(e) => {
            eprintln!("lingxi-cli attach: {e}");
            crate::exit_codes::RUNTIME_ERROR
        }
    }
}

/// Resolve `selector` and connect when it names a live attachable worker.
///
/// Keeping the `bg_attach` call in this function gives protocol v2 one stable
/// integration point.  `agents` uses the same function, so it cannot
/// accidentally start a second transcript writer while a live worker exists.
pub(crate) fn attach_target(home: &Path, selector: &str) -> std::io::Result<AttachDisposition> {
    let mut previous_stalled_auth: Option<String> = None;
    let mut reconnect_deadline: Option<Instant> = None;

    loop {
        let resolved = resolve_attach_target(home, selector);
        if let (
            Some(previous_auth),
            Some(ResolvedAttachTarget::Live(LiveAttachTarget { auth, .. })),
        ) = (&previous_stalled_auth, &resolved)
        {
            if auth == previous_auth {
                if reconnect_deadline.is_some_and(|deadline| Instant::now() < deadline) {
                    std::thread::sleep(STALL_RECONNECT_POLL);
                    continue;
                }
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "timed out waiting for stalled background worker to restart",
                ));
            }
        } else if previous_stalled_auth.is_some() {
            if reconnect_deadline.is_some_and(|deadline| Instant::now() < deadline) {
                std::thread::sleep(STALL_RECONNECT_POLL);
                continue;
            }
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "timed out waiting for stalled background worker to restart",
            ));
        }

        match resolved {
            None => return Ok(AttachDisposition::NotFound),
            Some(ResolvedAttachTarget::NotRunning { short, session_id }) => {
                return Ok(AttachDisposition::NotRunning { short, session_id });
            }
            Some(ResolvedAttachTarget::LiveEndpointUnavailable { short, session_id }) => {
                return Ok(AttachDisposition::LiveEndpointUnavailable { short, session_id });
            }
            Some(ResolvedAttachTarget::Live(target)) => {
                match crate::bg_attach::attach_to_socket(
                    &target.socket,
                    &target.auth,
                    &target.short,
                ) {
                    Ok(()) => return Ok(AttachDisposition::Attached),
                    Err(error) if crate::bg_attach::is_stall_restart_requested(&error) => {
                        previous_stalled_auth = Some(target.auth);
                        reconnect_deadline = Some(Instant::now() + STALL_RECONNECT_TIMEOUT);
                    }
                    Err(error) => return Err(error),
                }
            }
        }
    }
}

fn resolve_attach_target(home: &Path, selector: &str) -> Option<ResolvedAttachTarget> {
    use crate::agents_registry as reg;

    let jobs = reg::read_jobs(&reg::jobs_dir(home));
    let mut roster = crate::daemon_roster::read_roster(
        home,
        i32::try_from(std::process::id()).unwrap_or(0),
        false,
    )
    .into_roster();
    let probe = crate::daemon_roster::SystemProbe;
    // Reject PID-reused/stale records before trusting their authenticated
    // endpoint. This is an in-memory filter; the daemon remains the only roster
    // writer.
    let _ = crate::daemon_roster::retain_adoptable(&mut roster, &probe);
    resolve_attach_target_from(
        &jobs,
        &roster,
        selector,
        &|pid| crate::daemon_roster::ProcProbe::is_alive(&probe, pid),
        &|pid| crate::daemon_roster::ProcProbe::start_time(&probe, pid),
    )
}

/// Pure selector resolver used by command parsing/liveness tests.
///
/// Exact job ids win over session matches.  A session may have stale historical
/// jobs, so session lookup considers all matching shorts and prefers the newest
/// live roster record.  If a job reports a live worker but the roster lacks a
/// usable endpoint, the result is `LiveEndpointUnavailable` rather than
/// `NotRunning`; callers must not fall back to a second `--resume` writer.
fn resolve_attach_target_from(
    jobs: &[(String, crate::agents_registry::JobState)],
    roster: &crate::daemon_roster::Roster,
    selector: &str,
    is_alive: &dyn Fn(i32) -> bool,
    start_time: &dyn Fn(i32) -> Option<String>,
) -> Option<ResolvedAttachTarget> {
    let exact_short =
        jobs.iter().any(|(short, _)| short == selector) || roster.workers.contains_key(selector);

    let mut candidates = Vec::new();
    let mut seen = HashSet::new();
    if exact_short {
        candidates.push(selector.to_string());
    } else {
        for (short, job) in jobs {
            if job.session_id.as_deref() == Some(selector) && seen.insert(short.clone()) {
                candidates.push(short.clone());
            }
        }
        for (short, record) in &roster.workers {
            if record.session_id == selector && seen.insert(short.clone()) {
                candidates.push(short.clone());
            }
        }
    }
    if candidates.is_empty() {
        return None;
    }

    // Newest live worker wins for a session selector. Exact short lookup has
    // one candidate and is unaffected by this ordering.
    candidates.sort_by_key(|short| {
        std::cmp::Reverse(
            roster
                .workers
                .get(short)
                .map_or(i64::MIN, |record| record.started_at),
        )
    });

    let mut best_unavailable = None;
    let mut best_not_running = None;
    for short in candidates {
        let job = jobs
            .iter()
            .find_map(|(candidate, job)| (candidate == &short).then_some(job));
        let record = roster.workers.get(&short);
        let session_id = record
            .and_then(|record| (!record.session_id.is_empty()).then(|| record.session_id.clone()))
            .or_else(|| job.and_then(|job| job.session_id.clone()));

        if let Some(record) = record {
            // An attach endpoint grants interactive control over a process, so
            // liveness alone is insufficient: a recycled PID must never make
            // an old bearer token target an unrelated process. Legacy roster
            // records without a captured start time fail closed.
            let identity_verified = record.proc_start.as_deref().is_some_and(|stored| {
                is_alive(record.pid)
                    && start_time(record.pid)
                        .as_deref()
                        .is_some_and(|live| live == stored)
            });
            let session_matches =
                job.and_then(|job| job.session_id.as_deref())
                    .is_none_or(|job_session| {
                        record.session_id.is_empty() || record.session_id == job_session
                    });
            let worker_matches = job
                .and_then(|job| job.worker_pid)
                .is_none_or(|job_pid| job_pid == record.pid);
            if session_matches && worker_matches && identity_verified {
                let socket = record.pty_sock.as_deref().filter(|value| !value.is_empty());
                let auth = record.pty_auth.as_deref().filter(|value| !value.is_empty());
                if let (Some(socket), Some(auth)) = (socket, auth) {
                    return Some(ResolvedAttachTarget::Live(LiveAttachTarget {
                        short,
                        socket: PathBuf::from(socket),
                        auth: auth.to_string(),
                    }));
                }
                best_unavailable.get_or_insert(ResolvedAttachTarget::LiveEndpointUnavailable {
                    short,
                    session_id,
                });
                continue;
            }
        }

        if record.is_some_and(|record| {
            record.proc_start.as_deref().is_some_and(|stored| {
                is_alive(record.pid)
                    && start_time(record.pid)
                        .as_deref()
                        .is_some_and(|live| live == stored)
            })
        }) {
            best_unavailable
                .get_or_insert(ResolvedAttachTarget::LiveEndpointUnavailable { short, session_id });
        } else {
            best_not_running.get_or_insert(ResolvedAttachTarget::NotRunning { short, session_id });
        }
    }

    best_unavailable.or(best_not_running)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents_registry::JobState;
    use crate::daemon_roster::{
        empty_roster, Dispatch, DispatchSource, Isolation, Launch, WorkerRecord, PROTO,
    };
    use crate::{argv::Argv, commands::Commands};

    fn worker(session_id: &str, socket: &str, auth: &str, started_at: i64) -> WorkerRecord {
        WorkerRecord {
            pid: 4321,
            proc_start: Some("START-4321".to_string()),
            session_id: session_id.to_string(),
            rendezvous_sock: socket.to_string(),
            pty_sock: Some(socket.to_string()),
            messaging_sock: None,
            cli_version: Some("0.0.0".to_string()),
            started_at,
            attempt: 0,
            cwd: "/work".to_string(),
            worktree_path: None,
            dispatch: Dispatch {
                proto: PROTO,
                short: "bead0001".to_string(),
                nonce: None,
                session_id: session_id.to_string(),
                created_at: started_at,
                source: DispatchSource::Shell,
                cwd: "/work".to_string(),
                launch: Launch::Prompt {
                    args: vec!["hi".to_string()],
                },
                launch_spec: None,
                env: std::collections::BTreeMap::new(),
                reattach_env: None,
                worktree: None,
                isolation: Isolation::None,
                respawn_flags: Vec::new(),
                attach_stall_respawns: None,
                agent: None,
                routine: None,
                seed: None,
                cols: None,
                rows: None,
            },
            pending_respawn: None,
            dec_modes: None,
            rv_auth: Some(auth.to_string()),
            pty_auth: Some(auth.to_string()),
            extra: serde_json::Map::new(),
        }
    }

    #[test]
    fn public_command_parses_required_selector() {
        let argv = Argv::from_iter(["lingxi-cli", "attach", "bead0001"]).unwrap();
        match argv.command {
            Some(Commands::Attach(cli)) => assert_eq!(cli.target, "bead0001"),
            other => panic!("expected attach command, got {other:?}"),
        }

        assert!(Argv::from_iter(["lingxi-cli", "attach"]).is_err());
    }

    #[test]
    fn resolver_accepts_job_short_and_session_id() {
        let jobs = vec![(
            "bead0001".to_string(),
            JobState {
                session_id: Some("sid-live".to_string()),
                worker_pid: Some(4321),
                ..Default::default()
            },
        )];
        let mut roster = empty_roster(999);
        roster.workers.insert(
            "bead0001".to_string(),
            worker("sid-live", "/tmp/live.sock", "token-1", 100),
        );

        for selector in ["bead0001", "sid-live"] {
            let resolved =
                resolve_attach_target_from(&jobs, &roster, selector, &|pid| pid == 4321, &|pid| {
                    (pid == 4321).then(|| "START-4321".to_string())
                });
            assert_eq!(
                resolved,
                Some(ResolvedAttachTarget::Live(LiveAttachTarget {
                    short: "bead0001".to_string(),
                    socket: PathBuf::from("/tmp/live.sock"),
                    auth: "token-1".to_string(),
                }))
            );
        }
    }

    #[test]
    fn resolver_requires_v2_pty_fields_and_rejects_legacy_endpoint() {
        let jobs = vec![(
            "bead0001".to_string(),
            JobState {
                session_id: Some("sid-live".to_string()),
                worker_pid: Some(4321),
                ..Default::default()
            },
        )];
        let mut roster = empty_roster(999);
        let mut record = worker("sid-live", "/tmp/legacy.sock", "legacy-token", 100);
        record.pty_sock = Some("/tmp/pty.sock".to_string());
        record.pty_auth = Some("pty-token".to_string());
        roster.workers.insert("bead0001".to_string(), record);

        let resolved =
            resolve_attach_target_from(&jobs, &roster, "sid-live", &|pid| pid == 4321, &|pid| {
                (pid == 4321).then(|| "START-4321".to_string())
            });
        let Some(ResolvedAttachTarget::Live(target)) = resolved else {
            panic!("expected live target");
        };
        assert_eq!(target.socket, PathBuf::from("/tmp/pty.sock"));
        assert_eq!(target.auth, "pty-token");

        let record = roster.workers.get_mut("bead0001").unwrap();
        record.pty_sock = None;
        record.pty_auth = None;
        assert_eq!(
            resolve_attach_target_from(&jobs, &roster, "sid-live", &|pid| pid == 4321, &|pid| (pid
                == 4321)
                .then(|| "START-4321".to_string()),),
            Some(ResolvedAttachTarget::LiveEndpointUnavailable {
                short: "bead0001".to_string(),
                session_id: Some("sid-live".to_string()),
            })
        );
    }

    #[test]
    fn bare_job_pid_is_not_trusted_without_a_verified_roster_identity() {
        let jobs = vec![(
            "bead0001".to_string(),
            JobState {
                session_id: Some("sid-live".to_string()),
                worker_pid: Some(4321),
                ..Default::default()
            },
        )];
        let roster = empty_roster(999);
        assert_eq!(
            resolve_attach_target_from(&jobs, &roster, "sid-live", &|pid| pid == 4321, &|pid| (pid
                == 4321)
                .then(|| "START-4321".to_string()),),
            Some(ResolvedAttachTarget::NotRunning {
                short: "bead0001".to_string(),
                session_id: Some("sid-live".to_string()),
            })
        );
    }

    #[test]
    fn resolver_rejects_missing_or_recycled_process_identity() {
        let jobs = vec![(
            "bead0001".to_string(),
            JobState {
                session_id: Some("sid-live".to_string()),
                worker_pid: Some(4321),
                ..Default::default()
            },
        )];
        let mut roster = empty_roster(999);
        let mut record = worker("sid-live", "/tmp/live.sock", "token-1", 100);
        record.proc_start = None;
        roster.workers.insert("bead0001".to_string(), record);

        for live_start in [Some("START-4321".to_string()), Some("RECYCLED".to_string())] {
            assert_eq!(
                resolve_attach_target_from(
                    &jobs,
                    &roster,
                    "sid-live",
                    &|pid| pid == 4321,
                    &|_pid| live_start.clone(),
                ),
                Some(ResolvedAttachTarget::NotRunning {
                    short: "bead0001".to_string(),
                    session_id: Some("sid-live".to_string()),
                })
            );
        }

        roster.workers.get_mut("bead0001").unwrap().proc_start = Some("ORIGINAL".to_string());
        assert_eq!(
            resolve_attach_target_from(&jobs, &roster, "sid-live", &|pid| pid == 4321, &|_pid| {
                Some("RECYCLED".to_string())
            },),
            Some(ResolvedAttachTarget::NotRunning {
                short: "bead0001".to_string(),
                session_id: Some("sid-live".to_string()),
            })
        );
    }

    #[test]
    fn stopped_and_unknown_targets_are_distinct() {
        let jobs = vec![(
            "bead0001".to_string(),
            JobState {
                session_id: Some("sid-stopped".to_string()),
                worker_pid: Some(9999),
                ..Default::default()
            },
        )];
        let roster = empty_roster(999);
        assert_eq!(
            resolve_attach_target_from(&jobs, &roster, "sid-stopped", &|_pid| false, &|_pid| None,),
            Some(ResolvedAttachTarget::NotRunning {
                short: "bead0001".to_string(),
                session_id: Some("sid-stopped".to_string()),
            })
        );
        assert_eq!(
            resolve_attach_target_from(&jobs, &roster, "missing", &|_pid| false, &|_pid| None,),
            None
        );
    }

    #[test]
    fn session_selector_prefers_newest_live_job_over_stale_history() {
        let jobs = vec![
            (
                "old00001".to_string(),
                JobState {
                    session_id: Some("sid-shared".to_string()),
                    worker_pid: Some(9999),
                    ..Default::default()
                },
            ),
            (
                "new00001".to_string(),
                JobState {
                    session_id: Some("sid-shared".to_string()),
                    worker_pid: Some(4321),
                    ..Default::default()
                },
            ),
        ];
        let mut roster = empty_roster(999);
        roster.workers.insert(
            "old00001".to_string(),
            worker("sid-shared", "/tmp/old.sock", "old", 10),
        );
        roster.workers.insert(
            "new00001".to_string(),
            worker("sid-shared", "/tmp/new.sock", "new", 20),
        );

        let resolved =
            resolve_attach_target_from(&jobs, &roster, "sid-shared", &|pid| pid == 4321, &|pid| {
                (pid == 4321).then(|| "START-4321".to_string())
            });
        let Some(ResolvedAttachTarget::Live(target)) = resolved else {
            panic!("expected newest live target");
        };
        assert_eq!(target.short, "new00001");
    }
}

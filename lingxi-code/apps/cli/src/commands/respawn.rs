//! `lingxi-cli respawn [id|--all]` — restart background sessions with the
//! current CLI binary.
//!
//! A manual respawn is a durable stop → resume transition.  The existing
//! daemon owns worker/PTY termination and the normal pending-job sweep owns
//! spawning, so this command only rotates the launch kind and reopens the
//! durable job state.  No second supervisor or process protocol is introduced.

use clap::Args;
use std::path::Path;

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
fn prepare_resume(home: &Path, short: &str) -> Result<(), String> {
    let mut spec = match crate::background_launch::read_launch_spec(home, short) {
        Ok(spec) => spec,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            crate::background_launch::load_or_migrate_launch_spec(home, home, short)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "background launch context is missing".to_string())?;
            crate::background_launch::read_launch_spec(home, short)
                .map_err(|error| error.to_string())?
        }
        Err(error) => return Err(error.to_string()),
    };
    if spec.launch == crate::background_launch::BackgroundLaunchKind::Fresh {
        spec.launch = crate::background_launch::BackgroundLaunchKind::Resume;
        crate::background_launch::write_launch_spec(home, short, &spec)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Stop, switch to resume, and leave a durable `working` job for the daemon's
/// ordinary pending-worker sweep.  The operation is deliberately synchronous:
/// a success line means the old writer was confirmed gone and a new launch is
/// durably queued, not merely that a request file was emitted.
fn respawn_one(
    home: &Path,
    short: &str,
    job: &crate::agents_registry::JobState,
) -> Result<(), &'static str> {
    prepare_resume(home, short).map_err(|_| "launch context is unavailable")?;
    if !crate::commands::daemon::stop_background_job(home, short, job) {
        return Err("still running — couldn't confirm restart, retry in a moment");
    }
    crate::agents_registry::update_job_state(home, short, "working", None)
        .map_err(|_| "couldn't persist restart state, retry in a moment")?;
    Ok(())
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
                    print_single_error(&short, reason);
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
                print_single_error(&short, reason);
                crate::exit_codes::RUNTIME_ERROR
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}

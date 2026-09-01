//! `lingxi-cli stop|kill <id>` — stop a background session while retaining its
//! conversation for a later `attach`/resume.
//!
//! The public command delegates process and PTY cleanup to the daemon module's
//! shared control seam.  This keeps PID/PTY handling in one place and makes a
//! stop issued from a shell equivalent to the daemon's own graceful shutdown.

use clap::Args;
use std::path::Path;

/// The locked `claude stop --help` text from 2.1.252.  `kill` is a hidden
/// spelling alias, so it intentionally renders the canonical `stop` usage.
pub const STOP_HELP: &str = "Usage: claude stop <id>\n\n  Stop a background session. Its conversation is kept; resume it later with `claude attach <id>`.\n";

/// Bare usage emitted when `<id>` is omitted.
pub const STOP_USAGE: &str = "Usage: claude stop <id>";

#[derive(Debug, Clone, Args)]
#[command(disable_help_flag = true)]
pub struct Cli {
    /// Display help for command.
    #[arg(short = 'h', long = "help")]
    pub help: bool,

    /// Background session id (or an unambiguous prefix).
    #[arg(value_name = "id")]
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

fn resolve_job(
    home: &Path,
    id: &str,
) -> Result<(String, crate::agents_registry::JobState), crate::commands::rm::PrefixMatch> {
    let jobs = crate::agents_registry::read_jobs(&crate::agents_registry::jobs_dir(home));
    let shorts: Vec<String> = jobs
        .iter()
        .map(|(short, _)| short)
        .filter(|short| valid_short(short) && real_job_dir(home, short))
        .cloned()
        .collect();
    let short = match crate::commands::rm::match_prefix(&shorts, id) {
        crate::commands::rm::PrefixMatch::Unique(short) => short,
        other => return Err(other),
    };
    let job = jobs
        .into_iter()
        .find(|(candidate, _)| candidate == &short)
        .map(|(_, job)| job)
        .unwrap_or_default();
    Ok((short, job))
}

/// Run the `stop`/`kill` family.
pub async fn run(cli: &Cli) -> i32 {
    if cli.help {
        print!("{STOP_HELP}");
        return crate::exit_codes::SUCCESS;
    }
    let Some(id) = cli.id.as_deref().filter(|id| !id.is_empty()) else {
        eprintln!("{STOP_USAGE}");
        return crate::exit_codes::ARGV_ERROR;
    };

    let home = crate::run::lingxi_home_dir();
    let (short, job) = match resolve_job(&home, id) {
        Ok(job) => job,
        Err(crate::commands::rm::PrefixMatch::None) => {
            eprintln!("{}", crate::commands::rm::no_job_message(id));
            return crate::exit_codes::RUNTIME_ERROR;
        }
        Err(crate::commands::rm::PrefixMatch::Ambiguous(matches)) => {
            eprintln!("{}", crate::commands::rm::ambiguous_message(id, &matches));
            return crate::exit_codes::RUNTIME_ERROR;
        }
        Err(crate::commands::rm::PrefixMatch::Unique(_)) => unreachable!(),
    };

    if !crate::commands::daemon::stop_background_job(&home, &short, &job) {
        eprintln!("{}", crate::commands::rm::couldnt_confirm_message(&short));
        return crate::exit_codes::RUNTIME_ERROR;
    }
    tracing::info!(event = "cli_bg_stop", short = short.as_str());
    println!("stopped {short}");
    crate::exit_codes::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_and_usage_are_byte_exact() {
        assert_eq!(
            STOP_HELP,
            "Usage: claude stop <id>\n\n  Stop a background session. Its conversation is kept; resume it later with `claude attach <id>`.\n"
        );
        assert_eq!(STOP_USAGE, "Usage: claude stop <id>");
    }

    #[test]
    fn resolver_ignores_symlink_job_directories() {
        let home = tempfile::tempdir().unwrap();
        let jobs = crate::agents_registry::jobs_dir(home.path());
        std::fs::create_dir_all(jobs.join("abcd1234")).unwrap();
        std::fs::write(
            jobs.join("abcd1234/state.json"),
            r#"{"state":"working","tempo":"active"}"#,
        )
        .unwrap();
        let victim = home.path().join("victim");
        std::fs::create_dir_all(&victim).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&victim, jobs.join("deadbeef")).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(&victim, jobs.join("deadbeef")).unwrap();
        assert!(resolve_job(home.path(), "abcd").is_ok());
        assert!(matches!(
            resolve_job(home.path(), "dead"),
            Err(crate::commands::rm::PrefixMatch::None)
        ));
    }
}

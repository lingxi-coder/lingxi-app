//! `lingxi-cli logs <id>` — print a background session's recent terminal output.
//!
//! The daemon's attach stream is intentionally live-only: a command launched
//! after a detach cannot replay bytes from that socket.  Workers therefore
//! keep a bounded, owner-readable `output.log` beside `state.json`; this
//! command reads it through the rooted no-follow filesystem seam and prints
//! the most recent 500 lines, matching Claude's recent-output contract.

use clap::Args;
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// The locked `claude logs --help` text from 2.1.252.
pub const LOGS_HELP: &str =
    "Usage: claude logs <id>\n\n  Print the background session's recent terminal output.\n";

/// Bare usage emitted when `<id>` is omitted.
pub const LOGS_USAGE: &str = "Usage: claude logs <id>";

const MAX_LOG_LINES: usize = 500;

/// `logs` accepts one background-job prefix.  The positional is optional so
/// the handler can emit commander's bare usage line instead of clap's richer
/// missing-argument diagnostic.
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

fn resolve_short(home: &Path, id: &str) -> Result<String, crate::commands::rm::PrefixMatch> {
    let shorts: Vec<String> =
        crate::agents_registry::read_jobs(&crate::agents_registry::jobs_dir(home))
            .into_iter()
            .map(|(short, _)| short)
            .filter(|short| valid_short(short) && real_job_dir(home, short))
            .collect();
    match crate::commands::rm::match_prefix(&shorts, id) {
        crate::commands::rm::PrefixMatch::Unique(short) => Ok(short),
        other => Err(other),
    }
}

fn output_log_relative(short: &str) -> PathBuf {
    PathBuf::from("jobs")
        .join(short)
        .join(crate::background_launch::OUTPUT_LOG_FILE)
}

fn tail_lines(bytes: &[u8], max_lines: usize) -> &[u8] {
    if max_lines == 0 || bytes.is_empty() {
        return &[];
    }
    let mut starts = vec![0usize];
    for (index, byte) in bytes.iter().copied().enumerate() {
        if byte == b'\n' {
            starts.push(index + 1);
        }
    }
    // A final newline terminates the preceding line; it does not create a
    // user-visible extra line for tail purposes.
    let line_count = starts
        .len()
        .saturating_sub(usize::from(bytes.ends_with(b"\n")));
    if line_count <= max_lines {
        return bytes;
    }
    &bytes[starts[line_count - max_lines]..]
}

fn read_output_log(home: &Path, short: &str) -> Result<Option<Vec<u8>>, String> {
    match platform_api::rooted_fs::read_tail_bytes(
        home,
        &output_log_relative(short),
        crate::background_launch::OUTPUT_LOG_MAX_BYTES,
    ) {
        Ok(bytes) => Ok(Some(tail_lines(&bytes, MAX_LOG_LINES).to_vec())),
        Err(platform_api::FsError::NotFound(_)) => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

/// Run the `logs` command.
pub async fn run(cli: &Cli) -> i32 {
    if cli.help {
        print!("{LOGS_HELP}");
        return crate::exit_codes::SUCCESS;
    }
    let Some(id) = cli.id.as_deref().filter(|id| !id.is_empty()) else {
        eprintln!("{LOGS_USAGE}");
        return crate::exit_codes::ARGV_ERROR;
    };

    let home = crate::run::lingxi_home_dir();
    let short = match resolve_short(&home, id) {
        Ok(short) => short,
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

    match read_output_log(&home, &short) {
        Ok(Some(output)) => {
            if let Err(error) = std::io::stdout().write_all(&output) {
                eprintln!("Couldn't write logs for {short} — {error}");
                return crate::exit_codes::RUNTIME_ERROR;
            }
        }
        Ok(None) => {}
        Err(error) => {
            eprintln!("Couldn't read logs for {short} — {error}");
            return crate::exit_codes::RUNTIME_ERROR;
        }
    }
    tracing::info!(event = "cli_bg_logs", short = short.as_str());
    crate::exit_codes::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_and_usage_are_byte_exact() {
        assert_eq!(
            LOGS_HELP,
            "Usage: claude logs <id>\n\n  Print the background session's recent terminal output.\n"
        );
        assert_eq!(LOGS_USAGE, "Usage: claude logs <id>");
    }

    #[test]
    fn tail_lines_keeps_the_recent_lines_and_newline() {
        assert_eq!(tail_lines(b"one\ntwo\nthree\n", 2), b"two\nthree\n");
        assert_eq!(tail_lines(b"one\ntwo\nthree", 2), b"two\nthree");
        assert_eq!(tail_lines(b"one\ntwo", 5), b"one\ntwo");
    }

    #[test]
    fn output_logs_are_read_rooted_and_missing_logs_are_empty() {
        let home = tempfile::tempdir().unwrap();
        let job = crate::agents_registry::jobs_dir(home.path()).join("abcd1234");
        std::fs::create_dir_all(&job).unwrap();
        std::fs::write(
            job.join(crate::background_launch::OUTPUT_LOG_FILE),
            "old\nnew\n",
        )
        .unwrap();
        assert_eq!(
            read_output_log(home.path(), "abcd1234").unwrap().as_deref(),
            Some(b"old\nnew\n".as_slice())
        );
        assert_eq!(read_output_log(home.path(), "deadbeef").unwrap(), None);
    }

    #[test]
    fn large_and_non_utf8_logs_return_a_bounded_raw_tail() {
        let home = tempfile::tempdir().unwrap();
        let job = crate::agents_registry::jobs_dir(home.path()).join("abcd1234");
        std::fs::create_dir_all(&job).unwrap();
        let mut body = vec![
            b'x';
            usize::try_from(crate::background_launch::OUTPUT_LOG_MAX_BYTES).unwrap()
                + 128
        ];
        body.extend_from_slice(b"\nlatest\n");
        body.extend_from_slice(&[0xff, b'\n']);
        std::fs::write(job.join(crate::background_launch::OUTPUT_LOG_FILE), &body).unwrap();

        let tail = read_output_log(home.path(), "abcd1234")
            .unwrap()
            .expect("log exists");
        assert!(
            tail.len() <= usize::try_from(crate::background_launch::OUTPUT_LOG_MAX_BYTES).unwrap()
        );
        assert!(tail.ends_with(&[b'l', b'a', b't', b'e', b's', b't', b'\n', 0xff, b'\n']));
    }
}

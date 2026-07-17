//! `lingxi-cli rm <id>` — Delete a background session and its worktree
//! (cc 2.1.207 P2-11 remainder, dossier fix #3: the permanent-deletion
//! lifecycle).
//!
//! Byte-faithful surface for the real 2.1.207 `claude rm <id>` command
//! (verified via `strings`/byte-dumps of the binary):
//!
//! * `-h`/`--help` → the full usage + description block (stdout, exit 0):
//!   `Usage: claude rm <id>\n\n  Delete a background session and its worktree.
//!   Unlike `stop`, works on already-exited sessions.\n`
//! * missing `<id>` → stderr bare `Usage: claude rm <id>`, exit 1.
//! * `<id>` prefix-matches the `jobs/<short>` dirs
//!   ([`crate::agents_registry`]): no match → stderr `No job matching
//!   '<id>'. Run 'claude agents' to list running sessions.` exit 1; an
//!   ambiguous prefix → stderr `Ambiguous prefix '<id>', matches: <a>, <b>`
//!   exit 1 (matches sorted, joined with `, `).
//! * a still-running worker that cannot be confirmed stopped → stderr
//!   `couldn't confirm <id> was stopped — the background service may be
//!   restarting. Try again in a moment.` exit 1.
//! * otherwise the `jobs/<short>` state dir is removed (and, when the job
//!   carried a managed worktree that could not be removed, retained with the
//!   binary's `keptReason` text appended); stdout `removed <id>` (+ the
//!   worktree-kept suffix when applicable), exit 0.
//!
//! Telemetry: `tengu_bg_agent_action{action:"delete",source:"cli"}` and the
//! CLI analytics event `cli_bg_rm` (both parity-name only — routed through the
//! `tracing` event sink like `daemon.rs`'s `tengu_bg_*` emissions).
//!
//! The `claude` product name is kept in the user-typed recipe strings (help,
//! `Run 'claude agents'`) exactly as the binary/fixtures pin it — the same
//! convention `argv.rs` uses for its `claude --bg`/`claude agents` recipes and
//! `gateway.rs` uses for its `Usage: claude gateway` help.

use clap::Args;
use std::path::{Path, PathBuf};

/// The locked `claude rm --help` text — byte-identical to the 2.1.207 binary
/// (`Usage: claude rm <id>\n\n  Delete a background session and its worktree.
/// Unlike `stop`, works on already-exited sessions.\n`).
pub const RM_HELP: &str = "Usage: claude rm <id>\n\n  Delete a background session and its worktree. Unlike `stop`, works on already-exited sessions.\n";

/// The bare usage line (missing `<id>` / arg error), stderr, exit 1.
pub const RM_USAGE: &str = "Usage: claude rm <id>";

/// `rm` args — a single positional `<id>` (declared optional so the missing-arg
/// path can emit the bare `Usage:` line byte-exactly instead of clap's own
/// required-arg rendering) plus a manual `-h/--help` flag (help auto-flag
/// disabled, same idiom as `gateway.rs`).
#[derive(Debug, Clone, Args)]
#[command(disable_help_flag = true)]
pub struct Cli {
    /// Display help for command (manual: prints the locked help text).
    #[arg(short = 'h', long = "help")]
    pub help: bool,

    /// The background session id (prefix) to delete.
    #[arg(value_name = "id")]
    pub id: Option<String>,
}

/// Which `jobs/<short>` the `<id>` prefix resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrefixMatch {
    /// Exactly one job matched — the resolved short id.
    Unique(String),
    /// No job matched the prefix.
    None,
    /// More than one job matched (sorted short ids).
    Ambiguous(Vec<String>),
}

/// Resolve `<id>` against the known `jobs/<short>` ids (the binary prefix-match).
/// An exact id wins outright; otherwise every short with `id` as a prefix is a
/// candidate. `shorts` is assumed already sorted ([`crate::agents_registry::read_jobs`]
/// sorts ascending); the returned `Ambiguous` list preserves that order.
#[must_use]
pub fn match_prefix(shorts: &[String], id: &str) -> PrefixMatch {
    if shorts.iter().any(|s| s == id) {
        return PrefixMatch::Unique(id.to_string());
    }
    let matches: Vec<String> = shorts
        .iter()
        .filter(|s| s.starts_with(id))
        .cloned()
        .collect();
    match matches.len() {
        0 => PrefixMatch::None,
        1 => PrefixMatch::Unique(matches.into_iter().next().unwrap()),
        _ => PrefixMatch::Ambiguous(matches),
    }
}

/// The reason a job's managed worktree was kept rather than removed — the
/// binary `keptReason` map. Byte-exact reason strings; only `RemoveFailed` is
/// reachable from lingxi today (worktree dirty/branch/lock detection needs a
/// git+lock probe lingxi does not run here), but all four are pinned so the
/// surface matches the oracle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeptReason {
    /// `dirty` → the worktree has uncommitted changes.
    Dirty,
    /// `branch_mismatch` → the worktree is on a different branch.
    BranchMismatch,
    /// `live_lock` → the worktree is locked (live session / by hand).
    LiveLock,
    /// `remove_failed` → the removal itself errored.
    RemoveFailed,
}

impl KeptReason {
    /// The binary's `keptReason` phrase for this reason.
    #[must_use]
    pub fn text(self) -> &'static str {
        match self {
            KeptReason::Dirty => "has uncommitted changes",
            KeptReason::BranchMismatch => "is on a different branch",
            KeptReason::LiveLock => {
                "is locked \u{2014} in use by another live session, or locked by hand"
            }
            KeptReason::RemoveFailed => "could not be removed",
        }
    }
}

/// `No job matching '<id>'. Run 'claude agents' to list running sessions.`
#[must_use]
pub fn no_job_message(id: &str) -> String {
    format!("No job matching '{id}'. Run 'claude agents' to list running sessions.")
}

/// `Ambiguous prefix '<id>', matches: <a>, <b>`
#[must_use]
pub fn ambiguous_message(id: &str, matches: &[String]) -> String {
    format!("Ambiguous prefix '{id}', matches: {}", matches.join(", "))
}

/// `couldn't confirm <id> was stopped — the background service may be
/// restarting. Try again in a moment.`
#[must_use]
pub fn couldnt_confirm_message(short: &str) -> String {
    format!(
        "couldn't confirm {short} was stopped \u{2014} the background service may be restarting. Try again in a moment."
    )
}

/// `removed <id>` with the optional worktree-kept suffix
/// (`\n  worktree <reason> — kept at <path>`).
#[must_use]
pub fn removed_message(short: &str, kept: Option<(KeptReason, &str)>) -> String {
    match kept {
        None => format!("removed {short}"),
        Some((reason, path)) => format!(
            "removed {short}\n  worktree {} \u{2014} kept at {path}",
            reason.text()
        ),
    }
}

/// The managed-worktree path a job carries, if any: its `cwd` when that path is
/// under a `<DOT_DIR>/worktrees/` segment (the layout `job_origin_cwd` strips).
/// A job whose `cwd` is a plain project dir (the common lingxi `--bg` case,
/// where `background_dispatch` leaves `worktree` unset) has no managed worktree.
#[must_use]
pub fn managed_worktree_path(job: &crate::agents_registry::JobState) -> Option<PathBuf> {
    let cwd = job.cwd.as_deref()?;
    for sep in ['/', '\\'] {
        let marker = format!("{sep}{}{sep}worktrees{sep}", branding::DOT_DIR);
        if cwd.contains(&marker) {
            return Some(PathBuf::from(cwd));
        }
    }
    None
}

/// Remove a resolved `jobs/<short>` state dir and, best-effort, its managed
/// worktree; emit the `delete` telemetry; return the worktree-kept reason (if
/// the worktree survived) so the caller can render the `removed <id>` suffix.
///
/// `source` is the `tengu_bg_agent_action` provenance — `"cli"` from the `rm`
/// command, `"fleet"` from the agents view's Ctrl-X delete. Shared by both so
/// the deletion behaves identically regardless of entry point.
pub fn perform_delete(
    home: &Path,
    short: &str,
    job: &crate::agents_registry::JobState,
    source: &str,
) -> Option<(KeptReason, String)> {
    use crate::agents_registry as reg;

    // Best-effort worktree removal FIRST (a kept worktree is reported in the
    // output; a removed one is silent). Only a path clearly under the managed
    // worktrees dir is ever touched — never the user's project checkout.
    let kept = managed_worktree_path(job).and_then(|path| {
        if !path.exists() {
            return None;
        }
        match std::fs::remove_dir_all(&path) {
            Ok(()) => None,
            Err(_) => Some((KeptReason::RemoveFailed, path.display().to_string())),
        }
    });

    // Remove the job state dir (idempotent — a missing dir is a no-op).
    let dir = reg::jobs_dir(home).join(short);
    let _ = std::fs::remove_dir_all(&dir);

    // Parity-name telemetry (routed through the tracing event sink, like
    // daemon.rs's tengu_bg_* emissions).
    tracing::info!(
        event = "tengu_bg_agent_action",
        action = "delete",
        source,
        short
    );

    kept
}

/// Best-effort stop of a job's live worker: `SIGTERM` the recorded `workerPid`
/// (when alive) and poll briefly for it to exit. Returns whether the worker is
/// confirmed stopped (no live worker, or it exited within the grace window).
/// A `None`/dead `workerPid` is already stopped. Shared by the `rm` command and
/// the agents-view Ctrl-X delete / `Ctrl+X Ctrl+K` stop-all paths.
#[cfg(unix)]
#[must_use]
pub fn stop_worker(job: &crate::agents_registry::JobState) -> bool {
    use crate::agents_registry::process_alive;
    let Some(pid) = job.worker_pid else {
        return true;
    };
    if !process_alive(pid) {
        return true;
    }
    // Ask it to stop, then wait up to ~1s for the process to leave the table.
    let _ = nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(pid),
        Some(nix::sys::signal::Signal::SIGTERM),
    );
    for _ in 0..20 {
        if !process_alive(pid) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    false
}

#[cfg(not(unix))]
#[must_use]
pub fn stop_worker(job: &crate::agents_registry::JobState) -> bool {
    // No cheap signal/probe wired on non-unix hosts — treat as stopped (the
    // one-shot workers are terminal by the time `rm` runs).
    job.worker_pid.is_none()
}

/// Run the `rm` family (see the module doc for the binary-verified surface).
pub async fn run(cli: &Cli) -> i32 {
    use crate::agents_registry as reg;

    if cli.help {
        // Full help block, stdout, exit 0 (commander help path).
        print!("{RM_HELP}");
        return crate::exit_codes::SUCCESS;
    }
    let Some(id) = cli.id.as_deref().filter(|s| !s.is_empty()) else {
        // Missing `<id>` — bare usage, stderr, exit 1.
        eprintln!("{RM_USAGE}");
        return crate::exit_codes::ARGV_ERROR;
    };

    let home = crate::run::lingxi_home_dir();
    let jobs = reg::read_jobs(&reg::jobs_dir(&home));
    let shorts: Vec<String> = jobs.iter().map(|(s, _)| s.clone()).collect();

    let short = match match_prefix(&shorts, id) {
        PrefixMatch::None => {
            eprintln!("{}", no_job_message(id));
            return crate::exit_codes::RUNTIME_ERROR;
        }
        PrefixMatch::Ambiguous(matches) => {
            eprintln!("{}", ambiguous_message(id, &matches));
            return crate::exit_codes::RUNTIME_ERROR;
        }
        PrefixMatch::Unique(short) => short,
    };

    let job = jobs
        .iter()
        .find(|(s, _)| *s == short)
        .map(|(_, j)| j.clone())
        .unwrap_or_default();

    // Unlike `stop`, `rm` works on already-exited sessions — but a still-live
    // worker must be stopped first, and if we cannot confirm the stop we refuse
    // rather than orphan a running process.
    if !stop_worker(&job) {
        tracing::info!(
            event = "cli_bg_rm",
            short = short.as_str(),
            outcome = "kill_unconfirmed"
        );
        eprintln!("{}", couldnt_confirm_message(&short));
        return crate::exit_codes::RUNTIME_ERROR;
    }

    let kept = perform_delete(&home, &short, &job, "cli");
    tracing::info!(event = "cli_bg_rm", short = short.as_str(), removed = true);
    println!(
        "{}",
        removed_message(&short, kept.as_ref().map(|(r, p)| (*r, p.as_str())))
    );
    crate::exit_codes::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents_registry::{jobs_dir, write_job_state, JobStateWrite};

    fn shorts(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn argv_routes_rm_token_to_the_subcommand() {
        use crate::argv::Argv;
        use crate::commands::Commands;
        // `lingxi-cli rm <id>` resolves to the Rm subcommand with the positional
        // id (and reports the `rm` top-level name for the version gate).
        let a = Argv::from_iter(["lingxi-cli", "rm", "bc7c6b33"]).unwrap();
        let cmd = a.command.expect("rm token must parse as a subcommand");
        assert_eq!(cmd.top_level_name(), "rm");
        match cmd {
            Commands::Rm(c) => {
                assert_eq!(c.id.as_deref(), Some("bc7c6b33"));
                assert!(!c.help);
            }
            other => panic!("expected Rm, got {other:?}"),
        }
        // `rm -h` sets the manual help flag; no positional required.
        let a = Argv::from_iter(["lingxi-cli", "rm", "-h"]).unwrap();
        match a.command.unwrap() {
            Commands::Rm(c) => {
                assert!(c.help);
                assert!(c.id.is_none());
            }
            other => panic!("expected Rm, got {other:?}"),
        }
    }

    #[test]
    fn help_text_is_byte_exact() {
        // Verified against the 2.1.207 binary byte dump.
        assert_eq!(
            RM_HELP,
            "Usage: claude rm <id>\n\n  Delete a background session and its worktree. Unlike `stop`, works on already-exited sessions.\n"
        );
    }

    #[test]
    fn match_prefix_unique_none_ambiguous() {
        let s = shorts(&["aaaa1111", "aaab2222", "bbbb3333"]);
        // Exact id wins.
        assert_eq!(
            match_prefix(&s, "aaaa1111"),
            PrefixMatch::Unique("aaaa1111".to_string())
        );
        // Unique prefix.
        assert_eq!(
            match_prefix(&s, "bbbb"),
            PrefixMatch::Unique("bbbb3333".to_string())
        );
        // No match.
        assert_eq!(match_prefix(&s, "zzzz"), PrefixMatch::None);
        // Ambiguous prefix — matches preserve sorted order.
        assert_eq!(
            match_prefix(&s, "aaa"),
            PrefixMatch::Ambiguous(shorts(&["aaaa1111", "aaab2222"]))
        );
    }

    #[test]
    fn error_messages_are_byte_exact() {
        assert_eq!(
            no_job_message("dead"),
            "No job matching 'dead'. Run 'claude agents' to list running sessions."
        );
        assert_eq!(
            ambiguous_message("aaa", &shorts(&["aaaa1111", "aaab2222"])),
            "Ambiguous prefix 'aaa', matches: aaaa1111, aaab2222"
        );
        assert_eq!(
            couldnt_confirm_message("bc7c6b33"),
            "couldn't confirm bc7c6b33 was stopped \u{2014} the background service may be restarting. Try again in a moment."
        );
    }

    #[test]
    fn kept_reason_strings_match_binary_map() {
        // The binary `keptReason` map {dirty, branch_mismatch, live_lock,
        // remove_failed}.
        assert_eq!(KeptReason::Dirty.text(), "has uncommitted changes");
        assert_eq!(
            KeptReason::BranchMismatch.text(),
            "is on a different branch"
        );
        assert_eq!(
            KeptReason::LiveLock.text(),
            "is locked \u{2014} in use by another live session, or locked by hand"
        );
        assert_eq!(KeptReason::RemoveFailed.text(), "could not be removed");
    }

    #[test]
    fn removed_message_bare_and_kept_suffix() {
        assert_eq!(removed_message("bc7c6b33", None), "removed bc7c6b33");
        assert_eq!(
            removed_message("bc7c6b33", Some((KeptReason::RemoveFailed, "/w/tree"))),
            "removed bc7c6b33\n  worktree could not be removed \u{2014} kept at /w/tree"
        );
    }

    #[test]
    fn perform_delete_unlinks_the_job_state_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let respawn: Vec<String> = Vec::new();
        let job = JobStateWrite {
            state: "done",
            tempo: Some("idle"),
            name: None,
            session_id: Some("sid-1"),
            cwd: Some("/home/u/proj"),
            origin_cwd: Some("/home/u/proj"),
            created_at: Some("2026-07-04T00:00:00.000Z"),
            intent: Some("do the thing"),
            display_intent: None,
            template: Some("bg"),
            respawn_flags: &respawn,
            in_flight: None,
            backend: Some("daemon"),
            initial_prompt: Some("do the thing"),
            detail: None,
            worker_pid: None,
        };
        write_job_state(home, "bc7c6b33", &job).unwrap();
        let dir = jobs_dir(home).join("bc7c6b33");
        assert!(dir.exists());

        let stored = crate::agents_registry::read_job(home, "bc7c6b33").unwrap();
        let kept = perform_delete(home, "bc7c6b33", &stored, "cli");
        // Plain project cwd → no managed worktree → nothing kept.
        assert!(kept.is_none());
        assert!(!dir.exists(), "jobs/<short> state dir is unlinked");
    }

    #[test]
    fn managed_worktree_detected_only_under_worktrees_dir() {
        use crate::agents_registry::JobState;
        let mut job = JobState {
            cwd: Some("/home/u/proj".to_string()),
            ..JobState::default()
        };
        assert!(managed_worktree_path(&job).is_none());
        job.cwd = Some(format!(
            "/home/u/proj/{}/worktrees/fix-thing",
            branding::DOT_DIR
        ));
        assert_eq!(
            managed_worktree_path(&job),
            Some(PathBuf::from(format!(
                "/home/u/proj/{}/worktrees/fix-thing",
                branding::DOT_DIR
            )))
        );
    }

    #[test]
    fn perform_delete_keeps_unremovable_worktree() {
        // A managed worktree that cannot be removed is retained with the
        // `remove_failed` reason; the job state dir is still unlinked.
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        // Build a real managed-worktree dir, then make removal fail by pointing
        // the job at a path that exists but replacing it with a regular file
        // (remove_dir_all on a file errors).
        let wt = home
            .join("proj")
            .join(branding::DOT_DIR)
            .join("worktrees")
            .join("fix");
        std::fs::create_dir_all(wt.parent().unwrap()).unwrap();
        std::fs::write(&wt, b"not-a-dir").unwrap();
        let wt_str = wt.display().to_string();
        let job = crate::agents_registry::JobState {
            cwd: Some(wt_str.clone()),
            ..Default::default()
        };
        let respawn: Vec<String> = Vec::new();
        let state = JobStateWrite {
            state: "done",
            tempo: Some("idle"),
            name: None,
            session_id: Some("s"),
            cwd: Some(&wt_str),
            origin_cwd: None,
            created_at: None,
            intent: None,
            display_intent: None,
            template: Some("bg"),
            respawn_flags: &respawn,
            in_flight: None,
            backend: None,
            initial_prompt: None,
            detail: None,
            worker_pid: None,
        };
        write_job_state(home, "aaaa1111", &state).unwrap();

        let kept = perform_delete(home, "aaaa1111", &job, "cli");
        let (reason, path) = kept.expect("unremovable worktree is kept");
        assert_eq!(reason, KeptReason::RemoveFailed);
        assert_eq!(path, wt.display().to_string());
        assert!(!jobs_dir(home).join("aaaa1111").exists());
    }
}

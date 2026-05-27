//! Git status probe — produces `Option<GitStatus>` for a cwd.
//!
//! Shells out to the `git` CLI for read-only repo discovery + HEAD
//! branch resolution + dirty-tree detection. This is intentionally
//! simpler than the originally-planned `gix` integration: the
//! workspace pins Rust 1.82 (no edition 2024), and recent `gix`
//! transitives (`gix-trace ≥ 0.1.20`) require edition 2024.
//!
//! The plan locks the PROBE CONTRACT (return shape, error-degrades-to-None);
//! the underlying mechanism is implementation-detail. M5-04 may revisit.
//!
//! Errors at any step degrade gracefully to `None` so the assembler
//! falls back to the non-git env block without panicking.
#![forbid(unsafe_code)]

use crate::prompt::GitStatus;
use std::path::Path;
use std::process::Command;

/// Probe the cwd for a git repo. Returns `None` when:
/// - cwd is not inside a git repo;
/// - the `git` CLI is not on `$PATH`;
/// - the repo is unreadable (permissions, corruption);
/// - HEAD cannot be resolved.
///
/// On success, `working_dir_clean` is `true` when there are no
/// modified, added, deleted, or untracked entries. The
/// `file_changes_summary` field is currently empty in M5-03 (M5-04
/// may extend with `git diff --stat` output if profile permits).
#[must_use]
pub fn probe(cwd: &Path) -> Option<GitStatus> {
    // Discovery: `git rev-parse --is-inside-work-tree` returns "true" + 0
    // when cwd is in a work tree. Anything else → not a repo.
    let inside = run_git(cwd, &["rev-parse", "--is-inside-work-tree"])?;
    if inside.trim() != "true" {
        return None;
    }

    // Branch name. `--abbrev-ref HEAD` prints the short branch (e.g. "main")
    // or "HEAD" when detached.
    let branch_raw = run_git(cwd, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    let branch_trim = branch_raw.trim();
    let branch = if branch_trim == "HEAD" {
        "HEAD detached".to_string()
    } else {
        branch_trim.to_string()
    };

    // Dirty-tree detection. `git status --porcelain` is empty when clean,
    // non-empty when there are unstaged / staged / untracked changes.
    let status = run_git(cwd, &["status", "--porcelain"])?;
    let working_dir_clean = status.is_empty();

    Some(GitStatus {
        branch,
        working_dir_clean,
        file_changes_summary: String::new(),
    })
}

/// Run `git <args>` in `cwd` and return stdout when the command succeeded.
/// Returns `None` on any failure (binary missing, non-zero exit, unreadable
/// repo, …).
fn run_git(cwd: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()
}

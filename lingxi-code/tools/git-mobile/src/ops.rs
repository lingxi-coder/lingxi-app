//! Git operations — operation enum → `git2` calls (filled by Tasks 4-6/8).
//!
//! This module hosts the host-testable, deterministic `git2`-backed
//! implementations of each supported operation:
//!
//! - Task 4: workspace-anchored `open_repo` + path-escape validation +
//!   `GitOpError`.
//! - Task 5: read operations (status / diff / log / show / `branch_list`).
//! - Task 6: local write operations (add / commit / `branch_create` / checkout
//!   / merge fast-forward).
//! - Task 8: network operations (clone / fetch / pull) with the in-process
//!   credential callback + CA wiring.
//!
//! For Task 3 (the tool shell) this module is intentionally empty: the
//! `GitTool::call` dispatch returns a "not yet implemented" `InvalidInput`
//! error for every operation until the `ops::` functions land.

use std::path::Path;

use thiserror::Error;

/// Errors returned by git operations.
#[derive(Debug, Error)]
pub enum GitOpError {
    /// Requested path does not exist or cannot be canonicalized.
    #[error("not found: {0}")]
    NotFound(String),

    /// Requested path escapes the workspace root (symlink or `..` traversal).
    #[error("path escape: {0}")]
    Escape(String),

    /// A libgit2 error.
    #[error("libgit2: {0}")]
    Libgit2(String),

    /// The repository has uncommitted changes that would be clobbered.
    #[error("dirty worktree: {0}")]
    Dirty(String),

    /// The requested merge/pull cannot be resolved as a fast-forward.
    #[error("non-fast-forward: {0}")]
    NonFastForward(String),

    /// The caller supplied invalid or inconsistent parameters.
    #[error("invalid input: {0}")]
    InvalidInput(String),
}

impl GitOpError {
    /// Map a `git2::Error` into a [`GitOpError::Libgit2`].
    #[must_use]
    pub fn from_git2(e: &git2::Error) -> Self {
        Self::Libgit2(e.message().to_owned())
    }
}

/// Open a git repository at `repo_rel` (a path relative to `workspace_root`),
/// enforcing that the canonicalized repo path stays inside the canonicalized
/// workspace root.
///
/// This mirrors the containment check in
/// `AndroidMinijailSandbox::resolve_cwd` (canonicalize + `starts_with`), so
/// symlink escapes and `..` traversals are rejected before `git2` ever touches
/// the path.
///
/// # Errors
///
/// - [`GitOpError::NotFound`] — `workspace_root` or the joined path cannot be
///   canonicalized (directory does not exist).
/// - [`GitOpError::Escape`] — the canonicalized repo path lies outside the
///   canonicalized workspace root.
/// - [`GitOpError::Libgit2`] — `git2::Repository::open` failed (e.g. not a
///   git repo).
pub fn open_repo(workspace_root: &Path, repo_rel: &str) -> Result<git2::Repository, GitOpError> {
    // Canonicalize the workspace root first so we have a clean baseline.
    let canonical_root = workspace_root.canonicalize().map_err(|e| {
        GitOpError::NotFound(format!("workspace root {}: {e}", workspace_root.display()))
    })?;

    // Build the requested repo path and canonicalize it.
    let requested = canonical_root.join(repo_rel);
    let canonical_repo = requested
        .canonicalize()
        .map_err(|e| GitOpError::NotFound(format!("{}: {e}", requested.display())))?;

    // Containment check — mirrors `AndroidMinijailSandbox::resolve_cwd`.
    if !canonical_repo.starts_with(&canonical_root) {
        return Err(GitOpError::Escape(canonical_repo.display().to_string()));
    }

    git2::Repository::open(&canonical_repo).map_err(|e| GitOpError::from_git2(&e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn open_repo_under_workspace_ok() {
        let ws = tempdir().unwrap();
        let repo_dir = ws.path().join("r");
        std::fs::create_dir(&repo_dir).unwrap();
        git2::Repository::init(&repo_dir).unwrap();
        let repo = match open_repo(ws.path(), "r") {
            Ok(r) => r,
            Err(e) => panic!("open under workspace failed: {e}"),
        };
        // workdir is the repo dir (canonicalized)
        assert!(repo
            .workdir()
            .unwrap()
            .starts_with(ws.path().canonicalize().unwrap()));
    }

    /// Helper: call `open_repo` and extract the error, panicking if it
    /// unexpectedly succeeded (works around `git2::Repository: !Debug`).
    fn expect_err(workspace_root: &Path, repo_rel: &str, msg: &str) -> GitOpError {
        match open_repo(workspace_root, repo_rel) {
            Ok(_) => panic!("{msg}: expected Err but got Ok"),
            Err(e) => e,
        }
    }

    #[test]
    fn open_repo_escaping_workspace_rejected() {
        let ws = tempdir().unwrap();
        let outside = tempdir().unwrap();
        git2::Repository::init(outside.path()).unwrap();
        // a relative escape: "../" may resolve outside ws → Escape (or
        // NotFound if canonicalize fails because there is no directory there,
        // or Libgit2 if it resolves to something non-git).
        let err = expect_err(ws.path(), "../", "relative escape");
        assert!(
            matches!(
                err,
                GitOpError::Escape(_) | GitOpError::NotFound(_) | GitOpError::Libgit2(_)
            ),
            "unexpected err for ../: {err:?}"
        );

        // A symlink inside ws pointing outside → strict Escape.
        let link = ws.path().join("sneaky");
        std::os::unix::fs::symlink(outside.path(), &link).unwrap();
        let err = expect_err(ws.path(), "sneaky", "symlink escape");
        assert!(
            matches!(err, GitOpError::Escape(_)),
            "symlink escape must produce GitOpError::Escape, got: {err:?}"
        );
    }

    #[test]
    fn open_nonexistent_repo_named_error() {
        let ws = tempdir().unwrap();
        let err = expect_err(ws.path(), "missing", "nonexistent repo");
        assert!(
            matches!(err, GitOpError::NotFound(_)),
            "missing path must produce GitOpError::NotFound, got: {err:?}"
        );
        // not a panic
    }
}

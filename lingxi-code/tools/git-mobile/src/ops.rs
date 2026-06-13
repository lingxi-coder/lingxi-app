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

use serde::Serialize;
use thiserror::Error;

/// Upper bound on the number of UTF-8 characters of unified-diff text returned
/// by [`diff`] / [`show`]. libgit2 patches over an entire dirty tree can be
/// arbitrarily large; the model-facing tool truncates past this so a single
/// `Git` call can never blow the tool-result budget. When truncation occurs a
/// trailing `\n... [diff truncated]\n` marker is appended.
pub const GIT_DIFF_MAX_CHARS: usize = 100_000;

/// Default cap on the number of commits [`log`] walks back from HEAD when the
/// caller does not request an explicit limit.
pub const GIT_LOG_DEFAULT_MAX: usize = 50;

/// One entry of a `status` listing: a path and its human-readable status flags.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GitStatusEntry {
    /// Path relative to the repository workdir.
    pub path: String,
    /// Status flags as lowercase tokens, e.g. `["wt_modified"]` or
    /// `["index_new"]`. A single path may carry several flags at once
    /// (staged + unstaged changes).
    pub status: Vec<String>,
}

/// Metadata for a single commit, as surfaced by `log` / `show`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GitCommitInfo {
    /// Full commit OID, hex-encoded.
    pub oid: String,
    /// First line of the commit message (may be empty).
    pub summary: String,
    /// Author identity rendered as `Name <email>`.
    pub author: String,
    /// Author time as a Unix timestamp (seconds).
    pub time: i64,
}

/// One entry of a `branch_list` listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GitBranchInfo {
    /// Branch short name (e.g. `main`, `origin/main`).
    pub name: String,
    /// Whether this branch is the one HEAD points at.
    pub is_head: bool,
    /// Whether this is a remote-tracking branch.
    pub is_remote: bool,
}

/// Unified-diff payload returned by `diff` / `show`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GitDiff {
    /// The unified-diff text (patch format), possibly truncated.
    pub patch: String,
    /// Whether [`GitDiff::patch`] was truncated at [`GIT_DIFF_MAX_CHARS`].
    pub truncated: bool,
}

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

/// Render a [`git2::Status`] bitset as lowercase tokens (`wt_modified`,
/// `index_new`, …). A single path can carry several flags (staged + unstaged).
fn status_flags(s: git2::Status) -> Vec<String> {
    use git2::Status;
    let mut out = Vec::new();
    let mut push = |flag: Status, name: &str| {
        if s.contains(flag) {
            out.push(name.to_owned());
        }
    };
    push(Status::INDEX_NEW, "index_new");
    push(Status::INDEX_MODIFIED, "index_modified");
    push(Status::INDEX_DELETED, "index_deleted");
    push(Status::INDEX_RENAMED, "index_renamed");
    push(Status::INDEX_TYPECHANGE, "index_typechange");
    push(Status::WT_NEW, "wt_new");
    push(Status::WT_MODIFIED, "wt_modified");
    push(Status::WT_DELETED, "wt_deleted");
    push(Status::WT_TYPECHANGE, "wt_typechange");
    push(Status::WT_RENAMED, "wt_renamed");
    push(Status::CONFLICTED, "conflicted");
    push(Status::IGNORED, "ignored");
    out
}

/// List the working-tree + index status of every changed path, including
/// untracked files.
///
/// # Errors
///
/// [`GitOpError::Libgit2`] if libgit2 cannot compute the status (e.g. a bare
/// repository, which has no working tree).
pub fn status(repo: &git2::Repository) -> Result<Vec<GitStatusEntry>, GitOpError> {
    let mut opts = git2::StatusOptions::new();
    opts.include_untracked(true).include_ignored(false);
    let statuses = repo
        .statuses(Some(&mut opts))
        .map_err(|e| GitOpError::from_git2(&e))?;
    let mut entries = Vec::with_capacity(statuses.len());
    for entry in statuses.iter() {
        let path = entry.path().unwrap_or_default().to_owned();
        entries.push(GitStatusEntry {
            path,
            status: status_flags(entry.status()),
        });
    }
    Ok(entries)
}

/// Build a [`GitCommitInfo`] from a commit.
fn commit_info(commit: &git2::Commit<'_>) -> GitCommitInfo {
    let author = commit.author();
    let name = author.name().unwrap_or("");
    let email = author.email().unwrap_or("");
    GitCommitInfo {
        oid: commit.id().to_string(),
        summary: commit
            .summary()
            .ok()
            .flatten()
            .unwrap_or_default()
            .to_owned(),
        author: format!("{name} <{email}>"),
        time: author.when().seconds(),
    }
}

/// Walk commit history from HEAD, newest-first, capped at `max`
/// (default [`GIT_LOG_DEFAULT_MAX`]).
///
/// # Errors
///
/// [`GitOpError::Libgit2`] if HEAD is unborn or the revwalk fails.
pub fn log(repo: &git2::Repository, max: Option<usize>) -> Result<Vec<GitCommitInfo>, GitOpError> {
    let cap = max.unwrap_or(GIT_LOG_DEFAULT_MAX);
    let mut walk = repo.revwalk().map_err(|e| GitOpError::from_git2(&e))?;
    walk.set_sorting(git2::Sort::TIME | git2::Sort::TOPOLOGICAL)
        .map_err(|e| GitOpError::from_git2(&e))?;
    walk.push_head().map_err(|e| GitOpError::from_git2(&e))?;

    let mut out = Vec::new();
    for oid in walk.take(cap) {
        let oid = oid.map_err(|e| GitOpError::from_git2(&e))?;
        let commit = repo
            .find_commit(oid)
            .map_err(|e| GitOpError::from_git2(&e))?;
        out.push(commit_info(&commit));
    }
    Ok(out)
}

/// Render a [`git2::Diff`] to unified-patch text, truncating at
/// [`GIT_DIFF_MAX_CHARS`].
fn diff_to_text(diff: &git2::Diff<'_>) -> Result<GitDiff, GitOpError> {
    let mut patch = String::new();
    diff.print(git2::DiffFormat::Patch, |_delta, _hunk, line| {
        // Prefix bytes (`+`/`-`/` `) only for context/add/delete lines; libgit2
        // already embeds the prefix for file/hunk headers.
        match line.origin() {
            '+' | '-' | ' ' => patch.push(line.origin()),
            _ => {}
        }
        patch.push_str(&String::from_utf8_lossy(line.content()));
        true
    })
    .map_err(|e| GitOpError::from_git2(&e))?;

    if patch.chars().count() > GIT_DIFF_MAX_CHARS {
        let truncated: String = patch.chars().take(GIT_DIFF_MAX_CHARS).collect();
        Ok(GitDiff {
            patch: format!("{truncated}\n... [diff truncated]\n"),
            truncated: true,
        })
    } else {
        Ok(GitDiff {
            patch,
            truncated: false,
        })
    }
}

/// Unified diff of the working tree against HEAD (staged + unstaged changes),
/// truncated at [`GIT_DIFF_MAX_CHARS`].
///
/// # Errors
///
/// [`GitOpError::Libgit2`] if HEAD or the diff cannot be resolved.
pub fn diff(repo: &git2::Repository) -> Result<GitDiff, GitOpError> {
    let head_tree = repo
        .head()
        .and_then(|h| h.peel_to_tree())
        .map_err(|e| GitOpError::from_git2(&e))?;
    let mut opts = git2::DiffOptions::new();
    opts.include_untracked(true).recurse_untracked_dirs(true);
    let diff = repo
        .diff_tree_to_workdir_with_index(Some(&head_tree), Some(&mut opts))
        .map_err(|e| GitOpError::from_git2(&e))?;
    diff_to_text(&diff)
}

/// Resolve `rev` and, when it is (or peels to) a commit, return its metadata
/// plus the unified diff against its first parent (or against the empty tree
/// for a root commit), truncated at [`GIT_DIFF_MAX_CHARS`].
///
/// # Errors
///
/// - [`GitOpError::Libgit2`] if `rev` cannot be parsed or peeled.
/// - [`GitOpError::InvalidInput`] if `rev` does not resolve to a commit.
pub fn show(repo: &git2::Repository, rev: &str) -> Result<(GitCommitInfo, GitDiff), GitOpError> {
    let obj = repo
        .revparse_single(rev)
        .map_err(|e| GitOpError::from_git2(&e))?;
    let commit = obj
        .peel_to_commit()
        .map_err(|_| GitOpError::InvalidInput(format!("rev {rev} is not a commit")))?;

    let new_tree = commit.tree().map_err(|e| GitOpError::from_git2(&e))?;
    let parent_tree = match commit.parent(0) {
        Ok(parent) => Some(parent.tree().map_err(|e| GitOpError::from_git2(&e))?),
        Err(_) => None, // root commit — diff against the empty tree.
    };
    let diff = repo
        .diff_tree_to_tree(parent_tree.as_ref(), Some(&new_tree), None)
        .map_err(|e| GitOpError::from_git2(&e))?;
    Ok((commit_info(&commit), diff_to_text(&diff)?))
}

/// List every local + remote-tracking branch.
///
/// # Errors
///
/// [`GitOpError::Libgit2`] if the branch iterator fails.
pub fn branch_list(repo: &git2::Repository) -> Result<Vec<GitBranchInfo>, GitOpError> {
    let branches = repo.branches(None).map_err(|e| GitOpError::from_git2(&e))?;
    let mut out = Vec::new();
    for item in branches {
        let (branch, kind) = item.map_err(|e| GitOpError::from_git2(&e))?;
        let name = branch
            .name()
            .map_err(|e| GitOpError::from_git2(&e))?
            .unwrap_or("")
            .to_owned();
        out.push(GitBranchInfo {
            name,
            is_head: branch.is_head(),
            is_remote: matches!(kind, git2::BranchType::Remote),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// Build a repo at `dir` with a known two-commit history:
    /// - commit "first": adds file `a.txt` = "alpha\n".
    /// - commit "second": modifies `a.txt` -> "alpha2\n" and adds `b.txt`.
    ///
    /// Returns the repo plus the OIDs of (first, second), newest last.
    fn init_history(dir: &Path) -> (git2::Repository, git2::Oid, git2::Oid) {
        let repo = git2::Repository::init(dir).unwrap();
        let sig = git2::Signature::now("Tester", "tester@example.com").unwrap();

        // commit "first"
        std::fs::write(dir.join("a.txt"), "alpha\n").unwrap();
        let first = {
            let mut index = repo.index().unwrap();
            index.add_path(Path::new("a.txt")).unwrap();
            index.write().unwrap();
            let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "first", &tree, &[])
                .unwrap()
        };

        // commit "second"
        std::fs::write(dir.join("a.txt"), "alpha2\n").unwrap();
        std::fs::write(dir.join("b.txt"), "beta\n").unwrap();
        let second = {
            let mut index = repo.index().unwrap();
            index.add_path(Path::new("a.txt")).unwrap();
            index.add_path(Path::new("b.txt")).unwrap();
            index.write().unwrap();
            let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
            let parent = repo.find_commit(first).unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "second", &tree, &[&parent])
                .unwrap()
        };

        (repo, first, second)
    }

    #[test]
    fn status_reports_dirty_file() {
        let dir = tempdir().unwrap();
        let (repo, _first, _second) = init_history(dir.path());
        // Modify a tracked file (don't stage) + add a brand-new untracked file.
        std::fs::write(dir.path().join("a.txt"), "alpha-dirty\n").unwrap();
        std::fs::write(dir.path().join("untracked.txt"), "x\n").unwrap();

        let entries = status(&repo).unwrap();
        let a = entries
            .iter()
            .find(|e| e.path == "a.txt")
            .expect("a.txt should appear in status");
        assert!(
            a.status.iter().any(|s| s == "wt_modified"),
            "a.txt should be wt_modified, got {:?}",
            a.status
        );
        let u = entries
            .iter()
            .find(|e| e.path == "untracked.txt")
            .expect("untracked.txt should appear in status");
        assert!(
            u.status.iter().any(|s| s == "wt_new"),
            "untracked.txt should be wt_new, got {:?}",
            u.status
        );
    }

    #[test]
    fn log_returns_commits_newest_first() {
        let dir = tempdir().unwrap();
        let (repo, first, second) = init_history(dir.path());

        let commits = log(&repo, None).unwrap();
        assert_eq!(commits.len(), 2, "two commits expected");
        assert_eq!(commits[0].summary, "second");
        assert_eq!(commits[0].oid, second.to_string());
        assert_eq!(commits[1].summary, "first");
        assert_eq!(commits[1].oid, first.to_string());
        assert_eq!(commits[0].author, "Tester <tester@example.com>");

        // cap respected
        let capped = log(&repo, Some(1)).unwrap();
        assert_eq!(capped.len(), 1);
        assert_eq!(capped[0].summary, "second");
    }

    #[test]
    fn diff_shows_workdir_change() {
        let dir = tempdir().unwrap();
        let (repo, _first, _second) = init_history(dir.path());
        // Uncommitted change to a tracked file.
        std::fs::write(dir.path().join("a.txt"), "alpha-WORKDIR\n").unwrap();

        let d = diff(&repo).unwrap();
        assert!(
            d.patch.contains("alpha-WORKDIR"),
            "diff should contain the workdir change, got:\n{}",
            d.patch
        );
        assert!(d.patch.contains("a.txt"), "diff should name the file");
        assert!(!d.truncated, "small diff should not truncate");
    }

    #[test]
    fn show_returns_commit_diff() {
        let dir = tempdir().unwrap();
        let (repo, _first, second) = init_history(dir.path());

        let (info, d) = show(&repo, &second.to_string()).unwrap();
        assert_eq!(info.summary, "second");
        assert_eq!(info.oid, second.to_string());
        assert!(
            d.patch.contains("b.txt"),
            "show(second) should include new file b.txt, got:\n{}",
            d.patch
        );
        assert!(
            d.patch.contains("alpha2"),
            "show(second) should include the a.txt change, got:\n{}",
            d.patch
        );
    }

    #[test]
    fn branch_list_includes_default() {
        let dir = tempdir().unwrap();
        let (repo, _first, _second) = init_history(dir.path());

        let branches = branch_list(&repo).unwrap();
        // Default branch name varies (master vs main); assert on is_head.
        let head = branches
            .iter()
            .find(|b| b.is_head)
            .expect("a HEAD branch should exist");
        assert!(!head.is_remote, "default branch is local");
        assert!(!head.name.is_empty());
    }

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

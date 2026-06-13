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
//! Tasks 4-6 are implemented (repo open + read + local-write ops); Task 8 adds
//! the network ops (clone/fetch/pull) with the in-process token credential
//! callback + CA wiring (see [`crate::auth`]). `GitTool::call` dispatches the
//! local ops and the network ops here.

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

/// Default committer identity used when the repository config supplies neither
/// `user.name` nor `user.email`. Only the repository's own config is consulted
/// (never the global / system / env gitconfig), so write ops are deterministic
/// on a fresh app-private clone.
pub const DEFAULT_SIGNATURE_NAME: &str = "LingXi";
/// Default committer email paired with [`DEFAULT_SIGNATURE_NAME`].
pub const DEFAULT_SIGNATURE_EMAIL: &str = "noreply@lingxi";

/// Result of an [`add`] call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GitAddResult {
    /// Total entries in the index after staging (not the delta added by this
    /// call — the index includes all previously-tracked paths).
    pub index_entries: usize,
}

/// Result of a [`commit`] call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GitCommitResult {
    /// Full OID of the newly created commit, hex-encoded.
    pub oid: String,
}

/// Result of a [`branch_create`] call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GitBranchCreateResult {
    /// The branch that was created.
    pub branch: String,
}

/// Result of a [`checkout`] call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GitCheckoutResult {
    /// The target that HEAD now points at (branch name or detached OID).
    pub target: String,
}

/// Result of a [`merge`] call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GitMergeResult {
    /// Whether the merge was a fast-forward (the only kind v1 performs).
    pub fast_forward: bool,
    /// OID HEAD now points at after the fast-forward.
    pub oid: String,
}

/// Resolve the committer/author signature for a write op.
///
/// Reads ONLY the repository's own config (`user.name` / `user.email`); the
/// global, system, and `$GIT_*` env gitconfig are never consulted. When either
/// field is missing, the fixed [`DEFAULT_SIGNATURE_NAME`] /
/// [`DEFAULT_SIGNATURE_EMAIL`] default is used.
fn resolve_signature(repo: &git2::Repository) -> Result<git2::Signature<'static>, GitOpError> {
    // Open ONLY the repository-local config level: never the global / system /
    // `$GIT_*` env gitconfig. `open_level(Local)` may fail (e.g. no local
    // config file yet) — in that case treat every field as absent and fall back
    // to the fixed default, so the result is deterministic on a fresh clone.
    let local = repo
        .config()
        .and_then(|cfg| cfg.open_level(git2::ConfigLevel::Local))
        .ok();
    let get = |key: &str| -> Option<String> {
        local
            .as_ref()
            .and_then(|cfg| cfg.get_string(key).ok())
            .filter(|s| !s.is_empty())
    };
    let name = get("user.name").unwrap_or_else(|| DEFAULT_SIGNATURE_NAME.to_owned());
    let email = get("user.email").unwrap_or_else(|| DEFAULT_SIGNATURE_EMAIL.to_owned());
    git2::Signature::now(&name, &email).map_err(|e| GitOpError::from_git2(&e))
}

/// Stage `paths` into the index (defaulting to all changes, `["*"]`, when
/// empty), then persist the index.
///
/// # Errors
///
/// [`GitOpError::Libgit2`] if the index cannot be opened, the add fails, or the
/// write fails.
pub fn add(repo: &git2::Repository, paths: &[String]) -> Result<GitAddResult, GitOpError> {
    let mut index = repo.index().map_err(|e| GitOpError::from_git2(&e))?;
    let specs: Vec<&str> = if paths.is_empty() {
        vec!["*"]
    } else {
        paths.iter().map(String::as_str).collect()
    };
    index
        .add_all(specs.iter(), git2::IndexAddOption::DEFAULT, None)
        .map_err(|e| GitOpError::from_git2(&e))?;
    index.write().map_err(|e| GitOpError::from_git2(&e))?;
    Ok(GitAddResult {
        index_entries: index.len(),
    })
}

/// Build a tree from the current index and commit it on top of HEAD (or as a
/// root commit when HEAD is unborn). The committer/author identity is resolved
/// by [`resolve_signature`].
///
/// # Errors
///
/// - [`GitOpError::InvalidInput`] if `message` is empty.
/// - [`GitOpError::Libgit2`] if the index/tree/commit cannot be built.
pub fn commit(repo: &git2::Repository, message: &str) -> Result<GitCommitResult, GitOpError> {
    if message.is_empty() {
        return Err(GitOpError::InvalidInput("commit message is empty".into()));
    }
    let sig = resolve_signature(repo)?;
    let mut index = repo.index().map_err(|e| GitOpError::from_git2(&e))?;
    let tree_oid = index.write_tree().map_err(|e| GitOpError::from_git2(&e))?;
    let tree = repo
        .find_tree(tree_oid)
        .map_err(|e| GitOpError::from_git2(&e))?;

    // Parent is the current HEAD commit, if HEAD is born.
    let parent = match repo.head() {
        Ok(head) => Some(
            head.peel_to_commit()
                .map_err(|e| GitOpError::from_git2(&e))?,
        ),
        Err(_) => None, // unborn HEAD — root commit
    };
    let parents: Vec<&git2::Commit<'_>> = parent.iter().collect();

    let oid = repo
        .commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)
        .map_err(|e| GitOpError::from_git2(&e))?;
    Ok(GitCommitResult {
        oid: oid.to_string(),
    })
}

/// Create a new branch `name` pointing at the current HEAD commit.
///
/// # Errors
///
/// [`GitOpError::Libgit2`] if HEAD is unborn or the branch already exists.
pub fn branch_create(
    repo: &git2::Repository,
    name: &str,
) -> Result<GitBranchCreateResult, GitOpError> {
    if name.is_empty() {
        return Err(GitOpError::InvalidInput("branch name is empty".into()));
    }
    let head_commit = repo
        .head()
        .and_then(|h| h.peel_to_commit())
        .map_err(|e| GitOpError::from_git2(&e))?;
    repo.branch(name, &head_commit, false)
        .map_err(|e| GitOpError::from_git2(&e))?;
    Ok(GitBranchCreateResult {
        branch: name.to_owned(),
    })
}

/// Return `true` when the working tree + index have no changes that a checkout
/// could clobber (untracked files are ignored — they are never overwritten by a
/// safe checkout).
fn worktree_is_clean(repo: &git2::Repository) -> Result<bool, GitOpError> {
    let mut opts = git2::StatusOptions::new();
    opts.include_untracked(false).include_ignored(false);
    let statuses = repo
        .statuses(Some(&mut opts))
        .map_err(|e| GitOpError::from_git2(&e))?;
    Ok(statuses.is_empty())
}

/// Switch HEAD + the working tree to `target` (a branch name or revision).
///
/// Refuses to run on a dirty working tree ([`GitOpError::Dirty`]) so no
/// uncommitted change can be silently lost. The checkout itself is performed in
/// safe mode (NOT force), so libgit2 will also refuse rather than overwrite.
///
/// # Errors
///
/// - [`GitOpError::Dirty`] if the working tree has staged/unstaged changes.
/// - [`GitOpError::NotFound`] if `target` cannot be resolved.
/// - [`GitOpError::Libgit2`] if the checkout/`set_head` fails.
pub fn checkout(repo: &git2::Repository, target: &str) -> Result<GitCheckoutResult, GitOpError> {
    if target.is_empty() {
        return Err(GitOpError::InvalidInput("checkout target is empty".into()));
    }
    if !worktree_is_clean(repo)? {
        return Err(GitOpError::Dirty(format!(
            "working tree has uncommitted changes; refusing to checkout {target}"
        )));
    }

    // Resolve the target object and the ref name (if it is a branch) so we can
    // set a symbolic HEAD where possible (otherwise detach to the commit OID).
    let obj = repo
        .revparse_single(target)
        .map_err(|_| GitOpError::NotFound(format!("checkout target {target}")))?;
    let commit = obj
        .peel_to_commit()
        .map_err(|e| GitOpError::from_git2(&e))?;
    let tree = commit.tree().map_err(|e| GitOpError::from_git2(&e))?;

    // Safe checkout (no force): libgit2 refuses to overwrite local modifications.
    let mut co = git2::build::CheckoutBuilder::new();
    co.safe();
    repo.checkout_tree(tree.as_object(), Some(&mut co))
        .map_err(|e| GitOpError::from_git2(&e))?;

    // Prefer a symbolic HEAD when the target is a local branch.
    let head_target = if let Ok(branch) = repo.find_branch(target, git2::BranchType::Local) {
        let refname = branch
            .get()
            .name()
            .map_err(|e| GitOpError::from_git2(&e))?
            .to_owned();
        repo.set_head(&refname)
            .map_err(|e| GitOpError::from_git2(&e))?;
        target.to_owned()
    } else {
        repo.set_head_detached(commit.id())
            .map_err(|e| GitOpError::from_git2(&e))?;
        commit.id().to_string()
    };

    Ok(GitCheckoutResult {
        target: head_target,
    })
}

/// Fast-forward HEAD to `source` when possible; refuse otherwise.
///
/// Runs `merge_analysis` against `source`'s commit. Only an
/// `ANALYSIS_FASTFORWARD` result proceeds (move HEAD + `checkout_tree`); a
/// diverged history yields [`GitOpError::NonFastForward`] with NO 3-way merge
/// attempted and NO conflict markers written. An already-up-to-date HEAD is a
/// no-op fast-forward.
///
/// # Errors
///
/// - [`GitOpError::NotFound`] if `source` cannot be resolved.
/// - [`GitOpError::NonFastForward`] if the merge is not a fast-forward.
/// - [`GitOpError::Libgit2`] on any libgit2 failure.
pub fn merge(repo: &git2::Repository, source: &str) -> Result<GitMergeResult, GitOpError> {
    if source.is_empty() {
        return Err(GitOpError::InvalidInput("merge source is empty".into()));
    }
    let source_obj = repo
        .revparse_single(source)
        .map_err(|_| GitOpError::NotFound(format!("merge source {source}")))?;
    let source_commit = source_obj
        .peel_to_commit()
        .map_err(|e| GitOpError::from_git2(&e))?;
    let annotated = repo
        .find_annotated_commit(source_commit.id())
        .map_err(|e| GitOpError::from_git2(&e))?;

    let (analysis, _pref) = repo
        .merge_analysis(&[&annotated])
        .map_err(|e| GitOpError::from_git2(&e))?;

    if analysis.is_up_to_date() {
        // Already contains `source` — nothing to do; report the current HEAD.
        let head = repo
            .head()
            .and_then(|h| h.peel_to_commit())
            .map_err(|e| GitOpError::from_git2(&e))?;
        return Ok(GitMergeResult {
            fast_forward: true,
            oid: head.id().to_string(),
        });
    }

    if !analysis.is_fast_forward() {
        return Err(GitOpError::NonFastForward(format!(
            "merge of {source} is not a fast-forward; refusing (no 3-way merge in v1)"
        )));
    }

    // Fast-forward: move the current branch ref to the source commit and sync
    // the working tree.
    let target_oid = source_commit.id();
    let tree = source_commit
        .tree()
        .map_err(|e| GitOpError::from_git2(&e))?;
    let mut co = git2::build::CheckoutBuilder::new();
    co.safe();
    repo.checkout_tree(tree.as_object(), Some(&mut co))
        .map_err(|e| GitOpError::from_git2(&e))?;

    // Update the ref HEAD points at (or detached HEAD) to the target commit.
    match repo.head() {
        Ok(head) if head.is_branch() => {
            let refname = head
                .name()
                .map_err(|e| GitOpError::from_git2(&e))?
                .to_owned();
            let mut reference = repo
                .find_reference(&refname)
                .map_err(|e| GitOpError::from_git2(&e))?;
            reference
                .set_target(target_oid, "merge: fast-forward")
                .map_err(|e| GitOpError::from_git2(&e))?;
        }
        _ => {
            repo.set_head_detached(target_oid)
                .map_err(|e| GitOpError::from_git2(&e))?;
        }
    }

    Ok(GitMergeResult {
        fast_forward: true,
        oid: target_oid.to_string(),
    })
}

// =============================================================================
// Network operations (Task 8) — clone / fetch / pull
// =============================================================================

/// Per-operation network configuration carried by `GitTool::call` into the
/// network ops. The HTTPS `token` is supplied in-memory by the Kotlin host and
/// is never written to disk or a child-process env; `ca_dir` points libgit2's
/// TLS backend at a CA-certificate directory (Android system cacerts).
///
/// This rides a SEPARATE secret seam from the public `AndroidGitToolCtx` so the
/// token never enters the broadly-cloned public tool context — see
/// `tool-api`'s `BuiltinToolContext.android_git_secret`.
#[derive(Clone, Default)]
pub struct GitNetConfig {
    /// HTTPS token (PAT) used as the password in the credential callback, or
    /// `None` for anonymous / public remotes. Never logged or persisted.
    pub token: Option<String>,
    /// CA-certificate directory for TLS verification, or `None` to use the
    /// libgit2/OpenSSL defaults (the host `file://` tests need none).
    pub ca_dir: Option<String>,
}

// Manual redacting Debug — `token` must never reach a log line, mirroring
// `tool_api::AndroidGitSecret`. (The struct derives only `Clone`, not `Debug`,
// so this is the sole Debug path.)
impl std::fmt::Debug for GitNetConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitNetConfig")
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .field("ca_dir", &self.ca_dir)
            .finish()
    }
}

/// Result of a [`clone`] call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GitCloneResult {
    /// Absolute path of the freshly cloned working tree.
    pub path: String,
    /// OID of the cloned HEAD commit, hex-encoded.
    pub head: String,
}

/// Result of a [`fetch`] call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GitFetchResult {
    /// The remote that was fetched from.
    pub remote: String,
}

/// Result of a [`pull`] call (fetch + fast-forward merge).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GitPullResult {
    /// Whether the merge half was a fast-forward (the only kind v1 performs).
    pub fast_forward: bool,
    /// OID HEAD points at after the pull.
    pub oid: String,
}

/// Reject `git@…` / `ssh://…` remote URLs (G7 — HTTPS-only in v1).
///
/// # Errors
///
/// [`GitOpError::InvalidInput`] naming ssh/HTTPS when `url` is an SSH remote.
fn reject_ssh_url(url: &str) -> Result<(), GitOpError> {
    let lower = url.trim().to_ascii_lowercase();
    // scp-like `git@host:path` or explicit `ssh://` / `git+ssh://`.
    let is_ssh = lower.starts_with("ssh://")
        || lower.starts_with("git+ssh://")
        || lower.starts_with("git@")
        // generic `user@host:path` scp syntax (has `@` and a `:` before any `/`).
        || (lower.contains('@')
            && !lower.contains("://")
            && lower
                .split_once(':')
                .is_some_and(|(left, _)| left.contains('@')));
    if is_ssh {
        return Err(GitOpError::InvalidInput(
            "ssh URLs are not supported; use HTTPS".into(),
        ));
    }
    Ok(())
}

/// Clone `repo_url` into `dest_rel` (relative to `workspace_root`).
///
/// SSH URLs are rejected (G7). The destination is validated to stay inside the
/// canonicalized workspace root — since `dest` does not exist yet, its PARENT
/// is canonicalized and checked (mirrors [`open_repo`]'s containment guard).
/// The CA location is set first, then `RepoBuilder` clones with the
/// token-bearing fetch options.
///
/// # Errors
///
/// - [`GitOpError::InvalidInput`] — SSH URL, or empty `repo_url`.
/// - [`GitOpError::Escape`] — `dest_rel` resolves outside the workspace root.
/// - [`GitOpError::NotFound`] — the workspace root / dest parent cannot be
///   canonicalized.
/// - [`GitOpError::Libgit2`] — the clone (network / TLS / auth) failed.
pub fn clone(
    net: &GitNetConfig,
    workspace_root: &Path,
    repo_url: &str,
    dest_rel: &str,
) -> Result<GitCloneResult, GitOpError> {
    if repo_url.is_empty() {
        return Err(GitOpError::InvalidInput("clone requires `repo_url`".into()));
    }
    reject_ssh_url(repo_url)?;

    // Resolve + validate the destination. `dest` itself does not exist yet, so
    // canonicalize its PARENT and require that to be inside the workspace root,
    // then re-attach the final component.
    let canonical_root = workspace_root.canonicalize().map_err(|e| {
        GitOpError::NotFound(format!("workspace root {}: {e}", workspace_root.display()))
    })?;
    let requested = canonical_root.join(dest_rel);
    let parent = requested
        .parent()
        .ok_or_else(|| GitOpError::InvalidInput(format!("dest {dest_rel} has no parent")))?;
    let file_name = requested.file_name().ok_or_else(|| {
        GitOpError::InvalidInput(format!("dest {dest_rel} has no final component"))
    })?;
    let canonical_parent = parent
        .canonicalize()
        .map_err(|e| GitOpError::NotFound(format!("{}: {e}", parent.display())))?;
    if !canonical_parent.starts_with(&canonical_root) {
        return Err(GitOpError::Escape(canonical_parent.display().to_string()));
    }
    let dest = canonical_parent.join(file_name);

    // CA wiring first, then clone with token-bearing fetch options.
    crate::auth::set_ca_location(net.ca_dir.as_deref())?;
    let fetch_opts = crate::auth::make_fetch_options(net.token.as_deref());
    let mut builder = git2::build::RepoBuilder::new();
    builder.fetch_options(fetch_opts);
    let repo = builder
        .clone(repo_url, &dest)
        .map_err(|e| GitOpError::from_git2(&e))?;

    let head = repo
        .head()
        .and_then(|h| h.peel_to_commit())
        .map_err(|e| GitOpError::from_git2(&e))?;
    Ok(GitCloneResult {
        path: dest.display().to_string(),
        head: head.id().to_string(),
    })
}

/// Fetch from `remote_name` (default `origin`) using the token-bearing fetch
/// options. The remote's configured URL is also checked against the SSH guard.
///
/// # Errors
///
/// - [`GitOpError::InvalidInput`] — the remote URL is an SSH URL.
/// - [`GitOpError::Libgit2`] — the remote is missing or the fetch failed.
pub fn fetch(
    net: &GitNetConfig,
    repo: &git2::Repository,
    remote_name: &str,
) -> Result<GitFetchResult, GitOpError> {
    let name = if remote_name.is_empty() {
        "origin"
    } else {
        remote_name
    };
    let mut remote = repo
        .find_remote(name)
        .map_err(|e| GitOpError::from_git2(&e))?;
    if let Ok(url) = remote.url() {
        reject_ssh_url(url)?;
    }
    crate::auth::set_ca_location(net.ca_dir.as_deref())?;
    let mut fetch_opts = crate::auth::make_fetch_options(net.token.as_deref());
    // Empty refspec slice -> libgit2 uses the remote's configured default
    // refspecs (refs/heads/* -> refs/remotes/<remote>/*).
    let empty: [&str; 0] = [];
    remote
        .fetch(&empty, Some(&mut fetch_opts), None)
        .map_err(|e| GitOpError::from_git2(&e))?;
    Ok(GitFetchResult {
        remote: name.to_owned(),
    })
}

/// Pull: [`fetch`] then a **fast-forward-only** merge of the fetched
/// remote-tracking ref (`<remote>/<branch>`) into the current branch, reusing
/// [`merge`]'s ff logic. A diverged history yields [`GitOpError::NonFastForward`]
/// (no 3-way merge, no conflict markers).
///
/// `branch` defaults to the short name of the current HEAD branch when empty.
///
/// # Errors
///
/// - [`GitOpError::InvalidInput`] — the remote URL is SSH, or HEAD is detached
///   and no `branch` was supplied.
/// - [`GitOpError::NonFastForward`] — the merge is not a fast-forward.
/// - [`GitOpError::Libgit2`] — the fetch / ref resolution failed.
pub fn pull(
    net: &GitNetConfig,
    repo: &git2::Repository,
    remote_name: &str,
    branch: &str,
) -> Result<GitPullResult, GitOpError> {
    let remote = if remote_name.is_empty() {
        "origin"
    } else {
        remote_name
    };
    fetch(net, repo, remote)?;

    // Resolve the branch short name: explicit param, else current HEAD branch.
    let branch_name = if branch.is_empty() {
        let head = repo.head().map_err(|e| GitOpError::from_git2(&e))?;
        if !head.is_branch() {
            return Err(GitOpError::InvalidInput(
                "pull on a detached HEAD requires an explicit `branch`".into(),
            ));
        }
        head.shorthand()
            .map_err(|e| GitOpError::from_git2(&e))?
            .to_owned()
    } else {
        branch.to_owned()
    };

    // The fetched remote-tracking ref to fast-forward onto.
    let tracking = format!("refs/remotes/{remote}/{branch_name}");
    let merge_result = merge(repo, &tracking)?;
    Ok(GitPullResult {
        fast_forward: merge_result.fast_forward,
        oid: merge_result.oid,
    })
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

        // An ABSOLUTE repo param escapes too: `Path::join` replaces the base
        // with an absolute arg, so `<ws>.join("/abs/repo")` == `/abs/repo`,
        // which canonicalizes outside ws → strict Escape. Locks the guarantee
        // against a future join refactor.
        let abs = outside.path().to_str().unwrap().to_string();
        let err = expect_err(ws.path(), &abs, "absolute-path escape");
        assert!(
            matches!(err, GitOpError::Escape(_)),
            "absolute-path escape must produce GitOpError::Escape, got: {err:?}"
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

    /// Resolve the HEAD commit of `repo` (panics if unborn).
    fn head_commit(repo: &git2::Repository) -> GitCommitInfo {
        let commit = repo.head().unwrap().peel_to_commit().unwrap();
        commit_info(&commit)
    }

    #[test]
    fn add_then_commit_creates_head_commit_with_config_identity() {
        let dir = tempdir().unwrap();
        let (repo, _first, _second) = init_history(dir.path());
        // Set repo-local committer identity.
        {
            let mut cfg = repo.config().unwrap();
            cfg.set_str("user.name", "Cfg User").unwrap();
            cfg.set_str("user.email", "cfg@example.com").unwrap();
        }

        std::fs::write(dir.path().join("c.txt"), "gamma\n").unwrap();
        let added = add(&repo, &["c.txt".to_owned()]).unwrap();
        assert!(added.index_entries >= 1, "at least one path staged");

        let res = commit(&repo, "third").unwrap();
        let head = head_commit(&repo);
        assert_eq!(head.oid, res.oid, "returned oid is the new HEAD");
        assert_eq!(head.summary, "third");
        assert_eq!(
            head.author, "Cfg User <cfg@example.com>",
            "committer identity reflects repo config when set"
        );
    }

    #[test]
    fn commit_falls_back_to_default_identity_when_config_absent() {
        let dir = tempdir().unwrap();
        let (repo, _first, _second) = init_history(dir.path());
        // Ensure NO user identity is set in the repo config.
        {
            let mut cfg = repo.config().unwrap();
            let _ = cfg.remove("user.name");
            let _ = cfg.remove("user.email");
        }

        std::fs::write(dir.path().join("c.txt"), "gamma\n").unwrap();
        add(&repo, &["c.txt".to_owned()]).unwrap();
        let res = commit(&repo, "default-id").unwrap();

        let head = head_commit(&repo);
        assert_eq!(head.oid, res.oid);
        assert_eq!(
            head.author, "LingXi <noreply@lingxi>",
            "committer identity falls back to the fixed default when config is absent"
        );
    }

    #[test]
    fn add_empty_paths_stages_all() {
        let dir = tempdir().unwrap();
        let (repo, _first, _second) = init_history(dir.path());
        std::fs::write(dir.path().join("new1.txt"), "n1\n").unwrap();
        std::fs::write(dir.path().join("new2.txt"), "n2\n").unwrap();

        // Empty paths -> "*" -> stage everything.
        add(&repo, &[]).unwrap();
        let res = commit(&repo, "stage-all").unwrap();
        let (_info, d) = show(&repo, &res.oid).unwrap();
        assert!(d.patch.contains("new1.txt"), "new1 staged via *");
        assert!(d.patch.contains("new2.txt"), "new2 staged via *");
    }

    #[test]
    fn branch_create_makes_branch() {
        let dir = tempdir().unwrap();
        let (repo, _first, _second) = init_history(dir.path());

        branch_create(&repo, "feature").unwrap();
        let branches = branch_list(&repo).unwrap();
        assert!(
            branches.iter().any(|b| b.name == "feature" && !b.is_remote),
            "branch_create should add a local branch 'feature', got {branches:?}"
        );
    }

    #[test]
    fn checkout_switches_clean_worktree() {
        let dir = tempdir().unwrap();
        let (repo, _first, second) = init_history(dir.path());

        // Create a divergent branch "other" with its own commit.
        branch_create(&repo, "other").unwrap();
        checkout(&repo, "other").unwrap();
        std::fs::write(dir.path().join("other.txt"), "only-on-other\n").unwrap();
        add(&repo, &["other.txt".to_owned()]).unwrap();
        commit(&repo, "other-commit").unwrap();
        assert!(
            dir.path().join("other.txt").exists(),
            "other.txt present on 'other'"
        );

        // Back to the original branch (whatever the default name is): HEAD moves
        // to `second` and other.txt disappears from the worktree.
        // Find the default branch (the one that is `second`).
        let default = repo
            .branches(Some(git2::BranchType::Local))
            .unwrap()
            .filter_map(Result::ok)
            .find_map(|(b, _)| {
                let target = b.get().target();
                let name = b.name().ok().flatten().map(str::to_owned);
                match (target, name) {
                    (Some(t), Some(n)) if t == second && n != "other" => Some(n),
                    _ => None,
                }
            })
            .expect("a default branch pointing at `second`");

        checkout(&repo, &default).unwrap();
        let head = head_commit(&repo);
        assert_eq!(head.oid, second.to_string(), "HEAD switched to `second`");
        assert!(
            !dir.path().join("other.txt").exists(),
            "other.txt removed from worktree after switching away from 'other'"
        );
    }

    #[test]
    fn checkout_rejects_dirty_worktree() {
        let dir = tempdir().unwrap();
        let (repo, _first, _second) = init_history(dir.path());
        branch_create(&repo, "feature").unwrap();

        // Dirty the worktree: modify a tracked file without committing.
        std::fs::write(dir.path().join("a.txt"), "DIRTY-EDIT\n").unwrap();

        let err = match checkout(&repo, "feature") {
            Ok(_) => panic!("dirty checkout must be rejected"),
            Err(e) => e,
        };
        assert!(
            matches!(err, GitOpError::Dirty(_)),
            "dirty worktree must produce GitOpError::Dirty, got: {err:?}"
        );
        // No data loss: the uncommitted edit survives.
        let content = std::fs::read_to_string(dir.path().join("a.txt")).unwrap();
        assert_eq!(content, "DIRTY-EDIT\n", "uncommitted change preserved");
    }

    #[test]
    fn merge_fast_forward_advances() {
        let dir = tempdir().unwrap();
        let (repo, first, second) = init_history(dir.path());

        // Create a branch "behind" pointing at `first` (one commit behind HEAD).
        let first_commit = repo.find_commit(first).unwrap();
        repo.branch("behind", &first_commit, false).unwrap();

        // Switch to "behind" (clean worktree), then ff-merge the default branch's
        // tip `second`.
        checkout(&repo, "behind").unwrap();
        assert_eq!(head_commit(&repo).oid, first.to_string(), "on `first`");

        let res = merge(&repo, &second.to_string()).unwrap();
        assert!(res.fast_forward, "should be a fast-forward");
        assert_eq!(res.oid, second.to_string());
        assert_eq!(
            head_commit(&repo).oid,
            second.to_string(),
            "HEAD advanced to `second`"
        );
        // Worktree synced: b.txt (introduced in `second`) now exists.
        assert!(
            dir.path().join("b.txt").exists(),
            "worktree synced to second on ff"
        );
    }

    #[test]
    fn merge_non_ff_named_error() {
        let dir = tempdir().unwrap();
        let (repo, first, _second) = init_history(dir.path());

        // Branch "diverge" off `first`, give it a unique commit -> the two
        // histories diverge (neither is an ancestor of the other).
        let first_commit = repo.find_commit(first).unwrap();
        repo.branch("diverge", &first_commit, false).unwrap();
        checkout(&repo, "diverge").unwrap();
        std::fs::write(dir.path().join("d.txt"), "diverge\n").unwrap();
        add(&repo, &["d.txt".to_owned()]).unwrap();
        let diverge_tip = commit(&repo, "diverge-commit").unwrap();

        // Try to merge the default branch tip (`second`) -> non-ff.
        // Resolve `second` via the branch that still points at it.
        let other_tip = repo
            .branches(Some(git2::BranchType::Local))
            .unwrap()
            .filter_map(Result::ok)
            .find_map(|(b, _)| {
                let name = b.name().ok().flatten().map(str::to_owned)?;
                let target = b.get().target()?;
                (name != "diverge" && name != "behind").then_some(target)
            })
            .expect("default branch tip");

        let err = match merge(&repo, &other_tip.to_string()) {
            Ok(_) => panic!("diverged merge must be non-ff error"),
            Err(e) => e,
        };
        assert!(
            matches!(err, GitOpError::NonFastForward(_)),
            "diverged histories must produce GitOpError::NonFastForward, got: {err:?}"
        );
        // HEAD unchanged (still on the diverge tip), no conflict markers written.
        assert_eq!(
            head_commit(&repo).oid,
            diverge_tip.oid,
            "HEAD unchanged after refused non-ff merge"
        );
        let d_content = std::fs::read_to_string(dir.path().join("d.txt")).unwrap();
        assert!(
            !d_content.contains("<<<<<<<") && !d_content.contains(">>>>>>>"),
            "no conflict markers should be written on a refused non-ff merge"
        );
    }

    // ===== Network ops (Task 8) — file:// bare remotes, no real network =====

    /// Build a non-bare repo at `dir` with one commit (file `r.txt` = "v1\n")
    /// that can serve as a `file://` clone source, and return its `file://` URL
    /// plus the HEAD oid. A normal (non-bare) repo is a valid clone source.
    fn init_remote(dir: &Path) -> (String, git2::Oid) {
        let repo = git2::Repository::init(dir).unwrap();
        let sig = git2::Signature::now("Remote", "remote@example.com").unwrap();
        std::fs::write(dir.join("r.txt"), "v1\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("r.txt")).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let oid = repo
            .commit(Some("HEAD"), &sig, &sig, "remote-first", &tree, &[])
            .unwrap();
        let url = format!("file://{}", dir.canonicalize().unwrap().display());
        (url, oid)
    }

    /// Add a second commit (`r2.txt` = "v2\n") to the repo at `dir` and return
    /// the new HEAD oid. Used to advance a `file://` remote between fetches.
    fn advance_remote(dir: &Path) -> git2::Oid {
        let repo = git2::Repository::open(dir).unwrap();
        let sig = git2::Signature::now("Remote", "remote@example.com").unwrap();
        std::fs::write(dir.join("r2.txt"), "v2\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("r2.txt")).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let parent = repo.head().unwrap().peel_to_commit().unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "remote-second", &tree, &[&parent])
            .unwrap()
    }

    #[test]
    fn clone_from_file_remote_ok() {
        let remote_dir = tempdir().unwrap();
        let (url, head_oid) = init_remote(remote_dir.path());

        let ws = tempdir().unwrap();
        let net = GitNetConfig::default(); // no token / CA for file://
        let res = clone(&net, ws.path(), &url, "cloned").expect("file:// clone should succeed");

        assert_eq!(res.head, head_oid.to_string(), "cloned HEAD == remote HEAD");
        // The cloned working tree exists under the workspace with the file.
        let cloned = ws.path().join("cloned");
        assert!(cloned.join("r.txt").exists(), "cloned working file present");
        assert_eq!(
            std::fs::read_to_string(cloned.join("r.txt")).unwrap(),
            "v1\n"
        );
    }

    #[test]
    fn fetch_then_pull_ff_advances() {
        let remote_dir = tempdir().unwrap();
        let (url, _first) = init_remote(remote_dir.path());

        let ws = tempdir().unwrap();
        let net = GitNetConfig::default();
        clone(&net, ws.path(), &url, "cloned").unwrap();

        // Advance the remote with a new commit.
        let second = advance_remote(remote_dir.path());

        // Re-open the clone and fetch + pull (ff) -> local advances to `second`.
        let repo = open_repo(ws.path(), "cloned").unwrap();
        let f = fetch(&net, &repo, "origin").unwrap();
        assert_eq!(f.remote, "origin");

        let p = pull(&net, &repo, "origin", "").unwrap();
        assert!(p.fast_forward, "pull should be a fast-forward");
        assert_eq!(p.oid, second.to_string(), "local advanced to remote second");
        // Worktree synced: r2.txt now exists locally.
        assert!(
            ws.path().join("cloned").join("r2.txt").exists(),
            "ff pull synced the new file into the worktree"
        );
    }

    #[test]
    fn pull_non_ff_named_error() {
        let remote_dir = tempdir().unwrap();
        let (url, _first) = init_remote(remote_dir.path());

        let ws = tempdir().unwrap();
        let net = GitNetConfig::default();
        clone(&net, ws.path(), &url, "cloned").unwrap();

        // Advance the remote (so origin/<branch> is ahead) ...
        advance_remote(remote_dir.path());

        // ... and ALSO create a divergent local commit so the local branch is
        // not an ancestor of the remote tip -> non-ff.
        let repo = open_repo(ws.path(), "cloned").unwrap();
        std::fs::write(ws.path().join("cloned").join("local.txt"), "local\n").unwrap();
        add(&repo, &["local.txt".to_owned()]).unwrap();
        commit(&repo, "local-divergent").unwrap();

        let err = match pull(&net, &repo, "origin", "") {
            Ok(_) => panic!("diverged pull must be non-ff error"),
            Err(e) => e,
        };
        assert!(
            matches!(err, GitOpError::NonFastForward(_)),
            "diverged pull must produce GitOpError::NonFastForward, got: {err:?}"
        );
    }

    #[test]
    fn ssh_url_rejected() {
        let ws = tempdir().unwrap();
        let net = GitNetConfig::default();
        let err = match clone(&net, ws.path(), "git@github.com:x/y.git", "dest") {
            Ok(_) => panic!("ssh clone must be rejected"),
            Err(e) => e,
        };
        match &err {
            GitOpError::InvalidInput(msg) => {
                assert!(
                    msg.contains("ssh") && msg.contains("HTTPS"),
                    "ssh rejection should name ssh + HTTPS, got: {msg}"
                );
            }
            other => panic!("ssh clone must be InvalidInput, got: {other:?}"),
        }

        // ssh:// scheme is rejected too.
        assert!(matches!(
            clone(&net, ws.path(), "ssh://git@host/x.git", "dest"),
            Err(GitOpError::InvalidInput(_))
        ));
    }
}

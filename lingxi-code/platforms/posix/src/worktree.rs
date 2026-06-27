//! `git worktree`-backed [`WorktreeManager`] for desktop hosts.
//!
//! Shells out to the `git` CLI rooted at the configured repository root.
//! Worktrees live at `<repo_root>/.lingxi/worktrees/<flatten_slug(slug)>`
//! and use the branch-name prefix `worktree-` (NOT `lingxi/` or `claude/`).
//!
//! The branch prefix and path layout match claude-code's
//! `src/utils/worktree.ts` exactly — see plan M2-01 §"Critical 1:1 fidelity
//! items" for the rationale.
//!
//! Slug validation rules:
//! - Each `/`-separated segment is `[a-zA-Z0-9._-]+`.
//! - Total length 1..=64 chars.
//! - No empty segments.
//!
//! `/` is flattened to `+` for the on-disk directory name so the layout
//! stays flat. `+` is outside the allowlist so the mapping is injective.

use async_trait::async_trait;
use std::path::PathBuf;
use std::time::Duration;
use tokio::process::Command;
use traits::{
    WorktreeChangeSummary, WorktreeError, WorktreeHandle, WorktreeInfo, WorktreeManager,
};

/// Maximum allowed total length of a worktree slug.
///
/// Matches `MAX_WORKTREE_SLUG_LENGTH` in claude-code's TS reference at
/// `src/utils/worktree.ts`.
pub const MAX_WORKTREE_SLUG_LENGTH: usize = 64;

/// Validate a caller-supplied worktree slug.
///
/// Rules (mirrors claude-code's `src/utils/worktree.ts`):
/// - Total length 1..=64 chars.
/// - Each `/`-separated segment matches `^[a-zA-Z0-9._-]+$`.
/// - No empty segments (rejects `"/foo"`, `"foo/"`, `"a//b"`, `""`).
///
/// Returns [`WorktreeError::InvalidSlug`] with a human-readable detail when
/// any rule fails. The detail is suitable for direct surfacing in `/doctor`
/// or CLI error output.
pub fn validate_worktree_slug(slug: &str) -> Result<(), WorktreeError> {
    if slug.is_empty() {
        return Err(WorktreeError::InvalidSlug("slug is empty".into()));
    }
    if slug.len() > MAX_WORKTREE_SLUG_LENGTH {
        return Err(WorktreeError::InvalidSlug(format!(
            "slug exceeds {MAX_WORKTREE_SLUG_LENGTH} chars (got {})",
            slug.len()
        )));
    }
    for segment in slug.split('/') {
        if segment.is_empty() {
            return Err(WorktreeError::InvalidSlug(format!(
                "slug contains empty segment: {slug:?}"
            )));
        }
        for ch in segment.chars() {
            let allowed = ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-';
            if !allowed {
                return Err(WorktreeError::InvalidSlug(format!(
                    "slug contains invalid character {ch:?} in segment {segment:?}"
                )));
            }
        }
    }
    Ok(())
}

/// Parse `git worktree prune -v` stdout into pruned on-disk paths.
///
/// Each relevant line has the form `Removing worktrees/<name>: <reason>`.
/// We strip the `<name>` and resolve it against
/// `<repo_root>/.lingxi/worktrees/<name>` (where this codebase places its
/// worktrees per claude-code's layout). Lines that don't match the
/// expected prefix are silently skipped.
fn parse_prune_v_stdout(stdout: &str, repo_root: &std::path::Path) -> Vec<PathBuf> {
    stdout
        .lines()
        .filter_map(|line| {
            let rest = line.strip_prefix("Removing worktrees/")?;
            // <name>: <reason>
            let name = rest.split(':').next()?;
            if name.is_empty() {
                return None;
            }
            Some(repo_root.join(branding::DOT_DIR).join("worktrees").join(name))
        })
        .collect()
}

/// Count the non-blank lines of `git status --porcelain` stdout.
///
/// Byte-faithful to claude-code's
/// `count(status.stdout.split('\n'), l => l.trim() !== '')`
/// (`src/tools/ExitWorktreeTool/ExitWorktreeTool.ts:92`): split on `'\n'`
/// (NOT `lines()`, which also splits on `\r\n` and drops a trailing newline
/// differently), then count entries whose trimmed value is non-empty. A
/// trailing newline yields a final empty entry that is correctly excluded.
fn count_porcelain_changed_files(stdout: &str) -> usize {
    stdout.split('\n').filter(|l| !l.trim().is_empty()).count()
}

/// Flatten a `/`-separated slug into a single filesystem-friendly name.
///
/// Replaces every `/` with `+`. Because `+` is outside the allowed slug
/// character set (see [`validate_worktree_slug`]), this mapping is
/// injective: no two distinct valid slugs flatten to the same string.
///
/// Examples:
/// - `flatten_slug("user/feature")` → `"user+feature"`
/// - `flatten_slug("a/b/c")` → `"a+b+c"`
/// - `flatten_slug("plain")` → `"plain"`
#[must_use]
pub fn flatten_slug(slug: &str) -> String {
    slug.replace('/', "+")
}

/// Production [`WorktreeManager`] using the `git worktree` CLI.
pub struct PosixWorktreeManager {
    /// Absolute path to the main repository working copy.
    repo_root: PathBuf,
}

impl PosixWorktreeManager {
    /// Build a new `PosixWorktreeManager` rooted at `repo_root`.
    ///
    /// New worktrees are created at
    /// `<repo_root>/.lingxi/worktrees/<flatten_slug(slug)>`. The layout is
    /// fixed (matches claude-code) — there is no `worktree_base` knob.
    #[must_use]
    pub fn new(repo_root: PathBuf) -> Self {
        Self { repo_root }
    }
}

#[async_trait]
impl WorktreeManager for PosixWorktreeManager {
    async fn create_worktree(
        &self,
        slug: &str,
        base_branch: Option<&str>,
        copy_includes: &[PathBuf],
    ) -> Result<WorktreeHandle, WorktreeError> {
        validate_worktree_slug(slug)?;
        let flat = flatten_slug(slug);
        let branch_name = format!("worktree-{flat}");
        let worktree_path = self
            .repo_root
            .join(branding::DOT_DIR)
            .join("worktrees")
            .join(&flat);

        // git worktree add creates the leaf; the `.lingxi/worktrees/` parent
        // may not exist yet.
        if let Some(parent) = worktree_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| WorktreeError::Io(e.to_string()))?;
        }

        let mut cmd = Command::new("git");
        cmd.current_dir(&self.repo_root);
        cmd.arg("worktree")
            .arg("add")
            .arg("-b")
            .arg(&branch_name)
            .arg(&worktree_path);
        if let Some(base) = base_branch {
            cmd.arg(base);
        }

        let output = cmd
            .output()
            .await
            .map_err(|e| WorktreeError::Io(e.to_string()))?;
        if !output.status.success() {
            return Err(WorktreeError::Git(
                String::from_utf8_lossy(&output.stderr).into_owned(),
            ));
        }

        // Best-effort copy of caller-supplied include paths. Missing sources
        // are silently skipped — the intent is to ferry across e.g. `.env`
        // files that aren't tracked by git.
        for rel in copy_includes {
            let src = self.repo_root.join(rel);
            if !tokio::fs::try_exists(&src)
                .await
                .map_err(|e| WorktreeError::Io(e.to_string()))?
            {
                continue;
            }
            let dst = worktree_path.join(rel);
            if let Some(parent) = dst.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|e| WorktreeError::Io(e.to_string()))?;
            }
            tokio::fs::copy(&src, &dst)
                .await
                .map_err(|e| WorktreeError::Io(e.to_string()))?;
        }

        // Capture the worktree's initial HEAD — claude-code's
        // `originalHeadCommit` (the commit `git worktree add` checked out).
        // `worktree_change_summary` counts ahead-commits as
        // `rev-list --count <base>..HEAD`, so without this baseline a
        // clean-but-committed worktree would report `commits: 0` and be
        // auto-removed. Best-effort: a failure leaves `base_commit: None`,
        // which yields `commits: 0` (claude's `if (!headCommit)`).
        let base_commit = Command::new("git")
            .arg("-C")
            .arg(&worktree_path)
            .arg("rev-parse")
            .arg("HEAD")
            .output()
            .await
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty());

        Ok(WorktreeHandle {
            path: worktree_path,
            branch_name,
            base_commit,
        })
    }

    async fn remove_worktree(&self, handle: &WorktreeHandle) -> Result<(), WorktreeError> {
        let output = Command::new("git")
            .current_dir(&self.repo_root)
            .arg("worktree")
            .arg("remove")
            .arg(&handle.path)
            .output()
            .await
            .map_err(|e| WorktreeError::Io(e.to_string()))?;
        if !output.status.success() {
            return Err(WorktreeError::Git(
                String::from_utf8_lossy(&output.stderr).into_owned(),
            ));
        }
        Ok(())
    }

    async fn list_worktrees(&self) -> Result<Vec<WorktreeInfo>, WorktreeError> {
        let output = Command::new("git")
            .current_dir(&self.repo_root)
            .arg("worktree")
            .arg("list")
            .arg("--porcelain")
            .output()
            .await
            .map_err(|e| WorktreeError::Io(e.to_string()))?;
        if !output.status.success() {
            return Err(WorktreeError::Git(
                String::from_utf8_lossy(&output.stderr).into_owned(),
            ));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut out = Vec::new();
        let mut current: Option<WorktreeInfo> = None;
        for line in stdout.lines() {
            if let Some(rest) = line.strip_prefix("worktree ") {
                if let Some(c) = current.take() {
                    out.push(c);
                }
                current = Some(WorktreeInfo {
                    path: PathBuf::from(rest),
                    branch: String::new(),
                    created_at: std::time::SystemTime::now(),
                });
            } else if let Some(rest) = line.strip_prefix("branch refs/heads/") {
                if let Some(c) = current.as_mut() {
                    c.branch = rest.to_string();
                }
            }
        }
        if let Some(c) = current {
            out.push(c);
        }
        Ok(out)
    }

    async fn cleanup_stale(&self, _max_age: Duration) -> Result<Vec<PathBuf>, WorktreeError> {
        // `max_age` is currently unused: `git worktree prune` consults its
        // own `gc.worktreePruneExpire` setting. claude-code does not expose
        // a per-call override either. Explicit age filtering is M2-followup.
        let output = Command::new("git")
            .current_dir(&self.repo_root)
            .arg("worktree")
            .arg("prune")
            .arg("-v")
            .output()
            .await
            .map_err(|e| WorktreeError::Io(e.to_string()))?;
        if !output.status.success() {
            return Err(WorktreeError::Git(
                String::from_utf8_lossy(&output.stderr).into_owned(),
            ));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(parse_prune_v_stdout(&stdout, &self.repo_root))
    }

    fn is_supported(&self) -> bool {
        true
    }

    async fn worktree_change_summary(
        &self,
        handle: &WorktreeHandle,
    ) -> Result<Option<WorktreeChangeSummary>, WorktreeError> {
        // Mirror claude-code `countWorktreeChanges`
        // (ExitWorktreeTool.ts:79-113): `git status --porcelain` for the
        // working-tree dirty count. Fail-closed (`Ok(None)`) on a non-zero
        // exit — a lock file, corrupt index, or non-git path. A spawn failure
        // (no git binary) is a hard `Git` error, matching the rest of this
        // impl's error surface.
        let status = Command::new("git")
            .arg("-C")
            .arg(&handle.path)
            .arg("status")
            .arg("--porcelain")
            .output()
            .await
            .map_err(|e| WorktreeError::Git(e.to_string()))?;
        if !status.status.success() {
            return Ok(None);
        }
        let stdout = String::from_utf8_lossy(&status.stdout);
        let changed_files = count_porcelain_changed_files(&stdout);

        // Ahead-commit count, mirroring claude-code `countWorktreeChanges`
        // (ExitWorktreeTool.ts): `git rev-list --count <base>..HEAD` where
        // `<base>` is the worktree's `originalHeadCommit` captured at creation
        // ([`WorktreeHandle::base_commit`]). When the handle carries no baseline
        // (`None`) — or the rev-list spawn / parse fails — the count is `0`,
        // exactly claude's `if (!headCommit) commitsAhead = 0` /
        // `if (o.code !== 0) commitsAhead = 0`. The dirty working-tree count
        // already succeeded above, so a rev-list failure degrades only the
        // commit half rather than discarding the whole summary.
        let commits = match &handle.base_commit {
            Some(base) => Command::new("git")
                .arg("-C")
                .arg(&handle.path)
                .arg("rev-list")
                .arg("--count")
                .arg(format!("{base}..HEAD"))
                .output()
                .await
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| {
                    String::from_utf8_lossy(&o.stdout)
                        .trim()
                        .parse::<usize>()
                        .ok()
                })
                .unwrap_or(0),
            None => 0,
        };

        Ok(Some(WorktreeChangeSummary {
            changed_files,
            commits,
        }))
    }
}

#[cfg(test)]
mod slug_tests {
    use super::*;

    #[test]
    fn validate_accepts_legal_slugs() {
        for ok in [
            "feature",
            "user",
            "v1.2.3",
            "with-dash",
            "with_under",
            "with.dot",
            "user/feature",
            "topic/area/sub",
        ] {
            assert!(validate_worktree_slug(ok).is_ok(), "expected ok: {ok:?}");
        }
    }

    #[test]
    fn validate_rejects_bad_chars_and_segments() {
        for bad in ["a*b", "a b", "a:b", "a+b", "", "/foo", "foo/", "a//b"] {
            assert!(
                matches!(
                    validate_worktree_slug(bad),
                    Err(WorktreeError::InvalidSlug(_))
                ),
                "expected err: {bad:?}"
            );
        }
    }

    #[test]
    fn validate_rejects_over_64_chars() {
        assert!(matches!(
            validate_worktree_slug(&"a".repeat(65)),
            Err(WorktreeError::InvalidSlug(_)),
        ));
        assert!(validate_worktree_slug(&"a".repeat(64)).is_ok());
    }

    #[test]
    fn flatten_replaces_slashes_with_plus() {
        assert_eq!(flatten_slug("user/feature"), "user+feature");
        assert_eq!(flatten_slug("a/b/c"), "a+b+c");
        assert_eq!(flatten_slug("plain"), "plain");
    }

    #[test]
    fn flatten_is_injective_for_valid_slugs() {
        // The key property: no two valid slugs flatten to the same string,
        // because `+` is outside the allowed character set. `a+b` is not a
        // valid slug so it cannot collide with `a/b`'s flattened form.
        assert_ne!(flatten_slug("a/b"), flatten_slug("ab"));
        assert_ne!(flatten_slug("a/b/c"), flatten_slug("ab/c"));
        assert!(validate_worktree_slug("a+b").is_err());
    }
}

#[cfg(test)]
mod create_tests {
    use super::*;
    use tempfile::TempDir;
    use tokio::process::Command;
    use traits::WorktreeManager;

    /// Initialize a fresh git repo with one commit so worktree commands have
    /// something to branch from.
    async fn init_repo(dir: &std::path::Path) {
        async fn git(dir: &std::path::Path, args: &[&str]) {
            let mut c = Command::new("git");
            c.current_dir(dir);
            for a in args {
                c.arg(a);
            }
            assert!(
                c.output().await.unwrap().status.success(),
                "git {args:?} failed"
            );
        }
        git(dir, &["init", "-q", "-b", "main"]).await;
        git(dir, &["config", "user.email", "ci@test"]).await;
        git(dir, &["config", "user.name", "ci"]).await;
        tokio::fs::write(dir.join("seed.txt"), "seed")
            .await
            .unwrap();
        git(dir, &["add", "seed.txt"]).await;
        git(dir, &["commit", "-qm", "seed"]).await;
    }

    #[tokio::test]
    async fn create_uses_worktree_dash_prefix_and_dot_claude_layout() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().to_path_buf();
        init_repo(&repo).await;
        let handle = PosixWorktreeManager::new(repo.clone())
            .create_worktree("user/feature", None, &[])
            .await
            .unwrap();
        assert_eq!(handle.branch_name, "worktree-user+feature");
        assert_eq!(handle.path, repo.join(".lingxi/worktrees/user+feature"));
        assert!(handle.path.exists());
    }

    #[tokio::test]
    async fn create_rejects_invalid_slug_before_running_git() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().to_path_buf();
        init_repo(&repo).await;
        let err = PosixWorktreeManager::new(repo)
            .create_worktree("a*b", None, &[])
            .await
            .unwrap_err();
        assert!(matches!(err, WorktreeError::InvalidSlug(_)));
    }

    #[tokio::test]
    async fn create_copies_includes_when_source_exists() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().to_path_buf();
        init_repo(&repo).await;
        tokio::fs::write(repo.join(".env"), "API_KEY=secret")
            .await
            .unwrap();
        let handle = PosixWorktreeManager::new(repo.clone())
            .create_worktree("user/feature", None, &[std::path::PathBuf::from(".env")])
            .await
            .unwrap();
        let copied = tokio::fs::read_to_string(handle.path.join(".env"))
            .await
            .unwrap();
        assert_eq!(copied, "API_KEY=secret");
    }

    #[tokio::test]
    async fn create_skips_missing_copy_includes() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().to_path_buf();
        init_repo(&repo).await;
        let res = PosixWorktreeManager::new(repo)
            .create_worktree(
                "feat",
                None,
                &[std::path::PathBuf::from("does-not-exist.txt")],
            )
            .await;
        assert!(res.is_ok());
    }
}

#[cfg(test)]
mod cleanup_tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn parse_prune_output_extracts_names() {
        let stdout = "\
Removing worktrees/user+feature: gitdir file points to non-existent location
Removing worktrees/topic+area: gitdir file points to non-existent location
";
        let paths = parse_prune_v_stdout(stdout, &PathBuf::from("/tmp/repo"));
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/tmp/repo/.lingxi/worktrees/user+feature"),
                PathBuf::from("/tmp/repo/.lingxi/worktrees/topic+area"),
            ]
        );
    }

    #[test]
    fn parse_prune_output_ignores_unrelated_lines() {
        let stdout = "some random noise\nRemoving worktrees/ok: stale\nnot a removing line\n";
        let paths = parse_prune_v_stdout(stdout, &PathBuf::from("/r"));
        assert_eq!(paths, vec![PathBuf::from("/r/.lingxi/worktrees/ok")]);
    }

    #[test]
    fn parse_prune_output_empty_stdout() {
        assert!(parse_prune_v_stdout("", &PathBuf::from("/r")).is_empty());
    }
}

#[cfg(test)]
mod change_summary_tests {
    use super::*;
    use tempfile::TempDir;
    use tokio::process::Command;
    use traits::WorktreeManager;

    #[test]
    fn count_porcelain_clean_is_zero() {
        assert_eq!(count_porcelain_changed_files(""), 0);
        // Even a lone trailing newline (git's empty-status output) is zero.
        assert_eq!(count_porcelain_changed_files("\n"), 0);
    }

    #[test]
    fn count_porcelain_counts_non_blank_lines() {
        // Two changed entries with a trailing newline → 2 (the trailing
        // empty split entry is excluded, matching the TS `trim() !== ''`).
        let stdout = " M src/a.rs\n?? new.txt\n";
        assert_eq!(count_porcelain_changed_files(stdout), 2);
    }

    #[test]
    fn count_porcelain_ignores_whitespace_only_lines() {
        let stdout = " M a\n   \n A b\n";
        assert_eq!(count_porcelain_changed_files(stdout), 2);
    }

    async fn git(dir: &std::path::Path, args: &[&str]) {
        let mut c = Command::new("git");
        c.current_dir(dir);
        for a in args {
            c.arg(a);
        }
        assert!(
            c.output().await.unwrap().status.success(),
            "git {args:?} failed"
        );
    }

    /// Deterministic git repo: one commit, then a controllable dirty state.
    async fn init_repo(dir: &std::path::Path) {
        git(dir, &["init", "-q", "-b", "main"]).await;
        git(dir, &["config", "user.email", "ci@test"]).await;
        git(dir, &["config", "user.name", "ci"]).await;
        tokio::fs::write(dir.join("seed.txt"), "seed")
            .await
            .unwrap();
        git(dir, &["add", "seed.txt"]).await;
        git(dir, &["commit", "-qm", "seed"]).await;
    }

    #[tokio::test]
    async fn change_summary_clean_repo_is_zero() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().to_path_buf();
        init_repo(&repo).await;
        let handle = WorktreeHandle {
            path: repo.clone(),
            branch_name: "main".into(),
            base_commit: None,
        };
        let summary = PosixWorktreeManager::new(repo)
            .worktree_change_summary(&handle)
            .await
            .unwrap()
            .expect("git status succeeds → Some");
        assert_eq!(summary.changed_files, 0);
        assert_eq!(summary.commits, 0);
        assert!(!summary.is_dirty());
    }

    #[tokio::test]
    async fn change_summary_counts_uncommitted_files() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().to_path_buf();
        init_repo(&repo).await;
        // One tracked-file modification + one untracked file = 2 porcelain
        // lines.
        tokio::fs::write(repo.join("seed.txt"), "changed")
            .await
            .unwrap();
        tokio::fs::write(repo.join("untracked.txt"), "x")
            .await
            .unwrap();
        let handle = WorktreeHandle {
            path: repo.clone(),
            branch_name: "main".into(),
            base_commit: None,
        };
        let summary = PosixWorktreeManager::new(repo)
            .worktree_change_summary(&handle)
            .await
            .unwrap()
            .expect("git status succeeds → Some");
        assert_eq!(summary.changed_files, 2);
        assert!(summary.is_dirty());
        assert_eq!(
            summary.changed_files_phrase(),
            Some("2 uncommitted files".to_string())
        );
    }

    #[tokio::test]
    async fn change_summary_non_git_path_fails_closed() {
        // A directory that is not a git repo → git status exits non-zero →
        // Ok(None) (fail-closed "unknown").
        let tmp = TempDir::new().unwrap();
        let not_git = tmp.path().to_path_buf();
        let handle = WorktreeHandle {
            path: not_git.clone(),
            branch_name: "x".into(),
            base_commit: None,
        };
        let summary = PosixWorktreeManager::new(not_git)
            .worktree_change_summary(&handle)
            .await
            .unwrap();
        assert_eq!(summary, None, "non-git path must fail-closed to None");
    }

    #[tokio::test]
    async fn change_summary_counts_ahead_commits_with_clean_tree() {
        // The data-loss case: a CLEAN working tree (no porcelain lines) that
        // carries commits ahead of its base must still be reported dirty so the
        // worktree is KEPT, not auto-removed. Mirrors claude-code's keep half:
        // `commitsAhead = rev-list --count <originalHeadCommit>..HEAD`.
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().to_path_buf();
        init_repo(&repo).await;
        // Baseline = HEAD right after the seed commit.
        let base = String::from_utf8(
            Command::new("git")
                .current_dir(&repo)
                .args(["rev-parse", "HEAD"])
                .output()
                .await
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();
        // A second commit, leaving the working tree CLEAN.
        tokio::fs::write(repo.join("seed.txt"), "v2").await.unwrap();
        git(&repo, &["commit", "-qam", "v2"]).await;

        let handle = WorktreeHandle {
            path: repo.clone(),
            branch_name: "main".into(),
            base_commit: Some(base),
        };
        let summary = PosixWorktreeManager::new(repo)
            .worktree_change_summary(&handle)
            .await
            .unwrap()
            .expect("git status succeeds → Some");
        assert_eq!(summary.changed_files, 0, "working tree is clean");
        assert_eq!(summary.commits, 1, "one commit ahead of base");
        assert!(
            summary.is_dirty(),
            "clean-but-committed worktree must be kept (is_dirty via commits)"
        );
    }
}

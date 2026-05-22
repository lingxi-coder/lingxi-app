//! `git worktree`-backed [`WorktreeManager`] for Windows hosts.
//!
//! Shells out to the `git` CLI rooted at the configured repository root and
//! creates worktrees under a configured base directory. The branch name
//! follows the `lingxi/<slug>` convention.

use async_trait::async_trait;
use lingxi_traits::{WorktreeError, WorktreeHandle, WorktreeInfo, WorktreeManager};
use std::path::PathBuf;
use std::time::Duration;
use tokio::process::Command;

/// Production [`WorktreeManager`] using the `git worktree` CLI.
pub struct WindowsWorktreeManager {
    repo_root: PathBuf,
    worktree_base: PathBuf,
}

impl WindowsWorktreeManager {
    /// Build a new `WindowsWorktreeManager`.
    ///
    /// `repo_root` is the path to the main repository working copy. New
    /// worktrees are created as siblings inside `worktree_base`.
    #[must_use]
    pub fn new(repo_root: PathBuf, worktree_base: PathBuf) -> Self {
        Self {
            repo_root,
            worktree_base,
        }
    }
}

#[async_trait]
impl WorktreeManager for WindowsWorktreeManager {
    async fn create_worktree(
        &self,
        slug: &str,
        base_branch: Option<&str>,
        _copy_includes: &[PathBuf],
    ) -> Result<WorktreeHandle, WorktreeError> {
        let branch_name = format!("lingxi/{slug}");
        let worktree_path = self.worktree_base.join(slug);

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
        Ok(WorktreeHandle {
            path: worktree_path,
            branch_name,
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
        // TODO(M2-followup): parse pruned paths from stdout.
        Ok(Vec::new())
    }

    fn is_supported(&self) -> bool {
        true
    }
}

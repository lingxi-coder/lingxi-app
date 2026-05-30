//! Stub [`WorktreeManager`] — real `git worktree` shell-out lands in Plan 17.

use async_trait::async_trait;
use std::path::PathBuf;
use std::time::Duration;
use traits::{WorktreeError, WorktreeHandle, WorktreeInfo, WorktreeManager};

/// Stub worktree manager.
#[derive(Default)]
pub struct PosixWorktree;

impl PosixWorktree {
    /// Construct.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl WorktreeManager for PosixWorktree {
    async fn create_worktree(
        &self,
        _slug: &str,
        _base_branch: Option<&str>,
        _copy_includes: &[PathBuf],
    ) -> Result<WorktreeHandle, WorktreeError> {
        Err(WorktreeError::Unsupported)
    }

    async fn remove_worktree(&self, _handle: &WorktreeHandle) -> Result<(), WorktreeError> {
        Ok(())
    }

    async fn list_worktrees(&self) -> Result<Vec<WorktreeInfo>, WorktreeError> {
        Ok(vec![])
    }

    async fn cleanup_stale(&self, _max_age: Duration) -> Result<Vec<PathBuf>, WorktreeError> {
        Ok(vec![])
    }

    fn is_supported(&self) -> bool {
        false
    }
}

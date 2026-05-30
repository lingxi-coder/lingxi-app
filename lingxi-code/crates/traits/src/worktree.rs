//! Git worktree management abstraction.
//!
//! Engine code receives an `Arc<dyn WorktreeManager>` so subagents that need
//! isolated working copies (e.g. parallel coding tasks) can create one without
//! the engine depending on git directly. Platform crates provide concrete
//! implementations; on platforms without git the manager reports
//! [`is_supported`](WorktreeManager::is_supported) as `false` and subagents
//! degrade gracefully — see [`crate::worktree::WorktreeError::Unsupported`].
//!
//! See spec §10.7 (Worktree degradation policy).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;
use thiserror::Error;

/// Manages disposable git worktrees for subagents.
///
/// Implementations are platform-specific: a POSIX implementation may shell out
/// to `git worktree`, a Windows implementation may use libgit2, and platforms
/// without git support can return [`WorktreeError::Unsupported`] from every
/// method.
#[async_trait]
pub trait WorktreeManager: Send + Sync {
    /// Create a new worktree for the given `slug`, optionally branched from
    /// `base_branch`. `copy_includes` lists untracked paths (relative to the
    /// repository root) that should be copied into the worktree.
    async fn create_worktree(
        &self,
        slug: &str,
        base_branch: Option<&str>,
        copy_includes: &[PathBuf],
    ) -> Result<WorktreeHandle, WorktreeError>;

    /// Remove the worktree identified by `handle`. Implementations should be
    /// idempotent so callers can safely retry on transient errors.
    async fn remove_worktree(&self, handle: &WorktreeHandle) -> Result<(), WorktreeError>;

    /// Enumerate all worktrees currently managed.
    async fn list_worktrees(&self) -> Result<Vec<WorktreeInfo>, WorktreeError>;

    /// Garbage-collect worktrees older than `max_age`, returning the paths
    /// that were removed.
    async fn cleanup_stale(&self, max_age: Duration) -> Result<Vec<PathBuf>, WorktreeError>;

    /// `true` if the host platform supports worktrees. Engine code checks this
    /// before calling [`Self::create_worktree`] so that subagents marked as
    /// [`WorktreeRequirement::Optional`](crate::worktree::WorktreeError) can
    /// degrade to in-place execution.
    fn is_supported(&self) -> bool;
}

/// Stable handle to a worktree created by [`WorktreeManager::create_worktree`].
///
/// The handle is serializable so it can be embedded in snapshots and survive
/// process restarts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorktreeHandle {
    /// Absolute filesystem path to the worktree root.
    pub path: PathBuf,
    /// Git branch name checked out inside the worktree.
    pub branch_name: String,
}

/// Metadata for a worktree returned by [`WorktreeManager::list_worktrees`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorktreeInfo {
    /// Absolute filesystem path to the worktree root.
    pub path: PathBuf,
    /// Git branch name checked out inside the worktree.
    pub branch: String,
    /// Timestamp when the worktree was created.
    pub created_at: std::time::SystemTime,
}

/// Failure modes for [`WorktreeManager`] calls.
#[derive(Debug, Clone, Error)]
pub enum WorktreeError {
    /// Worktrees are not available on this platform (e.g. no git binary).
    #[error("worktree not supported on this platform")]
    Unsupported,
    /// Caller-supplied slug failed validation (bad char, too long, empty
    /// segment, ...). Validation rules are platform-agnostic — see
    /// `validate_worktree_slug` in the platform crates.
    #[error("invalid slug: {0}")]
    InvalidSlug(String),
    /// Underlying git invocation failed with the embedded message.
    #[error("git error: {0}")]
    Git(String),
    /// Filesystem error (permissions, disk full, ...).
    #[error("io error: {0}")]
    Io(String),
}

#[cfg(test)]
mod m2_01_tests {
    use super::*;

    #[test]
    fn invalid_slug_carries_message() {
        let e = WorktreeError::InvalidSlug("contains '*'".into());
        let s = format!("{e}");
        assert!(s.contains("invalid slug"));
        assert!(s.contains("contains '*'"));
    }
}

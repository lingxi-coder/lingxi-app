//! `TeamSpawnSeam` — narrow trait giving the coordinator's `TeamCreate` /
//! `TeamDelete` tools a typed way to start and stop a real `InProcessTeammate`
//! task WITHOUT a `coordinator` → `lingxi-tasks` dependency cycle.
//!
//! Mirrors the [`crate::task_registry::TaskRegistryHandle`] decoupling pattern:
//! the abstract seam lives here in `traits`; the concrete impl lives in
//! `lingxi-tasks` (lands in T04, calling the real `TaskRegistry::spawn`). Tests
//! inject an in-memory fake.
//!
//! The existing `TaskRegistryHandle::create` cannot carry the worker
//! `agent_id` / `name` (it builds a placeholder with a nil `AgentId` and an
//! empty name), so the coordinator needs this dedicated seam to spawn a
//! teammate keyed on the worker identity. `spawn_teammate` returns the
//! handler-generated `task_id`, which the coordinator reconciles back onto the
//! `WorkerAgent`.

use async_trait::async_trait;
use protocol::AgentId;
use thiserror::Error;

/// Failure modes for [`TeamSpawnSeam`] operations.
#[derive(Debug, Error)]
pub enum TeamSpawnError {
    /// No handler is registered for the teammate task type.
    #[error("TeamSpawn: unsupported task type: {0}")]
    Unsupported(String),
    /// The backing task could not be found (e.g. on kill of an unknown id).
    #[error("TeamSpawn: not found: {0}")]
    NotFound(String),
    /// Any other internal failure surfaced from the task registry.
    #[error("TeamSpawn: internal error: {0}")]
    Internal(String),
}

/// Spawn/kill seam for coordinator-managed teammate tasks.
///
/// Object-safe so the coordinator tools can hold an
/// `Arc<dyn TeamSpawnSeam>` and tests can inject a fake.
#[async_trait]
pub trait TeamSpawnSeam: Send + Sync {
    /// Start a real teammate task for `agent_id`, returning the
    /// handler-generated `task_id` (distinct from the worker's `AgentId`).
    async fn spawn_teammate(
        &self,
        agent_id: AgentId,
        name: String,
        description: String,
    ) -> Result<String, TeamSpawnError>;

    /// Kill the teammate task identified by its handler-generated `task_id`.
    async fn kill(&self, task_id: &str) -> Result<(), TeamSpawnError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn trait_is_object_safe() {
        let _: Option<Arc<dyn TeamSpawnSeam>> = None;
    }
}

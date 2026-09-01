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
    /// The teammate task has reached a terminal status and can no longer accept
    /// messages — the persistent runner dropped its receiver (killed / exited).
    /// The mailbox→runner pump treats this as a definitive "stop" signal and
    /// exits its loop, since the teammate will never come back (Rust teammates
    /// are persistent and only leave the loop on kill — there is no
    /// stopped-but-resumable state, so no auto-resume; see the pump docs).
    #[error("TeamSpawn: teammate terminated — cannot accept messages")]
    Terminated,
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
    ///
    /// `name` is the teammate's DISPLAY name (claude-code
    /// `TeammateContext.agentName`); `team_name` is the coordinator team it
    /// belongs to (`TeammateContext.teamName`). Both are threaded into the
    /// teammate's `SubagentContext` so its dispatched tools see the swarm
    /// identity (`getAgentName()` / `getTeammateContext()?.teamName`), which the
    /// swarm-only `TaskUpdate` side-effects key on.
    async fn spawn_teammate(
        &self,
        agent_id: AgentId,
        name: String,
        team_name: String,
        description: String,
    ) -> Result<String, TeamSpawnError>;

    /// Kill the teammate task identified by its handler-generated `task_id`.
    async fn kill(&self, task_id: &str) -> Result<(), TeamSpawnError>;

    /// Inject a message into a running teammate's turn loop — the Rust analogue
    /// of claude-code's `injectUserMessageToTeammate` (the in-process teammate
    /// receives it as a `UserMessage` and runs the next turn-set).
    ///
    /// Default: unsupported (returns [`TeamSpawnError::Unsupported`]) so existing
    /// fakes/impls that don't drive a live teammate stay correct. The production
    /// `TaskRegistry` overrides this to route to the task's
    /// [`TaskHandler::send_message`], so a coordinator `SendMessage` actually
    /// reaches the teammate's runner.
    ///
    /// A gone/terminated teammate maps to [`TeamSpawnError::Terminated`] — the
    /// mailbox→runner pump recognises that as its "stop" signal.
    async fn send_message(&self, task_id: &str, message: String) -> Result<(), TeamSpawnError> {
        let _ = (task_id, message);
        Err(TeamSpawnError::Unsupported(
            "TeamSpawnSeam::send_message not supported by this implementation".to_string(),
        ))
    }

    /// Report whether the teammate task `task_id` is still alive — i.e. running
    /// or resting (non-terminal) and thus still able to receive messages.
    ///
    /// The mailbox→runner pump polls this on each park timeout so it can EXIT —
    /// and let its mailbox be unregistered — once the teammate reaches a
    /// terminal state, EVEN WHEN no message ever arrives to surface the
    /// [`TeamSpawnError::Terminated`] via [`Self::send_message`]. Without it a
    /// terminated background agent leaks a task that re-parks on the timeout
    /// forever while a stale route silently blackholes later `SendMessage`s into
    /// an undrained inbox.
    ///
    /// Default: `true` (conservatively assume alive) so existing fakes/impls
    /// that don't track task lifecycle keep pumping unchanged. The production
    /// `TaskRegistry` overrides this to consult its terminal-status state.
    async fn is_alive(&self, task_id: &str) -> bool {
        let _ = task_id;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn trait_is_object_safe() {
        let _: Option<Arc<dyn TeamSpawnSeam>> = None;
    }

    /// A minimal seam that implements ONLY the two required methods and inherits
    /// the defaulted `send_message`, proving the default body is reachable and
    /// returns the unsupported error.
    struct DefaultOnlySeam;

    #[async_trait]
    impl TeamSpawnSeam for DefaultOnlySeam {
        async fn spawn_teammate(
            &self,
            _agent_id: AgentId,
            _name: String,
            _team_name: String,
            _description: String,
        ) -> Result<String, TeamSpawnError> {
            Ok(String::new())
        }
        async fn kill(&self, _task_id: &str) -> Result<(), TeamSpawnError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn send_message_default_is_unsupported() {
        let seam = DefaultOnlySeam;
        let err = seam
            .send_message("t-1", "hello".to_string())
            .await
            .expect_err("the defaulted send_message must return an error");
        assert!(
            matches!(err, TeamSpawnError::Unsupported(_)),
            "default send_message returns Unsupported; got {err:?}"
        );
    }
}

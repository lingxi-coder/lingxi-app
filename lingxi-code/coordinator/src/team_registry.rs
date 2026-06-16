//! Coordinator-side registry of teammate workers.
//!
//! Tracks each worker's metadata and status. The mailbox router (owned
//! by the registry) handles message delivery between coordinator and
//! teammates.

use crate::mailbox::{MailboxRouter, TeammateMailbox};
use protocol::AgentId;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::RwLock;

/// One spawned worker tracked by the [`TeamRegistry`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerAgent {
    /// Worker agent ID.
    pub agent_id: AgentId,
    /// Agent-type string (e.g. "explorer", "writer").
    pub agent_type: String,
    /// Display name.
    pub name: String,
    /// ID of the spawning coordinator.
    pub parent_id: Option<AgentId>,
    /// Current status.
    pub status: WorkerStatus,
    /// Task ID associated with this worker.
    pub task_id: String,
    /// Spawn timestamp.
    pub spawned_at: SystemTime,
    /// Last-seen activity timestamp.
    pub last_active_at: SystemTime,
}

/// Lifecycle status of a worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkerStatus {
    /// Idle; no work in flight.
    Idle,
    /// Actively working — `activity` is a free-form description.
    Working {
        /// Human-readable activity.
        activity: String,
    },
    /// Blocked waiting for an inbound message.
    AwaitingMessage,
    /// Finished its task.
    Completed,
    /// Failed with `error`.
    Failed {
        /// Error message.
        error: String,
    },
    /// Killed by the coordinator or user.
    Killed,
}

/// Registry of teammate workers owned by a coordinator session.
pub struct TeamRegistry {
    workers: RwLock<HashMap<AgentId, WorkerAgent>>,
    /// The coordinator agent's ID (parent of all workers in this registry).
    pub coordinator_id: AgentId,
    /// Shared mailbox router.
    pub mailbox_router: Arc<MailboxRouter>,
    /// Name of the single team this coordinator is running.
    ///
    /// Source of the reserved `ClientEvent::CoordinatorStatus { team }` DTO.
    /// We do not multiplex concurrent teams in this pass, so a single
    /// optional name suffices.
    team_name: RwLock<Option<String>>,
}

impl TeamRegistry {
    /// Construct an empty registry owned by `coordinator_id`.
    #[must_use]
    pub fn new(coordinator_id: AgentId) -> Self {
        Self {
            workers: RwLock::new(HashMap::new()),
            coordinator_id,
            mailbox_router: Arc::new(MailboxRouter::new()),
            team_name: RwLock::new(None),
        }
    }

    /// Spawn (i.e. register) a new worker.
    pub async fn spawn_worker(
        &self,
        agent_type: String,
        name: String,
        task_id: String,
    ) -> Result<AgentId, crate::mailbox::MailboxError> {
        let agent_id = AgentId::new();
        let mailbox = Arc::new(TeammateMailbox::new(agent_id));
        self.mailbox_router.register(agent_id, mailbox).await;
        self.workers.write().await.insert(
            agent_id,
            WorkerAgent {
                agent_id,
                agent_type,
                name,
                parent_id: Some(self.coordinator_id),
                status: WorkerStatus::Idle,
                task_id,
                spawned_at: SystemTime::now(),
                last_active_at: SystemTime::now(),
            },
        );
        Ok(agent_id)
    }

    /// Remove a worker and unregister its mailbox.
    pub async fn delete_worker(&self, agent_id: &AgentId) {
        self.workers.write().await.remove(agent_id);
        self.mailbox_router.unregister(agent_id).await;
    }

    /// List all currently registered workers.
    pub async fn list(&self) -> Vec<WorkerAgent> {
        self.workers.read().await.values().cloned().collect()
    }

    /// Update a worker's status and touch its `last_active_at` timestamp.
    ///
    /// No-op (no panic) if no worker with `agent_id` is registered.
    pub async fn update_status(&self, agent_id: &AgentId, status: WorkerStatus) {
        if let Some(worker) = self.workers.write().await.get_mut(agent_id) {
            worker.status = status;
            worker.last_active_at = SystemTime::now();
        }
    }

    /// Write back the handler-generated task id onto a worker.
    ///
    /// No-op (no panic) if no worker with `agent_id` is registered.
    pub async fn set_task_id(&self, agent_id: &AgentId, task_id: String) {
        if let Some(worker) = self.workers.write().await.get_mut(agent_id) {
            worker.task_id = task_id;
            worker.last_active_at = SystemTime::now();
        }
    }

    /// Find a worker by its (handler-generated) task id.
    ///
    /// Linear scan over the workers map — acceptable at the per-session team
    /// scale (a handful of workers); avoids a second index to keep in sync
    /// with `set_task_id` / `delete_worker`. An empty `task_id` never matches
    /// a freshly-spawned, not-yet-linked worker (their `task_id` is empty too,
    /// so callers must not look up the empty string).
    pub async fn find_by_task_id(&self, task_id: &str) -> Option<WorkerAgent> {
        if task_id.is_empty() {
            return None;
        }
        self.workers
            .read()
            .await
            .values()
            .find(|w| w.task_id == task_id)
            .cloned()
    }

    /// Resolve a worker by its display `name` (case-insensitive).
    ///
    /// Coordinator-side `SendMessage` accepts a bare teammate name as the
    /// recipient (TS `to`), which it resolves to the worker's [`AgentId`] before
    /// routing. Linear scan over the workers map — acceptable at per-session team
    /// scale (a handful of workers), mirroring [`Self::find_by_task_id`]. An
    /// empty `name` never matches.
    pub async fn find_by_name(&self, name: &str) -> Option<WorkerAgent> {
        if name.is_empty() {
            return None;
        }
        self.workers
            .read()
            .await
            .values()
            .find(|w| w.name.eq_ignore_ascii_case(name))
            .cloned()
    }

    /// Set (or clear) the team name for this coordinator session.
    pub async fn set_team_name(&self, name: Option<String>) {
        *self.team_name.write().await = name;
    }

    /// Read the current team name, if any.
    pub async fn team_name(&self) -> Option<String> {
        self.team_name.read().await.clone()
    }

    /// Count of workers whose status is non-terminal.
    ///
    /// Non-terminal = `Idle` | `Working { .. }` | `AwaitingMessage`.
    /// Terminal (excluded) = `Completed` | `Failed { .. }` | `Killed`.
    pub async fn active_worker_count(&self) -> u32 {
        let count = self
            .workers
            .read()
            .await
            .values()
            .filter(|w| {
                matches!(
                    w.status,
                    WorkerStatus::Idle
                        | WorkerStatus::Working { .. }
                        | WorkerStatus::AwaitingMessage
                )
            })
            .count();
        u32::try_from(count).unwrap_or(u32::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn spawn_then_update_status_transitions() {
        let reg = TeamRegistry::new(AgentId::new());
        let id = reg
            .spawn_worker("explorer".into(), "alpha".into(), String::new())
            .await
            .unwrap();

        // Freshly spawned worker is Idle.
        let before = reg.list().await;
        assert_eq!(before.len(), 1);
        assert_eq!(before[0].status, WorkerStatus::Idle);
        let last_active_before = before[0].last_active_at;

        // Sleep a hair so the monotonic clock can advance.
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;

        reg.update_status(
            &id,
            WorkerStatus::Working {
                activity: "running".into(),
            },
        )
        .await;

        let after = reg.list().await;
        assert_eq!(after.len(), 1);
        assert_eq!(
            after[0].status,
            WorkerStatus::Working {
                activity: "running".into()
            }
        );
        assert!(
            after[0].last_active_at >= last_active_before,
            "last_active_at must advance (or stay equal) on update_status"
        );
    }

    #[tokio::test]
    async fn update_status_failed_and_killed() {
        let reg = TeamRegistry::new(AgentId::new());
        let id = reg
            .spawn_worker("writer".into(), "beta".into(), String::new())
            .await
            .unwrap();

        reg.update_status(
            &id,
            WorkerStatus::Failed {
                error: "boom".into(),
            },
        )
        .await;
        assert_eq!(
            reg.list().await[0].status,
            WorkerStatus::Failed {
                error: "boom".into()
            }
        );

        reg.update_status(&id, WorkerStatus::Killed).await;
        assert_eq!(reg.list().await[0].status, WorkerStatus::Killed);
    }

    #[tokio::test]
    async fn find_by_task_id_roundtrip() {
        let reg = TeamRegistry::new(AgentId::new());
        let id = reg
            .spawn_worker("explorer".into(), "gamma".into(), String::new())
            .await
            .unwrap();

        // Not yet linked: empty task_id should not resolve to this worker.
        assert!(reg.find_by_task_id("task-xyz").await.is_none());

        reg.set_task_id(&id, "task-xyz".into()).await;
        let found = reg.find_by_task_id("task-xyz").await;
        assert!(found.is_some());
        let found = found.unwrap();
        assert_eq!(found.agent_id, id);
        assert_eq!(found.task_id, "task-xyz");

        // Unknown id resolves to nothing.
        assert!(reg.find_by_task_id("nope").await.is_none());
    }

    #[tokio::test]
    async fn active_worker_count_excludes_terminal() {
        let reg = TeamRegistry::new(AgentId::new());
        let working = reg
            .spawn_worker("a".into(), "w-working".into(), String::new())
            .await
            .unwrap();
        let idle = reg
            .spawn_worker("a".into(), "w-idle".into(), String::new())
            .await
            .unwrap();
        let awaiting = reg
            .spawn_worker("a".into(), "w-awaiting".into(), String::new())
            .await
            .unwrap();
        let failed = reg
            .spawn_worker("a".into(), "w-failed".into(), String::new())
            .await
            .unwrap();
        let killed = reg
            .spawn_worker("a".into(), "w-killed".into(), String::new())
            .await
            .unwrap();
        let completed = reg
            .spawn_worker("a".into(), "w-completed".into(), String::new())
            .await
            .unwrap();

        reg.update_status(
            &working,
            WorkerStatus::Working {
                activity: "running".into(),
            },
        )
        .await;
        // `idle` stays Idle (spawned default).
        let _ = idle;
        reg.update_status(&awaiting, WorkerStatus::AwaitingMessage)
            .await;
        reg.update_status(
            &failed,
            WorkerStatus::Failed {
                error: "x".into(),
            },
        )
        .await;
        reg.update_status(&killed, WorkerStatus::Killed).await;
        reg.update_status(&completed, WorkerStatus::Completed).await;

        // Non-terminal = Idle | Working | AwaitingMessage => 3.
        // Terminal (excluded) = Completed | Failed | Killed.
        assert_eq!(reg.active_worker_count().await, 3);
    }

    #[tokio::test]
    async fn team_name_set_get() {
        let reg = TeamRegistry::new(AgentId::new());
        assert_eq!(reg.team_name().await, None);

        reg.set_team_name(Some("alpha".into())).await;
        assert_eq!(reg.team_name().await, Some("alpha".to_string()));

        reg.set_team_name(None).await;
        assert_eq!(reg.team_name().await, None);
    }

    #[tokio::test]
    async fn update_status_unknown_agent_is_noop() {
        let reg = TeamRegistry::new(AgentId::new());
        // No worker registered: must not panic.
        reg.update_status(&AgentId::new(), WorkerStatus::Killed)
            .await;
        assert!(reg.list().await.is_empty());
    }
}

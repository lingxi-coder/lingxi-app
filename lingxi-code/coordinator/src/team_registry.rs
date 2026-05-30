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
}

impl TeamRegistry {
    /// Construct an empty registry owned by `coordinator_id`.
    #[must_use]
    pub fn new(coordinator_id: AgentId) -> Self {
        Self {
            workers: RwLock::new(HashMap::new()),
            coordinator_id,
            mailbox_router: Arc::new(MailboxRouter::new()),
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
}

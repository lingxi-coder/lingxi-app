//! Generic `Task` trait — implemented per [`TaskType`](crate::id::TaskType).

use async_trait::async_trait;
use lingxi_traits::{FileSystem, RuntimeSpawner};
use std::sync::Arc;
use thiserror::Error;

/// Generic interface implemented by per-type task handlers.
#[async_trait]
pub trait Task: Send + Sync {
    /// Human-readable handler name (for logs and metrics).
    fn name(&self) -> &str;
    /// Task type discriminant.
    fn task_type(&self) -> crate::id::TaskType;
    /// Spawn a new task instance.
    async fn spawn(&self, input: TaskSpawnInput, ctx: TaskContext)
        -> Result<TaskHandle, TaskError>;
    /// Kill a running task instance.
    async fn kill(&self, task_id: &str, ctx: TaskContext) -> Result<(), TaskError>;
    /// Whether this task type supports inbound messages.
    fn supports_messages(&self) -> bool {
        false
    }
    /// Deliver a message to a running task. Default implementation rejects.
    async fn send_message(
        &self,
        _task_id: &str,
        _message: String,
        _ctx: TaskContext,
    ) -> Result<(), TaskError> {
        Err(TaskError::Unsupported)
    }
}

/// Spawn-time input — one variant per task type.
#[derive(Debug, Clone)]
pub enum TaskSpawnInput {
    /// Spawn a local bash command.
    LocalBash {
        /// Bash command string.
        command: String,
        /// Optional time-out.
        timeout: Option<std::time::Duration>,
    },
    /// Spawn an in-process agent.
    LocalAgent {
        /// Agent target.
        agent_id: lingxi_protocol::AgentId,
        /// Initial prompt.
        prompt: String,
        /// Whether to start backgrounded.
        is_backgrounded: bool,
    },
    /// Spawn a remote agent.
    RemoteAgent {
        /// Endpoint URL.
        endpoint: String,
        /// Initial prompt.
        prompt: String,
    },
    /// Spawn an in-process teammate.
    InProcessTeammate {
        /// Agent target.
        agent_id: lingxi_protocol::AgentId,
        /// Display name.
        name: String,
    },
    /// Spawn a local workflow.
    LocalWorkflow {
        /// Workflow identifier.
        workflow_id: String,
    },
    /// Spawn an MCP monitor.
    MonitorMcp {
        /// Server name to monitor.
        server_name: String,
        /// Resources to watch.
        watch: Vec<String>,
    },
    /// Spawn a dream loop.
    Dream {
        /// Initial prompt.
        prompt: String,
        /// Optional cap on iterations.
        max_iterations: Option<u32>,
    },
}

/// Per-task execution context provided by the engine.
#[derive(Clone)]
pub struct TaskContext {
    /// File-system trait object.
    pub fs: Arc<dyn FileSystem>,
    /// Runtime spawner trait object.
    pub runtime: Arc<dyn RuntimeSpawner>,
}

/// Handle returned by [`Task::spawn`].
pub struct TaskHandle {
    /// Generated task ID.
    pub task_id: String,
    /// Optional cleanup hook to run on task termination.
    pub cleanup: Option<Arc<dyn Fn() + Send + Sync>>,
}

/// Errors produced by [`Task`] operations.
#[derive(Debug, Clone, Error)]
pub enum TaskError {
    /// Handler does not exist for the requested task type.
    #[error("unknown task type")]
    UnknownType,
    /// No task is registered with the given ID.
    #[error("task not found: {0}")]
    NotFound(String),
    /// Task already reached terminal status — cannot accept messages.
    #[error("terminated task — cannot accept messages")]
    TerminatedTask,
    /// Operation unsupported by the underlying handler.
    #[error("unsupported operation")]
    Unsupported,
    /// I/O failure.
    #[error("io: {0}")]
    Io(String),
    /// Internal handler error.
    #[error("internal: {0}")]
    Internal(String),
}

//! Polymorphic task state — one variant per [`TaskType`](crate::id::TaskType).

use crate::id::TaskType;
use protocol::AgentId;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::SystemTime;

/// Lifecycle state of a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStatus {
    /// Created but not yet started.
    Pending,
    /// Currently executing.
    Running,
    /// Finished successfully.
    Completed,
    /// Finished with an error.
    Failed,
    /// Killed by the user or the runtime.
    Killed,
}

impl TaskStatus {
    /// Whether the status represents a terminal (non-recoverable) state.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Killed)
    }
}

/// Fields shared by every task type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskStateBase {
    /// Task ID (e.g. `b3f9zk2x`).
    pub id: String,
    /// Task type discriminant.
    pub task_type: TaskType,
    /// Current status.
    pub status: TaskStatus,
    /// Human-readable description (shown in UI).
    pub description: String,
    /// Originating `tool_use_id` if launched from a tool call.
    pub tool_use_id: Option<String>,
    /// Wall-clock start time.
    pub start_time: SystemTime,
    /// Wall-clock end time, once terminal.
    pub end_time: Option<SystemTime>,
    /// Cumulative paused duration in milliseconds.
    pub total_paused_ms: u64,
    /// Path to the spool file accumulating stdout/stderr.
    pub output_file: PathBuf,
    /// Last byte offset surfaced to the caller (for incremental reads).
    pub output_offset: u64,
    /// Whether the user has been notified of completion.
    pub notified: bool,
}

/// Tagged union of per-type task states.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "task_type", rename_all = "snake_case")]
pub enum TaskState {
    /// Local bash command.
    LocalBash(LocalBashTaskState),
    /// Local agent.
    LocalAgent(LocalAgentTaskState),
    /// Remote agent.
    RemoteAgent(RemoteAgentTaskState),
    /// In-process teammate.
    InProcessTeammate(InProcessTeammateTaskState),
    /// Local workflow.
    LocalWorkflow(LocalWorkflowTaskState),
    /// MCP monitor.
    MonitorMcp(MonitorMcpTaskState),
    /// Dream loop.
    Dream(DreamTaskState),
}

impl TaskState {
    /// Borrow the common base fields regardless of variant.
    #[must_use]
    pub fn base(&self) -> &TaskStateBase {
        match self {
            Self::LocalBash(s) => &s.base,
            Self::LocalAgent(s) => &s.base,
            Self::RemoteAgent(s) => &s.base,
            Self::InProcessTeammate(s) => &s.base,
            Self::LocalWorkflow(s) => &s.base,
            Self::MonitorMcp(s) => &s.base,
            Self::Dream(s) => &s.base,
        }
    }
}

/// State specific to a local bash task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalBashTaskState {
    /// Shared base fields.
    #[serde(flatten)]
    pub base: TaskStateBase,
    /// Bash command string.
    pub command: String,
    /// OS process ID once running.
    pub pid: Option<u32>,
    /// Exit code once terminated.
    pub exit_code: Option<i32>,
}

/// State specific to an in-process agent task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalAgentTaskState {
    /// Shared base fields.
    #[serde(flatten)]
    pub base: TaskStateBase,
    /// Target agent.
    pub agent_id: AgentId,
    /// Initial prompt.
    pub prompt: String,
    /// Error message if the agent failed.
    pub error: Option<String>,
    /// Accumulated conversation messages.
    pub messages: Vec<protocol::ConversationMessage>,
    /// Inbound messages queued for delivery.
    pub pending_messages: Vec<String>,
    /// Whether the agent is currently backgrounded.
    pub is_backgrounded: bool,
}

/// State specific to a remote agent task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteAgentTaskState {
    /// Shared base fields.
    #[serde(flatten)]
    pub base: TaskStateBase,
    /// Remote session identifier.
    pub remote_session_id: String,
    /// Endpoint URL.
    pub remote_endpoint: String,
}

/// State specific to an in-process teammate task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InProcessTeammateTaskState {
    /// Shared base fields.
    #[serde(flatten)]
    pub base: TaskStateBase,
    /// Teammate agent ID.
    pub agent_id: AgentId,
    /// Inbound messages queued for delivery.
    pub pending_messages: Vec<String>,
}

/// State specific to a local workflow task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalWorkflowTaskState {
    /// Shared base fields.
    #[serde(flatten)]
    pub base: TaskStateBase,
    /// Workflow identifier.
    pub workflow_id: String,
    /// Index of the currently-executing step.
    pub current_step: usize,
}

/// State specific to an MCP monitor task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitorMcpTaskState {
    /// Shared base fields.
    #[serde(flatten)]
    pub base: TaskStateBase,
    /// Server name being monitored.
    pub server_name: String,
    /// Resource URIs watched.
    pub watch_resources: Vec<String>,
}

/// State specific to a dream task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DreamTaskState {
    /// Shared base fields.
    #[serde(flatten)]
    pub base: TaskStateBase,
    /// Number of iterations performed so far.
    pub iteration_count: u32,
    /// Optional cap on iterations.
    pub max_iterations: Option<u32>,
}

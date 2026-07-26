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
    /// Display name of the teammate / subagent that CREATED this task, if any.
    /// Additive + defaulted so older serialized rows remain valid.
    #[serde(default)]
    pub creator_teammate_name: Option<String>,
    /// Team name of the teammate / subagent that CREATED this task, if any.
    /// Additive + defaulted so older serialized rows remain valid.
    #[serde(default)]
    pub creator_team_name: Option<String>,
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
    /// Shell stdout event monitor.
    Monitor(MonitorTaskState),
    /// Backgrounded MCP tool call (`mcp_task`).
    McpTask(McpTaskState),
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
            Self::Monitor(s) => &s.base,
            Self::McpTask(s) => &s.base,
            Self::Dream(s) => &s.base,
        }
    }

    /// Mutably borrow the common base fields regardless of variant.
    #[must_use]
    pub fn base_mut(&mut self) -> &mut TaskStateBase {
        match self {
            Self::LocalBash(s) => &mut s.base,
            Self::LocalAgent(s) => &mut s.base,
            Self::RemoteAgent(s) => &mut s.base,
            Self::InProcessTeammate(s) => &mut s.base,
            Self::LocalWorkflow(s) => &mut s.base,
            Self::MonitorMcp(s) => &mut s.base,
            Self::Monitor(s) => &mut s.base,
            Self::McpTask(s) => &mut s.base,
            Self::Dream(s) => &mut s.base,
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
    /// Resolved subagent type label (one of the registered agent types, e.g.
    /// `general-purpose`). Independent sibling of [`Self::agent_id`] (which is a
    /// per-instance identity UUID). Mirrors claude-code `LocalAgentTaskState`'s
    /// `agentType` — surfaced verbatim as the `Stop` / `SubagentStop` hook
    /// `background_tasks[].agent_type` field (claude-code `Lic`'s `n.agentType`).
    /// `#[serde(default)]` so older on-disk task rows (pre-field) still parse.
    #[serde(default)]
    pub subagent_type: String,
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
    /// What the run reported when it terminated — final text, usage, and the
    /// kept-worktree coordinates — plus `killed_by` once a stop names its
    /// initiator. Populated by
    /// [`TaskRegistryHandle::set_agent_outcome`](traits::task_registry::TaskRegistryHandle::set_agent_outcome)
    /// / `kill_with_reason` and read by the notification drain, which before
    /// this always rendered a `local_agent` completion with no `<result>`,
    /// `<usage>` or `<worktree>` and every stop as the bare `was stopped`.
    ///
    /// `#[serde(default)]` so pre-field on-disk task rows still parse.
    #[serde(default)]
    pub outcome: AgentOutcomeState,
}

/// [`LocalAgentTaskState::outcome`] — the terminal notification payload plus
/// the stop initiator.
///
/// [`traits::task_registry::AgentTerminalOutcome`] is the WRITE shape (what a
/// terminating run reports); this is the stored shape, which additionally holds
/// `killed_by` because that arrives from the kill path rather than from the
/// run.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentOutcomeState {
    /// Final text response → the notification's `<result>` section.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    /// Run usage → the `<usage>` section.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<traits::task_registry::AgentRunUsage>,
    /// Who stopped the task (`"parent"` / `"user"`) → the killed-summary verb.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub killed_by: Option<String>,
    /// Kept isolation worktree path → gates and fills `<worktree>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_path: Option<String>,
    /// Kept isolation worktree branch → `<worktreeBranch>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_branch: Option<String>,
}

impl AgentOutcomeState {
    /// Merge a terminating run's report in. A `Some` field overwrites; a `None`
    /// leaves the stored value alone, so a later partial report (e.g. a kill
    /// that only carries a worktree) never erases an earlier result.
    pub fn merge(&mut self, incoming: traits::task_registry::AgentTerminalOutcome) {
        let traits::task_registry::AgentTerminalOutcome {
            result,
            usage,
            error: _,
            worktree_path,
            worktree_branch,
        } = incoming;
        if result.is_some() {
            self.result = result;
        }
        if usage.is_some() {
            self.usage = usage;
        }
        if worktree_path.is_some() {
            self.worktree_path = worktree_path;
        }
        if worktree_branch.is_some() {
            self.worktree_branch = worktree_branch;
        }
    }
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
    /// The model-authored workflow script source (carried for resume).
    #[serde(default)]
    pub script: String,
    /// Prior run id to resume journaled `agent()` results from, if any.
    #[serde(default)]
    pub resume_from_run_id: Option<String>,
    /// The `args` global value (JSON string), if any.
    #[serde(default)]
    pub args: Option<String>,
    /// The EFFECTIVE run id (`wf_…`) this workflow executes under — the
    /// launcher-minted id for a fresh run, or the resumed id. Stored so the
    /// resume gate (claude-code validateInput errorCode 3) can detect a
    /// `resumeFromRunId` that names a still-running workflow.
    #[serde(default)]
    pub run_id: Option<String>,
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

/// State specific to a shell stdout event monitor (`monitor_ws`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitorTaskState {
    /// Shared base fields.
    #[serde(flatten)]
    pub base: TaskStateBase,
    /// Shell command being monitored.
    pub command: String,
    /// Exit code once the command terminates.
    pub exit_code: Option<i32>,
}

/// State specific to a backgrounded MCP tool call (claude-code `mcp_task`,
/// minted by `callMcpToolWithAutoBackground`/`NZu`). Distinct from
/// [`MonitorMcpTaskState`], which watches a whole server; this tracks one
/// detached `tools/call`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTaskState {
    /// Shared base fields.
    #[serde(flatten)]
    pub base: TaskStateBase,
    /// MCP server name (`serverName`).
    pub server_name: String,
    /// MCP tool name (`toolName`).
    pub tool_name: String,
    /// Coarse MCP task status (`mcpStatus`): `"working"` | `"input_required"`
    /// | `"completed"` | `"cancelled"` | `"failed"`. Defaults to `"working"`.
    pub mcp_status: String,
    /// Latest human-readable status line (`statusMessage`), if any.
    pub status_message: Option<String>,
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

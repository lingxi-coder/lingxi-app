//! Hook event taxonomy (spec §9.1).
//!
//! The engine emits one of 28 well-known event kinds at observable points in
//! its lifecycle. Each kind carries a payload describing what just happened
//! (tool input, session metadata, file path, etc.). Registered hooks subscribe
//! by `HookEventType` (the type-tag enum) and inspect the carried `HookEvent`
//! payload when invoked.

use protocol::{AgentId, SessionId, ToolUseId};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

/// Type-tag for every supported hook event. Used by [`crate::HookDefinition`]
/// to declare which events a hook subscribes to without copying the full
/// payload schema.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum HookEventType {
    /// About to invoke a tool; hook may block, mutate input, or annotate.
    PreToolUse,
    /// Tool completed successfully; hook may augment or post-process result.
    PostToolUse,
    /// Tool returned an error; hook may convert / suppress / re-raise.
    PostToolUseFailure,
    /// A new session has been opened.
    SessionStart,
    /// Session terminated (gracefully or otherwise).
    SessionEnd,
    /// Engine setup phase — one-time initialization after process start.
    Setup,
    /// User submitted a top-level prompt; hook may rewrite or block it.
    UserPromptSubmit,
    /// Agent / session reached a stop signal.
    Stop,
    /// Agent / session stop attempt failed.
    StopFailure,
    /// A subagent (forked agent) has just been spawned.
    SubagentStart,
    /// A subagent has finished or been cancelled.
    SubagentStop,
    /// About to compact a long context window.
    PreCompact,
    /// Compaction finished; payload reports tokens freed.
    PostCompact,
    /// Engine is about to ask the user (or auto-policy) for permission.
    PermissionRequest,
    /// A permission request was denied.
    PermissionDenied,
    /// A teammate / agent has gone idle.
    TeammateIdle,
    /// A task (background work item) was created.
    TaskCreated,
    /// A task transitioned to a terminal status.
    TaskCompleted,
    /// An MCP server requested user elicitation.
    Elicitation,
    /// User completed (or cancelled) an elicitation; result available.
    ElicitationResult,
    /// Engine configuration values changed at runtime.
    ConfigChange,
    /// A new git worktree was created.
    WorktreeCreate,
    /// An existing git worktree was removed.
    WorktreeRemove,
    /// Per-session / per-cwd instruction files were (re)loaded.
    InstructionsLoaded,
    /// The current working directory changed (e.g., `cd` tool).
    CwdChanged,
    /// A watched file mutated on disk.
    FileChanged,
    /// A user-visible notification was raised by the engine or a hook.
    Notification,
}

/// Concrete payload for a hook event. The variant must match the
/// corresponding [`HookEventType`] returned by [`HookEvent::event_type`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HookEvent {
    /// About to invoke a tool. The hook may rewrite `tool_input` or return
    /// a `Block` decision to short-circuit the call.
    PreToolUse {
        /// Canonical name of the tool (e.g. `"Bash"`, `"Read"`).
        tool_name: String,
        /// Raw tool input JSON. Hooks may emit a mutated copy via
        /// [`crate::HookResponse::updated_input`].
        tool_input: Value,
        /// Unique ID of this tool invocation (correlates with `PostToolUse`).
        tool_use_id: ToolUseId,
    },
    /// A tool call completed successfully.
    PostToolUse {
        /// Canonical name of the tool.
        tool_name: String,
        /// The tool input the engine ultimately dispatched (post-mutation).
        tool_input: Value,
        /// Result payload returned by the tool.
        tool_output: Value,
        /// Tool invocation ID, matching the prior `PreToolUse`.
        tool_use_id: ToolUseId,
    },
    /// A tool call returned an error.
    PostToolUseFailure {
        /// Canonical name of the tool.
        tool_name: String,
        /// Stringified error for log / display.
        error: String,
        /// Tool invocation ID, matching the prior `PreToolUse`.
        tool_use_id: ToolUseId,
    },
    /// A new session was opened.
    SessionStart {
        /// New session ID.
        session_id: SessionId,
        /// Origin of the session (e.g. `"cli"`, `"resume"`, `"api"`).
        source: String,
    },
    /// A session terminated.
    SessionEnd {
        /// Session that ended.
        session_id: SessionId,
        /// Stringified reason (`"user_exit"`, `"signal"`, `"error: ..."`).
        reason: String,
    },
    /// Engine startup setup phase. Fired once early in the lifecycle.
    Setup,
    /// User submitted a top-level prompt. Hooks may rewrite or block.
    UserPromptSubmit {
        /// The user-supplied prompt text.
        prompt: String,
    },
    /// Agent / session reached a stop signal.
    Stop {
        /// Stringified reason.
        reason: String,
    },
    /// Agent / session stop attempt failed.
    StopFailure {
        /// Stringified error.
        error: String,
    },
    /// A subagent was spawned.
    SubagentStart {
        /// Newly assigned agent ID.
        agent_id: AgentId,
        /// Agent type discriminator (e.g. `"general-purpose"`).
        agent_type: String,
        /// Parent agent that spawned this subagent, if any.
        parent_agent_id: Option<AgentId>,
    },
    /// A subagent finished or was cancelled.
    SubagentStop {
        /// Subagent ID.
        agent_id: AgentId,
        /// Stringified status (e.g. `"completed"`, `"cancelled"`).
        status: String,
    },
    /// About to compact a long context window.
    PreCompact {
        /// Stringified reason (e.g. `"manual"`, `"threshold"`).
        reason: String,
    },
    /// Compaction finished.
    PostCompact {
        /// Summary text produced by the compaction pass.
        summary: String,
        /// Number of tokens reclaimed by the compaction.
        tokens_freed: u64,
    },
    /// Engine is about to request permission.
    PermissionRequest {
        /// Tool the permission applies to.
        tool_name: String,
        /// Tool input that triggered the request.
        tool_input: Value,
        /// Engine-supplied human-readable reason for the prompt.
        reason: String,
    },
    /// A permission request was denied.
    PermissionDenied {
        /// Tool the permission applied to.
        tool_name: String,
        /// Stringified reason for the denial.
        reason: String,
    },
    /// A teammate (agent) became idle.
    TeammateIdle {
        /// Idle agent.
        agent_id: AgentId,
    },
    /// A task was created.
    TaskCreated {
        /// Caller-supplied or engine-generated task ID.
        task_id: String,
        /// Task taxonomy bucket.
        task_type: String,
        /// Human-readable description for logs.
        description: String,
    },
    /// A task transitioned to a terminal status.
    TaskCompleted {
        /// Task ID matching the prior `TaskCreated`.
        task_id: String,
        /// Terminal status (e.g. `"success"`, `"failure"`, `"cancelled"`).
        status: String,
    },
    /// An MCP server requested user elicitation.
    Elicitation {
        /// Name of the MCP server.
        server_name: String,
        /// Server-supplied parameters.
        params: Value,
    },
    /// User completed (or cancelled) an elicitation.
    ElicitationResult {
        /// Name of the MCP server that requested elicitation.
        server_name: String,
        /// User-supplied result payload.
        result: Value,
    },
    /// Engine configuration changed at runtime.
    ConfigChange {
        /// One element per altered config key; shape is system-defined JSON.
        changes: Vec<Value>,
    },
    /// A new git worktree was created.
    WorktreeCreate {
        /// Absolute path of the new worktree.
        path: PathBuf,
        /// Branch name checked out in the worktree.
        branch: String,
    },
    /// An existing git worktree was removed.
    WorktreeRemove {
        /// Absolute path of the removed worktree.
        path: PathBuf,
    },
    /// Per-cwd / per-session instructions were (re)loaded.
    InstructionsLoaded {
        /// All instruction files included in the load.
        paths: Vec<PathBuf>,
    },
    /// The current working directory changed.
    CwdChanged {
        /// Previous cwd.
        old: PathBuf,
        /// New cwd.
        new: PathBuf,
    },
    /// A watched file mutated on disk.
    FileChanged {
        /// Path of the file that changed.
        path: PathBuf,
        /// Mutation kind (`"create"`, `"modify"`, `"delete"`, `"rename"`).
        kind: String,
    },
    /// A user-visible notification was raised.
    Notification {
        /// Human-readable message.
        message: String,
        /// Notification taxonomy (`"info"`, `"warn"`, `"error"`).
        kind: String,
    },
}

impl HookEvent {
    /// Return the type-tag for this event payload. Used by the registry to
    /// match events against subscribed hooks without copying the payload.
    #[must_use]
    pub fn event_type(&self) -> HookEventType {
        match self {
            Self::PreToolUse { .. } => HookEventType::PreToolUse,
            Self::PostToolUse { .. } => HookEventType::PostToolUse,
            Self::PostToolUseFailure { .. } => HookEventType::PostToolUseFailure,
            Self::SessionStart { .. } => HookEventType::SessionStart,
            Self::SessionEnd { .. } => HookEventType::SessionEnd,
            Self::Setup => HookEventType::Setup,
            Self::UserPromptSubmit { .. } => HookEventType::UserPromptSubmit,
            Self::Stop { .. } => HookEventType::Stop,
            Self::StopFailure { .. } => HookEventType::StopFailure,
            Self::SubagentStart { .. } => HookEventType::SubagentStart,
            Self::SubagentStop { .. } => HookEventType::SubagentStop,
            Self::PreCompact { .. } => HookEventType::PreCompact,
            Self::PostCompact { .. } => HookEventType::PostCompact,
            Self::PermissionRequest { .. } => HookEventType::PermissionRequest,
            Self::PermissionDenied { .. } => HookEventType::PermissionDenied,
            Self::TeammateIdle { .. } => HookEventType::TeammateIdle,
            Self::TaskCreated { .. } => HookEventType::TaskCreated,
            Self::TaskCompleted { .. } => HookEventType::TaskCompleted,
            Self::Elicitation { .. } => HookEventType::Elicitation,
            Self::ElicitationResult { .. } => HookEventType::ElicitationResult,
            Self::ConfigChange { .. } => HookEventType::ConfigChange,
            Self::WorktreeCreate { .. } => HookEventType::WorktreeCreate,
            Self::WorktreeRemove { .. } => HookEventType::WorktreeRemove,
            Self::InstructionsLoaded { .. } => HookEventType::InstructionsLoaded,
            Self::CwdChanged { .. } => HookEventType::CwdChanged,
            Self::FileChanged { .. } => HookEventType::FileChanged,
            Self::Notification { .. } => HookEventType::Notification,
        }
    }
}

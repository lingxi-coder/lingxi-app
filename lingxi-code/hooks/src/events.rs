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

/// Source of a [`HookEvent::ConfigChange`] — which settings layer (or skills)
/// mutated on disk. 1:1 with claude-code's `CONFIG_CHANGE_SOURCES`
/// (`coreSchemas.ts:662-668`); the wire `source` value is the kebab/snake
/// literal each variant serializes to (e.g. `"user_settings"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigChangeSource {
    /// `~/.claude/settings.json` (user-global).
    UserSettings,
    /// `.claude/settings.json` (project-shared, checked in).
    ProjectSettings,
    /// `.claude/settings.local.json` (project-local, git-ignored).
    LocalSettings,
    /// Enterprise-managed policy settings (never blockable by hooks).
    PolicySettings,
    /// A skill definition changed.
    Skills,
}

/// Which memory tier an [`HookEvent::InstructionsLoaded`] file belongs to.
/// 1:1 with claude-code's `INSTRUCTIONS_MEMORY_TYPES`
/// (`coreSchemas.ts:688-693`). Note: these serialize as `PascalCase` wire
/// literals (`"User"` / `"Project"` / `"Local"` / `"Managed"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InstructionsMemoryType {
    /// User-global instructions (`~/.claude/CLAUDE.md`).
    User,
    /// Project-shared instructions (`./CLAUDE.md`).
    Project,
    /// Project-local instructions (`./CLAUDE.local.md`).
    Local,
    /// Enterprise-managed (policy) instructions.
    Managed,
}

/// Why an [`HookEvent::InstructionsLoaded`] file was (re)loaded. 1:1 with
/// claude-code's `INSTRUCTIONS_LOAD_REASONS` (`coreSchemas.ts:680-686`); the
/// wire value is the `snake_case` literal each variant serializes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstructionsLoadReason {
    /// Eager load at session start.
    SessionStart,
    /// Lazy load triggered by traversing into a nested directory.
    NestedTraversal,
    /// Conditional rules whose `paths:` glob matched a touched file.
    PathGlobMatch,
    /// Loaded via an explicit `@include` directive.
    Include,
    /// Eager reload after a context compaction.
    Compact,
}

/// Elicitation presentation mode (`form` / `url`). 1:1 with the `mode` enum in
/// claude-code's `ElicitationHookInputSchema` (`coreSchemas.ts:634`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ElicitationMode {
    /// Render a structured form from `requested_schema`.
    Form,
    /// Open a URL for the user to complete out-of-band.
    Url,
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
        /// The tool input the engine ultimately dispatched (post-mutation),
        /// matching the `tool_input` carried by `PostToolUse`.
        tool_input: Value,
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
        /// Tool input that was denied (wire `tool_input`). 1:1 with the
        /// `PermissionDeniedHookInputSchema` field (`coreSchemas.ts:466`).
        tool_input: Value,
        /// Tool invocation ID, matching the prior `PreToolUse`
        /// (wire `tool_use_id`, `coreSchemas.ts:467`).
        tool_use_id: ToolUseId,
        /// Stringified reason for the denial.
        reason: String,
    },
    /// A teammate's query loop stopped and it is about to park awaiting the
    /// next message ("about to go idle").
    ///
    /// Field set mirrors `TeammateIdleHookInputSchema` (`coreSchemas.ts:591-598`)
    /// so the executor arm can reproduce the wire payload byte-faithfully:
    /// `teammate_name` (required) + `team_name` (required). claude-code fires
    /// `executeTeammateIdleHooks` from `stopHooks.ts:403`, gated on
    /// `isTeammate()`, sourcing `teammate_name` from `getAgentName() ?? ''` and
    /// `team_name` from `getTeamName() ?? ''` — so `team_name` may be `""` when
    /// the firing scope cannot reach the team identity.
    TeammateIdle {
        /// Name of the teammate going idle (wire `teammate_name`, required).
        teammate_name: String,
        /// Team the teammate belongs to (wire `team_name`, required; `""` when
        /// the firing scope has no team identity).
        team_name: String,
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
    ///
    /// Field set mirrors `TaskCompletedHookInputSchema`
    /// (`coreSchemas.ts:614-625`) so the executor arm can reproduce the wire
    /// payload byte-faithfully: `task_subject` (required), `task_description` /
    /// `teammate_name` / `team_name` (optional). `status` is carried for routing
    /// only — it is NOT part of the wire payload (the schema has no `status`
    /// field; claude-code fires `executeTaskCompletedHooks` only on the terminal
    /// transition).
    TaskCompleted {
        /// Task ID matching the prior `TaskCreated`.
        task_id: String,
        /// Terminal status (e.g. `"completed"`, `"failed"`). Routing only — not
        /// serialized into the wire payload.
        status: String,
        /// Task subject/title (wire `task_subject`, required). claude-code
        /// sources this from `existingTask.subject` / `task.subject`.
        task_subject: String,
        /// Task description (wire `task_description`, optional). claude-code
        /// sources this from `existingTask.description` / `task.description`.
        task_description: Option<String>,
        /// Name of the teammate completing the task (wire `teammate_name`,
        /// optional). claude-code sources this from `getAgentName()`.
        teammate_name: Option<String>,
        /// Team the teammate belongs to (wire `team_name`, optional).
        /// claude-code sources this from `getTeamName()`.
        team_name: Option<String>,
    },
    /// An MCP server requested user elicitation.
    Elicitation {
        /// Name of the MCP server (wire `mcp_server_name`).
        server_name: String,
        /// Human-readable prompt shown to the user (wire `message`, required).
        message: String,
        /// Presentation mode (`form` / `url`), if specified.
        mode: Option<ElicitationMode>,
        /// URL to open when `mode == Url`, if specified.
        url: Option<String>,
        /// Server-assigned elicitation ID, if specified.
        elicitation_id: Option<String>,
        /// JSON Schema describing the requested form fields, if specified
        /// (wire `requested_schema`).
        requested_schema: Option<Value>,
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
        /// Which settings layer (or skills) changed (wire `source`, required).
        source: ConfigChangeSource,
        /// Path to the changed file, when known (wire `file_path`, optional).
        file_path: Option<PathBuf>,
    },
    /// A new git worktree was created.
    WorktreeCreate {
        /// Requested worktree name (wire `name`, required). This is the only
        /// field in the claude-code `WorktreeCreate` wire schema; the hook's
        /// stdout returns the resolved path, so the input carries just `name`.
        name: String,
        /// Absolute path of the new worktree (engine-side context, not on the
        /// wire).
        path: PathBuf,
        /// Branch name checked out in the worktree (engine-side context, not
        /// on the wire).
        branch: String,
    },
    /// An existing git worktree was removed.
    WorktreeRemove {
        /// Absolute path of the removed worktree.
        path: PathBuf,
    },
    /// A per-cwd / per-session instruction file was (re)loaded. Fired once per
    /// file (claude-code `executeInstructionsLoadedHooks`), so the payload
    /// carries a single `file_path` rather than a list.
    InstructionsLoaded {
        /// The instruction file that was loaded (wire `file_path`, required).
        file_path: PathBuf,
        /// Which memory tier the file belongs to (wire `memory_type`, required).
        memory_type: InstructionsMemoryType,
        /// Why the file was (re)loaded (wire `load_reason`, required).
        load_reason: InstructionsLoadReason,
        /// `paths:` frontmatter globs that gated the load, if any (optional).
        globs: Option<Vec<String>>,
        /// File whose access triggered a lazy/conditional load, if any
        /// (wire `trigger_file_path`, optional).
        trigger_file_path: Option<PathBuf>,
        /// Parent instruction file that `@include`d this one, if any
        /// (wire `parent_file_path`, optional).
        parent_file_path: Option<PathBuf>,
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

/// A `hook_progress` progress message emitted once per matching hook *before*
/// that hook executes, mirroring claude-code's progress yield
/// (`utils/hooks.ts:2094-2116`):
///
/// ```ts
/// yield { message: { type: 'progress', data: {
///   type: 'hook_progress', hookEvent, hookName, command: getHookDisplayText(hook),
///   ...(hook.type === 'prompt' && { promptText: hook.prompt }),
///   ...('statusMessage' in hook && hook.statusMessage != null &&
///     { statusMessage: hook.statusMessage }),
/// }, … } }
/// ```
///
/// The spinner consumes [`Self::status_message`] in place of the default
/// `Running {event} hook…` line when it is `Some`. This crate ORIGINATES the
/// event (carrying the per-hook `statusMessage` text from the definition); the
/// TUI render that swaps it into the spinner line is presentation work tracked
/// separately (see the `hook_progress` component).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookProgressEvent {
    /// The hook event name (e.g. `"PreToolUse"`) — claude-code `hookEvent`.
    pub hook_event: String,
    /// Human-readable hook name — claude-code `hookName`.
    pub hook_name: String,
    /// Per-hook spinner override text. `Some` only when the hook declared a
    /// `statusMessage`; `None` falls back to the engine's generic running
    /// line. Mirrors the conditional-spread of claude-code's `statusMessage`
    /// field — the key is present in the TS payload only when non-null, which
    /// `Option` models exactly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_message: Option<String>,
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

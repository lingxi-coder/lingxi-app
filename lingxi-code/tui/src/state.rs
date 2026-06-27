//! `AppState` — the root TUI's owned state.
//!
//! M6-02 establishes the shape; M6-03..M6-05 fill the reserved
//! `streaming` and `pending_permission` slots.
//!
//! ## Deviations from plan
//!
//! The plan declared `cost: Money` and `permission_mode: PermissionMode`
//! sourced from `lingxi-traits`. Neither type lives there in the actual
//! tree:
//!
//! - `Money` doesn't exist anywhere in `lingxi-*`; cost is tracked as
//!   `nano_usd: u64` plus an optional formatter (`nano_usd_to_dollars_format`).
//!   M6-02 surfaces cost as a pre-formatted `String` ("$0.0000" until M6-06).
//! - `PermissionMode` lives in `lingxi-permission` with variant `Default`
//!   (not `Normal`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use permission::gate::{PermissionRequest, PermissionResponse};
use permission::PermissionMode;
use protocol::ToolUseId;
use tokio::sync::oneshot;

use crate::components::prompt_input::completion::CompletionState;
use crate::components::prompt_input::palette::PaletteState;
use crate::theme::{theme_for, Theme, ThemeSetting};

/// A rendered message in the scrollback buffer.
#[derive(Debug, Clone)]
pub enum RenderedMessage {
    /// User-submitted prompt (rendered with `> ` prefix, default fg color).
    UserText {
        /// The text body the user submitted.
        body: String,
        /// Unix-seconds timestamp at the moment of render.
        timestamp: i64,
    },
    /// Assistant response (rendered with `● ` prefix, cyan).
    AssistantText {
        /// The assistant body text.
        body: String,
        /// Unix-seconds timestamp.
        timestamp: i64,
    },
    /// System message (slash command output, hints, errors). Rendered in
    /// dim grey or red depending on `is_error`.
    SystemText {
        /// The system message body.
        body: String,
        /// Unix-seconds timestamp.
        timestamp: i64,
        /// `true` → render red; `false` → render dim grey.
        is_error: bool,
    },
    /// Assistant tool-use block (M6-04). Renders as `● Tool({input_preview})`
    /// when collapsed, or as `● Tool(...)\n<pretty json>` when expanded.
    /// Per-id expanded state lives in `AppState.expanded`.
    AssistantToolUse {
        /// Correlator (model-supplied `tool_use_id`).
        id: protocol::ToolUseId,
        /// Tool name (e.g. `"Read"`, `"Bash"`).
        tool: String,
        /// JSON input the tool was invoked with.
        input: serde_json::Value,
    },
    /// User-side tool result (M6-04). Renders with `└ ` prefix and
    /// dim-colored body. Line/byte truncation (100 lines / 4000 bytes)
    /// kicks in when expanded; the collapsed form shows only the first
    /// line plus a `(+N lines)` suffix.
    UserToolResult {
        /// Correlator matching the paired `AssistantToolUse.id`.
        id: protocol::ToolUseId,
        /// Tool name (used to gate Bash → ANSI parser).
        tool: String,
        /// JSON result payload.
        result: serde_json::Value,
        /// (M7-02) Pre-edit text for diff tools (`old_string` for Edit; `None`
        /// for Write/pure-add). Populated from the paired `ToolUseStart` input
        /// at construction time; `None` for non-diff tools.
        old_string: Option<String>,
        /// (M7-02) Post-edit text for diff tools (`new_string` for Edit;
        /// `content` for Write). `None` for non-diff tools.
        new_string: Option<String>,
        /// (M7-02) Edited file path (drives diff syntax language). `None` for
        /// non-diff tools.
        file_path: Option<String>,
    },
    /// (M7-04) Assistant thinking block. Collapsed → `∴ Thinking` + expand hint;
    /// expanded → `∴ Thinking…` + markdown body. `expanded` mirrors
    /// `AppState.expanded`-style state (default false → collapsed).
    AssistantThinking {
        /// The thinking text (markdown when expanded).
        thinking: String,
        /// `true` → render the full markdown body; `false` → header + hint only.
        expanded: bool,
    },
    /// (M7-04) Redacted thinking. Single dim+italic line `✻ Thinking…`.
    AssistantRedactedThinking,
    /// (M7-04) Compaction boundary. REPLACES M6-08's `[Compacted …]` `SystemText`.
    /// Renders `✻ Conversation compacted (ctrl+o for history)` (dim). Counts are
    /// retained for telemetry/debug parity though the rendered line omits them
    /// (claude-code parity — the boundary line carries no numbers).
    CompactBoundary {
        /// Message count before compaction (debug/telemetry parity; not rendered).
        messages_before: u32,
        /// Message count after compaction (debug/telemetry parity; not rendered).
        messages_after: u32,
    },
    /// (M7-04) Level-aware system text. info → plain dim body; warning/error →
    /// `●` marker + colored body.
    SystemTextRich {
        /// Message body.
        body: String,
        /// Severity → marker/color.
        level: SystemLevel,
    },
    /// (M7-04) API error with retry countdown footer.
    SystemApiError {
        /// Formatted API error text.
        error: String,
        /// 1-based retry attempt.
        retry_attempt: u32,
        /// Seconds until the next retry.
        retry_in_seconds: u32,
        /// Max retry attempts.
        max_retries: u32,
        /// `true` → error body was clipped; append `…` + expand hint.
        truncated: bool,
    },
    /// (M7-04) Rate-limit notice (error text + optional dim upsell line).
    RateLimit {
        /// The rate-limit notice text (error-colored).
        text: String,
        /// Optional dim upsell line.
        upsell: Option<String>,
    },
    /// (M7-04) Teammate shutdown request/rejected notice.
    Shutdown {
        /// Originating teammate id.
        from: String,
        /// Optional reason.
        reason: Option<String>,
        /// `true` → rejected response; `false` → request.
        rejected: bool,
    },
    /// (M9-03) Task assignment notice — claude-code `TaskAssignmentMessage`.
    TaskAssignment {
        /// Task id, rendered as `#{task_id}`.
        task_id: String,
        /// Assigning agent's name.
        assigned_by: String,
        /// Task subject / title.
        subject: String,
        /// Optional task description.
        description: Option<String>,
    },
    /// (M9-03) Background-agent notification — claude-code
    /// `UserAgentNotificationMessage`. `status`: completed/failed/killed/other
    /// → marker color.
    AgentNotification {
        /// Summary line (empty → renders nothing).
        summary: String,
        /// Optional status string.
        status: Option<String>,
    },
    /// (M9-03) Inbound channel message — claude-code `UserChannelMessage`.
    ChannelMessage {
        /// Source server (raw; renderer takes the leaf after the last `:`).
        server: String,
        /// Optional sender user.
        user: Option<String>,
        /// Message content (renderer collapses whitespace + truncates to 60).
        content: String,
    },
    /// (M9-03) Teammate message — claude-code `UserTeammateMessage`
    /// (task-completed + plain-note sub-types).
    UserTeammate {
        /// Display name (`leader` or teammate id).
        display_name: String,
        /// Optional agent color name (→ `agent_color_from_name`).
        color: Option<String>,
        /// Sub-type payload.
        kind: UserTeammateKind,
    },
    /// (M7-04) Advisor block.
    Advisor {
        /// Advisor block content kind.
        kind: AdvisorKind,
        /// `true` → render the full result text (markdown).
        verbose: bool,
    },
    /// (M7-04) Hook-progress line.
    HookProgress {
        /// Hook event name (e.g. `"PreToolUse"`).
        event: String,
        /// In-progress hook count for this event.
        count: u32,
        /// `true` → static transcript summary; `false` → live running line.
        transcript_summary: bool,
    },
    /// (M7-04) Plan approval request/response.
    PlanApproval {
        /// Request/approved/rejected content.
        kind: PlanApprovalKind,
    },
    /// (M7-05) User bash-mode command line (`!` prefix). Body is the
    /// command text already extracted from the `<bash-input>` engine tag.
    UserBashInput {
        /// The command line the user typed in `!` bash mode.
        command: String,
    },
    /// (M7-05) Bash tool output. stdout/stderr already extracted from the
    /// `<bash-stdout>`/`<bash-stderr>` engine tags. Body carries ANSI SGR
    /// codes and is parsed through the ANSI parser at render time.
    UserBashOutput {
        /// Standard output (ANSI-coded).
        stdout: String,
        /// Standard error (ANSI-coded).
        stderr: String,
    },
    /// (M7-05) Slash-command echo. `❯ /{command} {args}` or `❯ Skill(name)`.
    UserCommand {
        /// Command name (without leading slash).
        command: String,
        /// Argument string (may be empty).
        args: String,
        /// `true` → render `Skill(name)` form instead of `/name args`.
        is_skill: bool,
    },
    /// (M7-05) Output of a local (slash) command. stdout/stderr already
    /// extracted; body rendered as markdown under a `  ⎿  ` gutter.
    UserLocalCommandOutput {
        /// Local-command stdout.
        stdout: String,
        /// Local-command stderr.
        stderr: String,
    },
    /// (M7-05) Memory write (`# {input}`) + a saving acknowledgement line.
    UserMemoryInput {
        /// Text the user added to memory (from `<user-memory-input>`).
        input: String,
    },
    /// (M7-05) Plan-mode plan body, rendered as bordered markdown under a
    /// "Plan to implement" header.
    UserPlan {
        /// Markdown plan content.
        plan_content: String,
    },
    /// (M7-05) A user prompt echoed into scrollback. Long bodies are
    /// head+tail truncated (claude-code parity, 10k char cap).
    UserPrompt {
        /// The prompt body text.
        text: String,
    },
    /// (M7-05) MCP resource/polling update lines (`↻ server: target · reason`).
    UserResourceUpdate {
        /// Parsed update triples (server, target, optional reason).
        updates: Vec<(String, String, Option<String>)>,
    },
    /// (M7-05) Image attachment placeholder. Terminal image protocols are M8;
    /// this renders `[Image #N]`/`[Image]` + optional metadata only.
    UserImage {
        /// Stored image id, if any (drives `#N` suffix).
        image_id: Option<u64>,
        /// Optional metadata suffix (dims/name) shown in parens.
        metadata: Option<String>,
    },
    /// (M7-05) A non-tool attachment summary line (directory listing, file
    /// read, PDF/resource reference, etc.). Carries the parsed attachment
    /// kind; team/swarm/hook kinds defer to M8.
    Attachment {
        /// The parsed attachment kind.
        attachment: crate::components::messages::attachment::Attachment,
    },
    /// (M7-05) A fold of consecutive same-tool tool-use blocks. Collapsed →
    /// `● {tool} (×N)`; expanded → header + each child input/result pair.
    /// `group_id` (the first child's id) keys `AppState.expanded`.
    GroupedToolUse {
        /// Shared tool name for the group.
        tool: String,
        /// First child's id — the per-group expanded-map key.
        group_id: protocol::ToolUseId,
        /// `(input, result)` pairs in group order.
        entries: Vec<(serde_json::Value, serde_json::Value)>,
    },
    /// (M7-05) A fold of Read/Search/List tool runs into one count summary.
    /// Scope: read/search/list counts (git/PR/bash/mcp/memory parts → M8).
    CollapsedReadSearch {
        /// Number of search (Grep/Glob) tool uses.
        search_count: u64,
        /// Number of file reads.
        read_count: u64,
        /// Number of directory listings.
        list_count: u64,
        /// `true` while the group is still streaming (present-tense verbs).
        is_active: bool,
        /// Expanded-map key (first child's id).
        group_id: protocol::ToolUseId,
        /// Per-entry display lines, shown when expanded.
        entries: Vec<String>,
        /// Team memories recalled (M9-03; data feed wired later).
        mem_read: u64,
        /// Team-memory searches (M9-03; data feed wired later).
        mem_search: u64,
        /// Team memories written (M9-03; data feed wired later).
        mem_write: u64,
    },
}

/// (M7-04) System message severity → marker/color mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemLevel {
    /// Plain dim body, no marker.
    Info,
    /// `●` marker + yellow body.
    Warning,
    /// `●` marker + red body.
    Error,
}

impl Default for SystemLevel {
    fn default() -> Self {
        Self::Info
    }
}

/// (M7-04) Advisor block content kinds (claude-code `AdvisorMessage` subtypes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdvisorKind {
    /// `Advising` header (+ optional model / input descriptor).
    ServerToolUse {
        /// Optional advising model name.
        model: Option<String>,
        /// Optional input descriptor.
        input: Option<String>,
    },
    /// Advisor reviewed-and-applied result; `text` is the full feedback.
    Result {
        /// The advisor feedback text (markdown when verbose).
        text: String,
    },
    /// Redacted result — no expandable body.
    RedactedResult,
    /// `Advisor unavailable ({error_code})`.
    Error {
        /// The error code reported by the advisor service.
        error_code: String,
    },
}

impl Default for AdvisorKind {
    fn default() -> Self {
        Self::RedactedResult
    }
}

/// (M9-03) `UserTeammate` sub-type payloads (claude-code
/// `UserTeammateMessage`). plan-approval/shutdown reuse the existing
/// `PlanApproval`/`Shutdown` variants; `idle_notification` is suppressed at
/// the drain (M9-06), so it has no payload here.
#[derive(Debug, Clone)]
pub enum UserTeammateKind {
    /// `✓ Completed task #{task_id}` + optional ` ({task_subject})`.
    TaskCompleted {
        /// Completed task id.
        task_id: String,
        /// Optional task subject.
        task_subject: Option<String>,
    },
    /// Plain teammate note: optional summary + optional full content
    /// (shown indented when `is_transcript_mode`).
    Note {
        /// Optional one-line summary.
        summary: Option<String>,
        /// Optional full content.
        content: Option<String>,
        /// `true` → show full content (transcript mode).
        is_transcript_mode: bool,
    },
}

/// (M7-04) Plan-approval request/response kinds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanApprovalKind {
    /// Approval request from a teammate.
    Request {
        /// Originating teammate id.
        from: String,
        /// Markdown plan content.
        plan_content: String,
        /// Optional plan file path.
        plan_file_path: Option<String>,
    },
    /// Approved by `{name}`.
    Approved {
        /// Approver display name.
        name: String,
    },
    /// Rejected by `{name}` with optional feedback.
    Rejected {
        /// Rejector display name.
        name: String,
        /// Optional feedback text.
        feedback: Option<String>,
    },
}

impl Default for PlanApprovalKind {
    fn default() -> Self {
        Self::Approved {
            name: String::new(),
        }
    }
}

/// Snapshot of the status-line fields. Recomputed once per frame.
#[derive(Debug, Clone)]
pub struct StatusSnapshot {
    /// Model display name (e.g. `"claude-sonnet-4.5"`).
    pub model: String,
    /// Current working directory.
    pub cwd: PathBuf,
    /// Pre-formatted cost ("$0.0000" until M6-06 wires real Money).
    pub cost: String,
    /// Context window utilisation in [0.0, 1.0]; rendered as `{:.0}%`.
    pub context_pct: f32,
    /// Active permission mode (rendered as short label).
    pub permission_mode: PermissionMode,
    /// (M7-11) MCP servers configured (any state). Consumed by the Doctor
    /// screen's `DoctorDiagnostics::capture`. Defaults to `0` until M6-07's
    /// orchestrator MCP counts are surfaced onto the TUI status (M8 wires
    /// auto-connect); the parent spec accepts "configured, not connected"/`0`
    /// for v0.8.0.
    pub mcp_configured: u32,
    /// (M7-11) MCP servers currently connected. Defaults to `0` (auto-connect
    /// is M8).
    pub mcp_connected: u32,
    /// (M7-11) Terminal size (cols, rows) at the moment a screen opens.
    /// Defaults to `(0, 0)`; the live mount may refresh it before opening a
    /// screen (M7-16 / M8).
    pub term_size: (u16, u16),
}

impl Default for StatusSnapshot {
    fn default() -> Self {
        Self {
            model: String::new(),
            cwd: PathBuf::from("."),
            cost: "$0.0000".to_string(),
            context_pct: 0.0,
            permission_mode: PermissionMode::Default,
            mcp_configured: 0,
            mcp_connected: 0,
            term_size: (0, 0),
        }
    }
}

/// Marker for a turn currently being driven. Reserved (no in-flight UI
/// yet in M6-02; spinner + streaming arrive in M6-03).
#[derive(Debug)]
pub struct TurnInFlight {
    /// Process-local monotonic id for this turn.
    pub turn_id: u64,
    /// Cancellation token threaded into the orchestrator.
    pub cancel: tokio_util::sync::CancellationToken,
}

/// Permission-prompt slot — populated in M6-05.
///
/// M6-03 reserved a placeholder shape `{tool, input}`. M6-05 promotes it
/// to carry the full [`PermissionRequest`] enum (which covers
/// `ToolUseConfirm`, `ExitPlanMode`, and `BypassPermissionsMode`).
#[derive(Debug, Clone)]
pub struct PendingPermission {
    /// The full request the orchestrator is awaiting an answer for.
    pub request: PermissionRequest,
    /// (M9-07) Worker identity when this request is worker-originated (TUI-side;
    /// the `PermissionRequest` enum is in frozen `traits/`). Fixture/test-set
    /// until the worker pool is live.
    pub worker: Option<crate::components::permissions::worker::WorkerPermissionInfo>,
}

impl PendingPermission {
    /// Convenience accessor — the tool name for `ToolUseConfirm`
    /// variants, or a synthetic descriptor for the other two variants.
    #[must_use]
    pub fn tool(&self) -> &str {
        match &self.request {
            PermissionRequest::ToolUseConfirm { tool_name, .. } => tool_name.as_str(),
            PermissionRequest::ExitPlanMode { .. } => "exit_plan_mode",
            PermissionRequest::BypassPermissionsMode => "bypass_permissions",
        }
    }
}

/// (TUI-PERM) Open the permission dialog for `request`, attaching `resp_tx`
/// (the oneshot back to the orchestrator's `TuiPermissionGate`; `None` for the
/// legacy bridge variant which resolves elsewhere). Sets `started_at` so the
/// resolved-telemetry `elapsed_ms` is non-zero, and resets the per-dialog
/// `ToolUseConfirm` state. Fires the `permission_dialog_shown` event.
///
/// NOTE: this helper currently assumes the `ToolUseConfirm` request variant —
/// it resets only `tool_use_dialog_state` and tags telemetry `"tool_use"`. Every
/// `PermissionExchange` today (gate + legacy bridge) is a `ToolUseConfirm`. If a
/// future caller pushes `ExitPlanMode`/`BypassPermissionsMode` requests through
/// this path, generalize the dialog-state reset + the telemetry kind here.
pub fn open_permission_dialog(
    st: &mut AppState,
    request: PermissionRequest,
    resp_tx: Option<oneshot::Sender<PermissionResponse>>,
    worker: Option<crate::components::permissions::worker::WorkerPermissionInfo>,
) {
    st.pending_permission = Some(PendingPermission { request, worker });
    st.pending_permission_resp_tx = resp_tx;
    st.pending_permission_started_at = Some(Instant::now());
    st.tool_use_dialog_state =
        crate::components::permissions::tool_use_confirm::ToolUseConfirmState::default();
    crate::telemetry::permission_dialog_shown("tool_use");
}

/// (TUI-PERM) If no dialog is active and the FIFO queue is non-empty, pop the
/// front exchange and open it. No-op when a dialog is already active (one
/// active dialog at a time) or the queue is empty.
pub fn promote_next_permission(st: &mut AppState) {
    if st.pending_permission.is_some() {
        return;
    }
    if let Some(exchange) = st.permission_queue.pop_front() {
        open_permission_dialog(
            st,
            exchange.request,
            Some(exchange.resp_tx),
            exchange.worker,
        );
    }
}

/// Per-turn streaming state. Created on `TurnStarted`, dropped on
/// `TurnEnded`. Currently carries only the start instant for debugging;
/// M6-04 may add a tool-use map.
#[derive(Debug, Clone)]
pub struct StreamingState {
    /// Wall-clock instant the turn began (for latency telemetry).
    pub started_at: Instant,
}

impl StreamingState {
    /// Mint a fresh streaming state at `Instant::now()`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            started_at: Instant::now(),
        }
    }
}

impl Default for StreamingState {
    fn default() -> Self {
        Self::new()
    }
}

/// Backwards-compat alias for M6-01/M6-02 imports. The canonical M6-03
/// name is `StreamingState`. Remove in M7 once downstream callers
/// migrate.
pub type StreamingTurn = StreamingState;

/// (SS-08) The session's currently-active todo, threaded into the streaming
/// spinner so its verb reflects what the leader is doing right now
/// (claude-code `Spinner.tsx:162` — `currentTodo = tasksV2?.find(t =>
/// t.status !== 'pending' && t.status !== 'completed')`). Populated from the
/// live `TodoWrite` tool input (see [`crate::streaming::apply_event`]); the
/// spinner's `leaderVerb` resolves `active_form ?? subject ?? random`
/// (`Spinner.tsx:169`). A minimal local mirror of the task tool's `TodoTask`
/// — only the two fields the spinner reads — so the TUI stays decoupled from
/// the `task` crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentTodo {
    /// The todo's imperative content (claude-code `subject`/`content`). Used
    /// as the spinner verb when `active_form` is absent.
    pub subject: String,
    /// The present-continuous form shown in the spinner while the todo is
    /// active (claude-code `activeForm`, e.g. "Running tests"). Preferred over
    /// `subject`.
    pub active_form: Option<String>,
}

/// Root TUI state. Owned by the `App` root component.
// Many independent UI flags map 1:1 to claude-code's React state fields; collapsing
// them into enums would diverge from the reference layout and churn the public API.
#[allow(clippy::struct_excessive_bools)]
pub struct AppState {
    /// Scrollback buffer. (M7-03) Full log retained — no eviction; the
    /// `VirtualMessageList` windows the viewport.
    pub messages: Vec<RenderedMessage>,
    /// Reserved for M6-05 (permission dialogs). Set on
    /// `TurnEvent::PermissionRequest` (M6-03) but not yet rendered.
    pub pending_permission: Option<PendingPermission>,
    /// `Some(_)` while a turn is streaming; `None` between turns.
    /// Spinner mount predicate. Set on `TurnEvent::TurnStarted`, cleared
    /// on `TurnEvent::TurnEnded(_)`.
    pub streaming: Option<StreamingState>,
    /// Cancel token threaded into `run_turn_streaming_with_cancel`.
    /// `Some(_)` mirrors `streaming.is_some()`. Reset to `None` once
    /// `TurnEnded` propagates through the bridge.
    pub cancel_token: Option<tokio_util::sync::CancellationToken>,
    /// Prompt buffer (UTF-8 string; cursor is a byte index).
    pub prompt_text: String,
    /// Cursor byte-index into `prompt_text`. Always at a char boundary.
    pub prompt_cursor: usize,
    /// History buffer (most recent at end). Drives Up/Down recall.
    pub history: Vec<String>,
    /// Current position in `history`. `None` means "drafting a new line".
    pub history_cursor: Option<usize>,
    /// Scroll position. `0` = bottom (latest); higher = older.
    pub scroll_offset: usize,
    /// (M7-03) Per-message rendered-height cache backing line-based scroll
    /// math. Rebuilt by [`Self::refresh_height_cache`] when the log grows
    /// or the viewport width changes.
    pub height_cache: crate::components::virtual_message_list::HeightCache,
    /// (M7-03) Viewport width (terminal columns) the cache was last built
    /// for. A change here triggers a recompute.
    pub viewport_width: usize,
    /// Status-line snapshot (model, cwd, cost, ctx%, mode).
    pub status: StatusSnapshot,
    /// (M7-15) Active resolved render palette. Read by `StatusLine`, message
    /// renderers, and diff coloring. Mutated only via [`AppState::set_theme`]
    /// (or, for live preview, the theme picker writes `theme` directly).
    pub theme: Theme,
    /// (M7-15) Stored theme *preference* (`auto` + 6 names). `Auto` resolves
    /// to a concrete `ThemeName` for `theme`. Persisted to `settings.json`.
    pub theme_setting: ThemeSetting,
    /// (theme-syntax-toggle) Session-level `syntaxHighlightingDisabled`
    /// (claude-code app state). Toggled by Ctrl+T in the theme picker; drives
    /// the picker's syntax-status line + preview. Like `theme_setting`, disk
    /// persistence is a separate concern.
    pub syntax_highlighting_disabled: bool,
    /// (SS-06) `prefersReducedMotion` setting (claude-code app state) — when
    /// `true`, the streaming spinner pins its glyph and stops animating.
    /// Loaded from settings.json at startup; defaults to `false`.
    pub reduced_motion: bool,
    /// (SS-08) The currently-active todo (first non-pending/non-completed
    /// item from the latest `TodoWrite`), or `None` when no todo is active.
    /// Drives the streaming spinner's verb (`active_form ?? subject ??`
    /// random), mirroring claude-code `Spinner.tsx:162`/`:169`. Updated by
    /// [`crate::streaming::apply_event`] on each `TodoWrite` tool call and
    /// cleared when a turn ends.
    pub current_todo: Option<CurrentTodo>,
    /// `Some` while a turn is being driven by the orchestrator.
    pub in_flight_turn: Option<TurnInFlight>,
    /// Timestamp of the first idle Ctrl-C/Ctrl-D press; cleared after
    /// [`crate::app::SIGINT_WINDOW_MS`].
    pub sigint_armed_at: Option<Instant>,
    /// (RRS-08) Which key armed [`Self::sigint_armed_at`] — `"Ctrl-C"` or
    /// `"Ctrl-D"` — so the footer's "Press {key} again to exit" hint
    /// (claude-code `exitMessage.key`) names the right key. Meaningless when
    /// `sigint_armed_at` is `None`.
    pub sigint_armed_key: &'static str,
    /// Set by `/exit` (or second Ctrl-C within the arming window).
    pub should_exit: bool,
    /// (M7-12) Set by the Resume screen on Enter: the session UUID the user
    /// chose. The CLI reads this after the TUI exits to load + resume it. The
    /// Resume screen also flips `should_exit` so the mount unwinds back to the
    /// CLI, which then surfaces / loads the chosen session.
    pub resume_request: Option<uuid::Uuid>,
    /// (M7-13) Set by the Settings screen's Config tab on the edit key
    /// (`e`/`Enter`): the bridge pump observes this flag, awaits
    /// `OrchestratorHandle::edit_config_file()` (the ONLY settings write the
    /// engine exposes — §4 R7), re-snapshots the open screen, then clears the
    /// flag. The actual async handoff + re-snapshot pump is wired by M7-16
    /// (the screen-lifecycle/command cluster); the synchronous key path only
    /// raises the request so we never `.await` in a render/key callback.
    pub pending_config_edit: bool,
    /// (M7-13 review) Set by the `OpenSettings` keybinding (Ctrl-G) or the
    /// `/config` / `/status` submit intercept: the tab the Settings screen
    /// should open on. The SYNC key/submit path can only RAISE the request —
    /// it can't open the screen because the open needs an async
    /// `SettingsData::snapshot(handle, eff)` read. The async open pump in
    /// `root.rs` (the ticker `use_future`, where the `OrchestratorHandle` +
    /// `state.lock().await` are available) observes this flag, builds the
    /// snapshot, and calls [`Self::open_settings`]. Guarded so it fires once
    /// per request and never opens over a pending permission or another open
    /// screen (priority order, parent spec §2.5).
    pub pending_open_settings: Option<crate::screens::settings::SettingsTab>,
    /// (MULTIMODAL.1) Set by the `dispatch(KeyAction::Submit)` real-prompt
    /// branch: the user line to run as a streaming turn. The SYNC submit path
    /// echoes the `UserText` + clears the prompt, but it CANNOT spawn the turn
    /// itself — that needs the `OrchestratorHandle` + the bridge sender, both
    /// reachable only from the ticker `use_future`. So it RAISES this flag and
    /// the async turn-spawn pump in `root.rs` (`pump_turn`, on the same 100ms
    /// ticker as the screen-open pumps) observes it, drains any pasted/dragged
    /// image paths (`PasteState::take_image_paths`), DROPS the lock, and calls
    /// `app::spawn_streaming_turn` — which forwards the paths to
    /// `OrchestratorHandle::run_turn_streaming_with_images`. Mirrors
    /// `pending_open_settings`. `None` between submits. A non-intercepted slash
    /// command sets [`Self::pending_slash`] instead (so the async pump can
    /// consult the dispatcher); plain text sets this.
    pub pending_turn: Option<String>,
    /// A user-typed slash command that the sync `Submit` arm did NOT intercept
    /// (e.g. `/loop`, Markdown/Plugin prompt commands, or an unknown command).
    /// The sync path holds no slash `dispatcher`, so it raises the raw line
    /// here; the async `root::pump_slash` dispatches it (RunAsTurn → run the
    /// expanded prompt; Handled/Unknown → display the text). `None` between
    /// submits.
    pub pending_slash: Option<String>,
    /// A user-typed `!`-prefixed bash-mode command line (the text AFTER the
    /// `!`). The sync `Submit` arm raises it here — it cannot `.await` the
    /// sandboxed Bash executor — and echoes the command as a `UserBashInput`
    /// row. The async `root::pump_bash` consumes the flag, runs the command
    /// through the host `BashRunner` (the SAME sandboxed `BashTool` the model
    /// uses), and folds the captured stdout/stderr into a `UserBashOutput` row.
    /// `None` between submits, and on mounts with no runner wired the flag is
    /// simply left/cleared (no LLM turn, no raw spawn).
    pub pending_bash: Option<String>,
    /// (M6-04) Currently focused tool block (Up/Down in scroll mode walks
    /// this through the `AssistantToolUse` entries in scrollback order).
    pub focused_tool_id: Option<ToolUseId>,
    /// (M6-04) Per-tool expanded state (default false → collapsed). Keyed
    /// by the `ToolUseId` carried on `AssistantToolUse` / `UserToolResult`
    /// entries.
    pub expanded: HashMap<ToolUseId, bool>,
    /// (M7-02) Tool-call inputs stashed by id when `ToolUseStart` arrives, so
    /// the LATER `ToolUseResult` can correlate the call input (Edit's
    /// `old_string`/`new_string`/`file_path`, Write's `content`) into the
    /// `UserToolResult` diff fields. Chosen over a backward scan of `messages`
    /// because M7-03 will window the visible message slice — a stash keyed by
    /// id is robust to that windowing.
    pub tool_call_inputs: HashMap<ToolUseId, serde_json::Value>,
    /// (Batch-3 Task 9) Text of the most recently rendered
    /// [`RenderedMessage::RateLimit`] notice. `apply_event` compares the
    /// freshly composed text against this before pushing, so identical
    /// consecutive rate-limit notices never stack in the scrollback (the
    /// orchestrator already emits on header CHANGE, but distinct header
    /// snapshots can compose to the same text — e.g. a utilization tick
    /// within the same floored percentage).
    pub last_rate_limit_text: Option<String>,
    /// (B5 Task 5) One-shot guard for the overage-transition notice
    /// (`useRateLimitWarningNotification.tsx` `hasShownOverageNotification`).
    /// Set when the notice fires; reset when a `RateLimit` event shows the
    /// session is NOT using overage (tsx :70-72) — NOT by `/clear` (the TS
    /// flag is component state, untouched by transcript clears).
    pub has_shown_overage_notification: bool,
    /// (M6-05) Oneshot back-channel to the orchestrator for the active
    /// permission round-trip. `Some(_)` whenever `pending_permission`
    /// holds a real request that arrived over the bridge; `None` for
    /// stub requests synthesized by `apply_event` (M6-03) or while no
    /// dialog is open.
    pub pending_permission_resp_tx: Option<oneshot::Sender<PermissionResponse>>,
    /// (M6-05) Instant the active dialog opened — used to compute
    /// `elapsed_ms` in the resolved-telemetry event.
    pub pending_permission_started_at: Option<Instant>,
    /// (TUI-PERM) FIFO of permission exchanges received from the
    /// `TuiPermissionGate` that have NOT yet been promoted into the single
    /// active dialog slot. The pump pushes here; `promote_next_permission`
    /// pops the front when `pending_permission` is free. Guarantees a second
    /// concurrent `gate.check()` never overwrites the active dialog's
    /// `resp_tx` (streaming dispatches tools concurrently).
    pub permission_queue: std::collections::VecDeque<crate::permission_bridge::PermissionExchange>,
    /// (M6-05) Per-dialog state for the `ToolUseConfirm` dialog.
    pub tool_use_dialog_state:
        crate::components::permissions::tool_use_confirm::ToolUseConfirmState,
    /// (M6-05) Per-dialog state for the `ExitPlanMode` dialog.
    pub exit_plan_dialog_state: crate::components::permissions::exit_plan_mode::ExitPlanModeState,
    /// (M6-05) Per-dialog state for the `BypassPermissionsMode` dialog.
    pub bypass_dialog_state:
        crate::components::permissions::bypass_permissions::BypassPermissionsState,
    /// (ARGS.3) Command name (without leading `/`) → declared positional
    /// argument names (TS `argNames`). Built once at TUI init from the command
    /// registry; only markdown/plugin commands with a frontmatter `arguments`
    /// list have a non-empty entry (every built-in is absent/empty). Drives the
    /// inline progressive argument-hint rendered by `PromptInput`
    /// (`progressive_argument_hint`), mirroring how claude-code's `useTypeahead`
    /// reads `exactMatch.argNames`. Empty map ⇒ no command ever shows the hint.
    pub command_argument_names: HashMap<String, Vec<String>>,
    /// (M7-07) `/` slash-command palette overlay state. `open == false`
    /// between uses; the live dispatcher routes keys here at priority 3.
    pub palette: PaletteState,
    /// (M7-07) `@` file-ref completion overlay state. Priority 3, same as
    /// the palette — only one can be open at a time (palette wins on `/`).
    pub completion: CompletionState,
    /// (M7-08) Vim mode enabled flag. Default `false` → M6 default editing
    /// is unchanged. Togglable via `KeyAction::ToggleVim`. Acceptable-as-bool
    /// for v0.8.0; wiring to the M3 settings store is a follow-up.
    pub vim_enabled: bool,
    /// (M7-08) PromptInput-local vim state (M7 design §2.3). Only consulted
    /// when `vim_enabled` is true.
    pub vim: crate::components::prompt_input::VimState,
    /// (M7-10) Active Ctrl-R history-search overlay. `Some(_)` means the
    /// overlay owns all live keys (parent spec §2.5 priority 3).
    pub history_search: Option<crate::components::prompt_input::HistorySearchState>,
    /// (M7-10) Paste attachment registry + next `[Image #N]` id.
    pub paste: crate::components::prompt_input::PasteState,
    /// (M7-11) Active full-page screen overlay. `None` ⇒ REPL is live.
    /// Routed at priority 2 in `handle_live_key` (after permission,
    /// before input). Reused by M7-12/13/14.
    ///
    /// (M7-11 review) Each `Screen` variant CARRIES its own per-screen state
    /// inline (e.g. `Screen::Doctor(DoctorDiagnostics)`), so there is no
    /// parallel top-level `Option<…State>` field per screen to keep in sync —
    /// `open_*` sets the variant, `close_screen` is a single `= None`.
    pub active_screen: Option<crate::screens::Screen>,
    /// (M7-14) Message search/jump overlay state. A priority-3 input overlay
    /// (mutually exclusive with the M7-07 palette/completion + M7-10
    /// history-search overlays); `open` ⇒ it owns all live keys.
    pub message_selector: crate::components::message_selector::MessageSelectorState,
    /// (M9-01) Multi-agent presentation state (tasks + workers). Mutated only
    /// by `multiagent::apply::apply_multiagent_event`. Renderers (M9-03+) read
    /// it; empty until a feed is mounted (M9-05).
    pub multiagent: crate::multiagent::MultiAgentState,
    /// (M9-06) When `Some`, the transcript is in teammate-view mode for this
    /// teammate name; `Esc` returns. Entry is programmatic until the worker
    /// feed is live (R4/R5).
    pub viewing_teammate: Option<String>,
    /// (`/compact`) Set by the `/compact` submit intercept: a request to run a
    /// forced compaction pass. The SYNC submit path can't `.await`
    /// `OrchestratorHandle::force_compact`, so it only RAISES this flag; the async
    /// pump in `root.rs` (`pump_compact`, on the ticker `use_future`) runs the
    /// compaction OUTSIDE the `AppState` lock and folds the `CompactionSummary`
    /// into a `RenderedMessage::CompactBoundary` (or, on error, an `is_error`
    /// `SystemText`). Mirrors `pending_open_settings` — handle-backed, so the
    /// pump runs only when an `OrchestratorHandle` is wired.
    pub pending_compact: bool,
    /// (M9-08) Set by the `/agents` submit intercept: a request to open the
    /// agent-discovery screen. The SYNC submit path can't `.await
    /// OrchestratorHandle::list_agents`, so it only RAISES this flag; the async
    /// open pump in `root.rs` (the ticker `use_future`) observes it, fetches the
    /// catalog, and calls [`Self::open_agents`]. Mirrors
    /// `pending_open_settings`.
    pub pending_open_agents: bool,
    /// (M9-10) Set by the `/stats` submit intercept: a request to open the
    /// usage-stats screen. The SYNC submit path can't `.await` the multi-project
    /// `*.jsonl` fs walk (slow over many files), so it only RAISES this flag;
    /// the async open pump in `root.rs` (`pump_open_stats`, on the ticker
    /// `use_future`) walks `<lingxi_home>/projects/` OUTSIDE the `AppState`
    /// lock, aggregates, and calls [`Self::open_stats`]. Mirrors
    /// `pending_open_agents` — but the walk needs no `OrchestratorHandle`, so
    /// the pump runs unconditionally (not gated on a wired handle).
    pub pending_open_stats: bool,
    /// (M9-09 real data) Set by the `/skills` submit intercept: a request to
    /// open the read-only skill-registry viewer. The SYNC submit path can't
    /// `.await` the on-disk `.lingxi/skills/` dir walk (project ancestors + user
    /// home), so it only RAISES this flag; the async open pump in `root.rs`
    /// (`pump_open_skills`, on the ticker `use_future`) walks the dirs OUTSIDE
    /// the `AppState` lock, parses each `SKILL.md`, and calls
    /// [`Self::open_skills`] with the grouped sections. Mirrors
    /// `pending_open_stats` — the walk needs no `OrchestratorHandle`, so the
    /// pump runs unconditionally (not gated on a wired handle). When no skills
    /// exist on disk the sections vec is empty → the locked `No skills found`
    /// empty state.
    pub pending_open_skills: bool,
    /// Set by the `/mcp` submit intercept: a request to open the read-only
    /// MCP-server viewer. The SYNC submit path can't `.await`
    /// `OrchestratorHandle::list_mcp_servers`, so it only RAISES this flag; the
    /// async open pump in `root.rs` (`pump_open_mcp`, on the ticker `use_future`)
    /// fetches the servers OUTSIDE the `AppState` lock and calls
    /// [`Self::open_mcp`]. Mirrors `pending_open_agents` (handle-backed, so the
    /// pump runs only when a handle is wired).
    pub pending_open_mcp: bool,
    /// Set by the `/hooks` submit intercept: a request to open the read-only
    /// hooks viewer. Mirrors `pending_open_mcp` — the SYNC submit path can't
    /// `.await` `OrchestratorHandle::list_hooks`, so it RAISES this flag and
    /// `root::pump_open_hooks` fetches + opens via [`Self::open_hooks`].
    pub pending_open_hooks: bool,
    /// Set by the `/permissions` submit intercept: a request to open the
    /// read-only permissions viewer. Off-disk like `pending_open_skills` (no
    /// handle); `root::pump_open_permissions` reads the settings tiers on the
    /// blocking pool and opens via [`Self::open_permissions`].
    pub pending_open_permissions: bool,
    /// Set by the `/model` submit intercept: a request to open the model picker.
    /// The SYNC submit path can't `.await` `list_available_models`, so it RAISES
    /// this flag; `root::pump_open_model` fetches the models + reads the current
    /// [`StatusSnapshot::model`] and opens via [`Self::open_model`]. Mirrors
    /// `pending_open_mcp` (handle-backed).
    pub pending_open_model: bool,
    /// Set by the model picker's Enter (a committed selection): the `(request_model,
    /// provider_id_as_profile)` pair to switch to. The SYNC key path can't `.await`
    /// `OrchestratorHandle::switch_model`, so the picker raises this and
    /// `root::pump_switch_model` performs the async write OUTSIDE the lock, then
    /// updates [`StatusSnapshot::model`] (success) or pushes an error `SystemText`
    /// (failure). `None` = no pending switch. The second tuple element is
    /// `Some(provider_id)` when the picker knows which provider owns the model (the
    /// normal path), `None` only when there is genuinely no provider context.
    pub pending_switch_model: Option<(String, Option<String>)>,
    /// (Plan 3c §8) Per-provider availability map, threaded engine→TUI from
    /// `DesktopRuntime.provider_availability` at mount (via
    /// [`Self::set_provider_availability`]). `true` = the provider has a usable
    /// credential and is routable; `false` = unconfigured (the `/model` picker
    /// badges the row and offers `/connect`). A MISSING key is treated as
    /// available (`true`) by [`crate::screens::model::build_model_entries`], so an
    /// empty map (the default, before the engine populates it) keeps every row
    /// available — byte-identical to the historical behavior.
    pub provider_availability: std::collections::BTreeMap<String, bool>,
    /// (Plan 3c I1/I2) Authoritative `request_model -> (profile_name,
    /// provider_label)` map, assembled engine-side from the LIVE multi-provider
    /// `ClientConfig.providers` and threaded onto the App from
    /// `DesktopRuntime.model_providers` at mount (via
    /// [`Self::set_model_providers`]). Joined by
    /// [`crate::screens::model::build_model_entries`] so a bare available-model id
    /// from a USER-defined provider (no `/`, no catalog listing) resolves to its
    /// OWN provider group + gates on `provider_availability` — instead of
    /// mis-falling into `"builtin"`/`true` (which suppressed the `[Connect]` badge
    /// and let an unconfigured provider's row route directly). Empty (the default,
    /// before the engine populates it) keeps the historical Built-in fallback.
    pub model_providers: std::collections::BTreeMap<String, (String, String)>,
    /// (Plan 3c §8) Set by the `/model` picker's `ModelOutcome::Connect` on an
    /// unconfigured row, or by a `/connect <provider>` prompt intercept: the
    /// provider id to connect. `root::pump_open_connect` consumes it (opening the
    /// `/connect` screen). `None` = no pending connect.
    pub pending_connect: Option<String>,
    /// (Plan 3c §8) Set by the `/connect` screen's `SubmitKey`:
    /// `(provider_id, key)` for the host to persist. `root::pump_store_provider_key`
    /// performs the keychain write through [`Self::provider_key_store`]. `None` =
    /// no pending key write.
    pub pending_store_key: Option<(String, String)>,
    /// (Plan 3c C1) Engine credential store, threaded from the shared
    /// `DesktopRuntime.credentials` `CredentialManager` so
    /// `root::pump_store_provider_key` can actually persist a key collected by the
    /// `/connect` screen (`set_provider_key(id, key)`). `None` on a headless /
    /// no-store build (smoke gates / tests) — the pump then keeps its no-op log
    /// and stores nothing, byte-identical to before the seam was wired.
    pub provider_key_store: Option<std::sync::Arc<secret::CredentialManager>>,
    /// (`/connect` Copilot device-flow) The engine GitHub-Copilot OAuth
    /// device-flow driver, threaded from `DesktopRuntime.connect_copilot`. The
    /// copilot-login task in `root` calls `begin()` (gets the user code +
    /// verification URL and opens the browser) then `poll_to_completion()`
    /// (polls GitHub and stores the OAuth token). `None` on a headless /
    /// no-driver build (smoke gates / tests) — the Copilot `/connect` screen
    /// then just shows "Requesting device code…" inertly.
    pub copilot_connect_driver: Option<std::sync::Arc<dyn command_core::CopilotConnectDriver>>,
    /// (`/connect` Copilot device-flow) Raised when the Copilot device-flow
    /// screen opens (Public or Enterprise) so the main loop fires the
    /// copilot-login task exactly once. Drained by [`Self::take_pending_copilot_login`].
    pub pending_copilot_login: bool,
    /// (`/connect` Copilot Enterprise) GitHub host the next copilot login runs
    /// against — `None` = `github.com` (Public), `Some("company.ghe.com")` =
    /// GitHub Enterprise (set by the deployment-type popup). Read by the
    /// copilot-login task when it calls `begin(domain)`.
    pub copilot_login_domain: Option<String>,
    /// (`/color`) Session agent-color name set by the `/color <name>` command
    /// (claude-code `standaloneAgentContext.color`). `Some("cyan")` after
    /// `/color cyan`; `None` after `/color default` (reset). Maps to a render
    /// color via [`crate::multiagent::style::agent_color_from_name`]. Not
    /// persisted here — the disk write is the separate `pending_save_color`
    /// pump. RENDER SINK: when `Some(name)`, this drives a full-width colored
    /// rule line (`SessionColorBanner`) directly ABOVE the prompt, tinted by the
    /// agent color — the standalone-agent branch of claude-code `useSwarmBanner`
    /// (`PromptInput.tsx:2250-2267`). `None` hides the banner (no row), matching
    /// claude-code's `color: undefined` → `return null`. Fed into `ReplScreenProps`
    /// alongside `viewing_teammate` (app.rs).
    pub session_agent_color: Option<String>,
    /// (`/color`) Set by the `/color` submit intercept: a request to PERSIST the
    /// chosen color to the session transcript (claude-code `saveAgentColor`).
    /// The SYNC submit path can't `.await` the disk append, so it only RAISES
    /// the string to write — a color name, or the `"default"` reset sentinel
    /// (NOT empty, mirroring claude-code's truthiness-guard rationale). The
    /// async pump in `root.rs` (`pump_save_color`, on the ticker `use_future`)
    /// resolves the transcript path and appends OUTSIDE the `AppState` lock,
    /// then clears the flag. Mirrors `pending_open_stats` (no handle needed).
    pub pending_save_color: Option<String>,
    /// (PERM-1) Set by the `/permissions` delete-confirmation: a rule the user
    /// confirmed deleting. The async `pump_permission_delete` removes it from
    /// settings.json (OUTSIDE the lock), reloads the rules, and re-renders the
    /// screen, then clears the flag. `None` when no delete is pending.
    pub pending_permission_delete: Option<crate::screens::permissions::PermRuleRow>,
    /// (PERM-1) Set by the `/permissions` add-rule input: a new rule the user
    /// submitted. `pump_permission_add` appends it to Local settings + reloads.
    pub pending_permission_add: Option<crate::screens::permissions::PermRuleRow>,
    /// (PERM-1 Workspace tab) Set by the `/permissions` Workspace-tab add /
    /// remove: `(directory, add)` — `add == true` appends the directory to
    /// Local settings' `additionalDirectories`, `false` removes it.
    /// `pump_workspace_dir` performs the write + reload, then clears it.
    pub pending_workspace_dir: Option<(String, bool)>,
    /// (cp-05) Cached recursive project-file listing for `@`-completion,
    /// keyed by cwd. Computed once per cwd (lazily, on first non-empty `@`
    /// partial) rather than per keystroke — `git ls-files` is fast but not
    /// free, and claude-code itself amortizes this via a background-refreshed
    /// index rather than re-walking on every keystroke.
    pub project_file_cache: Option<(std::path::PathBuf, Vec<String>)>,
    /// (BGTASK-3) Set by the `/tasks` dialog's `x`-stop key: the task id to
    /// kill. `pump_task_stop` calls the multiagent feed's `kill` OUTSIDE the
    /// `AppState` lock, then clears the flag. The next poll picks up the
    /// resulting status change — no manual refresh needed here.
    pub pending_task_stop: Option<String>,
    /// (`/copy`) Set by the `/copy [N]` submit intercept: a request to write
    /// the selected assistant text to the system clipboard (claude-code
    /// `commands/copy/copy.tsx` → `setClipboard`). The SYNC submit path can't
    /// `.await` (the iocraft reconciler owns stdout, so a native clipboard
    /// shell-out must run OUTSIDE the render frame), so it only RAISES the text
    /// to copy. The async pump in `root.rs` (`pump_copy_clipboard`, on the
    /// ticker `use_future`) writes it via the platform clipboard utility
    /// (`pbcopy` on macOS — claude-code's `copyNative` darwin path) OUTSIDE the
    /// `AppState` lock, then clears the flag. The user-facing confirmation
    /// `SystemText` is pushed SYNCHRONOUSLY by the intercept (clipboard writes
    /// are best-effort, exactly as claude-code's OSC-52 path is). Mirrors
    /// `pending_save_color` (no handle needed).
    pub pending_copy_clipboard: Option<String>,
    /// (#6 main-loop parity) An allowlisted terminal escape sequence a hook
    /// returned, staged by `apply_event` from `TurnEvent::TerminalSequence`. The
    /// async pump in `root.rs` (`pump_terminal_sequence`, on the ticker
    /// `use_future`) writes the bytes to stdout OUTSIDE the `AppState` lock, then
    /// clears the flag — the TUI owns the controlling terminal the orchestrator
    /// lacks (claude-code `BEo`). Already validated + BEL-normalized by the
    /// orchestrator. Mirrors `pending_copy_clipboard`.
    pub pending_terminal_sequence: Option<String>,
    /// (A6) Resolved custom status-line text — `Some(text)` when the user
    /// configured `statusLine: {type:'command'}` in settings AND the command
    /// ran successfully (already passed through
    /// [`crate::components::status_line::format_custom_status_line`]). `None`
    /// (the default) leaves the built-in `model cwd cost ctx% mode` row in
    /// place, byte-identical to the historical behavior. Populated by the async
    /// status-line pump (re-run when the cost/context/model snapshot changes,
    /// debounced like claude-code's status effect); rendered via the `custom`
    /// prop on [`crate::components::status_line::StatusLine`].
    pub status_line_text: Option<String>,
    /// (A6) Parsed `statusLine` setting (`{type, command, padding}`), or `None`
    /// when unset / not a `command` config. The async pump reads `command` +
    /// `padding` from here; the `padding` also feeds the `StatusLine` render
    /// prop so the custom row pads identically to claude-code's
    /// `<Box paddingX={paddingX}>`. Read once at startup from the settings JSON.
    pub status_line_config: Option<crate::components::status_line_command::StatusLineConfig>,
    /// (A6 batch-6 Task 2) Set `true` by `apply_event` on `TurnEvent::TurnEnded`
    /// to arm one statusline-pump pass (the TUI analog of claude-code's
    /// `StatusLine.tsx` re-run on `lastAssistantMessageId`). The 300ms-debounced
    /// pump in `root.rs` consumes (clears) the flag, builds the command payload,
    /// runs it off-thread, and re-paints when the text changes. A terminal 429
    /// emits `ClientEvent::Error` (not `TurnEnded`), so it deliberately does NOT
    /// re-arm the statusline — TS-faithful (M8). DEFERRED: TS also re-runs on
    /// `permissionMode` / `vimMode` / `mainLoopModel` change (`StatusLine.tsx:236`);
    /// the TUI analogs are deferred until those values mutate `AppState`
    /// (intentional, recorded in spec rev2.11) — not a missed requirement.
    pub status_line_dirty: bool,
    /// (Batch-5 Task 4) Latest raw per-window rate-limit utilization snapshot,
    /// written by `apply_event` on every `TurnEvent::RawUtilization`
    /// (last-write-wins, mirroring claude-code's per-response `rawUtilization`
    /// tracking in `claudeAiLimits.ts`). `None` until the first API response
    /// carries the `anthropic-ratelimit-unified-{5h,7d}-*` headers. Feeds the
    /// status-line command input's OPTIONAL `rate_limits` field — omitted
    /// when no window resolved (`StatusLine.tsx:99-101`).
    pub raw_utilization: Option<crate::components::status_line_command::RawUtilizationSnapshot>,
    /// (TokenWarning) The live context-pressure banner pushed by the
    /// orchestrator each turn (`OutputStream::emit_context_pressure`), or `None`
    /// when the context is below the warning threshold. Rendered above the
    /// prompt as claude-code's `<TokenWarning>` line.
    pub context_pressure: Option<traits::ContextPressureBanner>,
    /// Shared subscription slot from the composition root (None in tests /
    /// print mode). Read at rate-limit compose time via
    /// [`Self::subscription_snapshot`].
    pub subscription: Option<traits::subscription::SharedSubscription>,
    /// (GAP D) Runtime keybindings keymap — the merged default + user
    /// `~/.lingxi/keybindings.json` bindings the live dispatch consults BEFORE
    /// the hardcoded `map_iocraft_key` table. Defaults to
    /// [`command_core::keybindings::Keymap::defaults`] (byte-identical to the
    /// hardcoded chords); the composition root replaces it via
    /// [`Self::set_keymap`] once the customization gate + path are resolved at
    /// boot. When `resolve` returns nothing the dispatch falls through to the
    /// legacy table, so an action with no adapter mapping — or a user with no
    /// config — sees zero behavior change.
    pub keymap: command_core::keybindings::Keymap,
    /// (GAP D) Pending multi-keystroke chord state, threaded across keystrokes
    /// by the live dispatcher (the analogue of claude-code's `pendingChord`
    /// React ref). `None` = not mid-chord.
    pub pending_chord: command_core::keybindings::keymap::PendingChord,
}

impl AppState {
    /// Construct a fresh `AppState` with the given status snapshot.
    #[must_use]
    pub fn new(status: StatusSnapshot) -> Self {
        Self {
            messages: Vec::new(),
            pending_permission: None,
            streaming: None,
            cancel_token: None,
            prompt_text: String::new(),
            prompt_cursor: 0,
            history: Vec::new(),
            history_cursor: None,
            scroll_offset: 0,
            height_cache: crate::components::virtual_message_list::HeightCache::default(),
            viewport_width: 0,
            status,
            theme: theme_for(ThemeSetting::Auto.resolve()),
            theme_setting: ThemeSetting::Auto,
            syntax_highlighting_disabled: false,
            reduced_motion: false,
            current_todo: None,
            in_flight_turn: None,
            sigint_armed_at: None,
            sigint_armed_key: "Ctrl-C",
            should_exit: false,
            resume_request: None,
            pending_config_edit: false,
            pending_open_settings: None,
            pending_compact: false,
            pending_turn: None,
            pending_slash: None,
            pending_bash: None,
            focused_tool_id: None,
            expanded: HashMap::new(),
            tool_call_inputs: HashMap::new(),
            last_rate_limit_text: None,
            has_shown_overage_notification: false,
            pending_permission_resp_tx: None,
            pending_permission_started_at: None,
            permission_queue: std::collections::VecDeque::new(),
            tool_use_dialog_state:
                crate::components::permissions::tool_use_confirm::ToolUseConfirmState::default(),
            exit_plan_dialog_state:
                crate::components::permissions::exit_plan_mode::ExitPlanModeState::default(),
            bypass_dialog_state:
                crate::components::permissions::bypass_permissions::BypassPermissionsState::default(
                ),
            command_argument_names: HashMap::new(),
            palette: PaletteState::default(),
            completion: CompletionState::default(),
            vim_enabled: false,
            vim: crate::components::prompt_input::VimState::default(),
            history_search: None,
            paste: crate::components::prompt_input::PasteState::default(),
            active_screen: None,
            message_selector: crate::components::message_selector::MessageSelectorState::default(),
            multiagent: crate::multiagent::MultiAgentState::default(),
            viewing_teammate: None,
            pending_open_agents: false,
            pending_open_stats: false,
            pending_open_skills: false,
            pending_open_mcp: false,
            pending_open_hooks: false,
            pending_open_permissions: false,
            pending_open_model: false,
            pending_switch_model: None,
            provider_availability: std::collections::BTreeMap::new(),
            model_providers: std::collections::BTreeMap::new(),
            pending_connect: None,
            pending_store_key: None,
            provider_key_store: None,
            copilot_connect_driver: None,
            pending_copilot_login: false,
            copilot_login_domain: None,
            session_agent_color: None,
            pending_save_color: None,
            pending_permission_delete: None,
            pending_permission_add: None,
            pending_workspace_dir: None,
            project_file_cache: None,
            pending_task_stop: None,
            pending_copy_clipboard: None,
            pending_terminal_sequence: None,
            status_line_text: None,
            status_line_config: None,
            status_line_dirty: false,
            raw_utilization: None,
            context_pressure: None,
            subscription: None,
            // (GAP D) Defaults keymap — byte-identical to the hardcoded chords.
            // The composition root swaps in the user-config-merged keymap via
            // `set_keymap` once the gate + path are resolved at boot.
            keymap: command_core::keybindings::Keymap::defaults(),
            pending_chord: None,
        }
    }

    /// (GAP D) Install the runtime keymap (merged default + user keybindings).
    /// Called by the composition root after [`command_core::keybindings::load_keybindings`]
    /// resolves the customization gate + `~/.lingxi/keybindings.json`. Replacing
    /// the default keymap is a no-op behaviorally when the gate is off (the load
    /// returns the same defaults), so this never regresses the hardcoded chords.
    pub fn set_keymap(&mut self, keymap: command_core::keybindings::Keymap) {
        self.keymap = keymap;
    }

    /// Current resolved subscription snapshot, if the composition root provided
    /// a slot and the seed/background fetch has filled it. A poisoned lock
    /// degrades to `None` (conservative copy, never a panic in the render
    /// path) — the documented `SharedSubscription` reader stance.
    #[must_use]
    pub fn subscription_snapshot(&self) -> Option<traits::subscription::SubscriptionSnapshot> {
        self.subscription
            .as_ref()
            .and_then(|s| s.read().ok())
            .and_then(|guard| guard.clone())
    }

    /// (ARGS.3) Populate the command name → `argNames` lookup from a command
    /// registry, mirroring how claude-code's typeahead reads each command's
    /// `argNames`. Only commands that declare a non-empty list are stored, so the
    /// map stays empty in practice (every built-in declares none) and the inline
    /// progressive hint never renders for them. Called once at TUI init where the
    /// registry is available; idempotent (replaces the map).
    pub fn set_command_argument_names(&mut self, registry: &command_api::CommandRegistry) {
        self.command_argument_names = registry
            .list_all()
            .into_iter()
            .filter(|c| !c.argument_names.is_empty())
            .map(|c| (c.name.clone(), c.argument_names.clone()))
            .collect();
    }

    /// (ARGS.3) Compute the inline progressive argument-hint for the current
    /// prompt buffer (e.g. `"[arg2] [arg3]"`), or `None` when nothing should
    /// render. Delegates to the pure
    /// [`crate::components::prompt_input::progressive_argument_hint`] gate, fed by
    /// this state's `command_argument_names` lookup. `None` for every built-in
    /// (none declare `argNames`) and whenever the buffer is not a fully-typed
    /// slash command followed by a trailing space.
    #[must_use]
    pub fn prompt_argument_hint(&self) -> Option<String> {
        crate::components::prompt_input::progressive_argument_hint(&self.prompt_text, |name| {
            self.command_argument_names
                .get(name)
                .map(std::vec::Vec::as_slice)
        })
    }

    /// (M7-11) Open the Doctor screen, carrying the captured diagnostics
    /// inside the `Screen::Doctor` variant.
    pub fn open_doctor(&mut self, diag: crate::screens::doctor::DoctorDiagnostics) {
        self.active_screen = Some(crate::screens::Screen::Doctor(diag));
        crate::telemetry::screen_opened("doctor");
    }

    /// (M7-13) Open the Settings screen on a given tab with a pre-read data
    /// snapshot, carrying both inside the `Screen::Settings` variant. The data
    /// snapshot is read on the async open path (`SettingsData::snapshot`) so the
    /// render/key callbacks stay synchronous.
    pub fn open_settings(&mut self, state: crate::screens::settings::SettingsState) {
        self.active_screen = Some(crate::screens::Screen::Settings(state));
        crate::telemetry::screen_opened("settings");
    }

    /// (M7-14) Open the Memory file editor on a fresh selector, carrying the
    /// per-screen state inside the `Screen::Memory` variant. Unlike Settings
    /// (whose snapshot read is async), the Memory open is fully synchronous —
    /// the tier list is re-resolved each frame from `hierarchy::walk` and the
    /// body is loaded lazily on Enter — so this can be called directly from the
    /// sync key/submit path (no async open pump needed).
    pub fn open_memory(&mut self) {
        self.active_screen = Some(crate::screens::Screen::Memory(
            crate::screens::memory::MemoryScreenState::default(),
        ));
        crate::telemetry::screen_opened("memory");
    }

    /// (M7-15) Apply a theme preference: store it and resolve the active
    /// palette. The caller persists the choice separately (Task 9).
    pub fn set_theme(&mut self, setting: ThemeSetting) {
        self.theme_setting = setting;
        self.theme = theme_for(setting.resolve());
    }

    /// (M7-15) Open the theme picker focused on the currently-active setting,
    /// carrying the picker state inside the `Screen::Theme` variant. Like
    /// `/memory` (and unlike `/config`'s async snapshot) the open is fully
    /// synchronous — the theme registry is static — so this is callable
    /// directly from the sync `/theme` submit path.
    pub fn open_theme_picker(&mut self) {
        self.active_screen = Some(crate::screens::Screen::Theme(
            crate::screens::theme::ThemePickerState::new(self.theme_setting)
                .with_syntax_disabled(self.syntax_highlighting_disabled),
        ));
        crate::telemetry::screen_opened("theme");
    }

    /// (M9-08) Open the agents screen with the given catalog rows.
    pub fn open_agents(&mut self, mut rows: Vec<crate::screens::agents::AgentRow>) {
        // (agents-08) Store rows in grouped display order so the section
        // headers + selection index stay aligned (the render inserts a header
        // at each source-group boundary).
        crate::screens::agents::sort_into_group_order(&mut rows);
        self.active_screen = Some(crate::screens::Screen::Agents(
            crate::screens::agents::AgentsScreenState {
                rows,
                selected: 0,
                mode: crate::screens::agents::AgentsDialogMode::List,
            },
        ));
        crate::telemetry::screen_opened("agents");
    }

    /// Open the `/mcp` server viewer with the given rows. Called by
    /// `root::pump_open_mcp` after the async `list_mcp_servers` fetch.
    pub fn open_mcp(&mut self, rows: Vec<crate::screens::mcp::McpRow>) {
        self.active_screen = Some(crate::screens::Screen::Mcp(
            crate::screens::mcp::McpScreenState {
                rows,
                selected: 0,
                mode: crate::screens::mcp::McpDialogMode::List,
            },
        ));
        crate::telemetry::screen_opened("mcp");
    }

    /// Open the `/hooks` viewer with the given rows. Called by
    /// `root::pump_open_hooks` after the async `list_hooks` fetch.
    pub fn open_hooks(&mut self, rows: Vec<crate::screens::hooks::HookRow>) {
        self.active_screen = Some(crate::screens::Screen::Hooks(
            crate::screens::hooks::HooksScreenState {
                rows,
                selected: 0,
                mode: crate::screens::hooks::HooksDialogMode::EventList,
                selected_event: None,
            },
        ));
        crate::telemetry::screen_opened("hooks");
    }

    /// Open the `/permissions` viewer with the loaded state. Called by
    /// `root::pump_open_permissions` after the off-disk settings read.
    pub fn open_permissions(
        &mut self,
        state: crate::screens::permissions::PermissionsScreenState,
    ) {
        self.active_screen = Some(crate::screens::Screen::Permissions(state));
        crate::telemetry::screen_opened("permissions");
    }

    /// (Plan 3c §8) Install the engine-computed per-provider availability map.
    /// Threaded from `DesktopRuntime.provider_availability` at TUI init so the
    /// `/model` picker can badge unconfigured providers and offer `/connect`.
    /// Idempotent (replaces the map); an empty map keeps every row available.
    pub fn set_provider_availability(&mut self, map: std::collections::BTreeMap<String, bool>) {
        self.provider_availability = map;
    }

    /// (Plan 3c I1/I2) Install the engine-computed `request_model ->
    /// (profile_name, provider_label)` map (`DesktopRuntime.model_providers`) so
    /// the `/model` picker can resolve a bare USER-provider model id to its own
    /// group + availability gate. Threaded at TUI init, mirroring
    /// [`Self::set_provider_availability`]; idempotent (replaces the map). An empty
    /// map keeps the historical Built-in fallback for unmapped bare ids.
    pub fn set_model_providers(
        &mut self,
        map: std::collections::BTreeMap<String, (String, String)>,
    ) {
        self.model_providers = map;
    }

    /// (Plan 3c C1) Bind the shared engine credential store
    /// (`DesktopRuntime.credentials`) so `root::pump_store_provider_key` persists
    /// a key the `/connect` screen collected. `None` keeps the store unbound (the
    /// pump stays a no-op). Threaded at TUI init, mirroring
    /// [`Self::set_provider_availability`].
    pub fn set_provider_key_store(
        &mut self,
        store: Option<std::sync::Arc<secret::CredentialManager>>,
    ) {
        self.provider_key_store = store;
    }

    /// (`/connect` Copilot device-flow) Thread the engine GitHub-Copilot OAuth
    /// device-flow driver at TUI init (mirrors [`Self::set_provider_key_store`]).
    /// `None` (smoke gates / tests) leaves the copilot-login task inert.
    pub fn set_copilot_connect_driver(
        &mut self,
        driver: Option<std::sync::Arc<dyn command_core::CopilotConnectDriver>>,
    ) {
        self.copilot_connect_driver = driver;
    }

    /// (`/connect` Copilot device-flow) Take + clear [`Self::pending_copilot_login`].
    /// The main loop calls this right after `pump_open_connect` and, when `true`,
    /// signals the copilot-login task to run the device flow exactly once.
    pub fn take_pending_copilot_login(&mut self) -> bool {
        std::mem::take(&mut self.pending_copilot_login)
    }

    /// (Plan 3c §6.3) Open the `/connect` credential screen with the given flow
    /// state. Called by `root::pump_open_connect` after a picker `Connect` outcome
    /// or a `/connect <provider>` intercept raised `pending_connect`.
    pub fn open_connect(&mut self, state: crate::screens::connect::ConnectScreenState) {
        self.active_screen = Some(crate::screens::Screen::Connect(state));
        crate::telemetry::screen_opened("connect");
    }

    /// (GitHub Copilot Enterprise) Open the deployment-type sub-flow (Public vs
    /// Enterprise) shown when connecting GitHub Copilot. Resolving it opens the
    /// device-flow `Connect` screen with the chosen `copilot_login_domain`.
    pub fn open_github_deployment(&mut self) {
        self.active_screen = Some(crate::screens::Screen::GithubDeployment(
            crate::screens::github_deploy::GithubDeploymentState::default(),
        ));
        crate::telemetry::screen_opened("github_deployment");
    }

    /// Open the grouped bare-`/connect` provider PICKER. Called synchronously by
    /// the app.rs dispatch intercept for a bare `/connect` (no provider arg).
    /// Selecting a row raises `pending_connect` → `root::pump_open_connect` opens
    /// the key-entry [`Self::open_connect`] screen. Mirrors [`Self::open_model`].
    pub fn open_connect_picker(
        &mut self,
        state: crate::screens::connect_picker::ConnectPickerState,
    ) {
        self.active_screen = Some(crate::screens::Screen::ConnectPicker(state));
        crate::telemetry::screen_opened("connect_picker");
    }

    /// Open the grouped `/model` picker with the merged rows + recent keys + the
    /// active model. Called by `root::pump_open_model` after the async
    /// `list_available_models` / `list_model_listings` fetch (rows are built by
    /// [`crate::screens::model::build_model_entries`], joining the App's
    /// `provider_availability` + `model_providers` maps). `recent` is empty until
    /// recents persistence lands.
    pub fn open_model(
        &mut self,
        rows: Vec<crate::screens::model::ModelRow>,
        recent: Vec<(String, String)>,
        current: String,
    ) {
        let mut state = crate::screens::model::ModelScreenState::new(rows, recent, current);
        // (opencode-style curation) Curate the picker to the providers the user
        // has ACTUALLY configured (explicit `true` in the availability map);
        // providers absent from the map are the default-available catalog dump
        // and are hidden. Empty map → no curation (show-all), unchanged.
        let configured: std::collections::BTreeSet<String> = self
            .provider_availability
            .iter()
            .filter(|(_, &ok)| ok)
            .map(|(pid, _)| pid.clone())
            .collect();
        state.set_configured(configured);
        self.active_screen = Some(crate::screens::Screen::Model(state));
        crate::telemetry::screen_opened("model");
    }

    /// (M9-09) Open the `/skills` registry viewer with the given grouped
    /// sections. Called by `root::pump_open_skills` after the async on-disk
    /// `.lingxi/skills/` walk (the frozen `OrchestratorHandle` exposes no
    /// `list_skills`, so the TUI reads the dirs itself off the UI executor).
    /// The sections are already in claude-code render order (project, user)
    /// with empty sections omitted; when no skill exists on disk the vec is
    /// empty → the locked `No skills found` empty state.
    pub fn open_skills(&mut self, sections: Vec<crate::screens::skills::SkillSection>) {
        self.active_screen = Some(crate::screens::Screen::Skills(
            crate::screens::skills::SkillsState::new(sections),
        ));
        crate::telemetry::screen_opened("skills");
    }

    /// (M9-10) Open the `/stats` usage-stats screen in the LOADING state.
    /// The multi-project `*.jsonl` fs walk + aggregation can scan many GB across
    /// thousands of files, so it MUST run off the UI thread: `root::pump_open_stats`
    /// opens this loading screen for instant feedback, then runs the walk on the
    /// blocking pool and calls `StatsState::set_data` on the live screen when done.
    pub fn open_stats_loading(&mut self) {
        self.active_screen = Some(crate::screens::Screen::Stats(
            crate::screens::stats::StatsState::loading(),
        ));
        crate::telemetry::screen_opened("stats");
    }

    /// (M9-06) Enter teammate-view for `name`.
    pub fn enter_teammate_view(&mut self, name: String) {
        self.viewing_teammate = Some(name);
    }

    /// (M9-06) Leave teammate-view. Returns `true` if a view was active.
    pub fn leave_teammate_view(&mut self) -> bool {
        self.viewing_teammate.take().is_some()
    }

    /// (M7-11) Close any active screen, returning to the REPL. Generic — every
    /// screen's per-screen state lives INSIDE its `Screen` variant, so dropping
    /// `active_screen` clears it. Reused by every M7 screen's close path; no
    /// per-screen clear line to add as M7-12/13/14 land new variants.
    pub fn close_screen(&mut self) {
        // (M7-16) Emit `tengu_tui_screen_closed` only on a real `Some → None`
        // transition — calling `close_screen` with no active screen is a no-op
        // and must not mint a spurious event.
        if self.active_screen.is_some() {
            self.active_screen = None;
            crate::telemetry::screen_closed();
        }
    }

    /// All tool ids in scrollback order. Iterates `messages` once.
    fn tool_ids(&self) -> Vec<ToolUseId> {
        self.messages
            .iter()
            .filter_map(|m| match m {
                RenderedMessage::AssistantToolUse { id, .. } => Some(id.clone()),
                _ => None,
            })
            .collect()
    }

    /// (M6-04) Advance focus to the next tool block in scrollback order.
    /// First call with `focused_tool_id == None` focuses the first tool;
    /// past the end stays on the last (no wrap-around).
    pub fn focus_next_tool(&mut self) {
        let ids = self.tool_ids();
        if ids.is_empty() {
            return;
        }
        self.focused_tool_id = match &self.focused_tool_id {
            None => Some(ids[0].clone()),
            Some(cur) => {
                let pos = ids.iter().position(|i| i == cur).unwrap_or(0);
                let next = (pos + 1).min(ids.len() - 1);
                Some(ids[next].clone())
            }
        };
    }

    /// (M6-04) Step focus back through tool blocks. Past the start stays
    /// on the first.
    pub fn focus_prev_tool(&mut self) {
        let ids = self.tool_ids();
        if ids.is_empty() {
            return;
        }
        self.focused_tool_id = match &self.focused_tool_id {
            None => Some(ids[0].clone()),
            Some(cur) => {
                let pos = ids.iter().position(|i| i == cur).unwrap_or(0);
                let prev = pos.saturating_sub(1);
                Some(ids[prev].clone())
            }
        };
    }

    /// (M6-04) Flip the expanded state for the given tool id. Inserts the
    /// flipped value (default starts at `false`, first toggle → `true`).
    pub fn toggle_expanded(&mut self, id: &ToolUseId) {
        let entry = self.expanded.entry(id.clone()).or_insert(false);
        *entry = !*entry;
    }

    /// Construct a default `AppState` with an empty status snapshot.
    /// Used by tests that don't care about model/cwd/cost.
    #[must_use]
    pub fn default_for_tests() -> Self {
        Self::new(StatusSnapshot::default())
    }

    /// Push a message. The full log is retained (M7-03 `VirtualMessageList`
    /// windows the viewport — no FIFO eviction).
    pub fn push_message(&mut self, msg: RenderedMessage) {
        self.messages.push(msg);
    }

    /// Push a [`RenderedMessage::CompactBoundary`], de-duplicating consecutive
    /// boundaries (Gap #4).
    ///
    /// A single manual `/compact` can drive TWO push paths in the desktop
    /// build: [`crate::root::pump_compact`] folds the `force_compact` result
    /// into a boundary, AND the orchestrator emits
    /// `OutputEvent::CompactionCompleted`, which the desktop output bridge
    /// converts to `TurnEvent::CompactionCompleted` and the streaming handler
    /// also renders as a boundary. Both firing for one compaction would render
    /// the `✻ Conversation compacted (ctrl+o for history)` marker TWICE.
    ///
    /// Collapsing consecutive boundaries yields exactly ONE marker regardless
    /// of which path(s) fire — and is context-independent: in unit tests where
    /// the mock `force_compact` does NOT emit the event, only `pump_compact`
    /// pushes (→ 1); in production both the bridge and `pump_compact` push
    /// (→ still 1). Two genuinely distinct compactions are always separated by
    /// the intervening conversation, so they never collapse.
    pub fn push_compact_boundary(&mut self, messages_before: u32, messages_after: u32) {
        if matches!(
            self.messages.last(),
            Some(RenderedMessage::CompactBoundary { .. })
        ) {
            return;
        }
        self.messages.push(RenderedMessage::CompactBoundary {
            messages_before,
            messages_after,
        });
    }

    /// Seed the scrollback from a RESUMED session's prior conversation, before
    /// the first render. `msgs` are the persisted turns already mapped to
    /// scrollback rows (via [`crate::replay::rebuild_messages`]); they REPLACE
    /// the (empty) initial `messages` so the resumed history shows on the very
    /// first frame — the analog of claude-code's REPL `initialMessages`.
    ///
    /// Also re-seeds the per-tool `expanded` map to the collapsed default for
    /// each replayed tool block, so the resumed tool-use/result rows render
    /// collapsed exactly like a freshly-streamed one (the map is keyed by
    /// `ToolUseId`; absent keys already default to collapsed, so this only
    /// makes the intent explicit and keeps focus-walk ids discoverable).
    ///
    /// The height cache is intentionally NOT recomputed here — it is rebuilt
    /// lazily on the first render by [`Self::refresh_height_cache`] with the
    /// real terminal width (computing it now with width `0` would just be
    /// thrown away). A FRESH session never calls this, so its first frame is
    /// byte-identical to today.
    pub fn seed_resumed_messages(&mut self, msgs: Vec<RenderedMessage>) {
        self.messages = msgs;
    }

    /// (M7-03) Rebuild the height cache if the log or `width` changed.
    /// Idempotent: a no-op when nothing changed (cheap len + width check).
    pub fn refresh_height_cache(&mut self, width: usize) {
        let stale = self.viewport_width != width || self.height_cache.len() != self.messages.len();
        if stale {
            self.height_cache.recompute(&self.messages, width);
            self.viewport_width = width;
        }
    }
}

/// (A6 batch-6 Task 2) Build the `(command, stdin-json)` pair for the
/// statusline pump, or `None` when the pump should not run. Pure — the render
/// loop calls this under the lock, then drops the lock before spawning the
/// command off-thread.
///
/// Returns `None` when no `statusLine` config is armed or `should_run` rejects
/// it. The JSON payload mirrors `buildStatusLineCommandInput` (the documented
/// subset — see [`crate::components::status_line_command`]).
#[must_use]
pub fn build_pump_payload(state: &AppState) -> Option<(String, String)> {
    use crate::components::status_line_command::{build_status_line_input, parse_cost_usd};

    let cfg = state.status_line_config.as_ref()?;
    // trusted=true: claude-code gates the statusline on the trust DIALOG
    // (hooks.ts:286-296 shouldSkipHookDueToTrust); lingxi has no
    // hasTrustDialogAccepted port — the hooks executor takes the same
    // upstream-trust stance (hooks/src/executor.rs:457). `should_run` keeps its
    // fail-closed `trusted` parameter for when a trust store lands.
    if !cfg.should_run(true) {
        return None;
    }
    // StatusSnapshot has one model string + cwd only — reuse `model` for
    // id+display and `cwd` for current_dir+project_dir (documented divergence);
    // `added_dirs` is not tracked in TUI state → empty.
    let json = build_status_line_input(
        &state.status.model,
        &state.status.model,
        &state.status.cwd,
        &state.status.cwd,
        &[],
        env!("CARGO_PKG_VERSION"),
        parse_cost_usd(&state.status.cost),
        state.status.context_pct,
        state.raw_utilization.as_ref(),
    );
    Some((cfg.command.clone(), json.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_status() -> StatusSnapshot {
        StatusSnapshot {
            model: "claude-sonnet-4.5".into(),
            cwd: PathBuf::from("/a/b"),
            cost: "$0.0000".to_string(),
            context_pct: 0.42,
            permission_mode: PermissionMode::Default,
            ..StatusSnapshot::default()
        }
    }

    #[test]
    fn app_state_carries_theme_and_set_theme_applies() {
        use crate::theme::{Theme, ThemeName, ThemeSetting};
        let mut s = AppState::new(fake_status());
        // Default: auto → resolves dark.
        assert_eq!(s.theme_setting, ThemeSetting::Auto);
        assert_eq!(s.theme, Theme::dark());
        // Switching applies both fields.
        s.set_theme(ThemeSetting::Named(ThemeName::Light));
        assert_eq!(s.theme_setting, ThemeSetting::Named(ThemeName::Light));
        assert_eq!(s.theme, Theme::light());
    }

    #[test]
    fn active_screen_defaults_none_and_open_close_toggles() {
        use crate::screens::doctor::DoctorDiagnostics;
        use crate::screens::Screen;
        let mut st = AppState::default_for_tests();
        assert_eq!(st.active_screen, None, "REPL is live by default");
        st.open_doctor(DoctorDiagnostics::capture(
            std::path::Path::new("/work"),
            0,
            0,
            (80, 24),
        ));
        // (M7-11 review) The Doctor variant carries its diagnostics inline.
        assert!(
            matches!(&st.active_screen, Some(Screen::Doctor(diag)) if diag.cwd == "/work"),
            "open carries captured diagnostics inside the variant"
        );
        st.close_screen();
        assert_eq!(
            st.active_screen, None,
            "close_screen returns to REPL (generic — drops the variant + its state)"
        );
    }

    #[test]
    fn open_memory_sets_active_screen_and_close_clears() {
        use crate::screens::Screen;
        let mut st = AppState::default_for_tests();
        assert_eq!(st.active_screen, None);
        st.open_memory();
        // (M7-14) The Memory variant carries its (default) editor state inline.
        assert!(
            matches!(&st.active_screen, Some(Screen::Memory(ms)) if !ms.editing && ms.selected == 0),
            "open_memory carries a fresh MemoryScreenState inside the variant"
        );
        st.close_screen();
        assert_eq!(st.active_screen, None);
    }

    /// (B4 Task 5) The composition-root subscription slot defaults to `None`
    /// (tests / print mode), and once a filled slot is attached,
    /// `subscription_snapshot` reads the snapshot through the shared lock.
    #[test]
    fn app_state_subscription_defaults_none_and_snapshot_reads_through() {
        let mut st = AppState::new(StatusSnapshot::default());
        assert!(st.subscription_snapshot().is_none());
        let slot: traits::subscription::SharedSubscription =
            std::sync::Arc::new(std::sync::RwLock::new(Some(
                traits::subscription::SubscriptionSnapshot {
                    is_subscriber: true,
                    ..Default::default()
                },
            )));
        st.subscription = Some(slot);
        assert!(st.subscription_snapshot().expect("snap").is_subscriber);
    }

    // ── (A6 batch-6 Task 2) build_pump_payload ─────────────────────────────

    use crate::components::status_line_command::{RawUtilizationSnapshot, StatusLineConfig};

    /// No `statusLine` config → the pump produces nothing.
    #[test]
    fn build_pump_payload_none_when_no_config() {
        let st = AppState::new(fake_status());
        assert!(st.status_line_config.is_none());
        assert!(build_pump_payload(&st).is_none());
    }

    /// A config present but not `should_run(true)` (non-`command` kind) → None.
    #[test]
    fn build_pump_payload_none_when_should_not_run() {
        let mut st = AppState::new(fake_status());
        // A `static`-typed config never runs (should_run gates type==command).
        st.status_line_config = StatusLineConfig::from_settings_value(
            &serde_json::json!({"type": "static", "command": "echo hi"}),
        );
        assert!(st.status_line_config.is_some());
        assert!(build_pump_payload(&st).is_none());
    }

    /// An armed `command` config → `Some((command, json))` carrying the
    /// stdin payload. `rate_limits` is OMITTED here (no raw utilization).
    #[test]
    fn build_pump_payload_some_for_armed_command_omits_rate_limits_without_windows() {
        let mut st = AppState::new(fake_status());
        st.status_line_config = StatusLineConfig::from_settings_value(
            &serde_json::json!({"type": "command", "command": "echo hi"}),
        );
        let (command, json) = build_pump_payload(&st).expect("armed config produces a payload");
        assert_eq!(command, "echo hi");
        let v: serde_json::Value = serde_json::from_str(&json).expect("stdin json parses");
        // Model + cwd reuse (StatusSnapshot has one model string + cwd only).
        assert_eq!(v["model"]["id"], "claude-sonnet-4.5");
        assert_eq!(v["model"]["display_name"], "claude-sonnet-4.5");
        assert_eq!(v["workspace"]["current_dir"], "/a/b");
        assert_eq!(v["workspace"]["project_dir"], "/a/b");
        assert_eq!(v["hook_event_name"], "Status");
        // No window resolved → rate_limits omitted (StatusLine.tsx:99-101).
        assert!(
            v.get("rate_limits").is_none(),
            "rate_limits absent when no window resolved: {v:?}"
        );
    }

    /// The payload carries `rate_limits` ONLY when `raw_utilization` has a
    /// fully-resolved window (both utilization AND `resets_at` Some) — the
    /// `build_status_line_input` conditional, asserted through the pump wiring.
    #[test]
    fn build_pump_payload_includes_rate_limits_when_window_resolved() {
        let mut st = AppState::new(fake_status());
        st.status_line_config = StatusLineConfig::from_settings_value(
            &serde_json::json!({"type": "command", "command": "echo hi"}),
        );
        st.raw_utilization = Some(RawUtilizationSnapshot {
            five_hour_utilization: Some(0.42),
            five_hour_resets_at: Some(1_750_000_000),
            seven_day_utilization: None,
            seven_day_resets_at: None,
        });
        let (_command, json) = build_pump_payload(&st).expect("armed config produces a payload");
        let v: serde_json::Value = serde_json::from_str(&json).expect("stdin json parses");
        let rl = v["rate_limits"].as_object().expect("rate_limits present");
        assert!((rl["five_hour"]["used_percentage"].as_f64().unwrap() - 42.0).abs() < 1e-9);
        assert_eq!(rl["five_hour"]["resets_at"].as_u64(), Some(1_750_000_000));
        assert!(rl.get("seven_day").is_none(), "partial window omitted");
    }

    /// A window with only utilization (no `resets_at`) is NOT fully resolved →
    /// `rate_limits` stays omitted.
    #[test]
    fn build_pump_payload_omits_rate_limits_for_partial_window() {
        let mut st = AppState::new(fake_status());
        st.status_line_config = StatusLineConfig::from_settings_value(
            &serde_json::json!({"type": "command", "command": "echo hi"}),
        );
        st.raw_utilization = Some(RawUtilizationSnapshot {
            five_hour_utilization: Some(0.42),
            five_hour_resets_at: None,
            seven_day_utilization: None,
            seven_day_resets_at: None,
        });
        let (_command, json) = build_pump_payload(&st).expect("payload");
        let v: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert!(
            v.get("rate_limits").is_none(),
            "partial window (no resets_at) omits rate_limits: {v:?}"
        );
    }

    #[test]
    fn vim_defaults_off_and_insert() {
        let s = AppState::new(StatusSnapshot::default());
        assert!(!s.vim_enabled);
        assert_eq!(s.vim.mode, crate::components::prompt_input::VimMode::Insert);
    }

    /// (ARGS.3) A registry seeded with a synthetic markdown command that
    /// declares `argument_names` makes `prompt_argument_hint` render the inline
    /// progressive hint after "/<cmd> ", consuming one arg per typed token; a
    /// built-in (no argNames) and a non-command buffer render nothing.
    #[test]
    fn prompt_argument_hint_renders_for_command_with_argnames() {
        use command_api::model::{CommandSource, SlashCommand, SlashCommandKind};
        use command_api::CommandRegistry;

        let mut reg = CommandRegistry::new();
        // Synthetic markdown command WITH declared argNames (does not depend on
        // any real built-in declaring them).
        reg.register_command(SlashCommand {
            name: "deploy".to_string(),
            description: "Deploy".to_string(),
            source: CommandSource::Project,
            kind: SlashCommandKind::Markdown {
                file_path: std::path::PathBuf::from("/x/deploy.md"),
                frontmatter: command_api::model::CommandFrontmatter::default(),
                prompt_template: String::new(),
            },
            argument_names: vec!["env".to_string(), "region".to_string()],
            ..SlashCommand::default()
        });
        // A built-in with NO argNames must never surface a hint.
        reg.register_command(SlashCommand {
            name: "help".to_string(),
            description: "Help".to_string(),
            ..SlashCommand::default()
        });

        let mut st = AppState::new(fake_status());
        st.set_command_argument_names(&reg);
        // Only the command with argNames made it into the map.
        assert_eq!(st.command_argument_names.len(), 1);
        assert!(st.command_argument_names.contains_key("deploy"));

        // "/deploy " → both names remain.
        st.prompt_text = "/deploy ".to_string();
        assert_eq!(
            st.prompt_argument_hint(),
            Some("[env] [region]".to_string())
        );
        // One arg typed → consumes the first name.
        st.prompt_text = "/deploy prod ".to_string();
        assert_eq!(st.prompt_argument_hint(), Some("[region]".to_string()));
        // Both filled → nothing.
        st.prompt_text = "/deploy prod us-east ".to_string();
        assert_eq!(st.prompt_argument_hint(), None);
        // No trailing space (still in palette / mid-arg) → nothing.
        st.prompt_text = "/deploy".to_string();
        assert_eq!(st.prompt_argument_hint(), None);
        // Built-in without argNames → nothing.
        st.prompt_text = "/help ".to_string();
        assert_eq!(st.prompt_argument_hint(), None);
        // Empty map (default state) → nothing for anything.
        let mut bare = AppState::new(fake_status());
        bare.prompt_text = "/deploy ".to_string();
        assert_eq!(bare.prompt_argument_hint(), None);
    }

    #[test]
    fn push_retains_full_log_no_eviction() {
        let mut s = AppState::new(fake_status());
        for i in 0..5000 {
            s.push_message(RenderedMessage::UserText {
                body: format!("msg{i}"),
                timestamp: 0,
            });
        }
        // Full retention: every message is kept, in order.
        assert_eq!(s.messages.len(), 5000);
        match &s.messages[0] {
            RenderedMessage::UserText { body, .. } => assert_eq!(body, "msg0"),
            _ => panic!("wrong variant"),
        }
        match &s.messages[4999] {
            RenderedMessage::UserText { body, .. } => assert_eq!(body, "msg4999"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn height_cache_refresh_tracks_messages_and_width() {
        use crate::components::virtual_message_list::HeightCache;
        let mut s = AppState::new(fake_status());
        s.push_message(RenderedMessage::UserText {
            body: "a\nb".into(),
            timestamp: 0,
        });
        s.push_message(RenderedMessage::UserText {
            body: "c".into(),
            timestamp: 0,
        });
        s.refresh_height_cache(80);
        assert_eq!(s.height_cache.total_lines(), 3); // 2 + 1
        assert_eq!(s.viewport_width, 80);
        // Width change recomputes.
        s.push_message(RenderedMessage::UserText {
            body: "x".repeat(20),
            timestamp: 0,
        });
        s.refresh_height_cache(10); // "xxxxxxxxxxxxxxxxxxxx" → ceil(20/10)=2
        assert_eq!(s.height_cache.total_lines(), 5); // 2 + 1 + 2
        let _ = HeightCache::default(); // type is reachable
    }

    #[test]
    fn new_is_empty_and_at_bottom() {
        let s = AppState::new(fake_status());
        assert!(s.messages.is_empty());
        assert_eq!(s.scroll_offset, 0);
        assert_eq!(s.prompt_cursor, 0);
        assert!(!s.should_exit);
    }

    #[test]
    fn new_state_has_closed_palette_and_completion() {
        let s = AppState::new(fake_status());
        assert!(!s.palette.open);
        assert!(s.palette.filter.is_empty());
        assert!(!s.completion.open);
        assert!(s.completion.filter.is_empty());
    }

    /// M6-04 Task 2: `RenderedMessage` gains `AssistantToolUse` and
    /// `UserToolResult` variants so the scrollback can carry rich tool
    /// blocks (not the M6-03 `SystemText` placeholders).
    #[test]
    fn rendered_message_carries_tool_use_and_result() {
        use protocol::ToolUseId;
        let id = ToolUseId::new();
        let call = RenderedMessage::AssistantToolUse {
            id: id.clone(),
            tool: "Read".into(),
            input: serde_json::json!({"file_path": "/tmp/x.rs"}),
        };
        let result = RenderedMessage::UserToolResult {
            id,
            tool: "Read".into(),
            result: serde_json::json!({"content": "fn main() {}"}),
            old_string: None,
            new_string: None,
            file_path: None,
        };
        assert!(matches!(call, RenderedMessage::AssistantToolUse { .. }));
        assert!(matches!(result, RenderedMessage::UserToolResult { .. }));
    }

    /// M6-04 Task 9: focus walks through `AssistantToolUse` entries in
    /// scrollback order; past either end stays put (no wrap-around).
    #[test]
    fn focus_walks_through_tool_calls_in_order() {
        use protocol::ToolUseId;
        let mut st = AppState::new(fake_status());
        let a = ToolUseId::new();
        let b = ToolUseId::new();
        st.push_message(RenderedMessage::UserText {
            body: "hi".into(),
            timestamp: 0,
        });
        st.push_message(RenderedMessage::AssistantToolUse {
            id: a.clone(),
            tool: "Read".into(),
            input: serde_json::json!({}),
        });
        st.push_message(RenderedMessage::AssistantText {
            body: "ok".into(),
            timestamp: 0,
        });
        st.push_message(RenderedMessage::AssistantToolUse {
            id: b.clone(),
            tool: "Bash".into(),
            input: serde_json::json!({}),
        });
        assert_eq!(st.focused_tool_id, None);
        st.focus_next_tool();
        assert_eq!(st.focused_tool_id, Some(a.clone()));
        st.focus_next_tool();
        assert_eq!(st.focused_tool_id, Some(b.clone()));
        st.focus_next_tool(); // past end — stays on last
        assert_eq!(st.focused_tool_id, Some(b.clone()));
        st.focus_prev_tool();
        assert_eq!(st.focused_tool_id, Some(a.clone()));
        st.focus_prev_tool(); // past start — stays on first
        assert_eq!(st.focused_tool_id, Some(a.clone()));
    }

    /// M7-04 Task 1: `RenderedMessage` carries the 10 batch-1 variants.
    #[test]
    fn rendered_message_carries_batch1_variants() {
        let variants = [
            RenderedMessage::AssistantThinking {
                thinking: "x".into(),
                expanded: false,
            },
            RenderedMessage::AssistantRedactedThinking,
            RenderedMessage::CompactBoundary {
                messages_before: 50,
                messages_after: 5,
            },
            RenderedMessage::SystemTextRich {
                body: "hi".into(),
                level: SystemLevel::Warning,
            },
            RenderedMessage::SystemApiError {
                error: "boom".into(),
                retry_attempt: 4,
                retry_in_seconds: 3,
                max_retries: 10,
                truncated: false,
            },
            RenderedMessage::RateLimit {
                text: "limited".into(),
                upsell: None,
            },
            RenderedMessage::Shutdown {
                from: "agent-1".into(),
                reason: Some("done".into()),
                rejected: false,
            },
            RenderedMessage::Advisor {
                kind: AdvisorKind::Result { text: "ok".into() },
                verbose: false,
            },
            RenderedMessage::HookProgress {
                event: "PreToolUse".into(),
                count: 2,
                transcript_summary: false,
            },
            RenderedMessage::PlanApproval {
                kind: PlanApprovalKind::Approved { name: "you".into() },
            },
        ];
        assert_eq!(variants.len(), 10);
        assert!(variants
            .iter()
            .any(|m| matches!(m, RenderedMessage::CompactBoundary { .. })));
    }

    /// `toggle_expanded` flips the per-id boolean, defaulting to `true`
    /// on the first toggle.
    #[test]
    fn toggle_expanded_flips_per_id() {
        use protocol::ToolUseId;
        let mut st = AppState::new(fake_status());
        let id = ToolUseId::new();
        assert!(!st.expanded.contains_key(&id));
        st.toggle_expanded(&id);
        assert_eq!(st.expanded.get(&id), Some(&true));
        st.toggle_expanded(&id);
        assert_eq!(st.expanded.get(&id), Some(&false));
    }

    #[test]
    fn app_state_carries_empty_multiagent_state() {
        let s = AppState::default_for_tests();
        assert!(s.multiagent.tasks.is_empty());
        assert!(s.multiagent.workers.is_empty());
    }

    #[test]
    fn open_permission_dialog_sets_started_at_and_active_slot() {
        let mut st = AppState::new(StatusSnapshot::default());
        let (tx, _rx) = oneshot::channel();
        let req = permission::gate::PermissionRequest::ToolUseConfirm {
            tool_name: "Write".to_string(),
            tool_input: serde_json::json!({"file_path": "a.txt"}),
            default_decision: permission::tool_default("Write"),
        };
        open_permission_dialog(&mut st, req, Some(tx), None);
        assert!(st.pending_permission.is_some());
        assert!(st.pending_permission_resp_tx.is_some());
        assert!(
            st.pending_permission_started_at.is_some(),
            "started_at must be set for telemetry"
        );
        // No worker → no badge.
        assert!(st.pending_permission.as_ref().unwrap().worker.is_none());
    }

    #[test]
    fn open_permission_dialog_carries_worker_attribution() {
        use crate::components::permissions::worker::WorkerPermissionInfo;
        let mut st = AppState::new(StatusSnapshot::default());
        let (tx, _rx) = oneshot::channel();
        let req = permission::gate::PermissionRequest::ToolUseConfirm {
            tool_name: "Bash".to_string(),
            tool_input: serde_json::json!({"command": "ls"}),
            default_decision: permission::tool_default("Bash"),
        };
        let worker = WorkerPermissionInfo {
            name: "researcher".to_string(),
            color: "researcher".to_string(),
            team: Some("alpha".to_string()),
        };
        open_permission_dialog(&mut st, req, Some(tx), Some(worker));
        // The worker rides onto the pending permission → the dialog renders a
        // dim `· @name` suffix on the title row (app.rs maps `pp.worker` →
        // `ToolUseConfirm`'s `worker_name` prop — perm-09).
        let pp = st.pending_permission.as_ref().unwrap();
        assert_eq!(pp.worker.as_ref().unwrap().name, "researcher");
    }

    #[test]
    fn promote_next_permission_is_fifo_and_respects_active_slot() {
        use crate::permission_bridge::PermissionExchange;
        let mut st = AppState::new(StatusSnapshot::default());
        let mk = |tool: &str| {
            let (tx, _rx) = oneshot::channel();
            PermissionExchange {
                request: permission::gate::PermissionRequest::ToolUseConfirm {
                    tool_name: tool.to_string(),
                    tool_input: serde_json::json!({}),
                    default_decision: permission::tool_default(tool),
                },
                resp_tx: tx,
                worker: None,
            }
        };
        st.permission_queue.push_back(mk("Write"));
        st.permission_queue.push_back(mk("Edit"));

        // First promote opens "Write".
        promote_next_permission(&mut st);
        assert_eq!(st.pending_permission.as_ref().unwrap().tool(), "Write");
        assert_eq!(st.permission_queue.len(), 1);

        // A second promote is a NO-OP while a dialog is active.
        promote_next_permission(&mut st);
        assert_eq!(st.pending_permission.as_ref().unwrap().tool(), "Write");
        assert_eq!(st.permission_queue.len(), 1);

        // Clear the active slot, then promote opens "Edit".
        st.pending_permission = None;
        promote_next_permission(&mut st);
        assert_eq!(st.pending_permission.as_ref().unwrap().tool(), "Edit");
        assert!(st.permission_queue.is_empty());
    }
}

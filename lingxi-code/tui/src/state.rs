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
    /// `Some` while a turn is being driven by the orchestrator.
    pub in_flight_turn: Option<TurnInFlight>,
    /// Timestamp of the first Ctrl-C while idle; cleared after 2s.
    pub sigint_armed_at: Option<Instant>,
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
    /// `pending_open_settings`. `None` between submits. Slash commands NEVER set
    /// this — they intercept + return earlier in the `Submit` arm.
    pub pending_turn: Option<String>,
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
    /// (M6-05) Oneshot back-channel to the orchestrator for the active
    /// permission round-trip. `Some(_)` whenever `pending_permission`
    /// holds a real request that arrived over the bridge; `None` for
    /// stub requests synthesized by `apply_event` (M6-03) or while no
    /// dialog is open.
    pub pending_permission_resp_tx: Option<oneshot::Sender<PermissionResponse>>,
    /// (M6-05) Instant the active dialog opened — used to compute
    /// `elapsed_ms` in the resolved-telemetry event.
    pub pending_permission_started_at: Option<Instant>,
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
    /// `use_future`) walks `<claude_home>/projects/` OUTSIDE the `AppState`
    /// lock, aggregates, and calls [`Self::open_stats`]. Mirrors
    /// `pending_open_agents` — but the walk needs no `OrchestratorHandle`, so
    /// the pump runs unconditionally (not gated on a wired handle).
    pub pending_open_stats: bool,
    /// (M9-09 real data) Set by the `/skills` submit intercept: a request to
    /// open the read-only skill-registry viewer. The SYNC submit path can't
    /// `.await` the on-disk `.claude/skills/` dir walk (project ancestors + user
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
    /// Set by the `/model` submit intercept: a request to open the model picker.
    /// The SYNC submit path can't `.await` `list_available_models`, so it RAISES
    /// this flag; `root::pump_open_model` fetches the models + reads the current
    /// [`StatusSnapshot::model`] and opens via [`Self::open_model`]. Mirrors
    /// `pending_open_mcp` (handle-backed).
    pub pending_open_model: bool,
    /// Set by the model picker's Enter (a committed selection): the model id to
    /// switch to. The SYNC key path can't `.await` `OrchestratorHandle::switch_model`,
    /// so the picker raises this and `root::pump_switch_model` performs the async
    /// write OUTSIDE the lock, then updates [`StatusSnapshot::model`] (success) or
    /// pushes an error `SystemText` (failure). `None` = no pending switch.
    pub pending_switch_model: Option<String>,
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
            in_flight_turn: None,
            sigint_armed_at: None,
            should_exit: false,
            resume_request: None,
            pending_config_edit: false,
            pending_open_settings: None,
            pending_compact: false,
            pending_turn: None,
            focused_tool_id: None,
            expanded: HashMap::new(),
            tool_call_inputs: HashMap::new(),
            pending_permission_resp_tx: None,
            pending_permission_started_at: None,
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
            pending_open_model: false,
            pending_switch_model: None,
            session_agent_color: None,
            pending_save_color: None,
            pending_copy_clipboard: None,
            status_line_text: None,
            status_line_config: None,
        }
    }

    /// (A6) Read the `statusLine` setting out of a loaded settings JSON value
    /// and stash the parsed [`StatusLineConfig`] on the state. A `command`-typed
    /// config arms the async status-line pump; any other shape (absent, or
    /// `type != "command"`) leaves `status_line_config == None` so the built-in
    /// status row renders. Idempotent — safe to call on each settings reload.
    pub fn load_status_line_setting(&mut self, settings: &serde_json::Value) {
        self.status_line_config = settings
            .get("statusLine")
            .and_then(crate::components::status_line_command::StatusLineConfig::from_settings_value);
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
            crate::screens::theme::ThemePickerState::new(self.theme_setting),
        ));
        crate::telemetry::screen_opened("theme");
    }

    /// (M9-08) Open the agents screen with the given catalog rows.
    pub fn open_agents(&mut self, rows: Vec<crate::screens::agents::AgentRow>) {
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
                mode: crate::screens::hooks::HooksDialogMode::List,
            },
        ));
        crate::telemetry::screen_opened("hooks");
    }

    /// Open the `/model` picker with the given model ids + the active model
    /// (pre-highlighted). Called by `root::pump_open_model` after the async
    /// `list_available_models` fetch.
    pub fn open_model(&mut self, models: Vec<String>, current: String) {
        self.active_screen = Some(crate::screens::Screen::Model(
            crate::screens::model::ModelScreenState::new(models, current),
        ));
        crate::telemetry::screen_opened("model");
    }

    /// (M9-09) Open the `/skills` registry viewer with the given grouped
    /// sections. Called by `root::pump_open_skills` after the async on-disk
    /// `.claude/skills/` walk (the frozen `OrchestratorHandle` exposes no
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
                RenderedMessage::AssistantToolUse { id, .. } => Some(*id),
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
        self.focused_tool_id = match self.focused_tool_id {
            None => Some(ids[0]),
            Some(cur) => {
                let pos = ids.iter().position(|i| *i == cur).unwrap_or(0);
                let next = (pos + 1).min(ids.len() - 1);
                Some(ids[next])
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
        self.focused_tool_id = match self.focused_tool_id {
            None => Some(ids[0]),
            Some(cur) => {
                let pos = ids.iter().position(|i| *i == cur).unwrap_or(0);
                let prev = pos.saturating_sub(1);
                Some(ids[prev])
            }
        };
    }

    /// (M6-04) Flip the expanded state for the given tool id. Inserts the
    /// flipped value (default starts at `false`, first toggle → `true`).
    pub fn toggle_expanded(&mut self, id: &ToolUseId) {
        let entry = self.expanded.entry(*id).or_insert(false);
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
            id,
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
            id: a,
            tool: "Read".into(),
            input: serde_json::json!({}),
        });
        st.push_message(RenderedMessage::AssistantText {
            body: "ok".into(),
            timestamp: 0,
        });
        st.push_message(RenderedMessage::AssistantToolUse {
            id: b,
            tool: "Bash".into(),
            input: serde_json::json!({}),
        });
        assert_eq!(st.focused_tool_id, None);
        st.focus_next_tool();
        assert_eq!(st.focused_tool_id, Some(a));
        st.focus_next_tool();
        assert_eq!(st.focused_tool_id, Some(b));
        st.focus_next_tool(); // past end — stays on last
        assert_eq!(st.focused_tool_id, Some(b));
        st.focus_prev_tool();
        assert_eq!(st.focused_tool_id, Some(a));
        st.focus_prev_tool(); // past start — stays on first
        assert_eq!(st.focused_tool_id, Some(a));
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
}

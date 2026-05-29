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

use lingxi_permission::gate::{PermissionRequest, PermissionResponse};
use lingxi_permission::PermissionMode;
use lingxi_protocol::ToolUseId;
use tokio::sync::oneshot;

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
        id: lingxi_protocol::ToolUseId,
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
        id: lingxi_protocol::ToolUseId,
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
        group_id: lingxi_protocol::ToolUseId,
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
        group_id: lingxi_protocol::ToolUseId,
        /// Per-entry display lines, shown when expanded.
        entries: Vec<String>,
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
}

impl Default for StatusSnapshot {
    fn default() -> Self {
        Self {
            model: String::new(),
            cwd: PathBuf::from("."),
            cost: "$0.0000".to_string(),
            context_pct: 0.0,
            permission_mode: PermissionMode::Default,
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
    /// `Some` while a turn is being driven by the orchestrator.
    pub in_flight_turn: Option<TurnInFlight>,
    /// Timestamp of the first Ctrl-C while idle; cleared after 2s.
    pub sigint_armed_at: Option<Instant>,
    /// Set by `/exit` (or second Ctrl-C within the arming window).
    pub should_exit: bool,
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
            in_flight_turn: None,
            sigint_armed_at: None,
            should_exit: false,
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
        }
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

    /// M6-04 Task 2: `RenderedMessage` gains `AssistantToolUse` and
    /// `UserToolResult` variants so the scrollback can carry rich tool
    /// blocks (not the M6-03 `SystemText` placeholders).
    #[test]
    fn rendered_message_carries_tool_use_and_result() {
        use lingxi_protocol::ToolUseId;
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
        use lingxi_protocol::ToolUseId;
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
        use lingxi_protocol::ToolUseId;
        let mut st = AppState::new(fake_status());
        let id = ToolUseId::new();
        assert!(!st.expanded.contains_key(&id));
        st.toggle_expanded(&id);
        assert_eq!(st.expanded.get(&id), Some(&true));
        st.toggle_expanded(&id);
        assert_eq!(st.expanded.get(&id), Some(&false));
    }
}

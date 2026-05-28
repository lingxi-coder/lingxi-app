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

/// Maximum number of messages retained in the scrollback. Excess messages
/// are evicted FIFO.
pub const SCROLLBACK_CAP: usize = 500;

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
    },
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
    /// Scrollback buffer (cap 500, FIFO eviction).
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
            messages: Vec::with_capacity(SCROLLBACK_CAP),
            pending_permission: None,
            streaming: None,
            cancel_token: None,
            prompt_text: String::new(),
            prompt_cursor: 0,
            history: Vec::new(),
            history_cursor: None,
            scroll_offset: 0,
            status,
            in_flight_turn: None,
            sigint_armed_at: None,
            should_exit: false,
            focused_tool_id: None,
            expanded: HashMap::new(),
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

    /// Push a message; evict the oldest if cap exceeded (FIFO).
    pub fn push_message(&mut self, msg: RenderedMessage) {
        self.messages.push(msg);
        if self.messages.len() > SCROLLBACK_CAP {
            self.messages.remove(0);
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
    fn push_evicts_oldest_at_cap() {
        let mut s = AppState::new(fake_status());
        for i in 0..(SCROLLBACK_CAP + 5) {
            s.push_message(RenderedMessage::UserText {
                body: format!("msg{i}"),
                timestamp: 0,
            });
        }
        assert_eq!(s.messages.len(), SCROLLBACK_CAP);
        // First retained = msg5 (msg0..=msg4 were evicted).
        match &s.messages[0] {
            RenderedMessage::UserText { body, .. } => assert_eq!(body, "msg5"),
            _ => panic!("wrong variant"),
        }
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

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
//!   M6-02 surfaces cost as a pre-formatted `String` ("$0.000" until M6-06).
//! - `PermissionMode` lives in `lingxi-permission` with variant `Default`
//!   (not `Normal`).

use std::path::PathBuf;
use std::time::Instant;

use lingxi_permission::PermissionMode;

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
}

/// Snapshot of the status-line fields. Recomputed once per frame.
#[derive(Debug, Clone)]
pub struct StatusSnapshot {
    /// Model display name (e.g. `"claude-sonnet-4.5"`).
    pub model: String,
    /// Current working directory.
    pub cwd: PathBuf,
    /// Pre-formatted cost ("$0.000" until M6-06 wires real Money).
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
            cost: "$0.000".to_string(),
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

/// Reserved permission-prompt slot. Filled by M6-05.
#[derive(Debug, Clone)]
pub struct PendingPermission;

/// Reserved streaming-turn slot. Filled by M6-03.
#[derive(Debug, Clone)]
pub struct StreamingTurn;

/// Root TUI state. Owned by the `App` root component.
pub struct AppState {
    /// Scrollback buffer (cap 500, FIFO eviction).
    pub messages: Vec<RenderedMessage>,
    /// Reserved for M6-05 (permission dialogs).
    pub pending_permission: Option<PendingPermission>,
    /// Reserved for M6-03 (streaming spinner / partial assistant text).
    pub streaming: Option<StreamingTurn>,
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
}

impl AppState {
    /// Construct a fresh `AppState` with the given status snapshot.
    #[must_use]
    pub fn new(status: StatusSnapshot) -> Self {
        Self {
            messages: Vec::with_capacity(SCROLLBACK_CAP),
            pending_permission: None,
            streaming: None,
            prompt_text: String::new(),
            prompt_cursor: 0,
            history: Vec::new(),
            history_cursor: None,
            scroll_offset: 0,
            status,
            in_flight_turn: None,
            sigint_armed_at: None,
            should_exit: false,
        }
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
            cost: "$0.000".to_string(),
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
}

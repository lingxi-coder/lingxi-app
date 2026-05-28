//! `tengu_tui_*` lifecycle events emitted by `lingxi-tui::run_tui_session`.
//!
//! M6-01 ships 4 events. M6-03 adds `streaming_render_{started,ended}`;
//! M6-05 adds `permission_dialog_{shown,resolved}`. Full inventory locked
//! at M6-09 release time.

/// Fired by `run_tui_session` after `RawGuard::enter()` succeeds and
/// before the first render. Payload: `session_id`, `cols`, `rows`.
pub const SESSION_STARTED: &str = "tengu_tui_session_started";

/// Fired by `run_tui_session` just before `RawGuard::exit()` runs,
/// regardless of exit reason (cancel / quit / error). Payload:
/// `session_id`, `duration_ms`, `ended_via` (one of
/// `"quit_key" / "cancelled" / "error"`).
pub const SESSION_ENDED: &str = "tengu_tui_session_ended";

/// Fired once per session, immediately after iocraft completes the first
/// reconcile (i.e. the user sees pixels). Payload: `session_id`,
/// `latency_ms` (since `SESSION_STARTED`).
pub const FIRST_RENDER: &str = "tengu_tui_first_render";

/// Fired on every `crossterm::event::Event::Resize`. Payload:
/// `session_id`, `cols`, `rows`.
pub const RESIZE: &str = "tengu_tui_resize";

// M6-03 additions: streaming render lifecycle.

/// Emitted by the render loop on the first render after the bridge
/// signals `TurnEvent::TurnStarted` (i.e. `state.streaming` transitions
/// `None → Some`). Payload: `session_id`. (M6-03)
pub const STREAMING_RENDER_STARTED: &str = "tengu_tui_streaming_render_started";

/// Emitted by the render loop on the first render after the bridge
/// signals `TurnEvent::TurnEnded(_)` (i.e. `state.streaming` transitions
/// `Some → None`). Payload: `session_id`, `outcome`. (M6-03)
pub const STREAMING_RENDER_ENDED: &str = "tengu_tui_streaming_render_ended";

// M6-05 additions: permission dialog lifecycle.

/// Emitted when a permission dialog transitions from `None` to `Some(_)`
/// in the TUI app state. Payload: `kind` (one of `"tool_use"`,
/// `"exit_plan_mode"`, `"bypass_permissions"`). (M6-05)
pub const PERMISSION_DIALOG_SHOWN: &str = "tengu_tui_permission_dialog_shown";

/// Emitted when the user resolves a permission dialog. Payload: `kind`,
/// `decision` (one of `"allow_once"`, `"allow_always"`, `"deny"`),
/// `persist` (bool), `elapsed_ms` (u64). (M6-05)
pub const PERMISSION_DIALOG_RESOLVED: &str = "tengu_tui_permission_dialog_resolved";

/// Order-locked array; appended into `tengu::ALL_EVENT_NAMES`.
pub(crate) const NAMES: &[&str] = &[
    SESSION_STARTED,
    SESSION_ENDED,
    FIRST_RENDER,
    RESIZE,
    STREAMING_RENDER_STARTED,
    STREAMING_RENDER_ENDED,
    PERMISSION_DIALOG_SHOWN,
    PERMISSION_DIALOG_RESOLVED,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_has_8_entries_after_m6_05() {
        // 4 (M6-01) + 2 (M6-03 streaming render) + 2 (M6-05 permission
        // dialog shown/resolved) = 8.
        assert_eq!(NAMES.len(), 8);
    }

    #[test]
    fn event_name_strings_are_locked() {
        assert_eq!(SESSION_STARTED, "tengu_tui_session_started");
        assert_eq!(SESSION_ENDED, "tengu_tui_session_ended");
        assert_eq!(FIRST_RENDER, "tengu_tui_first_render");
        assert_eq!(RESIZE, "tengu_tui_resize");
        assert_eq!(
            STREAMING_RENDER_STARTED,
            "tengu_tui_streaming_render_started"
        );
        assert_eq!(STREAMING_RENDER_ENDED, "tengu_tui_streaming_render_ended");
        assert_eq!(
            PERMISSION_DIALOG_SHOWN,
            "tengu_tui_permission_dialog_shown"
        );
        assert_eq!(
            PERMISSION_DIALOG_RESOLVED,
            "tengu_tui_permission_dialog_resolved"
        );
    }
}

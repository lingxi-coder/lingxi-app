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

/// Order-locked array; appended into `tengu::ALL_EVENT_NAMES`.
pub(crate) const NAMES: &[&str] = &[SESSION_STARTED, SESSION_ENDED, FIRST_RENDER, RESIZE];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_has_4_entries() {
        assert_eq!(NAMES.len(), 4);
    }

    #[test]
    fn event_name_strings_are_locked() {
        assert_eq!(SESSION_STARTED, "tengu_tui_session_started");
        assert_eq!(SESSION_ENDED, "tengu_tui_session_ended");
        assert_eq!(FIRST_RENDER, "tengu_tui_first_render");
        assert_eq!(RESIZE, "tengu_tui_resize");
    }
}

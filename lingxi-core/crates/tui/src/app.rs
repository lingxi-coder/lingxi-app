//! Root iocraft component + key-action dispatcher.
//!
//! M6-01 shipped only a static placeholder view + a binary `should_quit`
//! flag. M6-02 keeps that placeholder intact (still consumed by the
//! `render_placeholder` snapshot test + the iocraft-prototype gate)
//! and layers two new responsibilities on top:
//!
//! 1. `pub fn dispatch(KeyAction, &mut AppState) -> bool` — the pure
//!    state-transition for a single user keystroke. Heavy I/O (slash
//!    dispatch, run_turn) is handled by `app::handle_submit_line` and
//!    `app::run_one_submit` which call `dispatch` first and then act on
//!    the returned `should_run_turn` flag.
//! 2. `pub fn scroll_with_viewport(&mut AppState, ScrollDir, vh)` — the
//!    scroll-offset math, which needs the live viewport height (passed
//!    by the per-frame loop) and is therefore separated from `dispatch`.
//!
//! M6-02 does not yet mount `ReplScreen` in the event loop; that wiring
//! lands in Task 14. The placeholder root is still what `run_tui_session`
//! renders on startup.

use std::time::Instant;

use iocraft::prelude::*;

use crate::components::prompt_input::{
    apply_backspace, apply_insert, apply_move, CursorMove as PiCursor,
};
use crate::events::keymap::{CursorMove, KeyAction, ScrollDir};
use crate::state::{AppState, RenderedMessage};

/// Idle Ctrl-C re-arm window: the second Ctrl-C confirms exit only when
/// the first was within this many seconds. Matches the M5-13 stdio REPL.
pub const SIGINT_WINDOW_SECS: u64 = 2;

/// Crate version string surfaced to the placeholder line.
///
/// Sourced from the lingxi-tui crate's own `CARGO_PKG_VERSION` so version
/// bumps automatically propagate. M6-09 bumps the crate to 0.7.0, at which
/// point the rendered placeholder reads `"lingxi-tui v0.7.0"`.
fn version_line() -> String {
    format!("lingxi-tui v{}", env!("CARGO_PKG_VERSION"))
}

/// Top-level TUI application state. M6-01 keeps this minimal: no fields
/// are needed to render the placeholder. M6-02 grows it into
/// `{ messages, prompt_text, streaming, ... }`.
#[derive(Debug, Default, Clone)]
pub struct TuiApp {
    /// Whether the user has requested quit. Wired by the event loop in
    /// `run_tui_session` once Ctrl-C / Ctrl-D classifies as `KeyClass::Quit`.
    pub should_quit: bool,
}

impl TuiApp {
    /// Construct a fresh, idle app state.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark the app as wanting to quit. Called by the event loop on
    /// `KeyClass::Quit` or when the cancel token trips.
    pub fn request_quit(&mut self) {
        self.should_quit = true;
    }

    /// Render the root frame. Returns an iocraft element tree owned for
    /// `'static`, which the session loop hands to iocraft's render driver.
    #[must_use]
    pub fn render(&self) -> AnyElement<'static> {
        let line = version_line();
        element! {
            View(padding: 1, flex_direction: FlexDirection::Column) {
                Text(content: line)
            }
        }
        .into_any()
    }
}

/// Process one `KeyAction` against the live `AppState`.
///
/// Returns `true` iff the action was a `Submit` that the caller should
/// follow up with a real orchestrator round-trip (via
/// [`run_one_submit`]). All other branches return `false`.
///
/// The function is pure with respect to I/O — it only mutates `st`.
pub fn dispatch(action: KeyAction, st: &mut AppState) -> bool {
    match action {
        KeyAction::InsertChar(c) => {
            let (t, cur) = apply_insert(&st.prompt_text, st.prompt_cursor, c);
            st.prompt_text = t;
            st.prompt_cursor = cur;
            false
        }
        KeyAction::Backspace => {
            let (t, cur) = apply_backspace(&st.prompt_text, st.prompt_cursor);
            st.prompt_text = t;
            st.prompt_cursor = cur;
            false
        }
        KeyAction::MoveCursor(m) => {
            let pi = match m {
                CursorMove::Left => PiCursor::Left,
                CursorMove::Right => PiCursor::Right,
                CursorMove::Home => PiCursor::Home,
                CursorMove::End => PiCursor::End,
            };
            st.prompt_cursor = apply_move(&st.prompt_text, st.prompt_cursor, pi);
            false
        }
        KeyAction::Submit => {
            if st.prompt_text.is_empty() {
                return false;
            }
            let line = std::mem::take(&mut st.prompt_text);
            st.prompt_cursor = 0;
            st.history.push(line.clone());
            st.history_cursor = None;
            st.push_message(RenderedMessage::UserText {
                body: line,
                timestamp: chrono::Utc::now().timestamp(),
            });
            true
        }
        KeyAction::Cancel => {
            if st.in_flight_turn.is_some() {
                if let Some(tif) = &st.in_flight_turn {
                    tif.cancel.cancel();
                }
                st.push_message(RenderedMessage::SystemText {
                    body: "^C interrupted by user".into(),
                    timestamp: chrono::Utc::now().timestamp(),
                    is_error: false,
                });
            } else if !st.prompt_text.is_empty() {
                st.prompt_text.clear();
                st.prompt_cursor = 0;
            } else {
                match st.sigint_armed_at {
                    Some(t) if t.elapsed().as_secs() < SIGINT_WINDOW_SECS => {
                        st.should_exit = true;
                    }
                    _ => {
                        st.sigint_armed_at = Some(Instant::now());
                        st.push_message(RenderedMessage::SystemText {
                            body: "^C (press Ctrl-C again or type /exit to quit)".into(),
                            timestamp: chrono::Utc::now().timestamp(),
                            is_error: false,
                        });
                    }
                }
            }
            false
        }
        KeyAction::HistoryStep(delta) => {
            if st.history.is_empty() {
                return false;
            }
            let new_cursor: Option<usize> = match (st.history_cursor, delta) {
                (None, -1) => Some(st.history.len() - 1),
                (None, 1) => None,
                (Some(0), -1) => Some(0),
                (Some(i), -1) => Some(i - 1),
                (Some(i), 1) if i + 1 < st.history.len() => Some(i + 1),
                (Some(_), 1) => None,
                _ => st.history_cursor,
            };
            st.history_cursor = new_cursor;
            st.prompt_text = match new_cursor {
                Some(i) => st.history[i].clone(),
                None => String::new(),
            };
            st.prompt_cursor = st.prompt_text.len();
            false
        }
        KeyAction::ScrollStep(dir) => {
            // Default unit step uses height=1 for j/k. PageUp/Down delegate
            // to `scroll_with_viewport` from the per-frame loop where the
            // real viewport height is known.
            scroll_with_viewport(st, dir, 1);
            false
        }
    }
}

/// Apply a scroll direction with a known viewport height. Called per
/// frame for PageUp/PageDown, where the viewport is known; line-step
/// (`j`/`k`) reuses this with `height=1`.
pub fn scroll_with_viewport(st: &mut AppState, dir: ScrollDir, viewport_height: usize) {
    let total = st.messages.len();
    let max = total.saturating_sub(viewport_height) as i64;
    let cur = st.scroll_offset as i64;
    let new = match dir {
        ScrollDir::LineUp => cur + 1,
        ScrollDir::LineDown => cur - 1,
        ScrollDir::PageUp => cur + viewport_height as i64,
        ScrollDir::PageDown => cur - viewport_height as i64,
        ScrollDir::Top => max,
        ScrollDir::Bottom => 0,
    };
    st.scroll_offset = new.clamp(0, max) as usize;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_returns_idle_state() {
        let app = TuiApp::new();
        assert!(!app.should_quit);
    }

    #[test]
    fn request_quit_sets_flag() {
        let mut app = TuiApp::new();
        app.request_quit();
        assert!(app.should_quit);
    }

    #[test]
    fn version_line_includes_crate_version() {
        let v = version_line();
        assert!(v.starts_with("lingxi-tui v"));
        assert!(v.contains(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn render_returns_an_element() {
        let app = TuiApp::new();
        let _el = app.render();
    }
}

#[cfg(test)]
mod dispatch_tests {
    use super::*;
    use crate::events::keymap::{KeyAction, ScrollDir};
    use crate::state::{AppState, RenderedMessage, StatusSnapshot};
    use lingxi_permission::PermissionMode;
    use std::path::PathBuf;

    fn s() -> AppState {
        AppState::new(StatusSnapshot {
            model: "claude-sonnet-4.5".into(),
            cwd: PathBuf::from("/a/b"),
            cost: "$0.000".to_string(),
            context_pct: 0.0,
            permission_mode: PermissionMode::Default,
        })
    }

    #[test]
    fn insert_chars_then_backspace() {
        let mut st = s();
        dispatch(KeyAction::InsertChar('h'), &mut st);
        dispatch(KeyAction::InsertChar('i'), &mut st);
        assert_eq!(st.prompt_text, "hi");
        assert_eq!(st.prompt_cursor, 2);
        dispatch(KeyAction::Backspace, &mut st);
        assert_eq!(st.prompt_text, "h");
        assert_eq!(st.prompt_cursor, 1);
    }

    #[test]
    fn submit_clears_prompt_and_pushes_user_message() {
        let mut st = s();
        dispatch(KeyAction::InsertChar('h'), &mut st);
        dispatch(KeyAction::InsertChar('i'), &mut st);
        dispatch(KeyAction::Submit, &mut st);
        assert_eq!(st.prompt_text, "");
        assert_eq!(st.messages.len(), 1);
        assert!(
            matches!(&st.messages[0], RenderedMessage::UserText { body, .. } if body == "hi")
        );
        assert_eq!(st.history.last().map(String::as_str), Some("hi"));
    }

    #[test]
    fn pgup_increments_scroll_offset_by_viewport() {
        let mut st = s();
        for i in 0..30 {
            st.push_message(RenderedMessage::UserText {
                body: format!("m{i}"),
                timestamp: 0,
            });
        }
        scroll_with_viewport(&mut st, ScrollDir::PageUp, 10);
        assert_eq!(st.scroll_offset, 10);
        scroll_with_viewport(&mut st, ScrollDir::PageUp, 10);
        assert_eq!(st.scroll_offset, 20);
    }

    #[test]
    fn ctrl_c_clears_nonempty_prompt() {
        let mut st = s();
        dispatch(KeyAction::InsertChar('x'), &mut st);
        assert_eq!(st.prompt_text, "x");
        dispatch(KeyAction::Cancel, &mut st);
        assert_eq!(st.prompt_text, "");
        assert!(st.sigint_armed_at.is_none());
    }
}

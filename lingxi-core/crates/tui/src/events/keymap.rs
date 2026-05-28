//! Key-event classifier.
//!
//! M6-01 shipped a minimal `KeyClass` (Quit / Other) used by the
//! placeholder event loop to detect Ctrl-C / Ctrl-D.
//!
//! M6-02 adds the full `KeyAction` enum plus `map_key`, which the
//! `App::dispatch` state-machine consumes. Both enums coexist so that
//! M6-01's loop continues to work without modification — the new
//! `map_key` is layered on top.
//!
//! M6-05 adds [`handle_key`], a top-level dispatcher that owns the
//! focus-trap branch: when a permission dialog is open, ALL keystrokes
//! route into the active dialog's `handle_key` helper instead of
//! `map_key` / `dispatch`. This keeps the `PromptInput` text buffer
//! untouched and the scrollback keybinds inert while the dialog owns
//! the screen.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::components::permissions::{
    bypass_permissions, exit_plan_mode, tool_use_confirm, DialogResolution,
};
use crate::state::AppState;
use lingxi_permission::gate::PermissionRequest;

/// Legacy M6-01 quit classifier. Retained for the existing event-loop
/// behaviour test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyClass {
    /// Ctrl-C or Ctrl-D: request quit.
    Quit,
    /// Any other key — passed through to the focused component.
    Other,
}

/// Classify a key event into a `KeyClass` (Quit vs. Other).
#[must_use]
pub fn classify(key: &KeyEvent) -> KeyClass {
    match (key.code, key.modifiers) {
        (KeyCode::Char('c' | 'd'), KeyModifiers::CONTROL) => KeyClass::Quit,
        _ => KeyClass::Other,
    }
}

/// Scroll-direction primitive emitted by `map_key`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollDir {
    /// Up one line.
    LineUp,
    /// Down one line.
    LineDown,
    /// Up one viewport height.
    PageUp,
    /// Down one viewport height.
    PageDown,
    /// Top of buffer.
    Top,
    /// Bottom of buffer (latest).
    Bottom,
}

/// Cursor-movement primitive emitted by `map_key`. Distinct from
/// `components::prompt_input::CursorMove` so `events::keymap` stays
/// independent of the components module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorMove {
    /// One char left.
    Left,
    /// One char right.
    Right,
    /// Beginning of line.
    Home,
    /// End of line.
    End,
}

/// High-level action emitted by `map_key`. Consumed by `app::dispatch`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyAction {
    /// Insert a printable Unicode char at the prompt cursor.
    InsertChar(char),
    /// Delete the char before the prompt cursor.
    Backspace,
    /// Move the cursor (Left / Right / Home / End).
    MoveCursor(CursorMove),
    /// Enter — submit the current prompt.
    Submit,
    /// Ctrl-C — cancel turn / clear prompt / arm exit.
    Cancel,
    /// Up/Down — step through prompt history. `-1` = older, `+1` = newer.
    HistoryStep(i8),
    /// Scroll the scrollback by the given direction.
    ScrollStep(ScrollDir),
    /// (M6-04) Up/Down in scrollback mode — walk the focused tool block.
    /// `-1` = previous, `+1` = next.
    FocusToolStep(i8),
    /// (M6-04) `e` / Enter on a focused tool block — toggle expanded.
    ToggleExpanded,
}

/// Map a crossterm `KeyEvent` into a `KeyAction`. `prompt_empty` toggles
/// the vim-style scroll bindings (`j`/`k`/`g`/`G`). `focus_active` (M6-04)
/// promotes Up/Down to tool-focus walking and `e`/Enter to expanded toggle
/// — used when at least one tool block exists in the scrollback AND the
/// prompt is empty.
#[must_use]
pub fn map_key(evt: KeyEvent, prompt_empty: bool, focus_active: bool) -> Option<KeyAction> {
    use KeyAction::{
        Backspace, Cancel, FocusToolStep, HistoryStep, InsertChar, MoveCursor, ScrollStep, Submit,
        ToggleExpanded,
    };
    // M6-04 focus-mode bindings take priority when focus is active.
    if focus_active {
        match (evt.code, evt.modifiers) {
            (KeyCode::Up, _) => return Some(FocusToolStep(-1)),
            (KeyCode::Down, _) => return Some(FocusToolStep(1)),
            (KeyCode::Char('e'), KeyModifiers::NONE) | (KeyCode::Enter, _) if prompt_empty => {
                return Some(ToggleExpanded);
            }
            _ => {}
        }
    }
    match (evt.code, evt.modifiers) {
        (KeyCode::Enter, _) => Some(Submit),
        (KeyCode::Backspace, _) => Some(Backspace),
        (KeyCode::Char('c'), KeyModifiers::CONTROL) => Some(Cancel),
        (KeyCode::Left, _) => Some(MoveCursor(CursorMove::Left)),
        (KeyCode::Right, _) => Some(MoveCursor(CursorMove::Right)),
        (KeyCode::Home, _) => Some(MoveCursor(CursorMove::Home)),
        (KeyCode::End, _) => Some(MoveCursor(CursorMove::End)),
        (KeyCode::Up, _) => Some(HistoryStep(-1)),
        (KeyCode::Down, _) => Some(HistoryStep(1)),
        (KeyCode::PageUp, _) => Some(ScrollStep(ScrollDir::PageUp)),
        (KeyCode::PageDown, _) => Some(ScrollStep(ScrollDir::PageDown)),
        // Vim-style nav only when prompt is empty.
        (KeyCode::Char('j'), KeyModifiers::NONE) if prompt_empty => {
            Some(ScrollStep(ScrollDir::LineDown))
        }
        (KeyCode::Char('k'), KeyModifiers::NONE) if prompt_empty => {
            Some(ScrollStep(ScrollDir::LineUp))
        }
        (KeyCode::Char('g'), KeyModifiers::NONE) if prompt_empty => {
            Some(ScrollStep(ScrollDir::Top))
        }
        (KeyCode::Char('G'), KeyModifiers::SHIFT) if prompt_empty => {
            Some(ScrollStep(ScrollDir::Bottom))
        }
        // Printable chars (with optional SHIFT for capitals/symbols).
        (KeyCode::Char(c), m) if m == KeyModifiers::NONE || m == KeyModifiers::SHIFT => {
            Some(InsertChar(c))
        }
        _ => None,
    }
}

/// (M6-05) Top-level key dispatcher with the focus-trap branch.
///
/// When `state.pending_permission.is_some()`, this routes the key event
/// to the appropriate dialog's `handle_key` and (on resolution) ships the
/// `PermissionResponse` back over `pending_permission_resp_tx`. The
/// `PromptInput` buffer and scrollback keys are never touched.
///
/// When no dialog is open, this falls through to `map_key` + `dispatch`
/// (the existing M6-02 pipeline). Returns `true` iff the user submitted
/// a prompt line that should drive an orchestrator turn.
///
/// Telemetry: when a resolution is produced, fires
/// `tengu_tui_permission_dialog_resolved` with the decision, the
/// `persist` flag, and `elapsed_ms` since the dialog opened.
pub fn handle_key(state: &mut AppState, key: KeyEvent) -> bool {
    // === FOCUS TRAP (M6-05) ===
    if state.pending_permission.is_some() {
        let resolution = match state.pending_permission.as_ref().map(|p| &p.request) {
            Some(PermissionRequest::ToolUseConfirm { .. }) => {
                tool_use_confirm::handle_key(&mut state.tool_use_dialog_state, key)
            }
            Some(PermissionRequest::ExitPlanMode { .. }) => {
                exit_plan_mode::handle_key(&mut state.exit_plan_dialog_state, key)
            }
            Some(PermissionRequest::BypassPermissionsMode) => {
                bypass_permissions::handle_key(&mut state.bypass_dialog_state, key)
            }
            None => unreachable!("pending_permission is_some() but variant missing"),
        };
        if let Some(resolution) = resolution {
            resolve_pending_permission(state, resolution);
        }
        return false;
    }
    // === end focus trap ===

    if let Some(action) = map_key(
        key,
        state.prompt_text.is_empty(),
        state.focused_tool_id.is_some(),
    ) {
        crate::app::dispatch(action, state)
    } else {
        false
    }
}

/// (M6-05) Apply a [`DialogResolution`] to the `AppState`: ship the
/// response back to the orchestrator (if a `resp_tx` is attached), fire
/// the resolved-telemetry event, and clear the dialog slot.
fn resolve_pending_permission(state: &mut AppState, resolution: DialogResolution) {
    use crate::components::permissions::bypass_permissions::BypassPermissionsState;
    use crate::components::permissions::exit_plan_mode::ExitPlanModeState;
    use crate::components::permissions::tool_use_confirm::ToolUseConfirmState;
    let kind = match state.pending_permission.as_ref().map(|p| &p.request) {
        Some(PermissionRequest::ToolUseConfirm { .. }) => "tool_use",
        Some(PermissionRequest::ExitPlanMode { .. }) => "exit_plan_mode",
        Some(PermissionRequest::BypassPermissionsMode) => "bypass_permissions",
        None => return,
    };
    let elapsed_ms = state.pending_permission_started_at.map_or(0, |t| {
        u64::try_from(t.elapsed().as_millis()).unwrap_or(u64::MAX)
    });
    crate::telemetry::permission_dialog_resolved(
        kind,
        resolution.response,
        resolution.persist,
        elapsed_ms,
    );
    if let Some(tx) = state.pending_permission_resp_tx.take() {
        let _ = tx.send(resolution.response);
    }
    state.pending_permission = None;
    state.pending_permission_started_at = None;
    state.tool_use_dialog_state = ToolUseConfirmState::default();
    state.exit_plan_dialog_state = ExitPlanModeState::default();
    state.bypass_dialog_state = BypassPermissionsState::default();
}

#[cfg(test)]
mod classify_tests {
    use super::*;

    #[test]
    fn ctrl_c_is_quit() {
        let k = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(classify(&k), KeyClass::Quit);
    }

    #[test]
    fn ctrl_d_is_quit() {
        let k = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL);
        assert_eq!(classify(&k), KeyClass::Quit);
    }

    #[test]
    fn plain_letter_is_other() {
        let k = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        assert_eq!(classify(&k), KeyClass::Other);
    }

    #[test]
    fn escape_is_other_in_m6_01() {
        let k = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(classify(&k), KeyClass::Other);
    }
}

#[cfg(test)]
mod m6_02_tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn char_h_maps_to_insert() {
        assert!(matches!(
            map_key(k(KeyCode::Char('h')), false, false),
            Some(KeyAction::InsertChar('h'))
        ));
    }

    #[test]
    fn backspace_maps() {
        assert!(matches!(
            map_key(k(KeyCode::Backspace), false, false),
            Some(KeyAction::Backspace)
        ));
    }

    #[test]
    fn enter_maps_to_submit() {
        assert!(matches!(
            map_key(k(KeyCode::Enter), false, false),
            Some(KeyAction::Submit)
        ));
    }

    #[test]
    fn j_scrolls_only_when_prompt_empty() {
        assert!(matches!(
            map_key(k(KeyCode::Char('j')), true, false),
            Some(KeyAction::ScrollStep(ScrollDir::LineDown))
        ));
        assert!(matches!(
            map_key(k(KeyCode::Char('j')), false, false),
            Some(KeyAction::InsertChar('j'))
        ));
    }

    #[test]
    fn pgup_scrolls_pageup_always() {
        assert!(matches!(
            map_key(k(KeyCode::PageUp), false, false),
            Some(KeyAction::ScrollStep(ScrollDir::PageUp))
        ));
    }

    #[test]
    fn arrow_up_maps_to_history() {
        assert!(matches!(
            map_key(k(KeyCode::Up), false, false),
            Some(KeyAction::HistoryStep(-1))
        ));
    }

    #[test]
    fn ctrl_c_maps_to_cancel() {
        let evt = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(matches!(
            map_key(evt, false, false),
            Some(KeyAction::Cancel)
        ));
    }

    /// M6-04 T10: with `focus_active`, Up/Down walks tool focus instead
    /// of history.
    #[test]
    fn arrow_up_walks_focus_when_focus_active() {
        assert!(matches!(
            map_key(k(KeyCode::Up), true, true),
            Some(KeyAction::FocusToolStep(-1))
        ));
        assert!(matches!(
            map_key(k(KeyCode::Down), true, true),
            Some(KeyAction::FocusToolStep(1))
        ));
    }

    /// M6-04 T10: `e` and Enter (in focus mode with empty prompt) toggle.
    #[test]
    fn e_and_enter_toggle_when_focus_active() {
        assert!(matches!(
            map_key(k(KeyCode::Char('e')), true, true),
            Some(KeyAction::ToggleExpanded)
        ));
        assert!(matches!(
            map_key(k(KeyCode::Enter), true, true),
            Some(KeyAction::ToggleExpanded)
        ));
    }
}

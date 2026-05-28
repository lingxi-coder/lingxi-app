//! Key-event classifier.
//!
//! M6-01 shipped a minimal `KeyClass` (Quit / Other) used by the
//! placeholder event loop to detect Ctrl-C / Ctrl-D.
//!
//! M6-02 adds the full `KeyAction` enum plus `map_key`, which the
//! `App::dispatch` state-machine consumes. Both enums coexist so that
//! M6-01's loop continues to work without modification — the new
//! `map_key` is layered on top.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

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
}

/// Map a crossterm `KeyEvent` into a `KeyAction`. `prompt_empty` toggles
/// the vim-style scroll bindings (`j`/`k`/`g`/`G`).
#[must_use]
pub fn map_key(evt: KeyEvent, prompt_empty: bool) -> Option<KeyAction> {
    use KeyAction::{
        Backspace, Cancel, HistoryStep, InsertChar, MoveCursor, ScrollStep, Submit,
    };
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

#[cfg(test)]
mod tests {
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
            map_key(k(KeyCode::Char('h')), false),
            Some(KeyAction::InsertChar('h'))
        ));
    }

    #[test]
    fn backspace_maps() {
        assert!(matches!(
            map_key(k(KeyCode::Backspace), false),
            Some(KeyAction::Backspace)
        ));
    }

    #[test]
    fn enter_maps_to_submit() {
        assert!(matches!(
            map_key(k(KeyCode::Enter), false),
            Some(KeyAction::Submit)
        ));
    }

    #[test]
    fn j_scrolls_only_when_prompt_empty() {
        assert!(matches!(
            map_key(k(KeyCode::Char('j')), true),
            Some(KeyAction::ScrollStep(ScrollDir::LineDown))
        ));
        assert!(matches!(
            map_key(k(KeyCode::Char('j')), false),
            Some(KeyAction::InsertChar('j'))
        ));
    }

    #[test]
    fn pgup_scrolls_pageup_always() {
        assert!(matches!(
            map_key(k(KeyCode::PageUp), false),
            Some(KeyAction::ScrollStep(ScrollDir::PageUp))
        ));
    }

    #[test]
    fn arrow_up_maps_to_history() {
        assert!(matches!(
            map_key(k(KeyCode::Up), false),
            Some(KeyAction::HistoryStep(-1))
        ));
    }

    #[test]
    fn ctrl_c_maps_to_cancel() {
        let evt = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(matches!(map_key(evt, false), Some(KeyAction::Cancel)));
    }
}

//! Key-event classifier. M6-01 ships only the bindings needed to quit
//! the placeholder TUI (Ctrl-C then Ctrl-D). Full claude-code bindings
//! land in M6-02.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// The high-level action a key event maps to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    /// Ctrl-C or Ctrl-D: request quit.
    Quit,
    /// Any other key — passed through to the focused component.
    Other,
}

/// Classify a key event into a `KeyAction`.
#[must_use]
pub fn classify(key: &KeyEvent) -> KeyAction {
    match (key.code, key.modifiers) {
        (KeyCode::Char('c' | 'd'), KeyModifiers::CONTROL) => KeyAction::Quit,
        _ => KeyAction::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctrl_c_is_quit() {
        let k = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(classify(&k), KeyAction::Quit);
    }

    #[test]
    fn ctrl_d_is_quit() {
        let k = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL);
        assert_eq!(classify(&k), KeyAction::Quit);
    }

    #[test]
    fn plain_letter_is_other() {
        let k = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        assert_eq!(classify(&k), KeyAction::Other);
    }

    #[test]
    fn escape_is_other_in_m6_01() {
        // M6-05 will route Esc to permission-dialog deny; for M6-01 it's
        // a passthrough.
        let k = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(classify(&k), KeyAction::Other);
    }
}

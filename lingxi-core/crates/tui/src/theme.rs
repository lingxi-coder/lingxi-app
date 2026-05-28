//! Hardcoded color palette for the M6 TUI.
//!
//! Theme picker arrives in M7; M6-02 ships 4 locked colors:
//! - `ASSISTANT` = cyan
//! - `USER` = terminal default (`Color::Reset`)
//! - `ERROR` = red
//! - `DIM` = dark grey (for system messages)

use iocraft::Color;

/// Locked color constants for assistant / user / error / dim text.
pub struct TuiTheme;

impl TuiTheme {
    /// Assistant body text color — cyan.
    pub const ASSISTANT: Color = Color::Cyan;
    /// User prompt color — terminal default foreground (no override).
    pub const USER: Color = Color::Reset;
    /// Error/system-error text — red.
    pub const ERROR: Color = Color::Red;
    /// Dim text (system hints, footnotes) — dark grey.
    pub const DIM: Color = Color::DarkGrey;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assistant_is_cyan() {
        assert!(matches!(TuiTheme::ASSISTANT, Color::Cyan));
    }

    #[test]
    fn user_is_reset() {
        assert!(matches!(TuiTheme::USER, Color::Reset));
    }

    #[test]
    fn error_is_red() {
        assert!(matches!(TuiTheme::ERROR, Color::Red));
    }

    #[test]
    fn dim_is_dark_grey() {
        assert!(matches!(TuiTheme::DIM, Color::DarkGrey));
    }
}

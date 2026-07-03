//! `UserBashInputMessage` — `! ` prefix + command text.
//!
//! Literal locks (byte-for-byte from claude-code):
//!   - prefix: `! ` (claude-code `bashBorder`). The lingxi `Theme` ports only
//!     the keys it renders and has no dedicated `bashBorder` field; the prefix
//!     stays `TuiTheme::DIM` (= `Theme::dark().dim`). A dedicated bash-border
//!     `Theme` field is re-deferred — TODO(M8).
//!   - command text color: `text` → `TuiTheme::USER` (terminal default;
//!     claude renders the bash command line uncolored).
//!   source: claude-code/src/components/messages/UserBashInputMessage.tsx
#![allow(clippy::doc_markdown, clippy::doc_lazy_continuation)]

use iocraft::prelude::*;

use crate::render_iocraft::StyleColorIocraftExt;
use crate::theme::TuiTheme;

/// `! ` prefix glyph + space (color `bashBorder` in claude-code).
pub const PREFIX: &str = "! ";

/// Props for [`UserBashInputMessage`].
#[derive(Debug, Clone, Default, Props)]
pub struct UserBashInputProps {
    /// Command line text (already extracted from `<bash-input>`).
    pub command: String,
}

/// Pure string-form renderer: `"! {command}"`. Empty command → empty string.
#[must_use]
pub fn render_bash_input_to_string(command: &str) -> String {
    if command.is_empty() {
        return String::new();
    }
    format!("{PREFIX}{command}")
}

/// iocraft component.
#[component]
pub fn UserBashInputMessage(props: &UserBashInputProps) -> impl Into<AnyElement<'static>> {
    let command = props.command.clone();
    element! {
        View(flex_direction: FlexDirection::Row) {
            Text(content: PREFIX, color: TuiTheme::DIM.to_iocraft())
            Text(content: command, color: TuiTheme::USER.to_iocraft())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_is_bang_space_ascii() {
        assert_eq!(PREFIX.as_bytes(), &[0x21, 0x20]);
    }

    #[test]
    fn empty_command_renders_empty() {
        assert_eq!(render_bash_input_to_string(""), "");
    }

    #[test]
    fn command_gets_bang_prefix() {
        assert_eq!(render_bash_input_to_string("ls"), "! ls");
    }
}

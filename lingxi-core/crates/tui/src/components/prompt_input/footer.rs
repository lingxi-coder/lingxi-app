//! `PromptInputFooter` — the chrome below the editor: mode-indicator,
//! placeholder, the collapsed help hint, the newline hint, and a (M7-06
//! empty) suggestions area. Live suggestion filtering arrives in M7-07.
//!
//! Literal lock (claude-code PromptInput*): the prompt glyph is `❯ `
//! (figures.pointer + space), the newline hint is `shift + ⏎ for newline`,
//! and the help hint is `? for shortcuts`.

use iocraft::prelude::*;

use crate::theme::TuiTheme;

/// Which leading glyph the mode-indicator shows. M7-06 ships `Prompt`; the
/// `Bash` / vim-mode variants land in M7-07/M7-08.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FooterMode {
    /// Normal prompt mode — glyph `❯ `.
    #[default]
    Prompt,
    /// Bash mode — glyph `! ` (placeholder for M7-07; not yet keyboard-wired).
    Bash,
}

impl FooterMode {
    /// The leading glyph (claude-code PromptInputModeIndicator.tsx).
    #[must_use]
    pub fn glyph(self) -> &'static str {
        match self {
            FooterMode::Prompt => "❯ ",
            FooterMode::Bash => "! ",
        }
    }
}

/// Props for [`PromptInputFooter`].
#[derive(Default, Props)]
pub struct PromptInputFooterProps {
    /// Mode-indicator glyph selector.
    pub mode: FooterMode,
    /// Placeholder shown when the buffer is empty (None = no placeholder).
    pub placeholder: Option<String>,
    /// Whether the buffer is currently empty (drives placeholder visibility).
    pub is_empty: bool,
}

/// Render the footer: `[glyph][placeholder?]` row + a dim hint row
/// (`? for shortcuts     shift + ⏎ for newline`).
#[component]
pub fn PromptInputFooter(props: &PromptInputFooterProps) -> impl Into<AnyElement<'static>> {
    let glyph = props.mode.glyph().to_string();
    let placeholder = if props.is_empty {
        props.placeholder.clone().unwrap_or_default()
    } else {
        String::new()
    };
    element! {
        View(flex_direction: FlexDirection::Column) {
            View(flex_direction: FlexDirection::Row) {
                Text(content: glyph, color: TuiTheme::DIM)
                Text(content: placeholder, color: TuiTheme::DIM)
            }
            View(flex_direction: FlexDirection::Row, gap: 5) {
                Text(content: "? for shortcuts".to_string(), color: TuiTheme::DIM)
                Text(content: "shift + ⏎ for newline".to_string(), color: TuiTheme::DIM)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyph_prompt_is_pointer() {
        assert_eq!(FooterMode::Prompt.glyph(), "❯ ");
    }

    #[test]
    fn glyph_bash_is_bang() {
        assert_eq!(FooterMode::Bash.glyph(), "! ");
    }
}

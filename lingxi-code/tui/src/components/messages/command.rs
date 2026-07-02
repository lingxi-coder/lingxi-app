//! `UserCommandMessage` — `❯ /cmd args` or `❯ Skill(name)`.
//!
//! Literal locks (byte-for-byte from claude-code):
//!   - prefix: `❯ ` (figures.pointer U+276F + space, claude-code `subtle`).
//!     The lingxi `Theme` has no dedicated `subtle` key; the whole line uses
//!     `TuiTheme::USER` (terminal default), matching M7-05's single-color
//!     simplification. A per-span `subtle` prefix is re-deferred — TODO(M8).
//!   - slash form: `/{command} {args}` (args omitted when empty)
//!   - skill form: `Skill({command})`
//!   - body text color: `text` → `TuiTheme::USER` (terminal default).
//!   source: claude-code/src/components/messages/UserCommandMessage.tsx
#![allow(clippy::doc_markdown, clippy::doc_lazy_continuation)]

use iocraft::prelude::*;

use crate::render_iocraft::StyleColorIocraftExt;
use crate::theme::TuiTheme;

/// `❯ ` pointer prefix + space (figures.pointer U+276F, color `subtle`).
pub const PREFIX: &str = "\u{276F} ";

/// Props for [`UserCommandMessage`].
#[derive(Debug, Clone, Default, Props)]
pub struct UserCommandProps {
    /// Command name without the leading slash.
    pub command: String,
    /// Argument string (may be empty).
    pub args: String,
    /// Render the `Skill(name)` form.
    pub is_skill: bool,
}

/// Pure string renderer. Returns `""` when `command` is empty.
#[must_use]
pub fn render_command_to_string(command: &str, args: &str, is_skill: bool) -> String {
    if command.is_empty() {
        return String::new();
    }
    if is_skill {
        return format!("{PREFIX}Skill({command})");
    }
    // claude-code: `/${[command, args].filter(Boolean).join(' ')}`.
    let body = if args.is_empty() {
        format!("/{command}")
    } else {
        format!("/{command} {args}")
    };
    format!("{PREFIX}{body}")
}

/// iocraft component.
#[component]
pub fn UserCommandMessage(props: &UserCommandProps) -> impl Into<AnyElement<'static>> {
    let content = render_command_to_string(&props.command, &props.args, props.is_skill);
    element! {
        View(flex_direction: FlexDirection::Row) {
            Text(content: content, color: TuiTheme::USER.to_iocraft())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_glyph_is_u276f() {
        // ❯ = U+276F = 0xE2 0x9D 0xAF, then ASCII space.
        assert_eq!(PREFIX.as_bytes(), &[0xE2, 0x9D, 0xAF, 0x20]);
    }

    #[test]
    fn empty_command_empty() {
        assert_eq!(render_command_to_string("", "x", false), "");
    }

    #[test]
    fn slash_form_with_and_without_args() {
        assert_eq!(
            render_command_to_string("clear", "", false),
            "\u{276F} /clear"
        );
        assert_eq!(
            render_command_to_string("model", "sonnet", false),
            "\u{276F} /model sonnet"
        );
    }

    #[test]
    fn skill_form() {
        assert_eq!(
            render_command_to_string("brainstorm", "", true),
            "\u{276F} Skill(brainstorm)"
        );
    }
}

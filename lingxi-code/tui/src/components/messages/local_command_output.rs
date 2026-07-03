//! `UserLocalCommandOutputMessage` — `  ⎿  ` gutter + markdown body.
//!
//! Literal locks (byte-for-byte from claude-code):
//!   - gutter: `  ⎿  ` (2 spaces + U+23BF + 2 spaces, dim)
//!   - empty stdout+stderr → NO_CONTENT_MESSAGE = `(no content)`
//!   - body rendered as markdown (`render::markdown`)
//!   - SCOPE: IndentedContent path only; CloudLaunchContent (◇/◆ diamond
//!     prefixed lines) defers to M8.
//!   source: claude-code/src/components/messages/UserLocalCommandOutputMessage.tsx
//!           + constants/messages.ts (NO_CONTENT_MESSAGE)
#![allow(clippy::doc_markdown, clippy::doc_lazy_continuation)]

use iocraft::prelude::*;

use crate::render::markdown::{render as render_markdown, MarkdownTheme};
use crate::render::{StyleColor, StyledLine};
use crate::render_iocraft::StyleColorIocraftExt;
use crate::theme::TuiTheme;

/// Dim gutter prepended to each indented content block (2 spaces + U+23BF +
/// 2 spaces).
pub const GUTTER: &str = "  \u{23BF}  ";
/// Rendered when both stdout and stderr are empty (claude-code
/// `constants/messages.ts` `NO_CONTENT_MESSAGE`).
pub const NO_CONTENT_MESSAGE: &str = "(no content)";

/// Props for [`UserLocalCommandOutputMessage`].
#[derive(Debug, Clone, Default, Props)]
pub struct UserLocalCommandOutputProps {
    /// Local-command stdout.
    pub stdout: String,
    /// Local-command stderr.
    pub stderr: String,
}

/// Flatten a markdown block → plain text (one line per visual line). Routes
/// through M7-01's `render::markdown::render`; the string oracle joins the
/// styled lines' plain text with `\n` so measurement == render.
fn markdown_plain(text: &str) -> String {
    let theme = MarkdownTheme {
        inline_code: StyleColor::Default,
        // (M7-15) oracle flattens to plain text; code-block theme is immaterial.
        code_theme: crate::theme::ThemeName::Dark,
    };
    render_markdown(text, &theme)
        .iter()
        .map(StyledLine::plain_text)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Pure string renderer. Trims stdout/stderr; renders each non-empty part as
/// a markdown-flattened block under the gutter; empty → `NO_CONTENT_MESSAGE`.
///
/// Each gutter prefixes only the first line of a block (claude-code's
/// `IndentedContent` puts the `⎿` glyph in a fixed-width column to the left of
/// a flex markdown column); continuation lines align under it with plain
/// indentation matching the gutter width.
#[must_use]
pub fn render_local_output_to_string(stdout: &str, stderr: &str) -> String {
    let out = stdout.trim();
    let err = stderr.trim();
    if out.is_empty() && err.is_empty() {
        return NO_CONTENT_MESSAGE.to_string();
    }
    // Continuation indent matches the gutter's display width (5 columns).
    let cont = " ".repeat(5);
    let mut blocks: Vec<String> = Vec::new();
    for part in [out, err] {
        if part.is_empty() {
            continue;
        }
        let flat = markdown_plain(part);
        let mut block = String::new();
        for (i, line) in flat.lines().enumerate() {
            if i == 0 {
                block.push_str(GUTTER);
            } else {
                block.push('\n');
                block.push_str(&cont);
            }
            block.push_str(line);
        }
        // A flat body with no lines (empty markdown) still gets the gutter.
        if block.is_empty() {
            block.push_str(GUTTER);
        }
        blocks.push(block);
    }
    blocks.join("\n")
}

/// iocraft component. Renders the gutter-prefixed markdown body as dim text.
#[component]
pub fn UserLocalCommandOutputMessage(
    props: &UserLocalCommandOutputProps,
) -> impl Into<AnyElement<'static>> {
    let body = render_local_output_to_string(&props.stdout, &props.stderr);
    element! {
        View(flex_direction: FlexDirection::Column) {
            Text(content: body, color: TuiTheme::DIM.to_iocraft())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gutter_bytes() {
        // "  " + U+23BF (0xE2 0x8E 0xBF) + "  ".
        assert_eq!(
            GUTTER.as_bytes(),
            &[0x20, 0x20, 0xE2, 0x8E, 0xBF, 0x20, 0x20]
        );
    }

    #[test]
    fn no_content_literal() {
        assert_eq!(NO_CONTENT_MESSAGE, "(no content)");
    }

    #[test]
    fn no_content_when_empty() {
        assert_eq!(render_local_output_to_string("  ", "\n"), "(no content)");
    }

    #[test]
    fn gutter_prepended() {
        assert_eq!(render_local_output_to_string("ok", ""), "  \u{23BF}  ok");
    }
}

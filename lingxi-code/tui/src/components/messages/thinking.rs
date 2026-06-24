//! `AssistantThinkingMessage` — `∴ Thinking` collapsed / `∴ Thinking…` expanded.
//!
//! Literal locks (claude-code `AssistantThinkingMessage.tsx` +
//! `HighlightedThinkingText.tsx`):
//!   - collapsed header: `∴ Thinking` (U+2234 + space + "Thinking") dim+italic,
//!     followed by a `(ctrl+o to expand)` hint (`CtrlOToExpand`).
//!   - expanded header: `∴ Thinking…` (trailing U+2026), then markdown body
//!     indented 2 spaces (`paddingLeft={2}`), dim.
#![allow(clippy::needless_pass_by_value)]

use iocraft::prelude::*;

use crate::render::markdown::{render as render_markdown, MarkdownTheme};
use crate::render::StyleColor;
use crate::theme::TuiTheme;

/// `∴ ` marker. U+2234 (0xE2 0x88 0xB4) + ASCII space. dim+italic.
pub const THINKING_MARKER: &str = "\u{2234} ";
/// Collapsed-state expand hint (claude-code `CtrlOToExpand` surface, which
/// renders `(ctrl+o to expand)`).
pub const EXPAND_HINT: &str = "(ctrl+o to expand)";
/// Per-line body indent (2 spaces) — claude-code `paddingLeft={2}`.
pub const INDENT: &str = "  ";

/// Props for [`AssistantThinkingMessage`].
#[derive(Debug, Clone, Default, Props)]
pub struct ThinkingProps {
    /// The thinking text (markdown when expanded).
    pub thinking: String,
    /// `true` → render the full markdown body; `false` → header + hint only.
    pub expanded: bool,
}

/// Pure-string renderer (snapshot oracle).
///
/// Expanded form mirrors claude-code's `gap={1}` column: the `∴ Thinking…`
/// header, ONE blank row, then the indented markdown body.
#[must_use]
pub fn render_thinking_to_string(props: ThinkingProps) -> String {
    if !props.expanded {
        return format!("{THINKING_MARKER}Thinking {EXPAND_HINT}");
    }
    // Expanded: `∴ Thinking…`, a gap=1 blank row, then markdown body indented 2.
    let body = markdown_plain(&props.thinking);
    let mut lines = vec![format!("{THINKING_MARKER}Thinking\u{2026}")];
    if !body.is_empty() {
        lines.push(String::new()); // gap={1} blank row between header and body
        for line in body.lines() {
            lines.push(format!("{INDENT}{line}"));
        }
    }
    lines.join("\n")
}

/// Flatten markdown → plain text for the string oracle. Routes through
/// M7-01's `render::markdown::render`, then joins the styled lines' plain text
/// with `\n` (styling shows in the iocraft component, not the snapshot
/// string).
fn markdown_plain(text: &str) -> String {
    let theme = MarkdownTheme {
        inline_code: StyleColor::Default,
        // (M7-15) oracle flattens to plain text; code-block theme is immaterial.
        code_theme: crate::theme::ThemeName::Dark,
    };
    let lines = render_markdown(text, &theme);
    lines
        .iter()
        .map(crate::render::StyledLine::plain_text)
        .collect::<Vec<_>>()
        .join("\n")
}

/// iocraft component. Collapsed: a single dim+italic `∴ Thinking (ctrl+o to
/// expand)` line. Expanded: dim+italic `∴ Thinking…` header, a `gap={1}` blank
/// row, then the markdown body indented 2 — dim but NOT italic (claude-code
/// `AssistantThinkingMessage.tsx`: header `<Text dimColor italic>`, body
/// `<Box paddingLeft={2}><Markdown dimColor>` inside a `gap={1}` column).
#[component]
pub fn AssistantThinkingMessage(props: &ThinkingProps) -> impl Into<AnyElement<'static>> {
    if !props.expanded {
        let header = format!("{THINKING_MARKER}Thinking {EXPAND_HINT}");
        return element! {
            View(flex_direction: FlexDirection::Column) {
                Text(content: header, color: TuiTheme::DIM, italic: true)
            }
        }
        .into_any();
    }
    let header = format!("{THINKING_MARKER}Thinking\u{2026}");
    let body = {
        let flat = markdown_plain(&props.thinking);
        flat.lines()
            .map(|l| format!("{INDENT}{l}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    element! {
        // gap=1 inserts the blank row between the header and the body.
        View(flex_direction: FlexDirection::Column, gap: 1) {
            Text(content: header, color: TuiTheme::DIM, italic: true)
            Text(content: body, color: TuiTheme::DIM)
        }
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_bytes() {
        // U+2234 = 0xE2 0x88 0xB4, then ASCII space.
        assert_eq!(THINKING_MARKER.as_bytes(), &[0xE2, 0x88, 0xB4, 0x20]);
    }
}

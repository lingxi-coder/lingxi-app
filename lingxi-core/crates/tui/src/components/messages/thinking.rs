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
#[must_use]
pub fn render_thinking_to_string(props: ThinkingProps) -> String {
    if !props.expanded {
        return format!("{THINKING_MARKER}Thinking {EXPAND_HINT}");
    }
    // Expanded: `∴ Thinking…` then markdown body, each line indented 2.
    let body = markdown_plain(&props.thinking);
    let mut out = format!("{THINKING_MARKER}Thinking\u{2026}");
    for line in body.lines() {
        out.push('\n');
        out.push_str(INDENT);
        out.push_str(line);
    }
    out
}

/// Flatten markdown → plain text for the string oracle. Routes through
/// M7-01's `render::markdown::render`, then joins the styled lines' plain text
/// with `\n` (styling shows in the iocraft component, not the snapshot
/// string).
fn markdown_plain(text: &str) -> String {
    let theme = MarkdownTheme {
        inline_code: StyleColor::Default,
    };
    let lines = render_markdown(text, &theme);
    lines
        .iter()
        .map(crate::render::StyledLine::plain_text)
        .collect::<Vec<_>>()
        .join("\n")
}

/// iocraft component — dim+italic header; expanded body rendered dim.
#[component]
pub fn AssistantThinkingMessage(props: &ThinkingProps) -> impl Into<AnyElement<'static>> {
    let body = render_thinking_to_string(props.clone());
    element! {
        View(flex_direction: FlexDirection::Column) {
            Text(content: body, color: TuiTheme::DIM, italic: true)
        }
    }
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

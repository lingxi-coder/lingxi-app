//! `UserPlanMessage` — bordered "Plan to implement" + markdown body.
//!
//! Literal locks (byte-for-byte from claude-code):
//!   - header: `Plan to implement` (bold, color `planMode` → `theme.plan_mode`)
//!   - body: markdown (`render::markdown`), round-bordered box, borderColor
//!     `planMode` → `theme.plan_mode`.
//!   source: claude-code/src/components/messages/UserPlanMessage.tsx
#![allow(clippy::doc_markdown, clippy::doc_lazy_continuation)]

use iocraft::prelude::*;

use crate::render::markdown::{render as render_markdown, MarkdownTheme};
use crate::render::{StyleColor, StyledLine};
use crate::theme::Theme;

/// Exact header literal.
pub const HEADER: &str = "Plan to implement";

/// Props for [`UserPlanMessage`].
#[derive(Debug, Clone, Default, Props)]
pub struct UserPlanProps {
    /// Markdown plan content.
    pub plan_content: String,
    /// (M7-15) Active palette — `plan_mode` accent centralized here.
    pub theme: Theme,
}

/// Flatten the markdown plan body → plain text (one line per visual line).
/// Routes through M7-01's `render::markdown::render` so the string oracle and
/// the measurement proxy count the SAME flattened lines the component draws.
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

/// Pure string renderer: header, a (user-plan-missing-blank-line) blank line
/// (claude-code `marginBottom={1}` on the header), then the flattened
/// markdown plan body.
#[must_use]
pub fn render_plan_to_string(plan_content: &str) -> String {
    let body = markdown_plain(plan_content);
    if body.is_empty() {
        return HEADER.to_string();
    }
    format!("{HEADER}\n\n{body}")
}

/// iocraft component. Body routes through `render::markdown`; border is a
/// `round`-style View (borderColor `planMode` → `theme.plan_mode`).
///
/// (M7-15) The header + border accent is centralized into the active
/// [`Theme`]'s `plan_mode` color (was the dark-only `TuiTheme::ASSISTANT`
/// shim); the body stays terminal-default (claude-code renders plan body
/// uncolored).
#[component]
pub fn UserPlanMessage(props: &UserPlanProps) -> impl Into<AnyElement<'static>> {
    let body = render_plan_to_string(&props.plan_content);
    // Header is the first line, then the (user-plan-missing-blank-line) blank
    // line the oracle inserts; body lines follow. The component renders that
    // gap via the View's native `gap: 1` (not an embedded leading newline —
    // iocraft doesn't lay a `Text`'s embedded blank line out the same way).
    let mut lines = body.lines();
    let _ = lines.next(); // skip header (drawn separately, bold)
    let _ = lines.next(); // skip the blank gap line (the View's `gap: 1` draws it)
    let body_text = lines.collect::<Vec<_>>().join("\n");
    let accent = props.theme.plan_mode;
    element! {
        View(
            flex_direction: FlexDirection::Column,
            border_style: BorderStyle::Round,
            border_color: accent,
            gap: 1,
        ) {
            Text(content: HEADER, color: accent, weight: Weight::Bold)
            Text(content: body_text, color: Color::Reset)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_present() {
        assert!(render_plan_to_string("x").starts_with("Plan to implement"));
    }

    #[test]
    fn header_literal() {
        assert_eq!(HEADER, "Plan to implement");
    }

    #[test]
    fn plan_body_included() {
        let s = render_plan_to_string("- step one");
        assert!(s.contains("step one"), "got: {s}");
    }

    #[test]
    fn blank_line_between_header_and_body() {
        // (user-plan-missing-blank-line)
        let s = render_plan_to_string("- step one");
        assert_eq!(s, "Plan to implement\n\n- step one");
    }
}

//! `UserPlanMessage` — bordered "Plan to implement" + markdown body.
//!
//! Literal locks (byte-for-byte from claude-code):
//!   - header: `Plan to implement` (bold, color `planMode` → ASSISTANT)
//!     // TODO(M7-15): `planMode` → dedicated plan-mode color (header + border)
//!   - body: markdown (`render::markdown`), round-bordered box, borderColor
//!     `planMode` → ASSISTANT.
//!   source: claude-code/src/components/messages/UserPlanMessage.tsx
#![allow(clippy::doc_markdown, clippy::doc_lazy_continuation)]

use iocraft::prelude::*;

use crate::render::markdown::{render as render_markdown, MarkdownTheme};
use crate::render::{StyleColor, StyledLine};
use crate::theme::TuiTheme;

/// Exact header literal.
pub const HEADER: &str = "Plan to implement";

/// Props for [`UserPlanMessage`].
#[derive(Debug, Clone, Default, Props)]
pub struct UserPlanProps {
    /// Markdown plan content.
    pub plan_content: String,
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

/// Pure string renderer: header + flattened markdown plan body.
#[must_use]
pub fn render_plan_to_string(plan_content: &str) -> String {
    let body = markdown_plain(plan_content);
    if body.is_empty() {
        return HEADER.to_string();
    }
    format!("{HEADER}\n{body}")
}

/// iocraft component. Body routes through `render::markdown`; border is a
/// `round`-style View (borderColor `planMode` → ASSISTANT).
#[component]
pub fn UserPlanMessage(props: &UserPlanProps) -> impl Into<AnyElement<'static>> {
    let body = render_plan_to_string(&props.plan_content);
    // Header is the first line; body lines follow.
    let mut lines = body.lines();
    let _ = lines.next(); // skip header (drawn separately, bold)
    let body_text = lines.collect::<Vec<_>>().join("\n");
    element! {
        View(
            flex_direction: FlexDirection::Column,
            border_style: BorderStyle::Round,
            border_color: TuiTheme::ASSISTANT,
        ) {
            Text(content: HEADER, color: TuiTheme::ASSISTANT, weight: Weight::Bold)
            Text(content: body_text, color: TuiTheme::USER)
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
}

//! `AdvisorMessage` — advisor block (`server_tool_use` / result / error /
//! redacted).
//!
//! Literal lock (claude-code `AdvisorMessage.tsx`):
//!   - `server_tool_use`: `Advising` (bold) (+ ` using {model}` dim
//!     + ` · {input}` dim).
//!   - `advisor_result` non-verbose: `✔ Advisor has reviewed the conversation
//!     and will apply the feedback` (dim) + `CtrlOToExpand`; verbose → raw
//!     `{text}` dim.
//!   - `advisor_tool_result_error`: `Advisor unavailable ({error_code})`
//!     (error).
//!   - `advisor_redacted_result`: `✔ Advisor has reviewed the conversation and
//!     will apply the feedback` dim (no expand).
//!
//! `✔` = `figures.tick` (U+2714).
#![allow(clippy::needless_pass_by_value)]

use iocraft::prelude::*;

use crate::render::markdown::{render as render_markdown, MarkdownTheme};
use crate::render::StyleColor;
use crate::state::AdvisorKind;
use crate::theme::Theme;

/// `figures.tick`. U+2714 (0xE2 0x9C 0x94).
pub const TICK: &str = "\u{2714}";
/// Locked review line (non-verbose result + redacted result).
pub const REVIEWED_LINE: &str = "Advisor has reviewed the conversation and will apply the feedback";

/// Props for [`AdvisorMessage`].
#[derive(Debug, Clone, Default, Props)]
pub struct AdvisorProps {
    /// Advisor block content.
    pub kind: AdvisorKind,
    /// `true` → render the full result text (markdown via `render::markdown`).
    pub verbose: bool,
    /// (M7-15) Active palette — error/dim colors centralized here.
    pub theme: Theme,
}

/// Flatten markdown → plain text for the string oracle (verbose result body).
fn markdown_plain(text: &str) -> String {
    let theme = MarkdownTheme {
        inline_code: StyleColor::Default,
        // (M7-15) oracle flattens to plain text; code-block theme is immaterial.
        code_theme: crate::theme::ThemeName::Dark,
    };
    render_markdown(text, &theme)
        .iter()
        .map(crate::render::StyledLine::plain_text)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Pure-string renderer.
#[must_use]
pub fn render_advisor_to_string(props: AdvisorProps) -> String {
    match &props.kind {
        AdvisorKind::ServerToolUse { model, input } => {
            let mut out = "Advising".to_string();
            if let Some(m) = model {
                out.push_str(&format!(" using {m}"));
            }
            if let Some(i) = input {
                out.push_str(&format!(" \u{00B7} {i}")); // ` · ` middot
            }
            out
        }
        AdvisorKind::Result { text } => {
            if props.verbose {
                markdown_plain(text)
            } else {
                format!("{TICK} {REVIEWED_LINE}")
            }
        }
        AdvisorKind::RedactedResult => format!("{TICK} {REVIEWED_LINE}"),
        AdvisorKind::Error { error_code } => format!("Advisor unavailable ({error_code})"),
    }
}

/// iocraft component.
///
/// (M7-15) Colors centralized into the active [`Theme`]: error→`theme.error`,
/// everything else→`theme.dim`. The `server_tool_use` block restores the
/// claude-code per-span styling — `Advising` is bold, the ` using {model}
/// · {input}` descriptor dim — by splitting the line into two runs on one row.
#[component]
pub fn AdvisorMessage(props: &AdvisorProps) -> impl Into<AnyElement<'static>> {
    let body = render_advisor_to_string(props.clone());
    let theme = props.theme;
    // server_tool_use: bold `Advising` + dim descriptor (per-span, on one row).
    if let AdvisorKind::ServerToolUse { .. } = &props.kind {
        // `body` is "Advising[ using {model}][ · {input}]"; split off the bold
        // leading "Advising" word, the remainder is the dim descriptor.
        let descriptor = body.strip_prefix("Advising").unwrap_or("").to_string();
        return element! {
            View(flex_direction: FlexDirection::Column) {
                View(flex_direction: FlexDirection::Row) {
                    Text(content: "Advising".to_string(), color: theme.dim, weight: Weight::Bold)
                    Text(content: descriptor, color: theme.dim)
                }
            }
        }
        .into_any();
    }
    let color = match &props.kind {
        AdvisorKind::Error { .. } => theme.error,
        _ => theme.dim,
    };
    element! {
        View(flex_direction: FlexDirection::Column) {
            Text(content: body, color: color)
        }
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_is_u2714() {
        // ✔ = U+2714 = 0xE2 0x9C 0x94.
        assert_eq!(TICK.as_bytes(), &[0xE2, 0x9C, 0x94]);
    }
}

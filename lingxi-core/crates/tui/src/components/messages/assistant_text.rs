//! `AssistantTextMessage` — renders assistant text in cyan with a dot marker.
//!
//! Locked literals (plan §T0):
//!   L4 dot marker = "● " (U+25CF + space) — claude-code parity
//!   assistant color = cyan

use iocraft::prelude::*;

use crate::theme::TuiTheme;

/// Props for `AssistantTextMessage`.
#[derive(Default, Props)]
pub struct AssistantTextMessageProps {
    /// Assistant body (multi-line; rendered as a single Text block).
    pub body: String,
}

/// Render an assistant text body in cyan with the locked `"● "` prefix.
#[component]
pub fn AssistantTextMessage(
    props: &AssistantTextMessageProps,
) -> impl Into<AnyElement<'static>> {
    let content = format!("● {}", props.body);
    element! {
        View(flex_direction: FlexDirection::Column) {
            Text(content: content, color: TuiTheme::ASSISTANT)
        }
    }
}

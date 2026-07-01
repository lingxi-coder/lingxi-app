//! `AssistantRedactedThinkingMessage` — `✻ Thinking…` dim+italic, single line.
//!
//! Literal lock (claude-code `AssistantRedactedThinkingMessage.tsx`):
//! `✻ Thinking…` (U+273B + space + "Thinking" + U+2026), `dimColor` italic.
use iocraft::prelude::*;

use crate::theme::TuiTheme;
use crate::render_iocraft::StyleColorIocraftExt;

/// `✻ ` marker. U+273B (0xE2 0x9C 0xBB) + ASCII space.
pub const REDACTED_MARKER: &str = "\u{273B} ";

/// Pure-string renderer (no props — the line is fixed).
#[must_use]
pub fn render_redacted_thinking_to_string() -> String {
    format!("{REDACTED_MARKER}Thinking\u{2026}")
}

/// iocraft component.
#[component]
pub fn AssistantRedactedThinkingMessage() -> impl Into<AnyElement<'static>> {
    let body = render_redacted_thinking_to_string();
    element! {
        View(flex_direction: FlexDirection::Column) {
            Text(content: body, color: TuiTheme::DIM.to_iocraft(), italic: true)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn marker_bytes() {
        assert_eq!(REDACTED_MARKER.as_bytes(), &[0xE2, 0x9C, 0xBB, 0x20]);
    }
}

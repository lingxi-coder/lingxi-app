//! `SystemTextMessage` — level-aware system line.
//!
//! Literal lock (claude-code `SystemTextMessage.tsx`): info level → plain dim
//! body, no marker. Non-info → `●` (`BLACK_CIRCLE`) marker + body; warning →
//! yellow, error → red. (We lock the non-darwin `BLACK_CIRCLE` `●` to match
//! M6-04's existing tool-use `MARKER`.)
#![allow(clippy::needless_pass_by_value)]

use iocraft::prelude::*;

use crate::state::SystemLevel;
use crate::theme::TuiTheme;

/// `● ` marker (`BLACK_CIRCLE`, non-darwin form). U+25CF + ASCII space.
pub const MARKER: &str = "\u{25CF} ";

/// Props for [`SystemTextMessage`].
#[derive(Debug, Clone, Default, Props)]
pub struct SystemTextProps {
    /// Message body.
    pub body: String,
    /// Severity → marker/color.
    pub level: SystemLevel,
}

/// Pure-string renderer.
#[must_use]
pub fn render_system_text_to_string(props: SystemTextProps) -> String {
    match props.level {
        SystemLevel::Info => props.body,
        SystemLevel::Warning | SystemLevel::Error => format!("{MARKER}{}", props.body),
    }
}

/// iocraft component.
#[component]
pub fn SystemTextMessage(props: &SystemTextProps) -> impl Into<AnyElement<'static>> {
    let body = render_system_text_to_string(props.clone());
    let color = match props.level {
        SystemLevel::Info => TuiTheme::DIM,
        // TODO(M7-15): theme constants for warning/error.
        SystemLevel::Warning => Color::Yellow,
        SystemLevel::Error => TuiTheme::ERROR,
    };
    element! {
        View(flex_direction: FlexDirection::Column) {
            Text(content: body, color: color)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_bytes() {
        // ● = U+25CF = 0xE2 0x97 0x8F, then ASCII space.
        assert_eq!(MARKER.as_bytes(), &[0xE2, 0x97, 0x8F, 0x20]);
    }
}

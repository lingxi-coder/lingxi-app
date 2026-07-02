//! `SystemTextMessage` — level-aware system line.
//!
//! Literal lock (claude-code `SystemTextMessage.tsx`): info level → plain dim
//! body, no marker. Non-info → `●` (`BLACK_CIRCLE`) marker + body; warning →
//! yellow, error → red. (We lock the non-darwin `BLACK_CIRCLE` `●` to match
//! M6-04's existing tool-use `MARKER`.)
#![allow(clippy::needless_pass_by_value)]

use iocraft::prelude::*;

use crate::render_iocraft::StyleColorIocraftExt;
use crate::state::SystemLevel;
use crate::theme::Theme;

/// `BLACK_CIRCLE` marker (ma-03): `⏺ ` (U+23FA) on macOS, `● ` (U+25CF)
/// elsewhere — followed by an ASCII space.
pub const MARKER: &str = if cfg!(target_os = "macos") {
    "\u{23FA} "
} else {
    "\u{25CF} "
};

/// Props for [`SystemTextMessage`].
#[derive(Debug, Clone, Default, Props)]
pub struct SystemTextProps {
    /// Message body.
    pub body: String,
    /// Severity → marker/color.
    pub level: SystemLevel,
    /// (M7-15) Active palette — warning/error/dim colors centralized here.
    pub theme: Theme,
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
    // (M7-15) Centralized: info→dim, warning→theme.warning, error→theme.error.
    let color = match props.level {
        SystemLevel::Info => props.theme.dim,
        SystemLevel::Warning => props.theme.warning,
        SystemLevel::Error => props.theme.error,
    };
    element! {
        View(flex_direction: FlexDirection::Column) {
            Text(content: body, color: color.to_iocraft())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_bytes() {
        // (ma-03) `⏺ ` on macOS, `● ` elsewhere — glyph + ASCII space.
        let glyph = if cfg!(target_os = "macos") {
            "\u{23FA}"
        } else {
            "\u{25CF}"
        };
        assert_eq!(MARKER, format!("{glyph} "));
    }
}

//! `UserAgentNotificationMessage` — background-agent status notification.
//!
//! Literal lock (claude-code `UserAgentNotificationMessage.tsx`):
//! `{BLACK_CIRCLE} {summary}` where the circle's color is the status color
//! (completed→success, failed→error, killed→warning, else→text). Empty
//! summary renders nothing (claude-code returns null).
//! (agent-notification-black-circle-darwin) `BLACK_CIRCLE` is
//! platform-conditional: `⏺` (U+23FA) on macOS, `●` (U+25CF) elsewhere — see
//! [`MARKER`].
#![allow(clippy::needless_pass_by_value)]

use iocraft::prelude::*;

use crate::theme::Theme;

/// `BLACK_CIRCLE` marker (ma-03): `⏺ ` (U+23FA) on macOS, `● ` (U+25CF)
/// elsewhere — followed by a space.
pub const MARKER: &str = if cfg!(target_os = "macos") {
    "\u{23FA} "
} else {
    "\u{25CF} "
};

/// Props for [`UserAgentNotificationMessage`].
#[derive(Debug, Clone, Default, Props)]
pub struct UserAgentNotificationProps {
    /// Summary line.
    pub summary: String,
    /// Optional status string.
    pub status: Option<String>,
    /// Active palette.
    pub theme: Theme,
}

/// Marker color for a status string.
#[must_use]
pub fn status_color(status: Option<&str>, theme: &Theme) -> Color {
    match status {
        Some("completed") => theme.success,
        Some("failed") => theme.error,
        Some("killed") => theme.warning,
        _ => theme.text,
    }
}

/// Pure-string renderer. Empty summary → empty string.
#[must_use]
pub fn render_user_agent_notification_to_string(props: UserAgentNotificationProps) -> String {
    if props.summary.is_empty() {
        return String::new();
    }
    format!("{MARKER}{}", props.summary)
}

/// iocraft component. Marker colored by status; summary in default text.
#[component]
pub fn UserAgentNotificationMessage(
    props: &UserAgentNotificationProps,
) -> impl Into<AnyElement<'static>> {
    if props.summary.is_empty() {
        return element! { View {} }.into_any();
    }
    let marker_color = status_color(props.status.as_deref(), &props.theme);
    let summary = props.summary.clone();
    element! {
        View(flex_direction: FlexDirection::Row) {
            Text(content: MARKER, color: marker_color)
            Text(content: summary, color: props.theme.text)
        }
    }
    .into_any()
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

    #[test]
    fn renders_marker_and_summary() {
        let out = render_user_agent_notification_to_string(UserAgentNotificationProps {
            summary: "Task done".into(),
            status: Some("completed".into()),
            theme: Theme::dark(),
        });
        assert_eq!(out, format!("{MARKER}Task done"));
    }

    #[test]
    fn empty_summary_is_empty() {
        let out = render_user_agent_notification_to_string(UserAgentNotificationProps {
            summary: String::new(),
            status: None,
            theme: Theme::dark(),
        });
        assert_eq!(out, "");
    }

    #[test]
    fn status_color_map() {
        let t = Theme::dark();
        assert_eq!(status_color(Some("completed"), &t), t.success);
        assert_eq!(status_color(Some("failed"), &t), t.error);
        assert_eq!(status_color(Some("killed"), &t), t.warning);
        assert_eq!(status_color(Some("other"), &t), t.text);
        assert_eq!(status_color(None, &t), t.text);
    }
}

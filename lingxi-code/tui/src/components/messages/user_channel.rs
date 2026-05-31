//! `UserChannelMessage` — inbound channel message.
//!
//! Literal lock (claude-code `UserChannelMessage.tsx`):
//! `{CHANNEL_ARROW} {serverLeaf}[ · {user}]: {content}` — arrow in
//! `suggestion`, server/user dim, content default. `serverLeaf` is the
//! substring after the last `:` of the source (e.g. `plugin:slack:slack` →
//! `slack`). Content has whitespace collapsed and is truncated to 60 with `…`.
#![allow(clippy::needless_pass_by_value)]

use iocraft::prelude::*;

use crate::theme::Theme;

/// `← ` inbound-channel arrow (U+2190 + space).
pub const ARROW: &str = "\u{2190} ";
/// ` · ` user separator (space + U+00B7 + space).
pub const MIDDOT: &str = " \u{00B7} ";

/// Leaf server name: substring after the last `:` (or the whole string).
#[must_use]
pub fn display_server_name(source: &str) -> &str {
    match source.rfind(':') {
        Some(i) => &source[i + 1..],
        None => source,
    }
}

/// Collapse runs of whitespace to a single space and trim.
fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Truncate to `max` chars, appending `…` when cut.
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let kept: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{kept}\u{2026}")
}

/// Props for [`UserChannelMessage`].
#[derive(Debug, Clone, Default, Props)]
pub struct UserChannelProps {
    /// Source server (raw).
    pub server: String,
    /// Optional sender user.
    pub user: Option<String>,
    /// Message content.
    pub content: String,
    /// Active palette.
    pub theme: Theme,
}

/// The dim middle segment: `serverLeaf[ · user]: `.
fn middle_segment(server: &str, user: Option<&str>) -> String {
    let leaf = display_server_name(server);
    match user {
        Some(u) => format!("{leaf}{MIDDOT}{u}: "),
        None => format!("{leaf}: "),
    }
}

/// Pure-string renderer.
#[must_use]
pub fn render_user_channel_to_string(props: UserChannelProps) -> String {
    let mid = middle_segment(&props.server, props.user.as_deref());
    let body = truncate_chars(&collapse_ws(&props.content), 60);
    format!("{ARROW}{mid}{body}")
}

/// iocraft component. Arrow (suggestion) + dim middle + default content.
#[component]
pub fn UserChannelMessage(props: &UserChannelProps) -> impl Into<AnyElement<'static>> {
    let mid = middle_segment(&props.server, props.user.as_deref());
    let body = truncate_chars(&collapse_ws(&props.content), 60);
    element! {
        View(flex_direction: FlexDirection::Row) {
            Text(content: ARROW, color: props.theme.suggestion)
            Text(content: mid, color: props.theme.dim)
            Text(content: body, color: props.theme.text)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrow_and_middot_bytes() {
        assert_eq!(ARROW.as_bytes(), &[0xE2, 0x86, 0x90, 0x20]); // ← + space
        assert_eq!(MIDDOT.as_bytes(), &[0x20, 0xC2, 0xB7, 0x20]); // sp · sp
    }

    #[test]
    fn server_leaf() {
        assert_eq!(display_server_name("plugin:slack:slack"), "slack");
        assert_eq!(display_server_name("slack"), "slack");
    }

    #[test]
    fn with_user() {
        let out = render_user_channel_to_string(UserChannelProps {
            server: "plugin:slack:slack".into(),
            user: Some("bob".into()),
            content: "hello   there".into(),
            theme: Theme::dark(),
        });
        assert_eq!(out, "\u{2190} slack \u{00B7} bob: hello there");
    }

    #[test]
    fn without_user_and_truncation() {
        let long = "x".repeat(80);
        let out = render_user_channel_to_string(UserChannelProps {
            server: "irc".into(),
            user: None,
            content: long,
            theme: Theme::dark(),
        });
        // 59 x's + ellipsis.
        assert_eq!(out, format!("\u{2190} irc: {}\u{2026}", "x".repeat(59)));
    }
}

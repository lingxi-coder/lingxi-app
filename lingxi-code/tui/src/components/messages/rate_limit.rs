//! `RateLimitMessage` — error text + optional dim upsell line.
//!
//! Literal lock (claude-code `RateLimitMessage.tsx` `getUpsellMessage`). Note
//! the curly apostrophe U+2019 in "you’re" and the ellipsis U+2026 in
//! "Opening your options…". (rate-limit-missing-gutter) Wrapped in the
//! `MessageResponse` `  ⎿  ` gutter, same as `user_tool_result.rs`.
#![allow(clippy::needless_pass_by_value)]

use iocraft::prelude::*;

use crate::components::messages::user_tool_result::{INDENT, MARKER};
use crate::theme::TuiTheme;

/// Props for [`RateLimitMessage`].
#[derive(Debug, Clone, Default, Props)]
pub struct RateLimitProps {
    /// The rate-limit notice text (error-colored).
    pub text: String,
    /// Optional dim upsell line (one of [`upsell`]'s locked literals).
    pub upsell: Option<String>,
}

/// Locked upsell strings. Mirrors claude-code `getUpsellMessage`.
pub mod upsell {
    /// Max-20x + extra-usage enabled.
    pub const EXTRA_USAGE_FINISH: &str = "/extra-usage to finish what you\u{2019}re working on.";
    /// Max-20x, extra-usage disabled.
    pub const LOGIN_SWITCH: &str = "/login to switch to an API usage-billed account.";
    /// Auto-open menu.
    pub const OPENING_OPTIONS: &str = "Opening your options\u{2026}";
    /// Default (non-team, no extra-usage).
    pub const UPGRADE: &str = "/upgrade to increase your usage limit.";
    /// Team/enterprise, no billing access.
    pub const EXTRA_USAGE_ADMIN: &str = "/extra-usage to request more usage from your admin.";
    /// Fallback (team/enterprise generic).
    pub const UPGRADE_OR_EXTRA: &str =
        "/upgrade or /extra-usage to finish what you\u{2019}re working on.";
}

/// Pure-string renderer. (rate-limit-missing-gutter) `  ⎿  ` gutter on the
/// first row, 5-space `INDENT` on the upsell row.
#[must_use]
pub fn render_rate_limit_to_string(props: RateLimitProps) -> String {
    match props.upsell {
        Some(u) => format!("{MARKER}{}\n{INDENT}{u}", props.text),
        None => format!("{MARKER}{}", props.text),
    }
}

/// iocraft component — error text + optional dim upsell, gutter-wrapped.
#[component]
pub fn RateLimitMessage(props: &RateLimitProps) -> impl Into<AnyElement<'static>> {
    let text = format!("{MARKER}{}", props.text);
    // `Option<Element>` is iterable, so the `#(...)` fragment renders zero or
    // one upsell line (matches user_tool_result.rs's Vec-fragment pattern).
    let upsell: Vec<AnyElement<'static>> = props
        .upsell
        .clone()
        .map(|u| {
            element! { Text(content: format!("{INDENT}{u}"), color: TuiTheme::DIM) }.into_any()
        })
        .into_iter()
        .collect();
    element! {
        View(flex_direction: FlexDirection::Column) {
            Text(content: text, color: TuiTheme::ERROR)
            #(upsell)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::upsell;

    #[test]
    fn extra_usage_uses_curly_apostrophe() {
        assert!(upsell::EXTRA_USAGE_FINISH.contains('\u{2019}'));
        assert!(upsell::UPGRADE_OR_EXTRA.contains('\u{2019}'));
    }

    #[test]
    fn opening_options_uses_ellipsis() {
        assert!(upsell::OPENING_OPTIONS.ends_with('\u{2026}'));
    }
}

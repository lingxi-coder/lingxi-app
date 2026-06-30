//! `UserTextMessage` — renders the user's submitted prompt.
//!
//! Locked literals (plan §T0):
//!   L5 prefix = "> "
//!   user color = default fg (no override)
//!
//! Empty-message guard (claude-code parity, plan §A4): a body that is only
//! prompt-XML scaffolding tags (`<commit_analysis>…`, `<context>…`,
//! `<function_analysis>…`, `<pr_analysis>…`) — or the literal `(no content)` —
//! renders NOTHING, matching claude-code's `UserTextMessage.tsx:39-41`
//! (`return null` on `(no content)`) and the broader `isEmptyMessageText`
//! suppression applied to the text body. We return an EMPTY `View` (no `"> "`
//! prefix row) so the whole row disappears.
//!
//! (RRS-08) A body equal to [`INTERRUPT_MESSAGE`] (or the tool-use variant)
//! renders the SAME `InterruptedByUser` line a canceled/rejected tool result
//! does, instead of the `"> "`-prefixed text — claude-code's
//! `UserTextMessage.tsx` special-cases both exact constants.

use iocraft::prelude::*;

use crate::components::messages::text_guard::is_empty_message_text;
use crate::components::messages::user_tool_result::{INTERRUPTED_LINE, INTERRUPT_MESSAGE, MARKER};
use crate::theme::TuiTheme;

/// Props for `UserTextMessage`.
#[derive(Default, Props)]
pub struct UserTextMessageProps {
    /// Plain-text body to render.
    pub body: String,
}

/// Render a user-submitted prompt with the locked `"> "` prefix.
///
/// When [`is_empty_message_text`] reports the body is empty (only stripped
/// scaffolding tags, or `(no content)`), the entire row is suppressed — an
/// empty `View` is returned instead of the `"> "` prefix line, matching
/// claude-code's `return null`. (RRS-08) [`INTERRUPT_MESSAGE`] renders the
/// `InterruptedByUser` line instead.
#[component]
pub fn UserTextMessage(props: &UserTextMessageProps) -> impl Into<AnyElement<'static>> {
    if props.body == INTERRUPT_MESSAGE {
        let content = format!("{MARKER}{INTERRUPTED_LINE}");
        return element! {
            View(flex_direction: FlexDirection::Row) {
                Text(content: content, color: TuiTheme::DIM)
            }
        };
    }
    if is_empty_message_text(&props.body) {
        // claude-code returns `null`; the iocraft equivalent is an empty View
        // (zero rows), so the message contributes no visible output.
        return element! { View() };
    }
    let content = format!("> {}", props.body);
    element! {
        View(flex_direction: FlexDirection::Row) {
            Text(content: content, color: TuiTheme::USER)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_content_body_is_empty() {
        assert!(is_empty_message_text("(no content)"));
    }

    #[test]
    fn only_context_tag_is_empty() {
        // A user message whose body is only `<context>x</context>` is empty, so
        // the component must suppress the whole row (no `"> "` prefix).
        assert!(is_empty_message_text("<context>x</context>"));
    }

    #[test]
    fn plain_prompt_is_not_empty() {
        assert!(!is_empty_message_text("what is 2 + 2?"));
    }

    #[test]
    fn interrupt_message_renders_interrupted_by_user_line_not_prefix() {
        // (RRS-08)
        let mut el = element! {
            UserTextMessage(body: INTERRUPT_MESSAGE.to_string())
        };
        let out = el.to_string();
        assert!(out.contains(INTERRUPTED_LINE), "got: {out}");
        assert!(
            !out.contains("> ["),
            "must not use the plain \"> \" prefix: {out}"
        );
    }
}

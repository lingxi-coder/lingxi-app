//! `UserTextMessage` — renders the user's submitted prompt.
//!
//! Locked literals (plan §T0):
//!   L5 prefix = "> "
//!   user color = default fg (no override)

use iocraft::prelude::*;

use crate::theme::TuiTheme;

/// Props for `UserTextMessage`.
#[derive(Default, Props)]
pub struct UserTextMessageProps {
    /// Plain-text body to render.
    pub body: String,
}

/// Render a user-submitted prompt with the locked `"> "` prefix.
#[component]
pub fn UserTextMessage(props: &UserTextMessageProps) -> impl Into<AnyElement<'static>> {
    let content = format!("> {}", props.body);
    element! {
        View(flex_direction: FlexDirection::Row) {
            Text(content: content, color: TuiTheme::USER)
        }
    }
}

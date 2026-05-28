//! `Scrollback` — the flex-grow middle zone.
//!
//! M6-02 ships a simple capped buffer (the cap lives on `AppState`, set to 500).
//!
//! ## Scroll math
//! - `offset = 0` shows the latest `viewport_height` messages.
//! - `offset = max_offset` shows the oldest.
//!
//! ## Behaviour
//! - `PgUp` = `+viewport_height`
//! - `PgDn` = `-viewport_height`
//! - `j`    = `+1`
//! - `k`    = `-1`
//! - `g`    = `max`
//! - `G`    = `0`

use iocraft::prelude::*;

use crate::components::messages::{
    assistant_text::AssistantTextMessage, user_text::UserTextMessage,
};
use crate::state::RenderedMessage;
use crate::theme::TuiTheme;

/// Props for `Scrollback`.
#[derive(Default, Props)]
pub struct ScrollbackProps {
    /// Scrollback buffer to render (clone of `AppState::messages`).
    pub messages: Vec<RenderedMessage>,
    /// Current scroll position (0 = latest at bottom).
    pub scroll_offset: usize,
    /// Number of message rows visible.
    pub viewport_height: usize,
}

/// Scrollback component.
#[component]
pub fn Scrollback(props: &ScrollbackProps) -> impl Into<AnyElement<'static>> {
    let visible: Vec<RenderedMessage> =
        visible_slice(&props.messages, props.scroll_offset, props.viewport_height).to_vec();
    element! {
        View(flex_direction: FlexDirection::Column, flex_grow: 1.0) {
            #(visible.into_iter().map(render_message))
        }
    }
}

/// Compute the inclusive slice of messages visible at the given offset.
///
/// Returns a borrow into `messages` for the viewport. Empty buffer or
/// zero viewport → empty slice.
#[must_use]
pub fn visible_slice(
    messages: &[RenderedMessage],
    scroll_offset: usize,
    viewport_height: usize,
) -> &[RenderedMessage] {
    if messages.is_empty() || viewport_height == 0 {
        return &[];
    }
    let total = messages.len();
    let end = total.saturating_sub(scroll_offset);
    let start = end.saturating_sub(viewport_height);
    &messages[start..end]
}

/// Clamp a requested scroll offset to `[0, max]` where
/// `max = total_messages.saturating_sub(viewport_height)`.
#[must_use]
#[allow(
    clippy::cast_possible_wrap,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
pub fn clamp_offset(requested: i64, total_messages: usize, viewport_height: usize) -> usize {
    let max = total_messages.saturating_sub(viewport_height) as i64;
    requested.clamp(0, max) as usize
}

fn render_message(m: RenderedMessage) -> AnyElement<'static> {
    match m {
        RenderedMessage::UserText { body, .. } => element! {
            UserTextMessage(body: body)
        }
        .into_any(),
        RenderedMessage::AssistantText { body, .. } => element! {
            AssistantTextMessage(body: body)
        }
        .into_any(),
        RenderedMessage::SystemText { body, is_error, .. } => {
            let color = if is_error {
                TuiTheme::ERROR
            } else {
                TuiTheme::DIM
            };
            element! {
                Text(content: body, color: color)
            }
            .into_any()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make(n: usize) -> Vec<RenderedMessage> {
        (0..n)
            .map(|i| RenderedMessage::UserText {
                body: format!("m{i}"),
                timestamp: 0,
            })
            .collect()
    }

    #[test]
    fn empty_buffer_returns_empty_slice() {
        let v = visible_slice(&[], 0, 10);
        assert!(v.is_empty());
    }

    #[test]
    fn offset_zero_shows_tail() {
        let msgs = make(10);
        let v = visible_slice(&msgs, 0, 3);
        assert_eq!(v.len(), 3);
        assert!(matches!(&v[0], RenderedMessage::UserText { body, .. } if body == "m7"));
        assert!(matches!(&v[2], RenderedMessage::UserText { body, .. } if body == "m9"));
    }

    #[test]
    fn offset_two_pages_back() {
        let msgs = make(10);
        let v = visible_slice(&msgs, 6, 3); // viewport 3, offset 6 → indices 1..4
        assert_eq!(v.len(), 3);
        assert!(matches!(&v[0], RenderedMessage::UserText { body, .. } if body == "m1"));
        assert!(matches!(&v[2], RenderedMessage::UserText { body, .. } if body == "m3"));
    }

    #[test]
    fn clamp_above_max_pins_to_max() {
        // 10 messages, viewport 3 → max_offset = 7.
        assert_eq!(clamp_offset(99, 10, 3), 7);
    }

    #[test]
    fn clamp_below_zero_pins_to_zero() {
        assert_eq!(clamp_offset(-5, 10, 3), 0);
    }
}

//! M7-03 snapshot: a window of 3 mixed-height messages at a fixed offset.

use std::collections::HashMap;

use iocraft::prelude::*;
use tui::components::virtual_message_list::{HeightCache, VirtualMessageList};
use tui::state::RenderedMessage;

#[test]
fn window_three_mixed_messages_fixed_offset() {
    let messages = vec![
        RenderedMessage::UserText {
            body: "short user line".into(),
            timestamp: 0,
        },
        RenderedMessage::AssistantText {
            body: "first\nsecond\nthird\nfourth".into(), // 4 lines
            timestamp: 0,
        },
        RenderedMessage::SystemText {
            body: "system note".into(),
            timestamp: 0,
            is_error: false,
        },
    ];
    // (M7-03 review) The component no longer rebuilds the cache; callers
    // thread the already-width-synced `HeightCache` in. Build it here at the
    // same width (40) the prior `viewport_width` prop used.
    let cache = HeightCache::build(&messages, 40);
    let mut element = element! {
        VirtualMessageList(
            messages: messages,
            cache: cache,
            scroll_offset: 0_usize,
            viewport_height: 8_usize,
            expanded: HashMap::new(),
            focused_tool_id: Option::<protocol::ToolUseId>::None,
        )
    };
    insta::assert_snapshot!("window_three_mixed_fixed_offset", element.to_string());
}

//! M7-03 snapshot: a window of 3 mixed-height messages at a fixed offset.

use std::collections::HashMap;

use iocraft::prelude::*;
use lingxi_tui::components::virtual_message_list::VirtualMessageList;
use lingxi_tui::state::RenderedMessage;

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
    let mut element = element! {
        VirtualMessageList(
            messages: messages,
            scroll_offset: 0_usize,
            viewport_height: 8_usize,
            viewport_width: 40_usize,
            expanded: HashMap::new(),
            focused_tool_id: Option::<lingxi_protocol::ToolUseId>::None,
        )
    };
    insta::assert_snapshot!("window_three_mixed_fixed_offset", element.to_string());
}

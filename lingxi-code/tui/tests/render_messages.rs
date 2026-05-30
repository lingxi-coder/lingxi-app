//! Snapshot tests for `UserTextMessage` and `AssistantTextMessage`.
//!
//! Locks byte-strings L4 (`"● "`) and L5 (`"> "`) from M6-02 T0.

use iocraft::prelude::*;
use tui::components::messages::assistant_text::AssistantTextMessage;
use tui::components::messages::user_text::UserTextMessage;

#[test]
fn user_text_single_line() {
    let mut element = element! {
        UserTextMessage(body: "hi".to_string())
    };
    insta::assert_snapshot!("user_text_single_line", element.to_string());
}

#[test]
fn assistant_text_three_lines() {
    let mut element = element! {
        AssistantTextMessage(body: "line one\nline two\nline three".to_string())
    };
    insta::assert_snapshot!("assistant_text_three_lines", element.to_string());
}

//! Snapshot tests for `ReplScreen` (M6-02 T10).
//!
//! Validates that the three vertical zones compose in the right order
//! and that messages flow into the scrollback.

use std::path::PathBuf;

use iocraft::prelude::*;
use lingxi_permission::PermissionMode;
use lingxi_tui::screens::repl::ReplScreen;
use lingxi_tui::state::{RenderedMessage, StatusSnapshot};

fn status() -> StatusSnapshot {
    StatusSnapshot {
        model: "claude-sonnet-4.5".into(),
        cwd: PathBuf::from("/a/b"),
        cost: "$0.000".to_string(),
        context_pct: 0.42_f32,
        permission_mode: PermissionMode::Default,
    }
}

#[test]
fn repl_screen_default_empty() {
    let mut element = element! {
        ReplScreen(
            status: status(),
            messages: Vec::<RenderedMessage>::new(),
            prompt_text: String::new(),
            prompt_cursor: 0_usize,
            scroll_offset: 0_usize,
            viewport_height: 5_usize,
        )
    };
    insta::assert_snapshot!("repl_screen_default_empty", element.to_string());
}

#[test]
fn repl_screen_with_one_user_one_assistant() {
    let messages = vec![
        RenderedMessage::UserText {
            body: "hi".into(),
            timestamp: 0,
        },
        RenderedMessage::AssistantText {
            body: "Hello!".into(),
            timestamp: 0,
        },
    ];
    let mut element = element! {
        ReplScreen(
            status: status(),
            messages: messages,
            prompt_text: "next".to_string(),
            prompt_cursor: 4_usize,
            scroll_offset: 0_usize,
            viewport_height: 5_usize,
        )
    };
    insta::assert_snapshot!("repl_screen_user_assistant", element.to_string());
}

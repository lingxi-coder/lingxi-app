//! Snapshot tests for `ReplScreen` (M6-02 T10).
//!
//! Validates that the three vertical zones compose in the right order
//! and that messages flow into the scrollback.

use std::path::PathBuf;

use iocraft::prelude::*;
use permission::PermissionMode;
use tui::components::virtual_message_list::HeightCache;
use tui::screens::repl::ReplScreen;
use tui::state::{RenderedMessage, StatusSnapshot};

fn status() -> StatusSnapshot {
    StatusSnapshot {
        model: "claude-sonnet-4.5".into(),
        cwd: PathBuf::from("/a/b"),
        cost: "$0.0000".to_string(),
        context_pct: 0.42_f32,
        permission_mode: PermissionMode::Default,
        ..StatusSnapshot::default()
    }
}

#[test]
fn repl_screen_default_empty() {
    let mut element = element! {
        ReplScreen(
            status: status(),
            messages: Vec::<RenderedMessage>::new(),
            cache: HeightCache::default(),
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
    // (M7-03 review) `ReplScreen` now takes a threaded `HeightCache`. This
    // test previously left `viewport_width` unset (defaulted to 0 → the
    // component clamped to width 1 before building); build the cache at the
    // same width 1 so the snapshot output stays byte-identical.
    let cache = HeightCache::build(&messages, 1);
    let mut element = element! {
        ReplScreen(
            status: status(),
            messages: messages,
            cache: cache,
            prompt_text: "next".to_string(),
            prompt_cursor: 4_usize,
            scroll_offset: 0_usize,
            viewport_height: 5_usize,
        )
    };
    insta::assert_snapshot!("repl_screen_user_assistant", element.to_string());
}

/// (`/color`) When a session color is set, the standalone-agent banner rule
/// (`SessionColorBanner`) renders directly above the prompt — a full-width
/// `─` (U+2500) line. Mirrors claude-code `useSwarmBanner` standalone branch +
/// `PromptInput.tsx:2259`. With `None` it is hidden (no `─` row).
#[test]
fn repl_screen_session_color_banner_present_when_set() {
    let mut element = element! {
        ReplScreen(
            status: status(),
            messages: Vec::<RenderedMessage>::new(),
            cache: HeightCache::default(),
            prompt_text: String::new(),
            prompt_cursor: 0_usize,
            prompt_width: 12_usize,
            scroll_offset: 0_usize,
            viewport_height: 5_usize,
            session_agent_color: Some("cyan".to_string()),
        )
    };
    let out = element.to_string();
    assert!(
        out.contains('\u{2500}'),
        "expected a `─` rule row above the prompt when a session color is set; got:\n{out}"
    );
}

#[test]
fn repl_screen_no_session_color_banner_when_none() {
    let mut element = element! {
        ReplScreen(
            status: status(),
            messages: Vec::<RenderedMessage>::new(),
            cache: HeightCache::default(),
            prompt_text: String::new(),
            prompt_cursor: 0_usize,
            prompt_width: 12_usize,
            scroll_offset: 0_usize,
            viewport_height: 5_usize,
            session_agent_color: None,
        )
    };
    let out = element.to_string();
    assert!(
        !out.contains('\u{2500}'),
        "expected NO `─` rule row when no session color is set; got:\n{out}"
    );
}

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
    // The input view is ALWAYS bracketed by 2 `─` border rows; a session color
    // adds a THIRD `─` rule (the banner) above the prompt → 3 rule rows total.
    let rule_rows = out.lines().filter(|l| l.contains('\u{2500}')).count();
    assert_eq!(
        rule_rows, 3,
        "expected a session-color banner rule above the 2 input borders (3 total); got {rule_rows}:\n{out}"
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
    // No session color → only the 2 `─` input-border rows (no banner rule).
    let rule_rows = out.lines().filter(|l| l.contains('\u{2500}')).count();
    assert_eq!(
        rule_rows, 2,
        "expected only the 2 input-border rules (no banner) when no session color is set; got {rule_rows}:\n{out}"
    );
}

#[test]
fn repl_screen_context_pressure_banner_present_when_set() {
    // The orchestrator-computed TokenWarning banner renders directly above the
    // prompt (claude-code's `<TokenWarning>` in `PromptInput/Notifications.tsx`).
    let mut element = element! {
        ReplScreen(
            status: status(),
            messages: Vec::<RenderedMessage>::new(),
            cache: HeightCache::default(),
            prompt_text: String::new(),
            prompt_cursor: 0_usize,
            prompt_width: 80_usize,
            scroll_offset: 0_usize,
            viewport_height: 5_usize,
            context_pressure: Some(traits::ContextPressureBanner {
                text: "Context low (8% remaining) \u{00b7} Run /compact to compact & continue"
                    .to_string(),
                level: traits::ContextPressureLevel::Error,
            }),
        )
    };
    let out = element.to_string();
    assert!(
        out.contains("Context low (8% remaining)"),
        "expected the TokenWarning banner above the prompt; got:\n{out}"
    );
}

#[test]
fn repl_screen_no_context_pressure_banner_when_none() {
    let mut element = element! {
        ReplScreen(
            status: status(),
            messages: Vec::<RenderedMessage>::new(),
            cache: HeightCache::default(),
            prompt_text: String::new(),
            prompt_cursor: 0_usize,
            prompt_width: 80_usize,
            scroll_offset: 0_usize,
            viewport_height: 5_usize,
            context_pressure: None,
        )
    };
    let out = element.to_string();
    assert!(
        !out.contains("Context low"),
        "expected NO TokenWarning banner when context_pressure is None; got:\n{out}"
    );
}

#[test]
fn repl_screen_shows_scroll_indicator_only_when_scrolled_up() {
    let mut scrolled = element! {
        ReplScreen(
            status: status(),
            messages: Vec::<RenderedMessage>::new(),
            cache: HeightCache::default(),
            prompt_text: String::new(),
            prompt_cursor: 0_usize,
            prompt_width: 80_usize,
            scroll_offset: 42_usize,
            viewport_height: 5_usize,
        )
    };
    let out = scrolled.to_string();
    assert!(
        out.contains("Scrolled 42 lines"),
        "expected scroll indicator; got:\n{out}"
    );

    let mut bottom = element! {
        ReplScreen(
            status: status(),
            messages: Vec::<RenderedMessage>::new(),
            cache: HeightCache::default(),
            prompt_text: String::new(),
            prompt_cursor: 0_usize,
            prompt_width: 80_usize,
            scroll_offset: 0_usize,
            viewport_height: 5_usize,
        )
    };
    let out = bottom.to_string();
    assert!(
        !out.contains("Scrolled"),
        "indicator should hide at bottom; got:\n{out}"
    );
}

#[test]
fn repl_screen_renders_prompt_cursor_when_focused() {
    let mut element = element! {
        ReplScreen(
            status: status(),
            messages: Vec::<RenderedMessage>::new(),
            cache: HeightCache::default(),
            prompt_text: "abc".to_string(),
            prompt_cursor: 3_usize,
            prompt_width: 80_usize,
            scroll_offset: 0_usize,
            viewport_height: 5_usize,
        )
    };
    let out = element.to_string();
    assert!(
        out.contains("❯ abc "),
        "expected visible cursor cell after prompt text; got:\n{out}"
    );
}

#[test]
fn repl_screen_footer_shows_active_model() {
    let mut element = element! {
        ReplScreen(
            status: status(),
            messages: Vec::<RenderedMessage>::new(),
            cache: HeightCache::default(),
            prompt_text: String::new(),
            prompt_cursor: 0_usize,
            prompt_width: 80_usize,
            scroll_offset: 0_usize,
            viewport_height: 5_usize,
        )
    };
    let out = element.to_string();
    assert!(
        out.contains("claude-sonnet-4.5"),
        "expected bottom footer to show active model; got:\n{out}"
    );
}

#[test]
fn repl_screen_pins_prompt_to_bottom_of_fixed_parent() {
    let mut element = element! {
        View(width: 80, height: 12) {
            ReplScreen(
                status: status(),
                messages: Vec::<RenderedMessage>::new(),
                cache: HeightCache::default(),
                prompt_text: String::new(),
                prompt_cursor: 0_usize,
                prompt_width: 80_usize,
                scroll_offset: 0_usize,
                viewport_height: 6_usize,
            )
        }
    };
    let out = element.to_string();
    let lines: Vec<&str> = out.lines().collect();
    let prompt_row = lines
        .iter()
        .position(|line| line.contains('\u{276f}'))
        .expect("prompt row should render");
    assert!(
        prompt_row >= 8,
        "expected prompt to be pushed into the bottom zone of a 12-row frame; row={prompt_row}, frame:\n{out}"
    );
    assert!(
        lines
            .last()
            .is_some_and(|line| line.contains("claude-sonnet-4.5")),
        "expected the active model on the bottom footer row; got:\n{out}"
    );
}

#[test]
fn repl_screen_keeps_footer_fixed_with_tall_scrollback() {
    let messages: Vec<RenderedMessage> = (0..30)
        .map(|i| RenderedMessage::AssistantText {
            body: format!("line {i}"),
            timestamp: 0,
        })
        .collect();
    let cache = HeightCache::build(&messages, 80);
    let mut element = element! {
        View(width: 80, height: 12) {
            ReplScreen(
                status: status(),
                messages: messages,
                cache: cache,
                prompt_text: String::new(),
                prompt_cursor: 0_usize,
                prompt_width: 80_usize,
                scroll_offset: 0_usize,
                viewport_height: 6_usize,
            )
        }
    };
    let out = element.to_string();
    let lines: Vec<&str> = out.lines().collect();
    assert!(
        lines
            .last()
            .is_some_and(|line| line.contains("claude-sonnet-4.5")),
        "expected footer/model to remain on the bottom row with tall content; got:\n{out}"
    );
    assert!(
        !lines
            .iter()
            .rev()
            .take_while(|line| !line.contains("claude-sonnet-4.5"))
            .any(|line| line.contains("line ")),
        "scrollback content must not render below the footer; got:\n{out}"
    );
}

//! M7-14 behavior: search overlay routing, jump-back, export safety.

use lingxi_tui::components::message_selector::{
    export_transcript, handle_message_selector_key, message_line_offset, ExportError,
    SelectorAction,
};
use lingxi_tui::components::virtual_message_list::HeightCache;
use lingxi_tui::state::{AppState, RenderedMessage};

mod support;
use support::fake_status;

fn push(st: &mut AppState, body: &str) {
    st.push_message(RenderedMessage::UserText {
        body: body.into(),
        timestamp: 0,
    });
}

#[test]
fn selecting_a_match_sets_scroll_offset_to_that_message() {
    let mut st = AppState::new(fake_status());
    for i in 0..40 {
        push(&mut st, &format!("line {i}"));
    }
    st.refresh_height_cache(80); // M7-03: populate the line cache
    let vh = 10;

    st.message_selector.open();
    // Type "line 5" → matches "line 5".
    for c in "line 5".chars() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        handle_message_selector_key(
            &mut st.message_selector,
            &st.messages,
            KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
        );
    }
    // The first filtered hit is message index 5.
    let target = st.message_selector.filtered[st.message_selector.selected_filtered];
    assert_eq!(target, 5);

    // Jump: caller sets scroll_offset from message_line_offset.
    let cache = HeightCache::build(&st.messages, 80);
    let offset = message_line_offset(&st.messages, &cache, target, vh);
    st.scroll_offset = offset;
    // 40 one-line msgs: line_at_start[5]=5, total 40 → offset = 40-5-10 = 25.
    assert_eq!(st.scroll_offset, 25);
}

#[test]
fn esc_closes_the_overlay_without_a_jump() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut st = AppState::new(fake_status());
    push(&mut st, "hello");
    st.message_selector.open();
    st.message_selector.refilter_all(&st.messages);
    let action = handle_message_selector_key(
        &mut st.message_selector,
        &st.messages.clone(),
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
    );
    assert_eq!(action, SelectorAction::Close);
    assert!(!st.message_selector.open);
}

#[test]
fn export_default_path_and_overwrite_confirm() {
    use std::fs;
    use tempfile::TempDir;
    let tmp = TempDir::new().unwrap();
    let msgs = vec![
        RenderedMessage::UserText {
            body: "q".into(),
            timestamp: 0,
        },
        RenderedMessage::AssistantText {
            body: "a".into(),
            timestamp: 0,
        },
    ];
    // First export writes the file.
    let p = export_transcript(&msgs, tmp.path(), "out.txt", false).unwrap();
    assert!(p.exists());
    // Second export to the same name without overwrite → refused, file kept.
    let original = fs::read_to_string(&p).unwrap();
    match export_transcript(&msgs, tmp.path(), "out.txt", false) {
        Err(ExportError::Exists(_)) => {}
        other => panic!("expected Exists, got {other:?}"),
    }
    assert_eq!(fs::read_to_string(&p).unwrap(), original);
}

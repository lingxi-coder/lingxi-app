//! M7-03 behavior tests: line-based scroll, windowing, telemetry, cache.

use lingxi_tui::app::scroll_with_viewport;
use lingxi_tui::events::keymap::ScrollDir;
use lingxi_tui::state::{AppState, RenderedMessage};

mod support;
use support::fake_status;

fn push_line(st: &mut AppState, body: &str) {
    st.push_message(RenderedMessage::UserText {
        body: body.to_string(),
        timestamp: 0,
    });
}

#[test]
fn line_step_uses_total_lines_not_message_count() {
    let mut st = AppState::new(fake_status());
    // One 100-line message → total_lines = 100, message count = 1.
    push_line(&mut st, &vec!["x"; 100].join("\n"));
    let vh = 10;
    st.refresh_height_cache(80);
    // LineUp scrolls one line; max_offset = 100 - 10 = 90.
    scroll_with_viewport(&mut st, ScrollDir::LineUp, vh);
    assert_eq!(st.scroll_offset, 1);
    // Top pins to max_offset = 90 (lines), NOT 0 (messages.len - vh).
    scroll_with_viewport(&mut st, ScrollDir::Top, vh);
    assert_eq!(st.scroll_offset, 90);
    // Bottom returns to 0.
    scroll_with_viewport(&mut st, ScrollDir::Bottom, vh);
    assert_eq!(st.scroll_offset, 0);
}

#[test]
fn pgup_pgdn_step_by_viewport_lines() {
    let mut st = AppState::new(fake_status());
    push_line(&mut st, &vec!["x"; 100].join("\n"));
    st.refresh_height_cache(80);
    let vh = 8;
    scroll_with_viewport(&mut st, ScrollDir::PageUp, vh);
    scroll_with_viewport(&mut st, ScrollDir::PageUp, vh);
    assert_eq!(st.scroll_offset, 16);
    scroll_with_viewport(&mut st, ScrollDir::PageDown, vh);
    assert_eq!(st.scroll_offset, 8);
}

#[test]
fn jk_gg_keys_move_offset_line_based() {
    let mut st = AppState::new(fake_status());
    for i in 0..40 {
        push_line(&mut st, &format!("l{i}")); // 40 one-line msgs = 40 lines
    }
    st.refresh_height_cache(80);
    let vh = 10;
    // Per keymap.rs: k == LineUp (toward older, +offset); j == LineDown
    // (toward newer, -offset). LineUp first → offset 1.
    scroll_with_viewport(&mut st, ScrollDir::LineUp, vh);
    assert_eq!(st.scroll_offset, 1);
    scroll_with_viewport(&mut st, ScrollDir::LineDown, vh);
    assert_eq!(st.scroll_offset, 0);
    // g == Top → max_offset = 40 - 10 = 30. G == Bottom → 0.
    scroll_with_viewport(&mut st, ScrollDir::Top, vh);
    assert_eq!(st.scroll_offset, 30);
    scroll_with_viewport(&mut st, ScrollDir::Bottom, vh);
    assert_eq!(st.scroll_offset, 0);
}

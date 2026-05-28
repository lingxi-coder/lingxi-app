//! Behavior tests (M6-02 T12) for PgUp / PgDn / g / G against the
//! `scroll_with_viewport` helper.

use lingxi_tui::app::scroll_with_viewport;
use lingxi_tui::events::keymap::ScrollDir;
use lingxi_tui::state::{AppState, RenderedMessage};

mod support;
use support::fake_status;

fn buf_30() -> AppState {
    let mut st = AppState::new(fake_status());
    for i in 0..30 {
        st.push_message(RenderedMessage::AssistantText {
            body: format!("a{i}"),
            timestamp: 0,
        });
    }
    st
}

#[test]
fn pgup_twice_offsets_by_two_viewports() {
    let mut st = buf_30();
    let vh = 8;
    scroll_with_viewport(&mut st, ScrollDir::PageUp, vh);
    scroll_with_viewport(&mut st, ScrollDir::PageUp, vh);
    assert_eq!(st.scroll_offset, vh * 2);
}

#[test]
fn pgup_at_top_pins_to_max() {
    let mut st = buf_30();
    let vh = 8;
    // 30 messages, vh=8 → max_offset = 22.
    for _ in 0..10 {
        scroll_with_viewport(&mut st, ScrollDir::PageUp, vh);
    }
    assert_eq!(st.scroll_offset, 22);
}

#[test]
fn ctrl_g_jumps_top_then_shift_g_returns_bottom() {
    let mut st = buf_30();
    let vh = 5;
    scroll_with_viewport(&mut st, ScrollDir::Top, vh);
    assert_eq!(st.scroll_offset, 25); // max
    scroll_with_viewport(&mut st, ScrollDir::Bottom, vh);
    assert_eq!(st.scroll_offset, 0);
}

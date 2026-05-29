//! M7-03 behavior tests: line-based scroll, windowing, telemetry, cache.

use lingxi_tui::app::scroll_with_viewport;
use lingxi_tui::components::virtual_message_list::{render_window, HeightCache};
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

#[test]
fn scroll_telemetry_transitions_fire_on_0_to_nonzero_and_back() {
    let mut st = AppState::new(fake_status());
    for i in 0..40 {
        push_line(&mut st, &format!("t{i}"));
    }
    st.refresh_height_cache(80);
    let vh = 10;
    assert_eq!(st.scroll_offset, 0); // start at bottom

    // 0 → non-zero: scroll_started fires (offset becomes 10).
    scroll_with_viewport(&mut st, ScrollDir::PageUp, vh);
    assert_ne!(st.scroll_offset, 0);

    // non-zero → non-zero: no lifecycle emit (still scrolling).
    scroll_with_viewport(&mut st, ScrollDir::LineUp, vh);
    assert_ne!(st.scroll_offset, 0);

    // non-zero → 0: scroll_ended fires (back at bottom).
    scroll_with_viewport(&mut st, ScrollDir::Bottom, vh);
    assert_eq!(st.scroll_offset, 0);
}

/// Build 5000 mixed-height messages: every 10th is 50 lines tall, the
/// rest are 1 line. Returns (messages, total_lines).
fn mixed_5k() -> (Vec<RenderedMessage>, usize) {
    let msgs: Vec<RenderedMessage> = (0..5000)
        .map(|i| {
            let body = if i % 10 == 0 {
                vec!["x"; 50].join("\n") // 50 lines
            } else {
                format!("m{i}") // 1 line
            };
            RenderedMessage::UserText { body, timestamp: 0 }
        })
        .collect();
    // 500 tall (×50) + 4500 short (×1) = 25_000 + 4_500 = 29_500 lines.
    (msgs, 29_500)
}

#[test]
fn gate_5k_render_count_bounded_by_viewport() {
    let (msgs, total) = mixed_5k();
    let cache = HeightCache::build(&msgs, 80);
    assert_eq!(cache.total_lines(), total);
    let vh = 30;
    // At the bottom (offset 0) the window must be a tiny slice, never 5000.
    let win = render_window(&msgs, &cache, 0, vh);
    let count = win.indices().count();
    assert!(count < 100, "render count {count} not bounded by viewport");
    // The last visible message is the final one (offset 0 = tail).
    assert_eq!(win.last_index, 4999);
}

#[test]
fn gate_5k_exact_first_last_visible_at_offset() {
    let (msgs, total) = mixed_5k();
    let cache = HeightCache::build(&msgs, 80);
    assert_eq!(total, 29_500);
    let vh = 20;
    // Scroll so the viewport sits entirely inside the tall message at
    // index 2490 (i % 10 == 0). Compute its absolute start line:
    //   short msgs before any tall one... derive the start of msg 2490.
    let mut start = 0usize;
    for i in 0..2490 {
        start += cache.height_at(i);
    }
    // msg 2490 spans [start, start+50). Put the viewport at [start+10, start+30).
    // bottom_line = start + 30  ⇒  offset = total - bottom_line.
    let bottom_line = start + 30;
    let offset = total - bottom_line;
    let win = render_window(&msgs, &cache, offset, vh);
    // Entirely inside one tall message.
    assert_eq!(win.first_index, 2490);
    assert_eq!(win.last_index, 2490);
    assert_eq!(win.skip_top_lines, 10); // top_line - span_start = (start+10) - start
}

#[test]
fn gate_5k_offset_zero_first_visible_is_correct() {
    let (msgs, _total) = mixed_5k();
    let cache = HeightCache::build(&msgs, 80);
    let vh = 30;
    // offset 0 → top_line = total - 30. Tail messages 4970..4999 are all
    // 1-line except 4990 (i%10==0, 50 lines). Last 30 lines walk back:
    //   4999..4991 = 9 one-line msgs (9 lines), 4990 = 50 lines.
    // 9 + (need 21 more lines from msg 4990's 50) → first_index = 4990.
    let win = render_window(&msgs, &cache, 0, vh);
    assert_eq!(win.last_index, 4999);
    assert_eq!(win.first_index, 4990);
    // 50-line msg 4990 spans [total-9-50, total-9). top_line = total-30.
    // skip_top_lines = top_line - span_start = (total-30) - (total-59) = 29.
    assert_eq!(win.skip_top_lines, 29);
}

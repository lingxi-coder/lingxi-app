//! Snapshot tests for `SpinnerWithVerb` rendered output. (M6-03 Task 6)
//!
//! Locks the rendered glyph + verb for frame indices 0, 5, 9 — covering
//! the start, midpoint, and second-cycle position of the 12-frame loop.

use insta::assert_snapshot;
use tui::components::spinner::format_spinner_line;

#[test]
fn spinner_frame_0_crunching() {
    assert_snapshot!("spinner_frame_0", format_spinner_line(0, 0));
}

#[test]
fn spinner_frame_5_thinking() {
    assert_snapshot!("spinner_frame_5", format_spinner_line(5, 1));
}

#[test]
fn spinner_frame_9_generating() {
    assert_snapshot!("spinner_frame_9", format_spinner_line(9, 2));
}

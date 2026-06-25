//! Snapshot tests for `SpinnerWithVerb` rendered output. (M6-03 Task 6)
//!
//! Locks the rendered glyph + verb for frame indices 0, 5, 9 — covering
//! the start, midpoint, and second-cycle position of the 12-frame loop.

use insta::assert_snapshot;
use iocraft::prelude::*;
use tui::components::spinner::{format_spinner_line, frame_at_index, SpinnerWithVerb};

#[test]
fn spinner_frame_0_crunching() {
    assert_snapshot!("spinner_frame_0", format_spinner_line(0, 0));
}

#[test]
fn reduced_motion_pins_glyph_to_frame_zero() {
    // (SS-06) With reduced_motion the glyph is frame 0 (no animation). A
    // verb_override keeps the (otherwise random) verb deterministic.
    let mut el = element! {
        SpinnerWithVerb(reduced_motion: true, verb_override: Some(0usize))
    };
    let out = el.to_string();
    assert!(
        out.contains(frame_at_index(0)),
        "reduced-motion spinner must render the frame-0 glyph, got: {out:?}"
    );
}

#[test]
fn spinner_frame_5_thinking() {
    assert_snapshot!("spinner_frame_5", format_spinner_line(5, 1));
}

#[test]
fn spinner_frame_9_generating() {
    assert_snapshot!("spinner_frame_9", format_spinner_line(9, 2));
}

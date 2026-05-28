//! M6-06 — StatusLine renders the cost string passed via props verbatim
//! (4-decimal claude-code parity).
//!
//! The component takes `cost: String` (pre-formatted by the producer).
//! M6-06 changed the placeholder default from `"$0.000"` to `"$0.0000"`
//! and locks the rendering invariant: whatever 4-decimal string the
//! producer passes flows through to the rendered line.

use std::path::PathBuf;

use iocraft::prelude::*;
use lingxi_permission::PermissionMode;
use lingxi_tui::components::status_line::StatusLine;

#[test]
fn renders_four_decimal_cost() {
    // Render a StatusLine with cost = "$0.0234" (4-decimal claude-code
    // parity) and assert the rendered line contains the literal.
    let mut element = element! {
        StatusLine(
            model: "claude-opus-4-7".to_string(),
            cwd: PathBuf::from("/tmp/proj"),
            cost: "$0.0234".to_string(),
            context_pct: 0.42_f32,
            permission_mode: PermissionMode::Default,
        )
    };
    let rendered = element.to_string();
    assert!(
        rendered.contains("$0.0234"),
        "expected '$0.0234' in: {rendered}"
    );
    // Negative assertion: must NOT contain truncated 3-decimal form
    // followed by a space (the M6-02 placeholder shape).
    assert!(
        !rendered.contains("$0.023 "),
        "found 3-decimal leftover in: {rendered}"
    );
}

#[test]
fn zero_cost_renders_four_zeros() {
    // The producer's zero-cost render must now be `$0.0000` (4 decimals),
    // matching the M6-06 default. We pass it explicitly here to lock the
    // contract.
    let mut element = element! {
        StatusLine(
            model: "claude-opus-4-7".to_string(),
            cwd: PathBuf::from("/tmp"),
            cost: "$0.0000".to_string(),
            context_pct: 0.0_f32,
            permission_mode: PermissionMode::Default,
        )
    };
    let rendered = element.to_string();
    assert!(rendered.contains("$0.0000"), "got: {rendered}");
    // 3-decimal placeholder must be gone.
    assert!(
        !rendered.contains("$0.000 "),
        "M6-02 3-decimal placeholder still present: {rendered}"
    );
}

//! Snapshot test for `StatusLine` at a fixed state.
//!
//! Verifies field order and separators (byte-locks L1/L2/L3 from M6-02 T0).

use std::path::PathBuf;

use iocraft::prelude::*;
use lingxi_permission::PermissionMode;
use lingxi_tui::components::status_line::StatusLine;

#[test]
fn status_line_default_state() {
    let mut element = element! {
        StatusLine(
            model: "claude-sonnet-4.5".to_string(),
            cwd: PathBuf::from("/a/b"),
            cost: "$0.0000".to_string(),
            context_pct: 0.42_f32,
            permission_mode: PermissionMode::Default,
        )
    };
    let rendered = element.to_string();
    insta::assert_snapshot!("status_line_default", rendered);
}

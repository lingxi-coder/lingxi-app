//! Snapshot tests for the 3 M6-05 permission dialogs.
//!
//! Each snapshot locks the rendered ANSI frame so that future layout
//! changes are surfaced for explicit review (insta diff). Plain-text
//! assertions on substrings are also asserted alongside the snapshot
//! so a corrupted snapshot file is caught even when `cargo insta
//! review` hasn't been run.

use iocraft::prelude::*;
use lingxi_tui::components::permissions::bypass_permissions::BypassPermissionsMode;
use lingxi_tui::components::permissions::exit_plan_mode::ExitPlanMode;
use lingxi_tui::components::permissions::tool_use_confirm::ToolUseConfirm;
use lingxi_tui::components::permissions::DialogFocus;

#[test]
fn snapshot_tool_use_confirm_default_state() {
    let mut element = element! {
        ToolUseConfirm(
            tool_name: "Bash".to_string(),
            tool_input_pretty: "{\"command\":\"ls -la\"}".to_string(),
            focus: DialogFocus::AllowOnce,
        )
    };
    let frame = element.to_string();
    insta::assert_snapshot!("tool_use_confirm_default_state", &frame);
    // Substring locks (insulate against snapshot-file corruption).
    assert!(
        frame.contains("Claude needs your permission to use Bash"),
        "got: {frame}"
    );
    assert!(frame.contains("> [1] Allow Once"));
    assert!(frame.contains("  [2] Allow Always"));
    assert!(frame.contains("  [N] Deny"));
}

#[test]
fn snapshot_exit_plan_mode_with_5_line_plan() {
    let plan =
        "1. Read foo.rs\n2. Refactor bar()\n3. Add tests\n4. Run cargo test\n5. Commit".to_string();
    let mut element = element! {
        ExitPlanMode(
            plan: plan,
            focus: DialogFocus::AllowOnce,
        )
    };
    let frame = element.to_string();
    insta::assert_snapshot!("exit_plan_mode_with_5_line_plan", &frame);
    assert!(frame.contains("Claude Code needs your approval for the plan"));
    assert!(frame.contains("1. Read foo.rs"));
    assert!(frame.contains("5. Commit"));
    assert!(frame.contains("> [1] Allow Once"));
}

#[test]
fn snapshot_bypass_permissions_empty_typed() {
    let mut element = element! {
        BypassPermissionsMode(typed: String::new())
    };
    let frame = element.to_string();
    insta::assert_snapshot!("bypass_permissions_empty_typed", &frame);
    assert!(frame.contains("WARNING: Claude Code running in Bypass Permissions mode"));
    assert!(frame.contains("In Bypass Permissions mode"));
    assert!(frame.contains("By proceeding"));
}

#[test]
fn snapshot_bypass_permissions_partial_typed() {
    let mut element = element! {
        BypassPermissionsMode(typed: "ye".to_string())
    };
    let frame = element.to_string();
    insta::assert_snapshot!("bypass_permissions_partial_typed", &frame);
    assert!(frame.contains("WARNING: Claude Code running in Bypass Permissions mode"));
    assert!(frame.contains("Esc to cancel: ye"));
}

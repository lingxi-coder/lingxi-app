//! Snapshot tests for the 3 M6-05 permission dialogs.
//!
//! Each snapshot locks the rendered ANSI frame so that future layout
//! changes are surfaced for explicit review (insta diff). Plain-text
//! assertions on substrings are also asserted alongside the snapshot
//! so a corrupted snapshot file is caught even when `cargo insta
//! review` hasn't been run.

use iocraft::prelude::*;
use tui::components::permissions::bypass_permissions::BypassPermissionsMode;
use tui::components::permissions::exit_plan_mode::ExitPlanMode;
use tui::components::permissions::tool_use_confirm::ToolUseConfirm;
use tui::components::permissions::DialogFocus;

#[test]
fn snapshot_tool_use_confirm_default_state() {
    let mut element = element! {
        ToolUseConfirm(
            tool_name: "Bash".to_string(),
            tool_input: serde_json::json!({"command": "ls -la"}),
            cwd: std::path::PathBuf::from("/work"),
            focus: DialogFocus::AllowOnce,
        )
    };
    let frame = element.to_string();
    insta::assert_snapshot!("tool_use_confirm_default_state", &frame);
    // (perm-01/perm-03) Substring locks.
    assert!(frame.contains("Tool use"), "got: {frame}");
    assert!(frame.contains("Bash(ls -la)"), "got: {frame}");
    assert!(frame.contains("Do you want to proceed?"), "got: {frame}");
    assert!(frame.contains("> Yes"), "got: {frame}");
    assert!(
        frame.contains("Yes, and don't ask again for Bash commands in /work"),
        "got: {frame}"
    );
    assert!(frame.contains("  No"), "got: {frame}");
}

#[test]
fn tool_use_confirm_worker_name_renders_inline_on_title_row() {
    // (perm-09) Dim `· @name` suffix on the SAME row as the bold "Tool use"
    // title, not a separate badge line above it.
    let mut element = element! {
        ToolUseConfirm(
            tool_name: "Bash".to_string(),
            tool_input: serde_json::json!({"command": "ls -la"}),
            cwd: std::path::PathBuf::from("/work"),
            focus: DialogFocus::AllowOnce,
            worker_name: Some("alice".to_string()),
        )
    };
    let frame = element.to_string();
    let title_line = frame
        .lines()
        .find(|l| l.contains("Tool use"))
        .expect("a title line");
    assert!(
        title_line.contains("\u{00B7} @alice"),
        "got: {title_line:?}"
    );
}

#[test]
fn snapshot_tool_use_confirm_edit_shows_diff() {
    // (perm-02) An Edit permission request renders the structured diff inside
    // the dialog so the user sees the change before approving.
    let mut element = element! {
        ToolUseConfirm(
            tool_name: "Edit".to_string(),
            tool_input: serde_json::json!({
                "file_path": "/work/src/main.rs",
                "old_string": "let x = 1;",
                "new_string": "let x = 2;",
            }),
            cwd: std::path::PathBuf::from("/work"),
            focus: DialogFocus::AllowOnce,
        )
    };
    let frame = element.to_string();
    insta::assert_snapshot!("tool_use_confirm_edit_shows_diff", &frame);
    assert!(frame.contains("Tool use"), "got: {frame}");
    assert!(frame.contains("Edit(src/main.rs)"), "got: {frame}");
    // The diff body shows both the removed and added line content.
    assert!(frame.contains("let x = 1;"), "old line in diff: {frame}");
    assert!(frame.contains("let x = 2;"), "new line in diff: {frame}");
    assert!(frame.contains("Do you want to proceed?"), "got: {frame}");
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
    // (perm-06) "Ready to code?" + plan-approval options.
    assert!(frame.contains("Ready to code?"), "got: {frame}");
    assert!(frame.contains("1. Read foo.rs"));
    assert!(frame.contains("5. Commit"));
    assert!(
        frame.contains("> Yes, manually approve edits"),
        "got: {frame}"
    );
    assert!(frame.contains("Yes, auto-accept edits"), "got: {frame}");
    assert!(frame.contains("No, keep planning"), "got: {frame}");
}

#[test]
fn snapshot_bypass_permissions_empty_typed() {
    let mut element = element! {
        BypassPermissionsMode(typed: String::new())
    };
    let frame = element.to_string();
    insta::assert_snapshot!("bypass_permissions_empty_typed", &frame);
    assert!(frame.contains("WARNING: LingXi running in Bypass Permissions mode"));
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
    assert!(frame.contains("WARNING: LingXi running in Bypass Permissions mode"));
    assert!(frame.contains("Esc to cancel: ye"));
}

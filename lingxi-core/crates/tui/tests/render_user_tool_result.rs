//! Snapshot tests for `UserToolResultMessage` (M6-04 Tasks 5 + 6).

use insta::assert_snapshot;
use lingxi_protocol::ToolUseId;
use lingxi_tui::components::messages::user_tool_result::{
    render_user_tool_result_to_string, UserToolResultProps,
};

fn id() -> ToolUseId {
    ToolUseId::nil()
}

#[test]
fn short_5_line_result_renders_full() {
    let body = "line1\nline2\nline3\nline4\nline5";
    let s = render_user_tool_result_to_string(UserToolResultProps {
        id: id(),
        tool: "Read".into(),
        result: serde_json::json!({"content": body}),
        expanded: true,
        focused: false,
        ..Default::default()
    });
    assert_snapshot!(s, @r"
    └ line1
      line2
      line3
      line4
      line5
    ");
}

#[test]
fn long_200_line_result_shows_truncation_footer() {
    let body = (1..=200)
        .map(|i| format!("line{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let s = render_user_tool_result_to_string(UserToolResultProps {
        id: id(),
        tool: "Bash".into(),
        result: serde_json::json!({"content": body}),
        expanded: true,
        focused: false,
        ..Default::default()
    });
    // First 100 lines render; lines 101..=200 are dropped.
    assert!(s.starts_with("└ line1\n"), "got: {s}");
    assert!(s.contains("\n  line100\n"), "got tail: {s}");
    assert!(!s.contains("line101"), "should not include line101");
    assert!(
        s.ends_with("[output truncated, 100 more lines]"),
        "got: {s}"
    );
}

#[test]
fn collapsed_long_result_shows_first_line_plus_lines_suffix() {
    let body = (1..=10)
        .map(|i| format!("line{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let s = render_user_tool_result_to_string(UserToolResultProps {
        id: id(),
        tool: "Bash".into(),
        result: serde_json::json!({"content": body}),
        expanded: false,
        focused: false,
        ..Default::default()
    });
    assert_eq!(s, "└ line1 (+9 lines)");
}

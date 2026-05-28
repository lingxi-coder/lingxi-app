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
    });
    assert_snapshot!(s, @r"
    └ line1
      line2
      line3
      line4
      line5
    ");
}

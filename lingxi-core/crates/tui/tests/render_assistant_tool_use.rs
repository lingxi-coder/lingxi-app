//! Snapshot tests for `AssistantToolUseMessage` (M6-04 Tasks 3 + 4).

use insta::assert_snapshot;
use lingxi_protocol::ToolUseId;
use lingxi_tui::components::messages::assistant_tool_use::{
    render_assistant_tool_use_to_string, AssistantToolUseProps,
};

fn id() -> ToolUseId {
    ToolUseId::nil()
}

#[test]
fn collapsed_read_with_file_path() {
    let s = render_assistant_tool_use_to_string(AssistantToolUseProps {
        id: id(),
        tool: "Read".into(),
        input: serde_json::json!({"file_path": "/tmp/x.rs"}),
        expanded: false,
        focused: false,
    });
    assert_snapshot!(s, @r#"● Read({"file_path": "/tmp/x.rs"})"#);
}

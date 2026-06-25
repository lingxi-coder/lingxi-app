//! Snapshot tests for `AssistantToolUseMessage` (M6-04 Tasks 3 + 4).

use std::path::PathBuf;

use protocol::ToolUseId;
use tui::components::messages::assistant_tool_use::{
    render_assistant_tool_use_to_string, AssistantToolUseProps, MARKER,
};

fn id() -> ToolUseId {
    ToolUseId::from("toolu_test")
}

#[test]
fn collapsed_read_with_file_path() {
    // (ma-01) Per-tool preview: Read shows getDisplayPath(file_path) — here
    // cwd=/tmp shortens /tmp/x.rs to x.rs.
    let s = render_assistant_tool_use_to_string(AssistantToolUseProps {
        id: id(),
        tool: "Read".into(),
        input: serde_json::json!({"file_path": "/tmp/x.rs"}),
        expanded: false,
        focused: false,
        cwd: PathBuf::from("/tmp"),
        resolution: None,
    });
    // (ma-03) MARKER is the platform BLACK_CIRCLE (⏺ macOS / ● else).
    assert_eq!(s, format!("{MARKER} Read(x.rs)"));
}

#[test]
fn expanded_read_shows_pretty_json() {
    let s = render_assistant_tool_use_to_string(AssistantToolUseProps {
        id: id(),
        tool: "Read".into(),
        input: serde_json::json!({"file_path": "/tmp/x.rs", "limit": 100}),
        expanded: true,
        focused: false,
        cwd: PathBuf::from("/tmp"),
        resolution: None,
    });
    assert_eq!(
        s,
        format!("{MARKER} Read(x.rs)\n{{\n  \"file_path\": \"/tmp/x.rs\",\n  \"limit\": 100\n}}")
    );
}

#[test]
fn focused_collapsed_has_arrow_prefix() {
    let s = render_assistant_tool_use_to_string(AssistantToolUseProps {
        id: id(),
        tool: "Read".into(),
        input: serde_json::json!({"file_path": "/tmp/x.rs"}),
        expanded: false,
        focused: true,
        cwd: PathBuf::from("/tmp"),
        resolution: None,
    });
    assert_eq!(s, format!("> {MARKER} Read(x.rs)"));
}

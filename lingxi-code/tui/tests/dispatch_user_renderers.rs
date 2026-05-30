//! (M7-05) Dispatch coverage: each of the 12 new variants routes through
//! `render_entry_to_string` to the right renderer (catches wrong-arm
//! copy-paste the compiler can't).
#![allow(clippy::doc_markdown)]

use lingxi_protocol::ToolUseId;
use lingxi_tui::components::messages::attachment::Attachment;
use lingxi_tui::components::messages::render_entry_to_string;
use lingxi_tui::state::RenderedMessage;

fn s(m: &RenderedMessage) -> String {
    render_entry_to_string(m, /*focused*/ false, /*expanded*/ false)
}

#[test]
fn dispatch_bash_input() {
    assert_eq!(
        s(&RenderedMessage::UserBashInput {
            command: "ls".into()
        }),
        "! ls"
    );
}

#[test]
fn dispatch_bash_output() {
    assert_eq!(
        s(&RenderedMessage::UserBashOutput {
            stdout: "ok".into(),
            stderr: String::new()
        }),
        "ok"
    );
}

#[test]
fn dispatch_command() {
    assert_eq!(
        s(&RenderedMessage::UserCommand {
            command: "help".into(),
            args: String::new(),
            is_skill: false
        }),
        "\u{276F} /help"
    );
}

#[test]
fn dispatch_local_command_output() {
    assert_eq!(
        s(&RenderedMessage::UserLocalCommandOutput {
            stdout: String::new(),
            stderr: String::new()
        }),
        "(no content)"
    );
}

#[test]
fn dispatch_memory_input() {
    assert_eq!(
        s(&RenderedMessage::UserMemoryInput { input: "x".into() }),
        "# x\nGot it."
    );
}

#[test]
fn dispatch_plan() {
    assert!(s(&RenderedMessage::UserPlan {
        plan_content: "p".into()
    })
    .starts_with("Plan to implement"));
}

#[test]
fn dispatch_prompt() {
    assert_eq!(s(&RenderedMessage::UserPrompt { text: "hi".into() }), "hi");
}

#[test]
fn dispatch_resource_update() {
    let m = RenderedMessage::UserResourceUpdate {
        updates: vec![("fs".into(), "x.rs".into(), None)],
    };
    assert_eq!(s(&m), "\u{21BB} fs: x.rs");
}

#[test]
fn dispatch_image() {
    assert_eq!(
        s(&RenderedMessage::UserImage {
            image_id: Some(2),
            metadata: None
        }),
        "[Image #2]"
    );
}

#[test]
fn dispatch_attachment() {
    let m = RenderedMessage::Attachment {
        attachment: Attachment::NestedMemory {
            display_path: "M.md".into(),
        },
    };
    assert_eq!(s(&m), "Loaded M.md");
}

#[test]
fn dispatch_grouped_tool_use() {
    let m = RenderedMessage::GroupedToolUse {
        tool: "Read".into(),
        group_id: ToolUseId::new(),
        entries: vec![
            (serde_json::json!({}), serde_json::json!({"content": "a"})),
            (serde_json::json!({}), serde_json::json!({"content": "b"})),
        ],
    };
    assert_eq!(s(&m), "\u{25CF} Read (\u{00D7}2)");
}

#[test]
fn dispatch_collapsed_read_search() {
    let m = RenderedMessage::CollapsedReadSearch {
        search_count: 0,
        read_count: 1,
        list_count: 0,
        is_active: false,
        group_id: ToolUseId::new(),
        entries: vec![],
    };
    assert_eq!(s(&m), "  \u{23BF}  Read 1 file");
}

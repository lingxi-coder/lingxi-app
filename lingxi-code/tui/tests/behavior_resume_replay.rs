//! Behavior tests: transcript replay on session resume.
//!
//! A RESUMED session seeds `AppState.messages` with the prior conversation
//! (mapped to the right `RenderedMessage` kinds; tool results grouped under
//! their originating tool-use via the `sourceToolUseID` side-table) BEFORE the
//! first render — the Rust analog of claude-code's REPL `initialMessages`. A
//! FRESH session seeds NONE, so its initial state is byte-identical to today.
//!
//! These are pure state assertions over the public seam
//! (`tui::replay::rebuild_messages` + `AppState::seed_resumed_messages` +
//! `Runtime::with_resumed_messages`); they do NOT drive a PTY and do NOT depend
//! on any locked snapshot.

use protocol::{ContentBlock, ConversationMessage, MessageId, ToolUseId};
use tui::replay::{rebuild_from_jsonl, rebuild_messages};
use tui::session::Runtime;
use tui::state::{AppState, RenderedMessage, StatusSnapshot};

fn user(text: &str) -> ConversationMessage {
    ConversationMessage::User {
        id: MessageId::new(),
        content: vec![ContentBlock::Text { text: text.into() }],
    }
}

fn assistant(text: &str) -> ConversationMessage {
    ConversationMessage::Assistant {
        id: MessageId::new(),
        content: vec![ContentBlock::Text { text: text.into() }],
        stop_reason: None,
    }
}

/// A persisted transcript with text + a tool-use/result pair seeds the
/// scrollback in arrival order, with the result grouped under its tool-use.
#[test]
fn resumed_session_seeds_messages_in_order_with_tool_grouping() {
    let id = ToolUseId::new();
    let history = vec![
        user("first prompt"),
        assistant("let me check"),
        ConversationMessage::Assistant {
            id: MessageId::new(),
            content: vec![ContentBlock::ToolUse {
                id,
                name: "Read".into(),
                input: serde_json::json!({ "file_path": "/a" }),
            }],
            stop_reason: None,
        },
        ConversationMessage::User {
            id: MessageId::new(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: id,
                content: "contents".into(),
                is_error: false,
            }],
        },
        assistant("done"),
    ];

    let rows = rebuild_messages(&history);

    let mut state = AppState::new(StatusSnapshot::default());
    assert!(state.messages.is_empty(), "fresh state starts empty");
    state.seed_resumed_messages(rows);

    assert_eq!(state.messages.len(), 5);
    assert!(
        matches!(&state.messages[0], RenderedMessage::UserText { body, .. } if body == "first prompt")
    );
    assert!(
        matches!(&state.messages[1], RenderedMessage::AssistantText { body, .. } if body == "let me check")
    );
    assert!(
        matches!(&state.messages[2], RenderedMessage::AssistantToolUse { id: gid, tool, .. } if *gid == id && tool == "Read")
    );
    // The tool result is grouped under its originating tool-use id.
    match &state.messages[3] {
        RenderedMessage::UserToolResult { id: gid, tool, .. } => {
            assert_eq!(*gid, id);
            assert_eq!(tool, "Read");
        }
        other => panic!("expected UserToolResult, got {other:?}"),
    }
    assert!(
        matches!(&state.messages[4], RenderedMessage::AssistantText { body, .. } if body == "done")
    );
}

/// A FRESH session seeds no messages: `with_resumed_messages` is never called,
/// so `resumed_messages` is empty and the initial `AppState` is byte-identical
/// to a plain `AppState::new` — no replay.
#[test]
fn fresh_session_seeds_no_messages_byte_identical() {
    // The Runtime default carries no resumed messages.
    let runtime = Runtime::new(protocol::SessionId::new());
    assert!(
        runtime.resumed_messages.is_empty(),
        "a fresh Runtime carries no resumed transcript"
    );

    // The seeding guard in `run_tui_session` only fires for a non-empty vec, so
    // a fresh AppState is left exactly as `AppState::new` produced it.
    let fresh = AppState::new(StatusSnapshot::default());
    assert!(fresh.messages.is_empty());

    // Seeding with an empty vec is a no-op (still empty) — the same observable
    // state as never calling it.
    let mut seeded_empty = AppState::new(StatusSnapshot::default());
    seeded_empty.seed_resumed_messages(Vec::new());
    assert!(seeded_empty.messages.is_empty());
}

/// The `Runtime::with_resumed_messages` builder carries the seed through to the
/// runtime that `run_tui_session` consumes.
#[test]
fn with_resumed_messages_threads_seed_onto_runtime() {
    let rows = rebuild_messages(&[user("hi"), assistant("hello")]);
    let runtime = Runtime::new(protocol::SessionId::new()).with_resumed_messages(rows);
    assert_eq!(runtime.resumed_messages.len(), 2);
}

/// The loader's raw JSONL path (`rebuild_from_jsonl`) — the shape the CLI
/// `--resume <uuid>` branch holds — produces the same seed and grouping.
#[test]
fn jsonl_loader_path_seeds_and_groups() {
    let id = ToolUseId::new();
    let id_wire = serde_json::to_value(id).unwrap();
    let line = |ty: &str, content: serde_json::Value| -> session::jsonl::JsonlMessage {
        serde_json::from_value(serde_json::json!({
            "type": ty,
            "uuid": uuid::Uuid::new_v4().to_string(),
            "parentUuid": null,
            "sessionId": uuid::Uuid::new_v4().to_string(),
            "timestamp": "2026-05-25T14:30:00.000Z",
            "cwd": "/tmp",
            "version": "0.0.0",
            "message": { "content": content },
        }))
        .unwrap()
    };
    let msgs = vec![
        line("user", serde_json::json!("hi")),
        line(
            "assistant",
            serde_json::json!([
                { "type": "tool_use", "id": id_wire, "name": "Bash",
                  "input": { "command": "ls" } },
            ]),
        ),
        line(
            "user",
            serde_json::json!([
                { "type": "tool_result", "tool_use_id": id_wire,
                  "content": "a\nb", "is_error": false },
            ]),
        ),
    ];

    let rows = rebuild_from_jsonl(&msgs);
    let mut state = AppState::new(StatusSnapshot::default());
    state.seed_resumed_messages(rows);

    assert_eq!(state.messages.len(), 3);
    assert!(matches!(&state.messages[0], RenderedMessage::UserText { body, .. } if body == "hi"));
    assert!(
        matches!(&state.messages[1], RenderedMessage::AssistantToolUse { id: gid, tool, .. } if *gid == id && tool == "Bash")
    );
    assert!(
        matches!(&state.messages[2], RenderedMessage::UserToolResult { id: gid, tool, .. } if *gid == id && tool == "Bash")
    );
}

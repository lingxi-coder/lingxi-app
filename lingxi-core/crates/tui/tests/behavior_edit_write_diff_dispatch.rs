//! End-to-end behavior: Edit/Write `ToolUseStart` → `ToolUseResult` drives a
//! rendered `StructuredDiff` (M7-02 T13-wire).
//!
//! This exercises the SAME surface the live app uses: the streaming subscriber
//! (`apply_event`) turns the bridge's `TurnEvent`s into a
//! `RenderedMessage::UserToolResult`, and the scrollback dispatcher renders that
//! entry via the iocraft `UserToolResultMessage` component. The headline
//! deliverable is that the call INPUT (`old_string`/`new_string`/`file_path` for
//! Edit; `content` for Write) — which arrives on the EARLIER `ToolUseStart`
//! event — reaches the result-render site so the `-`/`+` diff actually renders.
//!
//! Before the wiring these assertions FAIL: the diff inputs default to `None` at
//! the dispatch site, the diff branch never fires, and the rendered output is
//! the plain `└ ` body with no `-`/`+` sigils.

use iocraft::prelude::*;
use lingxi_protocol::ToolUseId;
use lingxi_tui::components::messages::user_tool_result::UserToolResultMessage;
use lingxi_tui::events::orchestrator_bridge::TurnEvent;
use lingxi_tui::state::{AppState, RenderedMessage, StatusSnapshot};
use lingxi_tui::streaming::apply_event;
use tokio::sync::Notify;

/// Drive `ToolUseStart` + `ToolUseResult` through `apply_event`, then render the
/// resulting `UserToolResult` entry through the SAME mapping the scrollback
/// dispatcher uses (threading the diff inputs into `UserToolResultProps`). The
/// rendered terminal string is returned.
fn render_last_result(st: &AppState) -> String {
    let last = st.messages.last().expect("a UserToolResult entry");
    let RenderedMessage::UserToolResult {
        id,
        tool,
        result,
        old_string,
        new_string,
        file_path,
    } = last
    else {
        panic!("expected UserToolResult, got {last:?}");
    };
    // Mirror `scrollback::render_message`'s prop threading exactly: this is the
    // live dispatch path the app uses to turn a `RenderedMessage::UserToolResult`
    // into the rendered iocraft component.
    let mut element = element! {
        UserToolResultMessage(
            id: *id,
            tool: tool.clone(),
            result: result.clone(),
            expanded: false,
            focused: false,
            old_string: old_string.clone(),
            new_string: new_string.clone(),
            file_path: file_path.clone(),
        )
    };
    element.to_string()
}

#[test]
fn edit_tool_flow_renders_diff_with_old_and_new() {
    let mut st = AppState::new(StatusSnapshot::default());
    let id = ToolUseId::new();
    let n = Notify::new();
    apply_event(
        &mut st,
        TurnEvent::ToolUseStart {
            id,
            tool: "Edit".into(),
            input: serde_json::json!({
                "file_path": "/tmp/a.rs",
                "old_string": "foo()",
                "new_string": "bar()",
            }),
        },
        &n,
    );
    apply_event(
        &mut st,
        TurnEvent::ToolUseResult {
            id,
            tool: "Edit".into(),
            result: serde_json::json!({"content": "ok"}),
        },
        &n,
    );

    let out = render_last_result(&st);
    assert!(
        out.contains('-') && out.contains('+'),
        "Edit must render a -/+ diff, got:\n{out}"
    );
    assert!(out.contains("foo"), "old text present, got:\n{out}");
    assert!(out.contains("bar"), "new text present, got:\n{out}");
}

#[test]
fn write_tool_flow_renders_pure_add_diff() {
    let mut st = AppState::new(StatusSnapshot::default());
    let id = ToolUseId::new();
    let n = Notify::new();
    apply_event(
        &mut st,
        TurnEvent::ToolUseStart {
            id,
            tool: "Write".into(),
            input: serde_json::json!({
                "file_path": "/tmp/b.rs",
                "content": "new line 1\nnew line 2",
            }),
        },
        &n,
    );
    apply_event(
        &mut st,
        TurnEvent::ToolUseResult {
            id,
            tool: "Write".into(),
            result: serde_json::json!({"content": "ok"}),
        },
        &n,
    );

    let out = render_last_result(&st);
    assert!(
        out.contains('+'),
        "Write is a pure-add diff (+ present), got:\n{out}"
    );
    assert!(
        out.contains("new line 1"),
        "added content present, got:\n{out}"
    );
}

#[test]
fn bash_tool_flow_does_not_render_diff() {
    // Regression guard: a non-diff tool renders as the plain `└ ` body, never a
    // diff, even though it too flows through the populated dispatch path.
    let mut st = AppState::new(StatusSnapshot::default());
    let id = ToolUseId::new();
    let n = Notify::new();
    apply_event(
        &mut st,
        TurnEvent::ToolUseStart {
            id,
            tool: "Bash".into(),
            input: serde_json::json!({"command": "ls"}),
        },
        &n,
    );
    apply_event(
        &mut st,
        TurnEvent::ToolUseResult {
            id,
            tool: "Bash".into(),
            result: serde_json::json!({"content": "file_a.txt"}),
        },
        &n,
    );

    // The diff inputs are `None` for non-diff tools.
    let RenderedMessage::UserToolResult {
        old_string,
        new_string,
        ..
    } = st.messages.last().unwrap()
    else {
        panic!("expected UserToolResult");
    };
    assert!(old_string.is_none() && new_string.is_none());

    let out = render_last_result(&st);
    assert!(out.contains("file_a.txt"), "body rendered, got:\n{out}");
}

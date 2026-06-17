//! M6-04 Task 13: end-to-end flow — `TurnEvent` → `AppState` → rendered output.
//!
//! The plan-spec uses `app.apply_output_event(OutputEvent::…)`, but in this
//! codebase the equivalent contract is `streaming::apply_event(state, TurnEvent::…)`
//! (the bridge translates `OutputEvent::ToolCall/ToolResult` into
//! `TurnEvent::ToolUseStart/ToolUseResult`). We exercise the same surface
//! here.

use protocol::ToolUseId;
use tokio::sync::Notify;
use tui::components::messages::render_entry_to_string;
use tui::events::orchestrator_bridge::TurnEvent;
use tui::state::{AppState, StatusSnapshot};
use tui::streaming::apply_event;

#[test]
fn full_flow_call_then_result_renders_both_blocks() {
    let mut st = AppState::new(StatusSnapshot::default());
    let id = ToolUseId::new();
    let notify = Notify::new();
    apply_event(
        &mut st,
        TurnEvent::ToolUseStart {
            id: id.clone(),
            tool: "Read".into(),
            input: serde_json::json!({"file_path": "/tmp/x.rs"}),
        },
        &notify,
    );
    apply_event(
        &mut st,
        TurnEvent::ToolUseResult {
            id: id.clone(),
            tool: "Read".into(),
            result: serde_json::json!({"content": "fn main() {}"}),
        },
        &notify,
    );
    assert_eq!(st.messages.len(), 2);
    let s0 = render_entry_to_string(&st.messages[0], false, false);
    assert!(s0.contains("● Read"), "got: {s0}");
    let s1 = render_entry_to_string(&st.messages[1], false, false);
    assert!(s1.starts_with("└ fn main() {}"), "got: {s1}");
}

#[test]
fn expanded_state_flips_with_toggle() {
    let mut st = AppState::new(StatusSnapshot::default());
    let id = ToolUseId::new();
    let notify = Notify::new();
    apply_event(
        &mut st,
        TurnEvent::ToolUseStart {
            id: id.clone(),
            tool: "Read".into(),
            input: serde_json::json!({"file_path": "/tmp/x.rs"}),
        },
        &notify,
    );
    st.focus_next_tool();
    st.toggle_expanded(&id);
    let expanded = st.expanded.get(&id).copied().unwrap_or(false);
    let s = render_entry_to_string(&st.messages[0], true, expanded);
    // Expanded form contains the pretty-printed JSON body.
    assert!(s.contains("{\n  \"file_path\""), "got: {s}");
}

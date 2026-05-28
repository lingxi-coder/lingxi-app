//! M6-04 Tasks 10 + 11: focus walking, expand toggle, dispatcher routing.
//!
//! The plan-spec uses `handle_key(app, KeyEvent)` against a hypothetical
//! AppMode-aware keymap that this codebase doesn't have. We exercise the
//! same contract via `events::keymap::map_key` + `app::dispatch`, which
//! is what the iocraft root assembles in production.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use lingxi_protocol::ToolUseId;
use lingxi_tui::app::dispatch;
use lingxi_tui::components::messages::render_entry_to_string;
use lingxi_tui::events::keymap::map_key;
use lingxi_tui::state::{AppState, RenderedMessage, StatusSnapshot};

fn k(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn seed_state_with_two_tools() -> (AppState, ToolUseId, ToolUseId) {
    let mut st = AppState::new(StatusSnapshot::default());
    let a = ToolUseId::new();
    let b = ToolUseId::new();
    st.push_message(RenderedMessage::AssistantToolUse {
        id: a,
        tool: "Read".into(),
        input: serde_json::json!({"file_path": "/tmp/x"}),
    });
    st.push_message(RenderedMessage::AssistantToolUse {
        id: b,
        tool: "Bash".into(),
        input: serde_json::json!({"command": "ls"}),
    });
    (st, a, b)
}

#[test]
fn down_arrow_focuses_first_tool_then_second() {
    let (mut st, a, b) = seed_state_with_two_tools();
    let prompt_empty = true;
    let focus_active = true;
    let action = map_key(k(KeyCode::Down), prompt_empty, focus_active).unwrap();
    dispatch(action, &mut st);
    assert_eq!(st.focused_tool_id, Some(a));
    let action = map_key(k(KeyCode::Down), prompt_empty, focus_active).unwrap();
    dispatch(action, &mut st);
    assert_eq!(st.focused_tool_id, Some(b));
}

#[test]
fn e_keypress_toggles_expanded_for_focused_tool() {
    let (mut st, a, _b) = seed_state_with_two_tools();
    // Focus the first tool.
    let action = map_key(k(KeyCode::Down), true, true).unwrap();
    dispatch(action, &mut st);
    assert_eq!(
        st.expanded.get(&a).copied().unwrap_or(false),
        false,
        "default is collapsed"
    );
    // Toggle once → expanded.
    let action = map_key(k(KeyCode::Char('e')), true, true).unwrap();
    dispatch(action, &mut st);
    assert_eq!(st.expanded.get(&a).copied(), Some(true));
    // Toggle again → collapsed.
    let action = map_key(k(KeyCode::Char('e')), true, true).unwrap();
    dispatch(action, &mut st);
    assert_eq!(st.expanded.get(&a).copied(), Some(false));
}

#[test]
fn enter_keypress_toggles_expanded_for_focused_tool() {
    let (mut st, a, _b) = seed_state_with_two_tools();
    let action = map_key(k(KeyCode::Down), true, true).unwrap();
    dispatch(action, &mut st);
    let action = map_key(k(KeyCode::Enter), true, true).unwrap();
    dispatch(action, &mut st);
    assert_eq!(st.expanded.get(&a).copied(), Some(true));
}

/// M6-04 T11: the message dispatcher routes both new variants through
/// `render_entry_to_string`.
#[test]
fn dispatcher_routes_tool_call_and_result() {
    let id = ToolUseId::nil();
    let call = RenderedMessage::AssistantToolUse {
        id,
        tool: "Read".into(),
        input: serde_json::json!({"file_path": "/tmp/x"}),
    };
    let s = render_entry_to_string(&call, false, false);
    assert!(s.starts_with("● Read("), "got: {s}");
    let result = RenderedMessage::UserToolResult {
        id,
        tool: "Read".into(),
        result: serde_json::json!({"content": "hi"}),
    };
    let s2 = render_entry_to_string(&result, false, false);
    assert!(s2.starts_with("└ hi"), "got: {s2}");
}

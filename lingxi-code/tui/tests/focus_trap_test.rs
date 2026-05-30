//! M6-05 focus-trap behaviour tests.
//!
//! When `pending_permission.is_some()`, ALL key events route into the
//! active dialog and `PromptInput` / scrollback bindings are inert.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use permission::gate::{PermissionRequest, PromptDefault};
use serde_json::json;
use tui::events::keymap::handle_key;
use tui::state::{AppState, PendingPermission, StatusSnapshot};

fn k(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

#[test]
fn dialog_open_prompt_input_not_mutated() {
    let mut state = AppState::new(StatusSnapshot::default());
    state.prompt_text = "hello".to_string();
    state.pending_permission = Some(PendingPermission {
        request: PermissionRequest::ToolUseConfirm {
            tool_name: "Bash".to_string(),
            tool_input: json!({}),
            default_decision: PromptDefault::DenyByDefault,
        },
    });
    // User types 'h' — must NOT append to prompt_text.
    handle_key(&mut state, k('h'));
    assert_eq!(state.prompt_text, "hello");
    // The dialog is still open ('h' isn't a binding for ToolUseConfirm).
    assert!(state.pending_permission.is_some());
}

#[test]
fn no_dialog_open_prompt_input_accepts_keys() {
    let mut state = AppState::new(StatusSnapshot::default());
    state.prompt_text = "hel".to_string();
    state.prompt_cursor = 3;
    handle_key(&mut state, k('l'));
    assert_eq!(state.prompt_text, "hell");
}

//! M6-05 behaviour tests — drive the full keymap → dialog → response
//! oneshot path end-to-end.
//!
//! These tests verify the contract of `events::keymap::handle_key`:
//! when a permission dialog is pending and the user produces a
//! resolving keystroke, the matching `PermissionResponse` is sent over
//! the oneshot back-channel and the dialog slot is cleared.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use lingxi_permission::gate::{
    PermissionRequest, PermissionResponse, PromptDefault,
};
use lingxi_tui::events::keymap::handle_key;
use lingxi_tui::state::{AppState, PendingPermission, StatusSnapshot};
use serde_json::json;
use tokio::sync::oneshot;

fn k(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn open_tool_use_dialog(state: &mut AppState) -> oneshot::Receiver<PermissionResponse> {
    let (tx, rx) = oneshot::channel();
    state.pending_permission = Some(PendingPermission {
        request: PermissionRequest::ToolUseConfirm {
            tool_name: "Bash".to_string(),
            tool_input: json!({"command": "ls"}),
            default_decision: PromptDefault::DenyByDefault,
        },
    });
    state.pending_permission_resp_tx = Some(tx);
    state.pending_permission_started_at = Some(std::time::Instant::now());
    rx
}

#[tokio::test]
async fn feed_1_sends_allow_once() {
    let mut state = AppState::new(StatusSnapshot::default());
    let rx = open_tool_use_dialog(&mut state);
    handle_key(&mut state, k(KeyCode::Char('1')));
    let resp = rx.await.unwrap();
    assert_eq!(resp, PermissionResponse::AllowOnce);
    assert!(state.pending_permission.is_none());
    assert!(state.pending_permission_resp_tx.is_none());
}

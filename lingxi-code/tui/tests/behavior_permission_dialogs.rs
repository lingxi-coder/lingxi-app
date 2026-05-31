//! M6-05 behaviour tests — drive the full keymap → dialog → response
//! oneshot path end-to-end.
//!
//! These tests verify the contract of `events::keymap::handle_key`:
//! when a permission dialog is pending and the user produces a
//! resolving keystroke, the matching `PermissionResponse` is sent over
//! the oneshot back-channel and the dialog slot is cleared.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use permission::gate::{PermissionRequest, PermissionResponse, PromptDefault};
use serde_json::json;
use tokio::sync::oneshot;
use tui::events::keymap::handle_key;
use tui::state::{AppState, PendingPermission, StatusSnapshot};

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
        worker: None,
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

#[tokio::test]
async fn feed_2_sends_allow_always() {
    let mut state = AppState::new(StatusSnapshot::default());
    let rx = open_tool_use_dialog(&mut state);
    handle_key(&mut state, k(KeyCode::Char('2')));
    let resp = rx.await.unwrap();
    assert_eq!(resp, PermissionResponse::AllowAlways);
}

#[tokio::test]
async fn feed_lowercase_n_sends_deny() {
    let mut state = AppState::new(StatusSnapshot::default());
    let rx = open_tool_use_dialog(&mut state);
    handle_key(&mut state, k(KeyCode::Char('n')));
    let resp = rx.await.unwrap();
    assert_eq!(resp, PermissionResponse::Deny);
}

#[tokio::test]
async fn feed_uppercase_n_sends_deny() {
    let mut state = AppState::new(StatusSnapshot::default());
    let rx = open_tool_use_dialog(&mut state);
    handle_key(&mut state, k(KeyCode::Char('N')));
    let resp = rx.await.unwrap();
    assert_eq!(resp, PermissionResponse::Deny);
}

#[tokio::test]
async fn feed_esc_sends_deny() {
    let mut state = AppState::new(StatusSnapshot::default());
    let rx = open_tool_use_dialog(&mut state);
    handle_key(&mut state, k(KeyCode::Esc));
    let resp = rx.await.unwrap();
    assert_eq!(resp, PermissionResponse::Deny);
}

fn open_bypass_dialog(state: &mut AppState) -> oneshot::Receiver<PermissionResponse> {
    let (tx, rx) = oneshot::channel();
    state.pending_permission = Some(PendingPermission {
        request: PermissionRequest::BypassPermissionsMode,
        worker: None,
    });
    state.pending_permission_resp_tx = Some(tx);
    state.pending_permission_started_at = Some(std::time::Instant::now());
    rx
}

#[tokio::test]
async fn bypass_requires_typed_yes_then_enter() {
    let mut state = AppState::new(StatusSnapshot::default());
    let rx = open_bypass_dialog(&mut state);

    // Step 1: feed `y`, `e`, `s` — no resolution yet.
    handle_key(&mut state, k(KeyCode::Char('y')));
    assert!(state.pending_permission.is_some());
    handle_key(&mut state, k(KeyCode::Char('e')));
    assert!(state.pending_permission.is_some());
    handle_key(&mut state, k(KeyCode::Char('s')));
    assert!(state.pending_permission.is_some());

    // Step 2: feed Enter — now resolves to AllowOnce.
    handle_key(&mut state, k(KeyCode::Enter));
    let resp = rx.await.unwrap();
    assert_eq!(resp, PermissionResponse::AllowOnce);
    assert!(state.pending_permission.is_none());
}

#[tokio::test]
async fn bypass_enter_before_yes_does_not_resolve() {
    let mut state = AppState::new(StatusSnapshot::default());
    let _rx = open_bypass_dialog(&mut state);
    handle_key(&mut state, k(KeyCode::Char('y')));
    handle_key(&mut state, k(KeyCode::Enter));
    assert!(
        state.pending_permission.is_some(),
        "Enter before full 'yes' must not resolve"
    );
}

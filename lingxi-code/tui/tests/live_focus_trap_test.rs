//! M6-05 final-review regression test: the LIVE key path must enforce the
//! permission focus-trap.
//!
//! Background: `TuiRoot`'s `use_terminal_events` closure (M6-04) historically
//! went `map_iocraft_key` → `app::dispatch`, never branching on
//! `pending_permission` and never calling `keymap::handle_key`. So in the real
//! binary the dialog appeared but keystrokes silently mutated the hidden
//! prompt buffer; the `resp_tx` oneshot never fired and the orchestrator's
//! `TuiPermissionGate::check` await hung forever.
//!
//! These tests drive `root::handle_live_key` — THE function the live
//! `use_terminal_events` closure invokes — with an `iocraft::KeyEvent`
//! (crossterm-0.29 re-export, exactly the event type the live mount receives).
//! They assert the dialog resolves, the oneshot fires, and the prompt buffer
//! stays untouched while the dialog is open.

use std::time::Duration;

use iocraft::prelude::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use permission::gate::{PermissionRequest, PermissionResponse, PromptDefault};
use serde_json::json;
use tokio::sync::oneshot;
use tui::root::handle_live_key;
use tui::state::{AppState, PendingPermission, StatusSnapshot};

/// Build an iocraft (crossterm-0.29) key-press event.
fn key(code: KeyCode) -> KeyEvent {
    let mut k = KeyEvent::new(KeyEventKind::Press, code);
    k.modifiers = KeyModifiers::NONE;
    k
}

/// Await the oneshot with a short deadline. Against the (buggy) code where the
/// live path never fires `resp_tx`, this fails fast instead of hanging the
/// whole suite forever — which is precisely the real-world symptom the fix
/// addresses (the gate's `check` await would otherwise hang the turn).
async fn recv_response(rx: oneshot::Receiver<PermissionResponse>) -> PermissionResponse {
    tokio::time::timeout(Duration::from_secs(2), rx)
        .await
        .expect("live key path must fire the resp_tx oneshot (focus-trap)")
        .expect("oneshot sender must not be dropped")
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

/// (a) Pressing `1` through the LIVE path resolves to AllowOnce, fires the
/// oneshot, and clears `pending_permission`.
#[tokio::test]
async fn live_key_1_resolves_allow_once_and_fires_oneshot() {
    let mut state = AppState::new(StatusSnapshot::default());
    let rx = open_tool_use_dialog(&mut state);

    handle_live_key(&mut state, &key(KeyCode::Char('1')), 24);

    let resp = recv_response(rx).await;
    assert_eq!(resp, PermissionResponse::AllowOnce);
    assert!(
        state.pending_permission.is_none(),
        "dialog must clear after resolution"
    );
    assert!(state.pending_permission_resp_tx.is_none());
}

/// (b) Focus-trap: text keys must NOT mutate `prompt_text` while a dialog is
/// open, and the dialog must stay open (`h` isn't a ToolUseConfirm binding).
#[tokio::test]
async fn live_text_key_does_not_mutate_prompt_while_dialog_open() {
    let mut state = AppState::new(StatusSnapshot::default());
    state.prompt_text = "hello".to_string();
    state.prompt_cursor = 5;
    let _rx = open_tool_use_dialog(&mut state);

    handle_live_key(&mut state, &key(KeyCode::Char('h')), 24);

    assert_eq!(
        state.prompt_text, "hello",
        "prompt buffer must be untouched while dialog is open"
    );
    assert!(
        state.pending_permission.is_some(),
        "non-binding key must leave the dialog open"
    );
}

/// (c) When no dialog is open, the LIVE path still feeds text into the prompt
/// (regression guard — the focus-trap branch must not eat normal typing).
#[tokio::test]
async fn live_text_key_appends_when_no_dialog() {
    let mut state = AppState::new(StatusSnapshot::default());
    state.prompt_text = "hel".to_string();
    state.prompt_cursor = 3;

    handle_live_key(&mut state, &key(KeyCode::Char('l')), 24);

    assert_eq!(state.prompt_text, "hell");
}

/// (d) Esc through the LIVE path resolves a ToolUseConfirm to Deny.
#[tokio::test]
async fn live_key_esc_resolves_deny() {
    let mut state = AppState::new(StatusSnapshot::default());
    let rx = open_tool_use_dialog(&mut state);

    handle_live_key(&mut state, &key(KeyCode::Esc), 24);

    let resp = recv_response(rx).await;
    assert_eq!(resp, PermissionResponse::Deny);
    assert!(state.pending_permission.is_none());
}

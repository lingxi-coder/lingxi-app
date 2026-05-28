//! Parity driver for the M6-05 TUI permission dialogs.
//!
//! Walks the `tui_permission_dialogs.json` fixture and replays every
//! locked binding through `lingxi_tui::events::keymap::handle_key`,
//! asserting the matching `PermissionResponse` comes back on the
//! oneshot. Also asserts the label literals (header strings, button
//! labels, bypass body literals).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use lingxi_permission::gate::{PermissionRequest, PermissionResponse, PromptDefault};
use lingxi_test_harness::parity::load_fixture;
use lingxi_tui::events::keymap::handle_key;
use lingxi_tui::state::{AppState, PendingPermission, StatusSnapshot};
use serde_json::json;
use tokio::sync::oneshot;

fn key_from_str(s: &str) -> KeyEvent {
    match s {
        "1" => KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE),
        "2" => KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
        "n" => KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE),
        "N" => KeyEvent::new(KeyCode::Char('N'), KeyModifiers::NONE),
        "y" => KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
        "e" => KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE),
        "s" => KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE),
        "Esc" => KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        "Enter" | "Enter@AllowOnce" => KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        other => panic!("unknown key in fixture: {other}"),
    }
}

#[test]
fn parity_tui_permission_dialogs_labels_locked() {
    let fixture: serde_json::Value = load_fixture("tui_permission_dialogs");

    // Header literal locks.
    assert_eq!(
        fixture["labels"]["tool_use_confirm"]["header_template"],
        "Claude needs your permission to use {tool_name}"
    );
    assert_eq!(
        fixture["labels"]["exit_plan_mode"]["header"],
        "Claude Code needs your approval for the plan"
    );
    assert_eq!(
        fixture["labels"]["bypass_permissions"]["title"],
        "WARNING: Claude Code running in Bypass Permissions mode"
    );

    // Button labels (LingXi-locked).
    assert_eq!(
        fixture["labels"]["tool_use_confirm"]["button_allow_once"],
        "[1] Allow Once"
    );
    assert_eq!(
        fixture["labels"]["tool_use_confirm"]["button_allow_always"],
        "[2] Allow Always"
    );
    assert_eq!(
        fixture["labels"]["tool_use_confirm"]["button_deny"],
        "[N] Deny"
    );
    assert_eq!(
        fixture["labels"]["exit_plan_mode"]["button_allow_once"],
        "[1] Allow Once"
    );

    // Bypass confirmation word locked.
    assert_eq!(
        fixture["labels"]["bypass_permissions"]["confirm_word"],
        "yes"
    );

    // Focus-trap descriptor locked.
    assert_eq!(
        fixture["focus_trap"]["behavior"],
        "all_keys_route_to_dialog"
    );
}

fn map_expects(expects: &str) -> Option<PermissionResponse> {
    match expects {
        "AllowOnce" => Some(PermissionResponse::AllowOnce),
        "AllowAlways" => Some(PermissionResponse::AllowAlways),
        "Deny" => Some(PermissionResponse::Deny),
        "no-resolution" => None,
        other => panic!("unknown expects: {other}"),
    }
}

fn open_tool_use(state: &mut AppState) -> oneshot::Receiver<PermissionResponse> {
    let (tx, rx) = oneshot::channel();
    state.pending_permission = Some(PendingPermission {
        request: PermissionRequest::ToolUseConfirm {
            tool_name: "Bash".to_string(),
            tool_input: json!({}),
            default_decision: PromptDefault::DenyByDefault,
        },
    });
    state.pending_permission_resp_tx = Some(tx);
    state.pending_permission_started_at = Some(std::time::Instant::now());
    rx
}

fn open_exit_plan(state: &mut AppState) -> oneshot::Receiver<PermissionResponse> {
    let (tx, rx) = oneshot::channel();
    state.pending_permission = Some(PendingPermission {
        request: PermissionRequest::ExitPlanMode {
            plan: "1. Do x".to_string(),
        },
    });
    state.pending_permission_resp_tx = Some(tx);
    state.pending_permission_started_at = Some(std::time::Instant::now());
    rx
}

fn open_bypass(state: &mut AppState) -> oneshot::Receiver<PermissionResponse> {
    let (tx, rx) = oneshot::channel();
    state.pending_permission = Some(PendingPermission {
        request: PermissionRequest::BypassPermissionsMode,
    });
    state.pending_permission_resp_tx = Some(tx);
    state.pending_permission_started_at = Some(std::time::Instant::now());
    rx
}

#[tokio::test]
async fn parity_tool_use_confirm_bindings_round_trip() {
    let fixture: serde_json::Value = load_fixture("tui_permission_dialogs");
    for binding in fixture["key_bindings"]["tool_use_confirm"]
        .as_array()
        .unwrap()
    {
        let key_str = binding["key"].as_str().unwrap();
        let expected = map_expects(binding["expects"].as_str().unwrap());
        let mut state = AppState::new(StatusSnapshot::default());
        let rx = open_tool_use(&mut state);
        handle_key(&mut state, key_from_str(key_str));
        match expected {
            Some(want) => {
                let got = rx.await.unwrap_or_else(|_| panic!("no response for {key_str}"));
                assert_eq!(got, want, "tool_use_confirm key {key_str}");
            }
            None => panic!("no-resolution not used in tool_use_confirm"),
        }
    }
}

#[tokio::test]
async fn parity_exit_plan_mode_bindings_round_trip() {
    let fixture: serde_json::Value = load_fixture("tui_permission_dialogs");
    for binding in fixture["key_bindings"]["exit_plan_mode"]
        .as_array()
        .unwrap()
    {
        let key_str = binding["key"].as_str().unwrap();
        let expected = map_expects(binding["expects"].as_str().unwrap());
        let mut state = AppState::new(StatusSnapshot::default());
        let rx = open_exit_plan(&mut state);
        handle_key(&mut state, key_from_str(key_str));
        match expected {
            Some(want) => {
                let got = rx.await.unwrap_or_else(|_| panic!("no response for {key_str}"));
                assert_eq!(got, want, "exit_plan_mode key {key_str}");
            }
            None => panic!("no-resolution not used in exit_plan_mode"),
        }
    }
}

#[tokio::test]
async fn parity_bypass_permissions_sequences_round_trip() {
    let fixture: serde_json::Value = load_fixture("tui_permission_dialogs");
    for binding in fixture["key_bindings"]["bypass_permissions"]
        .as_array()
        .unwrap()
    {
        let keys: Vec<String> = binding["keys"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        let expected = map_expects(binding["expects"].as_str().unwrap());

        let mut state = AppState::new(StatusSnapshot::default());
        let mut rx = open_bypass(&mut state);
        for k in &keys {
            handle_key(&mut state, key_from_str(k));
        }
        match expected {
            Some(want) => {
                let got = rx
                    .await
                    .unwrap_or_else(|_| panic!("no response for {:?}", keys));
                assert_eq!(got, want, "bypass keys {keys:?}");
            }
            None => {
                // Must NOT have resolved: dialog still pending, rx still
                // un-fulfilled (try_recv).
                assert!(state.pending_permission.is_some(), "expected unresolved for {keys:?}");
                assert!(rx.try_recv().is_err(), "expected no oneshot send for {keys:?}");
            }
        }
    }
}

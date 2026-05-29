//! M7-11 behavior tests: screen-overlay routing through the SINGLE live-key
//! dispatcher. Drives `root::handle_live_key` — the exact function the live
//! `use_terminal_events` closure invokes.

use iocraft::prelude::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use lingxi_tui::root::handle_live_key;
use lingxi_tui::screens::doctor::DoctorDiagnostics;
use lingxi_tui::screens::Screen;
use lingxi_tui::state::{AppState, StatusSnapshot};

fn key(code: KeyCode) -> KeyEvent {
    let mut k = KeyEvent::new(KeyEventKind::Press, code);
    k.modifiers = KeyModifiers::NONE;
    k
}

fn diag() -> DoctorDiagnostics {
    DoctorDiagnostics::capture(std::path::Path::new("/work"), 0, 0, (80, 24))
}

#[test]
fn esc_closes_active_screen() {
    let mut st = AppState::new(StatusSnapshot::default());
    st.open_doctor(diag());
    assert_eq!(st.active_screen, Some(Screen::Doctor));
    handle_live_key(&mut st, &key(KeyCode::Esc), 24);
    assert_eq!(st.active_screen, None, "Esc closes the screen → back to REPL");
}

#[test]
fn q_closes_active_screen() {
    let mut st = AppState::new(StatusSnapshot::default());
    st.open_doctor(diag());
    handle_live_key(&mut st, &key(KeyCode::Char('q')), 24);
    assert_eq!(st.active_screen, None, "q closes the screen");
}

#[test]
fn text_key_does_not_leak_to_prompt_while_screen_open() {
    let mut st = AppState::new(StatusSnapshot::default());
    st.prompt_text = "draft".to_string();
    st.prompt_cursor = 5;
    st.open_doctor(diag());
    handle_live_key(&mut st, &key(KeyCode::Char('h')), 24);
    assert_eq!(st.prompt_text, "draft", "text key must NOT reach PromptInput");
    assert_eq!(
        st.active_screen,
        Some(Screen::Doctor),
        "non-close key keeps screen open"
    );
}

use lingxi_permission::gate::{PermissionRequest, PermissionResponse, PromptDefault};
use lingxi_tui::state::PendingPermission;
use serde_json::json;
use std::time::Duration;
use tokio::sync::oneshot;

/// §4 R4 SEAM: Doctor screen open WHILE a permission is pending. The
/// permission (priority 1) wins; its key resolves the DIALOG, not the screen.
#[tokio::test]
async fn permission_wins_over_open_screen() {
    let mut st = AppState::new(StatusSnapshot::default());

    // Doctor screen is open...
    st.open_doctor(diag());
    assert_eq!(st.active_screen, Some(Screen::Doctor));

    // ...and a permission arrives on top of it.
    let (tx, rx) = oneshot::channel();
    st.pending_permission = Some(PendingPermission {
        request: PermissionRequest::ToolUseConfirm {
            tool_name: "Bash".to_string(),
            tool_input: json!({"command": "ls"}),
            default_decision: PromptDefault::DenyByDefault,
        },
    });
    st.pending_permission_resp_tx = Some(tx);
    st.pending_permission_started_at = Some(std::time::Instant::now());

    // Press `1` (ToolUseConfirm: AllowOnce). Priority 1 fires FIRST.
    handle_live_key(&mut st, &key(KeyCode::Char('1')), 24);

    // The dialog resolved...
    let resp = tokio::time::timeout(Duration::from_secs(2), rx)
        .await
        .expect("permission key must resolve the dialog (priority 1 > 2)")
        .expect("oneshot not dropped");
    assert_eq!(resp, PermissionResponse::AllowOnce);
    assert!(st.pending_permission.is_none(), "dialog cleared");

    // ...and the screen is UNTOUCHED — `1` did not close or alter it.
    assert_eq!(
        st.active_screen,
        Some(Screen::Doctor),
        "screen must be unaffected: the permission consumed the key"
    );
}

/// Once the permission is gone, the SAME dispatcher routes the next key to the
/// screen (priority 2): `q` now closes Doctor.
#[test]
fn screen_routing_resumes_after_permission_clears() {
    let mut st = AppState::new(StatusSnapshot::default());
    st.open_doctor(diag());
    // No pending permission → priority 2 owns the key.
    handle_live_key(&mut st, &key(KeyCode::Char('q')), 24);
    assert_eq!(st.active_screen, None);
}

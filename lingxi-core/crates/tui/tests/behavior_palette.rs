//! M7-07 palette behavior + focus-trap, driving `root::handle_live_key` — THE
//! function the live `use_terminal_events` closure calls (mirrors
//! live_focus_trap_test.rs).

use iocraft::prelude::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use lingxi_tui::root::handle_live_key;
use lingxi_tui::state::{AppState, StatusSnapshot};

fn key(code: KeyCode) -> KeyEvent {
    let mut k = KeyEvent::new(KeyEventKind::Press, code);
    k.modifiers = KeyModifiers::NONE;
    k
}

#[test]
fn typing_slash_opens_palette_and_filters() {
    let mut st = AppState::new(StatusSnapshot::default());
    handle_live_key(&mut st, &key(KeyCode::Char('/')), 24);
    assert!(st.palette.open, "/ opens the palette");
    handle_live_key(&mut st, &key(KeyCode::Char('c')), 24);
    handle_live_key(&mut st, &key(KeyCode::Char('o')), 24);
    assert_eq!(st.prompt_text, "/co");
    assert!(st.palette.rows().iter().any(|r| r.name == "compact"));
}

#[test]
fn arrow_selects_then_tab_completes() {
    let mut st = AppState::new(StatusSnapshot::default());
    for ch in "/comp".chars() {
        handle_live_key(&mut st, &key(KeyCode::Char(ch)), 24);
    }
    let sel = st.palette.rows()[st.palette.selected].name.to_string();
    handle_live_key(&mut st, &key(KeyCode::Tab), 24);
    assert_eq!(st.prompt_text, format!("/{sel} "));
    assert!(!st.palette.open, "completing closes the palette");
}

#[test]
fn esc_dismisses_palette_but_keeps_prompt() {
    let mut st = AppState::new(StatusSnapshot::default());
    for ch in "/co".chars() {
        handle_live_key(&mut st, &key(KeyCode::Char(ch)), 24);
    }
    handle_live_key(&mut st, &key(KeyCode::Esc), 24);
    assert!(!st.palette.open);
    assert_eq!(st.prompt_text, "/co", "Esc dismisses overlay, not the text");
}

#[test]
fn focus_trap_down_key_does_not_scroll_scrollback() {
    // With the palette open, Down moves the palette selection — it must NOT
    // be interpreted as history/scroll on the underlying input surface.
    let mut st = AppState::new(StatusSnapshot::default());
    st.history.push("old line".into());
    for ch in "/co".chars() {
        handle_live_key(&mut st, &key(KeyCode::Char(ch)), 24);
    }
    let before = st.prompt_text.clone();
    handle_live_key(&mut st, &key(KeyCode::Down), 24);
    assert_eq!(st.palette.selected, 1, "Down moved palette selection");
    assert_eq!(st.prompt_text, before, "Down did not pull history into prompt");
}

#[test]
fn permission_pending_beats_open_palette() {
    // Cross-state seam (design §5.6): if a permission is pending AND the palette
    // is open, the permission focus-trap (priority 1) wins.
    use lingxi_permission::gate::{PermissionRequest, PromptDefault};
    use lingxi_tui::state::PendingPermission;
    use serde_json::json;
    use tokio::sync::oneshot;

    let mut st = AppState::new(StatusSnapshot::default());
    for ch in "/co".chars() {
        handle_live_key(&mut st, &key(KeyCode::Char(ch)), 24);
    }
    assert!(st.palette.open);
    let (tx, _rx) = oneshot::channel();
    st.pending_permission = Some(PendingPermission {
        request: PermissionRequest::ToolUseConfirm {
            tool_name: "Bash".into(),
            tool_input: json!({"command": "ls"}),
            default_decision: PromptDefault::DenyByDefault,
        },
    });
    st.pending_permission_resp_tx = Some(tx);
    st.pending_permission_started_at = Some(std::time::Instant::now());

    // A palette-navigation key must route to the permission dialog, not the
    // palette: the dialog stays open and the palette selection is untouched.
    handle_live_key(&mut st, &key(KeyCode::Down), 24);
    assert_eq!(st.palette.selected, 0, "permission owns keys; palette unchanged");
    assert!(st.pending_permission.is_some());
}

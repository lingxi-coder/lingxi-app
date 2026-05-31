//! M7-07 palette behavior + focus-trap, driving `root::handle_live_key` — THE
//! function the live `use_terminal_events` closure calls (mirrors
//! `live_focus_trap_test.rs`).

use iocraft::prelude::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use tui::root::handle_live_key;
use tui::state::{AppState, StatusSnapshot};

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
    assert_eq!(
        st.prompt_text, before,
        "Down did not pull history into prompt"
    );
}

#[test]
fn permission_pending_beats_open_palette() {
    // Cross-state seam (design §5.6): if a permission is pending AND the palette
    // is open, the permission focus-trap (priority 1) wins.
    use permission::gate::{PermissionRequest, PromptDefault};
    use serde_json::json;
    use tokio::sync::oneshot;
    use tui::state::PendingPermission;

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
        worker: None,
    });
    st.pending_permission_resp_tx = Some(tx);
    st.pending_permission_started_at = Some(std::time::Instant::now());

    // A palette-navigation key must route to the permission dialog, not the
    // palette: the dialog stays open and the palette selection is untouched.
    handle_live_key(&mut st, &key(KeyCode::Down), 24);
    assert_eq!(
        st.palette.selected, 0,
        "permission owns keys; palette unchanged"
    );
    assert!(st.pending_permission.is_some());
}

#[test]
fn enter_with_no_palette_match_submits_normally() {
    // "/zzzz" matches nothing → Enter must PassThrough to Submit (clears prompt).
    let mut st = AppState::new(StatusSnapshot::default());
    for ch in "/zzzznomatch".chars() {
        handle_live_key(&mut st, &key(KeyCode::Char(ch)), 24);
    }
    assert!(st.palette.open);
    assert!(st.palette.rows().is_empty());
    handle_live_key(&mut st, &key(KeyCode::Enter), 24);
    assert!(st.prompt_text.is_empty(), "Enter submitted the line");
}

#[test]
fn backspacing_the_slash_closes_palette() {
    let mut st = AppState::new(StatusSnapshot::default());
    handle_live_key(&mut st, &key(KeyCode::Char('/')), 24);
    assert!(st.palette.open);
    handle_live_key(&mut st, &key(KeyCode::Backspace), 24);
    assert_eq!(st.prompt_text, "");
    assert!(!st.palette.open, "removing the / closes the palette");
}

#[test]
fn at_token_after_text_opens_completion_not_palette() {
    let mut st = AppState::new(StatusSnapshot::default());
    for ch in "look @".chars() {
        handle_live_key(&mut st, &key(KeyCode::Char(ch)), 24);
    }
    assert!(!st.palette.open, "no leading / → palette stays closed");
    assert!(st.completion.open, "@ after text opens completion");
}

#[test]
fn m7_07_registers_no_new_telemetry_events() {
    // Decision D2: tengu_tui_command_palette_opened is DEFERRED. M7-07 must not
    // register it. The M7-16 audit (which IS the count audit this deferred to)
    // locked the registry at 330 — but palette telemetry STAYED deferred to M8
    // (per-keystroke open/close churn, no clean once-per-open transition), so
    // the count grew via screen_opened/screen_closed/search_opened + the v0.8.0
    // marker, NOT the palette. If this fails, the palette event leaked in.
    let names = telemetry::tengu::ALL_EVENT_NAMES;
    assert_eq!(names.len(), 330, "M7-16 locks the registry at 330");
    assert!(
        !names.contains(&"tengu_tui_command_palette_opened"),
        "palette telemetry is deferred to M8"
    );
}

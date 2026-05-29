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

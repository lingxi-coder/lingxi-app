//! M7-10 behavior: Ctrl-R history search owns keys while open, filters/cycles,
//! Enter accepts into the prompt, Esc restores the pre-search prompt.

use iocraft::prelude::*;
use tui::root::handle_live_key;
use tui::state::{AppState, StatusSnapshot};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(KeyEventKind::Press, code)
}
fn ctrl(c: char) -> KeyEvent {
    let mut k = KeyEvent::new(KeyEventKind::Press, KeyCode::Char(c));
    k.modifiers = KeyModifiers::CONTROL;
    k
}

fn state_with_history() -> AppState {
    let mut st = AppState::new(StatusSnapshot::default());
    st.history = vec![
        "git status".into(),
        "cargo test".into(),
        "git commit -m wip".into(),
        "cargo build".into(),
    ];
    st
}

#[test]
fn ctrl_r_opens_then_filters_then_accepts_into_prompt() {
    let mut st = state_with_history();
    handle_live_key(&mut st, &ctrl('r'), 20);
    assert!(st.history_search.is_some(), "Ctrl-R opens the overlay");

    // Typing 'g','i','t' narrows to the newest "git" entry (idx 2). A bare 'g'
    // would also match "car`g`o build", so we type the full token here.
    handle_live_key(&mut st, &key(KeyCode::Char('g')), 20);
    handle_live_key(&mut st, &key(KeyCode::Char('i')), 20);
    handle_live_key(&mut st, &key(KeyCode::Char('t')), 20);
    let hs = st.history_search.as_ref().unwrap();
    assert_eq!(hs.query, "git");
    assert_eq!(hs.match_index, Some(2));

    // Enter accepts the match into the prompt and closes the overlay.
    handle_live_key(&mut st, &key(KeyCode::Enter), 20);
    assert!(st.history_search.is_none(), "Enter closes the overlay");
    assert_eq!(st.prompt_text, "git commit -m wip");
    assert_eq!(st.prompt_cursor, "git commit -m wip".len());
}

#[test]
fn ctrl_r_cycles_to_older_match() {
    let mut st = state_with_history();
    handle_live_key(&mut st, &ctrl('r'), 20);
    handle_live_key(&mut st, &key(KeyCode::Char('c')), 20);
    handle_live_key(&mut st, &key(KeyCode::Char('a')), 20); // "ca" → idx 3
    assert_eq!(st.history_search.as_ref().unwrap().match_index, Some(3));
    handle_live_key(&mut st, &ctrl('r'), 20); // older "cargo" → idx 1
    assert_eq!(st.history_search.as_ref().unwrap().match_index, Some(1));
}

#[test]
fn esc_restores_pre_search_prompt() {
    let mut st = state_with_history();
    st.prompt_text = "half typed".into();
    st.prompt_cursor = "half typed".len();
    handle_live_key(&mut st, &ctrl('r'), 20);
    handle_live_key(&mut st, &key(KeyCode::Char('g')), 20);
    handle_live_key(&mut st, &key(KeyCode::Esc), 20);
    assert!(st.history_search.is_none());
    assert_eq!(
        st.prompt_text, "half typed",
        "Esc restores the original prompt"
    );
    assert_eq!(st.prompt_cursor, "half typed".len());
}

#[test]
fn no_match_state_keeps_overlay_open_and_match_none() {
    let mut st = state_with_history();
    handle_live_key(&mut st, &ctrl('r'), 20);
    handle_live_key(&mut st, &key(KeyCode::Char('z')), 20);
    let hs = st.history_search.as_ref().unwrap();
    assert_eq!(hs.query, "z");
    assert_eq!(hs.match_index, None, "no history line contains 'z'");
}

#[test]
fn empty_history_open_has_no_match() {
    let mut st = AppState::new(StatusSnapshot::default()); // history empty
    handle_live_key(&mut st, &ctrl('r'), 20);
    assert!(st.history_search.is_some());
    handle_live_key(&mut st, &key(KeyCode::Char('x')), 20);
    assert_eq!(st.history_search.as_ref().unwrap().match_index, None);
}

#[test]
fn while_active_normal_edit_keys_are_captured_by_search() {
    // The prompt must NOT receive characters while the overlay is open.
    let mut st = state_with_history();
    st.prompt_text = String::new();
    handle_live_key(&mut st, &ctrl('r'), 20);
    handle_live_key(&mut st, &key(KeyCode::Char('a')), 20);
    handle_live_key(&mut st, &key(KeyCode::Char('b')), 20);
    assert_eq!(
        st.prompt_text, "",
        "prompt untouched while search owns keys"
    );
    assert_eq!(st.history_search.as_ref().unwrap().query, "ab");
}

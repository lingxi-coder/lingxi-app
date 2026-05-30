//! M7-07 `@` completion behavior + focus-trap via `root::handle_live_key`.
//! These run in a temp dir so the cwd listing is deterministic.

use iocraft::prelude::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use tui::root::handle_live_key;
use tui::state::{AppState, StatusSnapshot};

fn key(code: KeyCode) -> KeyEvent {
    let mut k = KeyEvent::new(KeyEventKind::Press, code);
    k.modifiers = KeyModifiers::NONE;
    k
}

/// Build a state whose status.cwd points at a temp dir with two known files.
fn state_with_files() -> (AppState, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("alpha.rs"), "").unwrap();
    std::fs::write(dir.path().join("beta.rs"), "").unwrap();
    let status = StatusSnapshot {
        cwd: dir.path().to_path_buf(),
        ..StatusSnapshot::default()
    };
    (AppState::new(status), dir)
}

#[test]
fn typing_at_opens_completion_with_cwd_entries() {
    let (mut st, _dir) = state_with_files();
    handle_live_key(&mut st, &key(KeyCode::Char('@')), 24);
    assert!(st.completion.open, "@ opens completion");
    let rows = st.completion.rows();
    assert!(rows.iter().any(|r| r == "alpha.rs"));
    assert!(rows.iter().any(|r| r == "beta.rs"));
}

#[test]
fn filter_narrows_completion() {
    let (mut st, _dir) = state_with_files();
    for ch in "@al".chars() {
        handle_live_key(&mut st, &key(KeyCode::Char(ch)), 24);
    }
    let rows = st.completion.rows();
    assert_eq!(rows, vec!["alpha.rs".to_string()]);
}

#[test]
fn tab_inserts_path_with_at_and_trailing_space() {
    let (mut st, _dir) = state_with_files();
    for ch in "@al".chars() {
        handle_live_key(&mut st, &key(KeyCode::Char(ch)), 24);
    }
    handle_live_key(&mut st, &key(KeyCode::Tab), 24);
    assert_eq!(st.prompt_text, "@alpha.rs ");
    assert!(!st.completion.open, "insert closes completion");
}

#[test]
fn esc_dismisses_completion_keeps_text() {
    let (mut st, _dir) = state_with_files();
    for ch in "@al".chars() {
        handle_live_key(&mut st, &key(KeyCode::Char(ch)), 24);
    }
    handle_live_key(&mut st, &key(KeyCode::Esc), 24);
    assert!(!st.completion.open);
    assert_eq!(st.prompt_text, "@al");
}

#[test]
fn typing_plain_text_leaves_completion_closed_and_empty() {
    // Important #1: ordinary typing (no `@` token) must not open the completion
    // overlay. The live re-sync gates its cwd read behind an active `@` token, so
    // with no `@` the overlay stays closed and carries no candidates.
    let (mut st, _dir) = state_with_files();
    for ch in "hello".chars() {
        handle_live_key(&mut st, &key(KeyCode::Char(ch)), 24);
    }
    assert_eq!(st.prompt_text, "hello");
    assert!(!st.completion.open, "no `@` ⇒ completion stays closed");
    assert!(
        st.completion.candidates.is_empty(),
        "no `@` ⇒ no cwd candidates were read"
    );
}

#[test]
fn focus_trap_down_moves_completion_not_history() {
    let (mut st, _dir) = state_with_files();
    st.history.push("old".into());
    handle_live_key(&mut st, &key(KeyCode::Char('@')), 24); // both files listed
    let before = st.prompt_text.clone();
    handle_live_key(&mut st, &key(KeyCode::Down), 24);
    assert_eq!(st.completion.selected, 1);
    assert_eq!(st.prompt_text, before, "Down did not recall history");
}

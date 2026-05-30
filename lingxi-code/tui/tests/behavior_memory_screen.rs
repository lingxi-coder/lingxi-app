//! M7-14 behavior: memory screen open + tier round-trip through the M3 store.

use tui::screens::memory::{
    handle_memory_key, load_tier_body, save_tier_body, MemoryAction, MemoryScreenState,
    MemoryTierEntry,
};
use tui::screens::Screen;
use tui::state::AppState;

mod support;
use support::fake_status;

#[test]
fn open_memory_sets_active_screen() {
    let mut st = AppState::new(fake_status());
    // The live `/memory` submit intercept (app.rs dispatch) calls this.
    st.open_memory();
    assert!(matches!(st.active_screen, Some(Screen::Memory(_))));
}

#[test]
fn select_edit_save_round_trip_through_store() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use tempfile::TempDir;
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("CLAUDE.md");
    let tiers = vec![MemoryTierEntry {
        label: "Project memory".into(),
        description: String::new(),
        path: path.clone(),
        exists: false,
    }];
    let mut ms = MemoryScreenState::default();
    // Enter → open editor (empty body, new file).
    handle_memory_key(
        &mut ms,
        &tiers,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
    );
    assert!(ms.editing);
    // Type "hi".
    for c in "hi".chars() {
        handle_memory_key(
            &mut ms,
            &tiers,
            KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
        );
    }
    // Ctrl-S → Save action; persist via the store fn.
    let action = handle_memory_key(
        &mut ms,
        &tiers,
        KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
    );
    match action {
        MemoryAction::Save { path: p, body } => save_tier_body(&p, &body).unwrap(),
        other => panic!("expected Save, got {other:?}"),
    }
    // The store now holds the edited body.
    assert_eq!(load_tier_body(&path).unwrap(), "hi");
}

#[test]
fn esc_in_editor_does_not_write_to_disk() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use tempfile::TempDir;
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("CLAUDE.md");
    let tiers = vec![MemoryTierEntry {
        label: "Project memory".into(),
        description: String::new(),
        path: path.clone(),
        exists: false,
    }];
    let mut ms = MemoryScreenState::default();
    handle_memory_key(
        &mut ms,
        &tiers,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
    );
    for c in "draft".chars() {
        handle_memory_key(
            &mut ms,
            &tiers,
            KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
        );
    }
    // Esc returns to the selector and NEVER produces a Save action.
    let action = handle_memory_key(
        &mut ms,
        &tiers,
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
    );
    assert_eq!(action, MemoryAction::BackToSelector);
    // No write happened: the file was never created.
    assert!(!path.exists(), "cancel must not write to the store");
}

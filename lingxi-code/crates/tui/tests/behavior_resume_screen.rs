//! M7-12 behavior tests: list renders N rows from a fixture set of
//! `SessionMetadata`; arrow select moves; Enter selects the right uuid; Esc
//! cancels; empty set → empty-state. Plus the cross-state seam (Task 9):
//! a pending permission dialog (priority 1) outranks the Resume screen
//! (priority 2), so a Down key never moves the screen's selection.
//! (Spec §3 M7-12 test row + §2.5 priority ladder.)

use std::path::PathBuf;
use std::time::{Duration, UNIX_EPOCH};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use lingxi_session::jsonl::loader::SessionMetadata;
use lingxi_tui::screens::resume::{handle_resume_key, ResumeOutcome, ResumeRow, ResumeState};
use uuid::Uuid;

fn meta(title: &str, secs: u64, count: usize, uuid: Uuid) -> SessionMetadata {
    SessionMetadata {
        uuid,
        title: title.to_string(),
        modified: UNIX_EPOCH + Duration::from_secs(secs),
        message_count: count,
        path: PathBuf::from("/tmp/x.jsonl"),
    }
}

fn fixture_rows() -> Vec<ResumeRow> {
    vec![
        ResumeRow::from_meta(&meta("alpha", 300, 5, Uuid::from_u128(1))),
        ResumeRow::from_meta(&meta("beta", 200, 2, Uuid::from_u128(2))),
        ResumeRow::from_meta(&meta("gamma", 100, 1, Uuid::from_u128(3))),
    ]
}

fn k(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

#[test]
fn list_renders_three_rows() {
    let st = ResumeState::new(fixture_rows());
    assert_eq!(st.rows.len(), 3);
    assert_eq!(st.selected, 0);
}

#[test]
fn arrow_select_moves_then_enter_picks_right_uuid() {
    let mut st = ResumeState::new(fixture_rows());
    // Down twice → row index 2 (gamma, uuid 3).
    assert_eq!(
        handle_resume_key(&mut st, k(KeyCode::Down)),
        ResumeOutcome::Stay
    );
    assert_eq!(
        handle_resume_key(&mut st, k(KeyCode::Down)),
        ResumeOutcome::Stay
    );
    assert_eq!(st.selected, 2);
    assert_eq!(
        handle_resume_key(&mut st, k(KeyCode::Enter)),
        ResumeOutcome::Resume(Uuid::from_u128(3))
    );
}

#[test]
fn esc_cancels_without_resume() {
    let mut st = ResumeState::new(fixture_rows());
    assert_eq!(
        handle_resume_key(&mut st, k(KeyCode::Esc)),
        ResumeOutcome::Cancel
    );
}

#[test]
fn empty_set_is_empty_state() {
    let st = ResumeState::new(vec![]);
    assert!(st.is_empty());
    assert_eq!(st.selected_uuid(), None);
}

// --- Task 9: cross-state seam — permission focus-trap outranks the screen. ---

use lingxi_permission::gate::PermissionRequest;
use lingxi_tui::root::handle_live_key;
use lingxi_tui::screens::Screen;
use lingxi_tui::state::{AppState, PendingPermission, StatusSnapshot};

fn iocraft_down() -> iocraft::KeyEvent {
    iocraft::KeyEvent::new(iocraft::KeyEventKind::Press, iocraft::KeyCode::Down)
}

#[test]
fn permission_dialog_outranks_resume_screen() {
    let st_screen = ResumeState::new(fixture_rows());
    let mut app = AppState::new(StatusSnapshot::default());
    app.active_screen = Some(Screen::Resume(st_screen));
    // A permission dialog is also pending (priority 1).
    app.pending_permission = Some(PendingPermission {
        request: PermissionRequest::BypassPermissionsMode,
    });

    // Down arrow: priority 1 (permission) consumes it; the Resume screen's
    // selection must NOT move.
    handle_live_key(&mut app, &iocraft_down(), 24);

    if let Some(Screen::Resume(s)) = &app.active_screen {
        assert_eq!(s.selected, 0, "permission must outrank the screen");
    } else {
        panic!("resume screen should still be open");
    }
}

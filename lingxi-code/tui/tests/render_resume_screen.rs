//! M7-12 snapshot: Resume screen with 3 sessions + preview, and the
//! empty-state. Locks the rendered frame (insta) + substring asserts to
//! survive snapshot-file corruption (matches `snapshot_permission_dialogs.rs`).

use std::path::PathBuf;
use std::time::{Duration, UNIX_EPOCH};

use iocraft::prelude::*;
use session::jsonl::loader::SessionMetadata;
use tui::screens::resume::{ResumeRow, ResumeScreen, ResumeState};
use uuid::Uuid;

fn meta(title: &str, secs: u64, count: usize) -> SessionMetadata {
    SessionMetadata {
        uuid: Uuid::nil(),
        title: title.to_string(),
        modified: UNIX_EPOCH + Duration::from_secs(secs),
        created: UNIX_EPOCH + Duration::from_secs(secs),
        message_count: count,
        path: PathBuf::from("/tmp/x.jsonl"),
    }
}

#[test]
fn snapshot_resume_three_sessions_with_preview() {
    // Fixed `now` (UNIX_EPOCH + 900s) for deterministic relative-time metadata.
    let now = UNIX_EPOCH + Duration::from_secs(900);
    let st = ResumeState::new(vec![
        ResumeRow::from_meta_at(&meta("first session", 300, 5), now),
        ResumeRow::from_meta_at(&meta("second session", 200, 2), now),
        ResumeRow::from_meta_at(&meta("third session", 100, 1), now),
    ]);
    let mut element = element! { ResumeScreen(state: st) };
    let frame = element.to_string();
    insta::assert_snapshot!("resume_three_sessions_with_preview", &frame);
    assert!(frame.contains("Resume which session?"), "got: {frame}");
    assert!(frame.contains("> first session"), "got: {frame}");
    assert!(frame.contains("10 minutes ago \u{00b7} 5 messages"), "got: {frame}");
    // Third session's metadata uses the singular "1 message" (no parens).
    assert!(frame.contains("13 minutes ago \u{00b7} 1 message"), "got: {frame}");
    // Preview pane (selected row) still shows the parenthesized count + title.
    assert!(frame.contains("(5 messages)"), "got: {frame}");
    assert!(frame.contains("Title:    first session"), "got: {frame}");
}

#[test]
fn snapshot_resume_empty_state() {
    let st = ResumeState::new(vec![]);
    let mut element = element! { ResumeScreen(state: st) };
    let frame = element.to_string();
    insta::assert_snapshot!("resume_empty_state", &frame);
    assert!(
        frame.contains("No conversations found to resume."),
        "got: {frame}"
    );
}

//! Resume screen (M7-12) — an iocraft full-page view over the M5-08 session
//! loader (`session::jsonl::loader`). Lists recent sessions, previews
//! the selected one, resumes on Enter, cancels on Esc.
//!
//! Logic/state (`ResumeRow`, `ResumeState`, `handle_resume_key`) are pure and
//! terminal-free; `ResumeScreen` is a thin render over `ResumeState`. Mirrors
//! the M6-05 permission-dialog split.
//!
//! Literals are byte-locked from claude-code `ResumeConversation.tsx` and
//! `LogSelector.tsx`. The empty state renders two lines:
//! `"No conversations found to resume."` then (dim)
//! `"Press Ctrl+C to exit and start a new conversation."`. The header
//! `"Resume which session?"` is shared with the M5-08 stdio picker.
//!
//! claude-code's transient `"Loading conversations…"` / `"Resuming
//! conversation…"` literals are NOT rendered here: M7-12 loads sessions
//! synchronously before mount and hands control back to the CLI on Enter, so
//! neither frame exists — they are documented, not dead-coded.
#![forbid(unsafe_code)]

use crossterm::event::{KeyCode, KeyEvent};
use iocraft::prelude::*;
use session::jsonl::loader::{format_rfc3339_seconds, SessionMetadata};
use uuid::Uuid;

/// One display row derived from a [`SessionMetadata`]. Terminal-free.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeRow {
    /// Session UUID — the value Enter resolves to.
    pub uuid: Uuid,
    /// Title (already truncated to ≤ 50 chars + ellipsis by the loader).
    pub title: String,
    /// `[YYYY-MM-DDTHH:MM:SSZ]`-style timestamp body (without the brackets).
    pub modified_label: String,
    /// `(N message[s])` count label, singular for 1.
    pub count_label: String,
}

impl ResumeRow {
    /// Build a display row from a loader [`SessionMetadata`].
    ///
    /// The timestamp is formatted via the shared
    /// [`session::jsonl::loader::format_rfc3339_seconds`] — the very same
    /// function the M5-08 stdio picker uses — so this screen and the picker
    /// render timestamps byte-for-byte identically (no hand-copied formatter).
    #[must_use]
    pub fn from_meta(m: &SessionMetadata) -> Self {
        Self {
            uuid: m.uuid,
            title: m.title.clone(),
            modified_label: format_rfc3339_seconds(m.modified),
            count_label: if m.message_count == 1 {
                "(1 message)".to_string()
            } else {
                format!("({} messages)", m.message_count)
            },
        }
    }
}

/// Pure state for the Resume screen: the rows plus the selected index.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResumeState {
    /// Display rows, newest-first (the loader already sorts mtime desc).
    pub rows: Vec<ResumeRow>,
    /// Index into `rows` of the highlighted row. Always `< rows.len()` when
    /// `rows` is non-empty; meaningless (0) when empty.
    pub selected: usize,
}

impl ResumeState {
    /// Build from display rows. Selects the first row.
    #[must_use]
    pub fn new(rows: Vec<ResumeRow>) -> Self {
        Self { rows, selected: 0 }
    }

    /// `true` when there are no sessions to resume (empty-state).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The currently selected row, if any.
    #[must_use]
    pub fn selected_row(&self) -> Option<&ResumeRow> {
        self.rows.get(self.selected)
    }

    /// The UUID of the selected row — the value Enter resolves to.
    #[must_use]
    pub fn selected_uuid(&self) -> Option<Uuid> {
        self.selected_row().map(|r| r.uuid)
    }
}

/// What `handle_resume_key` tells the router to do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeOutcome {
    /// Keep the screen open (selection moved, or an inert key).
    Stay,
    /// Resume the session with this UUID (Enter on a real row).
    Resume(Uuid),
    /// Close the screen, return to REPL (Esc, or Enter on empty-state).
    Cancel,
}

/// Pure key handler for the Resume screen.
///
/// - `Up`/`k` → move selection up (clamped at 0).
/// - `Down`/`j` → move selection down (clamped at `len-1`).
/// - `Enter` → resume the selected uuid; on empty-state → `Cancel`.
/// - `Esc` / `q` → cancel.
/// - anything else → `Stay`.
#[must_use]
pub fn handle_resume_key(state: &mut ResumeState, key: KeyEvent) -> ResumeOutcome {
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => {
            state.selected = state.selected.saturating_sub(1);
            ResumeOutcome::Stay
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if !state.rows.is_empty() {
                state.selected = (state.selected + 1).min(state.rows.len() - 1);
            }
            ResumeOutcome::Stay
        }
        KeyCode::Enter => match state.selected_uuid() {
            Some(uuid) => ResumeOutcome::Resume(uuid),
            None => ResumeOutcome::Cancel,
        },
        KeyCode::Esc | KeyCode::Char('q') => ResumeOutcome::Cancel,
        _ => ResumeOutcome::Stay,
    }
}

/// Props for [`ResumeScreen`].
#[derive(Default, Props)]
pub struct ResumeScreenProps {
    /// The screen state (rows + selection). Cloned from `active_screen`.
    pub state: ResumeState,
}

/// iocraft component: header, the session list (or empty-state), and a
/// preview pane for the selected row. Footer hint matches the key handler.
#[component]
pub fn ResumeScreen(props: &ResumeScreenProps) -> impl Into<AnyElement<'static>> {
    let state = props.state.clone();

    if state.is_empty() {
        return element! {
            View(flex_direction: FlexDirection::Column, padding: 1) {
                Text(content: "No conversations found to resume.".to_string())
                Text(
                    content: "Press Ctrl+C to exit and start a new conversation.".to_string(),
                    color: Color::DarkGrey,
                )
            }
        }
        .into_any();
    }

    let header = "Resume which session?".to_string();
    let selected = state.selected;
    // Build one Text per row: "> N. <title>  [<modified>]  (<count>)".
    let row_lines: Vec<String> = state
        .rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let marker = if i == selected { "> " } else { "  " };
            format!(
                "{marker}{}. {}  [{}]  {}",
                i + 1,
                r.title,
                r.modified_label,
                r.count_label
            )
        })
        .collect();

    // Preview pane: title + uuid + count of the selected row (from metadata
    // already in hand — no extra file read, no engine change).
    let preview: Vec<String> = state
        .selected_row()
        .map(|r| {
            vec![
                format!("Title:    {}", r.title),
                format!("Session:  {}", r.uuid),
                format!("Messages: {}", r.count_label),
                format!("Modified: {}", r.modified_label),
            ]
        })
        .unwrap_or_default();

    let footer = "Up/Down select   Enter resume   Esc cancel".to_string();

    element! {
        View(flex_direction: FlexDirection::Column, padding: 1) {
            Text(content: header)
            View(flex_direction: FlexDirection::Column, padding_top: 1) {
                #(row_lines.into_iter().map(|line| element! {
                    Text(content: line)
                }))
            }
            View(
                flex_direction: FlexDirection::Column,
                border_style: BorderStyle::Round,
                padding: 1,
                margin_top: 1,
            ) {
                #(preview.into_iter().map(|line| element! {
                    Text(content: line)
                }))
            }
            View(margin_top: 1) {
                Text(content: footer, color: Color::DarkGrey)
            }
        }
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    fn meta(title: &str, secs: u64, count: usize) -> SessionMetadata {
        SessionMetadata {
            uuid: Uuid::nil(),
            title: title.to_string(),
            modified: UNIX_EPOCH + Duration::from_secs(secs),
            message_count: count,
            path: std::path::PathBuf::from("/tmp/x.jsonl"),
        }
    }

    #[test]
    fn row_formats_timestamp_and_singular_plural() {
        // 1748113392 = 2025-05-24T19:03:12Z (an arbitrary fixed epoch second).
        let row_one = ResumeRow::from_meta(&meta("hello", 1_748_113_392, 1));
        assert_eq!(row_one.modified_label, "2025-05-24T19:03:12Z");
        assert_eq!(row_one.count_label, "(1 message)");

        let row_many = ResumeRow::from_meta(&meta("hi", 1_748_113_392, 12));
        assert_eq!(row_many.count_label, "(12 messages)");
    }

    #[test]
    fn row_keeps_uuid_and_title() {
        let m = meta("fix the bug", 0, 3);
        let row = ResumeRow::from_meta(&m);
        assert_eq!(row.title, "fix the bug");
        assert_eq!(row.uuid, m.uuid);
    }

    #[test]
    fn state_from_rows_selects_first() {
        let rows = vec![
            ResumeRow::from_meta(&meta("a", 0, 1)),
            ResumeRow::from_meta(&meta("b", 0, 2)),
        ];
        let st = ResumeState::new(rows);
        assert_eq!(st.selected, 0);
        assert_eq!(st.selected_uuid(), Some(Uuid::nil()));
        assert!(!st.is_empty());
    }

    #[test]
    fn empty_state_has_no_selection() {
        let st = ResumeState::new(vec![]);
        assert!(st.is_empty());
        assert_eq!(st.selected_uuid(), None);
    }

    use crossterm::event::KeyModifiers;

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn down_moves_selection_clamped() {
        let mut st = ResumeState::new(vec![
            ResumeRow::from_meta(&meta("a", 0, 1)),
            ResumeRow::from_meta(&meta("b", 0, 1)),
        ]);
        assert_eq!(
            handle_resume_key(&mut st, k(KeyCode::Down)),
            ResumeOutcome::Stay
        );
        assert_eq!(st.selected, 1);
        // Past the end stays on the last row (no wrap).
        assert_eq!(
            handle_resume_key(&mut st, k(KeyCode::Down)),
            ResumeOutcome::Stay
        );
        assert_eq!(st.selected, 1);
    }

    #[test]
    fn up_moves_selection_clamped() {
        let mut st = ResumeState::new(vec![
            ResumeRow::from_meta(&meta("a", 0, 1)),
            ResumeRow::from_meta(&meta("b", 0, 1)),
        ]);
        st.selected = 1;
        assert_eq!(
            handle_resume_key(&mut st, k(KeyCode::Up)),
            ResumeOutcome::Stay
        );
        assert_eq!(st.selected, 0);
        // Past the start stays on the first row.
        assert_eq!(
            handle_resume_key(&mut st, k(KeyCode::Up)),
            ResumeOutcome::Stay
        );
        assert_eq!(st.selected, 0);
    }

    #[test]
    fn enter_resumes_selected_uuid() {
        let target = Uuid::from_u128(42);
        let mut second = meta("b", 0, 1);
        second.uuid = target;
        let mut st = ResumeState::new(vec![
            ResumeRow::from_meta(&meta("a", 0, 1)),
            ResumeRow::from_meta(&second),
        ]);
        let _ = handle_resume_key(&mut st, k(KeyCode::Down));
        assert_eq!(
            handle_resume_key(&mut st, k(KeyCode::Enter)),
            ResumeOutcome::Resume(target)
        );
    }

    #[test]
    fn esc_cancels() {
        let mut st = ResumeState::new(vec![ResumeRow::from_meta(&meta("a", 0, 1))]);
        assert_eq!(
            handle_resume_key(&mut st, k(KeyCode::Esc)),
            ResumeOutcome::Cancel
        );
    }

    #[test]
    fn enter_on_empty_cancels() {
        let mut st = ResumeState::new(vec![]);
        assert_eq!(
            handle_resume_key(&mut st, k(KeyCode::Enter)),
            ResumeOutcome::Cancel
        );
    }

    #[test]
    fn component_renders_rows_and_selection_marker() {
        let st = ResumeState::new(vec![
            ResumeRow::from_meta(&meta("first session", 1_748_113_392, 3)),
            ResumeRow::from_meta(&meta("second session", 1_748_113_300, 1)),
        ]);
        let mut element = element! { ResumeScreen(state: st) };
        let frame = element.to_string();
        assert!(frame.contains("Resume which session?"), "got: {frame}");
        assert!(frame.contains("first session"), "got: {frame}");
        assert!(frame.contains("(3 messages)"), "got: {frame}");
        assert!(frame.contains("(1 message)"), "got: {frame}");
        // Selected (row 0) prefixed "> ", unselected "  ".
        assert!(frame.contains("> 1."), "got: {frame}");
        assert!(frame.contains("  2."), "got: {frame}");
    }

    #[test]
    fn component_renders_empty_state() {
        let st = ResumeState::new(vec![]);
        let mut element = element! { ResumeScreen(state: st) };
        let frame = element.to_string();
        assert!(
            frame.contains("No conversations found to resume."),
            "got: {frame}"
        );
        assert!(
            frame.contains("Press Ctrl+C to exit and start a new conversation."),
            "got: {frame}"
        );
    }

    #[test]
    fn screen_routing_enter_clears_active_screen_with_resume_request() {
        use crate::screens::Screen;
        use crate::state::{AppState, StatusSnapshot};
        let target = Uuid::from_u128(7);
        let mut row = meta("only", 0, 1);
        row.uuid = target;
        let st_screen = ResumeState::new(vec![ResumeRow::from_meta(&row)]);

        let mut app = AppState::new(StatusSnapshot::default());
        app.active_screen = Some(Screen::Resume(st_screen));

        // Enter should request a resume and close the screen.
        crate::root::handle_live_key(&mut app, &iocraft_enter(), 24);
        assert!(app.active_screen.is_none(), "screen should close on Enter");
        assert_eq!(app.resume_request, Some(target));
    }

    #[test]
    fn screen_routing_esc_clears_active_screen_no_request() {
        use crate::screens::Screen;
        use crate::state::{AppState, StatusSnapshot};
        let st_screen = ResumeState::new(vec![ResumeRow::from_meta(&meta("only", 0, 1))]);
        let mut app = AppState::new(StatusSnapshot::default());
        app.active_screen = Some(Screen::Resume(st_screen));

        crate::root::handle_live_key(&mut app, &iocraft_esc(), 24);
        assert!(app.active_screen.is_none(), "screen should close on Esc");
        assert_eq!(app.resume_request, None);
    }

    // Build the iocraft (crossterm-0.29) KeyEvents the live path delivers.
    fn iocraft_enter() -> iocraft::KeyEvent {
        iocraft::KeyEvent::new(iocraft::KeyEventKind::Press, iocraft::KeyCode::Enter)
    }
    fn iocraft_esc() -> iocraft::KeyEvent {
        iocraft::KeyEvent::new(iocraft::KeyEventKind::Press, iocraft::KeyCode::Esc)
    }
}

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

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
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
    /// (Retained for the detail/preview surface.)
    pub modified_label: String,
    /// `(N message[s])` count label, singular for 1.
    pub count_label: String,
    /// (resume-metadata) Dim metadata line shown under the title:
    /// `<relative time ago> · <N> messages` (claude-code `formatLogMetadata`,
    /// joined with ` · `, no brackets/parens). Git branch is omitted (not
    /// carried on the session metadata).
    pub metadata_label: String,
}

/// Relative-time-ago string (claude-code `formatRelativeTimeAgo` with
/// `numeric:'always'`, long English units): `5 minutes ago`, `3 days ago`,
/// `in 2 hours` (future). Largest matching unit wins.
#[must_use]
pub fn relative_time_ago(modified: std::time::SystemTime, now: std::time::SystemTime) -> String {
    // Positive => in the past.
    let diff: i64 = match now.duration_since(modified) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => -(e.duration().as_secs() as i64),
    };
    const INTERVALS: &[(&str, i64)] = &[
        ("year", 31_536_000),
        ("month", 2_592_000),
        ("week", 604_800),
        ("day", 86_400),
        ("hour", 3_600),
        ("minute", 60),
        ("second", 1),
    ];
    for &(unit, secs) in INTERVALS {
        if diff.abs() >= secs {
            let value = diff / secs; // truncates toward zero
            let n = value.abs();
            let unit_str = if n == 1 {
                unit.to_string()
            } else {
                format!("{unit}s")
            };
            return if value >= 0 {
                format!("{n} {unit_str} ago")
            } else {
                format!("in {n} {unit_str}")
            };
        }
    }
    "0 seconds ago".to_string()
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
        Self::from_meta_at(m, std::time::SystemTime::now())
    }

    /// [`from_meta`](Self::from_meta) with an injected `now` for deterministic
    /// relative-time tests.
    #[must_use]
    pub fn from_meta_at(m: &SessionMetadata, now: std::time::SystemTime) -> Self {
        let msgs = if m.message_count == 1 {
            "1 message".to_string()
        } else {
            format!("{} messages", m.message_count)
        };
        Self {
            uuid: m.uuid,
            title: m.title.clone(),
            modified_label: format_rfc3339_seconds(m.modified),
            count_label: if m.message_count == 1 {
                "(1 message)".to_string()
            } else {
                format!("({} messages)", m.message_count)
            },
            metadata_label: format!("{} \u{00b7} {}", relative_time_ago(m.modified, now), msgs),
        }
    }
}

/// Pure state for the Resume screen: the rows plus the selected index.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResumeState {
    /// Display rows, newest-first (the loader already sorts mtime desc).
    pub rows: Vec<ResumeRow>,
    /// Index into the FILTERED rows of the highlighted row.
    pub selected: usize,
    /// (resume-old-form) Type-to-search query — filters rows by title
    /// (case-insensitive substring). Empty = show all (claude-code
    /// `LogSelector` search box).
    pub query: String,
    /// (resume-old-form-vs-logselector) `true` while the `/`-activated search
    /// box is focused — claude-code `LogSelector` `viewMode === "search"`
    /// (vs the default `"list"` browse mode). Gates the search-box line, the
    /// overflow counter (hidden in search mode), and the footer wording.
    pub in_search_mode: bool,
}

/// (resume-old-form-vs-logselector) Conservative lower bound on how many rows
/// fit in a typical viewport. The overflow `(idx of N)` counter is shown only
/// when the filtered list is longer than this — mirroring claude-code's
/// `displayedLogs.length > visibleCount` gate (whose `visibleCount` derives
/// from terminal height; LingXi has no height in props here, so a fixed
/// lower bound is used).
pub const VISIBLE_ROWS: usize = 10;

impl ResumeState {
    /// Build from display rows. Selects the first row.
    #[must_use]
    pub fn new(rows: Vec<ResumeRow>) -> Self {
        Self {
            rows,
            selected: 0,
            query: String::new(),
            in_search_mode: false,
        }
    }

    /// `true` when there are no sessions to resume at all (empty-state).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Rows visible under the current [`query`](Self::query) (all rows when the
    /// query is empty), case-insensitive title-substring match.
    #[must_use]
    pub fn filtered(&self) -> Vec<&ResumeRow> {
        if self.query.is_empty() {
            self.rows.iter().collect()
        } else {
            let q = self.query.to_lowercase();
            self.rows
                .iter()
                .filter(|r| r.title.to_lowercase().contains(&q))
                .collect()
        }
    }

    /// The currently selected (filtered) row, if any.
    #[must_use]
    pub fn selected_row(&self) -> Option<&ResumeRow> {
        self.filtered().get(self.selected).copied()
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
    let n = state.filtered().len();
    match key.code {
        // (resume-old-form) Arrows navigate; j/k/q no longer have special
        // meaning — they type into the search query (claude-code LogSelector is
        // not vim-modal).
        KeyCode::Up => {
            state.selected = state.selected.saturating_sub(1);
            ResumeOutcome::Stay
        }
        KeyCode::Down => {
            if n > 0 {
                state.selected = (state.selected + 1).min(n - 1);
            }
            ResumeOutcome::Stay
        }
        KeyCode::Enter => match state.selected_uuid() {
            Some(uuid) => ResumeOutcome::Resume(uuid),
            None => ResumeOutcome::Cancel,
        },
        // (resume-old-form-vs-logselector) Esc behaviour mirrors the
        // `LogSelector` search box: in search mode the first Esc clears the
        // query and drops back to the list (`viewMode "search" -> "list"`);
        // once the query is empty (or in plain list mode) Esc cancels.
        KeyCode::Esc => {
            if state.query.is_empty() {
                if state.in_search_mode {
                    state.in_search_mode = false;
                    ResumeOutcome::Stay
                } else {
                    ResumeOutcome::Cancel
                }
            } else {
                state.query.clear();
                state.selected = 0;
                state.in_search_mode = false;
                ResumeOutcome::Stay
            }
        }
        KeyCode::Backspace => {
            state.query.pop();
            state.selected = 0;
            ResumeOutcome::Stay
        }
        // (resume-old-form-vs-logselector) `/` activates the search box when in
        // list mode (claude-code `enterSearchMode`); inside search mode it is a
        // literal character typed into the query like any other printable char.
        KeyCode::Char('/')
            if !state.in_search_mode && key.modifiers == KeyModifiers::NONE =>
        {
            state.in_search_mode = true;
            ResumeOutcome::Stay
        }
        // Type-to-search: printable chars filter the list.
        KeyCode::Char(c)
            if key.modifiers == KeyModifiers::NONE || key.modifiers == KeyModifiers::SHIFT =>
        {
            state.query.push(c);
            state.selected = 0;
            ResumeOutcome::Stay
        }
        _ => ResumeOutcome::Stay,
    }
}

/// Props for [`ResumeScreen`].
#[derive(Default, Props)]
pub struct ResumeScreenProps {
    /// The screen state (rows + selection). Cloned from `active_screen`.
    pub state: ResumeState,
}

/// iocraft component: bold suggestion-colored "Resume Session" header (with an
/// overflow `(idx of N)` counter), an optional `/`-activated search box, the
/// session list (title + dim metadata line per row), and a dim Byline footer.
/// No always-on preview pane (resume-preview-pane-not-in-shipped).
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

    // (resume-old-form) claude-code `LogSelector` header is the bold
    // "Resume Session" title (not the stdio picker's "Resume which session?").
    let selected = state.selected;
    let filtered = state.filtered();
    let n = filtered.len();
    // (resume-old-form-vs-logselector) Bold suggestion-colored "Resume Session"
    // header. The "(idx of N)" position counter is appended (dim) ONLY in list
    // mode when the list overflows the viewport — claude-code renders it under
    // `viewMode === "list" && displayedLogs.length > visibleCount`. The index
    // is 1-based (claude-code `focusedIndex` starts at 1).
    let show_counter = !state.in_search_mode && n > VISIBLE_ROWS;
    let counter = show_counter.then(|| format!(" ({} of {})", selected + 1, n));
    // (resume-old-form-vs-logselector) `/`-activated search box, rendered only
    // while in search mode (claude-code `SearchBox`, `viewMode === "search"`).
    let search_line = state
        .in_search_mode
        .then(|| format!("Search: {}", state.query));
    // (resume-metadata) Each row is a title line + a dim metadata line below it
    // (`<relative time> · <N> messages`, paddingLeft 2). Rows are filtered by
    // the search query.
    let row_lines: Vec<(String, String)> = filtered
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let marker = if i == selected { "> " } else { "  " };
            (
                format!("{marker}{}", r.title),
                format!("  {}", r.metadata_label),
            )
        })
        .collect();

    // (resume-preview-pane-not-in-shipped) No always-on preview pane.
    // (resume-old-form-vs-logselector) Dim Byline footer, context-dependent on
    // the mode. claude-code's search footer reads `Type to Search · Enter
    // select · Esc clear`; the list footer carries the `Type to search · Esc
    // cancel` verbs (Ctrl+V preview / Ctrl+R rename hints land with those
    // features).
    let footer = if state.in_search_mode {
        "Type to Search \u{00B7} Enter select \u{00B7} Esc clear".to_string()
    } else {
        "Type to search \u{00B7} Esc cancel".to_string()
    };

    element! {
        View(flex_direction: FlexDirection::Column, padding: 1) {
            View(flex_direction: FlexDirection::Row) {
                Text(content: "Resume Session".to_string(), weight: Weight::Bold, color: Color::Blue)
                #(counter.map(|c| element! {
                    Text(content: c, color: Color::DarkGrey)
                }))
            }
            #(search_line.map(|s| element! {
                View(flex_direction: FlexDirection::Row) {
                    Text(content: s, color: Color::DarkGrey)
                }
            }))
            View(flex_direction: FlexDirection::Column, padding_top: 1) {
                #(row_lines.into_iter().map(|(title, meta)| element! {
                    View(flex_direction: FlexDirection::Column) {
                        Text(content: title)
                        Text(content: meta, color: Color::DarkGrey)
                    }
                }))
            }
            View(margin_top: 1) {
                Text(content: footer, color: Color::DarkGrey, italic: true)
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
            created: UNIX_EPOCH + Duration::from_secs(secs),
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
    fn type_to_search_filters_and_esc_clears_then_cancels() {
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        let mut st = ResumeState::new(vec![
            ResumeRow::from_meta_at(&meta("alpha project", 100, 1), now),
            ResumeRow::from_meta_at(&meta("beta project", 200, 1), now),
            ResumeRow::from_meta_at(&meta("alphabet", 300, 1), now),
        ]);
        // Type "alph" → filters to the two "alph…" titles.
        for c in "alph".chars() {
            assert_eq!(handle_resume_key(&mut st, k(KeyCode::Char(c))), ResumeOutcome::Stay);
        }
        assert_eq!(st.query, "alph");
        assert_eq!(st.filtered().len(), 2);
        // Backspace shrinks the query.
        let _ = handle_resume_key(&mut st, k(KeyCode::Backspace));
        assert_eq!(st.query, "alp");
        // Esc clears the query first (Stay), then a second Esc cancels.
        assert_eq!(handle_resume_key(&mut st, k(KeyCode::Esc)), ResumeOutcome::Stay);
        assert!(st.query.is_empty());
        assert_eq!(st.filtered().len(), 3);
        assert_eq!(handle_resume_key(&mut st, k(KeyCode::Esc)), ResumeOutcome::Cancel);
    }

    #[test]
    fn slash_activates_search_mode() {
        let mut st = ResumeState::new(vec![ResumeRow::from_meta(&meta("a", 0, 1))]);
        assert!(!st.in_search_mode);
        assert_eq!(
            handle_resume_key(&mut st, k(KeyCode::Char('/'))),
            ResumeOutcome::Stay
        );
        assert!(st.in_search_mode);
        // `/` is consumed by activation, not typed into the query.
        assert!(st.query.is_empty());
        // Subsequent chars type into the query.
        let _ = handle_resume_key(&mut st, k(KeyCode::Char('x')));
        assert_eq!(st.query, "x");
    }

    #[test]
    fn slash_in_search_mode_is_literal_char_not_reactivation() {
        let mut st = ResumeState::new(vec![ResumeRow::from_meta(&meta("a", 0, 1))]);
        st.in_search_mode = true;
        let _ = handle_resume_key(&mut st, k(KeyCode::Char('/')));
        // Already in search mode: `/` is a literal query char.
        assert_eq!(st.query, "/");
        assert!(st.in_search_mode);
    }

    #[test]
    fn search_mode_esc_clears_query_then_exits_then_cancels() {
        let mut st = ResumeState::new(vec![
            ResumeRow::from_meta(&meta("alpha", 0, 1)),
            ResumeRow::from_meta(&meta("beta", 0, 1)),
        ]);
        st.in_search_mode = true;
        st.query = "alp".to_string();
        // First Esc clears the query AND drops back to list mode (Stay).
        assert_eq!(
            handle_resume_key(&mut st, k(KeyCode::Esc)),
            ResumeOutcome::Stay
        );
        assert!(st.query.is_empty());
        assert!(!st.in_search_mode);
        // Second Esc (now plain list mode, empty query) cancels.
        assert_eq!(
            handle_resume_key(&mut st, k(KeyCode::Esc)),
            ResumeOutcome::Cancel
        );
    }

    #[test]
    fn empty_query_in_search_mode_esc_exits_to_list_not_cancel() {
        let mut st = ResumeState::new(vec![ResumeRow::from_meta(&meta("a", 0, 1))]);
        st.in_search_mode = true;
        // Empty query but in search mode: Esc exits search mode (Stay), not Cancel.
        assert_eq!(
            handle_resume_key(&mut st, k(KeyCode::Esc)),
            ResumeOutcome::Stay
        );
        assert!(!st.in_search_mode);
    }

    #[test]
    fn header_renders_counter_only_when_overflow() {
        // Few rows (<= VISIBLE_ROWS): no counter.
        let few = ResumeState::new(
            (0..3)
                .map(|i| ResumeRow::from_meta(&meta(&format!("s{i}"), 0, 1)))
                .collect(),
        );
        let mut el = element! { ResumeScreen(state: few) };
        let frame = el.to_string();
        assert!(frame.contains("Resume Session"), "got: {frame}");
        assert!(!frame.contains(" of "), "no counter expected, got: {frame}");

        // Many rows (> VISIBLE_ROWS): counter "(1 of N)".
        let n = VISIBLE_ROWS + 5;
        let many = ResumeState::new(
            (0..n)
                .map(|i| ResumeRow::from_meta(&meta(&format!("s{i}"), 0, 1)))
                .collect(),
        );
        let mut el = element! { ResumeScreen(state: many) };
        let frame = el.to_string();
        assert!(
            frame.contains(&format!("(1 of {n})")),
            "counter expected, got: {frame}"
        );
    }

    #[test]
    fn search_mode_renders_search_box_line() {
        let n = VISIBLE_ROWS + 5;
        let mut st = ResumeState::new(
            (0..n)
                .map(|i| ResumeRow::from_meta(&meta(&format!("s{i}"), 0, 1)))
                .collect(),
        );
        st.in_search_mode = true;
        st.query = "s1".to_string();
        let mut el = element! { ResumeScreen(state: st) };
        let frame = el.to_string();
        assert!(frame.contains("Search: s1"), "got: {frame}");
        // In search mode the overflow counter is suppressed.
        assert!(!frame.contains(" of "), "no counter in search mode, got: {frame}");
        // Search-mode footer wording.
        assert!(frame.contains("Type to Search"), "got: {frame}");
        assert!(frame.contains("Esc clear"), "got: {frame}");
    }

    #[test]
    fn list_mode_has_no_search_box_and_cancel_footer() {
        let st = ResumeState::new(vec![ResumeRow::from_meta(&meta("a", 0, 1))]);
        let mut el = element! { ResumeScreen(state: st) };
        let frame = el.to_string();
        assert!(!frame.contains("Search:"), "no search box in list mode, got: {frame}");
        assert!(frame.contains("Type to search \u{00b7} Esc cancel"), "got: {frame}");
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
    fn relative_time_ago_units() {
        let base = UNIX_EPOCH + Duration::from_secs(1_000_000);
        let ago = |secs: u64| relative_time_ago(base - Duration::from_secs(secs), base);
        assert_eq!(ago(30), "30 seconds ago");
        assert_eq!(ago(60), "1 minute ago");
        assert_eq!(ago(300), "5 minutes ago");
        assert_eq!(ago(3600), "1 hour ago");
        assert_eq!(ago(86_400), "1 day ago");
        assert_eq!(ago(2 * 86_400), "2 days ago");
        // Future.
        assert_eq!(
            relative_time_ago(base + Duration::from_secs(120), base),
            "in 2 minutes"
        );
    }

    #[test]
    fn component_renders_rows_and_selection_marker() {
        // Fixed `now` (5 min after the first session) for deterministic
        // relative-time metadata.
        let now = UNIX_EPOCH + Duration::from_secs(1_748_113_392 + 300);
        let st = ResumeState::new(vec![
            ResumeRow::from_meta_at(&meta("first session", 1_748_113_392, 3), now),
            ResumeRow::from_meta_at(&meta("second session", 1_748_113_300, 1), now),
        ]);
        let mut element = element! { ResumeScreen(state: st) };
        let frame = element.to_string();
        assert!(frame.contains("Resume Session"), "got: {frame}");
        assert!(frame.contains("first session"), "got: {frame}");
        // (resume-metadata) Dim metadata line: "<relative> · <N> messages".
        assert!(frame.contains("5 minutes ago \u{00b7} 3 messages"), "got: {frame}");
        assert!(frame.contains("1 message"), "got: {frame}");
        // Selected (row 0) prefixed "> ", unselected "  ".
        assert!(frame.contains("> first session"), "got: {frame}");
        assert!(frame.contains("  second session"), "got: {frame}");
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

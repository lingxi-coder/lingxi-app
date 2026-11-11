//! Resume picker (iocraft → ratatui port). A standalone full-screen list of
//! recent sessions: arrows navigate, type-to-search filters, Enter resumes the
//! selected uuid, Esc cancels. Rendered on the alternate screen via
//! [`crate::setup_terminal`] — distinct from the chat app's bottom-anchored
//! terminal, the same pattern the `agents_screen` full-screen view uses.
//!
//! The state + key handling (`ResumeState`, `handle_resume_key`) are pure and
//! terminal-free; only [`run_resume_picker`] touches the terminal. Literals are
//! byte-locked from claude-code `LogSelector.tsx` / `ResumeConversation.tsx`.
//! The `SessionMetadata → ResumeRow` conversion stays in the caller (cli) so
//! this crate does not depend on the `session` loader; callers build the dim
//! metadata line with [`relative_time_ago`].

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use tui_core::theme::{theme_for, Theme};
use uuid::Uuid;

/// One display row. Terminal-free; the caller maps a session-loader
/// `SessionMetadata` into this lean shape (title + prebuilt dim metadata line).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeRow {
    /// Session UUID — the value Enter resolves to.
    pub uuid: Uuid,
    /// Exact transcript displayed for this row, retained through selection.
    pub transcript_path: std::path::PathBuf,
    /// Title (already truncated to ≤ 50 chars + ellipsis by the loader).
    pub title: String,
    /// Dim metadata line shown under the title: `<relative time ago> · <N>
    /// messages` (claude-code `formatLogMetadata`). Built by the caller.
    pub metadata_label: String,
}

/// The selected UUID and its exact displayed source travel together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeSelection {
    pub uuid: Uuid,
    pub transcript_path: std::path::PathBuf,
}

/// Relative-time-ago string (claude-code `formatRelativeTimeAgo` with
/// `numeric:'always'`, long English units): `5 minutes ago`, `3 days ago`,
/// `in 2 hours` (future). Largest matching unit wins. Kept here (pure) so the
/// caller can build [`ResumeRow::metadata_label`] identically to the old path.
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

/// Conservative lower bound on how many rows fit in a typical viewport. The
/// overflow `(idx of N)` counter is shown only when the filtered list is longer
/// than this — mirroring claude-code's `displayedLogs.length > visibleCount`.
pub const VISIBLE_ROWS: usize = 10;

/// Number of resumable sessions enriched into the picker at a time. Claude
/// Code 2.1.246 uses `rQe = 50`: the first page contains up to 50 rows and the
/// next page is revealed when navigation reaches the loaded tail.
pub const RESUME_PAGE_SIZE: usize = 50;

/// Pure state for the Resume picker: the rows plus the selected index and the
/// type-to-search query.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResumeState {
    /// Display rows, newest-first (the loader already sorts mtime desc).
    pub rows: Vec<ResumeRow>,
    /// Catalog rows not yet exposed to the renderer. Keeping these separate
    /// reproduces Claude's `allStatLogs` + `nextIndex` picker shape without
    /// forcing the terminal to render every historical session at startup.
    remaining_rows: Vec<ResumeRow>,
    /// Index into the FILTERED rows of the highlighted row.
    pub selected: usize,
    /// Type-to-search query — filters rows by title (case-insensitive substring).
    pub query: String,
    /// `true` while the `/`-activated search box is focused (claude-code
    /// `LogSelector` `viewMode === "search"`).
    pub in_search_mode: bool,
}

impl ResumeState {
    /// Build from display rows. Selects the first row.
    #[must_use]
    pub fn new(rows: Vec<ResumeRow>) -> Self {
        let mut rows = rows;
        let remaining_rows = if rows.len() > RESUME_PAGE_SIZE {
            rows.split_off(RESUME_PAGE_SIZE)
        } else {
            Vec::new()
        };
        Self {
            rows,
            remaining_rows,
            selected: 0,
            query: String::new(),
            in_search_mode: false,
        }
    }

    /// Whether another 50-row page remains behind the currently rendered
    /// catalog tail.
    #[must_use]
    pub fn has_more(&self) -> bool {
        !self.remaining_rows.is_empty()
    }

    /// Reveal the next Claude-sized page. Returns the number of rows appended.
    fn load_more(&mut self) -> usize {
        let count = self.remaining_rows.len().min(RESUME_PAGE_SIZE);
        self.rows.extend(self.remaining_rows.drain(..count));
        count
    }

    /// `true` when there are no sessions to resume at all (empty-state).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Rows visible under the current query (all loaded rows when empty),
    /// case-insensitive title-substring match. Search spans the full catalog,
    /// including pages not yet revealed in list mode, so a session older than
    /// the first 50 remains directly discoverable.
    #[must_use]
    pub fn filtered(&self) -> Vec<&ResumeRow> {
        if self.query.is_empty() {
            self.rows.iter().collect()
        } else {
            let q = self.query.to_lowercase();
            self.rows
                .iter()
                .chain(self.remaining_rows.iter())
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

/// What [`handle_resume_key`] tells the loop to do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumeOutcome {
    /// Keep the picker open (selection moved, or an inert key).
    Stay,
    /// Resume the exact displayed source (Enter on a real row).
    Resume(ResumeSelection),
    /// Close the picker (Esc in list mode).
    Cancel,
}

/// Pure key handler for the Resume picker.
///
/// - `Up` → move selection up (clamped at 0).
/// - `Down` → move selection down (clamped at `len-1`).
/// - `Enter` → commit a focused search, then resume the selected uuid on the
///   next Enter; on empty-state → `Stay`.
/// - `Esc` → in search with a query, clear it while retaining search focus and
///   the selected source; in empty search leave search; in list mode cancel.
/// - `/` → enter search mode (in list mode).
/// - printable → type into the search query.
/// - anything else → `Stay`.
#[must_use]
pub fn handle_resume_key(state: &mut ResumeState, key: KeyEvent) -> ResumeOutcome {
    let mut n = state.filtered().len();
    match key.code {
        // Search owns keyboard focus until confirmation or an arrow moves it
        // to the results. Neither arrow skips the first matching row.
        KeyCode::Up | KeyCode::Down if state.in_search_mode && n > 0 => {
            state.in_search_mode = false;
            ResumeOutcome::Stay
        }
        KeyCode::Up => {
            state.selected = state.selected.saturating_sub(1);
            ResumeOutcome::Stay
        }
        KeyCode::Down => {
            // Claude 2.1.246 `/resume` initially enriches 50 rows and invokes
            // `onLoadMore` at the loaded tail. Reveal the next page before
            // advancing so one Down press moves naturally onto its first row.
            if n > 0 && state.selected + 1 >= n && state.has_more() {
                state.load_more();
                n = state.filtered().len();
            }
            if n > 0 {
                state.selected = (state.selected + 1).min(n - 1);
            }
            ResumeOutcome::Stay
        }
        // Native 2.1.321 keeps search editing separate from row selection.
        // The first Enter focuses the filtered list without opening a session.
        KeyCode::Enter if state.in_search_mode && n > 0 => {
            state.in_search_mode = false;
            ResumeOutcome::Stay
        }
        KeyCode::Enter => match state.selected_row() {
            Some(row) => ResumeOutcome::Resume(ResumeSelection {
                uuid: row.uuid,
                transcript_path: row.transcript_path.clone(),
            }),
            // Native 2.1.321 leaves both empty catalogs and empty search
            // results open on Enter; Escape remains the cancellation action.
            None => ResumeOutcome::Stay,
        },
        KeyCode::Esc => {
            if state.in_search_mode {
                if state.query.is_empty() {
                    state.in_search_mode = false;
                    ResumeOutcome::Stay
                } else {
                    let selected = state
                        .selected_row()
                        .map(|row| (row.uuid, row.transcript_path.clone()));
                    state.query.clear();
                    if let Some((uuid, path)) = selected {
                        while state.has_more()
                            && !state
                                .rows
                                .iter()
                                .any(|row| row.uuid == uuid && row.transcript_path == path)
                        {
                            state.load_more();
                        }
                        state.selected = state
                            .rows
                            .iter()
                            .position(|row| row.uuid == uuid && row.transcript_path == path)
                            .unwrap_or(0);
                    } else {
                        state.selected = 0;
                    }
                    ResumeOutcome::Stay
                }
            } else {
                ResumeOutcome::Cancel
            }
        }
        KeyCode::Backspace => {
            state.query.pop();
            state.selected = 0;
            ResumeOutcome::Stay
        }
        KeyCode::Char('/') if !state.in_search_mode && key.modifiers == KeyModifiers::NONE => {
            state.in_search_mode = true;
            ResumeOutcome::Stay
        }
        KeyCode::Char(c)
            if key.modifiers == KeyModifiers::NONE || key.modifiers == KeyModifiers::SHIFT =>
        {
            // Typing filters the list, so reveal the search box + query text
            // (the "Type to search" footer invites this in list mode; without
            // this the rows would filter with no visible query).
            state.in_search_mode = true;
            state.query.push(c);
            state.selected = 0;
            ResumeOutcome::Stay
        }
        _ => ResumeOutcome::Stay,
    }
}

/// The line index (into [`resume_lines`] output) of the SELECTED row's title
/// line, so a windowed renderer (the bottom-pane `/resume` picker) can scroll
/// it into view. Kept next to [`resume_lines`] so the two stay in lockstep:
/// header(1) + optional search box(1) + spacer(1), then two lines per row
/// (title + dim metadata). Returns `0` for the empty state (no rows).
#[must_use]
pub fn selected_title_line_index(state: &ResumeState) -> usize {
    if state.is_empty() {
        return 0;
    }
    let header = 1 + usize::from(state.in_search_mode || !state.query.is_empty()) + 1;
    header + state.selected * 2
}

/// Build the picker's display lines (header + optional counter/search +
/// per-row title/metadata + footer). Pure — the render and its snapshot tests
/// share this. Mirrors the iocraft `ResumeScreen` layout + byte-locked copy.
#[must_use]
pub fn resume_lines(state: &ResumeState, theme: &Theme) -> Vec<Line<'static>> {
    let dim = crate::style_adapter::to_ratatui(theme.dim);
    let suggestion = crate::style_adapter::to_ratatui(theme.suggestion);
    let dim_style = Style::default().fg(dim);

    if state.is_empty() {
        return vec![
            Line::from("No conversations found in this project."),
            Line::from(Span::styled("Esc to cancel", dim_style)),
        ];
    }

    let selected = state.selected;
    let filtered = state.filtered();
    let n = filtered.len();
    let mut lines: Vec<Line<'static>> = Vec::new();

    // Header: bold suggestion-colored "Resume session" + dim "(idx of N)" only
    // in list mode when the list overflows the viewport.
    let mut header = vec![Span::styled(
        "Resume session",
        Style::default().fg(suggestion).add_modifier(Modifier::BOLD),
    )];
    if !state.in_search_mode && n > VISIBLE_ROWS {
        header.push(Span::styled(
            format!(" ({} of {})", selected + 1, n),
            dim_style,
        ));
    }
    lines.push(Line::from(header));

    // Keep the committed filter visible after Enter moves focus to the rows.
    if state.in_search_mode || !state.query.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("Search: {}", state.query),
            dim_style,
        )));
    }

    // A blank spacer row (iocraft `padding_top: 1`), then the rows: a title
    // line ("> " on the selected) + a dim metadata line.
    lines.push(Line::from(""));
    for (i, r) in filtered.iter().enumerate() {
        let marker = if !state.in_search_mode && i == selected {
            "> "
        } else {
            "  "
        };
        lines.push(Line::from(format!("{marker}{}", r.title)));
        lines.push(Line::from(Span::styled(
            format!("  {}", r.metadata_label),
            dim_style,
        )));
    }

    // Footer (dim italic), mode-dependent copy.
    let footer = if state.in_search_mode {
        "Type to Search \u{00B7} Enter to select \u{00B7} Esc to clear"
    } else {
        "Type to search \u{00B7} Esc to cancel"
    };
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        footer,
        dim_style.add_modifier(Modifier::ITALIC),
    )));
    lines
}

/// Run the interactive resume picker on the alternate screen. Returns the
/// selected session, exact displayed path, and its capture made at Enter; or
/// `None` on cancel/empty-state. Blocking terminal IO — callers on an async
/// runtime should `spawn_blocking` it (like the chat `run_app`).
///
/// # Errors
/// Propagates the first terminal setup/draw/read IO error (the terminal is
/// restored before returning on the happy path).
pub fn run_resume_picker<T>(
    rows: Vec<ResumeRow>,
    mut capture: impl FnMut(&ResumeSelection) -> std::io::Result<T>,
) -> std::io::Result<Option<(ResumeSelection, T)>> {
    use crossterm::event::{Event, KeyEventKind};

    // Resolve the active theme the same way the chat app does (OSC-11 detection
    // + persisted preference), so the picker's accent/dim colors match.
    tui_core::theme_detect::detect_terminal_theme();
    let setting = tui_core::theme_persist::load_theme_setting()
        .unwrap_or(tui_core::theme::ThemeSetting::Auto);
    let theme = theme_for(setting.resolve());

    let mut state = ResumeState::new(rows);
    let mut terminal = crate::setup_terminal()?;
    let outcome = (|| -> std::io::Result<Option<(ResumeSelection, T)>> {
        loop {
            terminal.draw(|f| {
                let area = ratatui::layout::Rect {
                    x: f.area().x + 1,
                    y: f.area().y + 1,
                    width: f.area().width.saturating_sub(2),
                    height: f.area().height.saturating_sub(2),
                };
                let lines = resume_lines(&state, &theme);
                let total = u16::try_from(lines.len()).unwrap_or(u16::MAX);
                let selected = u16::try_from(selected_title_line_index(&state)).unwrap_or(0);
                let scroll = if area.height == 0 || total <= area.height {
                    0
                } else {
                    selected
                        .saturating_add(2)
                        .saturating_sub(area.height)
                        .min(total - area.height)
                };
                let para = Paragraph::new(lines).scroll((scroll, 0));
                f.render_widget(para, area);
            })?;
            match crossterm::event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => {
                    match handle_resume_key(&mut state, key) {
                        ResumeOutcome::Stay => {}
                        ResumeOutcome::Resume(selection) => {
                            let captured = capture(&selection)?;
                            return Ok(Some((selection, captured)));
                        }
                        ResumeOutcome::Cancel => return Ok(None),
                    }
                }
                _ => {}
            }
        }
    })();
    let _ = crate::restore_terminal(&mut terminal);
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;

    fn row(title: &str) -> ResumeRow {
        ResumeRow {
            uuid: Uuid::new_v4(),
            transcript_path: "/test/session.jsonl".into(),
            title: title.to_string(),
            metadata_label: "1 minute ago \u{00b7} 3 messages".to_string(),
        }
    }
    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn relative_time_singular_plural_and_future() {
        use std::time::{Duration, UNIX_EPOCH};
        let now = UNIX_EPOCH + Duration::from_secs(1_000_000);
        assert_eq!(
            relative_time_ago(now - Duration::from_secs(60), now),
            "1 minute ago"
        );
        assert_eq!(
            relative_time_ago(now - Duration::from_secs(120), now),
            "2 minutes ago"
        );
        assert_eq!(
            relative_time_ago(now + Duration::from_secs(7200), now),
            "in 2 hours"
        );
    }

    #[test]
    fn arrows_navigate_and_clamp() {
        let mut s = ResumeState::new(vec![row("a"), row("b"), row("c")]);
        assert_eq!(s.selected, 0);
        assert_eq!(
            handle_resume_key(&mut s, key(KeyCode::Up)),
            ResumeOutcome::Stay
        );
        assert_eq!(s.selected, 0, "clamped at top");
        let _ = handle_resume_key(&mut s, key(KeyCode::Down));
        let _ = handle_resume_key(&mut s, key(KeyCode::Down));
        let _ = handle_resume_key(&mut s, key(KeyCode::Down));
        assert_eq!(s.selected, 2, "clamped at bottom");
    }

    #[test]
    fn scrolling_past_the_first_fifty_reveals_the_next_page() {
        let rows: Vec<ResumeRow> = (0..55).map(|i| row(&format!("session {i}"))).collect();
        let mut s = ResumeState::new(rows);

        assert_eq!(s.rows.len(), RESUME_PAGE_SIZE);
        assert!(s.has_more());
        for _ in 0..RESUME_PAGE_SIZE - 1 {
            let _ = handle_resume_key(&mut s, key(KeyCode::Down));
        }
        assert_eq!(s.selected, RESUME_PAGE_SIZE - 1);
        assert_eq!(s.rows.len(), RESUME_PAGE_SIZE);

        let _ = handle_resume_key(&mut s, key(KeyCode::Down));
        assert_eq!(s.selected, RESUME_PAGE_SIZE);
        assert_eq!(s.rows.len(), 55);
        assert!(!s.has_more());
        assert_eq!(s.selected_row().unwrap().title, "session 50");
    }

    #[test]
    fn enter_resumes_exact_selected_row_with_duplicate_uuid() {
        let rows = vec![row("a"), row("b")];
        let want = rows[1].uuid;
        let mut rows = rows;
        rows[0].uuid = want;
        rows[0].transcript_path = "/first/session.jsonl".into();
        rows[1].transcript_path = "/second/session.jsonl".into();
        let mut s = ResumeState::new(rows);
        let _ = handle_resume_key(&mut s, key(KeyCode::Down));
        assert_eq!(
            handle_resume_key(&mut s, key(KeyCode::Enter)),
            ResumeOutcome::Resume(ResumeSelection {
                uuid: want,
                transcript_path: "/second/session.jsonl".into(),
            })
        );
    }

    #[test]
    fn esc_cancels_in_list_mode() {
        let mut s = ResumeState::new(vec![row("a")]);
        assert_eq!(
            handle_resume_key(&mut s, key(KeyCode::Esc)),
            ResumeOutcome::Cancel
        );
    }

    #[test]
    fn type_to_search_filters_and_slash_enters_search() {
        let mut s = ResumeState::new(vec![row("fix bug"), row("add feature")]);
        let _ = handle_resume_key(&mut s, key(KeyCode::Char('/')));
        assert!(s.in_search_mode);
        for c in "feat".chars() {
            let _ = handle_resume_key(&mut s, key(KeyCode::Char(c)));
        }
        assert_eq!(s.filtered().len(), 1);
        assert_eq!(s.filtered()[0].title, "add feature");
        // Esc clears the query while retaining search focus and selection.
        let _ = handle_resume_key(&mut s, key(KeyCode::Esc));
        assert!(s.query.is_empty());
        assert!(s.in_search_mode);
        assert_eq!(s.selected, 1);
        assert_eq!(s.filtered().len(), 2);
        assert_eq!(
            handle_resume_key(&mut s, key(KeyCode::Esc)),
            ResumeOutcome::Stay
        );
        assert!(!s.in_search_mode);
        assert_eq!(
            handle_resume_key(&mut s, key(KeyCode::Esc)),
            ResumeOutcome::Cancel
        );
    }

    #[test]
    fn search_enter_commits_filter_before_resuming_exact_source() {
        let rows = vec![row("recent work"), row("release Alpha")];
        let expected = ResumeSelection {
            uuid: rows[1].uuid,
            transcript_path: rows[1].transcript_path.clone(),
        };
        let mut s = ResumeState::new(rows);
        for c in "Alpha".chars() {
            let _ = handle_resume_key(&mut s, key(KeyCode::Char(c)));
        }
        assert_eq!(
            handle_resume_key(&mut s, key(KeyCode::Enter)),
            ResumeOutcome::Stay
        );
        assert!(!s.in_search_mode);
        assert_eq!(s.query, "Alpha");
        let text = resume_lines(&s, &tui_core::theme::Theme::dark())
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content)
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        assert!(text.iter().any(|line| line == "Search: Alpha"));
        assert!(text.iter().any(|line| line == "> release Alpha"));
        assert_eq!(
            handle_resume_key(&mut s, key(KeyCode::Enter)),
            ResumeOutcome::Resume(expected)
        );
    }

    #[test]
    fn committed_search_escape_cancels_without_changing_filter() {
        let mut s = ResumeState::new(vec![row("Alpha")]);
        let _ = handle_resume_key(&mut s, key(KeyCode::Char('A')));
        let _ = handle_resume_key(&mut s, key(KeyCode::Enter));
        assert_eq!(
            handle_resume_key(&mut s, key(KeyCode::Esc)),
            ResumeOutcome::Cancel
        );
        assert_eq!(s.query, "A");
    }

    #[test]
    fn search_arrows_focus_results_without_skipping_first_match() {
        for arrow in [KeyCode::Up, KeyCode::Down] {
            let rows = vec![row("session Beta"), row("session Alpha")];
            let expected = rows[0].uuid;
            let mut s = ResumeState::new(rows);
            for c in "session".chars() {
                let _ = handle_resume_key(&mut s, key(KeyCode::Char(c)));
            }
            assert_eq!(handle_resume_key(&mut s, key(arrow)), ResumeOutcome::Stay);
            assert!(!s.in_search_mode);
            assert_eq!(s.selected_uuid(), Some(expected));
            assert!(
                matches!(handle_resume_key(&mut s, key(KeyCode::Enter)), ResumeOutcome::Resume(selection) if selection.uuid == expected)
            );
        }
    }

    #[test]
    fn search_finds_a_session_beyond_the_first_page() {
        let mut rows: Vec<ResumeRow> = (0..RESUME_PAGE_SIZE)
            .map(|i| row(&format!("recent {i}")))
            .collect();
        let wanted = row("old release investigation");
        let wanted_uuid = wanted.uuid;
        rows.push(wanted);
        let mut s = ResumeState::new(rows);

        for c in "release".chars() {
            let _ = handle_resume_key(&mut s, key(KeyCode::Char(c)));
        }

        assert_eq!(s.filtered().len(), 1);
        assert_eq!(s.selected_uuid(), Some(wanted_uuid));
        assert_eq!(s.rows.len(), RESUME_PAGE_SIZE);
        assert!(s.has_more(), "search must not flatten list-mode pagination");
        let _ = handle_resume_key(&mut s, key(KeyCode::Esc));
        assert!(s.in_search_mode);
        assert_eq!(s.selected_uuid(), Some(wanted_uuid));
        assert_eq!(s.rows.len(), RESUME_PAGE_SIZE + 1);
    }

    #[test]
    fn empty_state_enter_stays_and_message_matches_reference() {
        let mut s = ResumeState::new(vec![]);
        assert_eq!(
            handle_resume_key(&mut s, key(KeyCode::Enter)),
            ResumeOutcome::Stay
        );
        let lines = resume_lines(&s, &tui_core::theme::Theme::dark());
        let text: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|sp| sp.content.as_ref()).collect())
            .collect();
        assert_eq!(text[0], "No conversations found in this project.");
        assert_eq!(text[1], "Esc to cancel");
    }

    #[test]
    fn header_and_footer_copy_locked() {
        let s = ResumeState::new(vec![row("hello")]);
        let lines = resume_lines(&s, &tui_core::theme::Theme::dark());
        let joined: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|sp| sp.content.as_ref()).collect())
            .collect();
        assert_eq!(joined[0], "Resume session");
        assert!(joined.iter().any(|l| l == "> hello"));
        assert!(joined
            .iter()
            .any(|l| l == "Type to search \u{00b7} Esc to cancel"));
    }
}

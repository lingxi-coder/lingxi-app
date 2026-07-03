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
    /// Title (already truncated to ≤ 50 chars + ellipsis by the loader).
    pub title: String,
    /// Dim metadata line shown under the title: `<relative time ago> · <N>
    /// messages` (claude-code `formatLogMetadata`). Built by the caller.
    pub metadata_label: String,
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

/// Pure state for the Resume picker: the rows plus the selected index and the
/// type-to-search query.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResumeState {
    /// Display rows, newest-first (the loader already sorts mtime desc).
    pub rows: Vec<ResumeRow>,
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

    /// Rows visible under the current query (all rows when empty),
    /// case-insensitive title-substring match.
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

/// What [`handle_resume_key`] tells the loop to do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeOutcome {
    /// Keep the picker open (selection moved, or an inert key).
    Stay,
    /// Resume the session with this UUID (Enter on a real row).
    Resume(Uuid),
    /// Close the picker (Esc, or Enter on empty-state).
    Cancel,
}

/// Pure key handler for the Resume picker.
///
/// - `Up` → move selection up (clamped at 0).
/// - `Down` → move selection down (clamped at `len-1`).
/// - `Enter` → resume the selected uuid; on empty-state → `Cancel`.
/// - `Esc` → in search with a query, clear it; in search with no query, leave
///   search; in list mode, cancel.
/// - `/` → enter search mode (in list mode).
/// - printable → type into the search query.
/// - anything else → `Stay`.
#[must_use]
pub fn handle_resume_key(state: &mut ResumeState, key: KeyEvent) -> ResumeOutcome {
    let n = state.filtered().len();
    match key.code {
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
        KeyCode::Char('/') if !state.in_search_mode && key.modifiers == KeyModifiers::NONE => {
            state.in_search_mode = true;
            ResumeOutcome::Stay
        }
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
            Line::from("No conversations found to resume."),
            Line::from(Span::styled(
                "Press Ctrl+C to exit and start a new conversation.",
                dim_style,
            )),
        ];
    }

    let selected = state.selected;
    let filtered = state.filtered();
    let n = filtered.len();
    let mut lines: Vec<Line<'static>> = Vec::new();

    // Header: bold suggestion-colored "Resume Session" + dim "(idx of N)" only
    // in list mode when the list overflows the viewport.
    let mut header = vec![Span::styled(
        "Resume Session",
        Style::default()
            .fg(suggestion)
            .add_modifier(Modifier::BOLD),
    )];
    if !state.in_search_mode && n > VISIBLE_ROWS {
        header.push(Span::styled(
            format!(" ({} of {})", selected + 1, n),
            dim_style,
        ));
    }
    lines.push(Line::from(header));

    // Search box (search mode only).
    if state.in_search_mode {
        lines.push(Line::from(Span::styled(
            format!("Search: {}", state.query),
            dim_style,
        )));
    }

    // A blank spacer row (iocraft `padding_top: 1`), then the rows: a title
    // line ("> " on the selected) + a dim metadata line.
    lines.push(Line::from(""));
    for (i, r) in filtered.iter().enumerate() {
        let marker = if i == selected { "> " } else { "  " };
        lines.push(Line::from(format!("{marker}{}", r.title)));
        lines.push(Line::from(Span::styled(
            format!("  {}", r.metadata_label),
            dim_style,
        )));
    }

    // Footer (dim italic), mode-dependent copy.
    let footer = if state.in_search_mode {
        "Type to Search \u{00B7} Enter select \u{00B7} Esc clear"
    } else {
        "Type to search \u{00B7} Esc cancel"
    };
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        footer,
        dim_style.add_modifier(Modifier::ITALIC),
    )));
    lines
}

/// Run the interactive resume picker on the alternate screen. Returns the
/// selected session uuid, or `None` on cancel/empty-state. Blocking terminal
/// IO — callers on an async runtime should `spawn_blocking` it (like the chat
/// `run_app`).
///
/// # Errors
/// Propagates the first terminal setup/draw/read IO error (the terminal is
/// restored before returning on the happy path).
pub fn run_resume_picker(rows: Vec<ResumeRow>) -> std::io::Result<Option<Uuid>> {
    use crossterm::event::{Event, KeyEventKind};

    // Resolve the active theme the same way the chat app does (OSC-11 detection
    // + persisted preference), so the picker's accent/dim colors match.
    tui_core::theme_detect::detect_terminal_theme();
    let setting = tui_core::theme_persist::load_theme_setting()
        .unwrap_or(tui_core::theme::ThemeSetting::Auto);
    let theme = theme_for(setting.resolve());

    let mut state = ResumeState::new(rows);
    let mut terminal = crate::setup_terminal()?;
    let outcome = (|| -> std::io::Result<Option<Uuid>> {
        loop {
            terminal.draw(|f| {
                let para = Paragraph::new(resume_lines(&state, &theme));
                let area = ratatui::layout::Rect {
                    x: f.area().x + 1,
                    y: f.area().y + 1,
                    width: f.area().width.saturating_sub(2),
                    height: f.area().height.saturating_sub(2),
                };
                f.render_widget(para, area);
            })?;
            match crossterm::event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => {
                    match handle_resume_key(&mut state, key) {
                        ResumeOutcome::Stay => {}
                        ResumeOutcome::Resume(uuid) => return Ok(Some(uuid)),
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
        assert_eq!(relative_time_ago(now - Duration::from_secs(60), now), "1 minute ago");
        assert_eq!(relative_time_ago(now - Duration::from_secs(120), now), "2 minutes ago");
        assert_eq!(relative_time_ago(now + Duration::from_secs(7200), now), "in 2 hours");
    }

    #[test]
    fn arrows_navigate_and_clamp() {
        let mut s = ResumeState::new(vec![row("a"), row("b"), row("c")]);
        assert_eq!(s.selected, 0);
        assert_eq!(handle_resume_key(&mut s, key(KeyCode::Up)), ResumeOutcome::Stay);
        assert_eq!(s.selected, 0, "clamped at top");
        handle_resume_key(&mut s, key(KeyCode::Down));
        handle_resume_key(&mut s, key(KeyCode::Down));
        handle_resume_key(&mut s, key(KeyCode::Down));
        assert_eq!(s.selected, 2, "clamped at bottom");
    }

    #[test]
    fn enter_resumes_selected_uuid() {
        let rows = vec![row("a"), row("b")];
        let want = rows[1].uuid;
        let mut s = ResumeState::new(rows);
        handle_resume_key(&mut s, key(KeyCode::Down));
        assert_eq!(handle_resume_key(&mut s, key(KeyCode::Enter)), ResumeOutcome::Resume(want));
    }

    #[test]
    fn esc_cancels_in_list_mode() {
        let mut s = ResumeState::new(vec![row("a")]);
        assert_eq!(handle_resume_key(&mut s, key(KeyCode::Esc)), ResumeOutcome::Cancel);
    }

    #[test]
    fn type_to_search_filters_and_slash_enters_search() {
        let mut s = ResumeState::new(vec![row("fix bug"), row("add feature")]);
        handle_resume_key(&mut s, key(KeyCode::Char('/')));
        assert!(s.in_search_mode);
        for c in "feat".chars() {
            handle_resume_key(&mut s, key(KeyCode::Char(c)));
        }
        assert_eq!(s.filtered().len(), 1);
        assert_eq!(s.filtered()[0].title, "add feature");
        // Esc with a query clears it and drops back to the list.
        handle_resume_key(&mut s, key(KeyCode::Esc));
        assert!(s.query.is_empty());
        assert_eq!(s.filtered().len(), 2);
    }

    #[test]
    fn empty_state_enter_cancels_and_lines_are_locked() {
        let mut s = ResumeState::new(vec![]);
        assert_eq!(handle_resume_key(&mut s, key(KeyCode::Enter)), ResumeOutcome::Cancel);
        let lines = resume_lines(&s, &tui_core::theme::Theme::dark());
        let text: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|sp| sp.content.as_ref()).collect())
            .collect();
        assert_eq!(text[0], "No conversations found to resume.");
        assert_eq!(text[1], "Press Ctrl+C to exit and start a new conversation.");
    }

    #[test]
    fn header_and_footer_copy_locked() {
        let s = ResumeState::new(vec![row("hello")]);
        let lines = resume_lines(&s, &tui_core::theme::Theme::dark());
        let joined: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|sp| sp.content.as_ref()).collect())
            .collect();
        assert_eq!(joined[0], "Resume Session");
        assert!(joined.iter().any(|l| l == "> hello"));
        assert!(joined.iter().any(|l| l == "Type to search \u{00b7} Esc cancel"));
    }
}

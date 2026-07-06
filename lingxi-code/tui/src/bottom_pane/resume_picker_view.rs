//! `/resume`: an interactive session picker, rendered IN THE BOTTOM PANE (a
//! full-frame [`BottomPaneView`], NOT the standalone alt-screen
//! [`crate::resume::run_resume_picker`] used at startup). On `Enter` it emits a
//! [`ViewOutcome::SwitchSession`] carrying the chosen session uuid; the owner
//! (`RataApp::run` → `run_app` → `run_ratatui`) unwinds its blocking loop and
//! RE-MOUNTS that session in-process so the JSONL writer is correctly retargeted
//! — this is NOT an in-place engine swap (which would fork the conversation file).
//!
//! The state + key handling are the PURE, terminal-free
//! [`crate::resume::ResumeState`] / [`crate::resume::handle_resume_key`] reused
//! verbatim from the startup picker; the render reuses
//! [`crate::resume::resume_lines`]. This view adds no new terminal code — only
//! the bottom-pane [`Renderable`]/[`BottomPaneView`] glue and a scroll offset
//! (via [`crate::resume::selected_title_line_index`]) that keeps the selected
//! row visible when the session list is taller than the pane.

use std::any::Any;

use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};
use tui_core::theme::Theme;

use crate::bottom_pane::view::{BottomPaneView, ViewOutcome};
use crate::renderable::Renderable;
use crate::resume::{
    handle_resume_key, resume_lines, selected_title_line_index, ResumeOutcome, ResumeRow,
    ResumeState,
};

/// The `/resume` interactive session picker view.
pub struct ResumePickerView {
    /// The pure picker state (rows + selection + type-to-search query), reused
    /// verbatim from the startup alt-screen picker.
    state: ResumeState,
    /// The active render palette (captured at construction — the picker paints
    /// dim/accent spans through [`resume_lines`]).
    theme: Theme,
}

impl ResumePickerView {
    /// Build the picker over `rows` (already mapped from session metadata by the
    /// CLI loader). Selects the newest (first) row.
    #[must_use]
    pub fn new(rows: Vec<ResumeRow>, theme: Theme) -> Self {
        Self {
            state: ResumeState::new(rows),
            theme,
        }
    }

    /// Build the picker pre-filtered by `query` (the `/resume <term>` argument):
    /// the search box opens focused and the list is filtered immediately.
    #[must_use]
    pub fn with_query(rows: Vec<ResumeRow>, theme: Theme, query: &str) -> Self {
        let mut state = ResumeState::new(rows);
        state.query = query.to_string();
        state.in_search_mode = true;
        Self { state, theme }
    }

    /// Test/inspection access to the pure state.
    #[must_use]
    pub fn state(&self) -> &ResumeState {
        &self.state
    }

    /// The vertical scroll offset (into [`resume_lines`] output) that keeps the
    /// selected row visible within a `viewport`-tall inner area. `0` until the
    /// selection would fall below the viewport, then just enough to reveal the
    /// selected row's title + metadata lines (the header scrolls off only when
    /// it must).
    fn scroll_offset(&self, total: u16, viewport: u16) -> u16 {
        if viewport == 0 || total <= viewport {
            return 0;
        }
        let max_scroll = total - viewport;
        let selected = u16::try_from(selected_title_line_index(&self.state)).unwrap_or(0);
        // Reveal the selected title (+ its metadata line): `selected + 2`.
        selected.saturating_add(2).saturating_sub(viewport).min(max_scroll)
    }
}

impl Renderable for ResumePickerView {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        Clear.render(area, buf);
        let block = Block::new().borders(Borders::ALL);
        let inner = block.inner(area);
        block.render(area, buf);
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        let lines = resume_lines(&self.state, &self.theme);
        let total = u16::try_from(lines.len()).unwrap_or(u16::MAX);
        let scroll = self.scroll_offset(total, inner.height);
        Paragraph::new(lines).scroll((scroll, 0)).render(inner, buf);
    }

    /// Body lines + the two border rows (no soft-wrap, so height is
    /// width-independent). The bottom viewport clamps this to its maximum and
    /// the body scrolls within what it gets (see [`Self::scroll_offset`]).
    fn desired_height(&self, _width: u16) -> u16 {
        u16::try_from(resume_lines(&self.state, &self.theme).len())
            .unwrap_or(u16::MAX)
            .saturating_add(2)
    }
}

impl BottomPaneView for ResumePickerView {
    fn handle_key(&mut self, key: KeyEvent) -> ViewOutcome {
        match handle_resume_key(&mut self.state, key) {
            ResumeOutcome::Stay => ViewOutcome::Pending,
            ResumeOutcome::Cancel => ViewOutcome::Cancelled,
            ResumeOutcome::Resume(uuid) => ViewOutcome::SwitchSession(uuid),
        }
    }

    /// A full-frame picker owns the whole viewport: no status row, no composer
    /// beneath (same contract as [`crate::bottom_pane::screen_view::ScreenView`]).
    fn wants_status_line(&self) -> bool {
        false
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers};
    use uuid::Uuid;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn row(title: &str) -> ResumeRow {
        ResumeRow {
            uuid: Uuid::new_v4(),
            title: title.to_string(),
            metadata_label: "1 minute ago \u{00b7} 3 messages".to_string(),
        }
    }

    fn view(rows: Vec<ResumeRow>) -> ResumePickerView {
        ResumePickerView::new(rows, Theme::dark())
    }

    #[test]
    fn enter_on_a_row_emits_switch_session_for_that_uuid() {
        let rows = vec![row("fix bug"), row("add feature")];
        let want = rows[1].uuid;
        let mut v = view(rows);
        v.handle_key(press(KeyCode::Down));
        match v.handle_key(press(KeyCode::Enter)) {
            ViewOutcome::SwitchSession(uuid) => assert_eq!(uuid, want),
            _ => panic!("expected SwitchSession"),
        }
    }

    #[test]
    fn esc_cancels_the_picker() {
        let mut v = view(vec![row("a")]);
        assert!(matches!(
            v.handle_key(press(KeyCode::Esc)),
            ViewOutcome::Cancelled
        ));
    }

    #[test]
    fn empty_state_enter_cancels() {
        let mut v = view(vec![]);
        assert!(matches!(
            v.handle_key(press(KeyCode::Enter)),
            ViewOutcome::Cancelled
        ));
    }

    #[test]
    fn with_query_pre_filters_the_list() {
        let v = ResumePickerView::with_query(
            vec![row("fix bug"), row("add feature")],
            Theme::dark(),
            "feat",
        );
        assert_eq!(v.state().filtered().len(), 1);
        assert_eq!(v.state().filtered()[0].title, "add feature");
    }

    #[test]
    fn full_frame_contract_suppresses_the_status_line() {
        assert!(!view(vec![row("a")]).wants_status_line());
    }

    #[test]
    fn scroll_offset_keeps_the_selected_row_visible() {
        // 12 rows → 2 lines each + header/spacer/footer; a short viewport must
        // scroll to reveal a low selection.
        let rows: Vec<ResumeRow> = (0..12).map(|i| row(&format!("s{i}"))).collect();
        let mut v = view(rows);
        // Select the last row.
        for _ in 0..11 {
            v.handle_key(press(KeyCode::Down));
        }
        let total = u16::try_from(resume_lines(v.state(), &Theme::dark()).len()).unwrap();
        // A viewport shorter than the content must scroll (> 0) and never past
        // the end.
        let vp = 8;
        let off = v.scroll_offset(total, vp);
        assert!(off > 0, "a low selection in a short viewport must scroll");
        assert!(off <= total - vp, "never scrolls past the end");
        // A viewport taller than the content never scrolls.
        assert_eq!(v.scroll_offset(total, total + 5), 0);
    }

    #[test]
    fn renders_a_row_title_into_the_buffer() {
        let v = view(vec![row("resume me")]);
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        v.render(area, &mut buf);
        let buf_ref = &buf;
        let text: String = (area.top()..area.bottom())
            .flat_map(|y| {
                (area.left()..area.right()).map(move |x| {
                    buf_ref
                        .cell(ratatui::layout::Position::new(x, y))
                        .map_or(" ", ratatui::buffer::Cell::symbol)
                        .to_string()
                })
            })
            .collect();
        assert!(text.contains("resume me"), "{text}");
        assert!(text.contains("Resume Session"), "{text}");
    }
}

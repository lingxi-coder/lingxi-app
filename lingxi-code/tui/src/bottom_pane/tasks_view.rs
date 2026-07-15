//! `/tasks` (alias `/bashes`): an interactive picker of live background tasks,
//! rendered as a full-frame [`BottomPaneView`] (same contract as
//! [`crate::bottom_pane::resume_picker_view::ResumePickerView`]). It lists a
//! snapshot of the live `TaskRegistry` (taken in
//! [`crate::chat_widget::ChatWidget::cmd_tasks`] via the proven-safe in-memory
//! `block_on` read idiom) with each task's status glyph + label. Pressing
//! `x`/`d`/Delete on a RUNNING row asks the owner to stop that task OFF-LOOP
//! ([`ViewOutcome::RunTaskAction`] → `TaskRegistryHandle::kill`); the row is
//! marked `killed` optimistically and the picker stays open so several tasks can
//! be stopped in one visit. `Esc`/`Enter`/`q` closes it.
//!
//! Faithful SUBSET of claude-code's `BackgroundTasksDialog` (list + stop). The
//! per-task-type DETAIL/output sub-dialogs and live re-polling are deferred: the
//! list is a snapshot at open time (re-run `/tasks` to refresh).

use std::any::Any;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};
use tui_core::multiagent::TaskRow;
use tui_core::theme::Theme;

use crate::bottom_pane::view::{BottomPaneView, TaskAction, ViewOutcome};
use crate::renderable::Renderable;

/// The `/tasks` interactive background-task picker.
pub struct TasksView {
    /// Snapshot of the live registry rows (newest-first as the registry orders
    /// them), taken when the picker was opened.
    rows: Vec<TaskRow>,
    /// Index of the highlighted row.
    selected: usize,
    /// Active render palette (accent header + dim footer hint).
    theme: Theme,
}

impl TasksView {
    /// Build the picker over `rows`, selecting the first row.
    #[must_use]
    pub fn new(rows: Vec<TaskRow>, theme: Theme) -> Self {
        Self {
            rows,
            selected: 0,
            theme,
        }
    }

    /// Whether `status` denotes a still-running task that can be stopped.
    fn is_killable(status: &str) -> bool {
        matches!(status, "running" | "pending" | "queued")
    }

    /// The status glyph for one row.
    fn glyph(status: &str) -> &'static str {
        match status {
            "running" | "pending" | "queued" => "\u{25cf}",
            "completed" => "\u{2713}",
            "failed" => "\u{2717}",
            "killed" => "\u{2298}",
            _ => "\u{2022}",
        }
    }

    /// Rendered body lines: header, spacer, one line per row, spacer, hint.
    fn lines(&self) -> Vec<Line<'static>> {
        let dim = crate::style_adapter::to_ratatui(self.theme.dim);
        let accent = crate::style_adapter::to_ratatui(self.theme.suggestion);
        let dim_style = Style::default().fg(dim);
        let mut lines: Vec<Line<'static>> = Vec::with_capacity(self.rows.len() + 4);
        lines.push(Line::from(Span::styled(
            format!("Background Tasks ({})", self.rows.len()),
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));
        for (i, r) in self.rows.iter().enumerate() {
            let marker = if i == self.selected {
                "\u{276f} "
            } else {
                "  "
            };
            let short: String = r.task_id.chars().take(9).collect();
            let label = r.command.clone().unwrap_or_else(|| r.description.clone());
            let text = format!(
                "{marker}{} {short}  {:<9}  {label}",
                Self::glyph(&r.status),
                r.status,
            );
            let style = if i == self.selected {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            lines.push(Line::from(Span::styled(text, style)));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "\u{2191}\u{2193} navigate \u{00b7} x stop task \u{00b7} Esc close",
            dim_style,
        )));
        lines
    }

    /// Line index of the selected row (header + spacer occupy 2 lines).
    fn selected_line_index(&self) -> usize {
        self.selected.saturating_add(2)
    }

    /// Scroll offset keeping the selected row visible in a `viewport`-tall inner
    /// area (same shape as the resume picker's offset).
    fn scroll_offset(&self, total: u16, viewport: u16) -> u16 {
        if viewport == 0 || total <= viewport {
            return 0;
        }
        let max_scroll = total - viewport;
        let selected = u16::try_from(self.selected_line_index()).unwrap_or(0);
        selected
            .saturating_add(1)
            .saturating_sub(viewport)
            .min(max_scroll)
    }

    /// Test/inspection access to the snapshot rows.
    #[must_use]
    pub fn rows(&self) -> &[TaskRow] {
        &self.rows
    }
}

impl Renderable for TasksView {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        Clear.render(area, buf);
        let block = Block::new().borders(Borders::ALL);
        let inner = block.inner(area);
        block.render(area, buf);
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        let lines = self.lines();
        let total = u16::try_from(lines.len()).unwrap_or(u16::MAX);
        let scroll = self.scroll_offset(total, inner.height);
        Paragraph::new(lines).scroll((scroll, 0)).render(inner, buf);
    }

    fn desired_height(&self, _width: u16) -> u16 {
        u16::try_from(self.lines().len())
            .unwrap_or(u16::MAX)
            .saturating_add(2)
    }
}

impl BottomPaneView for TasksView {
    fn handle_key(&mut self, key: KeyEvent) -> ViewOutcome {
        match key.code {
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => ViewOutcome::Cancelled,
            KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                ViewOutcome::Pending
            }
            KeyCode::Down => {
                if self.selected + 1 < self.rows.len() {
                    self.selected += 1;
                }
                ViewOutcome::Pending
            }
            KeyCode::Char('x') | KeyCode::Char('d') | KeyCode::Delete => {
                match self.rows.get(self.selected) {
                    Some(r) if Self::is_killable(&r.status) => {
                        let task_id = r.task_id.clone();
                        // Optimistic: reflect the stop in the list immediately;
                        // the real kill runs off-loop and its result lands in
                        // the transcript via `TurnEvent::SystemNotice`.
                        self.rows[self.selected].status = "killed".to_string();
                        ViewOutcome::RunTaskAction(TaskAction::Kill { task_id })
                    }
                    _ => ViewOutcome::Pending,
                }
            }
            _ => ViewOutcome::Pending,
        }
    }

    /// A full-frame picker owns the whole viewport: no status row, no composer
    /// beneath (same contract as the resume picker).
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
    use crossterm::event::KeyModifiers;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn row(id: &str, status: &str, desc: &str) -> TaskRow {
        TaskRow {
            task_id: id.to_string(),
            task_type: "local_bash".to_string(),
            status: status.to_string(),
            description: desc.to_string(),
            command: None,
        }
    }

    fn view(rows: Vec<TaskRow>) -> TasksView {
        TasksView::new(rows, Theme::dark())
    }

    #[test]
    fn stop_on_a_running_row_emits_task_action_and_marks_killed() {
        let mut v = view(vec![row("b00000001", "running", "cargo build")]);
        match v.handle_key(press(KeyCode::Char('x'))) {
            ViewOutcome::RunTaskAction(TaskAction::Kill { task_id }) => {
                assert_eq!(task_id, "b00000001");
            }
            _ => panic!("expected RunTaskAction"),
        }
        assert_eq!(v.rows()[0].status, "killed");
    }

    #[test]
    fn stop_on_a_terminal_row_is_ignored() {
        let mut v = view(vec![row("b00000001", "completed", "done")]);
        assert!(matches!(
            v.handle_key(press(KeyCode::Char('x'))),
            ViewOutcome::Pending
        ));
        assert_eq!(v.rows()[0].status, "completed");
    }

    #[test]
    fn esc_cancels_the_picker() {
        let mut v = view(vec![row("b1", "running", "x")]);
        assert!(matches!(
            v.handle_key(press(KeyCode::Esc)),
            ViewOutcome::Cancelled
        ));
    }

    #[test]
    fn down_and_up_move_the_selection_with_clamping() {
        let mut v = view(vec![row("a", "running", "1"), row("b", "running", "2")]);
        v.handle_key(press(KeyCode::Down));
        assert_eq!(v.selected, 1);
        v.handle_key(press(KeyCode::Down)); // clamp at the last row
        assert_eq!(v.selected, 1);
        v.handle_key(press(KeyCode::Up));
        assert_eq!(v.selected, 0);
    }

    #[test]
    fn full_frame_contract_suppresses_the_status_line() {
        assert!(!view(vec![row("a", "running", "x")]).wants_status_line());
    }

    #[test]
    fn renders_header_and_row_into_the_buffer() {
        let v = view(vec![row("b00000001", "running", "cargo build")]);
        let area = Rect::new(0, 0, 60, 10);
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
        assert!(text.contains("Background Tasks"), "{text}");
        assert!(text.contains("cargo build"), "{text}");
    }
}

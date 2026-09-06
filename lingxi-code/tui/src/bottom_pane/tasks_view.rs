//! `/tasks` (alias `/bashes`): an interactive picker of live background tasks,
//! rendered as a full-frame [`BottomPaneView`] (same contract as
//! [`crate::bottom_pane::resume_picker_view::ResumePickerView`]). It lists a
//! snapshot of the live `TaskRegistry` (taken in
//! [`crate::chat_widget::ChatWidget::cmd_tasks`] via the proven-safe in-memory
//! `block_on` read idiom) with each task's status glyph + label. Pressing
//! `x`/`d`/Delete on a RUNNING row asks the owner to stop that task OFF-LOOP
//! ([`ViewOutcome::RunTaskAction`] → `TaskRegistryHandle::kill`); the row is
//! marked `killed` optimistically and the picker stays open so several tasks can
//! be stopped in one visit. Mounted views also accept `TasksRefreshed` events,
//! preserving the selected task by ID across each replacement. `Esc`/`Enter`/
//! `q` closes it.
//!
//! Faithful SUBSET of claude-code's `BackgroundTasksDialog` (list + stop). The
//! per-task-type DETAIL/output sub-dialogs are deferred. The opening snapshot
//! remains useful when no live feed is available; the normal multi-agent event
//! path refreshes a mounted picker when wired.

use std::any::Any;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};
use tui_core::multiagent::{sanitize_task_text, MultiAgentEvent, TaskRow};
use tui_core::theme::Theme;

use crate::bottom_pane::view::{BottomPaneView, TaskAction, ViewOutcome};
use crate::renderable::Renderable;

/// The `/tasks` interactive background-task picker.
pub struct TasksView {
    /// Current registry rows (newest-first as the registry orders them).
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

    /// Replace the current task snapshot while keeping the same task selected
    /// when it is still present. If the selected task disappeared, retain its
    /// old position as closely as possible and clamp it to the new list.
    pub fn refresh_rows(&mut self, rows: Vec<TaskRow>) {
        let selected_id = self.rows.get(self.selected).map(|row| row.task_id.clone());
        let old_selected = self.selected;
        self.rows = rows;
        self.selected = selected_id
            .as_deref()
            .and_then(|task_id| self.rows.iter().position(|row| row.task_id == task_id))
            .unwrap_or_else(|| old_selected.min(self.rows.len().saturating_sub(1)));
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
        if self.rows.is_empty() {
            // The oracle's Background dialog keeps its empty state INSIDE the
            // dialog (`children: D.length === 0 ? "No tasks currently running"
            // : …`) — the agents-view entry opens the picker even with nothing
            // running, so the line has to render here rather than as a
            // transcript cell.
            lines.push(Line::from(Span::styled(
                "No tasks currently running",
                dim_style,
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("Esc close", dim_style)));
            return lines;
        }
        for (i, r) in self.rows.iter().enumerate() {
            let marker = if i == self.selected {
                "\u{276f} "
            } else {
                "  "
            };
            let short: String = r.task_id.chars().take(9).collect();
            let label = r.command.clone().unwrap_or_else(|| r.description.clone());
            let mut text = format!(
                "{marker}{} {short}  {:<9}  {label}",
                Self::glyph(&r.status),
                r.status,
            );
            if r.task_type == "local_fusion" && r.status == "running" {
                if let Some(stage) = r
                    .stage
                    .as_deref()
                    .map(sanitize_task_text)
                    .filter(|stage| !stage.is_empty())
                {
                    text.push_str("  [");
                    text.push_str(&stage);
                    text.push(']');
                }
            }
            if r.task_type == "local_fusion" && r.status == "failed" {
                if let Some(error) = r
                    .error
                    .as_deref()
                    .map(sanitize_task_text)
                    .filter(|error| !error.is_empty())
                {
                    text.push_str("  — ");
                    text.push_str(&error);
                }
            }
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

    fn apply_multiagent_event(&mut self, event: &MultiAgentEvent) {
        if let MultiAgentEvent::TasksRefreshed(rows) = event {
            self.refresh_rows(rows.clone());
        }
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
            stage: None,
            error: None,
        }
    }

    fn view(rows: Vec<TaskRow>) -> TasksView {
        TasksView::new(rows, Theme::dark())
    }

    /// The agents-view entry opens this picker with nothing running, so the
    /// oracle's in-dialog empty body has to render here (Background dialog:
    /// `children: D.length === 0 ? "No tasks currently running" : …`).
    #[test]
    fn empty_snapshot_renders_the_in_view_empty_state() {
        let v = view(Vec::new());
        let body: Vec<String> = v
            .lines()
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect();
        assert_eq!(body[0], "Background Tasks (0)");
        assert!(
            body.contains(&"No tasks currently running".to_string()),
            "empty state missing: {body:?}"
        );
        // Nothing to navigate or stop — only the close hint.
        assert!(!body.iter().any(|l| l.contains("stop task")));
        assert_eq!(body.last().unwrap(), "Esc close");
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
    fn refresh_preserves_selected_task_id_across_reorder_and_removal() {
        let mut v = view(vec![
            row("a", "running", "1"),
            row("b", "running", "2"),
            row("c", "running", "3"),
        ]);
        v.handle_key(press(KeyCode::Down));
        assert_eq!(v.rows()[v.selected].task_id, "b");

        v.apply_multiagent_event(&MultiAgentEvent::TasksRefreshed(vec![
            row("c", "running", "3"),
            row("a", "running", "1"),
            row("b", "completed", "2 done"),
        ]));
        assert_eq!(v.rows()[v.selected].task_id, "b");
        assert_eq!(v.rows()[v.selected].status, "completed");

        v.apply_multiagent_event(&MultiAgentEvent::TasksRefreshed(vec![
            row("c", "running", "3"),
            row("a", "running", "1"),
        ]));
        assert_eq!(
            v.selected, 1,
            "removed selection should clamp to a valid row"
        );
        assert_eq!(v.rows()[v.selected].task_id, "a");
    }

    #[test]
    fn refresh_during_an_optimistic_stop_keeps_the_same_task_selected() {
        let mut v = view(vec![
            row("a", "running", "first"),
            row("b", "running", "second"),
        ]);
        v.handle_key(press(KeyCode::Down));
        assert!(matches!(
            v.handle_key(press(KeyCode::Char('x'))),
            ViewOutcome::RunTaskAction(TaskAction::Kill { task_id }) if task_id == "b"
        ));

        // An unrelated task may settle while the off-loop kill is still in
        // flight. That refresh can carry the pre-kill status for `b`, but it
        // must not move the user's selection to a different task.
        v.apply_multiagent_event(&MultiAgentEvent::TasksRefreshed(vec![
            row("b", "running", "second"),
            row("a", "completed", "first done"),
        ]));
        assert_eq!(v.rows()[v.selected].task_id, "b");
    }

    #[test]
    fn mounted_tasks_view_applies_poller_refresh_events() {
        let mut stack = crate::bottom_pane::ViewStack::new();
        stack.push(Box::new(view(vec![row("a", "running", "old")])));
        stack.apply_multiagent_event(&MultiAgentEvent::TasksRefreshed(vec![row(
            "a",
            "completed",
            "new",
        )]));

        let mounted = stack
            .active()
            .and_then(|view| view.as_any().downcast_ref::<TasksView>())
            .expect("tasks view remains mounted");
        assert_eq!(mounted.rows()[0].status, "completed");
        assert_eq!(mounted.rows()[0].description, "new");
    }

    #[test]
    fn renders_fusion_stage_and_sanitized_failure_text() {
        let mut running = row("f00000001", "running", "compare sources");
        running.task_type = "local_fusion".into();
        running.stage = Some("Running panels 2/3".into());
        let mut failed = row("f00000002", "failed", "compare sources");
        failed.task_type = "local_fusion".into();
        failed.error = Some("provider\n\u{1b}[31mfailed\u{1b}[0m".into());

        let v = view(vec![running, failed]);
        let body: Vec<String> = v
            .lines()
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect();
        let rendered = body.join("\n");
        assert!(rendered.contains("Running panels 2/3"), "{rendered}");
        assert!(rendered.contains("provider failed"), "{rendered}");
        assert!(
            !rendered.contains('\u{1b}'),
            "ANSI escape leaked: {rendered:?}"
        );
        assert!(
            !rendered.contains("provider\n"),
            "failure remained multiline: {rendered:?}"
        );
    }

    #[test]
    fn ordinary_failed_rows_keep_the_existing_compact_rendering() {
        let mut ordinary = row("a00000001", "failed", "agent task");
        ordinary.task_type = "local_agent".into();
        ordinary.error = Some("ordinary detail".into());
        let rendered = view(vec![ordinary])
            .lines()
            .into_iter()
            .flat_map(|line| line.spans.into_iter().map(|span| span.content.into_owned()))
            .collect::<Vec<_>>();
        assert!(rendered.iter().any(|line| line.contains("agent task")));
        assert!(!rendered.iter().any(|line| line.contains("ordinary detail")));
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

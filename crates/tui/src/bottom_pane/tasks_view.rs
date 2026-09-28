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
//! per-task-type DETAIL/output sub-dialogs remain separate. The mounted list
//! refreshes against the live registry, including completed and parked agents.

use std::any::Any;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

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
    /// Current visible registry projection, grouped in oracle dialog order.
    rows: Vec<TaskRow>,
    registry: Option<Arc<dyn platform_api::task_registry::TaskRegistryHandle>>,
    parked_agents: HashSet<String>,
    monitors: HashSet<String>,
    last_refresh: Option<Instant>,
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
            registry: None,
            parked_agents: HashSet::new(),
            monitors: HashSet::new(),
            last_refresh: None,
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
    fn is_killable(&self, row: &TaskRow) -> bool {
        row.status == "running"
            || (row.task_type == "local_agent" && self.parked_agents.contains(&row.task_id))
    }

    /// Keep the mounted dialog synchronized with completion and rest transitions.
    #[must_use]
    pub fn with_registry(
        mut self,
        registry: Arc<dyn platform_api::task_registry::TaskRegistryHandle>,
    ) -> Self {
        self.registry = Some(registry);
        self.refresh(Instant::now());
        self
    }

    fn refresh(&mut self, now: Instant) {
        let Some(registry) = &self.registry else {
            return;
        };
        if self
            .last_refresh
            .is_some_and(|last| now.saturating_duration_since(last) < Duration::from_millis(200))
        {
            return;
        }
        self.last_refresh = Some(now);
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return;
        };
        let Ok(mut records) =
            runtime.block_on(registry.list(platform_api::task_registry::TaskListFilter::default()))
        else {
            return;
        };
        // 2.1.263 Cj/Vp: completed resumable agents are the explicit
        // exception; terminal shells and foreground-only rows are hidden.
        records.retain(|record| {
            record.completed_agent_visible
                || (matches!(record.status.as_str(), "running" | "pending")
                    && record.is_backgrounded != Some(false))
        });
        self.monitors = records
            .iter()
            .filter(|record| record.kind.as_deref() == Some("monitor"))
            .map(|record| record.task_id.clone())
            .collect();
        let selected_id = self.rows.get(self.selected).map(|row| row.task_id.clone());
        self.parked_agents = records
            .iter()
            .filter(|record| record.is_parked)
            .map(|record| record.task_id.clone())
            .collect();
        self.rows = records
            .into_iter()
            .map(tui_core::multiagent::task_row_from_record)
            .collect();
        let monitors = &self.monitors;
        self.rows.sort_by_key(|row| Self::group(row, monitors).0);
        self.selected = selected_id
            .and_then(|id| self.rows.iter().position(|row| row.task_id == id))
            .unwrap_or(self.selected.min(self.rows.len().saturating_sub(1)));
    }

    fn group(row: &TaskRow, monitors: &HashSet<String>) -> (u8, &'static str) {
        match row.task_type.as_str() {
            "in_process_teammate" => (0, "Agents"),
            "local_bash" if !monitors.contains(&row.task_id) => (1, "Shells"),
            "local_bash" | "monitor_mcp" | "monitor_ws" => (2, "Monitors"),
            "mcp_task" => (3, "MCP tasks"),
            "remote_agent" => (4, "Cloud agents"),
            "local_agent" if row.status == "completed" => (6, "Completed"),
            "local_agent" => (5, "Local agents"),
            "local_workflow" => (7, "Dynamic workflows"),
            "dream" => (8, ""),
            "auto_mode_scan" => (9, ""),
            _ => (10, "Other tasks"),
        }
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
        self.lines_at_width(None)
    }

    fn lines_at_width(&self, _width: Option<u16>) -> Vec<Line<'static>> {
        let dim = crate::style_adapter::to_ratatui(self.theme.dim);
        let accent = crate::style_adapter::to_ratatui(self.theme.suggestion);
        let dim_style = Style::default().fg(dim);
        let mut lines: Vec<Line<'static>> = Vec::with_capacity(self.rows.len() + 4);
        lines.push(Line::from(Span::styled(
            "Background",
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
        let mut previous_group = None;
        for (i, r) in self.rows.iter().enumerate() {
            let group = Self::group(r, &self.monitors);
            if previous_group != Some(group.0) {
                if previous_group.is_some() {
                    lines.push(Line::from(""));
                }
                if !group.1.is_empty()
                    && !(group.0 == 1
                        && self
                            .rows
                            .iter()
                            .all(|row| Self::group(row, &self.monitors).0 == 1))
                {
                    let count = self
                        .rows
                        .iter()
                        .filter(|row| Self::group(row, &self.monitors).0 == group.0)
                        .count();
                    lines.push(Line::from(Span::styled(
                        format!("{} ({count})", group.1),
                        dim_style,
                    )));
                }
                previous_group = Some(group.0);
            }
            let marker = if i == self.selected {
                "\u{276f} "
            } else {
                "  "
            };
            let label = if self.monitors.contains(&r.task_id) {
                r.description.clone()
            } else {
                r.command.clone().unwrap_or_else(|| r.description.clone())
            };
            let status = if r.awaiting_plan_approval {
                "awaiting approval"
            } else if r.task_type == "local_agent" && r.status == "completed" {
                "done"
            } else {
                &r.status
            };
            let unread = if r.status == "completed" && r.unread {
                ", unread"
            } else {
                ""
            };
            let model = r
                .model
                .as_deref()
                .filter(|model| !model.is_empty())
                .map(|model| {
                    let effort = r
                        .effort
                        .as_deref()
                        .filter(|effort| !effort.is_empty())
                        .map(|effort| format!(" ({effort})"))
                        .unwrap_or_default();
                    format!(" · {model}{effort}")
                })
                .unwrap_or_default();
            // A running Fusion row says which stage it is in, and a failed one
            // says why, both sanitized: the stage and error are model- and
            // provider-derived text reaching a terminal.
            let fusion = if r.task_type == "local_fusion" && r.status == "running" {
                r.stage
                    .as_deref()
                    .map(sanitize_task_text)
                    .filter(|stage| !stage.is_empty())
                    .map(|stage| format!(" [{stage}]"))
                    .unwrap_or_default()
            } else if r.task_type == "local_fusion" && r.status == "failed" {
                r.error
                    .as_deref()
                    .map(sanitize_task_text)
                    .filter(|error| !error.is_empty())
                    .map(|error| format!(" \u{2014} {error}"))
                    .unwrap_or_default()
            } else {
                String::new()
            };
            // A Fusion row's stage is the point of the row, and it renders
            // after the label. An unbounded label pushes it past the right
            // edge at ordinary terminal widths, so bound it the way
            // `local_agent` rows are already bounded.
            let label = if r.task_type == "local_fusion" && !fusion.is_empty() {
                tui_core::render::truncate_to_width_ellipsis(&label, 40)
            } else {
                label
            };
            let (label, model) = if r.task_type == "local_agent" {
                let model = tui_core::render::truncate_to_width_ellipsis(&model, 31);
                let model_width = unicode_width::UnicodeWidthStr::width(model.as_str());
                if model_width > 0 && 40usize.saturating_sub(model_width) >= 20 {
                    (
                        tui_core::render::truncate_to_width_ellipsis(&label, 40 - model_width),
                        model,
                    )
                } else {
                    (
                        tui_core::render::truncate_to_width_ellipsis(&label, 40),
                        String::new(),
                    )
                }
            } else {
                (label, String::new())
            };
            let text = format!(
                "{marker}{label} {} {status}{unread}{fusion}{model}",
                Self::glyph(&r.status)
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
            if self
                .rows
                .get(self.selected)
                .is_some_and(|row| self.is_killable(row))
            {
                "\u{2191}\u{2193} navigate \u{00b7} x stop task \u{00b7} Esc close"
            } else {
                "\u{2191}\u{2193} navigate \u{00b7} Esc close"
            },
            dim_style,
        )));
        lines
    }

    /// Line index of the selected row (header + spacer occupy 2 lines).
    fn selected_line_index(&self) -> usize {
        self.lines()
            .iter()
            .position(|line| line.to_string().starts_with("❯ "))
            .unwrap_or(2)
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
        let lines = self.lines_at_width(Some(inner.width));
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
    fn handle_tick(&mut self, now: Instant) -> ViewOutcome {
        self.refresh(now);
        ViewOutcome::Pending
    }

    fn needs_redraw(&self) -> bool {
        self.registry.is_some()
    }

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
            KeyCode::Char('x') | KeyCode::Char('d') | KeyCode::Delete
                if key.modifiers.is_empty() =>
            {
                match self.rows.get(self.selected) {
                    Some(r) if self.is_killable(r) => {
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
            unread: false,
            model: None,
            effort: None,
            awaiting_plan_approval: false,
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

    fn rendered_buffer(v: &TasksView, width: u16, height: u16) -> String {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        v.render(area, &mut buf);
        (area.top()..area.bottom())
            .map(|y| {
                (area.left()..area.right())
                    .map(|x| {
                        buf.cell(ratatui::layout::Position::new(x, y))
                            .map_or(" ", ratatui::buffer::Cell::symbol)
                            .to_string()
                    })
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The agents-view entry opens this picker with nothing running, so the
    /// oracle's in-dialog empty body has to render here (Background dialog:
    /// `children: D.length === 0 ? "No tasks currently running" : …`).
    #[test]
    fn plan_approval_label_clears_after_approval_or_rejection() {
        let mut task = row("t12345678", "running", "Review API");
        task.task_type = "in_process_teammate".into();
        task.awaiting_plan_approval = true;
        let pending: Vec<String> = view(vec![task.clone()])
            .lines()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert!(pending
            .iter()
            .any(|line| line.contains("awaiting approval") && line.contains("Review API")));
        for _decision in ["approved", "rejected"] {
            task.awaiting_plan_approval = false;
            let resolved: Vec<String> = view(vec![task.clone()])
                .lines()
                .iter()
                .map(ToString::to_string)
                .collect();
            assert!(!resolved
                .iter()
                .any(|line| line.contains("awaiting approval")));
            assert!(resolved
                .iter()
                .any(|line| line.contains("running") && line.contains("Review API")));
        }
    }

    #[test]
    fn completed_agent_row_shows_unread_and_real_model_effort() {
        let mut agent = row("agent1", "completed", "inspect files");
        agent.task_type = "local_agent".into();
        agent.unread = true;
        agent.model = Some("Sonnet".into());
        agent.effort = Some("high".into());
        let text = view(vec![agent])
            .lines()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("done, unread · Sonnet (high)"), "{text}");
        assert!(!text.contains("parked"));
    }

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
        assert_eq!(body[0], "Background");
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
        assert!(text.contains("Background"), "{text}");
        assert!(!text.contains("Background Tasks"), "{text}");
        assert!(text.contains("cargo build"), "{text}");
    }

    #[test]
    fn fusion_state_and_failure_prefix_survive_long_descriptions_at_real_widths() {
        let descriptions = [
            "a very long ASCII description ".repeat(12),
            "长描述 ".repeat(80),
        ];

        for width in [80, 100] {
            for description in &descriptions {
                let mut running = row("f00000001", "running", description);
                running.task_type = "local_fusion".into();
                running.stage = Some("Running panels 2/3".into());
                let running_text = rendered_buffer(&view(vec![running]), width, 8);
                assert!(
                    running_text.contains("Running panels 2/3"),
                    "running Fusion state clipped at {width} columns: {running_text:?}"
                );

                let mut failed = row("f00000002", "failed", description);
                failed.task_type = "local_fusion".into();
                failed.error = Some("provider\n\u{1b}[31mfailed\u{1b}[0m".into());
                let failed_text = rendered_buffer(&view(vec![failed]), width, 8);
                assert!(
                    failed_text.contains("— provider failed"),
                    "sanitized Fusion failure clipped at {width} columns: {failed_text:?}"
                );
                assert!(
                    !failed_text.contains('\u{1b}'),
                    "ANSI escape leaked into the Buffer: {failed_text:?}"
                );
            }
        }
    }

    #[test]
    fn queued_and_pending_tasks_cannot_be_stopped_but_parked_agents_can() {
        for status in ["queued", "pending"] {
            let mut v = view(vec![row("b1", status, "waiting")]);
            assert!(matches!(
                v.handle_key(press(KeyCode::Char('x'))),
                ViewOutcome::Pending
            ));
        }
        let mut parked = row("agent1", "completed", "resting");
        parked.task_type = "local_agent".into();
        let mut v = view(vec![parked]);
        v.parked_agents.insert("agent1".into());
        assert!(matches!(
            v.handle_key(press(KeyCode::Char('x'))),
            ViewOutcome::RunTaskAction(TaskAction::Kill { .. })
        ));
    }

    struct LiveRegistry(std::sync::Mutex<Vec<platform_api::task_registry::TaskRecord>>);
    #[async_trait::async_trait]
    impl platform_api::task_registry::TaskRegistryHandle for LiveRegistry {
        async fn create(
            &self,
            _i: platform_api::task_registry::TaskCreateInput,
        ) -> Result<
            platform_api::task_registry::TaskRecord,
            platform_api::task_registry::TaskRegistryError,
        > {
            unreachable!()
        }
        async fn get(
            &self,
            _id: &str,
        ) -> Result<
            Option<platform_api::task_registry::TaskRecord>,
            platform_api::task_registry::TaskRegistryError,
        > {
            unreachable!()
        }
        async fn list(
            &self,
            _f: platform_api::task_registry::TaskListFilter,
        ) -> Result<
            Vec<platform_api::task_registry::TaskRecord>,
            platform_api::task_registry::TaskRegistryError,
        > {
            Ok(self.0.lock().unwrap().clone())
        }
        async fn update(
            &self,
            _id: &str,
            _p: platform_api::task_registry::TaskUpdatePatch,
        ) -> Result<
            platform_api::task_registry::TaskRecord,
            platform_api::task_registry::TaskRegistryError,
        > {
            unreachable!()
        }
        async fn set_status(
            &self,
            _id: &str,
            _s: &str,
        ) -> Result<
            platform_api::task_registry::TaskRecord,
            platform_api::task_registry::TaskRegistryError,
        > {
            unreachable!()
        }
        async fn kill(
            &self,
            _id: &str,
        ) -> Result<
            platform_api::task_registry::TaskRecord,
            platform_api::task_registry::TaskRegistryError,
        > {
            unreachable!()
        }
        async fn output(
            &self,
            _id: &str,
            _o: Option<u64>,
        ) -> Result<
            platform_api::task_registry::TaskOutputChunk,
            platform_api::task_registry::TaskRegistryError,
        > {
            unreachable!()
        }
    }

    #[test]
    fn fusion_refresh_updates_selected_stage_in_the_rendered_buffer() {
        let mut first = row("a00000001", "running", "ordinary task");
        first.task_type = "local_agent".into();
        let mut fusion = row("f00000002", "running", "long Fusion description");
        fusion.task_type = "local_fusion".into();
        fusion.stage = Some("Running panels 1/3".into());
        let mut v = view(vec![first, fusion]);
        v.handle_key(press(KeyCode::Down));

        assert!(rendered_buffer(&v, 80, 8).contains("Running panels 1/3"));
        v.apply_multiagent_event(&MultiAgentEvent::TasksRefreshed(vec![
            row("a00000001", "running", "ordinary task"),
            TaskRow {
                unread: false,
                model: None,
                effort: None,
                awaiting_plan_approval: false,
                task_id: "f00000002".into(),
                task_type: "local_fusion".into(),
                status: "running".into(),
                description: "long Fusion description".into(),
                command: None,
                stage: Some("Running panels 2/3".into()),
                error: None,
            },
        ]));

        assert_eq!(v.rows()[v.selected].task_id, "f00000002");
        let rendered = rendered_buffer(&v, 80, 8);
        assert!(rendered.contains("Running panels 2/3"), "{rendered:?}");
        assert!(!rendered.contains("Running panels 1/3"), "{rendered:?}");
    }

    #[test]
    fn fusion_rendering_is_safe_for_narrow_buffers() {
        let mut running = row("f00000001", "running", "narrow");
        running.task_type = "local_fusion".into();
        running.stage = Some("Running panels 2/3".into());
        let mut failed = row("f00000002", "failed", "narrow");
        failed.task_type = "local_fusion".into();
        failed.error = Some("provider\n\u{1b}[31mfailed\u{1b}[0m".into());
        for width in [0, 1, 2, 3, 4, 5] {
            let _ = rendered_buffer(&view(vec![running.clone(), failed.clone()]), width, 8);
        }
    }

    #[test]
    /// The command, not the description, names a Fusion row. The grapheme half
    /// of this test went with `row_line`: `main`'s renderer truncates only
    /// `local_agent` rows, so a Fusion label is never cut here at all.
    fn fusion_label_keeps_command_precedence() {
        let mut fusion = row(
            "f00000001",
            "running",
            "description-must-not-replace-command",
        );
        fusion.task_type = "local_fusion".into();
        fusion.command =
            Some("e\u{301}e\u{301}e\u{301} \u{1f469}\u{200d}\u{1f4bb} command tail".into());
        fusion.stage = Some("P".into());

        let rendered = rendered_buffer(&view(vec![fusion]), 120, 8);
        assert!(rendered.contains("[P]"), "{rendered:?}");
        assert!(rendered.contains("command tail"), "{rendered:?}");
        assert!(
            !rendered.contains("description-must-not-replace-command"),
            "command precedence changed: {rendered:?}"
        );
    }

    #[test]
    fn mounted_dialog_refreshes_registry_and_preserves_selected_identity() {
        let registry = Arc::new(LiveRegistry(std::sync::Mutex::new(vec![])));
        let mut view = TasksView::new(vec![], Theme::dark()).with_registry(registry.clone());
        let mut record = platform_api::task_registry::TaskRecord {
            task_id: "agent1".into(),
            task_type: "local_agent".into(),
            status: "running".into(),
            description: "working".into(),
            ..Default::default()
        };
        let finished = platform_api::task_registry::TaskRecord {
            task_id: "shell-done".into(),
            task_type: "local_bash".into(),
            status: "completed".into(),
            ..Default::default()
        };
        let foreground = platform_api::task_registry::TaskRecord {
            task_id: "shell-fg".into(),
            task_type: "local_bash".into(),
            status: "running".into(),
            is_backgrounded: Some(false),
            ..Default::default()
        };
        *registry.0.lock().unwrap() = vec![record.clone(), finished, foreground];
        let now = Instant::now() + Duration::from_secs(1);
        view.handle_tick(now);
        assert_eq!(
            view.rows().len(),
            1,
            "terminal shells and foreground rows stay out of the dialog"
        );
        assert_eq!(view.rows()[0].status, "running");
        record.status = "completed".into();
        record.is_parked = true;
        record.is_backgrounded = Some(true);
        // The live registry computes Cj eligibility, rather than the dialog
        // treating every parked row as a visible completed task.
        record.completed_agent_visible = true;
        *registry.0.lock().unwrap() = vec![record];
        view.handle_tick(now + Duration::from_secs(1));
        assert_eq!(view.rows()[0].status, "completed");
        assert!(
            view.is_killable(&view.rows()[0]),
            "a parked persistent agent remains stoppable"
        );
        registry.0.lock().unwrap().clear();
        view.handle_tick(now + Duration::from_secs(2));
        assert!(view.rows().is_empty());
        assert!(view
            .lines()
            .iter()
            .any(|line| line.to_string() == "No tasks currently running"));
    }
}

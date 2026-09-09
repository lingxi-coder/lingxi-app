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
use tui_core::multiagent::TaskRow;
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
                "{marker}{label} {} {status}{unread}{model}",
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
        }
    }

    fn view(rows: Vec<TaskRow>) -> TasksView {
        TasksView::new(rows, Theme::dark())
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

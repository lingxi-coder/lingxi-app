//! `/workflows`: an interactive picker of workflow runs — claude-code's
//! `local-jsx` "Dynamic workflows" dialog (`zoa` list + `Rcr` detail).
//!
//! Rendered as a full-frame [`BottomPaneView`] with the same contract as
//! [`crate::bottom_pane::tasks_view::TasksView`] (workflow runs ARE background
//! tasks, `task_type == "local_workflow"`). The list is a snapshot of the live
//! `TaskRegistry` taken in [`crate::chat_widget::ChatWidget::cmd_workflows`],
//! filtered to workflow runs and enriched with each run's `wf_…` id, start/end
//! wall-clock, and the agent-count + phase/agent tree parsed from its output
//! spool ([`tui_core::multiagent::parse_workflow_spool`]).
//!
//! Layout mirrors the oracle: title "Dynamic workflows", a `N running · M
//! completed` subtitle, one flat newest-first list, per-row `{glyph} {name}
//! {meta}` (meta = `{agents} · {elapsed}`, dim). Interaction: `↑`/`↓` move;
//! `Enter` opens the run's phase/agent detail; `x`/`d`/`Delete` on a RUNNING run
//! stops it OFF-LOOP ([`ViewOutcome::RunTaskAction`] → `TaskRegistryHandle::kill`,
//! the seam `/tasks` uses) and marks the row `killed`, keeping the picker open;
//! `Esc`/`q` closes.
//!
//! The list is a snapshot at open time (re-run `/workflows` to refresh). The
//! oracle's third-level agent-transcript drill-down and the `s save` flow are
//! not ported; the detail view shows the phase/agent tree (its primary value).

use std::any::Any;
use std::time::{SystemTime, UNIX_EPOCH};

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};
use tui_core::multiagent::{WorkflowPhase, WorkflowRow};
use tui_core::theme::Theme;

use crate::bottom_pane::view::{BottomPaneView, TaskAction, ViewOutcome};
use crate::renderable::Renderable;

/// Current wall clock in epoch millis (`0` if the clock is before the epoch).
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Format an elapsed span (millis) compactly: `45s`, `1m 23s`, `2h 3m`.
fn format_elapsed(ms: u64) -> String {
    let secs = ms / 1000;
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    }
}

/// Elapsed for a row: `ended - started` for a terminal run, else `now - started`
/// (so a running workflow's timer grows on each render). `None` if no start.
fn row_elapsed(row: &WorkflowRow) -> Option<String> {
    let started = row.started_at_ms?;
    let end = row.ended_at_ms.unwrap_or_else(now_ms);
    Some(format_elapsed(end.saturating_sub(started)))
}

/// A run's display name, `meta.name` preferred, capped at 50 chars (49 + `…`).
fn row_name(row: &WorkflowRow) -> String {
    let raw = if row.name.is_empty() {
        "Dynamic workflow"
    } else {
        row.name.as_str()
    };
    if raw.chars().count() > 50 {
        let head: String = raw.chars().take(49).collect();
        format!("{head}\u{2026}")
    } else {
        raw.to_string()
    }
}

/// Whether `status` denotes a still-running run that can be stopped.
fn is_running(status: &str) -> bool {
    matches!(status, "running" | "pending" | "queued")
}

/// The list-row status glyph (oracle `Voa`): `✔` completed, `✘` failed/killed,
/// `⟳` otherwise (running/pending).
fn list_glyph(status: &str) -> &'static str {
    match status {
        "completed" => "\u{2714}",         // ✔
        "failed" | "killed" => "\u{2718}", // ✘
        _ => "\u{27f3}",                   // ⟳
    }
}

/// The `/workflows` interactive run picker (list mode).
pub struct WorkflowsView {
    /// Snapshot of the runs (newest-first as the registry orders them).
    rows: Vec<WorkflowRow>,
    /// Index of the highlighted row.
    selected: usize,
    /// Active render palette (accent header + dim footer hint).
    theme: Theme,
}

impl WorkflowsView {
    /// Build the picker over `rows`, selecting the first run.
    #[must_use]
    pub fn new(rows: Vec<WorkflowRow>, theme: Theme) -> Self {
        Self {
            rows,
            selected: 0,
            theme,
        }
    }

    /// The dim `N running · M completed` subtitle (oracle `epr`/`Lnn`). Empty
    /// when there are no runs.
    fn subtitle(&self) -> String {
        if self.rows.is_empty() {
            return String::new();
        }
        let running = self.rows.iter().filter(|r| r.status == "running").count();
        let completed = self.rows.len() - running;
        let mut parts = Vec::new();
        if running > 0 {
            parts.push(format!("{running} running"));
        }
        if completed > 0 {
            parts.push(format!("{completed} completed"));
        }
        parts.join(" \u{00b7} ")
    }

    /// The dim per-row meta: `{agents} · {elapsed}` (each shown only if present).
    fn row_meta(row: &WorkflowRow) -> String {
        let mut parts = Vec::new();
        if row.agent_count > 0 {
            let noun = if row.agent_count == 1 { "agent" } else { "agents" };
            parts.push(format!("{} {noun}", row.agent_count));
        }
        if let Some(e) = row_elapsed(row) {
            parts.push(e);
        }
        parts.join(" \u{00b7} ")
    }

    /// Rendered body lines: title, subtitle, spacer, one line per run, spacer,
    /// footer hint.
    fn lines(&self) -> Vec<Line<'static>> {
        let dim = crate::style_adapter::to_ratatui(self.theme.dim);
        let accent = crate::style_adapter::to_ratatui(self.theme.suggestion);
        let dim_style = Style::default().fg(dim);
        let mut lines: Vec<Line<'static>> = Vec::with_capacity(self.rows.len() + 5);
        lines.push(Line::from(Span::styled(
            "Dynamic workflows".to_string(),
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        )));
        let subtitle = self.subtitle();
        if !subtitle.is_empty() {
            lines.push(Line::from(Span::styled(subtitle, dim_style)));
        }
        lines.push(Line::from(""));
        if self.rows.is_empty() {
            lines.push(Line::from(Span::styled(
                "No dynamic workflows in this session.",
                dim_style,
            )));
        }
        for (i, r) in self.rows.iter().enumerate() {
            let marker = if i == self.selected {
                "\u{276f} "
            } else {
                "  "
            };
            let meta = Self::row_meta(r);
            let meta = if meta.is_empty() {
                String::new()
            } else {
                format!("  \u{00b7}  {meta}")
            };
            let text = format!("{marker}{} {}{meta}", list_glyph(&r.status), row_name(r));
            let style = if i == self.selected {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            lines.push(Line::from(Span::styled(text, style)));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(self.footer(), dim_style)));
        lines
    }

    /// The chord-hint footer (oracle input guide): `x stop` only when the
    /// selected run is running.
    fn footer(&self) -> String {
        let mut parts = Vec::new();
        if !self.rows.is_empty() {
            parts.push("\u{2191}\u{2193} select".to_string());
            parts.push("Enter view".to_string());
            if self
                .rows
                .get(self.selected)
                .is_some_and(|r| is_running(&r.status))
            {
                parts.push("x stop".to_string());
            }
        }
        parts.push("Esc close".to_string());
        parts.join(" \u{00b7} ")
    }

    /// Line index of the selected row (title + optional subtitle + spacer).
    fn selected_line_index(&self) -> usize {
        let header = if self.subtitle().is_empty() { 2 } else { 3 };
        self.selected.saturating_add(header)
    }

    /// Scroll offset keeping the selected row visible in a `viewport`-tall area.
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
    pub fn rows(&self) -> &[WorkflowRow] {
        &self.rows
    }
}

impl Renderable for WorkflowsView {
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

impl BottomPaneView for WorkflowsView {
    fn handle_key(&mut self, key: KeyEvent) -> ViewOutcome {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => ViewOutcome::Cancelled,
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
            KeyCode::Enter => match self.rows.get(self.selected) {
                Some(r) => {
                    ViewOutcome::OpenView(Box::new(WorkflowDetailView::new(r.clone(), self.theme)))
                }
                None => ViewOutcome::Cancelled,
            },
            KeyCode::Char('x') | KeyCode::Char('d') | KeyCode::Delete => {
                match self.rows.get(self.selected) {
                    Some(r) if is_running(&r.status) => {
                        let task_id = r.task_id.clone();
                        // Optimistic: reflect the stop immediately; the real
                        // kill runs off-loop and its result lands in the
                        // transcript via `TurnEvent::SystemNotice`.
                        self.rows[self.selected].status = "killed".to_string();
                        ViewOutcome::RunTaskAction(TaskAction::Kill { task_id })
                    }
                    _ => ViewOutcome::Pending,
                }
            }
            _ => ViewOutcome::Pending,
        }
    }

    /// A full-frame picker owns the whole viewport (same contract as `/tasks`).
    fn wants_status_line(&self) -> bool {
        false
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The human label for a parsed agent lifecycle state (oracle status labels).
fn agent_state_label(state: &str) -> &'static str {
    match state {
        "done" => "Completed",
        "error" => "Failed",
        "cached" => "Cached",
        "start" => "Running",
        _ => "Queued",
    }
}

/// Count of finished agents in a phase (done or served-from-cache).
fn phase_done(phase: &WorkflowPhase) -> usize {
    phase
        .agents
        .iter()
        .filter(|a| matches!(a.state.as_str(), "done" | "cached"))
        .count()
}

/// Read-only phase/agent detail for one run, pushed by `Enter` on the picker.
/// Mirrors the oracle detail's phase list (`{glyph} {title} {done}/{total}`)
/// with each phase's agents beneath. `Esc`/`Enter`/`q` returns to the list.
pub struct WorkflowDetailView {
    row: WorkflowRow,
    theme: Theme,
}

impl WorkflowDetailView {
    /// Build a detail view over one run snapshot.
    #[must_use]
    pub fn new(row: WorkflowRow, theme: Theme) -> Self {
        Self { row, theme }
    }

    fn lines(&self) -> Vec<Line<'static>> {
        let dim = crate::style_adapter::to_ratatui(self.theme.dim);
        let accent = crate::style_adapter::to_ratatui(self.theme.suggestion);
        let dim_style = Style::default().fg(dim);
        let label = |k: &str, v: String| -> Line<'static> {
            Line::from(vec![Span::styled(format!("{k:<10}"), dim_style), Span::raw(v)])
        };

        let mut lines = vec![
            Line::from(Span::styled(
                format!("Workflow \u{00b7} {}", row_name(&self.row)),
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            label("Status", self.row.status.clone()),
            label(
                "Run id",
                self.row.run_id.clone().unwrap_or_else(|| "\u{2014}".to_string()),
            ),
            label("Agents", self.row.agent_count.to_string()),
            label(
                "Elapsed",
                row_elapsed(&self.row).unwrap_or_else(|| "\u{2014}".to_string()),
            ),
        ];
        if !self.row.description.is_empty() {
            lines.push(label("Script", self.row.description.clone()));
        }

        lines.push(Line::from(""));
        if self.row.phases.is_empty() {
            lines.push(Line::from(Span::styled("No agents yet.", dim_style)));
        } else {
            lines.push(Line::from(Span::styled(
                "Phases".to_string(),
                Style::default().add_modifier(Modifier::BOLD),
            )));
            for phase in &self.row.phases {
                let done = phase_done(phase);
                let total = phase.agents.len();
                let glyph = if total > 0 && done == total {
                    "\u{2714}" // ✔ all done
                } else if phase.agents.iter().any(|a| a.state == "error") {
                    "\u{2718}" // ✘ a failure
                } else {
                    "\u{27f3}" // ⟳ in progress / no agents
                };
                let title = if phase.title.is_empty() {
                    format!("Phase {}", phase.index.saturating_add(1))
                } else {
                    phase.title.clone()
                };
                lines.push(Line::from(format!("  {glyph} {title}  {done}/{total}")));
                for agent in &phase.agents {
                    let name = if agent.label.is_empty() {
                        "agent"
                    } else {
                        agent.label.as_str()
                    };
                    lines.push(Line::from(Span::styled(
                        format!("      {name} \u{00b7} {}", agent_state_label(&agent.state)),
                        dim_style,
                    )));
                }
            }
        }

        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("Esc to go back", dim_style)));
        lines
    }
}

impl Renderable for WorkflowDetailView {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        Clear.render(area, buf);
        let block = Block::new().borders(Borders::ALL);
        let inner = block.inner(area);
        block.render(area, buf);
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        Paragraph::new(self.lines()).render(inner, buf);
    }

    fn desired_height(&self, _width: u16) -> u16 {
        u16::try_from(self.lines().len())
            .unwrap_or(u16::MAX)
            .saturating_add(2)
    }
}

impl BottomPaneView for WorkflowDetailView {
    fn handle_key(&mut self, key: KeyEvent) -> ViewOutcome {
        match key.code {
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => ViewOutcome::Cancelled,
            _ => ViewOutcome::Pending,
        }
    }

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
    use tui_core::multiagent::{WorkflowAgentRow, WorkflowPhase};

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn row(id: &str, status: &str, name: &str) -> WorkflowRow {
        WorkflowRow {
            task_id: id.to_string(),
            run_id: Some(format!("wf_{id}")),
            name: name.to_string(),
            status: status.to_string(),
            description: "script summary".to_string(),
            current_step: 1,
            started_at_ms: Some(1_000),
            ended_at_ms: None,
            agent_count: 3,
            phases: vec![WorkflowPhase {
                index: 0,
                title: "Scan".to_string(),
                agents: vec![
                    WorkflowAgentRow {
                        label: "grep".to_string(),
                        state: "done".to_string(),
                    },
                    WorkflowAgentRow {
                        label: "scan".to_string(),
                        state: "start".to_string(),
                    },
                ],
            }],
        }
    }

    fn view(rows: Vec<WorkflowRow>) -> WorkflowsView {
        WorkflowsView::new(rows, Theme::dark())
    }

    #[test]
    fn enter_opens_a_detail_view() {
        let mut v = view(vec![row("w00000001", "running", "deploy")]);
        assert!(matches!(
            v.handle_key(press(KeyCode::Enter)),
            ViewOutcome::OpenView(_)
        ));
    }

    #[test]
    fn stop_on_a_running_run_emits_kill_and_marks_killed() {
        let mut v = view(vec![row("w00000001", "running", "deploy")]);
        match v.handle_key(press(KeyCode::Char('x'))) {
            ViewOutcome::RunTaskAction(TaskAction::Kill { task_id }) => {
                assert_eq!(task_id, "w00000001");
            }
            _ => panic!("expected RunTaskAction(Kill)"),
        }
        assert_eq!(v.rows()[0].status, "killed");
    }

    #[test]
    fn stop_on_a_terminal_run_is_ignored() {
        let mut v = view(vec![row("w1", "completed", "done")]);
        assert!(matches!(
            v.handle_key(press(KeyCode::Char('x'))),
            ViewOutcome::Pending
        ));
        assert_eq!(v.rows()[0].status, "completed");
    }

    #[test]
    fn esc_cancels_the_picker() {
        let mut v = view(vec![row("w1", "running", "x")]);
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
        v.handle_key(press(KeyCode::Down)); // clamp at last row
        assert_eq!(v.selected, 1);
        v.handle_key(press(KeyCode::Up));
        assert_eq!(v.selected, 0);
    }

    #[test]
    fn subtitle_counts_running_and_completed() {
        let v = view(vec![
            row("a", "running", "1"),
            row("b", "completed", "2"),
            row("c", "completed", "3"),
        ]);
        assert_eq!(v.subtitle(), "1 running \u{00b7} 2 completed");
    }

    #[test]
    fn footer_shows_stop_only_when_selected_run_is_running() {
        let running = view(vec![row("a", "running", "1")]);
        assert!(running.footer().contains("x stop"));
        let done = view(vec![row("a", "completed", "1")]);
        assert!(!done.footer().contains("x stop"));
    }

    #[test]
    fn long_name_is_truncated_to_50() {
        let long = "x".repeat(80);
        let r = row("a", "running", &long);
        let name = row_name(&r);
        assert_eq!(name.chars().count(), 50);
        assert!(name.ends_with('\u{2026}'));
    }

    #[test]
    fn elapsed_formats_compactly() {
        assert_eq!(format_elapsed(45_000), "45s");
        assert_eq!(format_elapsed(83_000), "1m 23s");
        assert_eq!(format_elapsed(7_380_000), "2h 3m");
    }

    #[test]
    fn full_frame_contract_suppresses_the_status_line() {
        assert!(!view(vec![row("a", "running", "x")]).wants_status_line());
    }

    #[test]
    fn detail_renders_phase_and_agents() {
        let v = WorkflowDetailView::new(row("w1", "running", "deploy"), Theme::dark());
        let area = Rect::new(0, 0, 70, 20);
        let mut buf = Buffer::empty(area);
        v.render(area, &mut buf);
        let text = buf_text(&buf, area);
        assert!(text.contains("Scan"), "{text}");
        assert!(text.contains("1/2"), "one of two agents done: {text}");
        assert!(text.contains("Completed"), "agent state label: {text}");
    }

    #[test]
    fn renders_title_and_run_into_the_buffer() {
        let v = view(vec![row("w00000001", "running", "deploy-site")]);
        let area = Rect::new(0, 0, 70, 12);
        let mut buf = Buffer::empty(area);
        v.render(area, &mut buf);
        let text = buf_text(&buf, area);
        assert!(text.contains("Dynamic workflows"), "{text}");
        assert!(text.contains("deploy-site"), "{text}");
        assert!(text.contains("3 agents"), "{text}");
    }

    #[test]
    fn empty_list_shows_the_empty_state() {
        let v = view(vec![]);
        let area = Rect::new(0, 0, 70, 8);
        let mut buf = Buffer::empty(area);
        v.render(area, &mut buf);
        let text = buf_text(&buf, area);
        assert!(text.contains("No dynamic workflows in this session"), "{text}");
    }

    fn buf_text(buf: &Buffer, area: Rect) -> String {
        (area.top()..area.bottom())
            .flat_map(|y| {
                (area.left()..area.right()).map(move |x| {
                    buf.cell(ratatui::layout::Position::new(x, y))
                        .map_or(" ", ratatui::buffer::Cell::symbol)
                        .to_string()
                })
            })
            .collect()
    }
}

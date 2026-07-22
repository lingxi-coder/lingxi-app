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
//! oracle's third-level agent-transcript drill-down is not ported; the detail
//! view shows the phase/agent tree (its primary value). The `s` "Save dynamic
//! workflow" flow IS ported ([`WorkflowSaveView`]): when the selected run
//! carries an inline `script`, `s` opens a name/scope form that writes the
//! script to `.lingxi/workflows/<name>.js` (project) or the user config dir
//! (user) via [`tool_workflow::save_dynamic_workflow`].

use std::any::Any;
use std::cell::Cell;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};
use tool_workflow::{
    sanitize_workflow_name, save_dynamic_workflow, saved_feedback, WorkflowSaveError, WorkflowScope,
};
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

/// A run's display name (oracle `Hvp = workflowName ?? summary ?? description`),
/// capped at 50 chars (49 + `…`). LingXi: `name` (workflow_id / meta.name), then
/// the script `description` summary, then the literal fallback.
fn row_name(row: &WorkflowRow) -> String {
    let raw = if !row.name.is_empty() {
        row.name.as_str()
    } else if !row.description.is_empty() {
        row.description.as_str()
    } else {
        "Dynamic workflow"
    };
    if raw.chars().count() > 50 {
        let head: String = raw.chars().take(49).collect();
        format!("{head}\u{2026}")
    } else {
        raw.to_string()
    }
}

/// The default name pre-filled into the "Save dynamic workflow" form (oracle
/// `dQ_ = workflowName ?? summary`): the run's `name` (meta.name), else its
/// script `description` summary, else the literal `"workflow"`.
fn save_default_name(row: &WorkflowRow) -> String {
    if !row.name.is_empty() {
        row.name.clone()
    } else if !row.description.is_empty() {
        row.description.clone()
    } else {
        "workflow".to_string()
    }
}

/// Whether `status` denotes a run that can be stopped. Oracle `Rcr`/`zoa` gate
/// the `x` chord on `status === "running"` exactly (a `pending` run is not yet
/// stoppable), so match that — and keep it consistent with the subtitle's
/// running/completed split.
fn is_running(status: &str) -> bool {
    status == "running"
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

/// Chrome lines around the windowed row list (title + subtitle + 2 spacers +
/// footer) plus a 2-line reserve for the `↑/↓ N more` indicators — the analog of
/// the oracle's `vvp - 7` (`window = clamp(rows - 7, 3, len)`).
const LIST_CHROME: u16 = 7;

/// The `/workflows` interactive run picker (list mode).
pub struct WorkflowsView {
    /// Snapshot of the runs (newest-first — `cmd_workflows` sorts by start desc).
    rows: Vec<WorkflowRow>,
    /// Index of the highlighted row.
    selected: usize,
    /// Active render palette (accent header + dim footer hint).
    theme: Theme,
    /// Inner viewport height from the last `render`, so `lines()` can size the
    /// scroll window the same way the oracle does (`clamp(rows-7, 3, len)`).
    last_viewport: Cell<u16>,
}

impl WorkflowsView {
    /// Build the picker over `rows`, selecting the first run.
    #[must_use]
    pub fn new(rows: Vec<WorkflowRow>, theme: Theme) -> Self {
        Self {
            rows,
            selected: 0,
            theme,
            last_viewport: Cell::new(0),
        }
    }

    /// Windowed row bounds (oracle `Tar`): the `[start, end)` slice of rows to
    /// show plus how many are hidden above/below. `window = clamp(rows-7, 3, len)`
    /// (`Ky`: min-then-max), `start = clamp(selected-window+1, 0, len-window)`.
    fn window_bounds(&self) -> (usize, usize, usize, usize) {
        let len = self.rows.len();
        if len == 0 {
            return (0, 0, 0, 0);
        }
        let avail = usize::from(self.last_viewport.get().saturating_sub(LIST_CHROME));
        // Ky(avail, 3, len): min applied before max → at least 3, at most len,
        // but a window may still exceed len (the slice below clamps to len).
        let window = avail.min(len).max(3);
        let max_start = len.saturating_sub(window);
        let raw_start = (self.selected + 1).saturating_sub(window);
        let start = raw_start.min(max_start);
        let end = (start + window).min(len);
        (start, end, start, len - end)
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
            let noun = if row.agent_count == 1 {
                "agent"
            } else {
                "agents"
            };
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
        let success = crate::style_adapter::to_ratatui(self.theme.success);
        let error = crate::style_adapter::to_ratatui(self.theme.error);
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
        // Oracle `Tar` windowing: show only a slice, with dim `↑/↓ N more`
        // indicators when rows are hidden — a fixed-height window, not a
        // scrolling paragraph.
        let (start, end, more_above, more_below) = self.window_bounds();
        if more_above > 0 {
            lines.push(Line::from(Span::styled(
                format!("  \u{2191} {more_above} more above"),
                dim_style,
            )));
        }
        for (i, r) in self.rows.iter().enumerate().take(end).skip(start) {
            let selected = i == self.selected;
            let marker = if selected { "\u{276f} " } else { "  " };
            // Oracle `Voa`: the status glyph is colored (✔ success / ✘ error, ⟳
            // uncolored); the name uses the accent color + bold when selected;
            // the meta block is dim. Build discrete spans so each keeps its own
            // style (a single baked string loses all of this).
            let glyph_style = match r.status.as_str() {
                "completed" => Style::default().fg(success),
                "failed" | "killed" => Style::default().fg(error),
                _ => Style::default(),
            };
            let name_style = if selected {
                Style::default().fg(accent).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            let mut spans = vec![
                Span::styled(marker, name_style),
                Span::styled(list_glyph(&r.status), glyph_style),
                Span::raw(" "),
                Span::styled(row_name(r), name_style),
            ];
            let meta = Self::row_meta(r);
            if !meta.is_empty() {
                // Oracle gap between name and meta is exactly two spaces (no dot).
                spans.push(Span::styled(format!("  {meta}"), dim_style));
            }
            lines.push(Line::from(spans));
        }
        if more_below > 0 {
            lines.push(Line::from(Span::styled(
                format!("  \u{2193} {more_below} more below"),
                dim_style,
            )));
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
            if self
                .rows
                .get(self.selected)
                .is_some_and(|r| r.script.is_some())
            {
                parts.push("s save".to_string());
            }
        }
        parts.push("Esc close".to_string());
        parts.join(" \u{00b7} ")
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
        // Record the viewport so `lines()` sizes the scroll window the next call.
        self.last_viewport.set(inner.height);
        // The window already fits the viewport — no paragraph scroll needed.
        Paragraph::new(self.lines()).render(inner, buf);
    }

    fn desired_height(&self, _width: u16) -> u16 {
        // Request enough height for the full list + chrome; the window inside
        // `lines()` clamps to whatever the layout actually grants.
        u16::try_from(self.rows.len())
            .unwrap_or(u16::MAX)
            .saturating_add(LIST_CHROME)
    }
}

impl BottomPaneView for WorkflowsView {
    fn handle_key(&mut self, key: KeyEvent) -> ViewOutcome {
        // Oracle `zoa` keymap: only `escape`/space close, only `x` stops (no
        // `d`/`Delete`/`q` — those were non-parity convenience keys).
        match key.code {
            KeyCode::Esc | KeyCode::Char(' ') => ViewOutcome::Cancelled,
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
            KeyCode::Char('x') => match self.rows.get(self.selected) {
                Some(r) if is_running(&r.status) => {
                    let task_id = r.task_id.clone();
                    // Optimistic: reflect the stop immediately; the real kill
                    // runs off-loop and its result lands in the transcript via
                    // `TurnEvent::SystemNotice`.
                    self.rows[self.selected].status = "killed".to_string();
                    ViewOutcome::RunTaskAction(TaskAction::Kill { task_id })
                }
                _ => ViewOutcome::Pending,
            },
            // Oracle `s` chord (`chord:"s", action:"save"`), gated on the row
            // carrying a saveable inline script → open the "Save dynamic
            // workflow" form.
            KeyCode::Char('s') => match self.rows.get(self.selected) {
                Some(r) if r.script.is_some() => {
                    let script = r.script.clone().unwrap_or_default();
                    let default_name = save_default_name(r);
                    ViewOutcome::OpenView(Box::new(WorkflowSaveView::new(
                        script,
                        default_name,
                        self.theme,
                    )))
                }
                _ => ViewOutcome::Pending,
            },
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

/// The human label for a parsed agent lifecycle state (oracle `eRo` labels).
/// A `cached` (journal-replayed) agent is `done` in CC's model — there is no
/// "Cached" label — so it maps to "Completed" for strict parity.
fn agent_state_label(state: &str) -> &'static str {
    match state {
        "done" | "cached" => "Completed",
        "error" => "Failed",
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
    /// Lines scrolled down from the top (`↑`/`↓`), so a tall phase/agent tree
    /// is reachable rather than clipped.
    scroll: u16,
    /// The inner viewport height from the last `render`, so the `↓` handler can
    /// clamp `scroll` to the same max the render clamp uses (otherwise `scroll`
    /// runs past the bottom and the first N `↑` presses only bleed off dead
    /// state without moving the view).
    last_viewport: Cell<u16>,
}

impl WorkflowDetailView {
    /// Build a detail view over one run snapshot.
    #[must_use]
    pub fn new(row: WorkflowRow, theme: Theme) -> Self {
        Self {
            row,
            theme,
            scroll: 0,
            last_viewport: Cell::new(0),
        }
    }

    /// Max scroll offset given the current line count and last-rendered viewport.
    fn max_scroll(&self) -> u16 {
        let total = u16::try_from(self.lines().len()).unwrap_or(u16::MAX);
        total.saturating_sub(self.last_viewport.get())
    }

    fn lines(&self) -> Vec<Line<'static>> {
        let dim = crate::style_adapter::to_ratatui(self.theme.dim);
        let accent = crate::style_adapter::to_ratatui(self.theme.suggestion);
        let success = crate::style_adapter::to_ratatui(self.theme.success);
        let error = crate::style_adapter::to_ratatui(self.theme.error);
        let dim_style = Style::default().fg(dim);
        let label = |k: &str, v: String| -> Line<'static> {
            Line::from(vec![
                Span::styled(format!("{k:<10}"), dim_style),
                Span::raw(v),
            ])
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
                self.row
                    .run_id
                    .clone()
                    .unwrap_or_else(|| "\u{2014}".to_string()),
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
            for (ord, phase) in self.row.phases.iter().enumerate() {
                let done = phase_done(phase);
                let total = phase.agents.len();
                let all_done = total > 0 && done == total;
                let any_error = phase.agents.iter().any(|a| a.state == "error");
                // Oracle `kfo`: ✔ when done, ✘ on failure, else the 1-based phase
                // ORDINAL (its list position) — NOT a spinner. Colored to match.
                let (glyph, glyph_style) = if all_done {
                    ("\u{2714}".to_string(), Style::default().fg(success))
                } else if any_error {
                    ("\u{2718}".to_string(), Style::default().fg(error))
                } else {
                    ((ord + 1).to_string(), Style::default())
                };
                let title = if phase.title.is_empty() {
                    format!("Phase {}", ord + 1)
                } else {
                    phase.title.clone()
                };
                // Oracle: the `{done}/{total}` count shows ONLY when total > 0
                // (never `0/0`).
                let count = if total > 0 {
                    format!("  {done}/{total}")
                } else {
                    String::new()
                };
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(glyph, glyph_style),
                    Span::raw(format!(" {title}{count}")),
                ]));
                for agent in &phase.agents {
                    let name = if agent.label.is_empty() {
                        "agent"
                    } else {
                        agent.label.as_str()
                    };
                    // Per-agent status glyph (oracle `Oen`), colored by category:
                    // ✔ done/cached (success), ✘ error (error), · otherwise.
                    let (ag_glyph, ag_style) = match agent.state.as_str() {
                        "done" | "cached" => ("\u{2714}", Style::default().fg(success)),
                        "error" => ("\u{2718}", Style::default().fg(error)),
                        _ => ("\u{00b7}", dim_style),
                    };
                    lines.push(Line::from(vec![
                        Span::raw("      "),
                        Span::styled(ag_glyph, ag_style),
                        Span::styled(
                            format!(" {name} \u{00b7} {}", agent_state_label(&agent.state)),
                            dim_style,
                        ),
                    ]));
                }
            }
        }

        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "\u{2191}\u{2193} scroll \u{00b7} Esc back",
            dim_style,
        )));
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
        // Record the viewport so the `↓` key handler clamps to the same max.
        self.last_viewport.set(inner.height);
        let lines = self.lines();
        // Clamp the scroll so the last line can't be scrolled past the top.
        let total = u16::try_from(lines.len()).unwrap_or(u16::MAX);
        let max_scroll = total.saturating_sub(inner.height);
        let scroll = self.scroll.min(max_scroll);
        Paragraph::new(lines).scroll((scroll, 0)).render(inner, buf);
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
            KeyCode::Up | KeyCode::Char('k') => {
                self.scroll = self.scroll.saturating_sub(1);
                ViewOutcome::Pending
            }
            KeyCode::Down | KeyCode::Char('j') => {
                // Clamp to the same max the render uses, so `scroll` never runs
                // past the bottom (before the first render `last_viewport` is 0,
                // giving `total` — render still clamps the displayed offset).
                self.scroll = self.scroll.saturating_add(1).min(self.max_scroll());
                ViewOutcome::Pending
            }
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

/// The "Save dynamic workflow" form (oracle `iNt`, dialog `mode:"save"`), pushed
/// by the `s` chord when the selected run carries an inline script. A name input
/// (default from the run's meta.name / summary), `Tab` toggles scope
/// Project ⇄ User, `Enter` saves (a second `Enter` overwrites on EEXIST), `Esc`
/// cancels. The write itself goes through [`tool_workflow::save_dynamic_workflow`]
/// so the dir resolver + file modes (`0o700` dir / `0o600` file) stay 1:1 with
/// the launcher/read side; success emits `tengu_workflow_saved`.
pub struct WorkflowSaveView {
    /// The inline script to persist.
    script: String,
    /// The editable target name (sanitized to kebab on save/preview).
    name: String,
    /// Cursor position in `name`, as a char index.
    cursor: usize,
    /// Project (default) or User scope.
    scope: WorkflowScope,
    /// Active render palette.
    theme: Theme,
    /// Working directory used to resolve the project workflow dir.
    cwd: PathBuf,
    /// The already-existing path awaiting an overwrite-confirm (`Enter` again).
    /// Set on an EEXIST save; cleared by any edit or scope toggle.
    pending_overwrite: Option<String>,
    /// A non-EEXIST save error message (I/O failure).
    error: Option<String>,
    /// The success feedback once the workflow is saved (view then waits on `Esc`).
    feedback: Option<String>,
}

impl WorkflowSaveView {
    /// Build the save form for `script`, pre-filling `default_name`.
    #[must_use]
    pub fn new(script: String, default_name: String, theme: Theme) -> Self {
        let cursor = default_name.chars().count();
        Self {
            script,
            name: default_name,
            cursor,
            scope: WorkflowScope::Project,
            theme,
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            pending_overwrite: None,
            error: None,
            feedback: None,
        }
    }

    /// Clear the transient save state after an edit (name change / scope toggle),
    /// so a stale "already exists" prompt or error doesn't linger.
    fn clear_transient(&mut self) {
        self.pending_overwrite = None;
        self.error = None;
        self.feedback = None;
    }

    /// The sanitized name preview (oracle `nya = lme(wft.trim() || "workflow")`).
    fn preview_name(&self) -> String {
        sanitize_workflow_name(self.name.trim())
    }

    /// The subtitle preview path (oracle `a3p`): a relative `.lingxi/workflows/…`
    /// for project scope, the absolute user-config path for user scope.
    fn preview_path(&self) -> String {
        let file = format!("{}.js", self.preview_name());
        match self.scope {
            WorkflowScope::Project => {
                format!("{}/workflows/{file}", branding::DOT_DIR)
            }
            WorkflowScope::User => {
                tool_workflow::workflow_scope_dir(WorkflowScope::User, &self.cwd)
                    .join(&file)
                    .to_string_lossy()
                    .into_owned()
            }
        }
    }

    /// Perform the save (oracle `s3p`): no-op on an empty name; overwrite when an
    /// EEXIST prompt is pending; report success / already-exists / error.
    fn save(&mut self) {
        // Already saved — the form is showing feedback; ignore further Enter.
        if self.feedback.is_some() {
            return;
        }
        if self.name.trim().is_empty() {
            return;
        }
        let overwrite = self.pending_overwrite.is_some();
        match save_dynamic_workflow(&self.name, self.scope, &self.script, overwrite, &self.cwd) {
            Ok(saved) => {
                telemetry::emit_workflow_saved(
                    saved.scope.wire(),
                    overwrite,
                    saved.script_size_chars,
                );
                self.pending_overwrite = None;
                self.error = None;
                self.feedback = Some(saved_feedback(&saved.name, &saved.path));
            }
            Err(WorkflowSaveError::AlreadyExists { path, .. }) => {
                self.error = None;
                self.pending_overwrite = Some(path.to_string_lossy().into_owned());
            }
            Err(WorkflowSaveError::Io(e)) => {
                self.pending_overwrite = None;
                self.error = Some(e.to_string());
            }
        }
    }

    fn lines(&self) -> Vec<Line<'static>> {
        let dim = crate::style_adapter::to_ratatui(self.theme.dim);
        let accent = crate::style_adapter::to_ratatui(self.theme.suggestion);
        let success = crate::style_adapter::to_ratatui(self.theme.success);
        let error = crate::style_adapter::to_ratatui(self.theme.error);
        let dim_style = Style::default().fg(dim);

        let mut lines: Vec<Line<'static>> = Vec::with_capacity(10);
        lines.push(Line::from(Span::styled(
            "Save dynamic workflow".to_string(),
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(Span::styled(
            format!(
                "{} scope \u{00b7} {}",
                self.scope.label(),
                self.preview_path()
            ),
            dim_style,
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("Save as:".to_string(), dim_style)));
        lines.push(Line::from(vec![
            Span::styled("\u{276f} ".to_string(), Style::default().fg(accent)),
            Span::raw(self.name.clone()),
        ]));
        if let Some(path) = &self.pending_overwrite {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!(
                    "{path} already exists. Press Enter again to overwrite, or change the name."
                ),
                Style::default().fg(error),
            )));
        }
        if let Some(err) = &self.error {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                err.clone(),
                Style::default().fg(error),
            )));
        }
        if let Some(fb) = &self.feedback {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                fb.clone(),
                Style::default().fg(success),
            )));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(self.footer(), dim_style)));
        lines
    }

    /// The chord-hint footer: `Enter save`/`Enter overwrite` · `Tab toggle
    /// scope` · `Esc cancel`.
    fn footer(&self) -> String {
        let action = if self.pending_overwrite.is_some() {
            "overwrite"
        } else {
            "save"
        };
        format!("Enter {action} \u{00b7} Tab toggle scope \u{00b7} Esc cancel")
    }
}

impl Renderable for WorkflowSaveView {
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

impl BottomPaneView for WorkflowSaveView {
    fn handle_key(&mut self, key: KeyEvent) -> ViewOutcome {
        match key.code {
            KeyCode::Esc => ViewOutcome::Cancelled,
            KeyCode::Tab => {
                self.scope = self.scope.toggled();
                self.clear_transient();
                ViewOutcome::Pending
            }
            KeyCode::Enter => {
                self.save();
                ViewOutcome::Pending
            }
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    let idx = self.cursor - 1;
                    let byte = self
                        .name
                        .char_indices()
                        .nth(idx)
                        .map(|(b, _)| b)
                        .unwrap_or(0);
                    self.name.remove(byte);
                    self.cursor = idx;
                    self.clear_transient();
                }
                ViewOutcome::Pending
            }
            KeyCode::Left => {
                self.cursor = self.cursor.saturating_sub(1);
                ViewOutcome::Pending
            }
            KeyCode::Right => {
                self.cursor = (self.cursor + 1).min(self.name.chars().count());
                ViewOutcome::Pending
            }
            KeyCode::Char(c) if !c.is_control() => {
                let byte = self
                    .name
                    .char_indices()
                    .nth(self.cursor)
                    .map(|(b, _)| b)
                    .unwrap_or(self.name.len());
                self.name.insert(byte, c);
                self.cursor += 1;
                self.clear_transient();
                ViewOutcome::Pending
            }
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
            script: None,
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
    fn detail_scrolls_and_clamps_at_top() {
        let mut v = WorkflowDetailView::new(row("w1", "running", "deploy"), Theme::dark());
        assert_eq!(v.scroll, 0);
        v.handle_key(press(KeyCode::Down));
        assert_eq!(v.scroll, 1);
        v.handle_key(press(KeyCode::Up));
        assert_eq!(v.scroll, 0);
        v.handle_key(press(KeyCode::Up)); // clamp at top
        assert_eq!(v.scroll, 0);
        assert!(matches!(
            v.handle_key(press(KeyCode::Esc)),
            ViewOutcome::Cancelled
        ));
    }

    #[test]
    fn pending_run_is_not_stoppable() {
        // Oracle gates `x` on status === "running"; a pending run shows no stop.
        let mut v = view(vec![row("w1", "pending", "queued-run")]);
        assert!(!v.footer().contains("x stop"));
        assert!(matches!(
            v.handle_key(press(KeyCode::Char('x'))),
            ViewOutcome::Pending
        ));
        assert_eq!(v.rows()[0].status, "pending", "not killed");
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
        assert!(
            text.contains("No dynamic workflows in this session"),
            "{text}"
        );
    }

    #[test]
    fn list_glyph_covers_every_status() {
        assert_eq!(list_glyph("completed"), "\u{2714}"); // ✔
        assert_eq!(list_glyph("failed"), "\u{2718}"); // ✘
        assert_eq!(list_glyph("killed"), "\u{2718}"); // ✘
        assert_eq!(list_glyph("running"), "\u{27f3}"); // ⟳
        assert_eq!(list_glyph("pending"), "\u{27f3}"); // ⟳
        assert_eq!(list_glyph("nonsense"), "\u{27f3}"); // fallback ⟳
    }

    #[test]
    fn row_elapsed_uses_fixed_span_when_ended() {
        // Terminal run: elapsed is ended - started, independent of the clock.
        let mut r = row("w1", "completed", "done");
        r.started_at_ms = Some(1_000);
        r.ended_at_ms = Some(84_000); // 83s
        assert_eq!(row_elapsed(&r).as_deref(), Some("1m 23s"));
        // Running run: no end -> uses now (just assert it produces something).
        r.ended_at_ms = None;
        assert!(row_elapsed(&r).is_some());
        // No start -> no elapsed.
        r.started_at_ms = None;
        assert_eq!(row_elapsed(&r), None);
    }

    #[test]
    fn windowing_shows_a_slice_with_more_above_below_indicators() {
        // 40 rows in a short viewport → only a window renders, with "N more
        // above"/"below" indicators, and the selected row stays visible.
        let mut v = view(
            (0..40)
                .map(|i| row(&format!("w{i:02}"), "running", "r"))
                .collect(),
        );
        let area = Rect::new(0, 0, 60, 15); // inner height ~13
        let render = |v: &WorkflowsView| {
            let mut buf = Buffer::empty(area);
            v.render(area, &mut buf);
            buf_text(&buf, area)
        };
        // Selection at top → nothing hidden above, some hidden below.
        let text = render(&v);
        assert!(!text.contains("more above"), "top: {text}");
        assert!(
            text.contains("more below"),
            "top should hide rows below: {text}"
        );
        // Move selection to the end → rows hidden above, none below.
        v.selected = 39;
        let text = render(&v);
        assert!(
            text.contains("more above"),
            "end should hide rows above: {text}"
        );
        assert!(!text.contains("more below"), "end: {text}");
        // The windowed slice is far smaller than all 40 rows.
        let (start, end, _, _) = v.window_bounds();
        assert!(end - start < 40, "window slices the list: {start}..{end}");
        assert!(
            start <= v.selected && v.selected < end,
            "selected in window"
        );
    }

    #[test]
    fn detail_titleless_phase_label_uses_ordinal_position() {
        // A titleless phase renders "Phase {ordinal}" using its 1-based LIST
        // position (oracle `kfo` uses array index + 1), independent of the
        // parsed phase.index.
        let mut r = row("w1", "running", "x");
        r.phases = vec![WorkflowPhase {
            index: 7, // parsed index is ignored for the label
            title: String::new(),
            agents: vec![],
        }];
        let v = WorkflowDetailView::new(r, Theme::dark());
        let area = Rect::new(0, 0, 60, 16);
        let mut buf = Buffer::empty(area);
        v.render(area, &mut buf);
        let text = buf_text(&buf, area);
        assert!(text.contains("Phase 1"), "ordinal label: {text}");
        assert!(!text.contains("Phase 7"), "not the parsed index: {text}");
    }

    #[test]
    fn row_separates_name_and_meta_with_two_spaces_not_a_dot() {
        let mut r = row("w00000001", "running", "deploy-site");
        r.started_at_ms = Some(1_000);
        r.ended_at_ms = Some(46_000); // 45s
        let v = view(vec![r]);
        let area = Rect::new(0, 0, 70, 8);
        let mut buf = Buffer::empty(area);
        v.render(area, &mut buf);
        let text = buf_text(&buf, area);
        // Oracle gap is two spaces, no leading dot before the meta.
        assert!(text.contains("deploy-site  3 agents"), "{text}");
        assert!(
            !text.contains("deploy-site  \u{00b7}"),
            "no spurious dot: {text}"
        );
    }

    // ── Save dynamic workflow (`s` chord + WorkflowSaveView) ─────────────────

    fn unique_tmp(label: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "lingxi-saveview-{label}-{}-{nanos}",
            std::process::id()
        ))
    }

    fn save_view() -> WorkflowSaveView {
        WorkflowSaveView::new(
            "export const meta = { name: 'x' };\n".to_string(),
            "My WF".to_string(),
            Theme::dark(),
        )
    }

    #[test]
    fn s_chord_gated_on_inline_script() {
        // No inline script → no `s save` hint, `s` is a no-op.
        let mut no_script = view(vec![row("w1", "completed", "deploy")]);
        assert!(!no_script.footer().contains("s save"));
        assert!(matches!(
            no_script.handle_key(press(KeyCode::Char('s'))),
            ViewOutcome::Pending
        ));

        // Inline script present → `s save` hint, `s` opens the save form.
        let mut r = row("w1", "completed", "deploy");
        r.script = Some("export const meta = {};\n".to_string());
        let mut with_script = view(vec![r]);
        assert!(with_script.footer().contains("s save"));
        assert!(matches!(
            with_script.handle_key(press(KeyCode::Char('s'))),
            ViewOutcome::OpenView(_)
        ));
    }

    #[test]
    fn save_view_preview_scope_and_footer() {
        let mut v = save_view();
        // Name is kebab-sanitized in the preview.
        assert_eq!(v.preview_name(), "my-wf");
        assert_eq!(
            v.preview_path(),
            format!("{}/workflows/my-wf.js", branding::DOT_DIR)
        );
        assert!(v.footer().contains("Enter save"));
        // Tab toggles to User scope; the preview path becomes absolute.
        v.handle_key(press(KeyCode::Tab));
        assert_eq!(v.scope, WorkflowScope::User);
        assert!(v.preview_path().ends_with("workflows/my-wf.js"));
        assert_ne!(
            v.preview_path(),
            format!("{}/workflows/my-wf.js", branding::DOT_DIR)
        );
    }

    #[test]
    fn save_view_typing_edits_name_and_clears_prompt() {
        let mut v = save_view();
        v.pending_overwrite = Some("/x/my-wf.js".to_string());
        v.handle_key(press(KeyCode::Char('2')));
        assert_eq!(v.name, "My WF2");
        // Editing clears any pending overwrite prompt.
        assert!(v.pending_overwrite.is_none());
        v.handle_key(press(KeyCode::Backspace));
        assert_eq!(v.name, "My WF");
    }

    #[test]
    fn save_view_writes_file_and_shows_feedback() {
        let tmp = unique_tmp("write");
        std::fs::create_dir_all(&tmp).unwrap();
        let mut v = save_view();
        v.cwd = tmp.clone();
        assert!(matches!(
            v.handle_key(press(KeyCode::Enter)),
            ViewOutcome::Pending
        ));
        let path = tmp
            .join(branding::DOT_DIR)
            .join("workflows")
            .join("my-wf.js");
        assert!(path.exists(), "script written to project dir");
        let fb = v.feedback.as_deref().expect("feedback set");
        assert!(fb.starts_with("Dynamic workflow saved to "));
        assert!(fb.contains("Invoke as /my-wf or Workflow({name: \"my-wf\"})"));
        // Esc closes the form.
        assert!(matches!(
            v.handle_key(press(KeyCode::Esc)),
            ViewOutcome::Cancelled
        ));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn save_view_prompts_then_overwrites_on_second_enter() {
        let tmp = unique_tmp("overwrite");
        std::fs::create_dir_all(&tmp).unwrap();
        // Pre-create the target via a first save.
        let mut first = save_view();
        first.cwd = tmp.clone();
        first.handle_key(press(KeyCode::Enter));
        assert!(first.feedback.is_some());

        // A fresh form for the same name: first Enter → overwrite prompt.
        let mut v = save_view();
        v.cwd = tmp.clone();
        v.handle_key(press(KeyCode::Enter));
        assert!(v.pending_overwrite.is_some(), "EEXIST → overwrite prompt");
        assert!(v.feedback.is_none());
        assert!(v.footer().contains("Enter overwrite"));

        // Second Enter overwrites → feedback.
        v.handle_key(press(KeyCode::Enter));
        assert!(v.feedback.is_some(), "second Enter overwrites");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn save_view_empty_name_is_a_noop() {
        let tmp = unique_tmp("empty");
        std::fs::create_dir_all(&tmp).unwrap();
        let mut v = WorkflowSaveView::new("script".to_string(), "   ".to_string(), Theme::dark());
        v.cwd = tmp.clone();
        v.handle_key(press(KeyCode::Enter));
        assert!(v.feedback.is_none(), "blank name does not save");
        assert!(v.pending_overwrite.is_none());
        let _ = std::fs::remove_dir_all(&tmp);
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

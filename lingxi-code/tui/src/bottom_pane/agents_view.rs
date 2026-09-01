//! Live agents view driven by owner-supplied fleet snapshots.

use std::any::Any;
use std::cell::Cell;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};
use tui_core::theme::Theme;

use crate::agents_screen::{band_for_state, Band};
use crate::bottom_pane::view::{
    AgentSessionTarget, AgentsPaneRow, AgentsSnapshot, BottomPaneView, CommandAction,
    OwnerViewUpdate, ViewOutcome,
};
use crate::renderable::Renderable;

pub struct AgentsView {
    snapshot: AgentsSnapshot,
    selected: usize,
    scroll: Cell<u16>,
    last_relative_tick: Cell<u64>,
    theme: Theme,
}

impl AgentsView {
    #[must_use]
    pub fn new(snapshot: AgentsSnapshot, theme: Theme) -> Self {
        Self {
            snapshot,
            selected: 0,
            scroll: Cell::new(0),
            last_relative_tick: Cell::new(relative_tick_seconds()),
            theme,
        }
    }

    #[must_use]
    pub fn rows(&self) -> &[AgentsPaneRow] {
        &self.snapshot.rows
    }

    #[must_use]
    pub fn selected_index(&self) -> usize {
        self.selected
    }

    fn current_row(&self) -> Option<&AgentsPaneRow> {
        self.snapshot.rows.get(self.selected)
    }

    fn lines_and_selected_line(&self) -> (Vec<Line<'static>>, Option<u16>) {
        let dim = Style::default().fg(crate::style_adapter::to_ratatui(self.theme.dim));
        let accent = Style::default()
            .fg(crate::style_adapter::to_ratatui(self.theme.suggestion))
            .add_modifier(Modifier::BOLD);
        let mut lines = vec![
            Line::from(Span::styled("Agents", accent)),
            Line::from(Span::styled(
                format!(
                    "{} session{}",
                    self.snapshot.rows.len(),
                    if self.snapshot.rows.len() == 1 {
                        ""
                    } else {
                        "s"
                    }
                ),
                dim,
            )),
            Line::from(String::new()),
        ];
        let mut selected_line = None;
        if self.snapshot.rows.is_empty() {
            lines.push(Line::from(Span::styled("No agents yet".to_string(), dim)));
        } else {
            for band in Band::ALL {
                let members: Vec<(usize, &AgentsPaneRow)> = self
                    .snapshot
                    .rows
                    .iter()
                    .enumerate()
                    .filter(|(_, row)| band_for_state(&row.state) == band)
                    .collect();
                if members.is_empty() {
                    continue;
                }
                lines.push(Line::from(Span::styled(
                    band.label().to_string(),
                    Style::default().add_modifier(Modifier::BOLD),
                )));
                if !band.description().is_empty() {
                    lines.push(Line::from(Span::styled(
                        band.description().to_string(),
                        dim,
                    )));
                }
                for (index, row) in members {
                    if index == self.selected {
                        selected_line = Some(u16::try_from(lines.len()).unwrap_or(u16::MAX));
                    }
                    let marker = if index == self.selected { "› " } else { "  " };
                    let style = if index == self.selected {
                        Style::default().add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    };
                    lines.push(Line::from(Span::styled(
                        format!(
                            "{marker}{} {}  [{}]",
                            lifecycle_glyph(row),
                            row.name,
                            row.state
                        ),
                        style,
                    )));
                    lines.push(Line::from(Span::styled(detail_line(row), dim)));
                    if let Some(extra) = extra_line(row) {
                        lines.push(Line::from(Span::styled(extra, dim)));
                    }
                }
                lines.push(Line::from(String::new()));
            }
        }
        lines.push(Line::from(Span::styled(
            "Enter attach · l connect · ↑/↓ select · Esc close".to_string(),
            dim,
        )));
        (lines, selected_line)
    }

    fn ensure_selected_visible(&self, viewport_height: u16, selected_line: Option<u16>) -> u16 {
        let max_scroll = self.max_scroll(viewport_height);
        let mut scroll = self.scroll.get().min(max_scroll);
        if let Some(selected_line) = selected_line {
            let bottom = scroll.saturating_add(viewport_height.saturating_sub(1));
            if selected_line < scroll {
                scroll = selected_line;
            } else if selected_line > bottom {
                scroll = selected_line.saturating_sub(viewport_height.saturating_sub(1));
            }
        }
        scroll.min(max_scroll)
    }

    fn max_scroll(&self, viewport_height: u16) -> u16 {
        let total = u16::try_from(self.lines_and_selected_line().0.len()).unwrap_or(u16::MAX);
        total.saturating_sub(viewport_height)
    }
}

impl Renderable for AgentsView {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        Clear.render(area, buf);
        let block = Block::new().borders(Borders::ALL);
        let inner = block.inner(area);
        block.render(area, buf);
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        let (lines, selected_line) = self.lines_and_selected_line();
        let scroll = self.ensure_selected_visible(inner.height, selected_line);
        self.scroll.set(scroll);
        self.last_relative_tick.set(relative_tick_seconds());
        Paragraph::new(lines).scroll((scroll, 0)).render(inner, buf);
    }

    fn desired_height(&self, _width: u16) -> u16 {
        u16::try_from(self.lines_and_selected_line().0.len())
            .unwrap_or(u16::MAX)
            .saturating_add(2)
    }
}

impl BottomPaneView for AgentsView {
    fn handle_key(&mut self, key: KeyEvent) -> ViewOutcome {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char(' ') => ViewOutcome::Cancelled,
            KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                ViewOutcome::Pending
            }
            KeyCode::Down => {
                if self.selected + 1 < self.snapshot.rows.len() {
                    self.selected += 1;
                }
                ViewOutcome::Pending
            }
            KeyCode::Char('l') | KeyCode::Char('L') => {
                ViewOutcome::RunCommand(CommandAction::OpenConnectPicker)
            }
            KeyCode::Enter => self.current_row().map_or(ViewOutcome::Pending, |row| {
                uuid::Uuid::parse_str(&row.session_id).map_or(ViewOutcome::Pending, |session_id| {
                    ViewOutcome::OpenAgentSession(AgentSessionTarget {
                        session_id,
                        background: row.kind == "background",
                        live: row.status.is_some(),
                    })
                })
            }),
            _ => ViewOutcome::Pending,
        }
    }

    fn wants_status_line(&self) -> bool {
        false
    }

    fn needs_redraw(&self) -> bool {
        relative_tick_seconds() != self.last_relative_tick.get()
    }

    fn refresh_from_owner(&mut self, update: OwnerViewUpdate) {
        let OwnerViewUpdate::Agents(snapshot) = update else {
            return;
        };
        let selected_id = self.current_row().map(|row| row.session_id.clone());
        self.snapshot = sorted_snapshot(snapshot);
        self.selected = selected_id
            .as_deref()
            .and_then(|session_id| {
                self.snapshot
                    .rows
                    .iter()
                    .position(|row| row.session_id == session_id)
            })
            .unwrap_or_else(|| {
                self.selected
                    .min(self.snapshot.rows.len().saturating_sub(1))
            });
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn relative_tick_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn sorted_snapshot(mut snapshot: AgentsSnapshot) -> AgentsSnapshot {
    snapshot
        .rows
        .sort_by_key(|row| (band_for_state(&row.state), row.started_at_ms.unwrap_or(0)));
    snapshot
}

fn lifecycle_glyph(row: &AgentsPaneRow) -> &'static str {
    match row.state.as_str() {
        "done" => "✓",
        "failed" | "stopped" => "■",
        "blocked" => "◌",
        "queued" => "…",
        "nonresponsive" => "!",
        _ => match row.status.as_deref() {
            Some("idle") => "○",
            Some("waiting") => "◌",
            _ => "●",
        },
    }
}

fn detail_line(row: &AgentsPaneRow) -> String {
    let mut bits = vec![row.cwd.clone()];
    if !row.kind.is_empty() {
        bits.push(row.kind.clone());
    }
    if let Some(status) = row.status.as_ref() {
        bits.push(status.clone());
    }
    if let Some(model) = row.model.as_ref().filter(|model| !model.is_empty()) {
        bits.push(model.clone());
    }
    if let Some(started_at_ms) = row.started_at_ms {
        if let Some(label) = relative_started_at(started_at_ms) {
            bits.push(label);
        }
    }
    bits.join(" · ")
}

fn extra_line(row: &AgentsPaneRow) -> Option<String> {
    let mut bits = Vec::new();
    if let Some(waiting_for) = row.waiting_for.as_ref().filter(|value| !value.is_empty()) {
        bits.push(waiting_for.clone());
    } else if let Some(detail) = row.detail.as_ref().filter(|value| !value.is_empty()) {
        bits.push(detail.clone());
    }
    let stats = stats_label(row);
    if !stats.is_empty() {
        bits.push(stats);
    }
    (!bits.is_empty()).then(|| bits.join(" · "))
}

fn stats_label(row: &AgentsPaneRow) -> String {
    let mut stats = Vec::new();
    if let Some(tokens) = row.tokens {
        stats.push(format!("{tokens} tokens"));
    }
    if let Some(tool_calls) = row.tool_calls {
        stats.push(format!("{tool_calls} tools"));
    }
    stats.join(" · ")
}

fn relative_started_at(started_at_ms: u64) -> Option<String> {
    let started = UNIX_EPOCH.checked_add(Duration::from_millis(started_at_ms))?;
    Some(crate::resume::relative_time_ago(started, SystemTime::now()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn row(session_id: &str, state: &str, started_at_ms: u64) -> AgentsPaneRow {
        AgentsPaneRow {
            session_id: session_id.to_string(),
            name: format!("agent-{session_id}"),
            state: state.to_string(),
            kind: "background".to_string(),
            cwd: "/repo".to_string(),
            status: Some("busy".to_string()),
            waiting_for: None,
            detail: None,
            model: Some("gpt-5.4".to_string()),
            tokens: Some(12),
            tool_calls: Some(3),
            started_at_ms: Some(started_at_ms),
        }
    }

    #[test]
    fn owner_refresh_preserves_selection_by_session_id() {
        let mut view = AgentsView::new(
            AgentsSnapshot {
                rows: vec![row("11111111-1111-4111-8111-111111111111", "working", 2)],
            },
            Theme::dark(),
        );
        assert!(matches!(
            view.handle_key(press(KeyCode::Down)),
            ViewOutcome::Pending
        ));
        view.refresh_from_owner(OwnerViewUpdate::Agents(AgentsSnapshot {
            rows: vec![
                row("00000000-0000-4000-8000-000000000000", "blocked", 1),
                row("11111111-1111-4111-8111-111111111111", "working", 2),
            ],
        }));
        assert_eq!(view.selected_index(), 1);
    }

    #[test]
    fn enter_attaches_selected_session() {
        let mut view = AgentsView::new(
            AgentsSnapshot {
                rows: vec![row("11111111-1111-4111-8111-111111111111", "working", 1)],
            },
            Theme::dark(),
        );
        assert!(matches!(
            view.handle_key(press(KeyCode::Enter)),
            ViewOutcome::OpenAgentSession(AgentSessionTarget {
                session_id,
                background: true,
                live: true,
            }) if session_id
                == uuid::Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap()
        ));
    }

    #[test]
    fn arrow_navigation_and_escape_work() {
        let mut view = AgentsView::new(
            AgentsSnapshot {
                rows: vec![
                    row("11111111-1111-4111-8111-111111111111", "blocked", 1),
                    row("22222222-2222-4222-8222-222222222222", "working", 2),
                ],
            },
            Theme::dark(),
        );
        assert!(matches!(
            view.handle_key(press(KeyCode::Down)),
            ViewOutcome::Pending
        ));
        assert_eq!(view.selected_index(), 1);
        assert!(matches!(
            view.handle_key(press(KeyCode::Esc)),
            ViewOutcome::Cancelled
        ));
    }

    #[test]
    fn lifecycle_glyph_distinguishes_queued_and_nonresponsive_rows() {
        assert_eq!(
            lifecycle_glyph(&row("11111111-1111-4111-8111-111111111111", "queued", 1)),
            "…"
        );
        assert_eq!(
            lifecycle_glyph(&row(
                "11111111-1111-4111-8111-111111111111",
                "nonresponsive",
                1
            )),
            "!"
        );
    }

    #[test]
    fn login_key_routes_to_connect_without_changing_selection() {
        let mut view = AgentsView::new(
            AgentsSnapshot {
                rows: vec![
                    row("11111111-1111-4111-8111-111111111111", "blocked", 1),
                    row("22222222-2222-4222-8222-222222222222", "working", 2),
                ],
            },
            Theme::dark(),
        );
        let _ = view.handle_key(press(KeyCode::Down));
        assert!(matches!(
            view.handle_key(press(KeyCode::Char('l'))),
            ViewOutcome::RunCommand(CommandAction::OpenConnectPicker)
        ));
        assert_eq!(view.selected_index(), 1);
    }

    #[test]
    fn render_scroll_keeps_selected_row_visible_in_small_viewport() {
        let mut rows = Vec::new();
        for idx in 0..8 {
            rows.push(row(
                &format!("{idx:08}-0000-4000-8000-000000000000"),
                "working",
                idx as u64,
            ));
        }
        let mut view = AgentsView::new(AgentsSnapshot { rows }, Theme::dark());
        for _ in 0..6 {
            let _ = view.handle_key(press(KeyCode::Down));
        }
        let area = Rect::new(0, 0, 40, 8);
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf);
        let rendered: String = (0..area.height)
            .map(|row| {
                (0..area.width)
                    .map(|col| buf[(col, row)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("agent-00000006"), "{rendered}");
        assert!(view.scroll.get() > 0, "selection should scroll into view");
    }
}

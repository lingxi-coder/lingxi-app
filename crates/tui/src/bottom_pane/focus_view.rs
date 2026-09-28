//! `/focus`: a filtered transcript view showing prompts, completion summaries,
//! and assistant responses without tool chatter.

use std::any::Any;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};
use tui_core::theme::Theme;

use crate::bottom_pane::input_status;
use crate::bottom_pane::view::{BottomPaneView, FocusProjection, OwnerViewUpdate, ViewOutcome};
use crate::renderable::Renderable;

pub struct FocusView {
    projection: FocusProjection,
    scroll: u16,
    theme: Theme,
}

impl FocusView {
    #[must_use]
    pub fn new(projection: FocusProjection, theme: Theme) -> Self {
        Self {
            projection,
            scroll: 0,
            theme,
        }
    }

    #[must_use]
    pub fn projection(&self) -> &FocusProjection {
        &self.projection
    }

    fn lines(&self) -> Vec<Line<'static>> {
        let dim = Style::default().fg(crate::style_adapter::to_ratatui(self.theme.dim));
        let accent = Style::default()
            .fg(crate::style_adapter::to_ratatui(self.theme.suggestion))
            .add_modifier(Modifier::BOLD);
        let mut lines = vec![
            Line::from(Span::styled("Focus", accent)),
            Line::from(Span::styled(
                "Just your prompt, summary, and response".to_string(),
                dim,
            )),
            Line::from(String::new()),
        ];
        let activity = input_status::agent_lines(&self.projection.running_agents, &self.theme);
        if !activity.is_empty() {
            lines.push(Line::from(Span::styled("Activity", accent)));
            lines.extend(activity);
            lines.push(Line::from(String::new()));
        }
        if self.projection.lines.is_empty() {
            lines.push(Line::from(Span::styled(
                "No focused transcript yet.".to_string(),
                dim,
            )));
        } else {
            lines.push(Line::from(Span::styled("Transcript", accent)));
            lines.extend(self.projection.lines.iter().cloned().map(Line::from));
        }
        lines.push(Line::from(String::new()));
        lines.push(Line::from(Span::styled(
            "↑/↓ navigate · Esc close".to_string(),
            dim,
        )));
        lines
    }

    fn max_scroll(&self, viewport: u16) -> u16 {
        let total = u16::try_from(self.lines().len()).unwrap_or(u16::MAX);
        total.saturating_sub(viewport)
    }
}

impl Renderable for FocusView {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        Clear.render(area, buf);
        let block = Block::new().borders(Borders::ALL);
        let inner = block.inner(area);
        block.render(area, buf);
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        let scroll = self.scroll.min(self.max_scroll(inner.height));
        Paragraph::new(self.lines())
            .scroll((scroll, 0))
            .render(inner, buf);
    }

    fn desired_height(&self, _width: u16) -> u16 {
        u16::try_from(self.lines().len())
            .unwrap_or(u16::MAX)
            .saturating_add(2)
    }
}

impl BottomPaneView for FocusView {
    fn handle_key(&mut self, key: KeyEvent) -> ViewOutcome {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => ViewOutcome::Cancelled,
            KeyCode::Up => {
                self.scroll = self.scroll.saturating_sub(1);
                ViewOutcome::Pending
            }
            KeyCode::Down => {
                self.scroll = self.scroll.saturating_add(1);
                ViewOutcome::Pending
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(10);
                ViewOutcome::Pending
            }
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_add(10);
                ViewOutcome::Pending
            }
            _ => ViewOutcome::Pending,
        }
    }

    fn wants_status_line(&self) -> bool {
        false
    }

    fn refresh_from_owner(&mut self, update: OwnerViewUpdate) {
        match update {
            OwnerViewUpdate::Focus(projection) => self.projection = projection,
            OwnerViewUpdate::Agents(_) => {}
            OwnerViewUpdate::RunningAgents(agents) => self.projection.running_agents = agents,
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
    use tui_core::orchestrator_bridge::RunningAgentStatus;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn projection() -> FocusProjection {
        FocusProjection {
            lines: vec!["> Prompt".to_string(), "● Answer".to_string()],
            running_agents: vec![RunningAgentStatus {
                awaiting_plan_approval: false,
                id: "a1".to_string(),
                task_type: "local_agent".to_string(),
                agent_type: "Explore".to_string(),
                description: "Map flow".to_string(),
                status: "running".to_string(),
                custom_content: None,
            }],
        }
    }

    #[test]
    fn owner_refresh_replaces_projection_and_activity() {
        let mut view = FocusView::new(projection(), Theme::dark());
        view.refresh_from_owner(OwnerViewUpdate::Focus(FocusProjection {
            lines: vec!["> New prompt".to_string()],
            running_agents: Vec::new(),
        }));
        let text: Vec<String> = view
            .lines()
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.into_owned())
                    .collect()
            })
            .collect();
        assert!(text.iter().any(|line| line == "> New prompt"));
        assert!(!text.iter().any(|line| line.contains("local agent")));
    }

    #[test]
    fn esc_closes_and_arrows_scroll() {
        let mut view = FocusView::new(
            FocusProjection {
                lines: (0..30).map(|n| format!("line {n}")).collect(),
                running_agents: Vec::new(),
            },
            Theme::dark(),
        );
        assert!(matches!(
            view.handle_key(press(KeyCode::Down)),
            ViewOutcome::Pending
        ));
        assert_eq!(view.scroll, 1);
        assert!(matches!(
            view.handle_key(press(KeyCode::Esc)),
            ViewOutcome::Cancelled
        ));
    }
}

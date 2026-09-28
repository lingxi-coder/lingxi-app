//! `/web` provider picker: a centered, bordered, search-filterable list over
//! the fixed v1 provider set (Auto/DuckDuckGo/Tavily/Brave/SearXNG), backed
//! by the pure [`crate::web::picker`] reducer.
//!
//! Modeled on [`crate::bottom_pane::model_picker_view::ModelPickerView`]:
//! arrow keys move the highlight, typing filters (`query`), `Enter` opens the
//! per-provider config screen ([`crate::bottom_pane::web_config_view::WebConfigView`])
//! as a child view, `t` runs a test search for the highlighted provider
//! without leaving the picker, `Esc` cancels.

use std::any::Any;

use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

use crate::bottom_pane::dialog_view::centered_rect;
use crate::bottom_pane::view::{BottomPaneView, ViewOutcome, WebAction};
use crate::bottom_pane::web_config_view::WebConfigView;
use crate::renderable::Renderable;
use crate::web::picker::{
    handle_web_picker_key, WebConfigSnapshot, WebPickerOutcome, WebPickerState,
};

/// The `/web` provider picker view.
pub struct WebPickerView {
    state: WebPickerState,
}

impl WebPickerView {
    /// Build the picker over `snapshot` (the current configured/active state).
    #[must_use]
    pub fn new(snapshot: WebConfigSnapshot) -> Self {
        Self {
            state: WebPickerState::from_snapshot(snapshot),
        }
    }
}

impl Renderable for WebPickerView {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let visible = self.state.visible_indices();
        let content_width = visible
            .iter()
            .map(|&idx| {
                let row = &self.state.rows[idx];
                row.label.chars().count() + 2 + row.description.chars().count() + 2
            })
            .max()
            .unwrap_or(0)
            .max("No web providers match.".len())
            .max("Configure web search".len());
        let width = u16::try_from(content_width + 4)
            .unwrap_or(u16::MAX)
            .min(area.width.saturating_sub(4))
            .max(30);
        let rows = visible.len().max(1);
        let height = u16::try_from(rows + 6).unwrap_or(u16::MAX).min(area.height);
        let rect = centered_rect(width, height, area);

        Clear.render(rect, buf);
        let block = Block::new()
            .borders(Borders::ALL)
            .title("Configure web search");
        let inner = block.inner(rect);
        block.render(rect, buf);

        let mut lines: Vec<Line> = Vec::with_capacity(rows + 3);
        lines.push(Line::from(format!("Search: {}", self.state.query)));
        if visible.is_empty() {
            lines.push(Line::from("No web providers match."));
        } else {
            for (pos, &idx) in visible.iter().enumerate() {
                let row = &self.state.rows[idx];
                let marker = if pos == self.state.selected {
                    "❯ "
                } else if row.active {
                    "✓ "
                } else {
                    "  "
                };
                let style = if pos == self.state.selected {
                    Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
                } else {
                    Style::default()
                };
                lines.push(Line::from(Span::styled(
                    format!("{marker}{:<12}{}", row.label, row.description),
                    style,
                )));
            }
        }
        lines.push(Line::from(Span::styled(
            "type to search · Enter configure · t test · Esc close",
            Style::default().add_modifier(Modifier::DIM),
        )));
        Paragraph::new(lines).render(inner, buf);
    }

    fn desired_height(&self, _width: u16) -> u16 {
        let rows = self.state.visible_indices().len().max(1);
        u16::try_from(rows).unwrap_or(0) + 6
    }
}

impl BottomPaneView for WebPickerView {
    fn handle_key(&mut self, key: KeyEvent) -> ViewOutcome {
        match handle_web_picker_key(&mut self.state, key.code) {
            WebPickerOutcome::Stay => ViewOutcome::Pending,
            WebPickerOutcome::Select(provider) => ViewOutcome::OpenView(Box::new(
                WebConfigView::new(provider, self.state.snapshot.clone()),
            )),
            WebPickerOutcome::Test(provider) => ViewOutcome::RunWebAction(WebAction::TestSearch {
                provider,
                typed_key: None,
            }),
            WebPickerOutcome::Cancel => ViewOutcome::Cancelled,
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyModifiers};
    use tool_web::web_search_config::WebSearchProvider;

    use super::*;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn view() -> WebPickerView {
        WebPickerView::new(WebConfigSnapshot::default())
    }

    #[test]
    fn enter_on_first_row_opens_config_view_for_auto() {
        let mut v = view();
        // Auto is the first row (provider_order()).
        let outcome = v.handle_key(press(KeyCode::Enter));
        match outcome {
            ViewOutcome::OpenView(child) => {
                let cfg = child
                    .as_any()
                    .downcast_ref::<WebConfigView>()
                    .expect("opens a WebConfigView");
                assert_eq!(cfg.state.provider, WebSearchProvider::Auto);
            }
            _ => panic!("expected OpenView, got a different outcome"),
        }
    }

    #[test]
    fn enter_on_tavily_opens_its_config_view() {
        let mut v = view();
        v.handle_key(press(KeyCode::Down)); // DuckDuckGo
        v.handle_key(press(KeyCode::Down)); // Tavily
        let outcome = v.handle_key(press(KeyCode::Enter));
        match outcome {
            ViewOutcome::OpenView(child) => {
                let cfg = child
                    .as_any()
                    .downcast_ref::<WebConfigView>()
                    .expect("opens a WebConfigView");
                assert_eq!(cfg.state.provider, WebSearchProvider::Tavily);
            }
            _ => panic!("expected OpenView, got a different outcome"),
        }
    }

    #[test]
    fn t_runs_a_test_search_for_the_highlighted_provider() {
        let mut v = view();
        let outcome = v.handle_key(press(KeyCode::Char('t')));
        assert!(matches!(
            outcome,
            ViewOutcome::RunWebAction(WebAction::TestSearch {
                provider: WebSearchProvider::Auto,
                typed_key: None,
            })
        ));
    }

    #[test]
    fn esc_cancels() {
        let mut v = view();
        assert!(matches!(
            v.handle_key(press(KeyCode::Esc)),
            ViewOutcome::Cancelled
        ));
    }

    #[test]
    fn typing_filters_and_navigation_keeps_view_open() {
        let mut v = view();
        for c in "brave".chars() {
            assert!(matches!(
                v.handle_key(press(KeyCode::Char(c))),
                ViewOutcome::Pending
            ));
        }
        assert_eq!(v.state.visible_indices().len(), 1);
        let outcome = v.handle_key(press(KeyCode::Enter));
        match outcome {
            ViewOutcome::OpenView(child) => {
                let cfg = child
                    .as_any()
                    .downcast_ref::<WebConfigView>()
                    .expect("opens a WebConfigView");
                assert_eq!(cfg.state.provider, WebSearchProvider::Brave);
            }
            _ => panic!("expected OpenView, got a different outcome"),
        }
    }

    #[test]
    fn renders_title_and_footer() {
        let v = view();
        let area = Rect::new(0, 0, 60, 12);
        let mut buf = Buffer::empty(area);
        v.render(area, &mut buf);
        let text: String = (area.top()..area.bottom())
            .map(|y| {
                (area.left()..area.right())
                    .map(|x| {
                        buf.cell(ratatui::layout::Position::new(x, y))
                            .map_or(" ", ratatui::buffer::Cell::symbol)
                    })
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Configure web search"), "{text}");
        assert!(text.contains("Enter configure"), "{text}");
        assert!(text.contains("Auto"), "{text}");
    }
}

//! `/connect` login-method choice screen: shown for providers that offer
//! more than one method (Anthropic: Pro/Max OAuth + API key today), backed
//! by the pure [`crate::connect::method`] reducer.
//!
//! Opened as a child of
//! [`crate::bottom_pane::connect_picker_view::ConnectPickerView`] via
//! [`crate::bottom_pane::view::ViewOutcome::OpenView`]. Up/Down move the
//! highlight, `Enter` picks the highlighted method and routes into its flow
//! (the key-entry view for `ApiKey`, a [`ConnectAction`] effect for
//! Copilot/OAuth), `Esc` cancels the whole `/connect` flow.

use std::any::Any;

use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

use crate::bottom_pane::connect_key_view::ConnectKeyView;
use crate::bottom_pane::dialog_view::centered_rect;
use crate::bottom_pane::view::{BottomPaneView, ConnectAction, ViewOutcome};
use crate::connect::method::{handle_connect_method_key, ConnectMethodState, MethodChoiceOutcome};
use crate::connect::picker::ConnectMethod;
use crate::renderable::Renderable;

/// The `/connect` login-method choice view.
pub struct ConnectMethodView {
    state: ConnectMethodState,
}

impl ConnectMethodView {
    /// Build the choice screen for `provider_id`/`label` over `options`
    /// (from [`crate::connect::picker::provider_methods`]).
    #[must_use]
    pub fn new(provider_id: String, label: String, options: Vec<ConnectMethod>) -> Self {
        Self {
            state: ConnectMethodState::new(provider_id, label, options),
        }
    }

    /// The provider id this screen is choosing a method for (tests/routing).
    #[must_use]
    pub fn provider_id(&self) -> &str {
        &self.state.provider_id
    }
}

impl Renderable for ConnectMethodView {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let title = format!("Connect {}", self.state.label);
        let footer = "↑↓ select · Enter · Esc to cancel";
        let content_width = self
            .state
            .options
            .iter()
            .map(|o| o.choice_label().len() + 2)
            .max()
            .unwrap_or(0)
            .max(title.len())
            .max(footer.len());
        let width = u16::try_from(content_width + 4)
            .unwrap_or(u16::MAX)
            .min(area.width.saturating_sub(4))
            .max(30);
        let rows = self.state.options.len().max(1);
        let height = u16::try_from(rows + 4)
            .unwrap_or(u16::MAX)
            .min(area.height);
        let rect = centered_rect(width, height, area);

        Clear.render(rect, buf);
        let block = Block::new().borders(Borders::ALL).title(title);
        let inner = block.inner(rect);
        block.render(rect, buf);

        let mut lines: Vec<Line> = Vec::with_capacity(rows + 1);
        for (i, opt) in self.state.options.iter().enumerate() {
            let marker = if i == self.state.selected { "❯ " } else { "  " };
            let style = if i == self.state.selected {
                Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
            } else {
                Style::default()
            };
            lines.push(Line::from(Span::styled(
                format!("{marker}{}", opt.choice_label()),
                style,
            )));
        }
        lines.push(Line::from(Span::styled(
            footer,
            Style::default().add_modifier(Modifier::DIM),
        )));
        Paragraph::new(lines).render(inner, buf);
    }

    fn desired_height(&self, _width: u16) -> u16 {
        u16::try_from(self.state.options.len().max(1)).unwrap_or(0) + 4
    }
}

impl BottomPaneView for ConnectMethodView {
    fn handle_key(&mut self, key: KeyEvent) -> ViewOutcome {
        match handle_connect_method_key(&mut self.state, key.code) {
            MethodChoiceOutcome::Stay => ViewOutcome::Pending,
            MethodChoiceOutcome::Cancel => ViewOutcome::Cancelled,
            MethodChoiceOutcome::Pick { method } => {
                let provider_id = self.state.provider_id.clone();
                match method {
                    ConnectMethod::ApiKey => {
                        ViewOutcome::OpenView(Box::new(ConnectKeyView::new(&provider_id)))
                    }
                    ConnectMethod::CopilotDevice => {
                        ViewOutcome::RunConnectAction(ConnectAction::Copilot { provider_id })
                    }
                    ConnectMethod::Oauth | ConnectMethod::OAuthSoon => {
                        ViewOutcome::RunConnectAction(ConnectAction::OAuth { provider_id })
                    }
                }
            }
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyModifiers};

    use super::*;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn anthropic() -> ConnectMethodView {
        ConnectMethodView::new(
            "anthropic".to_string(),
            "Anthropic".to_string(),
            vec![ConnectMethod::Oauth, ConnectMethod::ApiKey],
        )
    }

    #[test]
    fn pick_api_key_opens_the_key_view() {
        let mut v = anthropic();
        v.handle_key(press(KeyCode::Down)); // ApiKey
        let outcome = v.handle_key(press(KeyCode::Enter));
        match outcome {
            ViewOutcome::OpenView(child) => {
                let key_view = child
                    .as_any()
                    .downcast_ref::<ConnectKeyView>()
                    .expect("opens a ConnectKeyView");
                assert_eq!(key_view.provider_id(), "anthropic");
            }
            _ => panic!("expected OpenView(ConnectKeyView)"),
        }
    }

    #[test]
    fn pick_oauth_runs_the_oauth_action() {
        let mut v = anthropic();
        let outcome = v.handle_key(press(KeyCode::Enter));
        assert!(matches!(
            outcome,
            ViewOutcome::RunConnectAction(ConnectAction::OAuth { ref provider_id })
                if provider_id == "anthropic"
        ));
    }

    #[test]
    fn pick_copilot_runs_the_copilot_action() {
        let mut v = ConnectMethodView::new(
            "github-copilot".to_string(),
            "GitHub Copilot".to_string(),
            vec![ConnectMethod::CopilotDevice],
        );
        let outcome = v.handle_key(press(KeyCode::Enter));
        assert!(matches!(
            outcome,
            ViewOutcome::RunConnectAction(ConnectAction::Copilot { ref provider_id })
                if provider_id == "github-copilot"
        ));
    }

    #[test]
    fn esc_cancels() {
        let mut v = anthropic();
        assert!(matches!(
            v.handle_key(press(KeyCode::Esc)),
            ViewOutcome::Cancelled
        ));
    }

    #[test]
    fn renders_title_and_both_labels() {
        let v = anthropic();
        let area = Rect::new(0, 0, 60, 8);
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
        assert!(text.contains("Connect Anthropic"), "{text}");
        assert!(text.contains("Sign in with Claude Pro/Max"), "{text}");
        assert!(text.contains("Use an API key"), "{text}");
    }
}

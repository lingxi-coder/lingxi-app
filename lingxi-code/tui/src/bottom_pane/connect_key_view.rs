//! `/connect` API-key entry screen: a masked key-input field for one
//! provider, backed by the pure [`crate::connect::screen`] reducer's
//! `ApiKey` flow.
//!
//! Opened as a child of
//! [`crate::bottom_pane::connect_picker_view::ConnectPickerView`] or
//! [`crate::bottom_pane::connect_method_view::ConnectMethodView`] via
//! [`crate::bottom_pane::view::ViewOutcome::OpenView`]. Typing edits the
//! masked buffer, `Enter` submits a non-empty key as
//! [`ViewOutcome::RunConnectAction`], `Esc` cancels.

use std::any::Any;

use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

use crate::bottom_pane::dialog_view::centered_rect;
use crate::bottom_pane::view::{BottomPaneView, ConnectAction, ViewOutcome};
use crate::connect::picker::provider_label;
use crate::connect::screen::{
    handle_connect_key, ConnectAction as ScreenAction, ConnectFlow, ConnectScreenState,
};
use crate::renderable::Renderable;

/// The `/connect` API-key entry view.
pub struct ConnectKeyView {
    state: ConnectScreenState,
}

impl ConnectKeyView {
    /// Build the masked key-entry field for `provider_id` (the header uses
    /// the curated [`provider_label`]).
    #[must_use]
    pub fn new(provider_id: &str) -> Self {
        Self {
            state: ConnectScreenState::api_key(provider_id, &provider_label(provider_id)),
        }
    }

    /// The provider id this screen is collecting a key for (tests/routing).
    #[must_use]
    pub fn provider_id(&self) -> &str {
        match &self.state.flow {
            ConnectFlow::ApiKey { provider_id, .. } => provider_id.as_str(),
            _ => "",
        }
    }

    fn label(&self) -> &str {
        match &self.state.flow {
            ConnectFlow::ApiKey { label, .. } => label.as_str(),
            _ => "",
        }
    }
}

impl Renderable for ConnectKeyView {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let title = format!("Connect {}", self.label());
        let masked: String = "\u{2022}".repeat(self.state.key_buffer.chars().count());
        let key_line = format!("Key: {masked}");
        let footer = "Paste your API key · Enter to save · Esc to cancel";
        let content_width = [title.len(), key_line.len(), footer.len()]
            .into_iter()
            .max()
            .unwrap_or(0);
        let width = u16::try_from(content_width + 4)
            .unwrap_or(u16::MAX)
            .min(area.width.saturating_sub(4))
            .max(30);
        let height = u16::try_from(2 + 4)
            .unwrap_or(u16::MAX)
            .min(area.height);
        let rect = centered_rect(width, height, area);

        Clear.render(rect, buf);
        let block = Block::new().borders(Borders::ALL).title(title);
        let inner = block.inner(rect);
        block.render(rect, buf);

        let lines = vec![
            Line::from(key_line),
            Line::from(Span::styled(footer, Style::default().add_modifier(Modifier::DIM))),
        ];
        Paragraph::new(lines).render(inner, buf);
    }

    fn desired_height(&self, _width: u16) -> u16 {
        6
    }
}

impl BottomPaneView for ConnectKeyView {
    fn handle_key(&mut self, key: KeyEvent) -> ViewOutcome {
        match handle_connect_key(&mut self.state, key.code) {
            ScreenAction::None => ViewOutcome::Pending,
            ScreenAction::Cancel => ViewOutcome::Cancelled,
            ScreenAction::SubmitKey { provider_id, key } => {
                ViewOutcome::RunConnectAction(ConnectAction::StoreKey { provider_id, key })
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

    fn key_char(c: char) -> KeyEvent {
        press(KeyCode::Char(c))
    }

    #[test]
    fn enter_with_a_typed_key_runs_store_key() {
        let mut v = ConnectKeyView::new("deepseek");
        for ch in "sk-secret".chars() {
            assert!(matches!(v.handle_key(key_char(ch)), ViewOutcome::Pending));
        }
        let outcome = v.handle_key(press(KeyCode::Enter));
        assert!(matches!(
            outcome,
            ViewOutcome::RunConnectAction(ConnectAction::StoreKey {
                ref provider_id,
                ref key,
            }) if provider_id == "deepseek" && key == "sk-secret"
        ));
    }

    #[test]
    fn empty_enter_stays_pending() {
        let mut v = ConnectKeyView::new("deepseek");
        assert!(matches!(
            v.handle_key(press(KeyCode::Enter)),
            ViewOutcome::Pending
        ));
    }

    #[test]
    fn esc_cancels() {
        let mut v = ConnectKeyView::new("deepseek");
        assert!(matches!(
            v.handle_key(press(KeyCode::Esc)),
            ViewOutcome::Cancelled
        ));
    }

    #[test]
    fn render_masks_the_key_and_never_shows_it_raw() {
        let mut v = ConnectKeyView::new("deepseek");
        for ch in "sk-secret".chars() {
            v.handle_key(key_char(ch));
        }
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
        assert!(text.contains("Connect DeepSeek"), "{text}");
        assert!(text.contains("\u{2022}\u{2022}\u{2022}"), "{text}");
        assert!(!text.contains("sk-secret"), "raw key must never render: {text}");
    }
}

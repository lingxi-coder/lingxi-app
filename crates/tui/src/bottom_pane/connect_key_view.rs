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

use crossterm::cursor::SetCursorStyle;
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
    handle_connect_key, handle_connect_paste, ConnectAction as ScreenAction, ConnectFlow,
    ConnectScreenState,
};
use crate::renderable::Renderable;

/// Display columns of the `"Key: "` prefix on the key-entry line — where the
/// masked buffer (and thus the text cursor) begins.
const KEY_PREFIX_COLS: u16 = 5;

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

    /// The centered dialog rect. Shared by [`Renderable::render`] and
    /// [`Renderable::cursor_pos`] so the text cursor lands exactly on the
    /// rendered "Key:" line (any drift between the two would misplace it).
    fn block_rect(&self, area: Rect) -> Rect {
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
        let height = u16::try_from(2 + 4).unwrap_or(u16::MAX).min(area.height);
        centered_rect(width, height, area)
    }
}

impl Renderable for ConnectKeyView {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let title = format!("Connect {}", self.label());
        let masked: String = "\u{2022}".repeat(self.state.key_buffer.chars().count());
        let key_line = format!("Key: {masked}");
        let footer = "Paste your API key · Enter to save · Esc to cancel";
        let rect = self.block_rect(area);

        Clear.render(rect, buf);
        let block = Block::new().borders(Borders::ALL).title(title);
        let inner = block.inner(rect);
        block.render(rect, buf);

        let lines = vec![
            Line::from(key_line),
            Line::from(Span::styled(
                footer,
                Style::default().add_modifier(Modifier::DIM),
            )),
        ];
        Paragraph::new(lines).render(inner, buf);
    }

    fn desired_height(&self, _width: u16) -> u16 {
        6
    }

    /// Claim a text cursor at the end of the masked key (the first inner row,
    /// after `"Key: "`), so the field reads like an editable input rather than
    /// a static line. Clamped inside the box.
    fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
        let inner = Block::new()
            .borders(Borders::ALL)
            .inner(self.block_rect(area));
        if inner.width == 0 || inner.height == 0 {
            return None;
        }
        let typed = u16::try_from(self.state.key_buffer.chars().count()).unwrap_or(u16::MAX);
        let x = inner
            .x
            .saturating_add(KEY_PREFIX_COLS)
            .saturating_add(typed)
            .min(inner.right().saturating_sub(1));
        Some((x, inner.y))
    }

    fn cursor_style(&self, _area: Rect) -> SetCursorStyle {
        SetCursorStyle::SteadyBar
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

    /// Pasting the API key (⌘V) appends into the masked buffer — the primary
    /// way keys are entered (the footer literally says "Paste your API key").
    /// Without this the modal's default swallows the paste and nothing happens.
    fn handle_paste(&mut self, text: &str) -> ViewOutcome {
        handle_connect_paste(&mut self.state, text);
        ViewOutcome::Pending
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn is_connect_flow(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use crossterm::cursor::SetCursorStyle;
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
    fn paste_fills_the_masked_key_and_enter_submits_it() {
        // The reported bug: ⌘V did nothing (the modal's default handle_paste
        // swallowed it). Paste must append into the field; Enter then stores it.
        let mut v = ConnectKeyView::new("openrouter");
        assert!(matches!(
            v.handle_paste("sk-or-v1-abc\n"),
            ViewOutcome::Pending
        ));
        let outcome = v.handle_key(press(KeyCode::Enter));
        assert!(matches!(
            outcome,
            ViewOutcome::RunConnectAction(ConnectAction::StoreKey { ref provider_id, ref key })
                if provider_id == "openrouter" && key == "sk-or-v1-abc"
        ));
    }

    #[test]
    fn claims_a_bar_cursor_that_tracks_the_end_of_the_masked_key() {
        // The reported bug: the cursor never moved into the field. The view now
        // claims a bar cursor after "Key: ", advancing one column per char.
        let mut v = ConnectKeyView::new("openrouter");
        let area = Rect::new(0, 0, 80, 12);
        let empty = v.cursor_pos(area).expect("claims a cursor even when empty");
        for _ in 0..4 {
            v.handle_key(press(KeyCode::Char('x')));
        }
        let filled = v.cursor_pos(area).expect("still claims a cursor");
        assert_eq!(filled.1, empty.1, "cursor stays on the key row");
        assert_eq!(filled.0, empty.0 + 4, "one column per masked char");
        assert!(matches!(v.cursor_style(area), SetCursorStyle::SteadyBar));
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
        assert!(
            !text.contains("sk-secret"),
            "raw key must never render: {text}"
        );
    }
}

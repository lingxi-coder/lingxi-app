//! `/web` provider configuration screen: text entry (API key / SearXNG URL)
//! and test-search status for one selected provider, backed by the pure
//! [`crate::web::config`] reducer.
//!
//! Opened as a child of [`crate::bottom_pane::web_picker_view::WebPickerView`]
//! via [`crate::bottom_pane::view::ViewOutcome::OpenView`]. `Enter` saves
//! (secret or settings, depending on the provider), `t` runs a test search
//! for keyless/already-configured providers, `Esc` cancels back to the
//! picker.

use std::any::Any;

use crossterm::cursor::SetCursorStyle;
use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};
use tool_web::web_search_config::WebSearchProvider;

use crate::bottom_pane::dialog_view::centered_rect;
use crate::bottom_pane::view::{BottomPaneView, ViewOutcome, WebAction};
use crate::renderable::Renderable;
use crate::web::config::{
    handle_web_config_key, handle_web_config_paste, WebConfigOutcome, WebConfigState,
};
use crate::web::picker::{provider_label, WebConfigSnapshot};

/// Display columns of the `"Input: "` prefix on the config input line — where
/// the value (and thus the text cursor) begins.
const INPUT_PREFIX_COLS: u16 = 7;

/// Providers whose config screen requires a typed value (API key or URL).
/// Auto/DuckDuckGo need no input; Tavily/Brave take a secret key; SearXNG
/// takes a URL (shown raw, not masked).
fn requires_input(provider: WebSearchProvider) -> bool {
    matches!(
        provider,
        WebSearchProvider::Tavily | WebSearchProvider::Brave | WebSearchProvider::Searxng
    )
}

fn is_masked(provider: WebSearchProvider) -> bool {
    matches!(provider, WebSearchProvider::Tavily | WebSearchProvider::Brave)
}

/// The `/web` per-provider configuration view.
pub struct WebConfigView {
    pub(crate) state: WebConfigState,
}

impl WebConfigView {
    /// Build the config screen for `provider` from the current `/web` snapshot.
    #[must_use]
    pub fn new(provider: WebSearchProvider, snapshot: WebConfigSnapshot) -> Self {
        Self {
            state: WebConfigState::new(provider, snapshot),
        }
    }

    /// Whether this provider shows an editable input line.
    fn show_input(&self) -> bool {
        requires_input(self.state.provider)
    }

    /// The value shown on the input line — masked for secret providers, raw
    /// for the SearXNG URL — or `None` for keyless providers.
    fn input_shown(&self) -> Option<String> {
        self.show_input().then(|| {
            if is_masked(self.state.provider) {
                "*".repeat(self.state.input.chars().count())
            } else {
                self.state.input.clone()
            }
        })
    }

    fn footer(&self) -> &'static str {
        if self.show_input() {
            "Enter save · Esc cancel"
        } else {
            "Enter save · t test · Esc cancel"
        }
    }

    /// The centered dialog rect. Shared by [`Renderable::render`] and
    /// [`Renderable::cursor_pos`] so the text cursor lands exactly on the
    /// rendered "Input:" line.
    fn block_rect(&self, area: Rect) -> Rect {
        let title = format!("Configure {}", provider_label(self.state.provider));
        let status = self.state.status_text();
        let input_line = self.input_shown();
        let footer = self.footer();
        let content_width = [title.len(), status.len(), footer.len()]
            .into_iter()
            .chain(input_line.as_ref().map(|s| s.len() + 7))
            .max()
            .unwrap_or(0);
        let width = u16::try_from(content_width + 4)
            .unwrap_or(u16::MAX)
            .min(area.width.saturating_sub(4))
            .max(30);
        let rows = 2 + usize::from(self.show_input());
        let height = u16::try_from(rows + 4).unwrap_or(u16::MAX).min(area.height);
        centered_rect(width, height, area)
    }
}

impl Renderable for WebConfigView {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let title = format!("Configure {}", provider_label(self.state.provider));
        let status = self.state.status_text();
        let input_line = self.input_shown();
        let footer = self.footer();
        let rect = self.block_rect(area);

        Clear.render(rect, buf);
        let block = Block::new().borders(Borders::ALL).title(title);
        let inner = block.inner(rect);
        block.render(rect, buf);

        let mut lines: Vec<Line> = vec![Line::from(status)];
        if let Some(shown) = input_line {
            lines.push(Line::from(format!("Input: {shown}")));
        }
        lines.push(Line::from(Span::styled(
            footer,
            Style::default().add_modifier(Modifier::DIM),
        )));
        Paragraph::new(lines).render(inner, buf);
    }

    fn desired_height(&self, _width: u16) -> u16 {
        let rows = 2 + usize::from(requires_input(self.state.provider));
        u16::try_from(rows).unwrap_or(0) + 4
    }

    /// Claim a text cursor at the end of the input value (the SECOND inner row,
    /// after the status line and the `"Input: "` prefix). Keyless providers
    /// have no field and claim nothing.
    fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
        if !self.show_input() {
            return None;
        }
        let inner = Block::new().borders(Borders::ALL).inner(self.block_rect(area));
        if inner.width == 0 || inner.height < 2 {
            return None;
        }
        let typed = u16::try_from(self.state.input.chars().count()).unwrap_or(u16::MAX);
        let x = inner
            .x
            .saturating_add(INPUT_PREFIX_COLS)
            .saturating_add(typed)
            .min(inner.right().saturating_sub(1));
        Some((x, inner.y + 1))
    }

    fn cursor_style(&self, _area: Rect) -> SetCursorStyle {
        SetCursorStyle::SteadyBar
    }
}

impl BottomPaneView for WebConfigView {
    fn handle_key(&mut self, key: KeyEvent) -> ViewOutcome {
        match handle_web_config_key(&mut self.state, key.code) {
            WebConfigOutcome::Stay => ViewOutcome::Pending,
            WebConfigOutcome::Close => ViewOutcome::Cancelled,
            WebConfigOutcome::SaveSecret { provider, secret } => {
                ViewOutcome::RunWebAction(WebAction::SaveSecret { provider, secret })
            }
            WebConfigOutcome::SaveSettings {
                provider,
                searxng_url,
            } => ViewOutcome::RunWebAction(WebAction::SaveSettings {
                provider,
                searxng_url,
            }),
            WebConfigOutcome::Test(provider) => {
                let typed_key = {
                    let trimmed = self.state.input.trim();
                    (!trimmed.is_empty()).then(|| trimmed.to_string())
                };
                ViewOutcome::RunWebAction(WebAction::TestSearch {
                    provider,
                    typed_key,
                })
            }
        }
    }

    /// Pasting (⌘V) an API key or SearXNG URL appends into the input buffer;
    /// the modal's default would otherwise swallow the paste with no effect.
    fn handle_paste(&mut self, text: &str) -> ViewOutcome {
        handle_web_config_paste(&mut self.state, text);
        ViewOutcome::Pending
    }

    fn as_any(&self) -> &dyn Any {
        self
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
    fn enter_with_a_tavily_key_saves_secret() {
        let mut v = WebConfigView::new(WebSearchProvider::Tavily, WebConfigSnapshot::default());
        for ch in "tvly-secret".chars() {
            assert!(matches!(v.handle_key(key_char(ch)), ViewOutcome::Pending));
        }
        let outcome = v.handle_key(press(KeyCode::Enter));
        assert!(matches!(
            outcome,
            ViewOutcome::RunWebAction(WebAction::SaveSecret {
                provider: WebSearchProvider::Tavily,
                ref secret,
            }) if secret == "tvly-secret"
        ));
    }

    #[test]
    fn enter_with_a_searxng_url_saves_settings() {
        let mut v = WebConfigView::new(WebSearchProvider::Searxng, WebConfigSnapshot::default());
        for ch in "https://s.example".chars() {
            v.handle_key(key_char(ch));
        }
        let outcome = v.handle_key(press(KeyCode::Enter));
        assert!(matches!(
            outcome,
            ViewOutcome::RunWebAction(WebAction::SaveSettings {
                provider: WebSearchProvider::Searxng,
                ref searxng_url,
            }) if searxng_url.as_deref() == Some("https://s.example")
        ));
    }

    #[test]
    fn t_runs_test_search_on_a_keyless_provider() {
        let mut v = WebConfigView::new(WebSearchProvider::DuckDuckGo, WebConfigSnapshot::default());
        let outcome = v.handle_key(key_char('t'));
        assert!(matches!(
            outcome,
            ViewOutcome::RunWebAction(WebAction::TestSearch {
                provider: WebSearchProvider::DuckDuckGo,
                typed_key: None,
            })
        ));
    }

    #[test]
    fn t_test_on_auto_uses_no_typed_key() {
        let mut v = WebConfigView::new(WebSearchProvider::Auto, WebConfigSnapshot::default());
        let outcome = v.handle_key(key_char('t'));
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
        let mut v = WebConfigView::new(WebSearchProvider::Auto, WebConfigSnapshot::default());
        assert!(matches!(v.handle_key(press(KeyCode::Esc)), ViewOutcome::Cancelled));
    }

    #[test]
    fn empty_tavily_secret_stays_open_with_a_failed_status() {
        let mut v = WebConfigView::new(WebSearchProvider::Tavily, WebConfigSnapshot::default());
        let outcome = v.handle_key(press(KeyCode::Enter));
        assert!(matches!(outcome, ViewOutcome::Pending));
        assert!(v.state.status_text().contains("cannot be empty"));
    }

    #[test]
    fn render_masks_tavily_key_but_shows_searxng_url_raw() {
        let mut v = WebConfigView::new(WebSearchProvider::Tavily, WebConfigSnapshot::default());
        for ch in "secret123".chars() {
            v.handle_key(key_char(ch));
        }
        let area = Rect::new(0, 0, 60, 10);
        let mut buf = Buffer::empty(area);
        v.render(area, &mut buf);
        let text = render_text(&buf, area);
        assert!(text.contains("*********"), "{text}");
        assert!(!text.contains("secret123"), "{text}");
        assert!(!text.contains("· t test"), "no test hint for Tavily: {text}");

        let mut v = WebConfigView::new(WebSearchProvider::Searxng, WebConfigSnapshot::default());
        for ch in "https://s.example".chars() {
            v.handle_key(key_char(ch));
        }
        let mut buf = Buffer::empty(area);
        v.render(area, &mut buf);
        let text = render_text(&buf, area);
        assert!(text.contains("https://s.example"), "{text}");
    }

    #[test]
    fn render_shows_test_hint_for_keyless_provider() {
        let v = WebConfigView::new(WebSearchProvider::DuckDuckGo, WebConfigSnapshot::default());
        let area = Rect::new(0, 0, 60, 10);
        let mut buf = Buffer::empty(area);
        v.render(area, &mut buf);
        let text = render_text(&buf, area);
        assert!(text.contains("t test"), "{text}");
    }

    #[test]
    fn paste_fills_the_key_field_and_enter_saves_it() {
        let mut v = WebConfigView::new(WebSearchProvider::Tavily, WebConfigSnapshot::default());
        assert!(matches!(
            v.handle_paste("tvly-secret\n"),
            ViewOutcome::Pending
        ));
        let outcome = v.handle_key(press(KeyCode::Enter));
        assert!(matches!(
            outcome,
            ViewOutcome::RunWebAction(WebAction::SaveSecret {
                provider: WebSearchProvider::Tavily,
                ref secret,
            }) if secret == "tvly-secret"
        ));
    }

    #[test]
    fn claims_a_bar_cursor_on_the_input_row_and_nothing_for_keyless_providers() {
        let mut v = WebConfigView::new(WebSearchProvider::Tavily, WebConfigSnapshot::default());
        let area = Rect::new(0, 0, 80, 12);
        let empty = v.cursor_pos(area).expect("an input provider claims a cursor");
        for _ in 0..3 {
            v.handle_key(key_char('k'));
        }
        let filled = v.cursor_pos(area).expect("still claims a cursor");
        assert_eq!(filled.1, empty.1, "cursor stays on the input row");
        assert_eq!(filled.0, empty.0 + 3, "one column per typed char");
        assert!(matches!(v.cursor_style(area), SetCursorStyle::SteadyBar));
        // Keyless providers (Auto/DuckDuckGo) have no field → no cursor claim.
        let keyless = WebConfigView::new(WebSearchProvider::DuckDuckGo, WebConfigSnapshot::default());
        assert_eq!(keyless.cursor_pos(area), None);
    }

    fn render_text(buf: &Buffer, area: Rect) -> String {
        (area.top()..area.bottom())
            .map(|y| {
                (area.left()..area.right())
                    .map(|x| {
                        buf.cell(ratatui::layout::Position::new(x, y))
                            .map_or(" ", ratatui::buffer::Cell::symbol)
                    })
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

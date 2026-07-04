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
use crate::web::config::{handle_web_config_key, WebConfigOutcome, WebConfigState};
use crate::web::picker::{provider_label, WebConfigSnapshot};

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
}

impl Renderable for WebConfigView {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let label = provider_label(self.state.provider);
        let title = format!("Configure {label}");
        let status = self.state.status_text();
        let show_input = requires_input(self.state.provider);
        let input_line = show_input.then(|| {
            if is_masked(self.state.provider) {
                "*".repeat(self.state.input.chars().count())
            } else {
                self.state.input.clone()
            }
        });
        let footer = if show_input {
            "Enter save · Esc cancel"
        } else {
            "Enter save · t test · Esc cancel"
        };

        let content_width = [title.len(), status.len(), footer.len()]
            .into_iter()
            .chain(input_line.as_ref().map(|s| s.len() + 7))
            .max()
            .unwrap_or(0);
        let width = u16::try_from(content_width + 4)
            .unwrap_or(u16::MAX)
            .min(area.width.saturating_sub(4))
            .max(30);
        let rows = 2 + usize::from(show_input);
        let height = u16::try_from(rows + 4)
            .unwrap_or(u16::MAX)
            .min(area.height);
        let rect = centered_rect(width, height, area);

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

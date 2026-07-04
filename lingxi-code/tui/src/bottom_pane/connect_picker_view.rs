//! `/connect` provider picker: a centered, grouped, search-filterable list of
//! the connectable LLM providers, backed by the pure
//! [`crate::connect::picker`] reducer.
//!
//! Modeled on [`crate::bottom_pane::web_picker_view::WebPickerView`]: arrow
//! keys move the highlight over selectable rows (headers are skipped),
//! typing filters (`query`), `Enter` routes into the method-choice screen
//! ([`crate::bottom_pane::connect_method_view::ConnectMethodView`]) for
//! multi-method providers or straight into the matching flow for
//! single-method ones, `Esc` cancels.

use std::any::Any;
use std::collections::BTreeMap;

use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

use crate::bottom_pane::connect_key_view::ConnectKeyView;
use crate::bottom_pane::connect_method_view::ConnectMethodView;
use crate::bottom_pane::dialog_view::centered_rect;
use crate::bottom_pane::view::{BottomPaneView, ConnectAction, ViewOutcome};
use crate::connect::picker::{
    handle_connect_picker_key, provider_label, provider_methods, ConnectMethod,
    ConnectPickerOutcome, ConnectPickerState, VisibleLine, SUB_HEADER,
};
use crate::renderable::Renderable;

/// The `/connect` provider picker view.
pub struct ConnectPickerView {
    state: ConnectPickerState,
    /// Per-provider login-method tag (from the catalog), keyed by
    /// `provider_id` — needed to route `Select` into the right flow.
    auth_methods: BTreeMap<String, String>,
}

impl ConnectPickerView {
    /// Build the picker from `auth_methods` (provider set + method tag)
    /// joined with `availability` (which providers already have a usable
    /// credential).
    #[must_use]
    pub fn new(auth_methods: BTreeMap<String, String>, availability: BTreeMap<String, bool>) -> Self {
        Self {
            state: ConnectPickerState::from_connectable(&auth_methods, &availability),
            auth_methods,
        }
    }

    /// Route a selected `provider_id` into the matching outcome: the
    /// method-choice screen for multi-method providers, or straight into the
    /// single method's flow otherwise.
    fn route_selection(&self, provider_id: String) -> ViewOutcome {
        let tag = self.auth_methods.get(&provider_id).map(String::as_str);
        let methods = provider_methods(&provider_id, tag);
        if methods.len() > 1 {
            let label = provider_label(&provider_id);
            return ViewOutcome::OpenView(Box::new(ConnectMethodView::new(
                provider_id, label, methods,
            )));
        }
        match methods.first().copied() {
            Some(ConnectMethod::ApiKey) | None => {
                ViewOutcome::OpenView(Box::new(ConnectKeyView::new(&provider_id)))
            }
            Some(ConnectMethod::CopilotDevice) => {
                ViewOutcome::RunConnectAction(ConnectAction::Copilot { provider_id })
            }
            Some(ConnectMethod::Oauth | ConnectMethod::OAuthSoon) => {
                ViewOutcome::RunConnectAction(ConnectAction::OAuth { provider_id })
            }
        }
    }
}

impl Renderable for ConnectPickerView {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let lines = self.state.visible_lines();
        let content_width = lines
            .iter()
            .map(|line| match line {
                VisibleLine::Header(h) => h.chars().count(),
                VisibleLine::Item(idx) => {
                    let row = &self.state.rows[*idx];
                    row.label.chars().count() + 2 + row.description.chars().count() + 2
                }
            })
            .max()
            .unwrap_or(0)
            .max("Connect a provider".len())
            .max(SUB_HEADER.len());
        let width = u16::try_from(content_width + 4)
            .unwrap_or(u16::MAX)
            .min(area.width.saturating_sub(4))
            .max(30);
        let item_count = lines.len().max(1);
        let height = u16::try_from(item_count + 7)
            .unwrap_or(u16::MAX)
            .min(area.height);
        let rect = centered_rect(width, height, area);

        Clear.render(rect, buf);
        let block = Block::new()
            .borders(Borders::ALL)
            .title("Connect a provider");
        let inner = block.inner(rect);
        block.render(rect, buf);

        let mut out: Vec<Line> = Vec::with_capacity(item_count + 4);
        out.push(Line::from(Span::styled(
            SUB_HEADER,
            Style::default().add_modifier(Modifier::DIM),
        )));
        out.push(Line::from(format!("Search: {}", self.state.query)));
        if lines.is_empty() {
            out.push(Line::from("No providers available."));
        } else {
            let mut item_pos = 0usize;
            for line in &lines {
                match line {
                    VisibleLine::Header(label) => {
                        out.push(Line::from(Span::styled(
                            label.clone(),
                            Style::default().add_modifier(Modifier::DIM),
                        )));
                    }
                    VisibleLine::Item(idx) => {
                        let row = &self.state.rows[*idx];
                        let marker = if item_pos == self.state.selected {
                            "❯ "
                        } else if row.connected {
                            "✓ "
                        } else {
                            "  "
                        };
                        let style = if item_pos == self.state.selected {
                            Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
                        } else {
                            Style::default()
                        };
                        out.push(Line::from(Span::styled(
                            format!("{marker}{:<22}{}", row.label, row.description),
                            style,
                        )));
                        item_pos += 1;
                    }
                }
            }
        }
        out.push(Line::from(Span::styled(
            "type to search · Enter to select · Esc to cancel",
            Style::default().add_modifier(Modifier::DIM),
        )));
        Paragraph::new(out).render(inner, buf);
    }

    fn desired_height(&self, _width: u16) -> u16 {
        let item_count = self.state.visible_lines().len().max(1);
        u16::try_from(item_count).unwrap_or(0) + 7
    }
}

impl BottomPaneView for ConnectPickerView {
    fn handle_key(&mut self, key: KeyEvent) -> ViewOutcome {
        match handle_connect_picker_key(&mut self.state, key.code) {
            ConnectPickerOutcome::Stay => ViewOutcome::Pending,
            ConnectPickerOutcome::Select { provider_id } => self.route_selection(provider_id),
            ConnectPickerOutcome::Cancel => ViewOutcome::Cancelled,
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

    fn auth_methods() -> BTreeMap<String, String> {
        [
            ("anthropic", "api_key"),
            ("openrouter", "api_key"),
            ("github-copilot", "copilot_device"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
    }

    fn view() -> ConnectPickerView {
        ConnectPickerView::new(auth_methods(), BTreeMap::new())
    }

    #[test]
    fn enter_on_a_single_api_key_provider_opens_the_key_view() {
        let mut v = view();
        // Selectable order: Popular group first (anthropic, github-copilot —
        // both `popular: true`), then Providers (openrouter).
        v.handle_key(press(KeyCode::Down)); // github-copilot
        v.handle_key(press(KeyCode::Down)); // openrouter
        let outcome = v.handle_key(press(KeyCode::Enter));
        match outcome {
            ViewOutcome::OpenView(child) => {
                let key_view = child
                    .as_any()
                    .downcast_ref::<ConnectKeyView>()
                    .expect("opens a ConnectKeyView");
                assert_eq!(key_view.provider_id(), "openrouter");
            }
            _ => panic!("expected OpenView for a single api_key provider"),
        }
    }

    #[test]
    fn enter_on_anthropic_opens_the_method_choice_view() {
        let mut v = view();
        // Anthropic is dual-method (Oauth + ApiKey) — highlighted by default
        // (first Popular row).
        let outcome = v.handle_key(press(KeyCode::Enter));
        match outcome {
            ViewOutcome::OpenView(child) => {
                let method_view = child
                    .as_any()
                    .downcast_ref::<ConnectMethodView>()
                    .expect("opens a ConnectMethodView for anthropic");
                assert_eq!(method_view.provider_id(), "anthropic");
            }
            _ => panic!("expected OpenView(ConnectMethodView) for anthropic"),
        }
    }

    #[test]
    fn enter_on_copilot_provider_runs_the_copilot_action() {
        let mut v = view();
        v.handle_key(press(KeyCode::Down)); // github-copilot
        let outcome = v.handle_key(press(KeyCode::Enter));
        assert!(matches!(
            outcome,
            ViewOutcome::RunConnectAction(ConnectAction::Copilot { ref provider_id })
                if provider_id == "github-copilot"
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
    fn typing_filters_and_enter_opens_the_matching_provider() {
        let mut v = view();
        for c in "openrouter".chars() {
            assert!(matches!(
                v.handle_key(press(KeyCode::Char(c))),
                ViewOutcome::Pending
            ));
        }
        let outcome = v.handle_key(press(KeyCode::Enter));
        match outcome {
            ViewOutcome::OpenView(child) => {
                let key_view = child
                    .as_any()
                    .downcast_ref::<ConnectKeyView>()
                    .expect("opens a ConnectKeyView");
                assert_eq!(key_view.provider_id(), "openrouter");
            }
            _ => panic!("expected OpenView for the filtered openrouter row"),
        }
    }

    #[test]
    fn renders_title_subheader_and_footer() {
        let v = view();
        let area = Rect::new(0, 0, 70, 14);
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
        assert!(text.contains("Connect a provider"), "{text}");
        assert!(text.contains("Enter to select"), "{text}");
        assert!(text.contains("Anthropic"), "{text}");
    }
}

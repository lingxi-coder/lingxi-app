//! Scrollable model picker view for `/model`: a centered, bordered selection
//! list over the session's [`ModelRow`]s (ported from the former
//! `picker::ModelPicker`, plan Phase 4).
//!
//! Unlike the read-only [`crate::bottom_pane::screen_view::ScreenView`] it is
//! INTERACTIVE — arrow keys move a highlight (the viewport follows), `Enter`
//! confirms the model ([`ViewOutcome::SwitchModel`]), `Esc` cancels. The
//! currently active model is marked with a `●`. Modeled on codex's
//! `list_selection_view` + the modal contract of
//! [`crate::bottom_pane::dialog_view::DialogView`].

use std::any::Any;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

use crate::bottom_pane::dialog_view::centered_rect;
use crate::bottom_pane::view::{BottomPaneView, ViewOutcome};
use crate::renderable::Renderable;
use crate::session::ModelRow;

/// Rows shown in the picker viewport before it scrolls.
const VIEWPORT: usize = 12;

/// An interactive, scrollable model selection list.
pub struct ModelPickerView {
    rows: Vec<ModelRow>,
    selected: usize,
    /// Top row index of the scroll window.
    offset: usize,
}

impl ModelPickerView {
    /// Build a picker over `rows`, starting the highlight on the current model
    /// (the row with `is_current`), or the first row when none is marked.
    #[must_use]
    pub fn new(rows: Vec<ModelRow>) -> Self {
        let selected = rows.iter().position(|r| r.is_current).unwrap_or(0);
        let offset = selected.saturating_sub(VIEWPORT - 1);
        Self {
            rows,
            selected,
            offset,
        }
    }

    /// Whether the picker has any rows to choose from.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The currently highlighted row index (exposed for tests).
    #[must_use]
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Keep the highlighted row inside the scroll window.
    fn follow(&mut self) {
        if self.selected < self.offset {
            self.offset = self.selected;
        } else if self.selected >= self.offset + VIEWPORT {
            self.offset = self.selected + 1 - VIEWPORT;
        }
    }

    fn max_width(&self) -> usize {
        self.rows
            .iter()
            .map(|r| r.display.chars().count() + r.provider_label.chars().count() + 3)
            .max()
            .unwrap_or(20)
    }
}

impl Renderable for ModelPickerView {
    /// Draw the picker centered over `area`, clearing the buffer beneath it.
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let width = u16::try_from(self.max_width() + 6)
            .unwrap_or(u16::MAX)
            .min(area.width.saturating_sub(4))
            .max(24);
        let visible = self.rows.len().min(VIEWPORT);
        let height = u16::try_from(visible + 4)
            .unwrap_or(u16::MAX)
            .min(area.height);
        let rect = centered_rect(width, height, area);

        Clear.render(rect, buf);
        let block = Block::new().borders(Borders::ALL).title("Select model");
        let inner = block.inner(rect);
        block.render(rect, buf);

        let end = (self.offset + VIEWPORT).min(self.rows.len());
        let mut lines: Vec<Line> = Vec::with_capacity(visible + 1);
        for (i, row) in self.rows[self.offset..end].iter().enumerate() {
            let idx = self.offset + i;
            let marker = if row.is_current { "● " } else { "  " };
            let caret = if idx == self.selected { "› " } else { "  " };
            let label = if row.provider_label.is_empty() {
                row.display.clone()
            } else {
                format!("{} ({})", row.display, row.provider_label)
            };
            let style = if idx == self.selected {
                Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
            } else {
                Style::default()
            };
            lines.push(Line::from(Span::styled(
                format!("{caret}{marker}{label}"),
                style,
            )));
        }
        lines.push(Line::from(Span::styled(
            "↑/↓ select · Enter switch · Esc cancel",
            Style::default().add_modifier(Modifier::DIM),
        )));
        Paragraph::new(lines).render(inner, buf);
    }

    /// The bottom-viewport rows the picker claims: its visible rows + modal
    /// chrome (locked layout value: 2 models → 6 rows at 80x24).
    fn desired_height(&self, _width: u16) -> u16 {
        u16::try_from(self.rows.len()).unwrap_or(0).min(12) + 4
    }
}

impl BottomPaneView for ModelPickerView {
    /// Route a key: arrows move the highlight (viewport follows, clamped at
    /// the list edges — locked behavior), `Enter` confirms, `Esc` cancels.
    fn handle_key(&mut self, key: KeyEvent) -> ViewOutcome {
        match key.code {
            KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                self.follow();
                ViewOutcome::Pending
            }
            KeyCode::Down => {
                if self.selected + 1 < self.rows.len() {
                    self.selected += 1;
                }
                self.follow();
                ViewOutcome::Pending
            }
            KeyCode::Enter => self
                .rows
                .get(self.selected)
                .map_or(ViewOutcome::Cancelled, |r| ViewOutcome::SwitchModel {
                    request_model: r.request_model.clone(),
                    profile: r.profile.clone(),
                }),
            KeyCode::Esc => ViewOutcome::Cancelled,
            _ => ViewOutcome::Pending,
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyModifiers;

    use super::*;

    fn rows() -> Vec<ModelRow> {
        vec![
            ModelRow {
                display: "Opus".into(),
                request_model: "claude-opus".into(),
                profile: Some("anthropic".into()),
                provider_label: "Anthropic".into(),
                is_current: false,
            },
            ModelRow {
                display: "Sonnet".into(),
                request_model: "claude-sonnet".into(),
                profile: Some("anthropic".into()),
                provider_label: "Anthropic".into(),
                is_current: true,
            },
        ]
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn starts_on_current_model() {
        let p = ModelPickerView::new(rows());
        assert_eq!(p.selected(), 1);
    }

    #[test]
    fn enter_confirms_selected_request_model_and_profile() {
        let mut p = ModelPickerView::new(rows());
        p.handle_key(press(KeyCode::Up)); // move to Opus
        assert_eq!(p.selected(), 0);
        assert!(matches!(
            p.handle_key(press(KeyCode::Enter)),
            ViewOutcome::SwitchModel { ref request_model, ref profile }
                if request_model == "claude-opus" && profile.as_deref() == Some("anthropic")
        ));
    }

    #[test]
    fn esc_cancels() {
        let mut p = ModelPickerView::new(rows());
        assert!(matches!(
            p.handle_key(press(KeyCode::Esc)),
            ViewOutcome::Cancelled
        ));
    }

    #[test]
    fn arrows_clamp_at_both_ends() {
        let mut p = ModelPickerView::new(rows());
        p.handle_key(press(KeyCode::Down)); // already last (index 1), clamps
        assert_eq!(p.selected(), 1);
        p.handle_key(press(KeyCode::Up));
        p.handle_key(press(KeyCode::Up)); // clamps at 0
        assert_eq!(p.selected(), 0);
    }

    #[test]
    fn empty_picker_enter_cancels() {
        let mut p = ModelPickerView::new(Vec::new());
        assert!(p.is_empty());
        assert!(matches!(
            p.handle_key(press(KeyCode::Enter)),
            ViewOutcome::Cancelled
        ));
    }

    #[test]
    fn desired_height_is_rows_plus_chrome() {
        assert_eq!(ModelPickerView::new(rows()).desired_height(80), 6);
        let many: Vec<ModelRow> = (0..30)
            .map(|i| ModelRow {
                display: format!("m{i}"),
                request_model: format!("m{i}"),
                profile: None,
                provider_label: String::new(),
                is_current: false,
            })
            .collect();
        // Caps at the 12-row scroll viewport + 4 chrome.
        assert_eq!(ModelPickerView::new(many).desired_height(80), 16);
    }

    #[test]
    fn render_centers_list_with_current_marker_into_buffer() {
        let p = ModelPickerView::new(rows());
        let area = Rect::new(0, 0, 60, 10);
        let mut buf = Buffer::empty(area);
        p.render(area, &mut buf);
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
        assert!(text.contains("Select model"), "{text}");
        assert!(text.contains("Opus (Anthropic)"), "{text}");
        // Sonnet is both current (●) and the starting highlight (›).
        assert!(text.contains("› ● Sonnet (Anthropic)"), "{text}");
        // The picker width is content-driven, so the footer hint clips to it.
        assert!(text.contains("↑/↓ select"), "{text}");
    }
}

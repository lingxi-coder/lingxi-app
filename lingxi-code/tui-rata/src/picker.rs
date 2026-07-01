//! Scrollable model picker for `/model`: a centered, bordered selection list
//! over the session's [`ModelRow`]s.
//!
//! Unlike the read-only [`crate::screens::FullScreen`] it is INTERACTIVE — arrow
//! keys move a highlight (the viewport follows), `Enter` confirms the model,
//! `Esc` cancels. The currently active model is marked with a `●`. Modeled on
//! codex's `list_selection_view` + the modal contract of
//! [`crate::overlay::Dialog`].

use crossterm::event::KeyCode;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::overlay::centered_rect;
use crate::session::ModelRow;

/// Rows shown in the picker viewport before it scrolls.
const VIEWPORT: usize = 12;

/// Result of routing a key into a [`ModelPicker`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerOutcome {
    /// Still open (navigation only).
    Pending,
    /// The user confirmed a model: `(request_model, profile)` — the exact args
    /// `OrchestratorHandle::switch_model` accepts.
    Selected(String, Option<String>),
    /// The user cancelled (`Esc`).
    Cancelled,
}

/// An interactive, scrollable model selection list.
pub struct ModelPicker {
    rows: Vec<ModelRow>,
    selected: usize,
    /// Top row index of the scroll window.
    offset: usize,
}

impl ModelPicker {
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

    /// Route a key: arrows move the highlight (viewport follows), `Enter`
    /// confirms, `Esc` cancels.
    pub fn on_key(&mut self, code: KeyCode) -> PickerOutcome {
        match code {
            KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                self.follow();
                PickerOutcome::Pending
            }
            KeyCode::Down => {
                if self.selected + 1 < self.rows.len() {
                    self.selected += 1;
                }
                self.follow();
                PickerOutcome::Pending
            }
            KeyCode::Enter => self.rows.get(self.selected).map_or(
                PickerOutcome::Cancelled,
                |r| PickerOutcome::Selected(r.request_model.clone(), r.profile.clone()),
            ),
            KeyCode::Esc => PickerOutcome::Cancelled,
            _ => PickerOutcome::Pending,
        }
    }

    /// Keep the highlighted row inside the scroll window.
    fn follow(&mut self) {
        if self.selected < self.offset {
            self.offset = self.selected;
        } else if self.selected >= self.offset + VIEWPORT {
            self.offset = self.selected + 1 - VIEWPORT;
        }
    }

    /// Draw the picker centered over `frame`, clearing the area beneath it.
    pub fn render(&self, frame: &mut ratatui::Frame) {
        let area = frame.area();
        let width = u16::try_from(self.max_width() + 6)
            .unwrap_or(u16::MAX)
            .min(area.width.saturating_sub(4))
            .max(24);
        let visible = self.rows.len().min(VIEWPORT);
        let height = u16::try_from(visible + 4)
            .unwrap_or(u16::MAX)
            .min(area.height);
        let rect = centered_rect(width, height, area);

        frame.render_widget(Clear, rect);
        let block = Block::new().borders(Borders::ALL).title("Select model");
        let inner = block.inner(rect);
        frame.render_widget(block, rect);

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
            lines.push(Line::from(Span::styled(format!("{caret}{marker}{label}"), style)));
        }
        lines.push(Line::from(Span::styled(
            "↑/↓ select · Enter switch · Esc cancel",
            Style::default().add_modifier(Modifier::DIM),
        )));
        frame.render_widget(Paragraph::new(lines), inner);
    }

    fn max_width(&self) -> usize {
        self.rows
            .iter()
            .map(|r| r.display.chars().count() + r.provider_label.chars().count() + 3)
            .max()
            .unwrap_or(20)
    }
}

#[cfg(test)]
mod tests {
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

    #[test]
    fn starts_on_current_model() {
        let p = ModelPicker::new(rows());
        assert_eq!(p.selected(), 1);
    }

    #[test]
    fn enter_confirms_selected_request_model_and_profile() {
        let mut p = ModelPicker::new(rows());
        p.on_key(KeyCode::Up); // move to Opus
        assert_eq!(p.selected(), 0);
        assert_eq!(
            p.on_key(KeyCode::Enter),
            PickerOutcome::Selected("claude-opus".into(), Some("anthropic".into()))
        );
    }

    #[test]
    fn esc_cancels() {
        let mut p = ModelPicker::new(rows());
        assert_eq!(p.on_key(KeyCode::Esc), PickerOutcome::Cancelled);
    }

    #[test]
    fn arrows_clamp_at_both_ends() {
        let mut p = ModelPicker::new(rows());
        p.on_key(KeyCode::Down); // already last (index 1), clamps
        assert_eq!(p.selected(), 1);
        p.on_key(KeyCode::Up);
        p.on_key(KeyCode::Up); // clamps at 0
        assert_eq!(p.selected(), 0);
    }

    #[test]
    fn empty_picker_enter_cancels() {
        let mut p = ModelPicker::new(Vec::new());
        assert!(p.is_empty());
        assert_eq!(p.on_key(KeyCode::Enter), PickerOutcome::Cancelled);
    }
}

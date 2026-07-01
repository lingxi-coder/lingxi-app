//! Reusable modal overlay: a centered, bordered dialog with a title, body
//! lines, and a selectable option list.
//!
//! The foundation for permission prompts and full-page pickers. Modeled on
//! codex's `bottom_pane` popups: a centered box drawn over the content with a
//! `Clear`, arrow-navigated options, `Enter`/`Esc`, and `1`–`9` shortcuts.

use crossterm::event::KeyCode;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

/// A modal dialog: title + body lines + a selectable option list.
pub struct Dialog {
    title: String,
    body: Vec<String>,
    options: Vec<String>,
    selected: usize,
}

/// Result of routing a key into a [`Dialog`].
pub enum DialogOutcome {
    /// Still open (navigation only).
    Pending,
    /// The user confirmed the option at this index.
    Selected(usize),
    /// The user cancelled (`Esc`).
    Cancelled,
}

impl Dialog {
    /// Build a dialog. `selected` starts at the first option.
    #[must_use]
    pub fn new(title: impl Into<String>, body: Vec<String>, options: Vec<String>) -> Self {
        Self {
            title: title.into(),
            body,
            options,
            selected: 0,
        }
    }

    /// The currently highlighted option index.
    #[must_use]
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Route a key: arrows move the highlight, `Enter` confirms, `Esc`
    /// cancels, `1`–`9` jump-select.
    pub fn on_key(&mut self, code: KeyCode) -> DialogOutcome {
        match code {
            KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                DialogOutcome::Pending
            }
            KeyCode::Down => {
                if self.selected + 1 < self.options.len() {
                    self.selected += 1;
                }
                DialogOutcome::Pending
            }
            KeyCode::Enter => DialogOutcome::Selected(self.selected),
            KeyCode::Esc => DialogOutcome::Cancelled,
            KeyCode::Char(c @ '1'..='9') => {
                let idx = (c as usize) - ('1' as usize);
                if idx < self.options.len() {
                    DialogOutcome::Selected(idx)
                } else {
                    DialogOutcome::Pending
                }
            }
            _ => DialogOutcome::Pending,
        }
    }

    /// Draw the dialog centered over `frame`, clearing the area beneath it.
    pub fn render(&self, frame: &mut ratatui::Frame) {
        let area = frame.area();
        let content_w = self
            .body
            .iter()
            .map(|s| s.chars().count())
            .chain(self.options.iter().map(|s| s.chars().count() + 4))
            .chain(std::iter::once(self.title.chars().count()))
            .max()
            .unwrap_or(20);
        let width = u16::try_from(content_w + 4)
            .unwrap_or(u16::MAX)
            .min(area.width.saturating_sub(4))
            .max(20);
        let body_extra = usize::from(!self.body.is_empty());
        let height = u16::try_from(self.body.len() + self.options.len() + body_extra + 4)
            .unwrap_or(u16::MAX)
            .min(area.height);
        let rect = centered_rect(width, height, area);

        frame.render_widget(Clear, rect);
        let block = Block::new()
            .borders(Borders::ALL)
            .title(self.title.clone());
        let inner = block.inner(rect);
        frame.render_widget(block, rect);

        let mut lines: Vec<Line> = self.body.iter().map(|b| Line::from(b.clone())).collect();
        if !self.body.is_empty() {
            lines.push(Line::from(""));
        }
        for (i, opt) in self.options.iter().enumerate() {
            let marker = if i == self.selected { "› " } else { "  " };
            let style = if i == self.selected {
                Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
            } else {
                Style::default()
            };
            lines.push(Line::from(Span::styled(format!("{marker}{opt}"), style)));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "↑/↓ select · Enter confirm · Esc cancel",
            Style::default().add_modifier(Modifier::DIM),
        )));

        frame.render_widget(Paragraph::new(lines), inner);
    }
}

/// A `width`×`height` rectangle centered within `area` (clamped to fit).
#[must_use]
pub fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dialog() -> Dialog {
        Dialog::new(
            "Approve?",
            vec!["Run `ls`?".to_string()],
            vec!["Yes".to_string(), "Always".to_string(), "No".to_string()],
        )
    }

    #[test]
    fn arrows_move_and_clamp() {
        let mut d = dialog();
        assert_eq!(d.selected(), 0);
        d.on_key(KeyCode::Up); // clamps at 0
        assert_eq!(d.selected(), 0);
        d.on_key(KeyCode::Down);
        d.on_key(KeyCode::Down);
        d.on_key(KeyCode::Down); // clamps at last
        assert_eq!(d.selected(), 2);
    }

    #[test]
    fn enter_confirms_highlight() {
        let mut d = dialog();
        d.on_key(KeyCode::Down);
        assert!(matches!(d.on_key(KeyCode::Enter), DialogOutcome::Selected(1)));
    }

    #[test]
    fn number_shortcut_selects() {
        let mut d = dialog();
        assert!(matches!(
            d.on_key(KeyCode::Char('3')),
            DialogOutcome::Selected(2)
        ));
        // Out-of-range number is ignored.
        assert!(matches!(
            d.on_key(KeyCode::Char('9')),
            DialogOutcome::Pending
        ));
    }

    #[test]
    fn esc_cancels() {
        let mut d = dialog();
        assert!(matches!(d.on_key(KeyCode::Esc), DialogOutcome::Cancelled));
    }

    #[test]
    fn centered_rect_is_centered_and_clamped() {
        let area = Rect::new(0, 0, 80, 24);
        let r = centered_rect(40, 10, area);
        assert_eq!(r.width, 40);
        assert_eq!(r.height, 10);
        assert_eq!(r.x, 20);
        assert_eq!(r.y, 7);
        // Oversized request clamps to the area.
        let big = centered_rect(200, 200, area);
        assert_eq!(big.width, 80);
        assert_eq!(big.height, 24);
    }
}

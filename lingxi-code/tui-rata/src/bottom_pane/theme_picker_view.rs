//! Theme picker view for `/theme`: a centered, bordered selection list over
//! the seven [`ThemeSetting`]s (plan Phase 8).
//!
//! Same interaction contract as
//! [`crate::bottom_pane::model_picker_view::ModelPickerView`] — arrow keys
//! move the highlight, `Enter` commits
//! ([`ViewOutcome::RunCommand`]([`CommandAction::SetTheme`])), `Esc` cancels
//! — over the option labels byte-locked to claude-code `ThemePicker.tsx`
//! (the same labels the iocraft `/theme` screen locked). The currently
//! active setting is marked with `●`.
//!
//! DIVERGENCE (deliberate, simpler view boundary): the iocraft picker
//! live-previewed the highlighted theme on every arrow move by mutating app
//! state each frame. A stacked [`BottomPaneView`] only reports outcomes on
//! completion, so this picker applies the theme on `Enter` only.

use std::any::Any;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};
use tui_core::theme::{ThemeName, ThemeSetting};

use crate::bottom_pane::dialog_view::centered_rect;
use crate::bottom_pane::view::{BottomPaneView, CommandAction, ViewOutcome};
use crate::renderable::Renderable;

/// Option labels, byte-locked to claude-code `ThemePicker.tsx`, aligned 1:1
/// with [`OPTIONS`].
pub const OPTION_LABELS: [&str; 7] = [
    "Auto (match terminal)",
    "Dark mode",
    "Light mode",
    "Dark mode (colorblind-friendly)",
    "Light mode (colorblind-friendly)",
    "Dark mode (ANSI colors only)",
    "Light mode (ANSI colors only)",
];

/// Option values in the claude-code `ThemePicker.tsx` display order
/// (dark-before-light within each pair — this deliberately differs from
/// `ThemeSetting::ALL`'s wire order, which lists light-daltonized first).
pub const OPTIONS: [ThemeSetting; 7] = [
    ThemeSetting::Auto,
    ThemeSetting::Named(ThemeName::Dark),
    ThemeSetting::Named(ThemeName::Light),
    ThemeSetting::Named(ThemeName::DarkDaltonized),
    ThemeSetting::Named(ThemeName::LightDaltonized),
    ThemeSetting::Named(ThemeName::DarkAnsi),
    ThemeSetting::Named(ThemeName::LightAnsi),
];

/// An interactive theme selection list over [`OPTIONS`].
pub struct ThemePickerView {
    /// The setting active when the picker opened (the `●` marker).
    current: ThemeSetting,
    /// Highlighted index into [`OPTIONS`] / [`OPTION_LABELS`].
    selected: usize,
}

impl ThemePickerView {
    /// Build a picker with the highlight starting on the active setting.
    #[must_use]
    pub fn new(current: ThemeSetting) -> Self {
        let selected = OPTIONS.iter().position(|s| *s == current).unwrap_or(0);
        Self { current, selected }
    }

    /// The currently highlighted option index (exposed for tests).
    #[must_use]
    pub fn selected(&self) -> usize {
        self.selected
    }

    fn max_label_width() -> usize {
        OPTION_LABELS
            .iter()
            .map(|l| l.chars().count())
            .max()
            .unwrap_or(20)
    }
}

impl Renderable for ThemePickerView {
    /// Draw the picker centered over `area`, clearing the buffer beneath it.
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let width = u16::try_from(Self::max_label_width() + 10)
            .unwrap_or(u16::MAX)
            .min(area.width.saturating_sub(4))
            .max(24);
        let height = u16::try_from(OPTION_LABELS.len() + 4)
            .unwrap_or(u16::MAX)
            .min(area.height);
        let rect = centered_rect(width, height, area);

        Clear.render(rect, buf);
        let block = Block::new().borders(Borders::ALL).title("Theme");
        let inner = block.inner(rect);
        block.render(rect, buf);

        let mut lines: Vec<Line> = Vec::with_capacity(OPTION_LABELS.len() + 1);
        for (idx, (label, setting)) in OPTION_LABELS.iter().zip(OPTIONS).enumerate() {
            let marker = if setting == self.current {
                "● "
            } else {
                "  "
            };
            let caret = if idx == self.selected { "› " } else { "  " };
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
            "↑/↓ select · Enter apply · Esc cancel",
            Style::default().add_modifier(Modifier::DIM),
        )));
        Paragraph::new(lines).render(inner, buf);
    }

    /// The bottom-viewport rows the picker claims: the 7 options + modal
    /// chrome (same rows+4 shape as the model picker).
    fn desired_height(&self, _width: u16) -> u16 {
        u16::try_from(OPTION_LABELS.len()).unwrap_or(7) + 4
    }
}

impl BottomPaneView for ThemePickerView {
    /// Route a key: arrows move the highlight (clamped at the list edges,
    /// matching the model picker's locked behavior), `Enter` commits the
    /// highlighted setting, `Esc` cancels.
    fn handle_key(&mut self, key: KeyEvent) -> ViewOutcome {
        match key.code {
            KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                ViewOutcome::Pending
            }
            KeyCode::Down => {
                if self.selected + 1 < OPTION_LABELS.len() {
                    self.selected += 1;
                }
                ViewOutcome::Pending
            }
            KeyCode::Enter => {
                ViewOutcome::RunCommand(CommandAction::SetTheme(OPTIONS[self.selected]))
            }
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

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn starts_on_the_active_setting() {
        let p = ThemePickerView::new(ThemeSetting::Named(ThemeName::Light));
        assert_eq!(p.selected(), 2, "Light mode is the third option");
        assert_eq!(ThemePickerView::new(ThemeSetting::Auto).selected(), 0);
    }

    #[test]
    fn enter_commits_the_highlighted_setting_as_a_command() {
        let mut p = ThemePickerView::new(ThemeSetting::Auto);
        p.handle_key(press(KeyCode::Down)); // → Dark mode
        assert!(matches!(
            p.handle_key(press(KeyCode::Enter)),
            ViewOutcome::RunCommand(CommandAction::SetTheme(ThemeSetting::Named(
                ThemeName::Dark
            )))
        ));
    }

    #[test]
    fn esc_cancels_and_arrows_clamp_at_both_ends() {
        let mut p = ThemePickerView::new(ThemeSetting::Auto);
        p.handle_key(press(KeyCode::Up)); // clamps at 0
        assert_eq!(p.selected(), 0);
        for _ in 0..20 {
            p.handle_key(press(KeyCode::Down));
        }
        assert_eq!(p.selected(), OPTION_LABELS.len() - 1, "clamps at the end");
        assert!(matches!(
            p.handle_key(press(KeyCode::Esc)),
            ViewOutcome::Cancelled
        ));
    }

    #[test]
    fn desired_height_is_options_plus_chrome() {
        assert_eq!(
            ThemePickerView::new(ThemeSetting::Auto).desired_height(80),
            11
        );
    }

    #[test]
    fn render_lists_all_locked_labels_with_current_marker() {
        let p = ThemePickerView::new(ThemeSetting::Named(ThemeName::DarkAnsi));
        let area = Rect::new(0, 0, 60, 14);
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
        assert!(text.contains("Theme"), "{text}");
        for label in OPTION_LABELS {
            assert!(text.contains(label), "missing {label}:\n{text}");
        }
        // DarkAnsi is both current (●) and the starting highlight (›).
        assert!(text.contains("› ● Dark mode (ANSI colors only)"), "{text}");
        assert!(text.contains("↑/↓ select"), "{text}");
    }
}

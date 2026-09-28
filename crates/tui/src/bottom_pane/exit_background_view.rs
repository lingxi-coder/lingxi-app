//! Exit interstitial for work that belongs to this process (2.1.263 `Sue`).
use crate::bottom_pane::dialog_view::{DialogOutcome, DialogView};
use crate::bottom_pane::view::{BottomPaneView, CommandAction, ViewOutcome};
use crate::renderable::Renderable;
use crossterm::event::KeyEvent;
use ratatui::{buffer::Buffer, layout::Rect};
use std::any::Any;

/// Keep work visible until the user chooses whether to terminate it.
pub struct ExitBackgroundView {
    dialog: DialogView,
    can_background: bool,
}
impl ExitBackgroundView {
    /// Show the optional handoff action only when the host has a durable backend.
    pub fn new(items: Vec<String>, can_background: bool) -> Self {
        let mut body = vec!["The following will stop when you exit:".into()];
        body.extend(items);
        let mut options = vec!["Exit and stop tasks".into()];
        if can_background {
            options.push("Move to background and exit".into());
        }
        options.push("Stay".into());
        Self {
            dialog: DialogView::new("Background work is running", body, options),
            can_background,
        }
    }
}
impl Renderable for ExitBackgroundView {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.dialog.render(area, buf);
    }
    fn desired_height(&self, width: u16) -> u16 {
        self.dialog.desired_height(width)
    }
}
impl BottomPaneView for ExitBackgroundView {
    fn handle_key(&mut self, key: KeyEvent) -> ViewOutcome {
        match self.dialog.on_key(key.code) {
            DialogOutcome::Selected(0) => ViewOutcome::RunCommand(CommandAction::Quit),
            DialogOutcome::Selected(1) if self.can_background => {
                ViewOutcome::RunCommand(CommandAction::BackgroundAndExit)
            }
            DialogOutcome::Selected(_) | DialogOutcome::Cancelled => ViewOutcome::Cancelled,
            DialogOutcome::Pending => ViewOutcome::Pending,
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers};
    #[test]
    fn exit_is_explicit_and_escape_keeps_work_running() {
        let mut view = ExitBackgroundView::new(vec!["build".into()], false);
        assert!(matches!(
            view.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            ViewOutcome::Cancelled
        ));
        let mut view = ExitBackgroundView::new(vec!["build".into()], false);
        assert!(matches!(
            view.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            ViewOutcome::RunCommand(CommandAction::Quit)
        ));
    }
    #[test]
    fn supported_handoff_is_a_distinct_action() {
        let mut view = ExitBackgroundView::new(vec!["build".into()], true);
        view.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert!(matches!(
            view.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            ViewOutcome::RunCommand(CommandAction::BackgroundAndExit)
        ));
    }
}

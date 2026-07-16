//! The `/cd` confirm view: a yes/no dialog over the byte-exact
//! "move this session's working directory" safety prompt (claude-code 2.1.207
//! `name:"cd"`), built on the shared [`crate::bottom_pane::dialog_view::DialogView`]
//! (the same modal foundation [`crate::bottom_pane::permission_view::PermissionView`]
//! uses).
//!
//! On `Yes` it yields [`ViewOutcome::ChangeDirectory`] carrying the
//! already-resolved absolute target; the owner (`ChatWidget::on_pane_outcome`)
//! routes it through the permission-effect channel so the CLI swaps the shared
//! `tool_api::SessionCwd` cell, emits `tengu_cd_command`, and prints the
//! result. `No`/`Esc` cancel (the working directory is untouched). The view
//! completes on either path — it is popped by `ViewStack::apply` (accept
//! catch-all for `ChangeDirectory`, the `Cancelled` arm for a decline).

use std::any::Any;
use std::path::PathBuf;

use command_api::cd::CONFIRM_PROMPT;
use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use crate::bottom_pane::dialog_view::{DialogOutcome, DialogView};
use crate::bottom_pane::view::{BottomPaneView, ViewOutcome};
use crate::renderable::Renderable;

/// A `/cd` confirmation awaiting the user's yes/no decision.
pub struct CdConfirmView {
    dialog: DialogView,
    /// The resolved, absolute target directory to move to on confirm.
    target: PathBuf,
}

impl CdConfirmView {
    /// Build the confirm dialog for the (already validated + absolute) `target`
    /// directory. `display` is the path string shown to the user.
    #[must_use]
    pub fn new(target: PathBuf, display: String) -> Self {
        Self {
            dialog: DialogView::new(
                "Change working directory",
                vec![CONFIRM_PROMPT.to_string(), format!("\u{2192} {display}")],
                vec!["Yes".to_string(), "No".to_string()],
            ),
            target,
        }
    }
}

impl Renderable for CdConfirmView {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.dialog.render(area, buf);
    }

    fn desired_height(&self, width: u16) -> u16 {
        self.dialog.desired_height(width)
    }
}

impl BottomPaneView for CdConfirmView {
    fn handle_key(&mut self, key: KeyEvent) -> ViewOutcome {
        match self.dialog.on_key(key.code) {
            DialogOutcome::Pending => ViewOutcome::Pending,
            // Option 0 = "Yes" → move; any other option ("No") or Esc cancels.
            DialogOutcome::Selected(0) => ViewOutcome::ChangeDirectory(self.target.clone()),
            DialogOutcome::Selected(_) | DialogOutcome::Cancelled => ViewOutcome::Cancelled,
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

    fn view() -> CdConfirmView {
        CdConfirmView::new(PathBuf::from("/tmp/target"), "/tmp/target".to_string())
    }

    #[test]
    fn yes_yields_change_directory_with_the_target() {
        let mut v = view();
        // Default highlight is option 0 ("Yes"); Enter confirms it.
        match v.handle_key(press(KeyCode::Enter)) {
            ViewOutcome::ChangeDirectory(path) => assert_eq!(path, PathBuf::from("/tmp/target")),
            // `ViewOutcome` is not `Debug` (it carries `Box<dyn BottomPaneView>`),
            // so report the failure without formatting the value.
            _ => panic!("expected ViewOutcome::ChangeDirectory"),
        }
    }

    #[test]
    fn number_shortcut_one_confirms() {
        let mut v = view();
        assert!(matches!(
            v.handle_key(press(KeyCode::Char('1'))),
            ViewOutcome::ChangeDirectory(_)
        ));
    }

    #[test]
    fn no_cancels_without_moving() {
        let mut v = view();
        // Move highlight to option 1 ("No") and confirm → Cancelled.
        v.handle_key(press(KeyCode::Down));
        assert!(matches!(
            v.handle_key(press(KeyCode::Enter)),
            ViewOutcome::Cancelled
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
    fn body_shows_the_byte_exact_prompt() {
        let area = Rect::new(0, 0, 100, 12);
        let mut buf = Buffer::empty(area);
        view().render(area, &mut buf);
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
            .join("");
        // The confirm prompt's distinctive tail is present (the full 142-char
        // line wraps across the modal, so assert on a contiguous fragment).
        assert!(
            text.contains("This moves the session"),
            "confirm prompt missing: {text}"
        );
    }

    #[test]
    fn change_directory_outcome_carries_pathbuf() {
        // A ViewOutcome::ChangeDirectory must carry a PathBuf usable downstream.
        let out = ViewOutcome::ChangeDirectory(PathBuf::from("/x"));
        assert!(matches!(out, ViewOutcome::ChangeDirectory(p) if p == PathBuf::from("/x")));
    }
}

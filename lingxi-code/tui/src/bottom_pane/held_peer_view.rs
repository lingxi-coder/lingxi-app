//! Held inbound peer-message dialog (claude-code 2.1.232
//! "Held message from another session").

use std::any::Any;

use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use crate::bottom_pane::dialog_view::{DialogOutcome, DialogView};
use crate::bottom_pane::view::{BottomPaneView, ViewOutcome};
use crate::renderable::Renderable;

/// A parked cross-session message awaiting Deliver / Decline.
pub struct HeldPeerView {
    dialog: DialogView,
    id: String,
}

impl HeldPeerView {
    /// Build the dialog for one held inbox item.
    #[must_use]
    pub fn new(held: traits::uds_inbox::HeldPeer) -> Self {
        let cause = traits::live_sessions::hold_cause_text(&held.hold_cause);
        let from = if held.from.is_empty() {
            "an unidentified session".to_string()
        } else {
            held.from.clone()
        };
        Self {
            dialog: DialogView::new(
                "Held message from another session",
                vec![
                    format!("Another Claude session sent a message: from {from}"),
                    cause.to_string(),
                    String::new(),
                    "Message body (this is what will be delivered):".to_string(),
                    held.preview,
                ],
                vec![
                    "Deliver this message to Claude".to_string(),
                    "Decline".to_string(),
                ],
            ),
            id: held.id,
        }
    }
}

impl Renderable for HeldPeerView {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.dialog.render(area, buf);
    }

    fn desired_height(&self, width: u16) -> u16 {
        self.dialog.desired_height(width)
    }
}

impl BottomPaneView for HeldPeerView {
    fn handle_key(&mut self, key: KeyEvent) -> ViewOutcome {
        match self.dialog.on_key(key.code) {
            DialogOutcome::Pending => ViewOutcome::Pending,
            DialogOutcome::Selected(0) => {
                let _ = traits::uds_inbox::resolve_held(&self.id, true);
                ViewOutcome::RewakePeer
            }
            DialogOutcome::Selected(_) => {
                let _ = traits::uds_inbox::resolve_held(&self.id, false);
                ViewOutcome::Cancelled
            }
            // Esc dismisses without denying; unannounce so it can reappear.
            DialogOutcome::Cancelled => {
                traits::uds_inbox::unannounce_held(&self.id);
                ViewOutcome::Cancelled
            }
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn view() -> HeldPeerView {
        HeldPeerView::new(traits::uds_inbox::HeldPeer {
            id: "m1".into(),
            from: "alpha".into(),
            preview: "hello".into(),
            hold_cause: "explicit-setting".into(),
            announced: true,
        })
    }

    #[test]
    fn deliver_requests_rewake() {
        assert!(matches!(
            view().handle_key(press(KeyCode::Enter)),
            ViewOutcome::RewakePeer
        ));
    }

    #[test]
    fn decline_cancels() {
        let mut v = view();
        v.handle_key(press(KeyCode::Down));
        assert!(matches!(
            v.handle_key(press(KeyCode::Enter)),
            ViewOutcome::Cancelled
        ));
    }

    #[test]
    fn esc_dismisses_without_rewake_or_decline() {
        assert!(matches!(
            view().handle_key(press(KeyCode::Esc)),
            ViewOutcome::Cancelled
        ));
    }
}

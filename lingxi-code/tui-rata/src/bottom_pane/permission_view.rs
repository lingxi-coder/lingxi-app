//! The permission prompt view: a [`DialogView`] over a
//! [`PermissionExchange`], owning the one-shot response channel (plan
//! Phase 4; ported from the former `RataApp::open_permission` +
//! `PendingPermission`).
//!
//! While on the view stack it owns the keyboard: `Enter`/`1`–`3` resolve with
//! the highlighted/numbered option, `Esc` denies. The response sender is
//! consumed by the FIRST resolution (single-shot guarantee — the regression
//! net lives in `app.rs`:`permission_resolution_is_single_shot_and_releases_keyboard`).

use std::any::Any;

use crossterm::cursor::SetCursorStyle;
use crossterm::event::KeyEvent;
use permission::gate::{PermissionRequest, PermissionResponse};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use tokio::sync::oneshot;
use tui_core::permission_bridge::PermissionExchange;

use crate::bottom_pane::dialog_view::{DialogOutcome, DialogView};
use crate::bottom_pane::view::{BottomPaneView, ViewOutcome};
use crate::renderable::Renderable;

/// The bottom-viewport rows the permission prompt claims. Locked layout value
/// (80x24 behavior lock: permission viewport = 9); the dialog clips its
/// footer chrome to it, exactly as the pre-view-stack `viewport_height` did.
const VIEWPORT_HEIGHT: u16 = 9;

/// A permission request awaiting the user's decision.
pub struct PermissionView {
    dialog: DialogView,
    /// One-shot response sender, consumed by the first resolution.
    resp_tx: Option<oneshot::Sender<PermissionResponse>>,
}

impl PermissionView {
    /// Build the prompt for `exchange` (dialog body mirrors the request; the
    /// response channel is taken from the exchange).
    #[must_use]
    pub fn new(exchange: PermissionExchange) -> Self {
        let who = exchange
            .worker
            .as_ref()
            .map_or_else(|| "The assistant".to_string(), |w| format!("@{}", w.name));
        let (tool, mut input) = match &exchange.request {
            PermissionRequest::ToolUseConfirm {
                tool_name,
                tool_input,
                ..
            } => (tool_name.clone(), tool_input.to_string()),
            PermissionRequest::ExitPlanMode { plan } => ("ExitPlanMode".to_string(), plan.clone()),
            PermissionRequest::BypassPermissionsMode => {
                ("BypassPermissionsMode".to_string(), String::new())
            }
        };
        if input.chars().count() > 68 {
            input = format!("{}…", input.chars().take(67).collect::<String>());
        }
        let dialog = DialogView::new(
            "Permission required",
            vec![format!("{who} wants to use {tool}:"), input],
            vec![
                "Yes, allow once".to_string(),
                "Yes, allow always".to_string(),
                "No, deny".to_string(),
            ],
        );
        Self {
            dialog,
            resp_tx: Some(exchange.resp_tx),
        }
    }

    /// Deliver `response` through the one-shot channel (first resolution only)
    /// and report it as the view's outcome.
    fn resolve(&mut self, response: PermissionResponse) -> ViewOutcome {
        if let Some(tx) = self.resp_tx.take() {
            let _ = tx.send(response);
        }
        ViewOutcome::PermissionResponse(response)
    }
}

impl Renderable for PermissionView {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.dialog.render(area, buf);
    }

    fn desired_height(&self, _width: u16) -> u16 {
        VIEWPORT_HEIGHT
    }

    fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
        self.dialog.cursor_pos(area)
    }

    fn cursor_style(&self, area: Rect) -> SetCursorStyle {
        self.dialog.cursor_style(area)
    }
}

impl BottomPaneView for PermissionView {
    fn handle_key(&mut self, key: KeyEvent) -> ViewOutcome {
        match self.dialog.on_key(key.code) {
            DialogOutcome::Pending => ViewOutcome::Pending,
            DialogOutcome::Selected(idx) => {
                let response = match idx {
                    0 => PermissionResponse::AllowOnce,
                    1 => PermissionResponse::AllowAlways,
                    _ => PermissionResponse::Deny,
                };
                self.resolve(response)
            }
            // Esc denies (a resolution, not a silent dismissal): the waiting
            // tool call must always receive an answer.
            DialogOutcome::Cancelled => self.resolve(PermissionResponse::Deny),
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

    fn tool_exchange() -> (PermissionExchange, oneshot::Receiver<PermissionResponse>) {
        let (resp_tx, resp_rx) = oneshot::channel();
        let request = PermissionRequest::ToolUseConfirm {
            tool_name: "Bash".to_string(),
            tool_input: serde_json::json!({ "command": "ls -la" }),
            default_decision: permission::gate::PromptDefault::DenyByDefault,
        };
        (
            PermissionExchange {
                request,
                resp_tx,
                worker: None,
            },
            resp_rx,
        )
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn enter_sends_allow_once_and_completes() {
        let (exchange, resp_rx) = tool_exchange();
        let mut view = PermissionView::new(exchange);
        assert!(matches!(
            view.handle_key(press(KeyCode::Char('x'))),
            ViewOutcome::Pending
        ));
        assert!(matches!(
            view.handle_key(press(KeyCode::Enter)),
            ViewOutcome::PermissionResponse(PermissionResponse::AllowOnce)
        ));
        assert_eq!(
            resp_rx.blocking_recv().unwrap(),
            PermissionResponse::AllowOnce
        );
    }

    #[test]
    fn esc_denies() {
        let (exchange, resp_rx) = tool_exchange();
        let mut view = PermissionView::new(exchange);
        assert!(matches!(
            view.handle_key(press(KeyCode::Esc)),
            ViewOutcome::PermissionResponse(PermissionResponse::Deny)
        ));
        assert_eq!(resp_rx.blocking_recv().unwrap(), PermissionResponse::Deny);
    }

    #[test]
    fn response_sender_is_consumed_exactly_once() {
        let (exchange, resp_rx) = tool_exchange();
        let mut view = PermissionView::new(exchange);
        view.handle_key(press(KeyCode::Char('2')));
        assert_eq!(
            resp_rx.blocking_recv().unwrap(),
            PermissionResponse::AllowAlways
        );
        // A second resolution finds the sender gone and cannot re-send (the
        // stack pops the view on the first resolution; this guards the
        // consume-once property even if it did not).
        assert!(matches!(
            view.handle_key(press(KeyCode::Enter)),
            ViewOutcome::PermissionResponse(_)
        ));
        assert!(view.resp_tx.is_none());
    }

    #[test]
    fn desired_height_is_the_locked_permission_viewport() {
        let (exchange, _resp_rx) = tool_exchange();
        let view = PermissionView::new(exchange);
        assert_eq!(view.desired_height(80), 9);
        assert!(view.wants_status_line());
    }
}

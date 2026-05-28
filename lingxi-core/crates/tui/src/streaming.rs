#![forbid(unsafe_code)]
//! Streaming subscriber: applies `TurnEvent`s to `AppState`. (M6-03)
//!
//! Pure mutator. No side effects beyond `state` mutation and
//! `notify.notify_one()`. The render loop calls this on every event
//! received from the bridge channel.

use crate::events::orchestrator_bridge::TurnEvent;
use crate::state::{AppState, PendingPermission, RenderedMessage, StreamingState};
use tokio::sync::Notify;

/// Apply one `TurnEvent` to `state` and signal the renderer.
///
/// Behavior contract:
/// - `TurnStarted` → `state.streaming = Some(StreamingState::new())`.
/// - `TextDelta(s)` → if the last message in `state.messages` is an
///   `AssistantText`, append `s` to its `body`. Otherwise push a NEW
///   `AssistantText` message with body `s`.
/// - `ToolUseStart{..}` / `ToolUseResult{..}` → push placeholder
///   `SystemText` rows (proper rendering lands in M6-04).
/// - `PermissionRequest{..}` → set `state.pending_permission` (M6-05).
/// - `TurnEnded(_)` → clear `state.streaming` and `state.cancel_token`.
///
/// After mutation, calls `notify.notify_one()`. The render loop is
/// expected to debounce these to ~30fps.
pub fn apply_event(state: &mut AppState, ev: TurnEvent, notify: &Notify) {
    match ev {
        TurnEvent::TurnStarted => {
            state.streaming = Some(StreamingState::new());
        }
        TurnEvent::TextDelta(text) => {
            // Append to last AssistantText if present; otherwise push new.
            if let Some(RenderedMessage::AssistantText { body, .. }) = state.messages.last_mut() {
                body.push_str(&text);
            } else {
                state.messages.push(RenderedMessage::AssistantText {
                    body: text,
                    timestamp: chrono::Utc::now().timestamp(),
                });
            }
        }
        TurnEvent::ToolUseStart { id, tool, input } => {
            // M6-04: rich tool-use block. Per-id expanded state lives in
            // `state.expanded` (default false → collapsed header).
            state.messages.push(RenderedMessage::AssistantToolUse {
                id,
                tool,
                input,
            });
        }
        TurnEvent::ToolUseResult { id, tool, result } => {
            state.messages.push(RenderedMessage::UserToolResult {
                id,
                tool,
                result,
            });
        }
        TurnEvent::PermissionRequest { tool, input } => {
            state.pending_permission = Some(PendingPermission { tool, input });
        }
        TurnEvent::TurnEnded(_outcome) => {
            state.streaming = None;
            state.cancel_token = None;
        }
    }
    notify.notify_one();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{AppState, StatusSnapshot};

    fn new_state() -> AppState {
        AppState::new(StatusSnapshot::default())
    }

    #[test]
    fn text_delta_creates_new_assistant_message_when_buffer_empty() {
        let mut s = new_state();
        let n = Notify::new();
        apply_event(&mut s, TurnEvent::TextDelta("hello".into()), &n);
        assert_eq!(s.messages.len(), 1);
        assert!(matches!(
            s.messages.last(),
            Some(RenderedMessage::AssistantText { body, .. }) if body == "hello"
        ));
    }

    #[test]
    fn five_deltas_concatenate_into_single_message() {
        let mut s = new_state();
        let n = Notify::new();
        for chunk in ["he", "ll", "o ", "wo", "rld"] {
            apply_event(&mut s, TurnEvent::TextDelta(chunk.into()), &n);
        }
        assert_eq!(s.messages.len(), 1);
        assert!(matches!(
            s.messages.last(),
            Some(RenderedMessage::AssistantText { body, .. }) if body == "hello world"
        ));
    }

    #[test]
    fn turn_started_sets_streaming_some() {
        let mut s = new_state();
        let n = Notify::new();
        assert!(s.streaming.is_none());
        apply_event(&mut s, TurnEvent::TurnStarted, &n);
        assert!(s.streaming.is_some());
    }

    #[test]
    fn turn_ended_clears_streaming_and_cancel_token() {
        let mut s = new_state();
        let n = Notify::new();
        s.streaming = Some(StreamingState::new());
        s.cancel_token = Some(tokio_util::sync::CancellationToken::new());
        apply_event(
            &mut s,
            TurnEvent::TurnEnded(lingxi_traits::TurnOutcome::EndTurn),
            &n,
        );
        assert!(s.streaming.is_none());
        assert!(s.cancel_token.is_none());
    }

    #[tokio::test]
    async fn apply_event_calls_notify_one() {
        let mut s = new_state();
        let n = Notify::new();
        // Pre-record a permit so we can detect notify_one.
        let waiter = n.notified();
        tokio::pin!(waiter);
        apply_event(&mut s, TurnEvent::TextDelta("x".into()), &n);
        let poll = futures::poll!(waiter.as_mut());
        assert!(matches!(poll, std::task::Poll::Ready(())));
    }

    #[test]
    fn permission_request_populates_pending_slot() {
        let mut s = new_state();
        let n = Notify::new();
        apply_event(
            &mut s,
            TurnEvent::PermissionRequest {
                tool: "Bash".into(),
                input: serde_json::json!({"command": "ls"}),
            },
            &n,
        );
        assert!(s.pending_permission.is_some());
        assert_eq!(s.pending_permission.as_ref().unwrap().tool, "Bash");
    }
}

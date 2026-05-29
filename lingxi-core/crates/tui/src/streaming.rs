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
            //
            // M7-02 (T13-wire): the tool-call INPUT carries the diff source
            // (Edit's `old_string`/`new_string`/`file_path`, Write's `content`).
            // It must reach the LATER `UserToolResult` render site. We stash it
            // by id here — chosen over a backward scan of `messages` so it stays
            // correct once M7-03 windows the visible message slice.
            state.tool_call_inputs.insert(id, input.clone());
            state
                .messages
                .push(RenderedMessage::AssistantToolUse { id, tool, input });
        }
        TurnEvent::ToolUseResult { id, tool, result } => {
            let (old_string, new_string, file_path) = state
                .tool_call_inputs
                .remove(&id)
                .map_or((None, None, None), |input| diff_inputs_for(&tool, &input));
            state.messages.push(RenderedMessage::UserToolResult {
                id,
                tool,
                result,
                old_string,
                new_string,
                file_path,
            });
        }
        TurnEvent::PermissionRequest { tool, input } => {
            // M6-03 bridge variant still carries the legacy {tool, input}
            // shape — translate to the M6-05 enum's `ToolUseConfirm`
            // arm. The richer bridge variant that ships the full
            // `PermissionRequest` enum lives on the dedicated
            // `mpsc<PermissionExchange>` channel owned by
            // `TuiPermissionGate` (see `permission_bridge.rs`).
            let default_decision = lingxi_permission::tool_default(&tool);
            state.pending_permission = Some(PendingPermission {
                request: lingxi_permission::gate::PermissionRequest::ToolUseConfirm {
                    tool_name: tool,
                    tool_input: input,
                    default_decision,
                },
            });
            state.pending_permission_started_at = Some(std::time::Instant::now());
            state.tool_use_dialog_state =
                crate::components::permissions::tool_use_confirm::ToolUseConfirmState::default();
            crate::telemetry::permission_dialog_shown("tool_use");
        }
        TurnEvent::TurnEnded(_outcome) => {
            state.streaming = None;
            state.cancel_token = None;
        }
        TurnEvent::CostUpdated(cost_str) => {
            // M6-06: update the StatusSnapshot cost so the next render
            // pass shows the post-turn dollar amount in the status line.
            state.status.cost = cost_str;
        }
        TurnEvent::CompactionCompleted {
            messages_before,
            messages_after,
            ..
        } => {
            // M6-08: append a `[Compacted N → M messages]` SystemText
            // line so the user sees the boundary marker. Proper
            // CompactBoundaryMessage rendering with a summary preview
            // lands in M7.
            state.messages.push(RenderedMessage::SystemText {
                body: format!("[Compacted {messages_before} → {messages_after} messages]"),
                timestamp: chrono::Utc::now().timestamp(),
                is_error: false,
            });
        }
    }
    notify.notify_one();
}

/// Extract the `(old_string, new_string, file_path)` diff inputs for a diff
/// tool from its call `input` JSON. Returns all-`None` for non-diff tools.
///
/// Mapping (claude-code parity):
///   - `Edit`  → `old_string` / `new_string` / `file_path` keys verbatim.
///   - `Write` → `old = None` (pure add), `new = content`, `file_path`.
///   - `MultiEdit` / `NotebookEdit` → only `file_path` populated; the old/new
///     bodies are multi-hunk (`edits[]`) / cell-shaped and don't map to a
///     single old→new pair. TODO(M8): render their full multi-hunk diff.
fn diff_inputs_for(
    tool: &str,
    input: &serde_json::Value,
) -> (Option<String>, Option<String>, Option<String>) {
    let str_key = |k: &str| {
        input
            .get(k)
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    };
    match tool {
        "Edit" => (
            str_key("old_string"),
            str_key("new_string"),
            str_key("file_path"),
        ),
        "Write" => (None, str_key("content"), str_key("file_path")),
        // TODO(M8): MultiEdit (`edits[]`) and NotebookEdit (cell-shaped) carry
        // no single old→new pair — surface only the path for now so the header
        // renders without a (wrong) single-hunk diff.
        "MultiEdit" | "NotebookEdit" => (None, None, str_key("file_path")),
        _ => (None, None, None),
    }
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
        assert_eq!(s.pending_permission.as_ref().unwrap().tool(), "Bash");
    }
}

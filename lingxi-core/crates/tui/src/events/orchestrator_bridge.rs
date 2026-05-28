//! Orchestrator → TUI event bridge. (M6-03)
//!
//! The `BridgeOutputStream` is an [`lingxi_traits::OutputStream`] impl that
//! forwards every orchestrator callback as a [`TurnEvent`] on an mpsc
//! channel. The TUI render loop drains the receiver and feeds events into
//! `crate::streaming::apply_event`, which mutates `AppState` and pokes a
//! `tokio::sync::Notify`.
//!
//! The lifecycle:
//! 1. TUI app creates an `mpsc::unbounded_channel::<TurnEvent>()`.
//! 2. TUI wraps the sender in `BridgeOutputStream` and passes it to the
//!    `ConversationOrchestrator` as its `output: Arc<dyn OutputStream>`.
//! 3. TUI keeps the receiver and the sender (so the channel stays open
//!    across multiple turns).
//! 4. For each user submit: TUI emits `TurnEvent::TurnStarted` directly,
//!    spawns `run_turn_streaming_with_cancel`, then awaits delta events.
//! 5. On `emit_end_turn` the bridge fires `TurnEvent::TurnEnded(_)`.

use async_trait::async_trait;
use lingxi_traits::{CostSnapshot, OutputStream, TurnOutcome};
use tokio::sync::mpsc::UnboundedSender;

/// Events flowing from the orchestrator into the TUI render loop.
///
/// Created in M6-03 as a TUI-local enum (not exposed on any orchestrator
/// trait). The bridge translates `OutputStream` callbacks into this enum.
/// Future expansion: `PermissionRequest` is wired in M6-05; `ThinkingDelta`
/// in M7.
#[derive(Debug, Clone)]
pub enum TurnEvent {
    /// Streaming text chunk from the assistant.
    TextDelta(String),
    /// A tool invocation is about to dispatch. `id` is a UUID v4 the
    /// bridge synthesizes so the TUI can correlate Start/Result.
    ToolUseStart {
        /// Stable id (UUID v4) so the TUI can correlate Start/Result.
        id: String,
        /// Name of the tool being invoked.
        tool: String,
        /// JSON input passed to the tool.
        input: serde_json::Value,
    },
    /// A tool result has returned.
    ToolUseResult {
        /// Correlator (poor-man's: tool name in M6-03; real id in M6-04).
        id: String,
        /// JSON result payload.
        result: serde_json::Value,
    },
    /// Permission gate fired. Wired in M6-05; the variant is reserved
    /// here so the enum stays append-only.
    PermissionRequest {
        /// Name of the tool the permission gate is checking.
        tool: String,
        /// JSON input the permission gate is being asked to approve.
        input: serde_json::Value,
    },
    /// Fired SYNCHRONOUSLY before the orchestrator future is awaited so
    /// the UI shows the spinner immediately on Enter.
    TurnStarted,
    /// Fired when the orchestrator returns. Carries the [`TurnOutcome`].
    TurnEnded(TurnOutcome),
}

/// `OutputStream` impl that forwards every callback as a `TurnEvent` on
/// an mpsc channel. Cloneable via `tx.clone()` if multiple producers are
/// ever needed (currently one bridge per session — the channel lives for
/// the whole TUI lifetime).
pub struct BridgeOutputStream {
    tx: UnboundedSender<TurnEvent>,
}

impl BridgeOutputStream {
    /// Wrap a sender. The receiver lives on the TUI side and is drained
    /// by the render loop.
    #[must_use]
    pub fn new(tx: UnboundedSender<TurnEvent>) -> Self {
        Self { tx }
    }
}

#[async_trait]
impl OutputStream for BridgeOutputStream {
    async fn emit_text(&self, text: &str) {
        let _ = self.tx.send(TurnEvent::TextDelta(text.to_string()));
    }

    async fn emit_tool_call(&self, tool: &str, input: &serde_json::Value) {
        let _ = self.tx.send(TurnEvent::ToolUseStart {
            id: uuid::Uuid::new_v4().to_string(),
            tool: tool.to_string(),
            input: input.clone(),
        });
    }

    async fn emit_tool_result(&self, tool: &str, result: &serde_json::Value) {
        // M6-03 uses tool name as a poor-man's correlator. M6-04 will
        // thread a real id through ToolUseStart → ToolUseResult.
        let _ = self.tx.send(TurnEvent::ToolUseResult {
            id: tool.to_string(),
            result: result.clone(),
        });
    }

    async fn emit_end_turn(&self, stop_reason: &str, _cost: &CostSnapshot) {
        // Map stop_reason → TurnOutcome. Mirrors the M5-13 stdio REPL
        // mapping. Unknown/unrecognised reasons fall back to EndTurn.
        let outcome = match stop_reason {
            "max_tokens" => TurnOutcome::MaxTurns,
            _ => TurnOutcome::EndTurn,
        };
        let _ = self.tx.send(TurnEvent::TurnEnded(outcome));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn emit_text_translates_to_text_delta() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let bridge = BridgeOutputStream::new(tx);
        bridge.emit_text("hello").await;
        let ev = rx.recv().await.unwrap();
        assert!(matches!(ev, TurnEvent::TextDelta(ref s) if s == "hello"));
    }

    #[tokio::test]
    async fn emit_tool_call_translates_to_tool_use_start() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let bridge = BridgeOutputStream::new(tx);
        bridge
            .emit_tool_call("Read", &serde_json::json!({"file_path": "/tmp/x"}))
            .await;
        match rx.recv().await.unwrap() {
            TurnEvent::ToolUseStart { tool, input, .. } => {
                assert_eq!(tool, "Read");
                assert_eq!(input["file_path"], "/tmp/x");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[tokio::test]
    async fn emit_end_turn_endturn_reason_translates_to_endturn() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let bridge = BridgeOutputStream::new(tx);
        let cost = lingxi_traits::CostSnapshot::default();
        bridge.emit_end_turn("end_turn", &cost).await;
        assert!(matches!(
            rx.recv().await.unwrap(),
            TurnEvent::TurnEnded(TurnOutcome::EndTurn)
        ));
    }

    #[tokio::test]
    async fn emit_end_turn_max_tokens_translates_to_maxturns() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let bridge = BridgeOutputStream::new(tx);
        let cost = lingxi_traits::CostSnapshot::default();
        bridge.emit_end_turn("max_tokens", &cost).await;
        assert!(matches!(
            rx.recv().await.unwrap(),
            TurnEvent::TurnEnded(TurnOutcome::MaxTurns)
        ));
    }

    #[tokio::test]
    async fn turn_started_is_caller_emitted_not_bridge() {
        // TurnStarted is fired by the SPAWNER (app.rs), not the bridge.
        // Documenting that contract: the bridge has no method that
        // produces TurnStarted; callers send it manually.
        let (tx, mut rx) = mpsc::unbounded_channel();
        tx.send(TurnEvent::TurnStarted).unwrap();
        assert!(matches!(rx.recv().await.unwrap(), TurnEvent::TurnStarted));
    }
}

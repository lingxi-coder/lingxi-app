//! `AdapterOutputStream` — the live-turn feed (plan F1-12).
//!
//! This is the direct analog of the TUI's `BridgeOutputStream`
//! (`tui/src/events/orchestrator_bridge.rs`), but instead of pushing a TUI-local
//! `TurnEvent` onto an mpsc channel it lowers each callback into a
//! `client_protocol::ClientEvent` DTO and forwards it through the
//! transport-agnostic [`ClientEventSink`]. The SAME stream therefore feeds both
//! transports (bridge-server WS and mobile `UniFFI`) — governing decision §0.1.
//!
//! It implements the five [`traits::OutputStream`] callbacks
//! (`traits/src/orchestrator.rs:423-456`):
//!
//! | callback                    | emitted `ClientEvent`(s)            |
//! |-----------------------------|-------------------------------------|
//! | `emit_text`                 | `TextDelta`                         |
//! | `emit_tool_call`            | `ToolUseStarted`                    |
//! | `emit_tool_result`          | `ToolUseResult`                     |
//! | `emit_end_turn`             | `CostUpdate` **then** `TurnEnded`   |
//! | `emit_compaction_completed` | `CompactionCompleted`               |
//!
//! All `serde_json::Value` lowering goes through the pure F1-11 fns in
//! [`crate::lowering`] so the wire form is identical to every other surface and
//! `client-protocol` itself never sees a `Value`.
//!
//! ## `is_error` derivation
//!
//! [`traits::OutputStream::emit_tool_result`] carries ONLY `(id, tool, &Value)`
//! — it has NO separate `is_error` flag (verified `traits/src/orchestrator.rs:436`).
//! The orchestrator signals a failed tool by shaping the emitted payload as
//! `{ "error": "<message>" }` (verified `orchestrator/src/turn_loop.rs:319-333`).
//! The adapter therefore derives `is_error` structurally: a JSON object carrying
//! a top-level `"error"` key is an error result. This mirrors the engine's own
//! error-payload contract rather than inventing a new signal.
//!
//! ## `stop_reason` → `TurnOutcomeDto`
//!
//! `emit_end_turn` cannot observe cancellation (the cancel token is handled by
//! the F1-13 turn wrapper), so it maps only the model's stop reason — mirroring
//! `BridgeOutputStream` (`tui/src/events/orchestrator_bridge.rs:157-160`):
//! `"max_tokens"` ⇒ [`TurnOutcomeDto::MaxTurns`], anything else ⇒
//! [`TurnOutcomeDto::EndTurn`]. The raw `stop_reason` is preserved verbatim in
//! `TurnEnded.stop_reason` for clients that need the exact string.

use std::sync::Arc;

use async_trait::async_trait;
use client_protocol::events::{ClientEvent, TurnOutcomeDto};
use traits::{CostSnapshot, OutputStream};

use crate::lowering::{lower_cost_snapshot, value_to_json_string};
use crate::sink::ClientEventSink;

/// An [`traits::OutputStream`] that lowers every live-turn callback into a
/// [`ClientEvent`] DTO and forwards it through an [`Arc<dyn ClientEventSink>`].
///
/// Connection-scoped: one stream per transport connection, holding the same
/// `Arc<dyn ClientEventSink>` as the permission gate and turn wrapper so all
/// three feed the one outbound channel (mirrors the single `BridgeOutputStream`
/// per TUI session).
pub struct AdapterOutputStream {
    sink: Arc<dyn ClientEventSink>,
}

impl AdapterOutputStream {
    /// Wrap a sink. The sink is shared with the rest of the connection-scoped
    /// adapter (permission gate, turn wrapper).
    #[must_use]
    pub fn new(sink: Arc<dyn ClientEventSink>) -> Self {
        Self { sink }
    }

    /// Derive the `is_error` flag from a tool-result payload.
    ///
    /// The orchestrator emits a failed tool result as a JSON object with a
    /// top-level `"error"` key (`orchestrator/src/turn_loop.rs:327`); any other
    /// shape is a success payload. This keeps the adapter aligned with the
    /// engine's existing error-payload contract.
    fn result_is_error(result: &serde_json::Value) -> bool {
        result.get("error").is_some()
    }

    /// Map a model `stop_reason` to a [`TurnOutcomeDto`].
    ///
    /// Mirrors `BridgeOutputStream` (`orchestrator_bridge.rs:157-160`):
    /// `"max_tokens"` ⇒ `MaxTurns`, everything else ⇒ `EndTurn`. Cancellation
    /// is NOT observable here — it is surfaced by the F1-13 turn wrapper.
    fn outcome_for(stop_reason: &str) -> TurnOutcomeDto {
        match stop_reason {
            "max_tokens" => TurnOutcomeDto::MaxTurns,
            _ => TurnOutcomeDto::EndTurn,
        }
    }
}

#[async_trait]
impl OutputStream for AdapterOutputStream {
    async fn emit_text(&self, text: &str) {
        self.sink
            .emit(ClientEvent::TextDelta {
                text: text.to_string(),
            })
            .await;
    }

    async fn emit_tool_call(
        &self,
        id: &protocol::ToolUseId,
        tool: &str,
        input: &serde_json::Value,
    ) {
        self.sink
            .emit(ClientEvent::ToolUseStarted {
                id: id.to_string(),
                tool: tool.to_string(),
                input_json: value_to_json_string(input),
            })
            .await;
    }

    async fn emit_tool_result(
        &self,
        id: &protocol::ToolUseId,
        tool: &str,
        result: &serde_json::Value,
    ) {
        self.sink
            .emit(ClientEvent::ToolUseResult {
                id: id.to_string(),
                tool: tool.to_string(),
                result_json: value_to_json_string(result),
                is_error: Self::result_is_error(result),
            })
            .await;
    }

    async fn emit_end_turn(&self, stop_reason: &str, cost: &CostSnapshot) {
        // Emit the cumulative cost update BEFORE the turn-end marker so a client
        // can refresh its cost line in the same render pass it ends the turn —
        // exactly the ordering `BridgeOutputStream::emit_end_turn` uses
        // (`orchestrator_bridge.rs:148-162`).
        let cost_dto = lower_cost_snapshot(cost);
        self.sink
            .emit(ClientEvent::CostUpdate {
                total_usd: cost_dto.total_usd,
                input_tokens: cost_dto.input_tokens,
                output_tokens: cost_dto.output_tokens,
                api_calls: cost_dto.api_calls,
                session_duration_secs: cost_dto.session_duration_secs,
                formatted: cost_dto.formatted.clone(),
            })
            .await;

        self.sink
            .emit(ClientEvent::TurnEnded {
                outcome: Self::outcome_for(stop_reason),
                stop_reason: Some(stop_reason.to_string()),
                cost: cost_dto,
            })
            .await;
    }

    async fn emit_compaction_completed(
        &self,
        messages_before: u32,
        messages_after: u32,
        bytes_saved: u64,
    ) {
        self.sink
            .emit(ClientEvent::CompactionCompleted {
                messages_before,
                messages_after,
                bytes_saved,
            })
            .await;
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::test_support::MockSink;

    /// `emit_text` → exactly one `TextDelta` carrying the payload verbatim.
    #[tokio::test]
    async fn emit_text_produces_text_delta() {
        let sink = MockSink::arc();
        let stream = AdapterOutputStream::new(sink.clone());

        stream.emit_text("hello world").await;

        let events = sink.events().await;
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            ClientEvent::TextDelta {
                text: "hello world".to_string()
            }
        );
    }

    /// `emit_tool_call` → one `ToolUseStarted`; the `Value` input is lowered to
    /// the `input_json` JSON String (F1-11) and the id to its `tu:` string form.
    #[tokio::test]
    async fn emit_tool_call_produces_tool_use_started() {
        let sink = MockSink::arc();
        let stream = AdapterOutputStream::new(sink.clone());

        let id = protocol::ToolUseId::new();
        let input = serde_json::json!({"file_path": "/tmp/x"});
        stream.emit_tool_call(&id, "Read", &input).await;

        let events = sink.events().await;
        assert_eq!(events.len(), 1);
        match &events[0] {
            ClientEvent::ToolUseStarted {
                id: gid,
                tool,
                input_json,
            } => {
                assert_eq!(*gid, id.to_string());
                assert_eq!(tool, "Read");
                // The lowered string round-trips back to the original Value.
                let back: serde_json::Value = serde_json::from_str(input_json).unwrap();
                assert_eq!(back, input);
            }
            other => panic!("expected ToolUseStarted, got {other:?}"),
        }
    }

    /// A success tool result (no top-level `"error"` key) lowers to
    /// `ToolUseResult { is_error: false }`.
    #[tokio::test]
    async fn emit_tool_result_success_is_not_error() {
        let sink = MockSink::arc();
        let stream = AdapterOutputStream::new(sink.clone());

        let id = protocol::ToolUseId::new();
        let result = serde_json::json!({"content": "ok", "lines": 3});
        stream.emit_tool_result(&id, "Read", &result).await;

        let events = sink.events().await;
        assert_eq!(events.len(), 1);
        match &events[0] {
            ClientEvent::ToolUseResult {
                id: gid,
                tool,
                result_json,
                is_error,
            } => {
                assert_eq!(*gid, id.to_string());
                assert_eq!(tool, "Read");
                let back: serde_json::Value = serde_json::from_str(result_json).unwrap();
                assert_eq!(back, result);
                assert!(!is_error);
            }
            other => panic!("expected ToolUseResult, got {other:?}"),
        }
    }

    /// A failed tool result — the orchestrator's `{ "error": "<msg>" }` payload
    /// shape (`turn_loop.rs:327`) — lowers to `ToolUseResult { is_error: true }`.
    #[tokio::test]
    async fn emit_tool_result_error_payload_sets_is_error() {
        let sink = MockSink::arc();
        let stream = AdapterOutputStream::new(sink.clone());

        let id = protocol::ToolUseId::new();
        let result = serde_json::json!({"error": "file not found"});
        stream.emit_tool_result(&id, "Read", &result).await;

        let events = sink.events().await;
        assert_eq!(events.len(), 1);
        match &events[0] {
            ClientEvent::ToolUseResult { is_error, .. } => assert!(*is_error),
            other => panic!("expected ToolUseResult, got {other:?}"),
        }
    }

    /// `emit_end_turn` produces BOTH a `CostUpdate` and a `TurnEnded`, in that
    /// order (the named F1-12 assertion). The cost lowers via `lower_cost_snapshot`.
    #[tokio::test]
    async fn emit_end_turn_produces_cost_then_turn_ended() {
        let sink = MockSink::arc();
        let stream = AdapterOutputStream::new(sink.clone());

        let cost = CostSnapshot {
            total_usd: 0.0123,
            input_tokens: 100,
            output_tokens: 50,
            api_calls: 3,
            session_duration: Duration::from_secs(125),
            ..Default::default()
        };
        stream.emit_end_turn("end_turn", &cost).await;

        let events = sink.events().await;
        assert_eq!(events.len(), 2, "expected CostUpdate then TurnEnded");

        // First: the cumulative cost update.
        match &events[0] {
            ClientEvent::CostUpdate {
                total_usd,
                input_tokens,
                output_tokens,
                api_calls,
                session_duration_secs,
                formatted,
            } => {
                #[allow(clippy::float_cmp)]
                {
                    assert_eq!(*total_usd, 0.0123);
                }
                assert_eq!(*input_tokens, 100);
                assert_eq!(*output_tokens, 50);
                assert_eq!(*api_calls, 3);
                assert_eq!(*session_duration_secs, 125);
                assert_eq!(formatted, "$0.0123");
            }
            other => panic!("expected CostUpdate first, got {other:?}"),
        }

        // Second: the turn-end marker, carrying the same lowered cost.
        match &events[1] {
            ClientEvent::TurnEnded {
                outcome,
                stop_reason,
                cost: cost_dto,
            } => {
                assert_eq!(*outcome, TurnOutcomeDto::EndTurn);
                assert_eq!(stop_reason.as_deref(), Some("end_turn"));
                assert_eq!(cost_dto.session_duration_secs, 125);
                assert_eq!(cost_dto.formatted, "$0.0123");
            }
            other => panic!("expected TurnEnded second, got {other:?}"),
        }
    }

    /// `"max_tokens"` stop reason ends the turn with the `MaxTurns` outcome,
    /// while the raw reason is preserved on `TurnEnded.stop_reason`.
    #[tokio::test]
    async fn emit_end_turn_max_tokens_maps_to_max_turns_outcome() {
        let sink = MockSink::arc();
        let stream = AdapterOutputStream::new(sink.clone());

        stream
            .emit_end_turn("max_tokens", &CostSnapshot::default())
            .await;

        let events = sink.events().await;
        assert_eq!(events.len(), 2);
        match &events[1] {
            ClientEvent::TurnEnded {
                outcome,
                stop_reason,
                ..
            } => {
                assert_eq!(*outcome, TurnOutcomeDto::MaxTurns);
                assert_eq!(stop_reason.as_deref(), Some("max_tokens"));
            }
            other => panic!("expected TurnEnded, got {other:?}"),
        }
    }

    /// `emit_compaction_completed` → one `CompactionCompleted` carrying the
    /// three counters verbatim.
    #[tokio::test]
    async fn emit_compaction_completed_produces_compaction_completed() {
        let sink = MockSink::arc();
        let stream = AdapterOutputStream::new(sink.clone());

        stream.emit_compaction_completed(42, 8, 1_024).await;

        let events = sink.events().await;
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            ClientEvent::CompactionCompleted {
                messages_before: 42,
                messages_after: 8,
                bytes_saved: 1_024,
            }
        );
    }

    /// The default-trait `emit_compaction_completed` is overridden — exercising
    /// it through an `Arc<dyn OutputStream>` proves the stream is object-safe and
    /// usable in the form the orchestrator binds (`Arc<dyn OutputStream>`).
    #[tokio::test]
    async fn usable_as_dyn_output_stream() {
        let sink = MockSink::arc();
        let stream: Arc<dyn OutputStream> = Arc::new(AdapterOutputStream::new(sink.clone()));

        stream.emit_text("via trait object").await;

        let events = sink.events().await;
        assert_eq!(
            events[0],
            ClientEvent::TextDelta {
                text: "via trait object".to_string()
            }
        );
    }
}

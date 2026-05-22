//! Side effects emitted by the reducer.
//!
//! Each variant is a request to do something with the outside world.
//! `EffectHandler` (in `lingxi-traits`) processes these. See spec §5.3.
//!
//! M1.1 ships a subset: API, render, persistence. Later plans (Tools, Hooks,
//! Memory, MCP, Agent, etc.) extend this enum.

use crate::ids::{RequestId, SessionId};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A side effect requested by the reducer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Effect {
    // — API —
    /// Send a fully-assembled request body. Reply arrives as `Event::ApiStream*`.
    SendApiRequest {
        /// Correlation ID used to match streaming events back to this request.
        request_id: RequestId,
        /// Provider-shape body (Anthropic JSON for now; `OpenAI` in Plan 4 expansion).
        request_body: Value,
    },

    // — Render (UI / TUI / SDK consumers) —
    /// Emit an incremental text delta to attached UI consumers.
    RenderStreamDelta {
        /// The new text fragment to append to the current assistant turn.
        text: String,
    },
    /// Emit a user-visible error message to attached UI consumers.
    RenderError {
        /// Human-readable error description.
        error: String,
    },
    /// Update token usage counters surfaced in the UI.
    RenderTokenUsageUpdate {
        /// Cumulative input tokens for the current turn.
        input_tokens: u64,
        /// Cumulative output tokens for the current turn.
        output_tokens: u64,
    },

    // — Persistence (full session model lands in Plan 10) —
    /// Persist a snapshot of session state to durable storage.
    PersistSessionSnapshot {
        /// Session this snapshot belongs to.
        session_id: SessionId,
        /// Opaque snapshot payload (schema defined in Plan 10).
        snapshot: Value,
    },

    // — Lifecycle —
    /// Request the host to load a previously-persisted session.
    LoadSession {
        /// Session identifier to load.
        session_id: SessionId,
    },
    /// Terminate the run loop with a reason for logs.
    Terminate {
        /// Free-form reason text; surfaced in logs and telemetry.
        reason: String,
    },

    // — Diagnostic —
    /// Reducer hit a (state, event) pair it doesn't have a transition for.
    /// Emitted instead of `tracing::warn!` to keep the reducer pure.
    RecordUnexpectedEvent {
        /// Name of the state the reducer was in.
        state_name: String,
        /// Name of the event variant that had no handler.
        event_name: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_api_request_roundtrip() {
        let e = Effect::SendApiRequest {
            request_id: RequestId::nil(),
            request_body: serde_json::json!({"model": "claude-opus-4-6"}),
        };
        let s = serde_json::to_string(&e).unwrap();
        let e2: Effect = serde_json::from_str(&s).unwrap();
        assert_eq!(e, e2);
    }

    #[test]
    fn render_stream_delta_carries_text() {
        let e = Effect::RenderStreamDelta {
            text: "hello".into(),
        };
        let s = serde_json::to_string(&e).unwrap();
        assert!(s.contains("hello"));
    }
}

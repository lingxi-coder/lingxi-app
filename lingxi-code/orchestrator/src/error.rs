//! Orchestrator-side errors.
//!
//! `MaxTurnsReached` carries the byte-locked Display literal
//! `"Reached maximum number of turns (<n>)"` — verified against
//! `claude-code/src/QueryEngine.ts:870` on 2026-05-25.

use llm_client::LlmError;
use thiserror::Error;

/// Failure modes of `ConversationOrchestrator::run_turn`.
///
/// `MaxTurnsReached` is byte-locked against `QueryEngine.ts:870`. Do not
/// change the format string without a corresponding spec amendment.
#[derive(Debug, Error)]
pub enum OrchestratorError {
    /// The configured `max_turns` budget was exhausted before the model
    /// emitted `stop_reason == "end_turn"`.
    #[error("Reached maximum number of turns ({max_turns})")]
    MaxTurnsReached {
        /// The configured ceiling that was reached.
        max_turns: u32,
    },

    /// The model API call failed (transport, rate-limit, context overflow,
    /// etc). Wraps `llm_client::LlmError` — the live path flows through
    /// `ProviderApiAdapter → DefaultLlmClient`.
    #[error("api call failed: {0}")]
    ApiCall(#[from] LlmError),

    /// A non-tool runtime error inside the orchestrator. Used for
    /// internal invariants (unexpected content block variant in the
    /// hot path, etc.). Test stubs use this for synthetic failures.
    #[error("orchestrator internal error: {0}")]
    Internal(String),

    /// Mid-stream byte-level error from the streaming transport. Surfaced
    /// when the SSE chunk fails to decode or the HTTP body is cut.
    ///
    /// No `#[from]` impl — the batched `ApiCall` variant already claims
    /// `LlmError`. Convert manually at the streaming call site via
    /// `OrchestratorError::Streaming(llm_err)`.
    #[error("streaming transport error: {0}")]
    Streaming(LlmError),

    /// Stream produced an event that violates the per-block protocol
    /// (out-of-order delta, double stop, type mismatch, malformed
    /// `tool_use` input JSON).
    #[error("streaming protocol violation: {0}")]
    StreamingProtocol(String),

    /// Stream ended cleanly before a `message_stop` arrived. Mirrors
    /// claude-code's "stream completed without `message_start`" fallback
    /// (claude.ts:2353) — surfaced as an explicit error rather than
    /// silently retrying.
    #[error("stream ended without message_stop event")]
    StreamEndedWithoutStop,

    /// Compaction layer surfaced an error. (M6-08)
    #[error("compaction failed: {0}")]
    Compaction(#[from] compaction::CompactionError),

    /// `force_compact` was cancelled mid-run by the supplied
    /// `CancellationToken`. (M6-08)
    #[error("compaction cancelled")]
    CompactionCancelled,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_turns_reached_display_is_byte_locked_against_query_engine_ts_870() {
        // Source: claude-code/src/QueryEngine.ts:870
        //   `Reached maximum number of turns (${message.attachment.maxTurns})`
        let err = OrchestratorError::MaxTurnsReached { max_turns: 30 };
        assert_eq!(err.to_string(), "Reached maximum number of turns (30)");
    }

    #[test]
    fn max_turns_reached_display_handles_one() {
        let err = OrchestratorError::MaxTurnsReached { max_turns: 1 };
        assert_eq!(err.to_string(), "Reached maximum number of turns (1)");
    }

    #[test]
    fn max_turns_reached_display_handles_large_number() {
        let err = OrchestratorError::MaxTurnsReached { max_turns: 9999 };
        assert_eq!(err.to_string(), "Reached maximum number of turns (9999)");
    }

    #[test]
    fn internal_display_carries_payload() {
        let err = OrchestratorError::Internal("synthetic".into());
        assert_eq!(err.to_string(), "orchestrator internal error: synthetic");
    }

    #[test]
    fn streaming_display_starts_with_locked_prefix() {
        let e = OrchestratorError::Streaming(llm_client::LlmError::Transport {
            message: "nope".into(),
        });
        let s = format!("{e}");
        assert!(s.starts_with("streaming transport error: "), "{s}");
    }

    #[test]
    fn streaming_protocol_display_carries_inner() {
        let e = OrchestratorError::StreamingProtocol("block 3 has no start".into());
        assert_eq!(
            format!("{e}"),
            "streaming protocol violation: block 3 has no start"
        );
    }

    #[test]
    fn stream_ended_without_stop_display_is_locked() {
        assert_eq!(
            format!("{}", OrchestratorError::StreamEndedWithoutStop),
            "stream ended without message_stop event"
        );
    }

    #[test]
    fn compaction_variant_projects_to_string() {
        // CompactionError::Api still wraps api_client::ApiError (until 3b);
        // construct via the string variant to keep error.rs free of api_client.
        let compact_err = compaction::CompactionError::Internal("test".into());
        let e = OrchestratorError::Compaction(compact_err);
        let s = e.to_string();
        assert!(s.contains("compaction"), "got: {s}");
    }

    #[test]
    fn compaction_cancelled_renders() {
        let e = OrchestratorError::CompactionCancelled;
        assert_eq!(e.to_string(), "compaction cancelled");
    }
}

//! Orchestrator-side errors.
//!
//! `MaxTurnsReached` carries the byte-locked Display literal
//! `"Reached maximum number of turns (<n>)"` — verified against
//! `claude-code/src/QueryEngine.ts:870` on 2026-05-25.
//!
//! Task 7: `RepeatedOverloaded` carries the byte-locked
//! `"Repeated 529 Overloaded errors"` copy from
//! `claude-code/src/services/api/errors.ts:166`.

use llm_client::LlmError;
use thiserror::Error;

/// Byte-locked copy for the "Repeated 529 Overloaded errors" error message.
///
/// Locked against `claude-code/src/services/api/errors.ts:166`:
/// ```ts
/// export const REPEATED_529_ERROR_MESSAGE = 'Repeated 529 Overloaded errors'
/// ```
/// Thrown by `withRetry.ts:359-362` for external, non-sandbox callers when
/// `consecutive_overloaded >= MAX_529_RETRIES` and no fallback model is
/// configured.  The Rust equivalent is [`OrchestratorError::RepeatedOverloaded`].
pub const REPEATED_529_ERROR_MESSAGE: &str = "Repeated 529 Overloaded errors";

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
    ///
    /// Note: `LlmError::Overloaded { repeated: true }` is **not** wrapped here;
    /// it is converted to [`OrchestratorError::RepeatedOverloaded`] instead.
    /// See the manual `From<LlmError>` impl below.
    #[error("api call failed: {0}")]
    ApiCall(LlmError),

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

    /// The API returned `Overloaded` (529) on every retry attempt and
    /// `consecutive_overloaded >= MAX_529_RETRIES` for an external,
    /// non-sandbox caller with no fallback model configured.
    ///
    /// Display is byte-locked to `errors.ts:166`:
    /// `"Repeated 529 Overloaded errors"` — do not change without a
    /// corresponding spec amendment.
    ///
    /// Task 7 (claude.ts parity): `ProviderApiAdapter` returns
    /// `LlmError::Overloaded { repeated: true }` from the
    /// `DriveStep::RepeatedOverloaded` arm; the `From<LlmError>` impl on
    /// `OrchestratorError` converts it to this variant.
    #[error("{}", REPEATED_529_ERROR_MESSAGE)]
    RepeatedOverloaded,
}

/// Convert `LlmError` → `OrchestratorError`.
///
/// `Overloaded { repeated: true }` maps to [`OrchestratorError::RepeatedOverloaded`]
/// so the byte-locked "Repeated 529 Overloaded errors" message surfaces correctly
/// (errors.ts:166).  All other variants wrap as [`OrchestratorError::ApiCall`].
impl From<LlmError> for OrchestratorError {
    fn from(e: LlmError) -> Self {
        match e {
            LlmError::Overloaded { repeated: true } => OrchestratorError::RepeatedOverloaded,
            other => OrchestratorError::ApiCall(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Step 3b (Task 7): byte-locked copy for the "Repeated 529 Overloaded errors" error.
    ///
    /// Source: `claude-code/src/services/api/errors.ts:166`
    /// ```ts
    /// export const REPEATED_529_ERROR_MESSAGE = 'Repeated 529 Overloaded errors'
    /// ```
    /// Thrown by `withRetry.ts:359-362` for external, non-sandbox callers when
    /// `consecutive_overloaded >= MAX_529_RETRIES` and no fallback model is configured.
    #[test]
    fn repeated_529_terminal_renders_byte_locked_copy() {
        // Locked against errors.ts:166: REPEATED_529_ERROR_MESSAGE = 'Repeated 529 Overloaded errors'
        assert_eq!(
            REPEATED_529_ERROR_MESSAGE,
            "Repeated 529 Overloaded errors",
            "REPEATED_529_ERROR_MESSAGE constant must be byte-locked"
        );
        let err = OrchestratorError::RepeatedOverloaded;
        assert_eq!(
            err.to_string(),
            "Repeated 529 Overloaded errors",
            "OrchestratorError::RepeatedOverloaded must display the byte-locked copy"
        );
    }

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
        // CompactionError::Api now wraps llm_client::LlmError (retyped in 3b-T1);
        // construct via the string variant for simplicity.
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

    // ── Fix 2: From<LlmError> routes repeated bit correctly ──────────────────

    /// `LlmError::Overloaded { repeated: true }` must convert to
    /// `OrchestratorError::RepeatedOverloaded` (not `ApiCall`).
    #[test]
    fn overloaded_repeated_true_converts_to_repeated_overloaded() {
        let e: OrchestratorError = LlmError::Overloaded { repeated: true }.into();
        assert!(
            matches!(e, OrchestratorError::RepeatedOverloaded),
            "Overloaded {{ repeated: true }} must convert to RepeatedOverloaded, got {e:?}"
        );
        assert_eq!(e.to_string(), REPEATED_529_ERROR_MESSAGE);
    }

    /// `LlmError::Overloaded { repeated: false }` must convert to `ApiCall`
    /// (the normal error path).
    #[test]
    fn overloaded_repeated_false_converts_to_api_call() {
        let e: OrchestratorError = LlmError::Overloaded { repeated: false }.into();
        assert!(
            matches!(e, OrchestratorError::ApiCall(_)),
            "Overloaded {{ repeated: false }} must convert to ApiCall, got {e:?}"
        );
    }

    /// All non-overloaded `LlmError` variants must convert to `ApiCall`.
    #[test]
    fn non_overloaded_llm_errors_convert_to_api_call() {
        let variants: Vec<LlmError> = vec![
            LlmError::Authentication,
            LlmError::PermissionDenied,
            LlmError::ProviderInternal,
            LlmError::Transport { message: "t".into() },
            LlmError::RateLimited { retry_after: None, scope: None },
            LlmError::ContextOverflow { token_gap: 0 },
            LlmError::InvalidRequest { message: "bad".into() },
        ];
        for variant in variants {
            let e: OrchestratorError = variant.clone().into();
            assert!(
                matches!(e, OrchestratorError::ApiCall(_)),
                "{variant:?} must convert to ApiCall, got {e:?}"
            );
        }
    }
}

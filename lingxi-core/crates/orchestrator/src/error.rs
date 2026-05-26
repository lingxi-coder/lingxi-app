//! Orchestrator-side errors.
//!
//! `MaxTurnsReached` carries the byte-locked Display literal
//! `"Reached maximum number of turns (<n>)"` — verified against
//! `claude-code/src/QueryEngine.ts:870` on 2026-05-25.

use lingxi_api_client::ApiError;
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

    /// The Anthropic API call failed after exhausting `AnthropicProvider`'s
    /// own retry budget (3 attempts, 500ms/1s/2s ± 20% jitter — see M3-03).
    #[error("api call failed: {0}")]
    ApiCall(#[from] ApiError),

    /// A non-tool runtime error inside the orchestrator. Used for
    /// internal invariants (unexpected `ContentBlockApi` variant in the
    /// hot path, etc.). Test stubs use this for synthetic failures.
    #[error("orchestrator internal error: {0}")]
    Internal(String),
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
}

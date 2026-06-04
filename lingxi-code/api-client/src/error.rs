//! Error type for the API client layer.
//!
//! Wraps transport errors from [`traits::HttpError`] and adds
//! semantic variants that downstream layers (retry, prompt-too-long
//! handler) match on.

use thiserror::Error;
use traits::HttpError;

/// Errors returned by the API client.
#[derive(Debug, Clone, Error)]
pub enum ApiError {
    /// Underlying HTTP transport failure.
    #[error(transparent)]
    Http(#[from] HttpError),

    /// The prompt exceeded the model's input token limit.
    #[error("prompt too long: server requested {token_gap} fewer input tokens")]
    PromptTooLong {
        /// Number of input tokens the server suggested removing.
        token_gap: u64,
        /// Raw server response body, retained for diagnostics.
        raw: String,
    },

    /// HTTP 429: rate limited by the provider.
    #[error("Rate limited; retrying in {retry_after_secs}s")]
    RateLimited {
        /// Number of seconds the server asked us to wait before retrying.
        retry_after_secs: u64,
    },

    /// HTTP 529 (or a streamed body carrying `"type":"overloaded_error"`): the
    /// provider is at transient capacity. `repeated` is set once the retry
    /// budget is exhausted on consecutive overloads, switching the display
    /// string to the byte-locked claude-code message.
    ///
    /// Display string is **byte-locked** to claude-code `errors.ts:166`
    /// (`REPEATED_529_ERROR_MESSAGE = 'Repeated 529 Overloaded errors'`) when
    /// `repeated`, else the lowercase `"overloaded"` token used by `is529Error`
    /// classification.
    #[error("{}", if *repeated { "Repeated 529 Overloaded errors" } else { "overloaded" })]
    Overloaded {
        /// `true` once the overload persists across the whole retry budget.
        repeated: bool,
    },

    /// The consecutive-529 counter reached `MAX_529_RETRIES` (3) on a
    /// non-custom Opus primary model and a fallback model is configured: the
    /// retry loop stops and signals the caller (the orchestrator turn loop) to
    /// re-issue the request against `fallback_model` instead of retrying.
    ///
    /// 1:1 with claude-code `FallbackTriggeredError` (`withRetry.ts:160-168`,
    /// thrown at `:347`). The display string is **byte-locked** to TS
    /// `withRetry.ts:165` (`Model fallback triggered: ${originalModel} -> ${fallbackModel}`).
    #[error("Model fallback triggered: {original_model} -> {fallback_model}")]
    FallbackTriggered {
        /// The primary model that hit the consecutive-529 threshold.
        original_model: String,
        /// The configured fallback model the caller should re-issue against.
        fallback_model: String,
    },

    /// Authentication failed (HTTP 401 / 403 / invalid API key).
    #[error("authentication failed: {0}")]
    Unauthorized(String),

    /// SSE stream contained a malformed event that could not be parsed.
    #[error("server returned malformed event: {0}")]
    MalformedStream(String),

    /// SSE stream terminated before a `message_stop` event.
    #[error("stream ended unexpectedly")]
    UnexpectedStreamEnd,

    /// Server-side error response (any non-2xx the caller didn't already
    /// classify into a more specific variant). 4xx other than 401 falls
    /// through here from the retry loop.
    #[error("server error (HTTP {status}): {body}")]
    Server {
        /// Numeric HTTP status code.
        status: u16,
        /// Response body verbatim.
        body: String,
    },

    /// Retry budget exhausted. Carries the last HTTP status seen, if any.
    #[error("retry budget exhausted (last status: {last_status:?})")]
    RetryExhausted {
        /// HTTP status of the last attempt, or `None` if every attempt
        /// failed with a transport error before reaching a status line.
        last_status: Option<u16>,
    },

    /// Provider does not support the requested model for this endpoint
    /// (e.g. Vertex `count_tokens` is restricted to a 3-model whitelist).
    #[error("unsupported model {model} for provider {provider}")]
    UnsupportedModel {
        /// Model identifier the caller requested.
        model: String,
        /// Provider name (`anthropic`, `vertex`, `bedrock`).
        provider: &'static str,
    },

    /// OAuth hook raised an error during a 401-driven refresh attempt.
    #[error("oauth refresh hook failed: {0}")]
    OAuthHook(#[from] crate::oauth_hook::OAuthHookError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_too_long_carries_token_gap() {
        let e = ApiError::PromptTooLong {
            token_gap: 1500,
            raw: "...".into(),
        };
        assert!(format!("{e}").contains("1500"));
    }

    #[test]
    fn rate_limited_display_string_is_byte_locked() {
        let e = ApiError::RateLimited {
            retry_after_secs: 7,
        };
        assert_eq!(format!("{e}"), "Rate limited; retrying in 7s");
    }

    #[test]
    fn overloaded_repeated_display_string_is_byte_locked() {
        // Byte-locked against claude-code errors.ts:166 REPEATED_529_ERROR_MESSAGE.
        let e = ApiError::Overloaded { repeated: true };
        assert_eq!(format!("{e}"), "Repeated 529 Overloaded errors");
    }

    #[test]
    fn overloaded_single_display_string() {
        let e = ApiError::Overloaded { repeated: false };
        assert_eq!(format!("{e}"), "overloaded");
    }

    #[test]
    fn fallback_triggered_display_string_is_byte_locked() {
        // Byte-locked against claude-code withRetry.ts:165
        // (`Model fallback triggered: ${originalModel} -> ${fallbackModel}`).
        let e = ApiError::FallbackTriggered {
            original_model: "claude-opus-4-6".into(),
            fallback_model: "claude-sonnet-4-6".into(),
        };
        assert_eq!(
            format!("{e}"),
            "Model fallback triggered: claude-opus-4-6 -> claude-sonnet-4-6"
        );
    }
}

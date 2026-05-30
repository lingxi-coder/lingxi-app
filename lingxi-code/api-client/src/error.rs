//! Error type for the API client layer.
//!
//! Wraps transport errors from [`lingxi_traits::HttpError`] and adds
//! semantic variants that downstream layers (retry, prompt-too-long
//! handler) match on.

use lingxi_traits::HttpError;
use thiserror::Error;

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
}

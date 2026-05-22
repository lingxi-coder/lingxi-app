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
    #[error("rate limited (HTTP 429): retry after {retry_after_secs}s")]
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
}

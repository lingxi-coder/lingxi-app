//! Provider-neutral error taxonomy.

use std::time::Duration;

/// Public provider-neutral error type for LLM client operations.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LlmError {
    /// Authentication failed or credentials are missing/invalid.
    #[error("authentication failed")]
    Authentication,
    /// Caller is authenticated but not allowed to perform the request.
    #[error("permission denied")]
    PermissionDenied,
    /// Provider rejected the request as invalid.
    #[error("invalid request: {message}")]
    InvalidRequest {
        /// Provider or validation message explaining why the request is invalid.
        message: String,
    },
    /// Provider rate-limited the request.
    #[error("rate limited")]
    RateLimited {
        /// Optional server-provided retry-after duration.
        retry_after: Option<Duration>,
        /// Optional provider-specific rate-limit scope.
        scope: Option<String>,
    },
    /// Provider quota or billing limit was exceeded.
    #[error("quota exceeded")]
    QuotaExceeded,
    /// Request exceeded provider/model context limits.
    ///
    /// `token_gap` is the actual-minus-limit token gap when the provider
    /// reported counts in the error message; `0` when unknown.
    #[error("context overflow")]
    ContextOverflow {
        /// How many tokens over the limit the prompt was, or `0` when unknown.
        token_gap: u64,
    },
    /// Requested model is unavailable.
    #[error("model unavailable")]
    ModelUnavailable,
    /// Provider returned a transient/internal failure.
    #[error("provider internal error")]
    ProviderInternal,
    /// Provider reported overload (Anthropic 529 / `overloaded_error`).
    #[error("provider overloaded")]
    Overloaded,
    /// Transport failed before a provider response was decoded.
    #[error("transport error: {message}")]
    Transport {
        /// Transport-layer failure message.
        message: String,
    },
    /// A stream failed after semantic events had been yielded.
    #[error("stream interrupted: {message}")]
    StreamInterrupted {
        /// Stream interruption detail safe for caller-facing diagnostics.
        message: String,
    },
    /// Pricing was required but unavailable.
    #[error("cost unavailable: {message}")]
    CostUnavailable {
        /// Pricing lookup failure message.
        message: String,
    },
    /// Request used a capability the route/model does not support.
    #[error("unsupported capability: {capability}")]
    UnsupportedCapability {
        /// Unsupported capability name.
        capability: String,
    },
}

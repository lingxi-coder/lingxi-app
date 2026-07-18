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
    /// Request body exceeded the provider's maximum size — a 413 whose message
    /// does NOT mention the context window (accumulated images/attachments
    /// pushed the raw request over the byte limit, not token overflow).
    ///
    /// Parity: claude-code 2.1.212 splits status 413 — a message containing
    /// `"context window"` stays prompt-too-long ([`LlmError::ContextOverflow`],
    /// which drives compaction); everything else becomes this DISTINCT variant,
    /// surfaced with the byte-exact `"Request too large (max 32MB). Accumulated
    /// images and attachments…"` notice. Terminal: compaction cannot shed image
    /// bytes, so it is never retried and never triggers the PTL recovery loop.
    #[error("request too large")]
    RequestTooLarge,
    /// Requested model is unavailable.
    #[error("model unavailable")]
    ModelUnavailable,
    /// Provider returned a transient/internal failure.
    #[error("provider internal error")]
    ProviderInternal,
    /// Provider reported overload (Anthropic 529 / `overloaded_error`).
    ///
    /// `repeated` is `true` when the caller has seen `consecutive_overloaded >=
    /// MAX_529_RETRIES` with no fallback available — the orchestrator sets this
    /// flag so upper layers can surface the byte-locked
    /// `"Repeated 529 Overloaded errors"` copy (`errors.ts:166`).  All sources
    /// that decode a fresh 529 from the wire set `repeated: false`.
    #[error("provider overloaded")]
    Overloaded {
        /// `true` when this is the repeated-overload terminal error; `false`
        /// for a fresh 529 from the wire.
        repeated: bool,
    },
    /// Transport failed before a provider response was decoded.
    #[error("transport error: {message}")]
    Transport {
        /// Transport-layer failure message.
        message: String,
    },
    /// Transport failed due to a TLS/SSL certificate error.
    ///
    /// A certificate failure (expired cert, self-signed cert, a corporate
    /// TLS-intercepting proxy, a protocol/handshake fault, …) is **terminal**:
    /// it is never retried, because retrying a handshake that can't succeed
    /// only burns the retry budget. This is split out from
    /// [`LlmError::Transport`] so retry classification can short-circuit it and
    /// callers can surface the fix hint.
    ///
    /// Parity: claude-code 2.1.201 `JF` cause-chain classifier + the `Gyo`/`bBp`
    /// code sets + the `YLe` hint (see [`crate::ssl`]). `code` is the matched
    /// Node/OpenSSL error code (e.g. `CERT_HAS_EXPIRED`); `message` is the
    /// user-facing `YLe` hint (which embeds the code and the remediation).
    #[error("{message}")]
    TlsCert {
        /// The matched TLS error code (a member of the `bBp` set), e.g.
        /// `CERT_HAS_EXPIRED`.
        code: String,
        /// The user-facing SSL fix hint (`YLe`) — includes the code and the
        /// `NODE_EXTRA_CA_CERTS` / `/doctor` remediation.
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

impl LlmError {
    /// Build a terminal [`LlmError::TlsCert`] for the given TLS error `code`,
    /// rendering the `YLe` fix hint into `message`.
    ///
    /// Parity: claude-code 2.1.201 — a cause-chain `code` in the `bBp` set
    /// yields `isSSLError`, which short-circuits the retry loop and surfaces the
    /// `YLe` hint (see [`crate::ssl::ssl_hint`]).
    #[must_use]
    pub fn tls_cert(code: impl Into<String>) -> Self {
        let code = code.into();
        let message = crate::ssl::ssl_hint(&code);
        LlmError::TlsCert { code, message }
    }

    /// The matched TLS error code when this is a [`LlmError::TlsCert`], else
    /// `None`. Mirrors reading `JF(e).code` on an `isSSLError` error.
    #[must_use]
    pub fn ssl_code(&self) -> Option<&str> {
        match self {
            LlmError::TlsCert { code, .. } => Some(code.as_str()),
            _ => None,
        }
    }
}

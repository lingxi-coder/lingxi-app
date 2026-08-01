//! Provider-neutral error taxonomy.

use std::time::Duration;

/// Public provider-neutral error type for LLM client operations.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LlmError {
    /// Authentication failed or credentials are missing/invalid.
    #[error("authentication failed")]
    Authentication,
    /// The stored OAuth refresh token was REJECTED by the IdP — the session is
    /// dead and only a fresh `/login` can revive it.
    ///
    /// Parity: claude-code models this as a distinct error class
    /// (`OAuthRefreshDeadError`, minified `qQt`) rather than a flavour of
    /// [`Self::Authentication`], and renders it with its own copy ("Login
    /// expired · Please run /login"). Kept a separate variant here for the same
    /// reason: a stale token hash or an unreachable IdP are also refresh
    /// failures, but neither means the user has to log in again.
    #[error("oauth session expired")]
    OAuthRefreshDead,
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

    /// Recover the HTTP status this error was decoded from, when it is
    /// knowable — the inverse of `providers::api_error_message`.
    ///
    /// claude-code does not keep status in a side field either; the SDK's
    /// `makeMessage` prefixes it onto the message (`${status} ${body}`) and
    /// downstream code parses it back off — `replace(/^429\s+/,"")` @230600821,
    /// `replace(/^400\s+/,"")` @230602614. This is that parse, generalised.
    ///
    /// Returns `None` rather than guessing: a wrong status written to the
    /// transcript's `apiErrorStatus` is worse than an absent one, which is the
    /// same trade the oracle makes when the error is not an `APIError` with a
    /// numeric status.
    #[must_use]
    pub fn http_status(&self) -> Option<u16> {
        match self {
            LlmError::InvalidRequest { message }
            | LlmError::Transport { message }
            | LlmError::StreamInterrupted { message }
            | LlmError::CostUnavailable { message } => api_error_status(message),
            _ => None,
        }
    }
}

/// Parse the leading `${status} ` that `api_error_message` writes.
///
/// Deliberately strict, because [`LlmError::InvalidRequest`] has ~200
/// construction sites that never went through a provider decoder and whose
/// messages are arbitrary prose. The shape must be exactly what `makeMessage`
/// emits:
///
/// * three ASCII digits,
/// * in `100..=599` — a real HTTP status, so `"999 bottles"` is prose,
/// * followed by a single space with a non-empty remainder.
///
/// Residual ambiguity is accepted and bounded: prose beginning `"404 "` parses
/// as a status. In practice a decoded provider error's remainder is the
/// stringified body (`{"type":"error",…}`), and the only consumer is an
/// `apiErrorStatus` field that previously held a hardcoded guess — so a rare
/// false positive replaces a systematic one.
#[must_use]
pub fn api_error_status(message: &str) -> Option<u16> {
    let (head, rest) = message.split_at(message.char_indices().nth(3).map_or(0, |(i, _)| i));
    if head.len() != 3 || !head.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let rest = rest.strip_prefix(' ')?;
    if rest.is_empty() {
        return None;
    }
    let status: u16 = head.parse().ok()?;
    (100..=599).contains(&status).then_some(status)
}

#[cfg(test)]
mod api_error_status_tests {
    use super::{api_error_status, LlmError};

    #[test]
    fn parses_the_shape_make_message_emits() {
        assert_eq!(
            api_error_status(r#"400 {"type":"error","error":{"message":"bad"}}"#),
            Some(400)
        );
        assert_eq!(api_error_status("429 slow down"), Some(429));
        assert_eq!(api_error_status("503 status code (no body)"), Some(503));
    }

    #[test]
    fn rejects_everything_that_is_not_that_shape() {
        // No space — `makeMessage` always emits exactly one.
        assert_eq!(api_error_status("400bad"), None);
        // Nothing after the space.
        assert_eq!(api_error_status("400 "), None);
        // Not a real HTTP status.
        assert_eq!(api_error_status("999 bottles"), None);
        assert_eq!(api_error_status("099 leading zero"), None);
        assert_eq!(api_error_status("600 too high"), None);
        // Not three digits.
        assert_eq!(api_error_status("40 x"), None);
        assert_eq!(api_error_status("4000 x"), None);
        // Ordinary prose, which is what most InvalidRequest sites carry.
        assert_eq!(api_error_status("invalid model name"), None);
        assert_eq!(api_error_status(""), None);
        // Multi-byte lead must not panic on a non-char-boundary split.
        assert_eq!(api_error_status("\u{4e2d}\u{6587} x"), None);
    }

    #[test]
    fn only_message_bearing_variants_expose_a_status() {
        assert_eq!(
            LlmError::InvalidRequest {
                message: "422 {}".to_string()
            }
            .http_status(),
            Some(422)
        );
        // A variant with no message can never carry a prefix.
        assert_eq!(LlmError::Authentication.http_status(), None);
        assert_eq!(LlmError::ProviderInternal.http_status(), None);
    }
}

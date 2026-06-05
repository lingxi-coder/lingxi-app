//! Prompt-too-long (413 / "prompt is too long") detection + token-gap parsing.
//!
//! When the Messages API rejects a request because the assembled prompt exceeds
//! the model's **input** token limit, it returns a 4xx (commonly **400** on
//! Anthropic, **413** elsewhere) whose error message has the shape:
//!
//! ```text
//! prompt is too long: 137500 tokens > 135000 maximum
//! ```
//!
//! claude-code parses the two numbers to learn how far over the limit the
//! prompt is, then the reactive-recovery loop drops the oldest API rounds to
//! shave off that gap and retries (see `compaction::ptl_retry`). Without a typed
//! error variant the orchestrator's recovery loop has nothing to match on, so
//! this module ports the classification + parsing and the
//! [`AnthropicProvider`](crate::AnthropicProvider) wires
//! [`reclassify_prompt_too_long`] into its non-2xx funnel.
//!
//! Ports of claude-code `src/services/api/errors.ts:62-118`:
//! * [`PROMPT_TOO_LONG_ERROR_MESSAGE`] — `PROMPT_TOO_LONG_ERROR_MESSAGE` (`:62`).
//! * [`parse_prompt_too_long_token_counts`] — `parsePromptTooLongTokenCounts`
//!   (`:85-96`), byte-identical lenient regex.
//! * [`prompt_too_long_token_gap`] — `getPromptTooLongTokenGap`'s gap arithmetic
//!   (`:104-118`): `actual − limit` when `> 0`, else "unknown".

#![forbid(unsafe_code)]

use crate::ApiError;
use regex::Regex;
use std::sync::OnceLock;

/// Byte-locked against claude-code `PROMPT_TOO_LONG_ERROR_MESSAGE`
/// (`errors.ts:62`): the model-facing assistant text surfaced when reactive
/// recovery is finally exhausted. The orchestrator re-exports this so the turn
/// loop emits the identical string.
pub const PROMPT_TOO_LONG_ERROR_MESSAGE: &str = "Prompt is too long";

/// Compiled prompt-too-long capture regex, lazily built once.
///
/// Pattern is byte-identical to claude-code `errors.ts:89-91`
/// (`/prompt is too long[^0-9]*(\d+)\s*tokens?\s*>\s*(\d+)/i`); the `i` flag is
/// expressed as the inline `(?i)` prefix. The leading `[^0-9]*` tolerates SDK
/// prefixes / punctuation between the phrase and the first number, so the same
/// pattern matches Vertex / Bedrock casing and JSON-enveloped bodies.
fn ptl_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)prompt is too long[^0-9]*(\d+)\s*tokens?\s*>\s*(\d+)")
            .expect("prompt-too-long regex is a valid, fixed pattern")
    })
}

/// Parse `(actual_tokens, limit_tokens)` out of a raw prompt-too-long error body
/// like `"prompt is too long: 137500 tokens > 135000 maximum"`.
///
/// 1:1 with claude-code `parsePromptTooLongTokenCounts` (`errors.ts:85-96`):
/// intentionally lenient (the raw string may be wrapped in SDK prefixes or JSON
/// envelopes, or use different casing). Either element is `None` when the body
/// does not match the pattern.
#[must_use]
pub fn parse_prompt_too_long_token_counts(raw: &str) -> (Option<u64>, Option<u64>) {
    match ptl_regex().captures(raw) {
        Some(caps) => (
            caps.get(1).and_then(|m| m.as_str().parse::<u64>().ok()),
            caps.get(2).and_then(|m| m.as_str().parse::<u64>().ok()),
        ),
        None => (None, None),
    }
}

/// Cheap case-insensitive gate for "this body is a prompt-too-long rejection".
///
/// Mirrors the `error.message.toLowerCase().includes('prompt is too long')`
/// guard claude-code uses before parsing (`yoloClassifier.ts:1467`).
#[must_use]
pub fn is_prompt_too_long_body(body: &str) -> bool {
    body.to_lowercase().contains("prompt is too long")
}

/// How many input tokens over the limit the prompt-too-long body reports, or
/// `0` when the counts are unparseable (Vertex / Bedrock formats omit them).
///
/// Mirrors `getPromptTooLongTokenGap` (`errors.ts:104-118`): `actual − limit`
/// when both parse and `actual > limit`, else "unknown". The compaction PTL
/// truncator treats `0` as "unknown gap" and falls back to its 20% heuristic,
/// matching the TS `undefined` path.
#[must_use]
pub fn prompt_too_long_token_gap(body: &str) -> u64 {
    match parse_prompt_too_long_token_counts(body) {
        (Some(actual), Some(limit)) if actual > limit => actual - limit,
        _ => 0,
    }
}

/// Build a typed [`ApiError::PromptTooLong`] from a non-2xx `(status, body)`
/// when it is a prompt-too-long rejection: **HTTP 413**, or any status whose
/// body says "prompt is too long" (Anthropic returns this as a 400). Returns
/// `None` for everything else so the caller keeps the original classification.
///
/// claude-code classifies these from the 400/413 error bodies; mirroring that
/// here is what makes the orchestrator's reactive recovery reachable.
#[must_use]
pub fn classify_prompt_too_long(status: u16, body: &str) -> Option<ApiError> {
    if status == 413 || is_prompt_too_long_body(body) {
        Some(ApiError::PromptTooLong {
            token_gap: prompt_too_long_token_gap(body),
            raw: body.to_string(),
        })
    } else {
        None
    }
}

/// Reclassify a generic [`ApiError::Server`] into [`ApiError::PromptTooLong`]
/// when its `(status, body)` is a prompt-too-long rejection; pass every other
/// error (and non-`Server` variants) through unchanged.
///
/// Applied at the [`AnthropicProvider`](crate::AnthropicProvider) non-2xx funnel
/// so the typed variant is actually constructed on the live path.
#[must_use]
pub fn reclassify_prompt_too_long(error: ApiError) -> ApiError {
    if let ApiError::Server { status, body } = &error {
        if let Some(ptl) = classify_prompt_too_long(*status, body) {
            return ptl;
        }
    }
    error
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_canonical_message() {
        let (actual, limit) =
            parse_prompt_too_long_token_counts("prompt is too long: 137500 tokens > 135000 maximum");
        assert_eq!(actual, Some(137_500));
        assert_eq!(limit, Some(135_000));
    }

    #[test]
    fn parses_case_insensitively() {
        let (actual, limit) =
            parse_prompt_too_long_token_counts("Prompt Is Too Long: 200 Tokens > 100 maximum");
        assert_eq!(actual, Some(200));
        assert_eq!(limit, Some(100));
    }

    #[test]
    fn parses_singular_token_and_json_envelope() {
        // Singular "token" (the `tokens?` quantifier) inside a JSON envelope.
        let body = r#"{"type":"error","error":{"message":"prompt is too long: 5 token > 4 maximum"}}"#;
        let (actual, limit) = parse_prompt_too_long_token_counts(body);
        assert_eq!(actual, Some(5));
        assert_eq!(limit, Some(4));
    }

    #[test]
    fn unparseable_yields_none() {
        let (actual, limit) = parse_prompt_too_long_token_counts("internal server error");
        assert_eq!(actual, None);
        assert_eq!(limit, None);
        // A PTL phrase with no numeric counts (Vertex/Bedrock) → both None.
        let (a2, l2) = parse_prompt_too_long_token_counts("prompt is too long for this model");
        assert_eq!((a2, l2), (None, None));
    }

    #[test]
    fn token_gap_is_actual_minus_limit() {
        assert_eq!(
            prompt_too_long_token_gap("prompt is too long: 137500 tokens > 135000 maximum"),
            2_500
        );
    }

    #[test]
    fn token_gap_zero_when_unknown_or_not_over() {
        // Unparseable → 0 (the truncator reads this as "unknown → 20% fallback").
        assert_eq!(prompt_too_long_token_gap("prompt is too long for this model"), 0);
        // actual <= limit (shouldn't happen for a real PTL) → 0, not underflow.
        assert_eq!(
            prompt_too_long_token_gap("prompt is too long: 100 tokens > 200 maximum"),
            0
        );
    }

    #[test]
    fn classifies_413_without_counts() {
        let e = classify_prompt_too_long(413, "Payload Too Large").expect("413 is PTL");
        match e {
            ApiError::PromptTooLong { token_gap, raw } => {
                assert_eq!(token_gap, 0); // no counts in the body
                assert_eq!(raw, "Payload Too Large");
            }
            other => panic!("expected PromptTooLong, got {other:?}"),
        }
    }

    #[test]
    fn classifies_400_prompt_too_long_body_with_gap() {
        let body = "prompt is too long: 137500 tokens > 135000 maximum";
        let e = classify_prompt_too_long(400, body).expect("PTL body is PTL");
        match e {
            ApiError::PromptTooLong { token_gap, .. } => assert_eq!(token_gap, 2_500),
            other => panic!("expected PromptTooLong, got {other:?}"),
        }
    }

    #[test]
    fn does_not_classify_unrelated_errors() {
        assert!(classify_prompt_too_long(500, "internal error").is_none());
        assert!(classify_prompt_too_long(429, "rate limited").is_none());
    }

    #[test]
    fn reclassify_only_rewrites_matching_server_errors() {
        // A PTL Server error is rewritten.
        let rewritten = reclassify_prompt_too_long(ApiError::Server {
            status: 400,
            body: "prompt is too long: 9 tokens > 4 maximum".into(),
        });
        assert!(matches!(
            rewritten,
            ApiError::PromptTooLong { token_gap: 5, .. }
        ));
        // A plain 500 Server error is left untouched.
        let untouched = reclassify_prompt_too_long(ApiError::Server {
            status: 500,
            body: "boom".into(),
        });
        assert!(matches!(untouched, ApiError::Server { status: 500, .. }));
        // Non-Server variants pass through.
        let rl = reclassify_prompt_too_long(ApiError::RateLimited {
            retry_after_secs: 3,
        });
        assert!(matches!(rl, ApiError::RateLimited { .. }));
    }
}

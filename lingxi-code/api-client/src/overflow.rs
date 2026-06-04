//! `max_tokens` context-overflow detection + re-shrink arithmetic.
//!
//! When the Anthropic Messages API rejects a request because the prompt plus
//! the requested `max_tokens` exceed the model's context window, it returns a
//! **400** whose error message has the byte-exact shape:
//!
//! ```text
//! input length and `max_tokens` exceed context limit: 188059 + 20000 > 200000
//! ```
//!
//! claude-code parses those three numbers, recomputes a safe `max_tokens` that
//! still fits the window, and retries the request **once more** with the
//! reduced cap instead of failing the turn.
//!
//! This module ports the two pure helpers that drive that behaviour:
//!
//! * [`parse_max_tokens_overflow`] — 1:1 with claude-code
//!   `parseMaxTokensContextOverflowError` (`withRetry.ts:550-595`).
//! * [`adjusted_max_tokens`] — 1:1 with the overflow branch arithmetic
//!   (`withRetry.ts:391-415`): `safetyBuffer = 1000`, `FLOOR_OUTPUT_TOKENS =
//!   3000`, and `minRequired = thinkingBudget + 1`.
//!
//! The retry wiring (mutating the request body and re-looping) lives in
//! `anthropic.rs::drive_retry_loop_with_429`; this module is intentionally
//! pure so the arithmetic can be unit-tested in isolation.

#![forbid(unsafe_code)]

use regex::Regex;
use std::sync::OnceLock;

/// Floor on the re-shrunk output-token budget. Byte-locked against claude-code
/// `FLOOR_OUTPUT_TOKENS` (`withRetry.ts:53`): the adjusted `max_tokens` is never
/// allowed below this, even if the available context is smaller — in which case
/// we instead give up and surface the original 400 (see
/// [`adjusted_max_tokens`]).
pub const FLOOR_OUTPUT_TOKENS: u64 = 3000;

/// Safety buffer subtracted from the context limit before recomputing the
/// output budget. Byte-locked against claude-code `safetyBuffer` (`withRetry.ts:393`).
pub const SAFETY_BUFFER: u64 = 1000;

/// The three numbers parsed out of a `max_tokens` context-overflow 400.
///
/// Field names mirror the TS object returned by
/// `parseMaxTokensContextOverflowError`: `{ inputTokens, maxTokens, contextLimit }`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Overflow {
    /// `A` in `A + B > C` — the prompt / input token count the server measured.
    pub input_tokens: u64,
    /// `B` in `A + B > C` — the `max_tokens` we asked for.
    pub max_tokens: u64,
    /// `C` in `A + B > C` — the model's context-window limit.
    pub context_limit: u64,
}

/// Compiled overflow regex, lazily built once.
///
/// Pattern is byte-identical to claude-code `withRetry.ts:571`:
/// ``/input length and `max_tokens` exceed context limit: (\d+) \+ (\d+) > (\d+)/``.
fn overflow_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // The pattern is a fixed literal; compilation cannot fail. Using
        // `expect` keeps the helper infallible without a fallible public API.
        Regex::new(r"input length and `max_tokens` exceed context limit: (\d+) \+ (\d+) > (\d+)")
            .expect("overflow regex is a valid, fixed pattern")
    })
}

/// Parse a `max_tokens` context-overflow **400** into its three numbers.
///
/// 1:1 with claude-code `parseMaxTokensContextOverflowError`
/// (`withRetry.ts:550-595`):
///
/// 1. Returns `None` unless `status == 400` (`:557`).
/// 2. Returns `None` unless the body contains the literal substring
///    `` input length and `max_tokens` exceed context limit `` (`:561-567`) —
///    this cheap `contains` gate precedes the regex exactly as TS does.
/// 3. Applies the byte-identical capture regex and parses the three groups as
///    base-10 integers (`:569-588`).
///
/// `body` is the server's error message text (claude-code reads
/// `error.message`); for the Rust transport this is the raw 400 response body,
/// which carries the same wording.
///
/// Note: the three captures are `\d+`, so the values always parse; the TS
/// `isNaN` guard (`:590`) is structurally unreachable here and is therefore not
/// reproduced as a separate branch.
#[must_use]
pub fn parse_max_tokens_overflow(status: u16, body: &str) -> Option<Overflow> {
    if status != 400 {
        return None;
    }
    // Cheap substring gate before the regex, mirroring TS `:561-567`.
    if !body.contains("input length and `max_tokens` exceed context limit") {
        return None;
    }
    let caps = overflow_regex().captures(body)?;
    // Groups are `\d+`; parse cannot fail for in-range values. We use `ok()?`
    // so an absurdly large number (overflowing u64) degrades to `None` rather
    // than panicking — TS parses to a JS number (f64), which never throws.
    let input_tokens = caps.get(1)?.as_str().parse::<u64>().ok()?;
    let max_tokens = caps.get(2)?.as_str().parse::<u64>().ok()?;
    let context_limit = caps.get(3)?.as_str().parse::<u64>().ok()?;
    Some(Overflow {
        input_tokens,
        max_tokens,
        context_limit,
    })
}

/// Recompute a safe `max_tokens` from a parsed [`Overflow`], or `None` to give
/// up (surface the original 400).
///
/// 1:1 with the overflow branch arithmetic in claude-code
/// (`withRetry.ts:391-415`):
///
/// ```text
/// availableContext = max(0, contextLimit - inputTokens - 1000)
/// if availableContext < 3000:          // FLOOR_OUTPUT_TOKENS
///     return None                       // throw error (give up)
/// minRequired = thinkingBudget + 1      // thinkingBudget = 0 when disabled
/// adjusted = max(3000, availableContext, minRequired)
/// ```
///
/// `thinking_budget` is the extended-thinking budget in tokens (0 when thinking
/// is disabled), threaded from the request options. The `+ 1` guarantees at
/// least one output token beyond the thinking budget.
///
/// The result is returned as a `u32` to match the request body's `max_tokens`
/// wire type. The clamp arithmetic happens in `u64` to avoid overflow; the
/// final value is bounded by `context_limit` (a realistic window ≤ a few
/// million) so the `u32` cast is lossless in practice. If, pathologically, the
/// value exceeds `u32::MAX` it is saturated to `u32::MAX`.
#[must_use]
pub fn adjusted_max_tokens(overflow: Overflow, thinking_budget: u64) -> Option<u32> {
    // availableContext = max(0, contextLimit - inputTokens - safetyBuffer).
    // saturating_sub gives the `max(0, ...)` clamp for free.
    let available_context = overflow
        .context_limit
        .saturating_sub(overflow.input_tokens)
        .saturating_sub(SAFETY_BUFFER);

    if available_context < FLOOR_OUTPUT_TOKENS {
        // Give up: not enough room even at the floor. Caller surfaces the
        // original 400 (claude-code `throw error`, withRetry.ts:404).
        return None;
    }

    // minRequired = thinkingBudget + 1 (at least one output token).
    let min_required = thinking_budget.saturating_add(1);

    let adjusted = FLOOR_OUTPUT_TOKENS
        .max(available_context)
        .max(min_required);

    Some(u32::try_from(adjusted).unwrap_or(u32::MAX))
}

#[cfg(test)]
mod parse_tests {
    use super::*;

    /// The exact sample message from the TS source comment (`withRetry.ts:569`).
    const SAMPLE: &str =
        "input length and `max_tokens` exceed context limit: 188059 + 20000 > 200000";

    #[test]
    fn parses_exact_sample() {
        let got = parse_max_tokens_overflow(400, SAMPLE).expect("should parse");
        assert_eq!(
            got,
            Overflow {
                input_tokens: 188_059,
                max_tokens: 20_000,
                context_limit: 200_000,
            }
        );
    }

    #[test]
    fn parses_when_embedded_in_a_larger_json_body() {
        // The real 400 body wraps the message in an error envelope; the
        // substring + regex must still find it.
        let body = format!(
            r#"{{"type":"error","error":{{"type":"invalid_request_error","message":"{SAMPLE}"}}}}"#
        );
        let got = parse_max_tokens_overflow(400, &body).expect("should parse from envelope");
        assert_eq!(got.input_tokens, 188_059);
        assert_eq!(got.max_tokens, 20_000);
        assert_eq!(got.context_limit, 200_000);
    }

    #[test]
    fn rejects_non_400_status() {
        // Same message, wrong status → None (TS `:557`).
        assert_eq!(parse_max_tokens_overflow(429, SAMPLE), None);
        assert_eq!(parse_max_tokens_overflow(500, SAMPLE), None);
        assert_eq!(parse_max_tokens_overflow(200, SAMPLE), None);
    }

    #[test]
    fn rejects_400_without_marker() {
        assert_eq!(
            parse_max_tokens_overflow(400, "some other invalid_request_error"),
            None
        );
    }

    #[test]
    fn rejects_marker_present_but_numbers_missing() {
        // The substring gate passes but the `A + B > C` regex does not match,
        // so the whole thing is None (TS `match.length !== 4`, `:574`).
        let body = "input length and `max_tokens` exceed context limit (no numbers here)";
        assert_eq!(parse_max_tokens_overflow(400, body), None);
    }

    #[test]
    fn parses_arbitrary_numbers() {
        let body = "input length and `max_tokens` exceed context limit: 5 + 7 > 11";
        assert_eq!(
            parse_max_tokens_overflow(400, body),
            Some(Overflow {
                input_tokens: 5,
                max_tokens: 7,
                context_limit: 11,
            })
        );
    }
}

#[cfg(test)]
mod adjust_tests {
    use super::*;

    /// The worked example from the spec: input 188059, limit 200000, thinking 0
    /// → available = 200000 - 188059 - 1000 = 10941 → max(3000, 10941, 1) = 10941.
    #[test]
    fn spec_sample_thinking_zero() {
        let overflow = Overflow {
            input_tokens: 188_059,
            max_tokens: 20_000,
            context_limit: 200_000,
        };
        assert_eq!(adjusted_max_tokens(overflow, 0), Some(10_941));
    }

    #[test]
    fn floor_wins_when_available_below_floor_but_room_remains() {
        // available = 100000 - 96000 - 1000 = 3000 (== floor) → not < floor →
        // max(3000, 3000, 1) = 3000.
        let overflow = Overflow {
            input_tokens: 96_000,
            max_tokens: 50_000,
            context_limit: 100_000,
        };
        assert_eq!(adjusted_max_tokens(overflow, 0), Some(3000u32));
    }

    #[test]
    fn gives_up_when_available_strictly_below_floor() {
        // available = 100000 - 96001 - 1000 = 2999 (< 3000) → None (give up).
        let overflow = Overflow {
            input_tokens: 96_001,
            max_tokens: 50_000,
            context_limit: 100_000,
        };
        assert_eq!(adjusted_max_tokens(overflow, 0), None);
    }

    #[test]
    fn tiny_context_gives_up() {
        // Spec "tiny-context" case: nearly no room → None.
        let overflow = Overflow {
            input_tokens: 199_000,
            max_tokens: 20_000,
            context_limit: 200_000,
        };
        // available = 200000 - 199000 - 1000 = 0 < 3000 → None.
        assert_eq!(adjusted_max_tokens(overflow, 0), None);
    }

    #[test]
    fn thinking_budget_raises_min_required() {
        // available = 200000 - 188059 - 1000 = 10941; thinking budget 15000 →
        // minRequired = 15001 → max(3000, 10941, 15001) = 15001.
        let overflow = Overflow {
            input_tokens: 188_059,
            max_tokens: 20_000,
            context_limit: 200_000,
        };
        assert_eq!(adjusted_max_tokens(overflow, 15_000), Some(15_001));
    }

    #[test]
    fn thinking_budget_below_available_does_not_raise() {
        // thinking 5000 → minRequired 5001 < available 10941 → stays 10941.
        let overflow = Overflow {
            input_tokens: 188_059,
            max_tokens: 20_000,
            context_limit: 200_000,
        };
        assert_eq!(adjusted_max_tokens(overflow, 5_000), Some(10_941));
    }

    #[test]
    fn saturates_to_zero_available_on_underflow() {
        // inputTokens > contextLimit → saturating_sub clamps to 0 → < floor → None.
        let overflow = Overflow {
            input_tokens: 300_000,
            max_tokens: 20_000,
            context_limit: 200_000,
        };
        assert_eq!(adjusted_max_tokens(overflow, 0), None);
    }

    #[test]
    fn constants_are_byte_locked() {
        assert_eq!(FLOOR_OUTPUT_TOKENS, 3000);
        assert_eq!(SAFETY_BUFFER, 1000);
    }
}

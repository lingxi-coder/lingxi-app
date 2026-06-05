//! Exponential-backoff retry middleware with ±20% jitter.
//!
//! Spec §7 lines 660-661: retry budget = 3 attempts, base delays
//! `[500, 1000, 2000]` ms before retries, with a jitter factor
//! sampled from `[0.8, 1.2)` for thundering-herd protection.
//!
//! Used by `anthropic.rs::do_request_with_middleware`. Public surface is
//! intentionally narrow — the loop is shaped around `Result<HttpResponse,
//! HttpError>` rather than a fully generic `Try` so the middleware can
//! introspect HTTP status codes without unwrapping a generic `Result`.

#![forbid(unsafe_code)]

use rand::Rng;
use std::time::Duration;

/// Default base delays in milliseconds before each retry attempt. **Locked
/// against spec §7**: changing these requires updating
/// `parity_messages_create.json`.
pub const DEFAULT_BASE_DELAYS_MS: &[u64] = &[500, 1_000, 2_000];

/// Default retry budget (3 attempts). **Locked against spec §7**.
pub const DEFAULT_RETRY_BUDGET: u8 = 3;

/// Consecutive-529 threshold before the fallback / repeated-overload decision
/// fires. Byte-locked to claude-code `withRetry.ts:54`
/// (`const MAX_529_RETRIES = 3`).
pub const MAX_529_RETRIES: u8 = 3;

/// Lower jitter bound — exclusive end is 1.2 to avoid doubling the delay.
pub const JITTER_LOW: f64 = 0.8;
/// Upper jitter bound (exclusive).
pub const JITTER_HIGH: f64 = 1.2;

/// Compute a jittered delay: `base_ms * uniform(0.8, 1.2)`.
///
/// Uses `rand::thread_rng()` so independent retry loops do not share state
/// across tokio tasks.
///
/// **Precision note**: the spec works in milliseconds where sub-ms accuracy
/// is irrelevant to network timing. `base_ms as f64` and the round-trip back
/// to `u64` are intentional — `clippy::cast_*` lints are silenced locally
/// with justification.
#[must_use]
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "ms-scale timing; factor is in [0.8, 1.2) so f64*u64 stays in u64 range and is non-negative"
)]
pub fn jittered_delay(base_ms: u64) -> Duration {
    let factor: f64 = rand::thread_rng().gen_range(JITTER_LOW..JITTER_HIGH);
    // Multiply in f64, cast back to u64 ms. Rounding direction does not matter
    // (we're in milliseconds; sub-ms accuracy is irrelevant to network timing).
    let ms = ((base_ms as f64) * factor) as u64;
    Duration::from_millis(ms)
}

#[cfg(test)]
mod jittered_delay {
    //! Tests grouped under a submodule named `jittered_delay` so that the
    //! cargo filter `retry::jittered_delay` matches every test below.
    use super::*;

    #[test]
    fn jitter_bounds_are_locked_against_spec() {
        assert!((JITTER_LOW - 0.8).abs() < f64::EPSILON);
        assert!((JITTER_HIGH - 1.2).abs() < f64::EPSILON);
    }

    #[test]
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "ms-scale comparison; the values fit u64 trivially"
    )]
    fn stays_within_plus_minus_20_percent() {
        // Sample many times; every result must fall in [0.8x, 1.2x).
        let base = 1_000u64;
        let lo = ((base as f64) * 0.8) as u64;
        let hi = ((base as f64) * 1.2) as u64;
        for _ in 0..2_000 {
            let d = jittered_delay(base);
            let ms = d.as_millis() as u64;
            assert!(
                ms >= lo && ms < hi,
                "delay {ms}ms outside [{lo}, {hi}) for base={base}",
            );
        }
    }

    #[test]
    fn default_base_delays_match_spec() {
        assert_eq!(DEFAULT_BASE_DELAYS_MS, &[500, 1_000, 2_000]);
        assert_eq!(DEFAULT_RETRY_BUDGET, 3);
    }
}

use crate::error::ApiError;
use crate::overflow::{parse_max_tokens_overflow, Overflow};
use protocol::HttpResponse;
use traits::HttpError;

/// Body substring the SDK leaks into the error message when a 529 status is
/// dropped during streaming. Byte-identical to claude-code `withRetry.ts:619`
/// (`error.message?.includes('"type":"overloaded_error"')`).
pub const OVERLOADED_ERROR_BODY_MARKER: &str = "\"type\":\"overloaded_error\"";

/// Whether an HTTP response is an Anthropic *529 / overloaded* signal.
///
/// 1:1 with claude-code `is529Error` (`withRetry.ts:610-621`): either the
/// status is literally `529`, **or** the response body contains the
/// `"type":"overloaded_error"` marker — the SDK sometimes fails to surface the
/// 529 status during streaming, so the body substring is the fallback. The
/// substring check is byte-identical to the TS `includes(...)` call.
#[must_use]
pub fn is_529(status: u16, body: &str) -> bool {
    status == 529 || body.contains(OVERLOADED_ERROR_BODY_MARKER)
}

/// Classification of a completed HTTP response for the retry loop.
///
/// Mirrors the decision tree in claude-code `shouldRetry`
/// (`withRetry.ts:696-787`), reduced to the cases the generic `with_retry`
/// loop adjudicates on `HttpResponse.status` / `body`:
///
/// * [`RetryClass::Retry`] — 529 / overloaded (tagged via `overloaded: true`),
///   408 request timeout, 409 lock timeout, or any 5xx server error.
/// * [`RetryClass::Fallthrough`] — 429 rate limit. The generic loop does not
///   own the subscriber-gate / reset-delay policy; the caller
///   (`anthropic.rs::drive_retry_loop_with_429`) intercepts and handles 429
///   before it reaches here, so in the loop this surfaces as a terminal
///   `Server` like any other unhandled status (see `with_retry`).
/// * [`RetryClass::AdjustAndRetry`] — the 400 `max_tokens` context-overflow
///   error (claude-code `withRetry.ts:727`). The generic loop cannot re-shrink
///   the request body, so the caller
///   (`anthropic.rs::drive_retry_loop_with_429`) intercepts this 400 before it
///   reaches the generic loop, recomputes `max_tokens`, mutates the body, and
///   re-attempts. This variant exists so the classifier *routes* the overflow
///   400 to a retry path rather than `Terminal` (Batch 1 dependency).
/// * [`RetryClass::Terminal`] — every other status (2xx is handled before
///   classification; 3xx, other 400/401/403, etc. are non-retryable here).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryClass {
    /// Retry (subject to the attempt budget). `overloaded` is `true` for the
    /// 529 / `overloaded_error` case so the loop can surface the byte-locked
    /// `ApiError::Overloaded { repeated: true }` on exhaustion.
    Retry {
        /// Set for 529 / streamed `overloaded_error`; clears for 408/409/5xx.
        overloaded: bool,
    },
    /// 429 — owned by the caller's rate-limit path, not this loop.
    Fallthrough,
    /// 400 `max_tokens` context overflow — owned by the caller's body-reshrink
    /// path (`drive_retry_loop_with_429`), not the generic loop. Carries the
    /// parsed [`Overflow`] numbers so the caller can recompute `max_tokens`.
    AdjustAndRetry(Overflow),
    /// Non-retryable; return the response as an error immediately.
    Terminal,
}

/// Classify a completed (already-received) [`HttpResponse`] for retry.
///
/// Order matches claude-code `shouldRetry`: the `overloaded_error` /529
/// short-circuit (`:722`) is checked first, then the `x-should-retry: false`
/// server directive (`:746`, terminal), then 408 (`:760`), 409 (`:763`), 429
/// (`:767`, caller-handled), and the trailing `status >= 500` rule (`:784`).
/// 2xx responses never reach here — `with_retry` returns them before
/// classifying.
#[must_use]
pub fn classify_retryable(resp: &HttpResponse) -> RetryClass {
    // Overloaded short-circuit first (claude-code checks the message body
    // before the status-code ladder, withRetry.ts:719-724).
    if is_529(resp.status, &resp.body) {
        return RetryClass::Retry { overloaded: true };
    }
    // 400 `max_tokens` context overflow — re-shrinkable (claude-code
    // withRetry.ts:727). Checked before the status ladder so the overflow 400
    // is routed to AdjustAndRetry instead of the trailing Terminal arm. Owned
    // by the caller (body reshrink), like the 429 Fallthrough.
    if let Some(overflow) = parse_max_tokens_overflow(resp.status, &resp.body) {
        return RetryClass::AdjustAndRetry(overflow);
    }
    // Server retry directive (claude-code `withRetry.ts:731-751`). An explicit
    // `x-should-retry: false` means "do not retry" — obeyed here for ALL status
    // codes (the ant-only `USER_TYPE==='ant'` 5xx carve-out at `:748` is omitted
    // in the external build). Checked AFTER the 529/overflow short-circuits
    // (which TS evaluates first at `:719`/`:727`) and BEFORE the status ladder,
    // so a 5xx carrying `x-should-retry: false` is terminal instead of retried.
    // The subscriber-gated `true` directive (`:737-742`,
    // `!isClaudeAISubscriber() || isEnterpriseSubscriber()`) is owned by the
    // caller's subscriber-aware path — like 429's `Fallthrough` — so it is NOT
    // handled here; a `true` value falls through to the status ladder
    // (documented partial port).
    if crate::rate_limit::header_value(&resp.headers, "x-should-retry") == Some("false") {
        return RetryClass::Terminal;
    }
    match resp.status {
        // Rate limit — owned by the caller (subscriber gates, reset delays).
        429 => RetryClass::Fallthrough,
        // 408 request timeout, 409 lock timeout, and any 5xx server error
        // (claude-code withRetry.ts:760, :763, :784) are retryable but not
        // overloaded; the 529/overloaded case was already handled above.
        408 | 409 | 500..=599 => RetryClass::Retry { overloaded: false },
        _ => RetryClass::Terminal,
    }
}

/// Consecutive-529 / Opus-fallback policy threaded into [`with_retry_ctl`].
///
/// Ports the consecutive-529 block of claude-code `withRetry.ts:326-365`. The
/// loop maintains a `consecutive_529` counter that increments on each 529 /
/// overloaded response and **resets to 0 on any non-529 outcome** (success,
/// timeout, or a non-overloaded retryable status). When the counter reaches
/// [`Self::max_529_retries`] *and* [`Self::allow_fallback`] is set, the loop
/// stops retrying and signals the caller:
///
/// * if [`Self::fallback_model`] is `Some` → [`ApiError::FallbackTriggered`]
///   (the orchestrator re-issues against the fallback model — claude-code
///   `withRetry.ts:347`);
/// * else if the user is external & not sandboxed → [`ApiError::Overloaded`]
///   `{ repeated: true }` with the byte-locked `Repeated 529 Overloaded errors`
///   message (claude-code `withRetry.ts:359`);
/// * else the loop keeps retrying (subject to the attempt budget), matching the
///   TS fall-through when neither branch applies.
///
/// `allow_fallback` is the **pre-computed** TS guard
/// `FALLBACK_FOR_ALL_PRIMARY_MODELS || (!isClaudeAISubscriber() &&
/// isNonCustomOpusModel(model))` (`withRetry.ts:331-332`) — the caller resolves
/// the env flag / subscriber state / Opus check once and hands the result in.
#[derive(Debug, Clone)]
pub struct RetryControl {
    /// Consecutive-529 threshold. Defaults to [`MAX_529_RETRIES`] (3).
    pub max_529_retries: u8,
    /// Fallback model to signal via [`ApiError::FallbackTriggered`] once the
    /// threshold trips; `None` disables the fallback signal.
    pub fallback_model: Option<String>,
    /// The primary model in flight — carried into
    /// [`ApiError::FallbackTriggered::original_model`].
    pub primary_model: String,
    /// Pre-computed TS guard (`FALLBACK_FOR_ALL_PRIMARY_MODELS ||
    /// (!is_subscriber && is_non_custom_opus(primary_model))`). When `false`,
    /// the consecutive-529 gate never trips and the loop behaves like the
    /// budget-only [`with_retry`].
    pub allow_fallback: bool,
    /// `USER_TYPE === 'external'` (claude-code `withRetry.ts:354`). Gates the
    /// no-fallback `Overloaded { repeated: true }` terminal branch.
    pub is_external: bool,
    /// `!!process.env.IS_SANDBOX` (claude-code `withRetry.ts:355`). When set,
    /// the no-fallback terminal branch is skipped (sandbox keeps retrying).
    pub is_sandbox: bool,
}

impl Default for RetryControl {
    /// Behaviour-neutral default: no fallback configured and the gate disabled,
    /// so [`with_retry_ctl`] reduces exactly to the legacy [`with_retry`]
    /// budget-only loop. Used by the thin [`with_retry`] wrapper and by callers
    /// that have not wired the consecutive-529 policy yet.
    fn default() -> Self {
        Self {
            max_529_retries: MAX_529_RETRIES,
            fallback_model: None,
            primary_model: String::new(),
            allow_fallback: false,
            is_external: false,
            is_sandbox: false,
        }
    }
}

/// Wraps an HTTP call in exponential backoff with jitter.
///
/// `attempts` is the **total** attempts (NOT retries). With `attempts = 3`
/// the closure runs at most 3 times, sleeping
/// `jittered_delay(base_delays_ms[i])` before attempt `i+1` for `i ∈ [0,
/// attempts-1)`. After the last attempt fails the function returns
/// [`ApiError::RetryExhausted`].
///
/// `base_delays_ms.len()` must be at least `attempts - 1`; shorter slices
/// fall back to repeating the last entry.
///
/// This is the budget-only entrypoint: it delegates to [`with_retry_ctl`] with
/// a default ([`RetryControl::default`]) policy, so the consecutive-529 /
/// Opus-fallback gate never fires. Callers that need the fallback signal use
/// [`with_retry_ctl`] directly.
///
/// # Errors
/// Returns the first non-retryable error encountered, or
/// [`ApiError::RetryExhausted`] if every attempt failed with a retryable
/// status (5xx / transport error). 4xx other than 401 are non-retryable;
/// 401 is handled at the caller (`anthropic.rs::do_request_with_middleware`).
pub async fn with_retry<F, Fut>(
    attempts: u8,
    base_delays_ms: &[u64],
    f: F,
) -> Result<HttpResponse, ApiError>
where
    F: FnMut(u8) -> Fut,
    Fut: std::future::Future<Output = Result<HttpResponse, HttpError>>,
{
    with_retry_ctl(attempts, base_delays_ms, &RetryControl::default(), f).await
}

/// [`with_retry`] plus the consecutive-529 / Opus-fallback policy carried by
/// [`RetryControl`].
///
/// Identical retry/backoff machinery as [`with_retry`], with one addition: a
/// `consecutive_529` counter (claude-code `withRetry.ts:186,334`). On each
/// `Retry { overloaded: true }` outcome the counter increments **only when
/// `ctl.allow_fallback`** (the pre-computed TS guard, `:331-332`); any non-529
/// outcome resets it to 0. When it reaches `ctl.max_529_retries`
/// (`:335`):
///
/// * `ctl.fallback_model.is_some()` → [`ApiError::FallbackTriggered`] (`:347`);
/// * else `ctl.is_external && !ctl.is_sandbox` → [`ApiError::Overloaded`]
///   `{ repeated: true }` (`:359`);
/// * else the loop keeps retrying under the normal attempt budget (TS
///   fall-through — neither branch throws).
///
/// # Errors
/// As [`with_retry`], plus [`ApiError::FallbackTriggered`] /
/// [`ApiError::Overloaded`] `{ repeated: true }` from the consecutive-529 gate.
pub async fn with_retry_ctl<F, Fut>(
    attempts: u8,
    base_delays_ms: &[u64],
    ctl: &RetryControl,
    mut f: F,
) -> Result<HttpResponse, ApiError>
where
    F: FnMut(u8) -> Fut,
    Fut: std::future::Future<Output = Result<HttpResponse, HttpError>>,
{
    let mut last_status: Option<u16> = None;
    // Set true while the most recent retryable failure was a 529 / overloaded
    // response, so an exhausted budget surfaces the byte-locked
    // `ApiError::Overloaded { repeated: true }` instead of generic exhaustion.
    let mut last_overloaded = false;
    // Consecutive-529 counter (claude-code withRetry.ts:186). Increments only
    // when the fallback gate is open (`ctl.allow_fallback`); resets to 0 on any
    // non-529 outcome (success / timeout / non-overloaded retryable status).
    let mut consecutive_529: u8 = 0;
    for attempt in 0..attempts {
        if attempt > 0 {
            let base = base_delays_ms
                .get(attempt as usize - 1)
                .copied()
                .or_else(|| base_delays_ms.last().copied())
                .unwrap_or(500);
            tokio::time::sleep(jittered_delay(base)).await;
        }
        match f(attempt).await {
            // Overloaded short-circuit BEFORE the 2xx success check: the SDK can
            // surface a streamed `overloaded_error` with a 2xx-shaped status, so
            // a body carrying the marker must be retried, never treated as
            // success (claude-code checks the message body first, withRetry.ts:719-724).
            Ok(resp) if (200..300).contains(&resp.status) && !is_529(resp.status, &resp.body) => {
                return Ok(resp);
            }
            Ok(resp) => match classify_retryable(&resp) {
                RetryClass::Retry { overloaded } => {
                    last_status = Some(resp.status);
                    last_overloaded = overloaded;
                    if overloaded {
                        // Consecutive-529 gate (claude-code withRetry.ts:326-365).
                        // Only counts when the pre-computed guard is open
                        // (FALLBACK_FOR_ALL_PRIMARY_MODELS || (!subscriber &&
                        // non-custom-opus)).
                        if ctl.allow_fallback {
                            consecutive_529 = consecutive_529.saturating_add(1);
                            if consecutive_529 >= ctl.max_529_retries {
                                if let Some(fallback) = &ctl.fallback_model {
                                    // claude-code withRetry.ts:347 — signal the
                                    // caller to re-issue against the fallback.
                                    return Err(ApiError::FallbackTriggered {
                                        original_model: ctl.primary_model.clone(),
                                        fallback_model: fallback.clone(),
                                    });
                                }
                                if ctl.is_external && !ctl.is_sandbox {
                                    // claude-code withRetry.ts:359 — external,
                                    // non-sandbox, no fallback → terminal.
                                    return Err(ApiError::Overloaded { repeated: true });
                                }
                                // Neither branch applies → TS falls through to
                                // the normal retry budget; keep retrying.
                            }
                        }
                    } else {
                        // Non-529 retryable (408/409/5xx) resets the consecutive
                        // counter (the 529 run is broken).
                        consecutive_529 = 0;
                    }
                    tracing::warn!(
                        target: "lingxi::api_client::retry",
                        attempt = attempt + 1,
                        status = resp.status,
                        overloaded,
                        consecutive_529,
                        "retryable response; will retry"
                    );
                    continue;
                }
                // 429 (Fallthrough), the 400 overflow (AdjustAndRetry), and
                // every Terminal status surface as ApiError::Server for the
                // caller to inspect. In production both the 429 and the
                // overflow 400 are intercepted in the caller's closure before
                // reaching here (drive_retry_loop_with_429) — the 429 sleeps
                // and re-loops, the overflow 400 reshrinks `max_tokens` and
                // re-loops; this generic arm is the fallback for callers that
                // do not intercept (the body-mutation reshrink is not possible
                // from inside the generic, body-agnostic loop).
                RetryClass::Fallthrough
                | RetryClass::AdjustAndRetry(_)
                | RetryClass::Terminal => {
                    return Err(ApiError::Server {
                        status: resp.status,
                        body: resp.body,
                    });
                }
            },
            Err(HttpError::Timeout(_) | HttpError::Connection(_)) => {
                last_overloaded = false;
                // A transport error is a non-529 outcome → reset the run.
                consecutive_529 = 0;
                tracing::warn!(
                    target: "lingxi::api_client::retry",
                    attempt = attempt + 1,
                    "transient transport error; will retry"
                );
                continue;
            }
            Err(other) => {
                return Err(ApiError::Http(other));
            }
        }
    }
    if last_overloaded {
        return Err(ApiError::Overloaded { repeated: true });
    }
    Err(ApiError::RetryExhausted { last_status })
}

#[cfg(test)]
mod with_retry_tests {
    use super::*;
    use protocol::HttpResponse;
    use std::sync::atomic::{AtomicU8, Ordering};
    use std::sync::Arc;

    fn ok(status: u16, body: &str) -> HttpResponse {
        HttpResponse {
            status,
            headers: Vec::new(),
            body: body.into(),
        }
    }

    #[tokio::test]
    async fn first_attempt_2xx_no_retry() {
        let count = Arc::new(AtomicU8::new(0));
        let c = Arc::clone(&count);
        let r = with_retry(3, &[1, 1, 1], move |_| {
            let c = Arc::clone(&c);
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(ok(200, "hello"))
            }
        })
        .await;
        assert!(r.is_ok());
        assert_eq!(count.load(Ordering::SeqCst), 1, "should not retry on 200");
    }

    #[tokio::test]
    async fn three_5xx_returns_retry_exhausted() {
        let count = Arc::new(AtomicU8::new(0));
        let c = Arc::clone(&count);
        let r = with_retry(3, &[1, 1, 1], move |_| {
            let c = Arc::clone(&c);
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(ok(503, "boom"))
            }
        })
        .await;
        match r {
            Err(ApiError::RetryExhausted { last_status }) => {
                assert_eq!(last_status, Some(503));
            }
            other => panic!("expected RetryExhausted, got {other:?}"),
        }
        assert_eq!(count.load(Ordering::SeqCst), 3, "all 3 attempts must run");
    }

    #[tokio::test]
    async fn five_xx_then_200_succeeds_in_two_tries() {
        let count = Arc::new(AtomicU8::new(0));
        let c = Arc::clone(&count);
        let r = with_retry(3, &[1, 1, 1], move |_| {
            let c = Arc::clone(&c);
            async move {
                let n = c.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    Ok(ok(500, "transient"))
                } else {
                    Ok(ok(200, "ok"))
                }
            }
        })
        .await;
        assert!(r.is_ok());
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn four_hundred_returns_immediately_no_retry() {
        let count = Arc::new(AtomicU8::new(0));
        let c = Arc::clone(&count);
        let r = with_retry(3, &[1, 1, 1], move |_| {
            let c = Arc::clone(&c);
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(ok(400, "bad request"))
            }
        })
        .await;
        match r {
            Err(ApiError::Server { status, .. }) => assert_eq!(status, 400),
            other => panic!("expected Server, got {other:?}"),
        }
        assert_eq!(count.load(Ordering::SeqCst), 1, "4xx must not retry");
    }

    #[tokio::test]
    async fn timeout_is_retried() {
        let count = Arc::new(AtomicU8::new(0));
        let c = Arc::clone(&count);
        let r = with_retry(3, &[1, 1, 1], move |_| {
            let c = Arc::clone(&c);
            async move {
                let n = c.fetch_add(1, Ordering::SeqCst);
                if n < 2 {
                    Err(HttpError::Timeout(std::time::Duration::from_millis(1)))
                } else {
                    Ok(ok(200, "recovered"))
                }
            }
        })
        .await;
        assert!(r.is_ok());
        assert_eq!(count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn five_twenty_nine_is_retried_then_exhausts_as_overloaded() {
        let count = Arc::new(AtomicU8::new(0));
        let c = Arc::clone(&count);
        let r = with_retry(3, &[1, 1, 1], move |_| {
            let c = Arc::clone(&c);
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(ok(529, "overloaded"))
            }
        })
        .await;
        // All attempts run, and exhaustion surfaces as Overloaded { repeated },
        // not generic RetryExhausted (this is the 529 short-circuit path).
        match r {
            Err(ApiError::Overloaded { repeated }) => assert!(repeated),
            other => panic!("expected Overloaded {{ repeated: true }}, got {other:?}"),
        }
        assert_eq!(count.load(Ordering::SeqCst), 3, "529 must be retried");
    }

    #[tokio::test]
    async fn five_twenty_nine_then_200_recovers() {
        let count = Arc::new(AtomicU8::new(0));
        let c = Arc::clone(&count);
        let r = with_retry(3, &[1, 1, 1], move |_| {
            let c = Arc::clone(&c);
            async move {
                let n = c.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    Ok(ok(529, "overloaded"))
                } else {
                    Ok(ok(200, "ok"))
                }
            }
        })
        .await;
        assert!(r.is_ok());
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn overloaded_body_on_200_is_retried() {
        // SDK streaming fallback: a 200 whose body carries the overloaded marker
        // must be classified Retry, not treated as success.
        let count = Arc::new(AtomicU8::new(0));
        let c = Arc::clone(&count);
        let r = with_retry(2, &[1], move |_| {
            let c = Arc::clone(&c);
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(ok(200, r#"{"type":"overloaded_error"}"#))
            }
        })
        .await;
        match r {
            Err(ApiError::Overloaded { repeated }) => assert!(repeated),
            other => panic!("expected Overloaded, got {other:?}"),
        }
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn overloaded_body_on_503_is_retried() {
        let count = Arc::new(AtomicU8::new(0));
        let c = Arc::clone(&count);
        let r = with_retry(2, &[1], move |_| {
            let c = Arc::clone(&c);
            async move {
                let n = c.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    Ok(ok(503, r#"{"type":"overloaded_error"}"#))
                } else {
                    Ok(ok(200, "recovered"))
                }
            }
        })
        .await;
        assert!(r.is_ok());
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn four_oh_eight_and_four_oh_nine_are_retried() {
        for status in [408u16, 409] {
            let count = Arc::new(AtomicU8::new(0));
            let c = Arc::clone(&count);
            let r = with_retry(3, &[1, 1, 1], move |_| {
                let c = Arc::clone(&c);
                async move {
                    let n = c.fetch_add(1, Ordering::SeqCst);
                    if n == 0 {
                        Ok(ok(status, "timeout"))
                    } else {
                        Ok(ok(200, "ok"))
                    }
                }
            })
            .await;
            assert!(r.is_ok(), "status {status} should retry then succeed");
            assert_eq!(count.load(Ordering::SeqCst), 2, "status {status}");
        }
    }

    #[tokio::test]
    async fn four_oh_three_is_terminal_no_retry() {
        let count = Arc::new(AtomicU8::new(0));
        let c = Arc::clone(&count);
        let r = with_retry(3, &[1, 1, 1], move |_| {
            let c = Arc::clone(&c);
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(ok(403, "forbidden"))
            }
        })
        .await;
        match r {
            Err(ApiError::Server { status, .. }) => assert_eq!(status, 403),
            other => panic!("expected Server, got {other:?}"),
        }
        assert_eq!(count.load(Ordering::SeqCst), 1, "403 must not retry");
    }

    #[tokio::test]
    async fn timeout_then_overloaded_exhaustion_is_not_overloaded() {
        // last_overloaded must reset after a transport error so a trailing
        // timeout exhaustion does not masquerade as Overloaded.
        let count = Arc::new(AtomicU8::new(0));
        let c = Arc::clone(&count);
        let r = with_retry(2, &[1], move |_| {
            let c = Arc::clone(&c);
            async move {
                let n = c.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    Ok(ok(529, "overloaded"))
                } else {
                    Err(HttpError::Timeout(std::time::Duration::from_millis(1)))
                }
            }
        })
        .await;
        // Second (final) attempt was a timeout → generic exhaustion, not Overloaded.
        match r {
            Err(ApiError::RetryExhausted { last_status }) => assert_eq!(last_status, Some(529)),
            other => panic!("expected RetryExhausted, got {other:?}"),
        }
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn sleeps_jittered_delay_between_attempts() {
        let start = std::time::Instant::now();
        let _r = with_retry(3, &[10, 20, 40], |_| async move {
            Ok::<_, HttpError>(ok(500, "transient"))
        })
        .await;
        let elapsed = start.elapsed();
        // After 3 attempts (2 sleeps), the floor on total sleep is
        // 10*0.8 + 20*0.8 = 24 ms. Allow some slack for scheduler noise.
        assert!(
            elapsed.as_millis() >= 20,
            "expected at least ~24ms of cumulative sleep, got {elapsed:?}",
        );
    }
}

#[cfg(test)]
mod consecutive_529_fallback {
    //! Tests for the consecutive-529 counter + Opus fallback policy
    //! (`with_retry_ctl` / [`RetryControl`]), named so the cargo filter
    //! `retry::consecutive_529_fallback` matches them all.
    use super::*;
    use protocol::HttpResponse;
    use std::sync::atomic::{AtomicU8, Ordering};
    use std::sync::Arc;

    fn ok(status: u16, body: &str) -> HttpResponse {
        HttpResponse {
            status,
            headers: Vec::new(),
            body: body.into(),
        }
    }

    /// Opus primary, gate open, fallback configured.
    fn ctl_with_fallback() -> RetryControl {
        RetryControl {
            fallback_model: Some("claude-sonnet-4-6".into()),
            primary_model: "claude-opus-4-6".into(),
            allow_fallback: true,
            is_external: true,
            is_sandbox: false,
            ..RetryControl::default()
        }
    }

    /// Opus primary, gate open, NO fallback, external + non-sandbox.
    fn ctl_no_fallback_external() -> RetryControl {
        RetryControl {
            fallback_model: None,
            primary_model: "claude-opus-4-6".into(),
            allow_fallback: true,
            is_external: true,
            is_sandbox: false,
            ..RetryControl::default()
        }
    }

    #[tokio::test]
    async fn three_529_with_fallback_triggers_fallback() {
        let count = Arc::new(AtomicU8::new(0));
        let c = Arc::clone(&count);
        let ctl = ctl_with_fallback();
        let r = with_retry_ctl(3, &[1, 1, 1], &ctl, move |_| {
            let c = Arc::clone(&c);
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(ok(529, "overloaded"))
            }
        })
        .await;
        match r {
            Err(ApiError::FallbackTriggered {
                original_model,
                fallback_model,
            }) => {
                assert_eq!(original_model, "claude-opus-4-6");
                assert_eq!(fallback_model, "claude-sonnet-4-6");
            }
            other => panic!("expected FallbackTriggered, got {other:?}"),
        }
        // The gate fires on the 3rd 529 (counter reaches MAX_529_RETRIES).
        assert_eq!(count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn three_529_no_fallback_external_is_repeated_overloaded() {
        let count = Arc::new(AtomicU8::new(0));
        let c = Arc::clone(&count);
        let ctl = ctl_no_fallback_external();
        let r = with_retry_ctl(3, &[1, 1, 1], &ctl, move |_| {
            let c = Arc::clone(&c);
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(ok(529, "overloaded"))
            }
        })
        .await;
        match r {
            Err(ApiError::Overloaded { repeated }) => assert!(repeated),
            other => panic!("expected Overloaded {{ repeated: true }}, got {other:?}"),
        }
        // Byte-locked terminal message.
        assert_eq!(
            format!("{}", ApiError::Overloaded { repeated: true }),
            "Repeated 529 Overloaded errors"
        );
        assert_eq!(count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn two_529_then_200_recovers_and_resets_counter() {
        let count = Arc::new(AtomicU8::new(0));
        let c = Arc::clone(&count);
        let ctl = ctl_with_fallback();
        let r = with_retry_ctl(3, &[1, 1, 1], &ctl, move |_| {
            let c = Arc::clone(&c);
            async move {
                let n = c.fetch_add(1, Ordering::SeqCst);
                if n < 2 {
                    Ok(ok(529, "overloaded"))
                } else {
                    Ok(ok(200, "ok"))
                }
            }
        })
        .await;
        // 529, 529, 200 → counter never reaches 3, request succeeds.
        assert!(r.is_ok());
        assert_eq!(count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn non_529_breaks_the_run_before_threshold() {
        // 529, 500, 529 → the 500 resets the consecutive-529 counter, so the
        // gate never trips even though there were two 529s. The loop exhausts
        // its 3-attempt budget; the final attempt was a 529 (overloaded), so
        // exhaustion surfaces as Overloaded { repeated: true } via the existing
        // last_overloaded path — NOT FallbackTriggered.
        let count = Arc::new(AtomicU8::new(0));
        let c = Arc::clone(&count);
        let ctl = ctl_with_fallback();
        let r = with_retry_ctl(3, &[1, 1, 1], &ctl, move |_| {
            let c = Arc::clone(&c);
            async move {
                let n = c.fetch_add(1, Ordering::SeqCst);
                // Attempt 1 (n==1) is a 500 that resets the 529 run; attempts 0
                // and 2 are 529s.
                if n == 1 {
                    Ok(ok(500, "transient"))
                } else {
                    Ok(ok(529, "overloaded"))
                }
            }
        })
        .await;
        match r {
            Err(ApiError::Overloaded { repeated }) => assert!(repeated),
            other => panic!("expected Overloaded (budget exhausted), got {other:?}"),
        }
        assert_eq!(count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn gate_closed_means_no_fallback_even_on_repeated_529() {
        // allow_fallback = false (e.g. a non-opus model, or a subscriber): the
        // consecutive-529 counter never increments, so neither FallbackTriggered
        // nor the early Repeated-Overloaded branch fires. The loop simply
        // exhausts its budget; because the last attempt was a 529 it still
        // surfaces Overloaded { repeated: true } via the budget path.
        let count = Arc::new(AtomicU8::new(0));
        let c = Arc::clone(&count);
        let ctl = RetryControl {
            fallback_model: Some("claude-sonnet-4-6".into()),
            primary_model: "claude-sonnet-4-6".into(),
            allow_fallback: false, // gate closed
            is_external: true,
            is_sandbox: false,
            ..RetryControl::default()
        };
        let r = with_retry_ctl(3, &[1, 1, 1], &ctl, move |_| {
            let c = Arc::clone(&c);
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(ok(529, "overloaded"))
            }
        })
        .await;
        match r {
            Err(ApiError::Overloaded { repeated }) => assert!(repeated),
            other => panic!("expected Overloaded (gate closed → budget path), got {other:?}"),
        }
        assert_eq!(count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn sandbox_skips_the_no_fallback_terminal_branch() {
        // No fallback, gate open, but IS_SANDBOX set → the early terminal branch
        // is skipped (TS `!process.env.IS_SANDBOX`). The loop keeps retrying and
        // exhausts its budget, still surfacing Overloaded via last_overloaded.
        let count = Arc::new(AtomicU8::new(0));
        let c = Arc::clone(&count);
        let ctl = RetryControl {
            fallback_model: None,
            primary_model: "claude-opus-4-6".into(),
            allow_fallback: true,
            is_external: true,
            is_sandbox: true, // sandbox → skip the early terminal branch
            ..RetryControl::default()
        };
        let r = with_retry_ctl(4, &[1, 1, 1, 1], &ctl, move |_| {
            let c = Arc::clone(&c);
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(ok(529, "overloaded"))
            }
        })
        .await;
        match r {
            Err(ApiError::Overloaded { repeated }) => assert!(repeated),
            other => panic!("expected Overloaded, got {other:?}"),
        }
        // All 4 attempts ran (gate never short-circuited).
        assert_eq!(count.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn non_external_no_fallback_keeps_retrying() {
        // Gate open, no fallback, USER_TYPE != external → neither terminal
        // branch fires; the loop runs to budget exhaustion (Overloaded via the
        // last_overloaded path because the final attempt was a 529).
        let count = Arc::new(AtomicU8::new(0));
        let c = Arc::clone(&count);
        let ctl = RetryControl {
            fallback_model: None,
            primary_model: "claude-opus-4-6".into(),
            allow_fallback: true,
            is_external: false, // not external
            is_sandbox: false,
            ..RetryControl::default()
        };
        let r = with_retry_ctl(3, &[1, 1, 1], &ctl, move |_| {
            let c = Arc::clone(&c);
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(ok(529, "overloaded"))
            }
        })
        .await;
        match r {
            Err(ApiError::Overloaded { repeated }) => assert!(repeated),
            other => panic!("expected Overloaded, got {other:?}"),
        }
        assert_eq!(count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn default_ctl_reduces_to_budget_only_loop() {
        // RetryControl::default() (allow_fallback=false, no fallback) must behave
        // exactly like the legacy budget-only with_retry: 3×529 → Overloaded via
        // the budget path, never FallbackTriggered.
        let count = Arc::new(AtomicU8::new(0));
        let c = Arc::clone(&count);
        let r = with_retry_ctl(3, &[1, 1, 1], &RetryControl::default(), move |_| {
            let c = Arc::clone(&c);
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(ok(529, "overloaded"))
            }
        })
        .await;
        match r {
            Err(ApiError::Overloaded { repeated }) => assert!(repeated),
            other => panic!("expected Overloaded, got {other:?}"),
        }
        assert_eq!(count.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn max_529_retries_is_byte_locked() {
        // claude-code withRetry.ts:54 — const MAX_529_RETRIES = 3.
        assert_eq!(MAX_529_RETRIES, 3);
        assert_eq!(RetryControl::default().max_529_retries, 3);
    }
}

#[cfg(test)]
mod classify {
    //! Pure-function tests for `is_529` / `classify_retryable`, named so the
    //! cargo filter `retry::classify` matches them all.
    use super::*;
    use protocol::HttpResponse;

    fn resp(status: u16, body: &str) -> HttpResponse {
        HttpResponse {
            status,
            headers: Vec::new(),
            body: body.into(),
        }
    }

    #[test]
    fn is_529_true_on_status() {
        assert!(is_529(529, ""));
    }

    #[test]
    fn is_529_true_on_body_marker() {
        // Byte-identical substring to claude-code withRetry.ts:619.
        assert!(is_529(200, r#"{"type":"overloaded_error"}"#));
        assert!(is_529(503, r#"prefix {"type":"overloaded_error"} suffix"#));
    }

    #[test]
    fn is_529_false_otherwise() {
        assert!(!is_529(200, "ok"));
        assert!(!is_529(503, "internal error"));
        // Near-miss strings (different quoting / type) must not match.
        assert!(!is_529(200, r#"{"type": "overloaded_error"}"#));
        assert!(!is_529(200, r#"{"type":"other_error"}"#));
    }

    #[test]
    fn marker_constant_is_byte_locked() {
        assert_eq!(OVERLOADED_ERROR_BODY_MARKER, "\"type\":\"overloaded_error\"");
    }

    #[test]
    fn classify_529_status_is_retry_overloaded() {
        assert_eq!(
            classify_retryable(&resp(529, "")),
            RetryClass::Retry { overloaded: true }
        );
    }

    #[test]
    fn classify_overloaded_body_is_retry_overloaded_regardless_of_status() {
        for status in [200u16, 503, 500] {
            assert_eq!(
                classify_retryable(&resp(status, r#"{"type":"overloaded_error"}"#)),
                RetryClass::Retry { overloaded: true },
                "status {status} with overloaded body must be Retry/overloaded",
            );
        }
    }

    #[test]
    fn classify_408_409_are_retry_not_overloaded() {
        assert_eq!(
            classify_retryable(&resp(408, "")),
            RetryClass::Retry { overloaded: false }
        );
        assert_eq!(
            classify_retryable(&resp(409, "")),
            RetryClass::Retry { overloaded: false }
        );
    }

    #[test]
    fn classify_429_is_fallthrough() {
        assert_eq!(classify_retryable(&resp(429, "")), RetryClass::Fallthrough);
    }

    fn resp_with_header(status: u16, body: &str, header: (&str, &str)) -> HttpResponse {
        HttpResponse {
            status,
            headers: vec![(header.0.to_string(), header.1.to_string())],
            body: body.into(),
        }
    }

    #[test]
    fn classify_x_should_retry_false_is_terminal_even_for_5xx() {
        // The server's explicit "do not retry" overrides the normal 5xx-retry.
        assert_eq!(
            classify_retryable(&resp_with_header(503, "boom", ("x-should-retry", "false"))),
            RetryClass::Terminal,
        );
        // Header name match is case-insensitive; the directive applies to non-5xx too.
        assert_eq!(
            classify_retryable(&resp_with_header(408, "", ("X-Should-Retry", "false"))),
            RetryClass::Terminal,
        );
    }

    #[test]
    fn classify_x_should_retry_false_does_not_override_529() {
        // 529/overloaded short-circuits BEFORE the directive (matching TS order),
        // so a 529 still retries even with `x-should-retry: false`.
        assert_eq!(
            classify_retryable(&resp_with_header(529, "", ("x-should-retry", "false"))),
            RetryClass::Retry { overloaded: true },
        );
    }

    #[test]
    fn classify_x_should_retry_true_falls_through_to_status_ladder() {
        // The subscriber-gated `true` directive is not handled here; a `true`
        // value leaves the status-based classification unchanged.
        assert_eq!(
            classify_retryable(&resp_with_header(503, "boom", ("x-should-retry", "true"))),
            RetryClass::Retry { overloaded: false },
        );
        assert_eq!(
            classify_retryable(&resp_with_header(429, "", ("x-should-retry", "true"))),
            RetryClass::Fallthrough,
        );
    }

    #[test]
    fn classify_5xx_is_retry_not_overloaded() {
        for status in [500u16, 502, 503, 599] {
            assert_eq!(
                classify_retryable(&resp(status, "boom")),
                RetryClass::Retry { overloaded: false },
                "status {status}",
            );
        }
    }

    #[test]
    fn classify_terminal_for_other_4xx_and_3xx() {
        for status in [300u16, 301, 400, 401, 403, 404, 422] {
            assert_eq!(
                classify_retryable(&resp(status, "")),
                RetryClass::Terminal,
                "status {status} should be Terminal",
            );
        }
    }

    #[test]
    fn classify_400_overflow_is_adjust_and_retry() {
        let body =
            "input length and `max_tokens` exceed context limit: 188059 + 20000 > 200000";
        assert_eq!(
            classify_retryable(&resp(400, body)),
            RetryClass::AdjustAndRetry(Overflow {
                input_tokens: 188_059,
                max_tokens: 20_000,
                context_limit: 200_000,
            }),
        );
    }

    #[test]
    fn classify_400_without_overflow_marker_stays_terminal() {
        // A plain 400 (no overflow message) is NOT re-shrinkable → Terminal.
        assert_eq!(
            classify_retryable(&resp(400, "some other invalid_request_error")),
            RetryClass::Terminal,
        );
    }

    #[test]
    fn classify_overflow_message_on_non_400_status_is_not_adjust() {
        // The overflow message only routes to AdjustAndRetry on a real 400;
        // the same text on a 500 follows the normal 5xx retry path.
        let body =
            "input length and `max_tokens` exceed context limit: 188059 + 20000 > 200000";
        assert_eq!(
            classify_retryable(&resp(500, body)),
            RetryClass::Retry { overloaded: false },
        );
    }
}

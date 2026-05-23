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
use lingxi_protocol::HttpResponse;
use lingxi_traits::HttpError;

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
/// # Errors
/// Returns the first non-retryable error encountered, or
/// [`ApiError::RetryExhausted`] if every attempt failed with a retryable
/// status (5xx / transport error). 4xx other than 401 are non-retryable;
/// 401 is handled at the caller (`anthropic.rs::do_request_with_middleware`).
pub async fn with_retry<F, Fut>(
    attempts: u8,
    base_delays_ms: &[u64],
    mut f: F,
) -> Result<HttpResponse, ApiError>
where
    F: FnMut(u8) -> Fut,
    Fut: std::future::Future<Output = Result<HttpResponse, HttpError>>,
{
    let mut last_status: Option<u16> = None;
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
            Ok(resp) if (200..300).contains(&resp.status) => {
                return Ok(resp);
            }
            Ok(resp) if (500..600).contains(&resp.status) => {
                last_status = Some(resp.status);
                tracing::warn!(
                    target: "lingxi::api_client::retry",
                    attempt = attempt + 1,
                    status = resp.status,
                    "5xx response; will retry"
                );
                continue;
            }
            Ok(resp) => {
                // Non-retryable HTTP response (other 4xx, 3xx, etc.).
                // Surface as ApiError::Server for the caller to inspect.
                return Err(ApiError::Server {
                    status: resp.status,
                    body: resp.body,
                });
            }
            Err(HttpError::Timeout(_) | HttpError::Connection(_)) => {
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
    Err(ApiError::RetryExhausted { last_status })
}

#[cfg(test)]
mod with_retry_tests {
    use super::*;
    use lingxi_protocol::HttpResponse;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU8, Ordering};

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

//! Exponential-backoff retry driver over llm-client's error taxonomy.
//!
//! This module ports the cadence constants and jitter helper from
//! `api-client/src/retry.rs` and adds a new pure decision function
//! [`next_step`] that maps `(RetryState, RetryControl, LlmError)` to a
//! [`DriveStep`] — what the caller should do next after one failed attempt.
//!
//! The function is **synchronous and pure**: the caller is responsible for
//! actually sleeping (`tokio::time::sleep`) and re-executing the request.
//! This separation keeps the driver unit-testable without a Tokio runtime and
//! leaves all transport / UI emission to Plan 3 wiring.

#![forbid(unsafe_code)]

use rand::Rng;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Cadence constants — byte-locked against api-client/src/retry.rs
// ---------------------------------------------------------------------------

/// Default base delays in milliseconds before each retry attempt. **Locked
/// against spec §7**: changing these requires updating
/// `parity_messages_create.json`.
pub const DEFAULT_BASE_DELAYS_MS: &[u64] = &[500, 1_000, 2_000];

/// Default maximum number of retries. Byte-locked to claude-code
/// `withRetry.ts:52` (`const DEFAULT_MAX_RETRIES = 10`). Combined with the
/// loop bound `attempt <= maxRetries + 1` (`withRetry.ts:189`) this permits up
/// to 11 executions / 10 sleeps at the default. Overridable via
/// `CLAUDE_CODE_MAX_RETRIES` (see [`max_retries_from_env`]).
pub const DEFAULT_MAX_RETRIES: u32 = 10;

/// Resolve the configured max-retries from a raw `CLAUDE_CODE_MAX_RETRIES`
/// value. Mirrors claude-code `getDefaultMaxRetries` (`withRetry.ts:789-793`):
/// when the env var is present its `parseInt` is used, otherwise (absent or
/// unparseable) the default of [`DEFAULT_MAX_RETRIES`] applies.
#[must_use]
pub fn max_retries_from_env_value(v: Option<&str>) -> u32 {
    match v {
        Some(s) => s.trim().parse::<u32>().unwrap_or(DEFAULT_MAX_RETRIES),
        None => DEFAULT_MAX_RETRIES,
    }
}

/// Read `CLAUDE_CODE_MAX_RETRIES` from the process environment and resolve the
/// effective max-retries. Mirrors claude-code `getDefaultMaxRetries`
/// (`withRetry.ts:789-796`).
#[must_use]
pub fn max_retries_from_env() -> u32 {
    max_retries_from_env_value(std::env::var("CLAUDE_CODE_MAX_RETRIES").ok().as_deref())
}

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

// ---------------------------------------------------------------------------
// Driver types
// ---------------------------------------------------------------------------

/// What the driver decided after one failed attempt.
#[derive(Debug, Clone, PartialEq)]
pub enum DriveStep {
    /// Sleep `delay` then retry the same request.
    RetryAfter(Duration),
    /// Re-encode with this `max_tokens` then retry (529-independent).
    AdjustMaxTokens(u32),
    /// Switch to the fallback model then retry.
    Fallback {
        /// The model identifier to use for the fallback attempt.
        fallback_model: String,
    },
    /// Surface the error to the caller.
    Terminal,
}

/// Per-call mutable retry state (attempts, consecutive overloads).
#[derive(Debug, Default)]
pub struct RetryState {
    /// Number of budget-consuming retry attempts taken so far.
    pub attempt: u8,
    /// Count of consecutive `Overloaded` errors without an intervening
    /// non-overloaded outcome.
    pub consecutive_overloaded: u8,
}

/// Consecutive-529 / Opus-fallback policy threaded into [`next_step`].
///
/// Ports the consecutive-529 block of claude-code `withRetry.ts:326-365`. The
/// driver maintains a `consecutive_overloaded` counter inside [`RetryState`]
/// that increments on each `Overloaded` response and **resets to 0 on any
/// non-overloaded error**. When the counter reaches [`MAX_529_RETRIES`] *and*
/// [`Self::allow_fallback`] is set and a fallback model is configured, the
/// driver returns [`DriveStep::Fallback`].
///
/// `allow_fallback` is the **pre-computed** TS guard
/// `FALLBACK_FOR_ALL_PRIMARY_MODELS || (!isClaudeAISubscriber() &&
/// isNonCustomOpusModel(model))` (`withRetry.ts:331-332`) — the caller resolves
/// the env flag / subscriber state / Opus check once and hands the result in.
#[derive(Debug, Clone)]
pub struct RetryControl {
    /// Consecutive-529 threshold. Defaults to [`MAX_529_RETRIES`] (3).
    pub max_529_retries: u8,
    /// Fallback model to signal via [`DriveStep::Fallback`] once the
    /// threshold trips; `None` disables the fallback signal.
    pub fallback_model: Option<String>,
    /// The primary model in flight — carried for caller context / logging.
    pub primary_model: String,
    /// Pre-computed TS guard (`FALLBACK_FOR_ALL_PRIMARY_MODELS ||
    /// (!is_subscriber && is_non_custom_opus(primary_model))`). When `false`,
    /// the consecutive-529 gate never trips and the loop behaves as budget-only.
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
    /// so [`next_step`] reduces to budget-only retry. Used by callers that
    /// have not wired the consecutive-529 policy yet.
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

/// Pure retry decision: given the current state, control policy, error, and
/// thinking budget, decide what the caller should do next.
///
/// ## Mutation contract
///
/// `next_step` mutates `state` **before** returning for budget-consuming
/// retries: `state.attempt` is incremented when a `RetryAfter` is returned
/// (so the caller's sleep + re-execute path sees the updated count on the
/// next call). `AdjustMaxTokens` does **not** consume an attempt (mirrors
/// api-client `AdjustAndRetry`). `consecutive_overloaded` is incremented for
/// `Overloaded` errors and reset to 0 for any non-overloaded error class.
///
/// ## Decision table
///
/// 1. [`LlmError::Overloaded`] — check consecutive gate first (evaluated in order,
///    first match wins):
///    - `allow_fallback && consecutive >= max_529_retries && fallback_model.is_some()`
///      → [`DriveStep::Fallback`] (does not consume an attempt).
///    - `allow_fallback && consecutive >= max_529_retries && fallback_model.is_none()
///      && is_external && !is_sandbox`
///      → [`DriveStep::Terminal`] immediately — external non-sandbox callers with
///      no fallback configured are terminated at the threshold rather than
///      exhausting the full budget (mirrors api-client `withRetry.ts:354-359`).
///    - Budget remains → [`DriveStep::RetryAfter`] (jittered, consumes an attempt,
///      increments consecutive counter).
///    - Budget exhausted → [`DriveStep::Terminal`].
/// 2. [`LlmError::RateLimited`] — server wins:
///    - `retry_after: Some(d)` → [`DriveStep::RetryAfter(d)`] verbatim (no jitter, consumes an attempt).
///    - `retry_after: None` → [`DriveStep::RetryAfter`] (jittered, consumes an attempt).
/// 3. [`LlmError::ProviderInternal`] / [`LlmError::Transport`] — budget check:
///    - Budget remains → [`DriveStep::RetryAfter`] (jittered, consumes an attempt).
///    - Exhausted → [`DriveStep::Terminal`].
/// 4. [`LlmError::InvalidRequest`] — overflow check:
///    - Message parses as overflow AND `adjusted_max_tokens` yields `Some(n)`
///      → [`DriveStep::AdjustMaxTokens(n)`] (does **not** consume an attempt).
///    - Otherwise → [`DriveStep::Terminal`].
/// 5. All other errors → [`DriveStep::Terminal`].
/// 6. Budget exhaustion: once `state.attempt >= DEFAULT_MAX_RETRIES`, every
///    otherwise-retryable class → [`DriveStep::Terminal`].
pub fn next_step(
    state: &mut RetryState,
    ctl: &RetryControl,
    error: &llm_client::LlmError,
    thinking_budget: u32,
) -> DriveStep {
    use llm_client::LlmError;

    match error {
        LlmError::Overloaded => {
            // Increment consecutive counter (always, regardless of allow_fallback,
            // so the count is accurate when allow_fallback later becomes true).
            state.consecutive_overloaded = state.consecutive_overloaded.saturating_add(1);

            // Check the fallback / external-terminal gates BEFORE budget exhaustion
            // (mirrors api-client withRetry.ts:335-359).
            if ctl.allow_fallback && state.consecutive_overloaded >= ctl.max_529_retries {
                if let Some(fallback) = &ctl.fallback_model {
                    // api-client withRetry.ts:347 — signal the caller to re-issue
                    // against the fallback model.
                    return DriveStep::Fallback {
                        fallback_model: fallback.clone(),
                    };
                }
                // api-client withRetry.ts:354-359 — external, non-sandbox, no fallback
                // configured → terminate immediately rather than exhausting budget.
                if ctl.is_external && !ctl.is_sandbox {
                    return DriveStep::Terminal;
                }
                // Neither branch applies (internal or sandbox) → fall through to the
                // normal budget-driven retry path.
            }

            if u32::from(state.attempt) >= DEFAULT_MAX_RETRIES {
                return DriveStep::Terminal;
            }

            let base = base_delay_ms(state.attempt);
            state.attempt = state.attempt.saturating_add(1);
            DriveStep::RetryAfter(jittered_delay(base))
        }

        LlmError::RateLimited { retry_after, .. } => {
            // Reset the consecutive-overloaded counter: a rate-limit is not a 529.
            state.consecutive_overloaded = 0;

            if u32::from(state.attempt) >= DEFAULT_MAX_RETRIES {
                return DriveStep::Terminal;
            }

            let delay = match retry_after {
                Some(d) => *d,
                None => jittered_delay(base_delay_ms(state.attempt)),
            };
            state.attempt = state.attempt.saturating_add(1);
            DriveStep::RetryAfter(delay)
        }

        LlmError::ProviderInternal | LlmError::Transport { .. } => {
            // Reset the consecutive-overloaded counter.
            state.consecutive_overloaded = 0;

            if u32::from(state.attempt) >= DEFAULT_MAX_RETRIES {
                return DriveStep::Terminal;
            }

            let base = base_delay_ms(state.attempt);
            state.attempt = state.attempt.saturating_add(1);
            DriveStep::RetryAfter(jittered_delay(base))
        }

        LlmError::InvalidRequest { message } => {
            // Reset the consecutive-overloaded counter.
            state.consecutive_overloaded = 0;

            // Overflow check: parse the message and compute adjusted max_tokens.
            if let Some(overflow) = crate::model::overflow::parse_overflow_message(message) {
                if let Some(new_max) =
                    crate::model::overflow::adjusted_max_tokens(overflow, u64::from(thinking_budget))
                {
                    // AdjustMaxTokens does NOT consume a budget attempt.
                    return DriveStep::AdjustMaxTokens(new_max);
                }
            }

            DriveStep::Terminal
        }

        // All other LlmError variants are unconditionally terminal.
        LlmError::Authentication
        | LlmError::PermissionDenied
        | LlmError::ContextOverflow { .. }
        | LlmError::QuotaExceeded
        | LlmError::ModelUnavailable
        | LlmError::StreamInterrupted { .. }
        | LlmError::CostUnavailable { .. }
        | LlmError::UnsupportedCapability { .. } => {
            state.consecutive_overloaded = 0;
            DriveStep::Terminal
        }
    }
}

/// Select the base delay for attempt index `attempt` from the default table,
/// clamping to the last entry if the index is out of range.
fn base_delay_ms(attempt: u8) -> u64 {
    let idx = (attempt as usize).min(DEFAULT_BASE_DELAYS_MS.len() - 1);
    DEFAULT_BASE_DELAYS_MS[idx]
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod jittered_delay_tests {
    //! Cadence lock tests ported byte-exact from api-client/src/retry.rs.
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
    }

    /// claude-code `withRetry.ts:52` — `const DEFAULT_MAX_RETRIES = 10`.
    #[test]
    fn default_retry_budget_matches_claude_code() {
        assert_eq!(DEFAULT_MAX_RETRIES, 10);
    }

    /// claude-code `withRetry.ts:789-796` — `CLAUDE_CODE_MAX_RETRIES` overrides
    /// the default when present and parseable; falls back to 10 otherwise.
    #[test]
    fn claude_code_max_retries_env_overrides() {
        assert_eq!(max_retries_from_env_value(Some("5")), 5);
        // Absent → default.
        assert_eq!(max_retries_from_env_value(None), DEFAULT_MAX_RETRIES);
        // Unparseable → default (mirrors parseInt fallback semantics here).
        assert_eq!(max_retries_from_env_value(Some("notanint")), DEFAULT_MAX_RETRIES);
    }

    #[test]
    fn max_529_retries_is_byte_locked() {
        assert_eq!(MAX_529_RETRIES, 3);
        assert_eq!(RetryControl::default().max_529_retries, 3);
    }
}

#[cfg(test)]
mod next_step_tests {
    use super::*;
    use llm_client::{LlmError, RetryDecision, RetryPolicy};
    use std::time::Duration;

    /// `state.attempt` value at which the default budget is exhausted (==
    /// [`DEFAULT_MAX_RETRIES`], narrowed to the `u8` field type). 10 fits `u8`.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "DEFAULT_MAX_RETRIES is 10 and fits u8 trivially"
    )]
    const EXHAUSTED_ATTEMPT: u8 = DEFAULT_MAX_RETRIES as u8;

    fn ctl_default() -> RetryControl {
        RetryControl::default()
    }

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

    // --- Table point 1: Overloaded ---

    #[test]
    fn overloaded_below_threshold_retries_with_jitter() {
        let mut state = RetryState::default();
        let ctl = ctl_with_fallback();
        // First two overloads: counter < MAX_529_RETRIES (3), budget not exhausted.
        let step = next_step(&mut state, &ctl, &LlmError::Overloaded, 0);
        assert!(
            matches!(step, DriveStep::RetryAfter(_)),
            "first overload should retry, got {step:?}"
        );
        assert_eq!(state.consecutive_overloaded, 1);
        assert_eq!(state.attempt, 1);

        let step2 = next_step(&mut state, &ctl, &LlmError::Overloaded, 0);
        assert!(
            matches!(step2, DriveStep::RetryAfter(_)),
            "second overload should retry, got {step2:?}"
        );
        assert_eq!(state.consecutive_overloaded, 2);
        assert_eq!(state.attempt, 2);
    }

    #[test]
    fn overloaded_at_max_529_retries_with_fallback_triggers_fallback() {
        let mut state = RetryState::default();
        let ctl = ctl_with_fallback();
        // Drive 3 overloads: on the 3rd, consecutive >= MAX_529_RETRIES.
        next_step(&mut state, &ctl, &LlmError::Overloaded, 0); // consecutive=1
        next_step(&mut state, &ctl, &LlmError::Overloaded, 0); // consecutive=2
        let step = next_step(&mut state, &ctl, &LlmError::Overloaded, 0); // consecutive=3
        assert_eq!(
            step,
            DriveStep::Fallback {
                fallback_model: "claude-sonnet-4-6".to_string()
            },
            "third overload should trigger fallback"
        );
    }

    #[test]
    fn overloaded_consecutive_counter_resets_on_non_overloaded() {
        let mut state = RetryState::default();
        let ctl = ctl_with_fallback();
        // Two overloads then a transport error resets the counter.
        next_step(&mut state, &ctl, &LlmError::Overloaded, 0);
        next_step(&mut state, &ctl, &LlmError::Overloaded, 0);
        assert_eq!(state.consecutive_overloaded, 2);

        next_step(
            &mut state,
            &ctl,
            &LlmError::Transport {
                message: "net error".into(),
            },
            0,
        );
        assert_eq!(
            state.consecutive_overloaded, 0,
            "consecutive counter must reset on non-overloaded error"
        );
    }

    #[test]
    fn overloaded_budget_exhausted_is_terminal() {
        let mut state = RetryState {
            attempt: EXHAUSTED_ATTEMPT,
            consecutive_overloaded: 0,
        };
        let ctl = ctl_default();
        let step = next_step(&mut state, &ctl, &LlmError::Overloaded, 0);
        assert_eq!(
            step,
            DriveStep::Terminal,
            "budget exhausted overloaded must be terminal"
        );
    }

    #[test]
    fn overloaded_without_fallback_configured_retries_then_terminal() {
        // allow_fallback=true but no fallback model → no Fallback, just budget retry.
        let mut state = RetryState::default();
        let ctl = RetryControl {
            allow_fallback: true,
            fallback_model: None,
            primary_model: "claude-opus-4-6".into(),
            ..RetryControl::default()
        };
        // Internal (is_external=false) + no fallback model: the consecutive-529
        // gate never terminates early, so the loop is budget-driven. Drive the
        // full default budget (10 retries) then assert the 11th call is Terminal.
        for _ in 0..DEFAULT_MAX_RETRIES {
            let step = next_step(&mut state, &ctl, &LlmError::Overloaded, 0);
            assert!(
                matches!(step, DriveStep::RetryAfter(_)),
                "within budget overloaded should RetryAfter, got {step:?}"
            );
        }
        // Budget exhausted: the next call is terminal.
        let step = next_step(&mut state, &ctl, &LlmError::Overloaded, 0);
        assert_eq!(
            step,
            DriveStep::Terminal,
            "overloaded without fallback model should be terminal after budget"
        );
    }

    // --- External no-fallback terminal branch (api-client withRetry.ts:354-359) ---

    /// api-client gate: `allow_fallback && consecutive_529 >= max_529_retries
    ///   && fallback_model.is_none() && is_external && !is_sandbox`
    /// → Terminal immediately (does not wait for budget exhaustion).
    #[test]
    fn overloaded_external_without_fallback_terminates_at_threshold() {
        let mut state = RetryState::default();
        let ctl = RetryControl {
            allow_fallback: true,
            fallback_model: None,
            primary_model: "claude-opus-4-6".into(),
            is_external: true,
            is_sandbox: false,
            max_529_retries: MAX_529_RETRIES,
        };
        // Drive consecutive_overloaded up to max_529_retries.
        // Attempts 1 and 2: below threshold, should still retry.
        let s1 = next_step(&mut state, &ctl, &LlmError::Overloaded, 0);
        assert!(
            matches!(s1, DriveStep::RetryAfter(_)),
            "attempt 1 should be RetryAfter, got {s1:?}"
        );
        let s2 = next_step(&mut state, &ctl, &LlmError::Overloaded, 0);
        assert!(
            matches!(s2, DriveStep::RetryAfter(_)),
            "attempt 2 should be RetryAfter, got {s2:?}"
        );
        // Attempt 3: consecutive_overloaded reaches MAX_529_RETRIES (3).
        // External + no-sandbox + no-fallback → Terminal (before budget exhaustion).
        assert_eq!(
            state.attempt, 2,
            "should have used 2 budget slots (not budget exhausted)"
        );
        let s3 = next_step(&mut state, &ctl, &LlmError::Overloaded, 0);
        assert_eq!(
            s3,
            DriveStep::Terminal,
            "external no-fallback at threshold must be Terminal, got {s3:?}"
        );
    }

    #[test]
    fn overloaded_sandbox_external_keeps_retrying() {
        // is_sandbox=true disables the early terminal, even if external + no fallback.
        let mut state = RetryState::default();
        let ctl = RetryControl {
            allow_fallback: true,
            fallback_model: None,
            primary_model: "claude-opus-4-6".into(),
            is_external: true,
            is_sandbox: true,
            max_529_retries: MAX_529_RETRIES,
        };
        // Drive through the threshold — sandbox must NOT terminate early.
        next_step(&mut state, &ctl, &LlmError::Overloaded, 0);
        next_step(&mut state, &ctl, &LlmError::Overloaded, 0);
        let s3 = next_step(&mut state, &ctl, &LlmError::Overloaded, 0);
        // consecutive_overloaded == 3 == MAX_529_RETRIES, but is_sandbox → keep retrying.
        assert!(
            matches!(s3, DriveStep::RetryAfter(_)),
            "sandbox should keep retrying past threshold, got {s3:?}"
        );
    }

    #[test]
    fn overloaded_internal_without_fallback_keeps_retrying() {
        // is_external=false → the external terminal branch never fires.
        let mut state = RetryState::default();
        let ctl = RetryControl {
            allow_fallback: true,
            fallback_model: None,
            primary_model: "claude-opus-4-6".into(),
            is_external: false,
            is_sandbox: false,
            max_529_retries: MAX_529_RETRIES,
        };
        next_step(&mut state, &ctl, &LlmError::Overloaded, 0);
        next_step(&mut state, &ctl, &LlmError::Overloaded, 0);
        let s3 = next_step(&mut state, &ctl, &LlmError::Overloaded, 0);
        // consecutive_overloaded == 3, but is_external=false → keep retrying (budget-driven).
        assert!(
            matches!(s3, DriveStep::RetryAfter(_)),
            "internal (non-external) should keep retrying past threshold, got {s3:?}"
        );
    }

    // --- Table point 2: RateLimited ---

    #[test]
    fn rate_limited_server_delay_used_verbatim() {
        let mut state = RetryState::default();
        let ctl = ctl_default();
        let server_delay = Duration::from_secs(30);
        let step = next_step(
            &mut state,
            &ctl,
            &LlmError::RateLimited {
                retry_after: Some(server_delay),
                scope: None,
            },
            0,
        );
        assert_eq!(
            step,
            DriveStep::RetryAfter(server_delay),
            "server-provided retry_after must be used verbatim"
        );
        assert_eq!(state.attempt, 1);
    }

    #[test]
    fn rate_limited_no_server_delay_uses_jitter() {
        let mut state = RetryState::default();
        let ctl = ctl_default();
        let step = next_step(
            &mut state,
            &ctl,
            &LlmError::RateLimited {
                retry_after: None,
                scope: None,
            },
            0,
        );
        assert!(
            matches!(step, DriveStep::RetryAfter(_)),
            "rate-limited without server delay should retry with jitter, got {step:?}"
        );
        assert_eq!(state.attempt, 1);
    }

    #[test]
    fn rate_limited_resets_consecutive_overloaded() {
        let mut state = RetryState {
            attempt: 0,
            consecutive_overloaded: 2,
        };
        let ctl = ctl_default();
        next_step(
            &mut state,
            &ctl,
            &LlmError::RateLimited {
                retry_after: None,
                scope: None,
            },
            0,
        );
        assert_eq!(
            state.consecutive_overloaded, 0,
            "rate-limited must reset consecutive counter"
        );
    }

    // --- Table point 3: ProviderInternal / Transport ---

    #[test]
    fn provider_internal_retries_within_budget() {
        let mut state = RetryState::default();
        let ctl = ctl_default();
        let step = next_step(&mut state, &ctl, &LlmError::ProviderInternal, 0);
        assert!(
            matches!(step, DriveStep::RetryAfter(_)),
            "ProviderInternal should retry within budget, got {step:?}"
        );
        assert_eq!(state.attempt, 1);
    }

    #[test]
    fn transport_error_retries_within_budget() {
        let mut state = RetryState::default();
        let ctl = ctl_default();
        let step = next_step(
            &mut state,
            &ctl,
            &LlmError::Transport {
                message: "connection refused".into(),
            },
            0,
        );
        assert!(
            matches!(step, DriveStep::RetryAfter(_)),
            "Transport should retry within budget, got {step:?}"
        );
        assert_eq!(state.attempt, 1);
    }

    #[test]
    fn provider_internal_budget_exhausted_is_terminal() {
        let mut state = RetryState {
            attempt: EXHAUSTED_ATTEMPT,
            consecutive_overloaded: 0,
        };
        let ctl = ctl_default();
        let step = next_step(&mut state, &ctl, &LlmError::ProviderInternal, 0);
        assert_eq!(
            step,
            DriveStep::Terminal,
            "ProviderInternal with exhausted budget must be Terminal"
        );
    }

    // --- Table point 4: InvalidRequest / overflow ---

    #[test]
    fn invalid_request_overflow_message_adjusts_max_tokens() {
        let mut state = RetryState::default();
        let ctl = ctl_default();
        let msg = "input length and `max_tokens` exceed context limit: 188059 + 20000 > 200000";
        let step = next_step(
            &mut state,
            &ctl,
            &LlmError::InvalidRequest {
                message: msg.to_string(),
            },
            0,
        );
        // adjusted: available = 200000 - 188059 - 1000 = 10941
        assert_eq!(
            step,
            DriveStep::AdjustMaxTokens(10_941),
            "overflow message should yield AdjustMaxTokens"
        );
        // AdjustMaxTokens must NOT consume a budget attempt.
        assert_eq!(
            state.attempt, 0,
            "AdjustMaxTokens must not increment attempt"
        );
    }

    #[test]
    fn invalid_request_non_overflow_is_terminal() {
        let mut state = RetryState::default();
        let ctl = ctl_default();
        let step = next_step(
            &mut state,
            &ctl,
            &LlmError::InvalidRequest {
                message: "some other validation error".to_string(),
            },
            0,
        );
        assert_eq!(
            step,
            DriveStep::Terminal,
            "non-overflow InvalidRequest must be Terminal"
        );
    }

    #[test]
    fn invalid_request_overflow_but_no_room_is_terminal() {
        let mut state = RetryState::default();
        let ctl = ctl_default();
        // available = 200000 - 199000 - 1000 = 0 < 3000 → adjusted_max_tokens returns None.
        let msg = "input length and `max_tokens` exceed context limit: 199000 + 20000 > 200000";
        let step = next_step(
            &mut state,
            &ctl,
            &LlmError::InvalidRequest {
                message: msg.to_string(),
            },
            0,
        );
        assert_eq!(
            step,
            DriveStep::Terminal,
            "overflow with no room should be Terminal (not AdjustMaxTokens)"
        );
    }

    // --- Table point 5: always-terminal errors ---

    #[test]
    fn terminal_error_classes_are_terminal() {
        let errors = vec![
            LlmError::Authentication,
            LlmError::PermissionDenied,
            LlmError::ContextOverflow { token_gap: 0 },
            LlmError::QuotaExceeded,
            LlmError::ModelUnavailable,
            LlmError::StreamInterrupted {
                message: "eof".into(),
            },
            LlmError::CostUnavailable {
                message: "no price".into(),
            },
            LlmError::UnsupportedCapability {
                capability: "thinking".into(),
            },
        ];
        for error in &errors {
            let mut state = RetryState::default();
            let ctl = ctl_default();
            let step = next_step(&mut state, &ctl, error, 0);
            assert_eq!(
                step,
                DriveStep::Terminal,
                "{error:?} should be Terminal"
            );
        }
    }

    // --- Table point 6: budget exhaustion for retryable classes ---

    #[test]
    fn budget_exhaustion_makes_all_retryable_classes_terminal() {
        let retryable_errors = vec![
            LlmError::ProviderInternal,
            LlmError::Transport {
                message: "net".into(),
            },
            LlmError::RateLimited {
                retry_after: None,
                scope: None,
            },
            // Overloaded without fallback configured:
            LlmError::Overloaded,
        ];
        for error in &retryable_errors {
            let mut state = RetryState {
                attempt: EXHAUSTED_ATTEMPT,
                consecutive_overloaded: 0,
            };
            let ctl = ctl_default();
            let step = next_step(&mut state, &ctl, error, 0);
            assert_eq!(
                step,
                DriveStep::Terminal,
                "budget-exhausted {error:?} must be Terminal"
            );
        }
    }

    // --- Loop bound: initial + DEFAULT_MAX_RETRIES retries (claude-code withRetry.ts:189) ---

    /// claude-code `withRetry.ts:189` — `for (attempt = 1; attempt <= maxRetries + 1; attempt++)`
    /// at the default permits up to 11 executions / 10 sleeps. Modeled here as
    /// `next_step` returning `RetryAfter` for the first 10 invocations (each
    /// consuming a budget slot / sleep) and only becoming `Terminal` on the
    /// 11th invocation (the 11th execution would have no further retry).
    #[test]
    fn next_step_terminal_after_eleven_executions_at_default() {
        let mut state = RetryState::default();
        let ctl = ctl_default();
        // 10 retryable failures each yield RetryAfter (10 sleeps after the
        // initial execution = 11 total executions worth of attempts).
        for i in 0..DEFAULT_MAX_RETRIES {
            let step = next_step(&mut state, &ctl, &LlmError::ProviderInternal, 0);
            assert!(
                matches!(step, DriveStep::RetryAfter(_)),
                "invocation {i} (attempt before={}) should RetryAfter, got {step:?}",
                state.attempt
            );
        }
        assert_eq!(
            u32::from(state.attempt),
            DEFAULT_MAX_RETRIES,
            "after 10 RetryAfter steps the attempt counter equals the budget"
        );
        // 11th invocation: budget exhausted → Terminal.
        let final_step = next_step(&mut state, &ctl, &LlmError::ProviderInternal, 0);
        assert_eq!(
            final_step,
            DriveStep::Terminal,
            "the 11th execution attempt must be Terminal at the default budget"
        );
    }

    // --- Table point 8: cross-check with RetryPolicy ---

    /// Every error that `RetryPolicy::classify_error` calls `DoNotRetry` must
    /// yield `DriveStep::Terminal` from `next_step` (except the `AdjustMaxTokens`
    /// special case for `InvalidRequest`). Conversely, every error that
    /// `RetryPolicy` calls `Retry` must yield a non-terminal step when the
    /// budget is fresh.
    ///
    /// This test ensures the two classification layers cannot silently drift.
    #[test]
    fn retry_policy_and_next_step_agree_on_terminal_vs_retryable() {
        let policy = RetryPolicy;

        // Errors that RetryPolicy says DoNotRetry — must all be Terminal from
        // next_step (except InvalidRequest overflow, which is tested separately).
        let do_not_retry = vec![
            LlmError::Authentication,
            LlmError::PermissionDenied,
            LlmError::ContextOverflow { token_gap: 0 },
            LlmError::QuotaExceeded,
            LlmError::ModelUnavailable,
            LlmError::StreamInterrupted {
                message: "x".into(),
            },
            LlmError::CostUnavailable {
                message: "x".into(),
            },
            LlmError::UnsupportedCapability {
                capability: "x".into(),
            },
        ];
        for error in &do_not_retry {
            assert_eq!(
                policy.classify_error(error),
                RetryDecision::DoNotRetry,
                "RetryPolicy expected DoNotRetry for {error:?}"
            );
            let mut state = RetryState::default();
            let ctl = ctl_default();
            let step = next_step(&mut state, &ctl, error, 0);
            assert_eq!(
                step,
                DriveStep::Terminal,
                "next_step must be Terminal for {error:?} (DoNotRetry in RetryPolicy)"
            );
        }

        // Errors that RetryPolicy says Retry — must all be non-terminal from
        // next_step when budget is fresh.
        let do_retry = vec![
            LlmError::ProviderInternal,
            LlmError::Overloaded,
            LlmError::Transport {
                message: "err".into(),
            },
            LlmError::RateLimited {
                retry_after: None,
                scope: None,
            },
        ];
        for error in &do_retry {
            assert!(
                matches!(
                    policy.classify_error(error),
                    RetryDecision::Retry { .. }
                ),
                "RetryPolicy expected Retry for {error:?}"
            );
            let mut state = RetryState::default();
            let ctl = ctl_default();
            let step = next_step(&mut state, &ctl, error, 0);
            assert_ne!(
                step,
                DriveStep::Terminal,
                "next_step must be non-terminal for {error:?} (Retry in RetryPolicy) with fresh budget"
            );
        }

        // InvalidRequest: RetryPolicy says DoNotRetry; next_step may yield
        // AdjustMaxTokens (overflow) or Terminal (non-overflow). Both are
        // acceptable; the AdjustMaxTokens case is documented as the special case.
        let invalid_non_overflow = LlmError::InvalidRequest {
            message: "bad param".into(),
        };
        assert_eq!(
            policy.classify_error(&invalid_non_overflow),
            RetryDecision::DoNotRetry,
        );
        let mut state = RetryState::default();
        let step = next_step(&mut state, &ctl_default(), &invalid_non_overflow, 0);
        assert_eq!(
            step,
            DriveStep::Terminal,
            "non-overflow InvalidRequest must be Terminal (matches RetryPolicy DoNotRetry)"
        );
    }

    // --- Jitter uses DEFAULT_BASE_DELAYS_MS[min(attempt, len-1)] ---

    #[test]
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_lossless,
        reason = "ms-scale comparison; the values fit u64 trivially"
    )]
    fn jitter_uses_correct_base_delay_for_attempt() {
        // attempt=0 → base=500ms, attempt=1 → 1000ms, attempt=2+ → 2000ms
        // Check that delay is in the correct jittered range.
        for (attempt_idx, &expected_base) in DEFAULT_BASE_DELAYS_MS.iter().enumerate() {
            let lo = (expected_base as f64 * JITTER_LOW) as u64;
            let hi = (expected_base as f64 * JITTER_HIGH) as u64;
            // Sample several times to catch stochastic issues.
            for _ in 0..50 {
                // attempt_idx comes from enumerate() over a 3-element slice — fits u8.
                #[allow(clippy::cast_possible_truncation)]
                let attempt_u8 = attempt_idx as u8;
                let mut state = RetryState {
                    attempt: attempt_u8,
                    consecutive_overloaded: 0,
                };
                let ctl = ctl_default();
                let step = next_step(&mut state, &ctl, &LlmError::ProviderInternal, 0);
                if let DriveStep::RetryAfter(d) = step {
                    // as_millis() returns u128; the values are in [400, 2400] so truncation is safe.
                    #[allow(clippy::cast_possible_truncation)]
                    let ms = d.as_millis() as u64;
                    assert!(
                        ms >= lo && ms < hi,
                        "attempt={attempt_idx}: delay {ms}ms outside [{lo},{hi})"
                    );
                } else {
                    panic!("expected RetryAfter for attempt={attempt_idx}, got {step:?}");
                }
            }
        }
    }
}

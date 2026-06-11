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
//!
//! ## 429 subscriber gate (parity `withRetry.ts:767`)
//!
//! `RateLimited` (HTTP 429) is retried **only when** `!is_subscriber ||
//! is_enterprise`. A plain Claude.ai subscriber (non-enterprise) hits their
//! rate limit and must wait; retrying would just burn the budget and hammer
//! the server. Enterprise subscribers and API-key users do retry (same as
//! claude-code).
//!
//! ## `resolve_retry_control` (parity `api-client/src/anthropic.rs:1187-1211`)
//!
//! Computes the per-request [`RetryControl`] from env + subscriber state.
//! Injectable via [`ResolveRetryEnv`] for deterministic tests.
//! `CLAUDE_CODE_UNATTENDED_RETRY` / persistent-mode is Anthropic-internal-only —
//! not ported.

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
    /// Surface a "Repeated 529 Overloaded errors" error to the caller.
    ///
    /// Emitted when `allow_fallback && consecutive_overloaded >= max_529_retries
    /// && fallback_model.is_none() && is_external && !is_sandbox` — the external
    /// non-sandbox terminal branch (TS `withRetry.ts:354-362`). Distinct from
    /// [`Terminal`] so callers can produce the byte-locked
    /// `REPEATED_529_ERROR_MESSAGE` copy (`errors.ts:166`) without needing to
    /// re-inspect the [`RetryState`].
    RepeatedOverloaded,
}

/// Per-call mutable retry state (attempts, consecutive overloads).
#[derive(Debug, Default)]
pub struct RetryState {
    /// Number of budget-consuming retry attempts taken so far.
    pub attempt: u8,
    /// Count of consecutive `Overloaded` errors without an intervening
    /// non-overloaded outcome.
    pub consecutive_overloaded: u8,
    /// `true` when the configured credential is a Claude.ai OAuth subscriber.
    ///
    /// Gates the 429 retry: `!is_subscriber || is_enterprise` mirrors
    /// `withRetry.ts:767`. A subscriber-non-enterprise 429 is **terminal**;
    /// enterprise subscribers and API-key users retry.
    pub is_subscriber: bool,
    /// `true` when the subscriber is an enterprise account.
    ///
    /// Re-enables 429 retry for subscribers on enterprise plans
    /// (`withRetry.ts:767`).
    pub is_enterprise: bool,
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
    /// Maximum number of retries. Defaults to [`DEFAULT_MAX_RETRIES`] (10).
    ///
    /// Overridable via `CLAUDE_CODE_MAX_RETRIES` — wired by
    /// [`resolve_retry_control`] reading [`ResolveRetryEnv::max_retries`].
    /// Mirrors `withRetry.ts:789-796` (`getMaxRetries`).
    pub max_retries: u32,
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
            max_retries: DEFAULT_MAX_RETRIES,
            fallback_model: None,
            primary_model: String::new(),
            allow_fallback: false,
            is_external: false,
            is_sandbox: false,
        }
    }
}

// ---------------------------------------------------------------------------
// resolve_retry_control — ported from api-client/src/anthropic.rs:1187-1211
// ---------------------------------------------------------------------------

/// Injectable environment for [`resolve_retry_control`].
///
/// Mirrors the same `UserAgentEnv` injectable pattern (Task 3) so tests can
/// supply deterministic values without mutating `std::env`.  The
/// `from_process_env()` constructor reads the real process environment for
/// production callers.
///
/// **Not ported:** `CLAUDE_CODE_UNATTENDED_RETRY` / persistent-mode is
/// Anthropic-internal-only and has no effect in external builds.
#[derive(Debug, Clone, Default)]
pub struct ResolveRetryEnv {
    /// Raw value of `FALLBACK_FOR_ALL_PRIMARY_MODELS`. A non-empty string is
    /// truthy (mirrors JS `process.env.FALLBACK_FOR_ALL_PRIMARY_MODELS ||`).
    pub fallback_for_all: Option<String>,
    /// Raw value of `USER_TYPE`. `"external"` means `is_external = true`.
    pub user_type: Option<String>,
    /// Whether `IS_SANDBOX` is defined at all. Any defined value (including
    /// empty string) counts as sandboxed — mirrors TS `!!process.env.IS_SANDBOX`.
    pub is_sandbox_defined: bool,
    /// Raw value of `CLAUDE_CODE_MAX_RETRIES`. When `Some`, parsed as `u32`;
    /// absent or unparseable → [`DEFAULT_MAX_RETRIES`].
    /// Mirrors `withRetry.ts:789-796` (`getMaxRetries`).
    pub max_retries: Option<String>,
}

impl ResolveRetryEnv {
    /// Read the live process environment. For production callers.
    #[must_use]
    pub fn from_process_env() -> Self {
        Self {
            fallback_for_all: std::env::var("FALLBACK_FOR_ALL_PRIMARY_MODELS").ok(),
            user_type: std::env::var("USER_TYPE").ok(),
            is_sandbox_defined: std::env::var_os("IS_SANDBOX").is_some(),
            max_retries: std::env::var("CLAUDE_CODE_MAX_RETRIES").ok(),
        }
    }
}

/// Compute the per-request [`RetryControl`] from env + subscriber state.
///
/// Ports `api-client/src/anthropic.rs::resolve_retry_control` (`:1187-1211`)
/// re-typed off api-client. Accepts an injectable [`ResolveRetryEnv`] so
/// tests can supply deterministic values.
///
/// **Policy:**
/// - `allow_fallback` = `FALLBACK_FOR_ALL_PRIMARY_MODELS` truthy **or**
///   (`!is_subscriber` and `is_non_custom_opus(model)`)
/// - `is_external` = `USER_TYPE == "external"`
/// - `is_sandbox` = `IS_SANDBOX` env var is defined (any value)
/// - `max_retries` = `CLAUDE_CODE_MAX_RETRIES` parsed as `u32`, or
///   [`DEFAULT_MAX_RETRIES`] when absent/unparseable (mirrors
///   `withRetry.ts:789-796`)
///
/// **Not ported:** `CLAUDE_CODE_UNATTENDED_RETRY` / persistent-mode is
/// ant-only.
#[must_use]
pub fn resolve_retry_control(
    model: &str,
    fallback_model: Option<String>,
    is_subscriber: bool,
    env: &ResolveRetryEnv,
) -> RetryControl {
    // TS raw `process.env.FALLBACK_FOR_ALL_PRIMARY_MODELS ||` — truthy means a
    // non-empty string; an empty string is falsy in JS.
    let fallback_for_all = env
        .fallback_for_all
        .as_deref()
        .is_some_and(|v| !v.is_empty());
    let allow_fallback =
        fallback_for_all || (!is_subscriber && crate::model::fallback::is_non_custom_opus(model));
    let is_external = env.user_type.as_deref() == Some("external");
    // TS `!!process.env.IS_SANDBOX` — present (defined) is sandboxed.
    let is_sandbox = env.is_sandbox_defined;
    // `CLAUDE_CODE_MAX_RETRIES` → withRetry.ts:789-796 `getMaxRetries`.
    let max_retries = max_retries_from_env_value(env.max_retries.as_deref());
    RetryControl {
        fallback_model,
        primary_model: model.to_string(),
        allow_fallback,
        is_external,
        is_sandbox,
        max_retries,
        ..RetryControl::default()
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
/// 6. Budget exhaustion: once `state.attempt >= ctl.max_retries`, every
///    otherwise-retryable class → [`DriveStep::Terminal`].
///    `ctl.max_retries` defaults to [`DEFAULT_MAX_RETRIES`] and is overridden
///    by `CLAUDE_CODE_MAX_RETRIES` via [`resolve_retry_control`].
pub fn next_step(
    state: &mut RetryState,
    ctl: &RetryControl,
    error: &llm_client::LlmError,
    thinking_budget: u32,
) -> DriveStep {
    use llm_client::LlmError;

    match error {
        LlmError::Overloaded { .. } => {
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
                // api-client withRetry.ts:354-362 — external, non-sandbox, no fallback
                // configured → terminate immediately with the byte-locked
                // "Repeated 529 Overloaded errors" copy (errors.ts:166).
                if ctl.is_external && !ctl.is_sandbox {
                    return DriveStep::RepeatedOverloaded;
                }
                // Neither branch applies (internal or sandbox) → fall through to the
                // normal budget-driven retry path.
            }

            if u32::from(state.attempt) >= ctl.max_retries {
                return DriveStep::Terminal;
            }

            let base = base_delay_ms(state.attempt);
            state.attempt = state.attempt.saturating_add(1);
            DriveStep::RetryAfter(jittered_delay(base))
        }

        LlmError::RateLimited { retry_after, .. } => {
            // Reset the consecutive-overloaded counter: a rate-limit is not a 529.
            state.consecutive_overloaded = 0;

            // 429 subscriber gate (parity withRetry.ts:767):
            //   retry_429_allowed = !is_subscriber || is_enterprise
            // A plain Claude.ai subscriber (non-enterprise) must NOT retry 429s
            // — they've hit their usage limit and the server won't honour more
            // requests until the window resets. Enterprise + API-key users retry.
            if state.is_subscriber && !state.is_enterprise {
                return DriveStep::Terminal;
            }

            if u32::from(state.attempt) >= ctl.max_retries {
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

            if u32::from(state.attempt) >= ctl.max_retries {
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
        let step = next_step(&mut state, &ctl, &LlmError::Overloaded { repeated: false }, 0);
        assert!(
            matches!(step, DriveStep::RetryAfter(_)),
            "first overload should retry, got {step:?}"
        );
        assert_eq!(state.consecutive_overloaded, 1);
        assert_eq!(state.attempt, 1);

        let step2 = next_step(&mut state, &ctl, &LlmError::Overloaded { repeated: false }, 0);
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
        next_step(&mut state, &ctl, &LlmError::Overloaded { repeated: false }, 0); // consecutive=1
        next_step(&mut state, &ctl, &LlmError::Overloaded { repeated: false }, 0); // consecutive=2
        let step = next_step(&mut state, &ctl, &LlmError::Overloaded { repeated: false }, 0); // consecutive=3
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
        next_step(&mut state, &ctl, &LlmError::Overloaded { repeated: false }, 0);
        next_step(&mut state, &ctl, &LlmError::Overloaded { repeated: false }, 0);
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
            ..RetryState::default()
        };
        let ctl = ctl_default();
        let step = next_step(&mut state, &ctl, &LlmError::Overloaded { repeated: false }, 0);
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
            let step = next_step(&mut state, &ctl, &LlmError::Overloaded { repeated: false }, 0);
            assert!(
                matches!(step, DriveStep::RetryAfter(_)),
                "within budget overloaded should RetryAfter, got {step:?}"
            );
        }
        // Budget exhausted: the next call is terminal.
        let step = next_step(&mut state, &ctl, &LlmError::Overloaded { repeated: false }, 0);
        assert_eq!(
            step,
            DriveStep::Terminal,
            "overloaded without fallback model should be terminal after budget"
        );
    }

    // --- External no-fallback terminal branch (api-client withRetry.ts:354-359) ---

    /// api-client gate: `allow_fallback && consecutive_529 >= max_529_retries
    ///   && fallback_model.is_none() && is_external && !is_sandbox`
    /// → `RepeatedOverloaded` immediately (does not wait for budget exhaustion).
    ///
    /// The `RepeatedOverloaded` step is distinct from `Terminal` so the adapter
    /// can produce the byte-locked `"Repeated 529 Overloaded errors"` copy
    /// (`errors.ts:166`, TS `withRetry.ts:359-362`).
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
            max_retries: DEFAULT_MAX_RETRIES,
        };
        // Drive consecutive_overloaded up to max_529_retries.
        // Attempts 1 and 2: below threshold, should still retry.
        let s1 = next_step(&mut state, &ctl, &LlmError::Overloaded { repeated: false }, 0);
        assert!(
            matches!(s1, DriveStep::RetryAfter(_)),
            "attempt 1 should be RetryAfter, got {s1:?}"
        );
        let s2 = next_step(&mut state, &ctl, &LlmError::Overloaded { repeated: false }, 0);
        assert!(
            matches!(s2, DriveStep::RetryAfter(_)),
            "attempt 2 should be RetryAfter, got {s2:?}"
        );
        // Attempt 3: consecutive_overloaded reaches MAX_529_RETRIES (3).
        // External + no-sandbox + no-fallback → RepeatedOverloaded (before budget exhaustion).
        assert_eq!(
            state.attempt, 2,
            "should have used 2 budget slots (not budget exhausted)"
        );
        let s3 = next_step(&mut state, &ctl, &LlmError::Overloaded { repeated: false }, 0);
        assert_eq!(
            s3,
            DriveStep::RepeatedOverloaded,
            "external no-fallback at threshold must be RepeatedOverloaded, got {s3:?}"
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
            max_retries: DEFAULT_MAX_RETRIES,
        };
        // Drive through the threshold — sandbox must NOT terminate early.
        next_step(&mut state, &ctl, &LlmError::Overloaded { repeated: false }, 0);
        next_step(&mut state, &ctl, &LlmError::Overloaded { repeated: false }, 0);
        let s3 = next_step(&mut state, &ctl, &LlmError::Overloaded { repeated: false }, 0);
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
            max_retries: DEFAULT_MAX_RETRIES,
        };
        next_step(&mut state, &ctl, &LlmError::Overloaded { repeated: false }, 0);
        next_step(&mut state, &ctl, &LlmError::Overloaded { repeated: false }, 0);
        let s3 = next_step(&mut state, &ctl, &LlmError::Overloaded { repeated: false }, 0);
        // consecutive_overloaded == 3, but is_external=false → keep retrying (budget-driven).
        assert!(
            matches!(s3, DriveStep::RetryAfter(_)),
            "internal (non-external) should keep retrying past threshold, got {s3:?}"
        );
    }

    // --- Table point 2: RateLimited + subscriber gate (withRetry.ts:767) ---

    /// withRetry.ts:767: subscriber non-enterprise 429 → terminal (no retry).
    #[test]
    fn subscriber_429_is_terminal() {
        let mut state = RetryState {
            is_subscriber: true,
            is_enterprise: false,
            ..RetryState::default()
        };
        let ctl = ctl_default();
        let step = next_step(
            &mut state,
            &ctl,
            &LlmError::RateLimited {
                retry_after: Some(Duration::from_secs(5)),
                scope: None,
            },
            0,
        );
        assert_eq!(
            step,
            DriveStep::Terminal,
            "subscriber non-enterprise 429 must be terminal (withRetry.ts:767)"
        );
        // No budget should be consumed (terminal path).
        assert_eq!(
            state.attempt, 0,
            "terminal 429 must not consume a budget slot"
        );
    }

    /// withRetry.ts:767: enterprise subscriber 429 → retries (enterprise re-enables).
    #[test]
    fn enterprise_subscriber_429_retries() {
        let mut state = RetryState {
            is_subscriber: true,
            is_enterprise: true,
            ..RetryState::default()
        };
        let ctl = ctl_default();
        let server_delay = Duration::from_secs(10);
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
            "enterprise subscriber 429 must retry (withRetry.ts:767)"
        );
        assert_eq!(state.attempt, 1);
    }

    /// withRetry.ts:767: API-key user (non-subscriber) 429 → retries.
    #[test]
    fn api_key_user_429_retries() {
        let mut state = RetryState {
            is_subscriber: false,
            is_enterprise: false,
            ..RetryState::default()
        };
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
            "API-key (non-subscriber) 429 must retry, got {step:?}"
        );
        assert_eq!(state.attempt, 1);
    }

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
            ..RetryState::default()
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
            ..RetryState::default()
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
            LlmError::Overloaded { repeated: false },
        ];
        for error in &retryable_errors {
            let mut state = RetryState {
                attempt: EXHAUSTED_ATTEMPT,
                consecutive_overloaded: 0,
                ..RetryState::default()
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
            LlmError::Overloaded { repeated: false },
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
                    ..RetryState::default()
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

#[cfg(test)]
mod resolve_retry_control_tests {
    //! Env-matrix tests mirroring api-client's `resolve_retry_control` tests.
    use super::*;
    use llm_client::LlmError;

    fn env(fallback_for_all: Option<&str>, user_type: Option<&str>, is_sandbox: bool) -> ResolveRetryEnv {
        ResolveRetryEnv {
            fallback_for_all: fallback_for_all.map(str::to_string),
            user_type: user_type.map(str::to_string),
            is_sandbox_defined: is_sandbox,
            max_retries: None,
        }
    }

    const OPUS_MODEL: &str = "claude-opus-4-6";
    const SONNET_MODEL: &str = "claude-sonnet-4-20250514";

    // --- allow_fallback ---

    /// `FALLBACK_FOR_ALL_PRIMARY_MODELS` non-empty → `allow_fallback = true`
    /// regardless of model or subscriber status.
    #[test]
    fn fallback_for_all_truthy_enables_allow_fallback() {
        let ctl = resolve_retry_control(
            SONNET_MODEL,
            None,
            true, // is_subscriber
            &env(Some("1"), None, false),
        );
        assert!(ctl.allow_fallback, "non-empty FALLBACK_FOR_ALL_PRIMARY_MODELS must set allow_fallback");
    }

    /// `FALLBACK_FOR_ALL_PRIMARY_MODELS` empty string is falsy (JS semantics).
    #[test]
    fn fallback_for_all_empty_string_is_falsy() {
        let ctl = resolve_retry_control(
            SONNET_MODEL,
            None,
            false,
            &env(Some(""), None, false),
        );
        assert!(
            !ctl.allow_fallback,
            "empty FALLBACK_FOR_ALL_PRIMARY_MODELS must be falsy"
        );
    }

    /// Non-subscriber + Opus model → `allow_fallback = true`.
    #[test]
    fn non_subscriber_opus_allows_fallback() {
        let ctl = resolve_retry_control(
            OPUS_MODEL,
            Some("claude-sonnet-4-6".into()),
            false, // not subscriber
            &env(None, None, false),
        );
        assert!(ctl.allow_fallback, "non-subscriber + opus → allow_fallback");
    }

    /// Subscriber + Opus model → `allow_fallback = false` (unless `FALLBACK_FOR_ALL`).
    #[test]
    fn subscriber_opus_no_fallback_unless_env() {
        let ctl = resolve_retry_control(
            OPUS_MODEL,
            Some("claude-sonnet-4-6".into()),
            true, // subscriber
            &env(None, None, false),
        );
        assert!(!ctl.allow_fallback, "subscriber + opus must NOT set allow_fallback unless FALLBACK_FOR_ALL");
    }

    /// Non-subscriber + non-Opus model → `allow_fallback = false`.
    #[test]
    fn non_subscriber_non_opus_no_fallback() {
        let ctl = resolve_retry_control(
            SONNET_MODEL,
            None,
            false,
            &env(None, None, false),
        );
        assert!(!ctl.allow_fallback, "non-subscriber + non-opus must NOT allow fallback");
    }

    // --- is_external ---

    #[test]
    fn user_type_external_sets_is_external() {
        let ctl = resolve_retry_control(SONNET_MODEL, None, false, &env(None, Some("external"), false));
        assert!(ctl.is_external);
    }

    #[test]
    fn user_type_non_external_clears_is_external() {
        let ctl = resolve_retry_control(SONNET_MODEL, None, false, &env(None, Some("internal"), false));
        assert!(!ctl.is_external);
    }

    #[test]
    fn user_type_absent_clears_is_external() {
        let ctl = resolve_retry_control(SONNET_MODEL, None, false, &env(None, None, false));
        assert!(!ctl.is_external);
    }

    // --- is_sandbox ---

    #[test]
    fn is_sandbox_defined_sets_is_sandbox() {
        let ctl = resolve_retry_control(SONNET_MODEL, None, false, &env(None, None, true));
        assert!(ctl.is_sandbox);
    }

    #[test]
    fn is_sandbox_absent_clears_is_sandbox() {
        let ctl = resolve_retry_control(SONNET_MODEL, None, false, &env(None, None, false));
        assert!(!ctl.is_sandbox);
    }

    // --- fallback_model is threaded through ---

    #[test]
    fn fallback_model_is_threaded_through() {
        let ctl = resolve_retry_control(
            SONNET_MODEL,
            Some("claude-sonnet-4-6".into()),
            false,
            &env(None, None, false),
        );
        assert_eq!(ctl.fallback_model.as_deref(), Some("claude-sonnet-4-6"));
        assert_eq!(ctl.primary_model, SONNET_MODEL);
    }

    // --- from_process_env smoke test (should not panic) ---

    #[test]
    fn from_process_env_does_not_panic() {
        let _ = ResolveRetryEnv::from_process_env();
    }

    // --- Fix 1: CLAUDE_CODE_MAX_RETRIES drives next_step via resolve_retry_control ---

    /// `CLAUDE_CODE_MAX_RETRIES=2` → `ctl.max_retries = 2` → `next_step` returns
    /// `Terminal` after 3 executions (2 sleeps), not 11.
    ///
    /// Mirrors `withRetry.ts:789-796` `getMaxRetries → options.maxRetries ??
    /// CLAUDE_CODE_MAX_RETRIES ?? 10`.
    #[test]
    fn claude_code_max_retries_env_drives_next_step() {
        // Inject max_retries="2" via ResolveRetryEnv (avoids mutating std::env).
        let env = ResolveRetryEnv {
            max_retries: Some("2".to_string()),
            ..ResolveRetryEnv::default()
        };
        let ctl = resolve_retry_control(SONNET_MODEL, None, false, &env);
        assert_eq!(ctl.max_retries, 2, "ctl.max_retries must be 2");

        let mut state = RetryState::default();
        // First 2 calls → RetryAfter (2 sleeps).
        for i in 0..2u32 {
            let step = next_step(&mut state, &ctl, &LlmError::ProviderInternal, 0);
            assert!(
                matches!(step, DriveStep::RetryAfter(_)),
                "call {i}: expected RetryAfter, got {step:?}"
            );
        }
        // 3rd call → budget exhausted (attempt=2 >= max_retries=2) → Terminal.
        let final_step = next_step(&mut state, &ctl, &LlmError::ProviderInternal, 0);
        assert_eq!(
            final_step,
            DriveStep::Terminal,
            "3rd call must be Terminal (CLAUDE_CODE_MAX_RETRIES=2)"
        );
    }

    /// Absent or unparseable `CLAUDE_CODE_MAX_RETRIES` falls back to `DEFAULT_MAX_RETRIES`.
    #[test]
    fn claude_code_max_retries_absent_uses_default() {
        let env = ResolveRetryEnv::default(); // max_retries: None
        let ctl = resolve_retry_control(SONNET_MODEL, None, false, &env);
        assert_eq!(
            ctl.max_retries, DEFAULT_MAX_RETRIES,
            "absent CLAUDE_CODE_MAX_RETRIES must default to {DEFAULT_MAX_RETRIES}"
        );
    }

    /// Unparseable value falls back to `DEFAULT_MAX_RETRIES` (mirrors parseInt semantics).
    #[test]
    fn claude_code_max_retries_unparseable_uses_default() {
        let env = ResolveRetryEnv {
            max_retries: Some("notanumber".to_string()),
            ..ResolveRetryEnv::default()
        };
        let ctl = resolve_retry_control(SONNET_MODEL, None, false, &env);
        assert_eq!(
            ctl.max_retries, DEFAULT_MAX_RETRIES,
            "unparseable CLAUDE_CODE_MAX_RETRIES must default to {DEFAULT_MAX_RETRIES}"
        );
    }
}

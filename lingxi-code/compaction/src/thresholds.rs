//! Verified constants from the claude-code reference (see spec §13.2), plus the
//! pure threshold math and token-warning state machine ported from
//! `src/services/compact/autoCompact.ts:30-158`
//! (`getEffectiveContextWindowSize`, `getAutoCompactThreshold`,
//! `calculateTokenWarningState`, `isAutoCompactEnabled`).
//!
//! ## Divergences from TS
//!
//! - `is_auto_compact_enabled` takes the config flag as a `bool` parameter
//!   rather than calling `getGlobalConfig().autoCompactEnabled`. The
//!   `compaction` crate does not depend on a config crate, so the caller reads
//!   `GlobalConfig` and passes the resolved `auto_compact_enabled` flag. The two
//!   env gates (`DISABLE_COMPACT`, `DISABLE_AUTO_COMPACT`) are read here.
//! - `calculate_token_warning_state` takes `auto_compact_enabled: bool` for the
//!   same reason (TS calls `isAutoCompactEnabled()` twice internally).

use crate::context_window::{context_window_for_model, max_output_tokens_for_model};

/// Buffer tokens before the autocompact threshold kicks in.
pub const AUTOCOMPACT_BUFFER_TOKENS: u64 = 13_000;
/// Buffer tokens before warning-level threshold.
pub const WARNING_THRESHOLD_BUFFER_TOKENS: u64 = 20_000;
/// Buffer tokens before error-level threshold.
pub const ERROR_THRESHOLD_BUFFER_TOKENS: u64 = 20_000;
/// Buffer tokens kept available after a manual compact request.
pub const MANUAL_COMPACT_BUFFER_TOKENS: u64 = 3_000;
/// Maximum output tokens budgeted for the autocompact summary call.
pub const MAX_OUTPUT_TOKENS_FOR_SUMMARY: u64 = 20_000;
/// Maximum consecutive autocompact failures before the circuit breaker trips.
///
/// claude-code v2.1.183 `jho = 3` (`bin/claude.exe` offset 203009353).
pub const MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES: u32 = 3;
/// Turns-since-previous-compact ceiling below which a fresh compact counts as a
/// "rapid refill" (the context refilled to the limit within this many turns).
///
/// claude-code v2.1.183 `Who = 3` (`bin/claude.exe` offset 203009353).
pub const RAPID_REFILL_TURN_WINDOW: u32 = 3;
/// Consecutive rapid-refill count at which the thrashing breaker trips.
///
/// claude-code v2.1.183 `f6n = 3` (`bin/claude.exe` offset 203009353).
pub const MAX_CONSECUTIVE_RAPID_REFILLS: u32 = 3;

/// Byte-exact thrashing message surfaced (on the reactive PTL path) when the
/// rapid-refill breaker trips.
///
/// 1:1 with claude-code v2.1.183 `Rho` (`bin/claude.exe` offset 203009521).
/// `${Who}` / `${f6n}` are both `3` (the constants are interpolated at module
/// init), so the literal carries the resolved `3`s.
pub const RAPID_REFILL_THRASHING_MESSAGE: &str = "Autocompact is thrashing: the context refilled to the limit within 3 turns of the previous compact, 3 times in a row. A file being read or a tool output is likely too large for the context window. Try reading in smaller chunks, or use /clear to start fresh.";
/// Maximum number of recent files restored into the post-compact prompt.
pub const POST_COMPACT_MAX_FILES_TO_RESTORE: usize = 5;
/// Total token budget shared across post-compact file restoration.
pub const POST_COMPACT_TOKEN_BUDGET: u64 = 50_000;
/// Per-file token budget for post-compact file restoration.
pub const POST_COMPACT_MAX_TOKENS_PER_FILE: u64 = 5_000;
/// Per-skill token budget for post-compact skill restoration.
pub const POST_COMPACT_MAX_TOKENS_PER_SKILL: u64 = 5_000;
/// Total token budget shared across post-compact skill restoration.
pub const POST_COMPACT_SKILLS_TOKEN_BUDGET: u64 = 25_000;
/// Maximum prompt-too-long retry attempts before giving up.
pub const MAX_PTL_RETRIES: u32 = 3;
/// Maximum streaming retries for the compaction summary call.
pub const MAX_COMPACT_STREAMING_RETRIES: u32 = 2;

/// Which compaction layer was applied during an iteration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CompactionLayer {
    /// Drop oldest messages (cheap, no LLM).
    Snip,
    /// Time-based clearing of large tool results.
    Microcompact,
    /// Cached microcompact path (wired in a later plan).
    CachedMicrocompact,
    /// Aggressive context-collapse (wired in a later plan).
    ContextCollapse,
    /// LLM-driven summarization.
    Autocompact,
    /// Partial autocompact path.
    PartialAutocompact,
}

/// Why compaction was triggered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CompactionReason {
    /// Approaching the model's context window.
    TokenLimit,
    /// The user explicitly requested a compact.
    ManualRequest,
    /// Server reported the prompt was too long.
    PromptTooLong,
    /// Microcompact warned that recent results were too large.
    MicrocompactWarn,
}

/// Per-agent tracking state used to coordinate autocompact across iterations.
///
/// Mirrors the `autoCompactTracking` object claude-code threads through
/// `autoCompactIfNeeded` (`bin/claude.exe`, the `{compacted, turnId,
/// turnCounter, consecutiveFailures, consecutiveRapidRefills}` shape set at
/// offset 202919683).
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct AutoCompactTrackingState {
    /// Whether autocompact has run (`compacted`). Set `true` when a compact
    /// ran; consulted by the rapid-refill breaker and the per-turn
    /// `turnCounter++` increment.
    pub compacted: bool,
    /// Turns since the previous compact (`turnCounter`): reset to `0` when a
    /// compact runs, incremented each turn thereafter while `compacted`.
    pub turn_counter: u32,
    /// Identifier of the turn that last compacted (`turnId`).
    pub turn_id: String,
    /// How many autocompact attempts have failed in a row
    /// (`consecutiveFailures`).
    pub consecutive_failures: u32,
    /// How many compacts in a row each occurred within
    /// [`RAPID_REFILL_TURN_WINDOW`] turns of the previous one
    /// (`consecutiveRapidRefills`). Drives the thrashing breaker.
    pub consecutive_rapid_refills: u32,
}

/// The rapid-refill (thrashing) count for `state`: how many consecutive
/// compacts have each occurred within [`RAPID_REFILL_TURN_WINDOW`] turns of the
/// previous one.
///
/// 1:1 with `kho` (`bin/claude.exe` offset 203004820):
/// ```text
/// function kho(e){
///   return e?.compacted===!0 && e.turnCounter<Who
///     ? (e?.consecutiveRapidRefills ?? 0) + 1
///     : 0
/// }
/// ```
/// When the previous compact ran (`compacted`) AND the context refilled within
/// `Who` turns (`turn_counter < RAPID_REFILL_TURN_WINDOW`), the running rapid-
/// refill count is incremented; otherwise it resets to `0`. The thrashing
/// breaker trips when this reaches [`MAX_CONSECUTIVE_RAPID_REFILLS`].
#[must_use]
pub fn rapid_refill_count(state: &AutoCompactTrackingState) -> u32 {
    if state.compacted && state.turn_counter < RAPID_REFILL_TURN_WINDOW {
        state.consecutive_rapid_refills.saturating_add(1)
    } else {
        0
    }
}

/// Returns the context window size minus the max output tokens reserved for the
/// compaction summary.
///
/// Mirrors `getEffectiveContextWindowSize` (`autoCompact.ts:33-49`):
/// `context_window − min(max_output_tokens, MAX_OUTPUT_TOKENS_FOR_SUMMARY)`,
/// honoring the `LINGXI_AUTO_COMPACT_WINDOW` env clamp (a positive integer
/// caps the context window via `min`).
#[must_use]
pub fn effective_context_window_size(model: &str, betas: &[String]) -> u64 {
    let reserved_tokens_for_summary =
        max_output_tokens_for_model(model).min(MAX_OUTPUT_TOKENS_FOR_SUMMARY);
    let mut context_window = context_window_for_model(model, betas);

    if let Ok(raw) = std::env::var("LINGXI_AUTO_COMPACT_WINDOW") {
        if let Some(parsed) = parse_positive_u64(&raw) {
            context_window = context_window.min(parsed);
        }
    }

    context_window.saturating_sub(reserved_tokens_for_summary)
}

/// Returns the token count at which autocompact should fire.
///
/// Mirrors `getAutoCompactThreshold` (`autoCompact.ts:72-91`):
/// `effective − AUTOCOMPACT_BUFFER_TOKENS`, honoring the
/// `LINGXI_AUTOCOMPACT_PCT_OVERRIDE` env override (a float in `(0, 100]` yields
/// `floor(effective × pct/100)`, then `min`'d with the buffer-based threshold).
#[must_use]
pub fn auto_compact_threshold(model: &str, betas: &[String]) -> u64 {
    let effective_context_window = effective_context_window_size(model, betas);
    let autocompact_threshold = effective_context_window.saturating_sub(AUTOCOMPACT_BUFFER_TOKENS);

    if let Ok(raw) = std::env::var("LINGXI_AUTOCOMPACT_PCT_OVERRIDE") {
        if let Ok(parsed) = raw.trim().parse::<f64>() {
            if parsed.is_finite() && parsed > 0.0 && parsed <= 100.0 {
                // Math.floor(effective * (pct / 100)).
                #[allow(
                    clippy::cast_precision_loss,
                    clippy::cast_sign_loss,
                    clippy::cast_possible_truncation
                )]
                let percentage_threshold =
                    (effective_context_window as f64 * (parsed / 100.0)).floor() as u64;
                return percentage_threshold.min(autocompact_threshold);
            }
        }
    }

    autocompact_threshold
}

/// Snapshot of where the current token usage sits relative to every threshold.
///
/// Mirrors the object returned by `calculateTokenWarningState`
/// (`autoCompact.ts:93-145`).
// Mirrors the flat object returned by TS `calculateTokenWarningState`: four
// independent threshold-crossing flags. Collapsing them into enums would diverge
// from the byte-faithful field set and the serde wire shape, so the bool flags
// are intentional here.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TokenWarningState {
    /// Percentage of the threshold still available, clamped to `>= 0`
    /// (`Math.round` parity).
    pub percent_left: u8,
    /// `token_usage >= threshold − WARNING_THRESHOLD_BUFFER_TOKENS`.
    pub is_above_warning_threshold: bool,
    /// `token_usage >= threshold − ERROR_THRESHOLD_BUFFER_TOKENS`.
    pub is_above_error_threshold: bool,
    /// Autocompact is enabled AND `token_usage >= auto_compact_threshold`.
    pub is_above_auto_compact_threshold: bool,
    /// `token_usage >= blocking_limit` (the hard manual-compact ceiling).
    pub is_at_blocking_limit: bool,
}

/// Computes the [`TokenWarningState`] for `token_usage` against `model`.
///
/// Mirrors `calculateTokenWarningState` (`autoCompact.ts:93-145`). The
/// `auto_compact_enabled` flag is supplied by the caller (see module docs);
/// `LINGXI_BLOCKING_LIMIT_OVERRIDE` is honored here.
#[must_use]
pub fn calculate_token_warning_state(
    token_usage: u64,
    model: &str,
    betas: &[String],
    auto_compact_enabled: bool,
) -> TokenWarningState {
    let autocompact_threshold = auto_compact_threshold(model, betas);
    let threshold = if auto_compact_enabled {
        autocompact_threshold
    } else {
        effective_context_window_size(model, betas)
    };

    // Math.max(0, Math.round(((threshold - tokenUsage) / threshold) * 100)).
    let percent_left = percent_left_of(threshold, token_usage);

    let warning_threshold = threshold.saturating_sub(WARNING_THRESHOLD_BUFFER_TOKENS);
    let error_threshold = threshold.saturating_sub(ERROR_THRESHOLD_BUFFER_TOKENS);

    let is_above_warning_threshold = token_usage >= warning_threshold;
    let is_above_error_threshold = token_usage >= error_threshold;

    let is_above_auto_compact_threshold =
        auto_compact_enabled && token_usage >= autocompact_threshold;

    let actual_context_window = effective_context_window_size(model, betas);
    let default_blocking_limit = actual_context_window.saturating_sub(MANUAL_COMPACT_BUFFER_TOKENS);

    // Allow override for testing (positive integer wins, else the default).
    let blocking_limit = std::env::var("LINGXI_BLOCKING_LIMIT_OVERRIDE")
        .ok()
        .and_then(|raw| parse_positive_u64(&raw))
        .unwrap_or(default_blocking_limit);

    let is_at_blocking_limit = token_usage >= blocking_limit;

    TokenWarningState {
        percent_left,
        is_above_warning_threshold,
        is_above_error_threshold,
        is_above_auto_compact_threshold,
        is_at_blocking_limit,
    }
}

/// Whether autocompact is enabled, honoring the two env kill-switches and the
/// caller-supplied config flag.
///
/// Mirrors `isAutoCompactEnabled` (`autoCompact.ts:147-158`):
/// `DISABLE_COMPACT` and `DISABLE_AUTO_COMPACT` (truthy) force `false`;
/// otherwise the result is `config_auto_compact_enabled`. The config flag is a
/// parameter because the `compaction` crate has no config-crate dependency (see
/// module docs).
#[must_use]
pub fn is_auto_compact_enabled(config_auto_compact_enabled: bool) -> bool {
    if env_truthy("DISABLE_COMPACT") {
        return false;
    }
    // Allow disabling just auto-compact (keeps manual /compact working).
    if env_truthy("DISABLE_AUTO_COMPACT") {
        return false;
    }
    config_auto_compact_enabled
}

/// `Math.max(0, Math.round(((threshold - usage) / threshold) * 100))`, clamped
/// to `u8`. Returns `0` when `threshold == 0` (avoids div-by-zero; TS would
/// yield `NaN → 0` via `Math.max(0, …)` semantics for our usage).
fn percent_left_of(threshold: u64, usage: u64) -> u8 {
    if threshold == 0 {
        return 0;
    }
    if usage >= threshold {
        return 0;
    }
    #[allow(clippy::cast_precision_loss)]
    let ratio = (threshold - usage) as f64 / threshold as f64;
    // JS Math.round: round half away from zero for positive values → (x + 0.5).floor().
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let rounded = (ratio * 100.0 + 0.5).floor() as i64;
    // `clamp(0, 100)` guarantees the value fits in `u8`, so `try_from` never
    // fails; this avoids the (false-positive) sign-loss cast.
    u8::try_from(rounded.clamp(0, 100)).unwrap_or(0)
}

/// Read an env var and apply `isEnvTruthy` ([`traits::env::is_env_truthy`])
/// semantics (`1` / `true` / `yes` / `on`, case-insensitive, trimmed).
fn env_truthy(name: &str) -> bool {
    traits::env::is_env_truthy(std::env::var(name).ok().as_deref())
}

/// Parse a base-10 unsigned integer that must be `> 0`; returns `None`
/// otherwise. Mirrors the `parseInt(...)` + `!isNaN && > 0` env-override guard
/// (JS `parseInt` reads a leading digit run, so a numeric prefix is accepted).
fn parse_positive_u64(raw: &str) -> Option<u64> {
    let digits: String = raw
        .trim_start()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse::<u64>().ok().filter(|&v| v > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Threshold math reads several process-wide env vars. Serialize all tests
    // that touch env so they don't race; restore prior values inside the guard.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    const ENV_VARS: &[&str] = &[
        "LINGXI_AUTO_COMPACT_WINDOW",
        "LINGXI_AUTOCOMPACT_PCT_OVERRIDE",
        "LINGXI_BLOCKING_LIMIT_OVERRIDE",
        "LINGXI_MAX_CONTEXT_TOKENS",
        "LINGXI_MAX_OUTPUT_TOKENS",
        "CLAUDE_CODE_DISABLE_1M_CONTEXT",
        "USER_TYPE",
        "DISABLE_COMPACT",
        "DISABLE_AUTO_COMPACT",
    ];

    /// Snapshot the env vars this module manipulates, clear them, run `body`,
    /// then restore. Holds [`ENV_LOCK`] for the duration so tests don't race on
    /// the shared process environment.
    fn with_clean_env(body: impl FnOnce()) {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let saved: Vec<(&str, Option<String>)> = ENV_VARS
            .iter()
            .map(|&k| (k, std::env::var(k).ok()))
            .collect();
        for &k in ENV_VARS {
            std::env::remove_var(k);
        }
        body();
        for (k, v) in saved {
            match v {
                Some(val) => std::env::set_var(k, val),
                None => std::env::remove_var(k),
            }
        }
    }

    // sonnet-4-6, no betas, no env overrides:
    //   context_window = 200_000, max_output = 32_000,
    //   reserved = min(32_000, 20_000) = 20_000 → effective = 180_000.
    const MODEL: &str = "claude-sonnet-4-6-20251001";
    const EFFECTIVE: u64 = 180_000;
    const AUTOCOMPACT: u64 = EFFECTIVE - AUTOCOMPACT_BUFFER_TOKENS; // 167_000

    #[test]
    fn effective_and_autocompact_baseline() {
        with_clean_env(|| {
            assert_eq!(effective_context_window_size(MODEL, &[]), EFFECTIVE);
            assert_eq!(auto_compact_threshold(MODEL, &[]), AUTOCOMPACT);
        });
    }

    #[test]
    fn opus_4_8_bedrock_id_uses_the_same_auto_compact_boundary() {
        // Claude Code 2.1.217 fixed Opus 4.8 Bedrock sessions never reaching
        // auto-compact. Provider-shaped IDs must canonicalize to the same 1M
        // window / 64k output tier as the first-party model id.
        with_clean_env(|| {
            let first_party = "claude-opus-4-8";
            let bedrock = "us.anthropic.claude-opus-4-8-v1:0";
            assert_eq!(
                effective_context_window_size(bedrock, &[]),
                effective_context_window_size(first_party, &[])
            );
            assert_eq!(
                auto_compact_threshold(bedrock, &[]),
                auto_compact_threshold(first_party, &[])
            );
        });
    }

    #[test]
    fn is_auto_compact_enabled_env_gates() {
        with_clean_env(|| {
            assert!(is_auto_compact_enabled(true));
            assert!(!is_auto_compact_enabled(false));

            std::env::set_var("DISABLE_COMPACT", "1");
            assert!(!is_auto_compact_enabled(true));
            std::env::remove_var("DISABLE_COMPACT");

            std::env::set_var("DISABLE_AUTO_COMPACT", "true");
            assert!(!is_auto_compact_enabled(true));
            std::env::remove_var("DISABLE_AUTO_COMPACT");

            // Non-truthy values do not disable.
            std::env::set_var("DISABLE_COMPACT", "0");
            assert!(is_auto_compact_enabled(true));
        });
    }

    // --- TokenWarningState field boundaries (autocompact enabled) ---------- //
    // threshold = AUTOCOMPACT = 167_000.
    //   warning  threshold = 167_000 - 20_000 = 147_000
    //   error    threshold = 167_000 - 20_000 = 147_000
    //   autocompact        = 167_000
    //   blocking limit     = effective(180_000) - 3_000 = 177_000

    #[test]
    fn warning_threshold_boundaries() {
        with_clean_env(|| {
            // just below warning
            let below = calculate_token_warning_state(146_999, MODEL, &[], true);
            assert!(!below.is_above_warning_threshold);
            // exactly at warning
            let at = calculate_token_warning_state(147_000, MODEL, &[], true);
            assert!(at.is_above_warning_threshold);
            // above
            let above = calculate_token_warning_state(150_000, MODEL, &[], true);
            assert!(above.is_above_warning_threshold);
        });
    }

    #[test]
    fn error_threshold_boundaries() {
        with_clean_env(|| {
            assert!(
                !calculate_token_warning_state(146_999, MODEL, &[], true).is_above_error_threshold
            );
            assert!(
                calculate_token_warning_state(147_000, MODEL, &[], true).is_above_error_threshold
            );
            assert!(
                calculate_token_warning_state(160_000, MODEL, &[], true).is_above_error_threshold
            );
        });
    }

    #[test]
    fn auto_compact_threshold_boundaries_enabled() {
        with_clean_env(|| {
            assert!(
                !calculate_token_warning_state(AUTOCOMPACT - 1, MODEL, &[], true)
                    .is_above_auto_compact_threshold
            );
            assert!(
                calculate_token_warning_state(AUTOCOMPACT, MODEL, &[], true)
                    .is_above_auto_compact_threshold
            );
            assert!(
                calculate_token_warning_state(AUTOCOMPACT + 5_000, MODEL, &[], true)
                    .is_above_auto_compact_threshold
            );
        });
    }

    #[test]
    fn auto_compact_threshold_always_false_when_disabled() {
        with_clean_env(|| {
            // Even above the autocompact threshold, the flag is false when
            // autocompact is disabled. The active threshold is then `effective`.
            let st = calculate_token_warning_state(AUTOCOMPACT + 5_000, MODEL, &[], false);
            assert!(!st.is_above_auto_compact_threshold);
        });
    }

    #[test]
    fn blocking_limit_boundaries() {
        with_clean_env(|| {
            // blocking limit = effective(180_000) - MANUAL_COMPACT_BUFFER(3_000) = 177_000.
            let limit = EFFECTIVE - MANUAL_COMPACT_BUFFER_TOKENS;
            assert_eq!(limit, 177_000);
            assert!(
                !calculate_token_warning_state(limit - 1, MODEL, &[], true).is_at_blocking_limit
            );
            assert!(calculate_token_warning_state(limit, MODEL, &[], true).is_at_blocking_limit);
            assert!(
                calculate_token_warning_state(limit + 1, MODEL, &[], true).is_at_blocking_limit
            );
        });
    }

    // --- percent_left rounding parity -------------------------------------- //

    #[test]
    fn percent_left_rounding_and_clamp() {
        // Math.round((threshold - usage)/threshold*100), clamped >= 0.
        // threshold = 167_000.
        with_clean_env(|| {
            // usage = 0 → 100%.
            assert_eq!(
                calculate_token_warning_state(0, MODEL, &[], true).percent_left,
                100
            );
            // usage == threshold → 0%.
            assert_eq!(
                calculate_token_warning_state(AUTOCOMPACT, MODEL, &[], true).percent_left,
                0
            );
            // usage > threshold → clamped to 0.
            assert_eq!(
                calculate_token_warning_state(AUTOCOMPACT + 100_000, MODEL, &[], true).percent_left,
                0
            );
            // half-way: usage = 83_500 → remaining 83_500/167_000 = 0.5 → round → 50.
            assert_eq!(
                calculate_token_warning_state(83_500, MODEL, &[], true).percent_left,
                50
            );
        });
    }

    #[test]
    fn percent_left_rounds_half_up() {
        // Construct a fraction that rounds half away from zero (JS Math.round).
        // threshold = 200 (via override), usage = 99 → (200-99)/200*100 = 50.5 → 51.
        with_clean_env(|| {
            std::env::set_var("LINGXI_AUTOCOMPACT_PCT_OVERRIDE", "100");
            // pct=100 makes autocompact_threshold = min(floor(effective*1.0), effective-buffer)
            //   = min(180_000, 167_000) = 167_000. Not what we want for a tiny threshold.
            // Instead use the helper directly for the rounding invariant.
            std::env::remove_var("LINGXI_AUTOCOMPACT_PCT_OVERRIDE");
            assert_eq!(super::percent_left_of(200, 99), 51);
            // 50.4 → 50: usage=101 → (200-101)/200*100 = 49.5 → 50.
            assert_eq!(super::percent_left_of(200, 101), 50);
            // threshold 0 → 0.
            assert_eq!(super::percent_left_of(0, 0), 0);
        });
    }

    // --- env overrides ----------------------------------------------------- //

    #[test]
    fn auto_compact_window_clamps_context() {
        with_clean_env(|| {
            // Clamp context window to 50_000. reserved = 20_000 → effective = 30_000.
            std::env::set_var("LINGXI_AUTO_COMPACT_WINDOW", "50000");
            assert_eq!(effective_context_window_size(MODEL, &[]), 30_000);
            // A clamp larger than the real window is a no-op (min picks the smaller).
            std::env::set_var("LINGXI_AUTO_COMPACT_WINDOW", "999999");
            assert_eq!(effective_context_window_size(MODEL, &[]), EFFECTIVE);
            // Invalid / zero values are ignored.
            std::env::set_var("LINGXI_AUTO_COMPACT_WINDOW", "0");
            assert_eq!(effective_context_window_size(MODEL, &[]), EFFECTIVE);
            std::env::set_var("LINGXI_AUTO_COMPACT_WINDOW", "abc");
            assert_eq!(effective_context_window_size(MODEL, &[]), EFFECTIVE);
        });
    }

    #[test]
    fn autocompact_pct_override() {
        with_clean_env(|| {
            // pct=10 → floor(180_000 * 0.10) = 18_000; min(18_000, 167_000) = 18_000.
            std::env::set_var("LINGXI_AUTOCOMPACT_PCT_OVERRIDE", "10");
            assert_eq!(auto_compact_threshold(MODEL, &[]), 18_000);
            // pct=100 → floor(180_000) = 180_000; min(180_000, 167_000) = 167_000.
            std::env::set_var("LINGXI_AUTOCOMPACT_PCT_OVERRIDE", "100");
            assert_eq!(auto_compact_threshold(MODEL, &[]), AUTOCOMPACT);
            // Out of range (>100) ignored.
            std::env::set_var("LINGXI_AUTOCOMPACT_PCT_OVERRIDE", "150");
            assert_eq!(auto_compact_threshold(MODEL, &[]), AUTOCOMPACT);
            // Zero / negative ignored.
            std::env::set_var("LINGXI_AUTOCOMPACT_PCT_OVERRIDE", "0");
            assert_eq!(auto_compact_threshold(MODEL, &[]), AUTOCOMPACT);
        });
    }

    #[test]
    fn blocking_limit_override() {
        with_clean_env(|| {
            std::env::set_var("LINGXI_BLOCKING_LIMIT_OVERRIDE", "100000");
            let below = calculate_token_warning_state(99_999, MODEL, &[], true);
            assert!(!below.is_at_blocking_limit);
            let at = calculate_token_warning_state(100_000, MODEL, &[], true);
            assert!(at.is_at_blocking_limit);
            // Invalid override falls back to the default (177_000).
            std::env::set_var("LINGXI_BLOCKING_LIMIT_OVERRIDE", "notanumber");
            let st = calculate_token_warning_state(100_000, MODEL, &[], true);
            assert!(!st.is_at_blocking_limit);
        });
    }

    #[test]
    fn disabled_autocompact_uses_effective_as_threshold() {
        with_clean_env(|| {
            // When disabled, threshold = effective(180_000), so percent_left at
            // usage 0 is 100 and at usage=180_000 is 0.
            assert_eq!(
                calculate_token_warning_state(0, MODEL, &[], false).percent_left,
                100
            );
            assert_eq!(
                calculate_token_warning_state(EFFECTIVE, MODEL, &[], false).percent_left,
                0
            );
            // warning threshold = 180_000 - 20_000 = 160_000.
            assert!(
                !calculate_token_warning_state(159_999, MODEL, &[], false)
                    .is_above_warning_threshold
            );
            assert!(
                calculate_token_warning_state(160_000, MODEL, &[], false)
                    .is_above_warning_threshold
            );
        });
    }

    // --- #54 rapid-refill (thrashing) breaker ------------------------------ //

    #[test]
    fn rapid_refill_count_zero_when_not_previously_compacted() {
        // `compacted=false` → kho returns 0 regardless of the other fields.
        let state = AutoCompactTrackingState {
            compacted: false,
            turn_counter: 0,
            consecutive_rapid_refills: 5,
            ..Default::default()
        };
        assert_eq!(rapid_refill_count(&state), 0);
    }

    #[test]
    fn rapid_refill_count_zero_when_turn_counter_at_or_above_window() {
        // turnCounter >= Who(3) → not a rapid refill → reset to 0.
        let state = AutoCompactTrackingState {
            compacted: true,
            turn_counter: 3,
            consecutive_rapid_refills: 2,
            ..Default::default()
        };
        assert_eq!(rapid_refill_count(&state), 0);
        let state4 = AutoCompactTrackingState {
            turn_counter: 4,
            ..state
        };
        assert_eq!(rapid_refill_count(&state4), 0);
    }

    #[test]
    fn rapid_refill_count_increments_within_window() {
        // compacted && turnCounter < Who → consecutiveRapidRefills + 1.
        // First rapid refill: prev count 0 → 1.
        let state0 = AutoCompactTrackingState {
            compacted: true,
            turn_counter: 0,
            consecutive_rapid_refills: 0,
            ..Default::default()
        };
        assert_eq!(rapid_refill_count(&state0), 1);
        // Second: prev count 1 → 2.
        let state1 = AutoCompactTrackingState {
            consecutive_rapid_refills: 1,
            turn_counter: 1,
            ..state0.clone()
        };
        assert_eq!(rapid_refill_count(&state1), 2);
        // Third: prev count 2 → 3 = MAX_CONSECUTIVE_RAPID_REFILLS → breaker trips.
        let state2 = AutoCompactTrackingState {
            consecutive_rapid_refills: 2,
            turn_counter: 2,
            ..state0
        };
        let count = rapid_refill_count(&state2);
        assert_eq!(count, 3);
        assert!(count >= MAX_CONSECUTIVE_RAPID_REFILLS, "breaker trips at 3");
    }

    #[test]
    fn rapid_refill_constants_match_binary() {
        // jho=3, Who=3, f6n=3.
        assert_eq!(MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES, 3);
        assert_eq!(RAPID_REFILL_TURN_WINDOW, 3);
        assert_eq!(MAX_CONSECUTIVE_RAPID_REFILLS, 3);
    }

    #[test]
    fn rapid_refill_thrashing_message_is_byte_exact() {
        // Byte-exact `Rho` (bin/claude.exe offset 203009521), with ${Who}/${f6n}
        // resolved to 3.
        assert_eq!(
            RAPID_REFILL_THRASHING_MESSAGE,
            "Autocompact is thrashing: the context refilled to the limit within 3 turns of the previous compact, 3 times in a row. A file being read or a tool output is likely too large for the context window. Try reading in smaller chunks, or use /clear to start fresh."
        );
    }
}

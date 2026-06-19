//! Pure threshold arithmetic for the orchestrator pass (Batch 3).
//!
//! These are the model-independent kernels behind the model-aware helpers in
//! [`crate::thresholds`]. The orchestrator works with already-resolved token
//! numbers (a window, a max-output budget, a precomputed threshold), so it
//! needs the raw math without re-resolving the model/env each call. Each
//! function mirrors the corresponding TS computation in
//! `src/services/compact/autoCompact.ts`:
//!
//! - [`effective_context_window`] ↔ `getEffectiveContextWindowSize`
//!   (`autoCompact.ts:33-49`), the `contextWindow − min(maxOutput, …)` step.
//! - [`auto_compact_threshold`] ↔ `getAutoCompactThreshold`
//!   (`autoCompact.ts:72-91`), the `effective − AUTOCOMPACT_BUFFER_TOKENS` step.
//! - [`should_auto_compact`] ↔ the threshold test inside `shouldAutoCompact`
//!   (`autoCompact.ts:225-238`), including the `snipTokensFreed` subtraction.
//!
//! The env-override branches (`CLAUDE_CODE_AUTO_COMPACT_WINDOW`,
//! `CLAUDE_AUTOCOMPACT_PCT_OVERRIDE`) live in the model-aware
//! [`crate::thresholds`] functions; these kernels take post-resolution numbers.

use crate::thresholds::{AUTOCOMPACT_BUFFER_TOKENS, MAX_OUTPUT_TOKENS_FOR_SUMMARY};

/// `context_window − min(max_output_tokens, MAX_OUTPUT_TOKENS_FOR_SUMMARY)`.
///
/// Mirrors `getEffectiveContextWindowSize` (`autoCompact.ts:33-49`): the model's
/// context window minus the output budget reserved for the summary call, capped
/// at [`MAX_OUTPUT_TOKENS_FOR_SUMMARY`]. Saturating so an out-of-range
/// `max_output` never underflows.
#[must_use]
pub fn effective_context_window(max_output_tokens: u64, context_window: u64) -> u64 {
    let reserved = max_output_tokens.min(MAX_OUTPUT_TOKENS_FOR_SUMMARY);
    context_window.saturating_sub(reserved)
}

/// `effective_window − AUTOCOMPACT_BUFFER_TOKENS`.
///
/// Mirrors `getAutoCompactThreshold` (`autoCompact.ts:72-91`) without the
/// `CLAUDE_AUTOCOMPACT_PCT_OVERRIDE` env branch (that lives in the model-aware
/// [`crate::thresholds::auto_compact_threshold`]). Saturating so a tiny window
/// never underflows.
#[must_use]
pub fn auto_compact_threshold(effective_window: u64) -> u64 {
    effective_window.saturating_sub(AUTOCOMPACT_BUFFER_TOKENS)
}

/// `true` when `estimated_tokens − snip_freed >= threshold`.
///
/// Mirrors the proactive auto-compact gate `l3p`→`KRe`/`pFi` in claude-code
/// v2.1.183: `l3p` subtracts the snip offset (`SC(...)-o`) before calling the
/// level classifier, and the classifier's compact arm is `n.enabled&&e>=o`
/// (`>=`, inclusive at the boundary). Snip already removed messages but the
/// surviving assistant usage still reflects pre-snip context, so the freed
/// delta is subtracted before comparing against the threshold. Saturating
/// subtraction so an over-large `snip_freed` clamps to zero rather than wrapping.
#[must_use]
pub fn should_auto_compact(estimated_tokens: u64, snip_freed: u64, threshold: u64) -> bool {
    estimated_tokens.saturating_sub(snip_freed) >= threshold
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec values: window 200k, max_output 20k → effective 180k, threshold 167k.
    #[test]
    fn threshold_arithmetic_matches_spec() {
        let effective = effective_context_window(20_000, 200_000);
        assert_eq!(effective, 180_000);
        assert_eq!(auto_compact_threshold(effective), 167_000);
    }

    #[test]
    fn effective_window_caps_reserved_at_summary_budget() {
        // max_output above the summary cap → reserve only the cap (20k).
        assert_eq!(effective_context_window(64_000, 200_000), 180_000);
        // max_output below the cap → reserve the smaller amount.
        assert_eq!(effective_context_window(8_192, 200_000), 191_808);
    }

    #[test]
    fn effective_window_saturates() {
        // Degenerate window smaller than the reserve → 0, never underflow.
        assert_eq!(effective_context_window(20_000, 10_000), 0);
    }

    #[test]
    fn auto_compact_threshold_saturates() {
        assert_eq!(auto_compact_threshold(5_000), 0);
    }

    #[test]
    fn should_auto_compact_inclusive_at_threshold() {
        // Above the threshold fires.
        assert!(should_auto_compact(167_001, 0, 167_000));
        // Exactly at the threshold fires (inclusive `>=`, matching the
        // binary's `e>=o` compact arm in pFi).
        assert!(should_auto_compact(167_000, 0, 167_000));
        // One below the threshold does NOT fire.
        assert!(!should_auto_compact(166_999, 0, 167_000));
        // Well below does not fire.
        assert!(!should_auto_compact(100_000, 0, 167_000));
    }

    #[test]
    fn should_auto_compact_subtracts_snip_freed() {
        // 170k tokens, but snip already freed 5k → 165k < 167k → no compact.
        assert!(!should_auto_compact(170_000, 5_000, 167_000));
        // Snip freed exactly down to the threshold → 167k >= 167k → compact.
        assert!(should_auto_compact(170_000, 3_000, 167_000));
        // Only 2k freed → 168k >= 167k → compact.
        assert!(should_auto_compact(170_000, 2_000, 167_000));
        // Over-large snip_freed clamps to zero, not above threshold.
        assert!(!should_auto_compact(100_000, 999_999, 167_000));
    }
}

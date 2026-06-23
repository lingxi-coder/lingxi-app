//! Model context-window and max-output-token tables.
//!
//! Ports the model → window / output lookups from the claude-code reference:
//! `src/utils/context.ts` (`getContextWindowForModel`, `getModelMaxOutputTokens`)
//! and `src/services/api/claude.ts` (`getMaxOutputTokensForModel`).
//!
//! ## Divergences from TS
//!
//! These functions take a `model: &str` (and `betas: &[String]`) and reproduce
//! the byte-faithful canonical-name table and env-override handling. A few TS
//! branches depend on subsystems not reachable from the `compaction` crate
//! without new dependencies; they are intentionally omitted and noted here:
//!
//! - `getModelCapability(model)` (litellm-style capability table) — the TS
//!   `max_input_tokens` / `max_tokens` override paths are skipped. No Rust
//!   capability registry is reachable from `compaction/`.
//! - `resolveAntModel` / `USER_TYPE === 'ant'` ant-model context-window and
//!   max-token overrides — the ant-model registry lives elsewhere. The
//!   `CLAUDE_CODE_MAX_CONTEXT_TOKENS` override (which TS *also* gates on
//!   `USER_TYPE === 'ant'`) IS honored here because it is a pure env read.
//! - `getSonnet1mExpTreatmentEnabled` (`GrowthBook` `coral_reef_sonnet`
//!   client-data-cache flag) — `GrowthBook` config has no Rust equivalent here;
//!   omitted.
//! - `isMaxTokensCapEnabled` (`GrowthBook` `tengu_otk_slot_v1` slot-reservation
//!   cap → drop default to [`CAPPED_DEFAULT_MAX_TOKENS`]) — `GrowthBook` flag is
//!   unwired in Rust, so the cap is NOT applied; [`max_output_tokens_for_model`]
//!   returns the model's native default. The `CLAUDE_CODE_MAX_OUTPUT_TOKENS`
//!   env override (a pure env read) IS honored, clamped to the upper limit.

use traits::env::is_env_truthy;

/// Default model context window (200k tokens for all models right now).
/// Mirrors `MODEL_CONTEXT_WINDOW_DEFAULT` in `utils/context.ts`.
pub const MODEL_CONTEXT_WINDOW_DEFAULT: u64 = 200_000;

/// Default max output tokens. Mirrors `MAX_OUTPUT_TOKENS_DEFAULT`.
const MAX_OUTPUT_TOKENS_DEFAULT: u64 = 32_000;
/// Upper limit for max output tokens (the unknown-model fallback). Mirrors the
/// binary `YCe` else-branch `NWu` = 128_000 (v2.1.183).
const MAX_OUTPUT_TOKENS_UPPER_LIMIT: u64 = 128_000;

/// 1M-context beta header. Mirrors `CONTEXT_1M_BETA_HEADER` in `constants/betas.ts`.
pub const CONTEXT_1M_BETA_HEADER: &str = "context-1m-2025-08-07";

/// `true` if 1M context is disabled via `CLAUDE_CODE_DISABLE_1M_CONTEXT`.
/// Mirrors `is1mContextDisabled`.
fn is_1m_context_disabled() -> bool {
    is_env_truthy(std::env::var("CLAUDE_CODE_DISABLE_1M_CONTEXT").ok().as_deref())
}

/// `true` if `model` carries an explicit `[1m]` suffix (case-insensitive),
/// unless 1M context is disabled. Mirrors `has1mContext`.
fn has_1m_context(model: &str) -> bool {
    if is_1m_context_disabled() {
        return false;
    }
    // TS uses /\[1m\]/i — a case-insensitive substring of the literal "[1m]".
    model.to_lowercase().contains("[1m]")
}

/// `true` if the canonical model family supports 1M context (sonnet-4 family
/// or opus-4-6), unless 1M context is disabled. Mirrors `modelSupports1M`.
fn model_supports_1m(model: &str) -> bool {
    if is_1m_context_disabled() {
        return false;
    }
    let canonical = canonical_name(model);
    canonical.contains("claude-sonnet-4") || canonical.contains("opus-4-6")
}

/// Resolve a full model id to a shorter canonical family name.
///
/// Ports `firstPartyNameToCanonical` (`utils/model/model.ts:217-270`). The
/// Bedrock-ARN `resolveOverriddenModel` indirection in `getCanonicalName` is a
/// no-op for the substring checks we perform, so it is folded in here.
fn canonical_name(model: &str) -> String {
    let name = model.to_lowercase();
    // Order matters: check more specific versions first (4-8/4-7/4-6 before 4-5 before 4).
    // opus-4-8/4-7 must precede the bare `claude-opus-4` catch so they resolve to
    // their own canonical (binary `getCanonicalName` preserves them) and get the
    // 64k/128k max-output tier (binary `YCe`), not the bare-family 32k/32k.
    if name.contains("claude-opus-4-8") {
        return "claude-opus-4-8".to_string();
    }
    if name.contains("claude-opus-4-7") {
        return "claude-opus-4-7".to_string();
    }
    if name.contains("claude-opus-4-6") {
        return "claude-opus-4-6".to_string();
    }
    if name.contains("claude-opus-4-5") {
        return "claude-opus-4-5".to_string();
    }
    if name.contains("claude-opus-4-1") {
        return "claude-opus-4-1".to_string();
    }
    if name.contains("claude-opus-4") {
        return "claude-opus-4".to_string();
    }
    if name.contains("claude-sonnet-4-6") {
        return "claude-sonnet-4-6".to_string();
    }
    if name.contains("claude-sonnet-4-5") {
        return "claude-sonnet-4-5".to_string();
    }
    if name.contains("claude-sonnet-4") {
        return "claude-sonnet-4".to_string();
    }
    if name.contains("claude-haiku-4-5") {
        return "claude-haiku-4-5".to_string();
    }
    if name.contains("claude-3-7-sonnet") {
        return "claude-3-7-sonnet".to_string();
    }
    if name.contains("claude-3-5-sonnet") {
        return "claude-3-5-sonnet".to_string();
    }
    if name.contains("claude-3-5-haiku") {
        return "claude-3-5-haiku".to_string();
    }
    if name.contains("claude-3-opus") {
        return "claude-3-opus".to_string();
    }
    if name.contains("claude-3-sonnet") {
        return "claude-3-sonnet".to_string();
    }
    if name.contains("claude-3-haiku") {
        return "claude-3-haiku".to_string();
    }
    // Fall back to the original (lowercased) name if no pattern matches. The TS
    // `/(claude-(\d+-\d+-)?\w+)/` regex only narrows the string for the unmatched
    // case; substring checks against the full lowercased name are equivalent for
    // our purposes, so we return it unmodified.
    name
}

/// Returns the context window size for `model`, honoring `betas`.
///
/// Mirrors `getContextWindowForModel` (`utils/context.ts:51-98`), minus the
/// model-capability and ant-model registry branches (see module docs).
#[must_use]
pub fn context_window_for_model(model: &str, betas: &[String]) -> u64 {
    // CLAUDE_CODE_MAX_CONTEXT_TOKENS override (TS gates on USER_TYPE === 'ant').
    // Takes precedence over all other resolution, including 1M detection.
    if std::env::var("USER_TYPE").ok().as_deref() == Some("ant") {
        if let Ok(raw) = std::env::var("CLAUDE_CODE_MAX_CONTEXT_TOKENS") {
            if let Some(parsed) = parse_positive_i64(&raw) {
                return parsed;
            }
        }
    }

    // [1m] suffix — explicit client-side opt-in, respected over all detection.
    if has_1m_context(model) {
        return 1_000_000;
    }

    // CONTEXT_1M_BETA_HEADER beta + 1M-capable model → 1M.
    if betas.iter().any(|b| b == CONTEXT_1M_BETA_HEADER) && model_supports_1m(model) {
        return 1_000_000;
    }

    MODEL_CONTEXT_WINDOW_DEFAULT
}

/// Returns the model's default and upper limit for max output tokens.
///
/// Ports `getModelMaxOutputTokens` (`utils/context.ts:149-210`), minus the
/// ant-model and model-capability branches (see module docs).
fn model_max_output_tokens(model: &str) -> (u64, u64) {
    let m = canonical_name(model);
    // Binary `YCe` (v2.1.183 getModelMaxOutputTokens): fable-5/mythos-5/opus-4-8/
    // opus-4-7/opus-4-6 → 64k/128k; sonnet-4-6 → 32k/128k; opus-4-5/sonnet-4-0/4-5/
    // haiku-4-5 → 32k/64k; opus-4-1/4-0 → 32k/32k; else → 32k/128k.
    let (default_tokens, upper_limit) = if m.contains("opus-4-8")
        || m.contains("opus-4-7")
        || m.contains("fable-5")
        || m.contains("mythos-5")
    {
        (64_000, 128_000)
    } else if m.contains("opus-4-6") {
        (64_000, 128_000)
    } else if m.contains("sonnet-4-6") {
        (32_000, 128_000)
    } else if m.contains("opus-4-5") || m.contains("sonnet-4") || m.contains("haiku-4") {
        (32_000, 64_000)
    } else if m.contains("opus-4-1") || m.contains("opus-4") {
        (32_000, 32_000)
    } else if m.contains("claude-3-opus") {
        (4_096, 4_096)
    } else if m.contains("claude-3-sonnet") {
        (8_192, 8_192)
    } else if m.contains("claude-3-haiku") {
        (4_096, 4_096)
    } else if m.contains("3-5-sonnet") || m.contains("3-5-haiku") {
        (8_192, 8_192)
    } else if m.contains("3-7-sonnet") {
        (32_000, 64_000)
    } else {
        (MAX_OUTPUT_TOKENS_DEFAULT, MAX_OUTPUT_TOKENS_UPPER_LIMIT)
    };
    (default_tokens, upper_limit)
}

/// Returns the effective max output tokens for `model`.
///
/// Mirrors `getMaxOutputTokensForModel` (`api/claude.ts:3399-3419`). The
/// GrowthBook-gated `isMaxTokensCapEnabled` slot cap is unwired in Rust (see
/// module docs), so the native default is used; the
/// `CLAUDE_CODE_MAX_OUTPUT_TOKENS` env override is honored, clamped to the
/// model's upper limit.
#[must_use]
pub fn max_output_tokens_for_model(model: &str) -> u64 {
    let (default_tokens, upper_limit) = model_max_output_tokens(model);

    // validateBoundedIntEnvVar('CLAUDE_CODE_MAX_OUTPUT_TOKENS', …, default, upper):
    // a positive integer override is clamped to the upper limit; otherwise the
    // default is used.
    if let Ok(raw) = std::env::var("CLAUDE_CODE_MAX_OUTPUT_TOKENS") {
        if let Some(parsed) = parse_positive_i64(&raw) {
            return parsed.min(upper_limit);
        }
    }

    default_tokens
}

/// Returns the maximum thinking-budget tokens for `model`.
///
/// Mirrors claude-code's `getMaxThinkingTokensForModel`
/// (`getModelMaxOutputTokens(model).upperLimit - 1`): the fixed-budget thinking
/// ceiling is one below the model's max-output upper limit. Used by the
/// orchestrator when the model does not support adaptive thinking and a concrete
/// `budget_tokens` must be supplied.
#[must_use]
pub fn max_thinking_tokens_for_model(model: &str) -> u32 {
    let (_default_tokens, upper_limit) = model_max_output_tokens(model);
    u32::try_from(upper_limit.saturating_sub(1)).unwrap_or(u32::MAX)
}

/// Parse a base-10 integer that must be `> 0`; returns `None` otherwise.
/// Mirrors the `parseInt(...)` + `!isNaN && > 0` guard used throughout the TS
/// env-override code. Leading whitespace and a trailing non-numeric suffix are
/// tolerated to match JS `parseInt` semantics for the common cases.
fn parse_positive_i64(raw: &str) -> Option<u64> {
    let trimmed = raw.trim_start();
    // JS parseInt reads a leading run of digits (with optional sign). We accept
    // a clean unsigned integer, which covers every realistic override value.
    let digits: String = trimmed
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

    #[test]
    fn default_window_for_unknown_and_known_models() {
        // No betas, no [1m] suffix → 200k default for every model.
        for model in [
            "claude-opus-4-6-20260101",
            "claude-sonnet-4-6-20251001",
            "claude-3-5-haiku-20241022",
            "some-unknown-model",
        ] {
            assert_eq!(context_window_for_model(model, &[]), 200_000, "{model}");
        }
    }

    #[test]
    fn one_m_suffix_unlocks_1m_window() {
        assert_eq!(
            context_window_for_model("claude-sonnet-4-6[1m]", &[]),
            1_000_000
        );
        // Case-insensitive on the suffix.
        assert_eq!(
            context_window_for_model("claude-sonnet-4-6[1M]", &[]),
            1_000_000
        );
    }

    #[test]
    fn beta_header_unlocks_1m_only_for_capable_models() {
        let betas = vec![CONTEXT_1M_BETA_HEADER.to_string()];
        // sonnet-4 family is 1M-capable.
        assert_eq!(
            context_window_for_model("claude-sonnet-4-6-20251001", &betas),
            1_000_000
        );
        // opus-4-6 is 1M-capable.
        assert_eq!(
            context_window_for_model("claude-opus-4-6-20260101", &betas),
            1_000_000
        );
        // A non-capable model with the beta still gets the default.
        assert_eq!(
            context_window_for_model("claude-3-5-haiku-20241022", &betas),
            200_000
        );
    }

    #[test]
    fn max_output_tokens_canonical_table() {
        assert_eq!(max_output_tokens_for_model("claude-opus-4-6-20260101"), 64_000);
        assert_eq!(
            max_output_tokens_for_model("claude-sonnet-4-6-20251001"),
            32_000
        );
        assert_eq!(max_output_tokens_for_model("claude-opus-4-5-x"), 32_000);
        assert_eq!(max_output_tokens_for_model("claude-sonnet-4-20250101"), 32_000);
        assert_eq!(max_output_tokens_for_model("claude-haiku-4-5-x"), 32_000);
        assert_eq!(max_output_tokens_for_model("claude-opus-4-1-x"), 32_000);
        // Binary YCe: opus-4-8/4-7/fable-5/mythos-5 → 64k (NOT the bare-opus-4 32k).
        assert_eq!(max_output_tokens_for_model("claude-opus-4-8-20260115"), 64_000);
        assert_eq!(max_output_tokens_for_model("claude-opus-4-7-x"), 64_000);
        assert_eq!(max_output_tokens_for_model("claude-fable-5"), 64_000);
        assert_eq!(max_output_tokens_for_model("claude-mythos-5"), 64_000);
        assert_eq!(max_output_tokens_for_model("claude-opus-4-20250514"), 32_000);
        assert_eq!(max_output_tokens_for_model("claude-3-opus-20240229"), 4_096);
        assert_eq!(max_output_tokens_for_model("claude-3-sonnet-20240229"), 8_192);
        assert_eq!(max_output_tokens_for_model("claude-3-haiku-20240307"), 4_096);
        assert_eq!(max_output_tokens_for_model("claude-3-5-sonnet-20241022"), 8_192);
        assert_eq!(max_output_tokens_for_model("claude-3-5-haiku-20241022"), 8_192);
        assert_eq!(max_output_tokens_for_model("claude-3-7-sonnet-20250219"), 32_000);
        // Unknown model → default.
        assert_eq!(max_output_tokens_for_model("mystery-model"), 32_000);
    }

    #[test]
    fn parse_positive_i64_semantics() {
        assert_eq!(parse_positive_i64("12345"), Some(12_345));
        assert_eq!(parse_positive_i64("  42"), Some(42));
        assert_eq!(parse_positive_i64("100abc"), Some(100));
        assert_eq!(parse_positive_i64("0"), None);
        assert_eq!(parse_positive_i64(""), None);
        assert_eq!(parse_positive_i64("abc"), None);
        assert_eq!(parse_positive_i64("-5"), None);
    }

    #[test]
    fn env_truthy_matches_ts() {
        for v in ["1", "true", "TRUE", " yes ", "on", "On"] {
            assert!(is_env_truthy(Some(v)), "{v} should be truthy");
        }
        for v in ["0", "false", "no", "off", "", "anything"] {
            assert!(!is_env_truthy(Some(v)), "{v} should be falsy");
        }
        assert!(!is_env_truthy(None));
    }
}

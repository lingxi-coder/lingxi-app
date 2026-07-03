//! Thinking-mode model predicates and per-request thinking configuration.
//!
//! Ports claude-code's thinking gating from `services/api/claude.ts:1596-1630`
//! (`modelSupportsThinking`, `modelSupportsAdaptiveThinking`,
//! `getMaxThinkingTokensForModel`) onto the orchestrator's request-build path.
//!
//! ## Canonical-name handling
//!
//! These predicates compare the model's **canonical** name (date suffix stripped)
//! for either a `claude-3-` prefix (thinking support) or exact membership in the
//! adaptive-thinking set. The canonical names this module must distinguish —
//! `claude-opus-4-8`, `claude-opus-4-7`, `claude-fable-5`, `claude-mythos-5`,
//! `claude-sonnet-4-6` — are finer-grained than `compaction`'s
//! `firstPartyNameToCanonical` (which collapses `claude-opus-4-8` → `claude-opus-4`).
//! We therefore carry a dedicated [`canonical`] helper here that preserves the
//! full family-version so the exact-equality adaptive set is byte-faithful to the
//! v2.1.183 binary.

/// Reduce a full model id to its canonical family-version name (date suffix
/// stripped), preserving the precision the thinking predicates require.
///
/// claude-code's `firstPartyNameToCanonical` maps e.g.
/// `claude-opus-4-8-20260115` → `claude-opus-4-8`. We recognize each known
/// family-version as a substring and return it; for an unrecognized id we strip
/// a trailing `-YYYYMMDD` date (and any `[1m]`-style suffix) and return the
/// lowercased remainder. The exact value only matters for the known set —
/// unknown ids fall through to the firstParty default in both predicates.
#[must_use]
pub fn canonical(model: &str) -> String {
    let name = model.to_lowercase();
    // Most specific first. These are the family-versions the predicates key on.
    const KNOWN: &[&str] = &[
        "claude-fable-5",
        "claude-mythos-5",
        "claude-mythos-preview",
        "claude-opus-4-8",
        "claude-opus-4-7",
        "claude-opus-4-6",
        "claude-opus-4-5",
        "claude-opus-4-1",
        "claude-opus-4-0",
        // sonnet-5 before the sonnet-4-x arms (2.1.198 registry; mutually
        // exclusive substrings — "claude-sonnet-4-5" does NOT contain it).
        "claude-sonnet-5",
        "claude-sonnet-4-6",
        "claude-sonnet-4-5",
        "claude-sonnet-4-0",
        "claude-haiku-4-5",
    ];
    for k in KNOWN {
        if name.contains(k) {
            return (*k).to_string();
        }
    }
    // `claude-opus-4` / `claude-sonnet-4` (undated family) after the dotted ones.
    for k in ["claude-opus-4", "claude-sonnet-4", "claude-haiku-4"] {
        if name.contains(k) {
            return k.to_string();
        }
    }
    // claude-3-* families — preserve the `claude-3-` prefix for the thinking gate.
    for k in [
        "claude-3-7-sonnet",
        "claude-3-5-sonnet",
        "claude-3-5-haiku",
        "claude-3-opus",
        "claude-3-sonnet",
        "claude-3-haiku",
    ] {
        if name.contains(k) {
            return k.to_string();
        }
    }
    name
}

/// `modelSupportsThinking` (firstParty path): every model whose canonical name
/// does NOT start with `claude-3-` supports thinking.
///
/// fable-5 / mythos-5 / opus-4-8 / sonnet-4-6 / haiku-4-5 → `true`;
/// the claude-3 families → `false`. Unknown ids default to `true`.
#[must_use]
pub fn model_supports_thinking(model: &str) -> bool {
    !canonical(model).starts_with("claude-3-")
}

/// `modelSupportsAdaptiveThinking` — EXACT v2.1.183 binary list (canonical
/// equality, NOT substring `.includes`).
///
/// `true` for `{claude-fable-5, claude-mythos-5, claude-opus-4-8, claude-opus-4-7,
/// claude-opus-4-6, claude-sonnet-4-6}`; `false` for the explicit non-adaptive
/// set `{claude-3-*, claude-opus-4-0, claude-opus-4-1, claude-opus-4-5,
/// claude-sonnet-4-0, claude-sonnet-4-5, claude-haiku-4-5}`; any other
/// (unknown) canonical defaults to `true` (firstParty default).
#[must_use]
pub fn model_supports_adaptive_thinking(model: &str) -> bool {
    let c = canonical(model);
    // Explicit TRUE set. 2.1.198 `Vit`: sonnet-5 resolves TRUE via the registry
    // capability check `lB(n,"adaptive_thinking")` (its capabilities include
    // "adaptive_thinking"); folded into the static TRUE set here.
    const ADAPTIVE: &[&str] = &[
        "claude-fable-5",
        "claude-mythos-5",
        "claude-opus-4-8",
        "claude-opus-4-7",
        "claude-opus-4-6",
        "claude-sonnet-5",
        "claude-sonnet-4-6",
    ];
    if ADAPTIVE.contains(&c.as_str()) {
        return true;
    }
    // Explicit FALSE set.
    const NON_ADAPTIVE: &[&str] = &[
        "claude-opus-4-0",
        "claude-opus-4-1",
        "claude-opus-4-5",
        "claude-sonnet-4-0",
        "claude-sonnet-4-5",
        "claude-haiku-4-5",
        // bare family fall-throughs that resolve to a 4.0-class model
        "claude-opus-4",
        "claude-sonnet-4",
        "claude-haiku-4",
    ];
    if NON_ADAPTIVE.contains(&c.as_str()) {
        return false;
    }
    if c.starts_with("claude-3-") {
        return false;
    }
    // Unknown (incl. mythos-preview, future ids) → firstParty default.
    true
}

/// `rhn` — the v2.1.185 temperature-gate model set (binary `function rhn`
/// @195164989). When thinking is disabled, claude-code sends `temperature:1`
/// ONLY for these models; for every OTHER model — including the default
/// `claude-opus-4-8`, `claude-opus-4-7`, `claude-fable-5`, `claude-mythos-5`,
/// and any unknown id — it omits the `temperature` field entirely.
///
/// Binary: substring match on `claude-3-` plus canonical equality on the
/// explicit 4.x list `{opus-4-0, opus-4-1, opus-4-5, opus-4-6, sonnet-4-0,
/// sonnet-4-5, sonnet-4-6, haiku-4-5}`. NOTE this is a DISTINCT set from
/// [`model_supports_adaptive_thinking`] (which puts opus-4-8/4-7/fable-5/
/// mythos-5 in its TRUE set) — do not conflate them.
#[must_use]
pub fn model_sends_temperature(model: &str) -> bool {
    let c = canonical(model);
    if c.starts_with("claude-3-") {
        return true;
    }
    const TEMP: &[&str] = &[
        "claude-opus-4-0",
        "claude-opus-4-1",
        "claude-opus-4-5",
        "claude-opus-4-6",
        "claude-sonnet-4-0",
        "claude-sonnet-4-5",
        "claude-sonnet-4-6",
        "claude-haiku-4-5",
        // bare family fall-throughs resolve to a 4.0-class id, all in the TRUE set
        "claude-opus-4",
        "claude-sonnet-4",
        "claude-haiku-4",
    ];
    TEMP.contains(&c.as_str())
}

/// Session/request thinking configuration, mirroring claude-code's resolved
/// `thinking` intent before it is rendered into the Anthropic `thinking` field.
///
/// `Default` is [`ThinkingConfig::Adaptive`] — claude-code's default for
/// adaptive-capable models (`alwaysThinkingEnabled` true by default).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ThinkingConfig {
    /// Thinking is off — no `thinking` field, and `temperature:1` is sent.
    Disabled,
    /// Adaptive thinking — the model decides depth (default).
    Adaptive,
    /// Fixed thinking budget cap.
    Enabled {
        /// Requested maximum thinking-budget tokens.
        budget_tokens: u32,
    },
}

impl Default for ThinkingConfig {
    fn default() -> Self {
        Self::Adaptive
    }
}

/// `true` when the named env var is truthy under the strict claude-code
/// allowlist (`1`/`true`/`yes`/`on`). Shared gate for
/// `LINGXI_DISABLE_THINKING` / `LINGXI_DISABLE_ADAPTIVE_THINKING`.
#[must_use]
pub fn is_thinking_env_disabled(name: &str) -> bool {
    traits::env::is_env_truthy(std::env::var(name).ok().as_deref())
}

/// Session-level "thinking is on" gate: the session [`ThinkingConfig`] is not
/// `Disabled` AND the `LINGXI_DISABLE_THINKING` kill switch is off. Drives
/// both the `reasoning` field (via [`reasoning_for_request`]) and the
/// thinking-off `temperature:1` rule in `ApiService::build_request`.
#[must_use]
pub fn session_thinking_active(thinking: ThinkingConfig) -> bool {
    thinking != ThinkingConfig::Disabled && !is_thinking_env_disabled("LINGXI_DISABLE_THINKING")
}

/// Resolve the SESSION [`ThinkingConfig`] into the per-request
/// [`crate::ReasoningConfig`] for `model` — the exact logic
/// `ApiService::build_request` applies to every main-loop AND subagent request
/// (claude.ts:1596-1630), extracted so the compaction side-query path can
/// inherit the SAME session thinking configuration (cc 2.1.198 "Subagents +
/// compaction inherit extended thinking config"; binary: the summarizer call
/// passes `thinkingConfig: mXt(r)` — the session `options.thinkingConfig` —
/// @216945141/@216926189).
///
/// The byte-faithful claude-code thinking shape (Adaptive default, canonical
/// max-output budget cap) is Anthropic-specific. The OpenAI/Gemini codecs
/// mistranslate `Adaptive` to a forced `effort="high"` / `thinkingBudget=0`,
/// so it must NOT be applied to non-Claude models: those only honor an
/// EXPLICIT fixed budget (the provider applies its own reasoning default
/// otherwise).
#[must_use]
pub fn reasoning_for_request(
    thinking: ThinkingConfig,
    model: &str,
    max_tokens: Option<u32>,
) -> Option<crate::ReasoningConfig> {
    if !session_thinking_active(thinking) {
        return None;
    }
    if crate::model::context_window::is_claude_family(model) {
        // Claude path — unchanged from claude-code.
        if !model_supports_thinking(model) {
            return None;
        }
        if !is_thinking_env_disabled("LINGXI_DISABLE_ADAPTIVE_THINKING")
            && model_supports_adaptive_thinking(model)
        {
            return Some(crate::ReasoningConfig::Adaptive);
        }
        let mut budget = crate::model::context_window::max_thinking_tokens_for_model(model);
        if let ThinkingConfig::Enabled { budget_tokens } = thinking {
            budget = budget_tokens;
        }
        // budget_tokens must stay strictly below max_tokens.
        budget = budget.min(max_tokens.unwrap_or(u32::MAX).saturating_sub(1));
        Some(crate::ReasoningConfig::Enabled {
            budget_tokens: budget,
        })
    } else {
        // Non-Claude: only honor an EXPLICIT fixed budget.
        match thinking {
            ThinkingConfig::Enabled { budget_tokens } => {
                let budget =
                    budget_tokens.min(max_tokens.unwrap_or(u32::MAX).saturating_sub(1));
                Some(crate::ReasoningConfig::Enabled {
                    budget_tokens: budget,
                })
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_strips_date_and_preserves_family_version() {
        assert_eq!(canonical("claude-opus-4-8-20260115"), "claude-opus-4-8");
        assert_eq!(canonical("claude-sonnet-4-6-20251114"), "claude-sonnet-4-6");
        assert_eq!(canonical("claude-haiku-4-5-20251001"), "claude-haiku-4-5");
        assert_eq!(canonical("claude-fable-5"), "claude-fable-5");
        assert_eq!(canonical("claude-3-5-haiku-20241022"), "claude-3-5-haiku");
        // bare opus-4 family (no dotted minor) → the undated family name
        assert_eq!(canonical("claude-opus-4-20250514"), "claude-opus-4");
    }

    #[test]
    fn supports_thinking_excludes_claude_3() {
        for m in [
            "claude-fable-5",
            "claude-opus-4-8-20260115",
            "claude-sonnet-4-6-20251114",
            "claude-haiku-4-5-20251001",
        ] {
            assert!(model_supports_thinking(m), "{m} should support thinking");
        }
        for m in [
            "claude-3-opus-20240229",
            "claude-3-5-sonnet-20241022",
            "claude-3-5-haiku-20241022",
            "claude-3-7-sonnet-20250219",
        ] {
            assert!(
                !model_supports_thinking(m),
                "{m} should NOT support thinking"
            );
        }
    }

    #[test]
    fn sonnet_5_canonical_adaptive_and_no_temperature() {
        // 2.1.198: canonical preserves claude-sonnet-5 (incl. dated ids), it is
        // adaptive-thinking (registry capability), and it is NOT in the sIn
        // temperature set (temperature omitted when thinking is off).
        assert_eq!(canonical("claude-sonnet-5"), "claude-sonnet-5");
        assert_eq!(canonical("claude-sonnet-5-20260203"), "claude-sonnet-5");
        // Neighbor ids must NOT resolve to sonnet-5.
        assert_eq!(canonical("claude-sonnet-4-5-20250929"), "claude-sonnet-4-5");
        assert_eq!(canonical("claude-3-5-sonnet-20241022"), "claude-3-5-sonnet");
        assert!(model_supports_adaptive_thinking("claude-sonnet-5"));
        assert!(model_supports_thinking("claude-sonnet-5"));
        assert!(!model_sends_temperature("claude-sonnet-5"));
        // sonnet-4-6 keeps its (distinct) behavior: adaptive AND temperature.
        assert!(model_sends_temperature("claude-sonnet-4-6"));
    }

    #[test]
    fn adaptive_thinking_exact_binary_list() {
        for m in [
            "claude-fable-5",
            "claude-mythos-5",
            "claude-sonnet-5",
            "claude-opus-4-8-20260115",
            "claude-opus-4-7-20251201",
            "claude-opus-4-6-20260101",
            "claude-sonnet-4-6-20251114",
        ] {
            assert!(model_supports_adaptive_thinking(m), "{m} adaptive=true");
        }
        for m in [
            "claude-opus-4-0-20250514",
            "claude-opus-4-1-20250805",
            "claude-opus-4-5-20251101",
            "claude-sonnet-4-0-20250514",
            "claude-sonnet-4-5-20250929",
            "claude-haiku-4-5-20251001",
            "claude-3-5-haiku-20241022",
            "claude-3-opus-20240229",
        ] {
            assert!(!model_supports_adaptive_thinking(m), "{m} adaptive=false");
        }
        // Unknown → firstParty default true.
        assert!(model_supports_adaptive_thinking("claude-some-future-9"));
    }

    #[test]
    fn thinking_config_default_is_adaptive() {
        assert_eq!(ThinkingConfig::default(), ThinkingConfig::Adaptive);
    }
}

//! Per-model capability registry — claude-code's `capabilities: [...]` array.
//!
//! The oracle's model table carries an explicit capability list per model, and
//! consumers ask `LN(model, "<capability>")` rather than pattern-matching the
//! model NAME. 2.1.220 @225786114-225787817:
//!
//! ```text
//! claude-opus-4-8 => [effort, max_effort, xhigh_effort, adaptive_thinking,
//!                     mid_conv_system, context_management, fast_mode, lean_prompt]
//! claude-opus-5   => [... , lean_prompt, refusal_fallback, opus_5_prompt_bundle]
//! claude-fable-5  => [... , lean_prompt, fable_5_mitigations, refusal_fallback]
//! ```
//!
//! WHY THIS EXISTS. The port decided "does this model take the lean system
//! prompt" by hardcoding a model-name list. It produced the right answer for
//! the models that were enumerated, and the right answer for `claude-opus-5`
//! only BY ACCIDENT — opus-5 was in neither branch and reached a fallthrough
//! that happens to return the correct result. A name list is also wrong in the
//! dangerous direction for anything added later: an unknown model silently
//! inherits whatever the fallthrough does, and nothing fails.
//!
//! This registry is deliberately NOT a guess-from-the-name function. A model
//! that is not listed returns `false` for every capability, which is the
//! conservative direction: a caller asking "does this support X" gets "no"
//! rather than a wrong "yes" derived from a substring match.

/// A model capability, spelled exactly as the oracle's wire string.
///
/// Only the variants the port actually consults are enumerated; the rest of
/// each model's list is preserved in [`capabilities_for`] so adding a consumer
/// later is a lookup, not another archaeology pass over the binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelCapability {
    /// Model takes the LEAN (short) system prompt — the 2.1.219+ "new rules of
    /// context engineering" path.
    LeanPrompt,
    /// Model supports a mid-conversation system-prompt swap.
    MidConvSystem,
    /// Model participates in refusal-fallback routing.
    RefusalFallback,
    /// Model ships the Opus-5 prompt bundle.
    Opus5PromptBundle,
    /// Model ships the Fable-5 mitigations.
    Fable5Mitigations,
    /// Model supports fast mode.
    FastMode,
}

impl ModelCapability {
    /// The oracle's wire string for this capability.
    #[must_use]
    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::LeanPrompt => "lean_prompt",
            Self::MidConvSystem => "mid_conv_system",
            Self::RefusalFallback => "refusal_fallback",
            Self::Opus5PromptBundle => "opus_5_prompt_bundle",
            Self::Fable5Mitigations => "fable_5_mitigations",
            Self::FastMode => "fast_mode",
        }
    }
}

/// The capability list for a first-party model id, verbatim from the 2.1.220
/// table. Unknown ids (including every third-party / non-Anthropic model)
/// return an empty slice.
///
/// Matching is on the BARE model id. A caller holding a provider-qualified id
/// (`openrouter/anthropic/claude-opus-5`) or a `-eap` / `[1m]` suffixed id must
/// normalize before asking — see [`capabilities_for_loose`].
#[must_use]
pub fn capabilities_for(model_id: &str) -> &'static [&'static str] {
    match model_id {
        "claude-opus-4-5" => &["context_management"],
        "claude-opus-4-6" => &[
            "effort",
            "max_effort",
            "adaptive_thinking",
            "context_management",
        ],
        "claude-opus-4-7" => &[
            "effort",
            "max_effort",
            "xhigh_effort",
            "adaptive_thinking",
            "context_management",
            "fast_mode",
        ],
        "claude-opus-4-8" => &[
            "effort",
            "max_effort",
            "xhigh_effort",
            "adaptive_thinking",
            "mid_conv_system",
            "context_management",
            "fast_mode",
            "lean_prompt",
        ],
        "claude-opus-5" => &[
            "effort",
            "max_effort",
            "xhigh_effort",
            "adaptive_thinking",
            "mid_conv_system",
            "context_management",
            "fast_mode",
            "lean_prompt",
            "refusal_fallback",
            "opus_5_prompt_bundle",
        ],
        "claude-fable-5" => &[
            "effort",
            "max_effort",
            "xhigh_effort",
            "adaptive_thinking",
            "rejects_disabled_thinking",
            "mid_conv_system",
            "context_management",
            "lean_prompt",
            "fable_5_mitigations",
            "refusal_fallback",
        ],
        // Present in the table with NO capabilities. The oracle's lean-prompt
        // consumer special-cases it BY NAME (`|| t === "claude-mythos-5"`)
        // precisely because it lacks the capability but must still take the
        // non-standard branch — so do not "fix" this to include lean_prompt.
        "claude-mythos-5" => &[],
        _ => &[],
    }
}

/// Strip the decorations a model id can carry before a registry lookup:
/// a provider/profile prefix (`openrouter/anthropic/…`), a `[1m]` context
/// suffix, and an `-eap` early-access suffix.
#[must_use]
pub fn normalize_model_id(model_id: &str) -> String {
    let bare = model_id.rsplit('/').next().unwrap_or(model_id);
    let bare = bare.split('[').next().unwrap_or(bare);
    let lower = bare.trim().to_ascii_lowercase();
    lower
        .strip_suffix("-eap")
        .map_or(lower.clone(), str::to_string)
}

/// [`capabilities_for`] after [`normalize_model_id`].
#[must_use]
pub fn capabilities_for_loose(model_id: &str) -> &'static [&'static str] {
    capabilities_for(&normalize_model_id(model_id))
}

/// Does `model_id` carry `capability`? Oracle `LN(model, capability)`.
///
/// An unknown model is `false` for everything — the conservative answer.
#[must_use]
pub fn has_capability(model_id: &str, capability: ModelCapability) -> bool {
    capabilities_for_loose(model_id).contains(&capability.as_wire())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lean_prompt_models_match_the_oracle_table() {
        for id in ["claude-opus-4-8", "claude-opus-5", "claude-fable-5"] {
            assert!(
                has_capability(id, ModelCapability::LeanPrompt),
                "{id} must have lean_prompt"
            );
        }
        for id in [
            "claude-opus-4-5",
            "claude-opus-4-6",
            "claude-opus-4-7",
            "claude-mythos-5",
        ] {
            assert!(
                !has_capability(id, ModelCapability::LeanPrompt),
                "{id} must NOT have lean_prompt"
            );
        }
    }

    #[test]
    fn opus_5_carries_its_bundle_and_refusal_fallback() {
        assert!(has_capability(
            "claude-opus-5",
            ModelCapability::Opus5PromptBundle
        ));
        assert!(has_capability(
            "claude-opus-5",
            ModelCapability::RefusalFallback
        ));
        // ...and 4-8 does not, despite also having lean_prompt.
        assert!(!has_capability(
            "claude-opus-4-8",
            ModelCapability::Opus5PromptBundle
        ));
        assert!(!has_capability(
            "claude-opus-4-8",
            ModelCapability::RefusalFallback
        ));
    }

    #[test]
    fn fable_5_carries_its_mitigations() {
        assert!(has_capability(
            "claude-fable-5",
            ModelCapability::Fable5Mitigations
        ));
        assert!(!has_capability(
            "claude-opus-5",
            ModelCapability::Fable5Mitigations
        ));
    }

    #[test]
    fn an_unknown_model_has_no_capabilities() {
        // The conservative direction: "no" rather than a wrong "yes" inferred
        // from a substring match.
        for id in ["gpt-5", "deepseek/deepseek-chat", "claude-opus-9", ""] {
            assert!(capabilities_for_loose(id).is_empty(), "{id}");
            assert!(!has_capability(id, ModelCapability::LeanPrompt), "{id}");
        }
    }

    #[test]
    fn decorated_ids_normalize_before_lookup() {
        for id in [
            "claude-opus-5",
            "CLAUDE-OPUS-5",
            "claude-opus-5[1m]",
            "claude-opus-5-eap",
            "openrouter/anthropic/claude-opus-5",
            "  claude-opus-5  ",
        ] {
            assert!(
                has_capability(id, ModelCapability::LeanPrompt),
                "{id} must resolve to claude-opus-5"
            );
        }
    }

    #[test]
    fn fast_mode_matches_the_table() {
        assert!(has_capability("claude-opus-4-7", ModelCapability::FastMode));
        assert!(has_capability("claude-opus-5", ModelCapability::FastMode));
        // fable-5's list deliberately omits fast_mode.
        assert!(!has_capability("claude-fable-5", ModelCapability::FastMode));
    }
}

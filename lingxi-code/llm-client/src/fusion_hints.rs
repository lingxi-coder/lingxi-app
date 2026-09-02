//! Checked-in Fusion model hints.
//!
//! Automatic quality/fast presets never infer rank from a model name or catalog
//! order. Rows whose `request_model` is missing from the live catalog are
//! skipped (the model stays ineligible). Unknown models default to
//! `fusion_hints: None`.

use platform_api::{FusionCostClass, FusionLatencyClass, FusionModelHints};

/// Look up checked-in hints for an exact `(profile_name, request_model)` pair.
#[must_use]
pub fn hints_for(profile_name: &str, request_model: &str) -> Option<FusionModelHints> {
    TABLE
        .iter()
        .find(|(profile, model, _)| *profile == profile_name && *model == request_model)
        .map(|(_, _, hints)| *hints)
}

/// Rows that are present in [`crate::catalog::builtin_presets`] / Anthropic
/// first-party profiles. Keep IDs byte-identical to the catalog; do not fuzzy-match.
const TABLE: &[(&str, &str, FusionModelHints)] = &[
    // Anthropic first-party
    (
        "anthropic",
        "claude-opus-5",
        q(
            100,
            FusionLatencyClass::Standard,
            FusionCostClass::High,
            true,
        ),
    ),
    (
        "anthropic",
        "claude-sonnet-5",
        q(
            90,
            FusionLatencyClass::Standard,
            FusionCostClass::Medium,
            true,
        ),
    ),
    (
        "anthropic",
        "claude-fable-5-1",
        q(105, FusionLatencyClass::Slow, FusionCostClass::High, true),
    ),
    (
        "anthropic",
        "claude-haiku-4-5",
        q(60, FusionLatencyClass::Fast, FusionCostClass::Low, false),
    ),
    // OpenAI
    (
        "openai",
        "gpt-5.6-sol",
        q(100, FusionLatencyClass::Slow, FusionCostClass::High, true),
    ),
    (
        "openai",
        "gpt-5.6-terra",
        q(
            90,
            FusionLatencyClass::Standard,
            FusionCostClass::Medium,
            true,
        ),
    ),
    (
        "openai",
        "gpt-5.6-luna",
        q(75, FusionLatencyClass::Fast, FusionCostClass::Low, false),
    ),
    (
        "openai-chatgpt",
        "gpt-5.6-sol",
        q(
            100,
            FusionLatencyClass::Slow,
            FusionCostClass::Subscription,
            true,
        ),
    ),
    (
        "openai-chatgpt",
        "gpt-5.6-terra",
        q(
            90,
            FusionLatencyClass::Standard,
            FusionCostClass::Subscription,
            true,
        ),
    ),
    (
        "openai-chatgpt",
        "gpt-5.6-luna",
        q(
            75,
            FusionLatencyClass::Fast,
            FusionCostClass::Subscription,
            false,
        ),
    ),
    // DeepSeek
    (
        "deepseek",
        "deepseek-v4-pro",
        q(
            90,
            FusionLatencyClass::Standard,
            FusionCostClass::Medium,
            true,
        ),
    ),
    (
        "deepseek",
        "deepseek-v4-flash",
        q(75, FusionLatencyClass::Fast, FusionCostClass::Low, true),
    ),
    // Kimi
    (
        "kimi",
        "kimi-k3",
        q(
            90,
            FusionLatencyClass::Standard,
            FusionCostClass::Medium,
            true,
        ),
    ),
    (
        "kimi",
        "kimi-k2.7-code-highspeed",
        q(75, FusionLatencyClass::Fast, FusionCostClass::Low, false),
    ),
    (
        "kimi-code",
        "k3-256k",
        q(
            90,
            FusionLatencyClass::Standard,
            FusionCostClass::Subscription,
            true,
        ),
    ),
    (
        "kimi-code",
        "kimi-for-coding-highspeed",
        q(
            75,
            FusionLatencyClass::Fast,
            FusionCostClass::Subscription,
            false,
        ),
    ),
    // GLM
    (
        "glm-coding",
        "glm-5.3",
        q(
            90,
            FusionLatencyClass::Standard,
            FusionCostClass::Subscription,
            true,
        ),
    ),
    (
        "glm-coding",
        "glm-5.2-highspeed",
        q(
            75,
            FusionLatencyClass::Fast,
            FusionCostClass::Subscription,
            true,
        ),
    ),
    (
        "zai",
        "glm-5.2",
        q(
            85,
            FusionLatencyClass::Standard,
            FusionCostClass::Medium,
            true,
        ),
    ),
    (
        "zai",
        "glm-5.3-flash",
        q(75, FusionLatencyClass::Fast, FusionCostClass::Low, true),
    ),
    // Gemini
    (
        "gemini",
        "gemini-3.1-pro-preview",
        q(
            90,
            FusionLatencyClass::Standard,
            FusionCostClass::Medium,
            true,
        ),
    ),
    (
        "gemini",
        "gemini-3.7-flash",
        q(75, FusionLatencyClass::Fast, FusionCostClass::Low, true),
    ),
    // GitHub Copilot (exact ids from the copilot slice)
    (
        "github-copilot",
        "claude-opus-5",
        q(
            100,
            FusionLatencyClass::Standard,
            FusionCostClass::Subscription,
            true,
        ),
    ),
    (
        "github-copilot",
        "claude-sonnet-5",
        q(
            90,
            FusionLatencyClass::Standard,
            FusionCostClass::Subscription,
            true,
        ),
    ),
    (
        "github-copilot",
        "claude-fable-5",
        q(
            75,
            FusionLatencyClass::Fast,
            FusionCostClass::Subscription,
            false,
        ),
    ),
    (
        "github-copilot",
        "gpt-5.6-sol",
        q(
            100,
            FusionLatencyClass::Slow,
            FusionCostClass::Subscription,
            true,
        ),
    ),
    (
        "github-copilot",
        "gpt-5.6-terra",
        q(
            90,
            FusionLatencyClass::Standard,
            FusionCostClass::Subscription,
            true,
        ),
    ),
    (
        "github-copilot",
        "gpt-5.6-luna",
        q(
            75,
            FusionLatencyClass::Fast,
            FusionCostClass::Subscription,
            false,
        ),
    ),
    (
        "github-copilot",
        "gemini-3.1-pro-preview",
        q(
            90,
            FusionLatencyClass::Standard,
            FusionCostClass::Subscription,
            true,
        ),
    ),
    (
        "github-copilot",
        "gemini-3.7-flash",
        q(
            75,
            FusionLatencyClass::Fast,
            FusionCostClass::Subscription,
            true,
        ),
    ),
    // OpenRouter (exact slice ids)
    (
        "openrouter",
        "anthropic/claude-sonnet-5",
        q(
            90,
            FusionLatencyClass::Standard,
            FusionCostClass::High,
            true,
        ),
    ),
    (
        "openrouter",
        "anthropic/claude-fable-5.1",
        q(105, FusionLatencyClass::Slow, FusionCostClass::High, true),
    ),
    (
        "openrouter",
        "openai/gpt-5.6-sol",
        q(100, FusionLatencyClass::Slow, FusionCostClass::High, true),
    ),
    (
        "openrouter",
        "openai/gpt-5.6-terra",
        q(
            90,
            FusionLatencyClass::Standard,
            FusionCostClass::Medium,
            true,
        ),
    ),
    (
        "openrouter",
        "google/gemini-3.1-pro-preview",
        q(
            90,
            FusionLatencyClass::Standard,
            FusionCostClass::Medium,
            true,
        ),
    ),
    (
        "openrouter",
        "deepseek/deepseek-v4-pro",
        q(
            90,
            FusionLatencyClass::Standard,
            FusionCostClass::Medium,
            true,
        ),
    ),
];

const fn q(
    quality_rank: u16,
    latency_class: FusionLatencyClass,
    cost_class: FusionCostClass,
    judge_eligible: bool,
) -> FusionModelHints {
    FusionModelHints {
        eligible: true,
        quality_rank,
        latency_class,
        cost_class,
        judge_eligible,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::builtin_presets;
    use crate::provider_settings::anthropic_model_profiles;

    #[test]
    fn unknown_ids_are_ineligible() {
        assert!(hints_for("anthropic", "not-a-real-model").is_none());
        assert!(hints_for("no-such-profile", "claude-opus-5").is_none());
    }

    #[test]
    fn table_ids_exist_in_catalog_or_anthropic_profiles() {
        let catalog = builtin_presets();
        let anthropic: Vec<String> = anthropic_model_profiles()
            .into_iter()
            .map(|m| m.request_model)
            .collect();
        for (profile, model, _) in TABLE {
            if *profile == "anthropic" {
                assert!(
                    anthropic.iter().any(|id| id == model),
                    "anthropic catalog missing `{model}`"
                );
                continue;
            }
            let provider = catalog
                .providers
                .iter()
                .find(|p| p.profile_name == *profile)
                .unwrap_or_else(|| panic!("missing profile `{profile}`"));
            assert!(
                provider.models.iter().any(|m| m.request_model == *model),
                "profile `{profile}` missing model `{model}`"
            );
        }
    }

    #[test]
    fn builtin_presets_receive_hints_for_table_rows() {
        let hints = hints_for("openai", "gpt-5.6-sol").expect("hinted");
        assert!(hints.eligible);
        assert_eq!(hints.quality_rank, 100);
        assert!(hints.judge_eligible);
        let catalog = builtin_presets();
        assert!(catalog.providers.iter().any(|p| {
            p.profile_name == "openai" && p.models.iter().any(|m| m.request_model == "gpt-5.6-sol")
        }));
    }

    #[test]
    fn unlisted_catalog_models_stay_ineligible() {
        let catalog = builtin_presets();
        let openai = catalog
            .providers
            .iter()
            .find(|p| p.profile_name == "openai")
            .unwrap();
        let unlisted = openai
            .models
            .iter()
            .find(|m| hints_for("openai", &m.request_model).is_none())
            .expect("openai slice has models outside the hint table");
        assert!(hints_for("openai", &unlisted.request_model).is_none());
    }
}

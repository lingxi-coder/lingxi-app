//! Translation helpers between `llm_client::Usage` and
//! `cost::Usage` plus model-string → `ProviderId` resolution.
//!
//! Used by M6-06 to feed `LlmResponse.usage` into `CostTracker`.

use cost::pricing::ProviderId;
use cost::usage::{ApiSpeed, ServerToolUsage, TokenUsage, Usage};
use cost::ModelRef;
use llm_client::Usage as LlmUsage;

/// The default profile name used for bare / `claude-*` model strings.
/// Mirrors `providers::model_spec::DEFAULT_PROFILE`.
const DEFAULT_PROFILE: &str = "anthropic";

/// Parse a model string into `(profile, bare_model)` for cost routing.
///
/// Replicates `providers::ModelSpec::parse` semantics:
/// - `"claude-*"` → profile `"anthropic"`, model = full string (back-compat).
/// - `"profile/model"` (non-empty both sides) → `(profile, model)`.
/// - Everything else (bare string, no `/`) → `("anthropic", full string)`.
fn split_profile(model: &str) -> (String, String) {
    if model.starts_with("claude-") {
        return (DEFAULT_PROFILE.to_string(), model.to_string());
    }
    match model.split_once('/') {
        Some((profile, bare)) if !profile.is_empty() && !bare.is_empty() => {
            (profile.to_string(), bare.to_string())
        }
        _ => (DEFAULT_PROFILE.to_string(), model.to_string()),
    }
}

/// Translate an `llm_client::Usage` into the cost crate's `Usage` shape.
///
/// Maps `billable_tokens.cache_write` → `TokenUsage::cache_write`
/// and `billable_tokens.cache_read` → `TokenUsage::cache_read`.
///
/// Also surfaces the two cost-side billing signals the API response carries:
/// `server_tool_use.web_search_requests` (billed per request — COST.5) and the
/// `speed` tier, where `"fast"` maps to [`ApiSpeed::Fast`] so the Opus-4.6
/// fast-mode rates fire (COST.3). Any non-`"fast"` speed string maps to
/// [`ApiSpeed::Standard`]; an absent `speed`/`server_tool_use` stays `None`,
/// matching claude-code (`utils/cost-tracker.ts:282`, `utils/modelCost.ts:139`).
#[must_use]
pub(crate) fn llm_usage_to_cost_usage(usage: &LlmUsage) -> Usage {
    Usage {
        tokens: TokenUsage {
            input: usage.billable_tokens.input,
            output: usage.billable_tokens.output,
            cache_write: usage.billable_tokens.cache_write,
            cache_read: usage.billable_tokens.cache_read,
            reasoning_output: usage.billable_tokens.reasoning_output,
        },
        server_tool_use: usage.server_tool_use.map(|s| ServerToolUsage {
            // cost's counter is u32; clamp the (u64) wire value defensively.
            web_search_requests: u32::try_from(s.web_search_requests).unwrap_or(u32::MAX),
        }),
        speed: usage.speed.as_deref().map(|s| {
            if s == "fast" {
                ApiSpeed::Fast
            } else {
                ApiSpeed::Standard
            }
        }),
    }
}

/// Map a provider-profile name to its cost [`ProviderId`].
///
/// Mirrors the registry's choice: built-in `anthropic`/`openai`/`gemini` map
/// to their first-party ids. The managed-cloud profiles map to the price
/// table that applies: `bedrock` has its own (`AmazonBedrock`); `vertex`
/// reuses Gemini list prices (Vertex *is* Gemini); `azure` reuses `OpenAI`
/// list prices (wire-compatible). Any other (settings-declared) profile name
/// is an `OpenAI`-compatible endpoint.
#[must_use]
fn provider_id_for_profile(profile: &str) -> ProviderId {
    match profile {
        "anthropic" => ProviderId::Anthropic,
        // `azure` reuses OpenAI list prices (wire-compatible).
        "openai" | "azure" => ProviderId::OpenAI,
        // Vertex *is* Gemini — reuse the Gemini price table.
        "gemini" | "vertex" => ProviderId::GoogleGemini,
        // Bedrock has its own price table.
        "bedrock" => ProviderId::AmazonBedrock,
        other => ProviderId::OpenAICompatible {
            name: other.to_string(),
        },
    }
}

/// Build a fully-qualified [`ModelRef`] from a model string: the prefix selects
/// the provider, and the local model id (prefix stripped) is what the price
/// catalog is keyed on. `claude-*` / bare strings keep the full string as the
/// model id, so Anthropic cost attribution is byte-identical to before.
#[must_use]
pub(crate) fn model_ref_from_string(model: &str) -> ModelRef {
    let (profile, bare) = split_profile(model);
    ModelRef {
        provider: provider_id_for_profile(&profile),
        model: bare,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm_client::{ServerToolUsage as LlmServerToolUsage, TokenUsage as LlmTokenUsage};

    /// Resolve a model-name string to its `ProviderId` by parsing the
    /// `provider/model` prefix. Only needed in tests — the production path goes
    /// through `model_ref_from_string`.
    fn provider_from_model(model: &str) -> ProviderId {
        let (profile, _) = split_profile(model);
        provider_id_for_profile(&profile)
    }

    // --- split_profile tests (ported from providers::model_spec::tests) -----

    #[test]
    fn split_prefixed_splits_profile_and_model() {
        let (p, m) = split_profile("openai/gpt-4o");
        assert_eq!(p, "openai");
        assert_eq!(m, "gpt-4o");
    }

    #[test]
    fn split_bare_string_is_anthropic_backcompat() {
        let (p, m) = split_profile("claude-opus-4-7");
        assert_eq!(p, "anthropic");
        assert_eq!(m, "claude-opus-4-7");
    }

    #[test]
    fn split_claude_with_slash_stays_anthropic() {
        // A claude model id is never reinterpreted as profile/model.
        let (p, m) = split_profile("claude-3-5/sonnet");
        assert_eq!(p, "anthropic");
        assert_eq!(m, "claude-3-5/sonnet");
    }

    #[test]
    fn split_non_claude_no_slash_is_anthropic_profile() {
        let (p, m) = split_profile("some-model");
        assert_eq!(p, "anthropic");
        assert_eq!(m, "some-model");
    }

    #[test]
    fn split_custom_profile_name() {
        let (p, m) = split_profile("groq/llama-3.3-70b");
        assert_eq!(p, "groq");
        assert_eq!(m, "llama-3.3-70b");
    }

    fn make_llm_usage(
        input: u64, output: u64, cache_write: u64, cache_read: u64,
        server_tool_use: Option<LlmServerToolUsage>,
        speed: Option<String>,
    ) -> LlmUsage {
        LlmUsage {
            billable_tokens: LlmTokenUsage {
                input,
                output,
                cache_write,
                cache_read,
                reasoning_output: 0,
            },
            server_tool_use,
            speed,
            ..Default::default()
        }
    }

    #[test]
    fn translates_tokens_one_to_one() {
        let usage = make_llm_usage(100, 50, 20, 10, None, None);
        let u = llm_usage_to_cost_usage(&usage);
        assert_eq!(u.tokens.input, 100);
        assert_eq!(u.tokens.output, 50);
        assert_eq!(u.tokens.cache_write, 20);
        assert_eq!(u.tokens.cache_read, 10);
        assert_eq!(u.tokens.reasoning_output, 0);
        // Absent server_tool_use / speed default to None — no regression.
        assert!(u.server_tool_use.is_none());
        assert!(u.speed.is_none());
    }

    use cost::pricing::{ModelRef, PricingCatalog, ProviderId};
    use cost::{ApiSpeed, CostCalculator};

    fn opus_4_6_pricing() -> cost::ModelPricing {
        let c = PricingCatalog::builtin_reference();
        let mr = ModelRef {
            provider: ProviderId::Anthropic,
            model: "claude-opus-4-6".into(),
        };
        c.resolve(&mr).unwrap().0
    }

    #[test]
    fn web_search_requests_thread_through_and_bill_one_cent_each() {
        // COST.5: usage.server_tool_use.web_search_requests on the wire → cost
        // Usage → billed at $0.01 (10_000_000 nano-USD) per request.
        let usage = make_llm_usage(0, 0, 0, 0, Some(LlmServerToolUsage { web_search_requests: 3 }), None);
        let u = llm_usage_to_cost_usage(&usage);
        assert_eq!(u.server_tool_use.unwrap().web_search_requests, 3);
        // No tokens → only the web-search charge: 3 × $0.01 = 30_000_000 nano-USD.
        assert_eq!(
            CostCalculator::calculate_nano_usd(&u, &opus_4_6_pricing()),
            30_000_000
        );
    }

    #[test]
    fn speed_fast_threads_through_and_bills_opus_4_6_fast_tier() {
        // COST.3: usage.speed == "fast" → cost ApiSpeed::Fast → Opus-4.6
        // rebills at the $30/$150 fast tier instead of the catalog $5/$25.
        let usage = make_llm_usage(1_000_000, 0, 0, 0, None, Some("fast".to_string()));
        let u = llm_usage_to_cost_usage(&usage);
        assert_eq!(u.speed, Some(ApiSpeed::Fast));
        // 1M input × $30/Mtok = 30e9 nano-USD.
        assert_eq!(
            CostCalculator::calculate_nano_usd(&u, &opus_4_6_pricing()),
            30_000_000_000
        );
    }

    #[test]
    fn non_fast_speed_maps_to_standard_and_keeps_base_tier() {
        // A non-"fast" speed string maps to Standard (explicitly not fast), so
        // Opus-4.6 stays on the $5/$25 catalog tier.
        let usage = make_llm_usage(1_000_000, 0, 0, 0, None, Some("standard".to_string()));
        let u = llm_usage_to_cost_usage(&usage);
        assert_eq!(u.speed, Some(ApiSpeed::Standard));
        assert_eq!(
            CostCalculator::calculate_nano_usd(&u, &opus_4_6_pricing()),
            5_000_000_000
        );
    }

    #[test]
    fn provider_from_model_maps_prefixes() {
        assert_eq!(
            provider_from_model("claude-opus-4-7"),
            ProviderId::Anthropic
        );
        assert_eq!(
            provider_from_model("anthropic/claude-opus-4-7"),
            ProviderId::Anthropic
        );
        assert_eq!(provider_from_model("openai/gpt-4o"), ProviderId::OpenAI);
        assert_eq!(
            provider_from_model("gemini/gemini-2.0-flash"),
            ProviderId::GoogleGemini
        );
        assert_eq!(
            provider_from_model("some-bare-model"),
            ProviderId::Anthropic
        );
        assert_eq!(
            provider_from_model("groq/llama-3.3-70b"),
            ProviderId::OpenAICompatible {
                name: "groq".to_string()
            }
        );
    }

    #[test]
    fn managed_cloud_profiles_map_to_priced_providers() {
        // Bedrock has its own price table; Vertex reuses Gemini; Azure reuses OpenAI.
        assert_eq!(
            provider_from_model("bedrock/anthropic.claude-3-5-sonnet-20241022-v2:0"),
            ProviderId::AmazonBedrock
        );
        assert_eq!(
            provider_from_model("vertex/gemini-2.0-flash"),
            ProviderId::GoogleGemini
        );
        assert_eq!(provider_from_model("azure/gpt-4o"), ProviderId::OpenAI);
    }

    #[test]
    fn model_ref_strips_prefix_for_priced_lookup() {
        // Prefixed → provider + stripped local id (matches price-table keys).
        let mr = model_ref_from_string("openai/gpt-4o");
        assert_eq!(mr.provider, ProviderId::OpenAI);
        assert_eq!(mr.model, "gpt-4o");
        // Anthropic back-compat: full string kept as the model id.
        let mr = model_ref_from_string("claude-opus-4-7");
        assert_eq!(mr.provider, ProviderId::Anthropic);
        assert_eq!(mr.model, "claude-opus-4-7");
    }
}

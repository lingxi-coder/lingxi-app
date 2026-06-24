//! Translation helpers between `llm_client::Usage` and
//! `cost::Usage` plus model-string → `ProviderId` resolution.
//!
//! Also contains the bridge that populates an `llm_client::PricingCatalog` from
//! the `cost::PricingCatalog` so `LlmResponse.cost` carries real estimates.
//!
//! Used by M6-06 to feed `LlmResponse.usage` into `CostTracker`.

use cost::pricing::{ProviderId, TokenClass};
use cost::usage::{ApiSpeed, ServerToolUsage, TokenUsage, Usage};
use cost::ModelRef;
use llm_client::Usage as LlmUsage;

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
            cache_write_1h: 0,
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

fn llm_provider_to_cost_provider(provider: &llm_client::ProviderId) -> ProviderId {
    match provider {
        llm_client::ProviderId::AnthropicFirstParty => ProviderId::Anthropic,
        llm_client::ProviderId::OpenAI | llm_client::ProviderId::AzureOpenAI => ProviderId::OpenAI,
        llm_client::ProviderId::Gemini
        | llm_client::ProviderId::VertexGemini
        | llm_client::ProviderId::VertexClaude => ProviderId::GoogleGemini,
        llm_client::ProviderId::BedrockClaude => ProviderId::AmazonBedrock,
        llm_client::ProviderId::OpenAICompatible { name } => ProviderId::OpenAICompatible {
            name: name.clone(),
        },
        llm_client::ProviderId::Custom { name } => ProviderId::Custom { name: name.clone() },
    }
}

/// Build a fully-qualified [`ModelRef`] from a model string: the prefix selects
/// the provider, and the local model id (prefix stripped) is what the price
/// catalog is keyed on. `claude-*` / bare strings keep the full string as the
/// model id, so Anthropic cost attribution is byte-identical to before.
#[must_use]
pub(crate) fn model_ref_from_string(model: &str) -> ModelRef {
    let (profile, bare) = llm_client::split_profile_model(model);
    let pricing_provider = llm_client::pricing_provider_id_for_profile(
        &profile,
        &llm_client::ProviderId::OpenAICompatible {
            name: profile.clone(),
        },
    );
    ModelRef {
        provider: llm_provider_to_cost_provider(&pricing_provider),
        model: bare,
    }
}

/// Map a `cost::pricing::ProviderId` to its `llm_client::ProviderId` equivalent.
///
/// Mapping:
/// - `Anthropic` → `AnthropicFirstParty`
/// - `OpenAI` → `OpenAI`
/// - `GoogleGemini` → `Gemini`
/// - `AmazonBedrock` → `BedrockClaude`
/// - `OpenAICompatible { name }` → `OpenAICompatible { name }`
/// - `Custom { name }` → `Custom { name }`
fn cost_provider_to_llm_provider(p: &ProviderId) -> llm_client::ProviderId {
    match p {
        ProviderId::Anthropic => llm_client::ProviderId::AnthropicFirstParty,
        ProviderId::OpenAI => llm_client::ProviderId::OpenAI,
        ProviderId::GoogleGemini => llm_client::ProviderId::Gemini,
        ProviderId::AmazonBedrock => llm_client::ProviderId::BedrockClaude,
        ProviderId::OpenAICompatible { name } => llm_client::ProviderId::OpenAICompatible {
            name: name.clone(),
        },
        ProviderId::Custom { name } => llm_client::ProviderId::Custom { name: name.clone() },
    }
}

/// Build an `llm_client::PricingCatalog` populated from a `cost::PricingCatalog`.
///
/// Conversion: for each [`cost::ModelPricing`] entry, the `billing_model` is the
/// catalog key (the stripped model name the cost crate uses, e.g. `"claude-opus-4-6"`),
/// and per-bucket rates are converted from **nano-USD per token** to
/// **USD per million tokens** via `usd_per_million = nano_usd_per_token as f64 / 1000.0`.
///
/// Missing token classes in a cost entry produce `0.0` for that bucket in the
/// llm-client `TokenPricing` (never an error).  Provider defaults are not
/// iterable from the cost catalog and are omitted; only explicitly-keyed model
/// entries are transferred.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn llm_catalog_from_cost(
    catalog: &cost::pricing::PricingCatalog,
) -> llm_client::PricingCatalog {
    let mut out = llm_client::PricingCatalog::empty();
    for entry in catalog.entries() {
        let provider = cost_provider_to_llm_provider(&entry.model_ref.provider);
        let billing_model = entry.model_ref.model.clone();
        let nano_to_usd = |class: TokenClass| -> f64 {
            entry
                .token_rates
                .get(&class)
                .map_or(0.0, |m| m.nano_usd_per_token as f64 / 1000.0)
        };
        let pricing = llm_client::TokenPricing {
            input_per_million: nano_to_usd(TokenClass::Input),
            output_per_million: nano_to_usd(TokenClass::Output),
            cache_write_per_million: nano_to_usd(TokenClass::CacheWrite),
            cache_read_per_million: nano_to_usd(TokenClass::CacheRead),
            reasoning_per_million: nano_to_usd(TokenClass::ReasoningOutput),
        };
        out = out.with_price(provider, billing_model, pricing);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm_client::{ServerToolUsage as LlmServerToolUsage, TokenUsage as LlmTokenUsage};

    /// Resolve a model-name string to its `ProviderId` by parsing the
    /// `provider/model` prefix. Only needed in tests — the production path goes
    /// through `model_ref_from_string`.
    fn provider_from_model(model: &str) -> ProviderId {
        let (profile, _) = llm_client::split_profile_model(model);
        let llm_provider = llm_client::pricing_provider_id_for_profile(
            &profile,
            &llm_client::ProviderId::OpenAICompatible {
                name: profile.clone(),
            },
        );
        llm_provider_to_cost_provider(&llm_provider)
    }

    // --- split_profile_model tests (ported from providers::model_spec::tests) -----

    #[test]
    fn split_prefixed_splits_profile_and_model() {
        let (p, m) = llm_client::split_profile_model("openai/gpt-4o");
        assert_eq!(p, "openai");
        assert_eq!(m, "gpt-4o");
    }

    #[test]
    fn split_bare_string_is_anthropic_backcompat() {
        let (p, m) = llm_client::split_profile_model("claude-opus-4-7");
        assert_eq!(p, "anthropic");
        assert_eq!(m, "claude-opus-4-7");
    }

    #[test]
    fn split_claude_with_slash_stays_anthropic() {
        // A claude model id is never reinterpreted as profile/model.
        let (p, m) = llm_client::split_profile_model("claude-3-5/sonnet");
        assert_eq!(p, "anthropic");
        assert_eq!(m, "claude-3-5/sonnet");
    }

    #[test]
    fn split_non_claude_no_slash_is_anthropic_profile() {
        let (p, m) = llm_client::split_profile_model("some-model");
        assert_eq!(p, "anthropic");
        assert_eq!(m, "some-model");
    }

    #[test]
    fn split_custom_profile_name() {
        let (p, m) = llm_client::split_profile_model("groq/llama-3.3-70b");
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

    // ── 3c-T3: llm_catalog_from_cost bridge conversion tests ────────────────

    /// Pinned conversion: `claude-opus-4-6` input rate is `5_000` `nano_usd/token`
    /// (= `5_000 / 1_000` = `5.0` `usd/million`).  Output is `25_000` nano → `25.0` `usd/M`.
    /// Cache-write is `6_250` → `6.25` `usd/M`.  Cache-read is `500` → `0.5` `usd/M`.
    #[test]
    fn bridge_opus_4_6_converts_exact_rates() {
        use cost::pricing::PricingCatalog as CostCatalog;
        use llm_client::{CostEstimator, PricingPolicy, Usage as LlmUsage, TokenUsage};

        let cost_cat = CostCatalog::builtin_reference();
        let llm_cat = llm_catalog_from_cost(&cost_cat);

        // Build an estimator with MarkUnestimated so unknown models return None-cost.
        let estimator = CostEstimator::new(llm_cat, PricingPolicy::MarkUnestimated);

        // Construct the PricingModelRef that the estimator needs.
        // billing_model is the raw model name (no prefix) as stored in the catalog.
        let pricing_ref = llm_client::PricingModelRef {
            pricing_provider_id: llm_client::ProviderId::AnthropicFirstParty,
            billing_model: "claude-opus-4-6".to_string(),
            request_model: "claude-opus-4-6".to_string(),
            display_model: "Claude Opus 4.6".to_string(),
        };
        let usage = LlmUsage {
            billable_tokens: TokenUsage {
                input: 1_000_000,
                output: 1_000_000,
                cache_write: 0,
                cache_read: 0,
                reasoning_output: 0,
            },
            ..Default::default()
        };
        let estimate = estimator.estimate(pricing_ref, &usage).expect("opus-4-6 must be priced");
        // 1M input × $5.0/M = $5.0
        let total = estimate.total_cost_usd.expect("total_cost_usd must be Some");
        let expected = 5.0 + 25.0; // input + output
        assert!(
            (total - expected).abs() < 1e-9,
            "expected total ${expected}, got ${total}"
        );
        let input = estimate.input_cost_usd.unwrap();
        assert!(
            (input - 5.0).abs() < 1e-9,
            "input cost must be $5.0/M, got ${input}"
        );
        let output = estimate.output_cost_usd.unwrap();
        assert!(
            (output - 25.0).abs() < 1e-9,
            "output cost must be $25.0/M, got ${output}"
        );
    }

    /// Unknown model → cost stays `None` (`MarkUnestimated` policy).
    #[test]
    fn bridge_unknown_model_returns_unestimated() {
        use cost::pricing::PricingCatalog as CostCatalog;
        use llm_client::{CostEstimator, PricingPolicy, Usage as LlmUsage};

        let cost_cat = CostCatalog::builtin_reference();
        let llm_cat = llm_catalog_from_cost(&cost_cat);
        let estimator = CostEstimator::new(llm_cat, PricingPolicy::MarkUnestimated);

        let pricing_ref = llm_client::PricingModelRef {
            pricing_provider_id: llm_client::ProviderId::AnthropicFirstParty,
            billing_model: "claude-nonexistent-9999".to_string(),
            request_model: "claude-nonexistent-9999".to_string(),
            display_model: "Claude Nonexistent".to_string(),
        };
        let estimate = estimator
            .estimate(pricing_ref, &LlmUsage::default())
            .expect("MarkUnestimated must not error");
        // Unpriced → total_cost_usd is None.
        assert!(
            estimate.total_cost_usd.is_none(),
            "unpriced model must yield None total_cost_usd"
        );
        assert!(!estimate.estimated, "estimated flag must be false for unpriced");
    }

    /// Provider mapping: `OpenAI` `gpt-4o` converts at the expected rates.
    #[test]
    fn bridge_openai_gpt4o_maps_to_correct_provider() {
        use cost::pricing::PricingCatalog as CostCatalog;
        use llm_client::{CostEstimator, PricingPolicy, Usage as LlmUsage, TokenUsage};

        let cost_cat = CostCatalog::builtin_reference();
        let llm_cat = llm_catalog_from_cost(&cost_cat);
        let estimator = CostEstimator::new(llm_cat, PricingPolicy::MarkUnestimated);

        let pricing_ref = llm_client::PricingModelRef {
            pricing_provider_id: llm_client::ProviderId::OpenAI,
            billing_model: "gpt-4o".to_string(),
            request_model: "gpt-4o".to_string(),
            display_model: "GPT-4o".to_string(),
        };
        let usage = LlmUsage {
            billable_tokens: TokenUsage {
                input: 1_000_000,
                ..Default::default()
            },
            ..Default::default()
        };
        let estimate = estimator.estimate(pricing_ref, &usage).expect("gpt-4o must be priced");
        // gpt-4o input = 2_500 nano_usd/token → 2.5 usd/M
        let input = estimate.input_cost_usd.expect("input_cost_usd must be Some");
        assert!(
            (input - 2.5).abs() < 1e-9,
            "gpt-4o input must be $2.5/M, got ${input}"
        );
    }
}

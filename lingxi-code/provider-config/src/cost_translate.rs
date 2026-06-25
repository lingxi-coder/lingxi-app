//! Build the `cost::PricingCatalog` returned by `assemble` from the merged
//! provider profiles (Plan 3c §8). Anthropic / `OpenAI` / Gemini reference tiers
//! come from `cost::PricingCatalog::builtin_reference()`; any other profile model
//! is priced from its **real** models.dev rates (via the llm-client preset
//! pricing catalog). Only a model with no reference tier AND no models.dev price
//! falls back to the default-unknown ($5/$25) row so the turn is priced, not
//! errored — instead of mis-billing every non-Anthropic model at the Claude
//! Opus tier (which over-charged e.g. deepseek-chat ~36×/~89×).

use std::collections::HashMap;

use llm_client::{ProviderId as LlmProviderId, ProviderProfile, TokenPricing};

use cost::pricing::{
    MoneyPerToken, NonTokenBillableUnit, PricingSource, ProviderId as CostProviderId, TokenClass,
};
use cost::{ModelPricing, ModelRef, PricingCatalog};

/// Convert a per-million-token USD price to nano-USD per token.
/// `$X / Mtok` = `X * 1000` nano-USD per token (1 USD = 1e9 nano-USD; 1 Mtok = 1e6 tokens).
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn nano_per_token(usd_per_million: f64) -> u64 {
    (usd_per_million * 1000.0).round().max(0.0) as u64
}

/// Build a real `cost::ModelPricing` from the models.dev [`TokenPricing`].
fn model_pricing_from_token_pricing(mr: &ModelRef, tp: &TokenPricing) -> ModelPricing {
    let mut rates: HashMap<TokenClass, MoneyPerToken> = HashMap::new();
    rates.insert(
        TokenClass::Input,
        MoneyPerToken {
            nano_usd_per_token: nano_per_token(tp.input_per_million),
        },
    );
    rates.insert(
        TokenClass::Output,
        MoneyPerToken {
            nano_usd_per_token: nano_per_token(tp.output_per_million),
        },
    );
    rates.insert(
        TokenClass::CacheWrite,
        MoneyPerToken {
            nano_usd_per_token: nano_per_token(tp.cache_write_per_million),
        },
    );
    rates.insert(
        TokenClass::CacheRead,
        MoneyPerToken {
            nano_usd_per_token: nano_per_token(tp.cache_read_per_million),
        },
    );
    // Only meter a separate reasoning bucket when the model actually prices one
    // (most bill reasoning at the output rate → field is 0.0 → omitted).
    if tp.reasoning_per_million > 0.0 {
        rates.insert(
            TokenClass::ReasoningOutput,
            MoneyPerToken {
                nano_usd_per_token: nano_per_token(tp.reasoning_per_million),
            },
        );
    }
    ModelPricing {
        model_ref: mr.clone(),
        token_rates: rates,
        // models.dev non-Anthropic models bill only tokens; no per-request units.
        non_token_rates_nano_usd: HashMap::<NonTokenBillableUnit, u64>::new(),
        effective_from: None,
        source: PricingSource::BuiltInReference {
            provider: mr.provider.clone(),
        },
    }
}

fn cost_provider_id(profile_name: &str, provider_id: &LlmProviderId) -> CostProviderId {
    let pricing_provider =
        llm_client::pricing_provider_id_for_profile(profile_name, provider_id);
    match pricing_provider {
        LlmProviderId::AnthropicFirstParty => CostProviderId::Anthropic,
        LlmProviderId::OpenAI | LlmProviderId::AzureOpenAI => CostProviderId::OpenAI,
        LlmProviderId::Gemini | LlmProviderId::VertexGemini | LlmProviderId::VertexClaude => {
            CostProviderId::GoogleGemini
        }
        LlmProviderId::BedrockClaude => CostProviderId::AmazonBedrock,
        LlmProviderId::OpenAICompatible { name } | LlmProviderId::Custom { name } => {
            CostProviderId::OpenAICompatible { name }
        }
    }
}

/// Build the cost catalog: reference tiers + a default-unknown row for every
/// non-Anthropic profile model the reference catalog does not already price.
#[must_use]
pub fn pricing_for(providers: &[ProviderProfile]) -> PricingCatalog {
    let mut catalog = PricingCatalog::builtin_reference();
    // Real per-model rates from the bundled models.dev slices (deepseek, openai,
    // openrouter, zai/glm, github-copilot, …), keyed by (llm ProviderId, id).
    let preset_pricing = llm_client::builtin_presets().pricing;
    for profile in providers {
        if profile.profile_name == "anthropic" {
            continue; // Anthropic tiers already in builtin_reference.
        }
        let provider = cost_provider_id(&profile.profile_name, &profile.provider_id);
        for model in &profile.models {
            let mr = ModelRef {
                provider: provider.clone(),
                model: model.billing_model.clone(),
            };
            if catalog.resolve(&mr).is_ok() {
                continue; // already priced by the reference catalog.
            }
            // Prefer the model's real models.dev price over the $5/$25 default.
            if let Some(tp) = preset_pricing.get(&profile.provider_id, &model.billing_model) {
                catalog = catalog.with_entry(model_pricing_from_token_pricing(&mr, &tp));
                continue;
            }
            // No reference tier and no models.dev price → priced, not errored.
            catalog = catalog.with_entry(ModelPricing {
                model_ref: mr.clone(),
                ..PricingCatalog::default_unknown_pricing(&mr)
            });
        }
    }
    catalog
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm_client::{
        AuthStrategy, Capabilities, CredentialConfig, ModelProfile, PricingConfig, ProtocolFamily,
    };

    fn user_profile(name: &str, model: &str) -> ProviderProfile {
        ProviderProfile {
            provider_id: LlmProviderId::OpenAICompatible {
                name: name.to_string(),
            },
            profile_name: name.to_string(),
            base_url: "https://x".to_string(),
            protocol: ProtocolFamily::OpenAiChat,
            auth: AuthStrategy::Bearer,
            credential: CredentialConfig::Static {
                id: name.to_string(),
            },
            models: vec![ModelProfile {
                display_model: model.to_string(),
                request_model: model.to_string(),
                billing_model: model.to_string(),
                aliases: Vec::new(),
                capabilities: Capabilities::default(),
            }],
            pricing: PricingConfig::default(),
            signing: None,
            azure: None,
            supports_websockets: false,
            supports_websocket_compression: false,
            websocket_connect_timeout_ms: None,
        }
    }

    #[test]
    fn anthropic_reference_tiers_preserved() {
        let cat = pricing_for(&[]);
        let opus = ModelRef {
            provider: CostProviderId::Anthropic,
            model: "claude-opus-4-6".to_string(),
        };
        assert!(cat.resolve(&opus).is_ok());
    }

    #[test]
    fn preset_model_uses_real_models_dev_price_not_default_unknown() {
        // Feed the real bundled presets through pricing_for; deepseek-chat must
        // bill at its true $0.14/$0.28 rate, NOT the $5/$25 Claude default.
        let providers = llm_client::builtin_presets().providers;
        let cat = pricing_for(&providers);
        let mr = ModelRef {
            provider: CostProviderId::OpenAICompatible {
                name: "deepseek".to_string(),
            },
            model: "deepseek-chat".to_string(),
        };
        let (p, _res) = cat.resolve(&mr).expect("priced");
        assert_eq!(
            p.token_rates[&cost::pricing::TokenClass::Input].nano_usd_per_token,
            140
        );
        assert_eq!(
            p.token_rates[&cost::pricing::TokenClass::Output].nano_usd_per_token,
            280
        );
    }

    #[test]
    fn gemini_model_uses_real_price_not_default_unknown() {
        // M6: with the Gemini slice vendored, gemini-2.5-pro (not in
        // builtin_reference) bills at its real $1.25/$10 rate, not $5/$25.
        let providers = llm_client::builtin_presets().providers;
        let cat = pricing_for(&providers);
        let mr = ModelRef {
            provider: CostProviderId::GoogleGemini,
            model: "gemini-2.5-pro".to_string(),
        };
        let (p, _res) = cat.resolve(&mr).expect("priced");
        assert_eq!(
            p.token_rates[&cost::pricing::TokenClass::Input].nano_usd_per_token,
            1_250
        );
        assert_eq!(
            p.token_rates[&cost::pricing::TokenClass::Output].nano_usd_per_token,
            10_000
        );
    }

    #[test]
    fn unpriced_user_model_gets_default_unknown_row() {
        let cat = pricing_for(&[user_profile("groq", "llama-3.3-70b")]);
        let mr = ModelRef {
            provider: CostProviderId::OpenAICompatible {
                name: "groq".to_string(),
            },
            model: "llama-3.3-70b".to_string(),
        };
        let (p, _res) = cat.resolve(&mr).expect("priced");
        // $5/$25 default-unknown tier.
        assert_eq!(
            p.token_rates[&cost::pricing::TokenClass::Input].nano_usd_per_token,
            5_000
        );
        assert_eq!(
            p.token_rates[&cost::pricing::TokenClass::Output].nano_usd_per_token,
            25_000
        );
    }
}

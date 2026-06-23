//! Build the `cost::PricingCatalog` returned by `assemble` from the merged
//! provider profiles (Plan 3c §8). Anthropic / `OpenAI` / Gemini reference tiers
//! come from `cost::PricingCatalog::builtin_reference()`; any other profile model
//! that the reference catalog does not already price gets an explicit
//! default-unknown ($5/$25) row so non-Anthropic turns are priced, not errored.

use llm_client::{ProviderId as LlmProviderId, ProviderProfile};

use cost::pricing::ProviderId as CostProviderId;
use cost::{ModelPricing, ModelRef, PricingCatalog};

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

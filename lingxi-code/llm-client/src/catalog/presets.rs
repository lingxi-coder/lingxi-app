//! Built-in provider presets: vendored models.dev metadata + hand-authored
//! routing (base URL, protocol, auth, credential). The routing table is the
//! source of truth for wire/auth and overrides the snapshot's advisory `api`.

use crate::catalog::map::{to_metadata, to_model_profile, to_pricing};
use crate::catalog::models_dev::ProviderSlice;
use crate::{
    AuthStrategy, CredentialConfig, PricingCatalog, ProtocolFamily, ProviderId, ProviderProfile,
};
use traits::{ModelBillingMode, ModelPricing};

/// Built-in catalog: provider profiles plus a matching pricing catalog.
#[derive(Debug, Clone)]
pub struct BuiltinCatalog {
    /// Provider profiles ready to merge into [`crate::ClientConfig`].
    pub providers: Vec<ProviderProfile>,
    /// Pricing entries ready to merge into a [`PricingCatalog`].
    pub pricing: PricingCatalog,
}

/// One hand-authored routing entry bound to a vendored slice.
struct Preset {
    /// Profile + registry name (stable, user-facing).
    profile_name: &'static str,
    /// Routing base URL (overrides the snapshot's `api`).
    base_url: &'static str,
    /// Wire protocol family.
    protocol: ProtocolFamily,
    /// Auth application strategy.
    auth: AuthStrategy,
    /// Provider identity used for pricing + serialization.
    provider_id: ProviderId,
    /// Credential lookup (env var name, or `None` for OAuth-based presets).
    credential_env: Option<&'static str>,
    /// User-facing billing semantics for this provider route.
    billing_mode: ModelBillingMode,
    /// Embedded models.dev slice JSON.
    slice_json: &'static str,
}

const OPENROUTER: &str = include_str!("../../data/models-dev/openrouter.json");
const DEEPSEEK: &str = include_str!("../../data/models-dev/deepseek.json");
const KIMI: &str = include_str!("../../data/models-dev/kimi.json");
const KIMI_CODE: &str = include_str!("../../data/models-dev/kimi-code.json");
const GLM_CODING: &str = include_str!("../../data/models-dev/zhipuai-coding-plan.json");
const ZAI: &str = include_str!("../../data/models-dev/zai.json");
const OPENAI: &str = include_str!("../../data/models-dev/openai.json");
const OPENAI_CHATGPT: &str = include_str!("../../data/models-dev/openai-chatgpt.json");
const GITHUB_COPILOT: &str = include_str!("../../data/models-dev/github-copilot.json");
const GEMINI: &str = include_str!("../../data/models-dev/gemini.json");

fn presets() -> Vec<Preset> {
    vec![
        Preset {
            profile_name: "openrouter",
            base_url: "https://openrouter.ai/api/v1",
            protocol: ProtocolFamily::OpenAiChat,
            auth: AuthStrategy::ApiKey,
            provider_id: ProviderId::OpenAICompatible {
                name: "openrouter".to_string(),
            },
            credential_env: Some("OPENROUTER_API_KEY"),
            billing_mode: ModelBillingMode::PerToken,
            slice_json: OPENROUTER,
        },
        Preset {
            profile_name: "deepseek",
            base_url: "https://api.deepseek.com",
            protocol: ProtocolFamily::OpenAiChat,
            auth: AuthStrategy::ApiKey,
            provider_id: ProviderId::OpenAICompatible {
                name: "deepseek".to_string(),
            },
            credential_env: Some("DEEPSEEK_API_KEY"),
            billing_mode: ModelBillingMode::PerToken,
            slice_json: DEEPSEEK,
        },
        // Kimi Open Platform (China): OpenAI-compatible Chat Completions wire
        // with bearer API-key auth. Keep the stable user-facing profile id
        // `kimi` even though models.dev calls this source `moonshotai-cn`.
        Preset {
            profile_name: "kimi",
            base_url: "https://api.moonshot.cn/v1",
            protocol: ProtocolFamily::OpenAiChat,
            auth: AuthStrategy::ApiKey,
            provider_id: ProviderId::OpenAICompatible {
                name: "kimi".to_string(),
            },
            credential_env: Some("MOONSHOT_API_KEY"),
            billing_mode: ModelBillingMode::PerToken,
            slice_json: KIMI,
        },
        // Kimi Code is a distinct membership-backed service. Its API keys,
        // model ids, quotas, and endpoint are not interchangeable with the
        // pay-as-you-go Kimi Open Platform profile above.
        Preset {
            profile_name: "kimi-code",
            base_url: "https://api.kimi.com/coding/v1",
            protocol: ProtocolFamily::OpenAiChat,
            auth: AuthStrategy::ApiKey,
            provider_id: ProviderId::OpenAICompatible {
                name: "kimi-code".to_string(),
            },
            credential_env: Some("KIMI_API_KEY"),
            billing_mode: ModelBillingMode::Subscription,
            slice_json: KIMI_CODE,
        },
        // GLM coding plan: Anthropic-compatible endpoint (reuses AnthropicMessagesCodec).
        // The snapshot's api points at /api/coding/paas/v4 (OpenAI-style); we override.
        Preset {
            profile_name: "glm-coding",
            base_url: "https://open.bigmodel.cn/api/anthropic",
            protocol: ProtocolFamily::AnthropicMessages,
            auth: AuthStrategy::ApiKey,
            provider_id: ProviderId::Custom {
                name: "glm-coding".to_string(),
            },
            credential_env: Some("ZHIPU_API_KEY"),
            billing_mode: ModelBillingMode::Subscription,
            slice_json: GLM_CODING,
        },
        // Z.AI: international GLM API (the global counterpart to the China-only
        // open.bigmodel.cn). OpenAI-compatible wire, pay-per-token. A distinct
        // env var (not ZHIPU_API_KEY, which glm-coding claims) keeps the z.ai
        // and bigmodel keys from cross-wiring — they are separate accounts.
        Preset {
            profile_name: "zai",
            base_url: "https://api.z.ai/api/paas/v4",
            protocol: ProtocolFamily::OpenAiChat,
            auth: AuthStrategy::ApiKey,
            provider_id: ProviderId::OpenAICompatible {
                name: "zai".to_string(),
            },
            credential_env: Some("ZAI_API_KEY"),
            billing_mode: ModelBillingMode::PerToken,
            slice_json: ZAI,
        },
        // OpenAI first-party: Responses API (codex removed the chat wire, so all
        // OpenAI traffic is Responses-only). API-key auth as a Bearer token.
        // ChatGPT account/OAuth login is a separate phase (P2; see
        // docs/superpowers/specs/2026-06-16-openai-auth-codex-parity-design.md).
        Preset {
            profile_name: "openai",
            base_url: "https://api.openai.com/v1",
            protocol: ProtocolFamily::OpenAiResponses,
            auth: AuthStrategy::Bearer,
            provider_id: ProviderId::OpenAI,
            credential_env: Some("OPENAI_API_KEY"),
            billing_mode: ModelBillingMode::PerToken,
            slice_json: OPENAI,
        },
        // OpenAI via ChatGPT-account OAuth login: routes to the Codex backend
        // (Responses API). Credential is OAuth (no env var) → resolved by the
        // openai-oauth credential provider via MultiCredentialProvider, keyed by
        // credential_id "openai-chatgpt". See P2 design doc.
        Preset {
            profile_name: "openai-chatgpt",
            base_url: "https://chatgpt.com/backend-api/codex",
            protocol: ProtocolFamily::OpenAiResponses,
            auth: AuthStrategy::ChatGptOAuth,
            provider_id: ProviderId::OpenAICompatible {
                name: "openai-chatgpt".to_string(),
            },
            credential_env: None,
            billing_mode: ModelBillingMode::Subscription,
            slice_json: OPENAI_CHATGPT,
        },
        // GitHub Copilot: OpenAI-compatible wire; GitHub OAuth token used
        // directly as the bearer via AuthStrategy::CopilotBearer (no exchange).
        Preset {
            profile_name: "github-copilot",
            base_url: "https://api.githubcopilot.com",
            protocol: ProtocolFamily::OpenAiChat,
            auth: AuthStrategy::CopilotBearer,
            provider_id: ProviderId::OpenAICompatible {
                name: "github-copilot".to_string(),
            },
            credential_env: Some("GITHUB_TOKEN"),
            billing_mode: ModelBillingMode::Subscription,
            slice_json: GITHUB_COPILOT,
        },
        // Google Gemini (first-party): generateContent wire; API key sent as the
        // `x-goog-api-key` header (AuthStrategy::ApiKey + GeminiGenerateContent).
        // Vendoring this slice gives every Gemini model its real context/output
        // limits and per-model price, instead of the Claude 200k/32k defaults and
        // the $5/$25 default-unknown pricing tier.
        Preset {
            profile_name: "gemini",
            base_url: "https://generativelanguage.googleapis.com/v1beta",
            protocol: ProtocolFamily::GeminiGenerateContent,
            auth: AuthStrategy::ApiKey,
            provider_id: ProviderId::Gemini,
            credential_env: Some("GEMINI_API_KEY"),
            billing_mode: ModelBillingMode::PerToken,
            slice_json: GEMINI,
        },
    ]
}

/// Assemble the built-in provider catalog from the vendored snapshots.
///
/// # Panics
/// Panics only if a vendored slice fails to parse — that is a build-time data
/// defect (the JSON is embedded and tested), never a runtime/host condition.
#[must_use]
pub fn builtin_presets() -> BuiltinCatalog {
    let mut providers = Vec::new();
    let mut pricing = PricingCatalog::empty();
    let openai_reference: ProviderSlice =
        serde_json::from_str(OPENAI).expect("vendored openai slice must parse");

    for preset in presets() {
        let slice: ProviderSlice = serde_json::from_str(preset.slice_json)
            .unwrap_or_else(|e| panic!("vendored slice {} parse: {e}", preset.profile_name));

        let mut models = Vec::with_capacity(slice.models.len());
        for model in slice.models.values() {
            let mut profile = to_model_profile(model);
            if preset.profile_name == "openai-chatgpt" {
                if let Some(reference) = openai_reference.models.get(&model.id) {
                    profile.metadata = to_metadata(reference);
                    profile.description = reference.description.clone();
                }
            }
            let display_pricing = profile
                .metadata
                .pricing
                .get_or_insert_with(|| ModelPricing {
                    billing_mode: preset.billing_mode,
                    source: Some("official".to_string()),
                    ..ModelPricing::default()
                });
            display_pricing.billing_mode = preset.billing_mode;
            if preset.billing_mode == ModelBillingMode::Subscription
                || (preset.profile_name == "openai" && model.id.starts_with("gpt-5.6-"))
            {
                display_pricing.source = Some("official".to_string());
            }
            if preset.billing_mode == ModelBillingMode::Subscription {
                display_pricing.input_per_million = None;
                display_pricing.output_per_million = None;
                display_pricing.cache_read_per_million = None;
                display_pricing.cache_write_per_million = None;
                display_pricing.reasoning_per_million = None;
                display_pricing.tiers.clear();
            }
            models.push(profile);
            // Subscription routes do not have a per-token charge.  Some
            // models.dev slices encode those plans as an all-zero price (and
            // Copilot may publish the underlying API-equivalent rate), but
            // exposing either value to CostTracker would turn a subscription
            // into misleading "$0" or token-dollar usage.  Keep the display
            // metadata above subscription-aware and leave the token catalog
            // unpriced as well.
            if preset.billing_mode != ModelBillingMode::Subscription {
                if let Some(price) = to_pricing(model) {
                    pricing =
                        pricing.with_price(preset.provider_id.clone(), model.id.clone(), price);
                }
            }
            // Multi-provider fix: register the model's real token limits so the
            // window / max-output functions return them for non-Claude models
            // instead of the Claude 200k / 32k defaults. Keyed by both the wire
            // id and the display name so whichever string the session carries
            // resolves. Claude ids ignore the registry (see context_window).
            if let Some(limit) = model.limit {
                // Skip degenerate slices (e.g. image models with context:0) so
                // they fall back to the default window rather than reporting 0.
                if limit.context > 0 && limit.output > 0 {
                    let limits = crate::model::model_limits::ModelLimits {
                        context_window: limit.context,
                        max_output_tokens: limit.output,
                    };
                    crate::model::model_limits::register(&model.id, limits);
                    crate::model::model_limits::register(&model.name, limits);
                }
            }
        }
        providers.push(ProviderProfile {
            provider_id: preset.provider_id.clone(),
            profile_name: preset.profile_name.to_string(),
            base_url: preset.base_url.to_string(),
            protocol: preset.protocol.clone(),
            auth: preset.auth.clone(),
            credential: match preset.credential_env {
                Some(var) => CredentialConfig::Env {
                    var: var.to_string(),
                },
                None => CredentialConfig::Static {
                    id: preset.profile_name.to_string(),
                },
            },
            models,
            pricing: crate::config::PricingConfig {
                billing_mode: preset.billing_mode,
                ..crate::config::PricingConfig::default()
            },
            // main-only fields: catalog presets are all OpenAI/Anthropic-style
            // (no AwsSigV4 / AzureOpenAi), so both default to None.
            signing: None,
            azure: None,
            supports_websockets: matches!(preset.profile_name, "openai" | "openai-chatgpt"),
            supports_websocket_compression: false,
            websocket_connect_timeout_ms: None,
            vision_delegate: (preset.profile_name == "deepseek")
                .then_some("deepseek-v4-flash-vision-exp".to_string()),
        });
    }

    BuiltinCatalog { providers, pricing }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_presets_register_real_model_limits() {
        use crate::model::context_window::{context_window_for_model, max_output_tokens_for_model};
        // Assembling the catalog registers each model's real limits.
        let _ = builtin_presets();
        // DeepSeek V4: real 1,000,000 / 384,000 — NOT the Claude 200k / 32k.
        assert_eq!(
            context_window_for_model("deepseek-v4-flash", &[]),
            1_000_000
        );
        assert_eq!(max_output_tokens_for_model("deepseek-v4-flash"), 384_000);
        // gpt-4.1: real 1,047,576 / 32,768.
        assert_eq!(context_window_for_model("gpt-4.1", &[]), 1_047_576);
        assert_eq!(max_output_tokens_for_model("gpt-4.1"), 32_768);
        // gemini-2.5-pro: real 1,048,576 / 65,536 (M6 — Gemini slice vendored).
        assert_eq!(context_window_for_model("gemini-2.5-pro", &[]), 1_048_576);
        assert_eq!(max_output_tokens_for_model("gemini-2.5-pro"), 65_536);
    }

    #[test]
    fn every_preset_yields_expected_model_counts() {
        let catalog = builtin_presets();
        assert_eq!(catalog.providers.len(), 10);
        let count = |name: &str| {
            catalog
                .providers
                .iter()
                .find(|p| p.profile_name == name)
                .map_or(0, |p| p.models.len())
        };
        // Exact counts guard against a truncated/partial re-vendor of a slice.
        assert_eq!(count("openrouter"), 398);
        assert_eq!(count("deepseek"), 3);
        let deepseek = catalog
            .providers
            .iter()
            .find(|p| p.profile_name == "deepseek")
            .expect("deepseek preset present");
        assert!(deepseek.models.iter().all(|m| !matches!(
            m.request_model.as_str(),
            "deepseek-chat" | "deepseek-reasoner"
        )));
        assert_eq!(count("kimi"), 10);
        assert_eq!(count("kimi-code"), 4);
        assert_eq!(count("glm-coding"), 9);
        assert_eq!(count("zai"), 16);
        // OpenAI API and ChatGPT OAuth profiles intentionally share the latest
        // GPT-5.6 ids; callers qualify the profile when choosing a route.
        assert_eq!(count("openai"), 55);
        assert_eq!(count("openai-chatgpt"), 3);
        assert_eq!(count("github-copilot"), 36);
        // Gemini slice vendored verbatim from models.dev (google provider).
        assert_eq!(count("gemini"), 42);
        let openai = catalog
            .providers
            .iter()
            .find(|p| p.profile_name == "openai")
            .expect("openai preset present");
        assert_eq!(openai.protocol, ProtocolFamily::OpenAiResponses);
        assert_eq!(openai.provider_id, ProviderId::OpenAI);
        assert_eq!(openai.base_url, "https://api.openai.com/v1");
        assert!(openai.supports_websockets);
        let chatgpt = catalog
            .providers
            .iter()
            .find(|p| p.profile_name == "openai-chatgpt")
            .expect("present");
        assert_eq!(chatgpt.protocol, ProtocolFamily::OpenAiResponses);
        assert_eq!(chatgpt.auth, AuthStrategy::ChatGptOAuth);
        assert_eq!(chatgpt.base_url, "https://chatgpt.com/backend-api/codex");
        assert!(chatgpt.supports_websockets);
    }

    #[test]
    fn subscription_presets_never_render_zero_token_prices() {
        let catalog = builtin_presets();
        for profile_name in [
            "openai-chatgpt",
            "kimi-code",
            "glm-coding",
            "github-copilot",
        ] {
            let provider = catalog
                .providers
                .iter()
                .find(|provider| provider.profile_name == profile_name)
                .expect("subscription provider");
            assert_eq!(
                provider.pricing.billing_mode,
                ModelBillingMode::Subscription
            );
            for model in &provider.models {
                let pricing = model.metadata.pricing.as_ref().expect("billing metadata");
                assert_eq!(pricing.billing_mode, ModelBillingMode::Subscription);
                assert_eq!(pricing.input_per_million, None);
                assert_eq!(pricing.output_per_million, None);
            }
        }
    }

    #[test]
    fn subscription_presets_are_absent_from_token_pricing_catalog() {
        let catalog = builtin_presets();
        let kimi_code = ProviderId::OpenAICompatible {
            name: "kimi-code".to_string(),
        };
        assert!(catalog.pricing.get(&kimi_code, "k3").is_none());

        let copilot = ProviderId::OpenAICompatible {
            name: "github-copilot".to_string(),
        };
        assert!(catalog.pricing.get(&copilot, "claude-opus-4.6").is_none());
    }

    #[test]
    fn chatgpt_models_inherit_openai_limits_without_api_pricing() {
        let catalog = builtin_presets();
        let provider = catalog
            .providers
            .iter()
            .find(|provider| provider.profile_name == "openai-chatgpt")
            .unwrap();
        let model = provider
            .models
            .iter()
            .find(|model| model.request_model == "gpt-5.6-luna")
            .unwrap();
        assert_eq!(model.metadata.context_window_tokens, Some(1_050_000));
        assert_eq!(model.metadata.max_output_tokens, Some(128_000));
        assert_eq!(
            model.metadata.pricing.as_ref().unwrap().billing_mode,
            ModelBillingMode::Subscription
        );
    }

    #[test]
    fn route_listings_keep_provider_specific_pricing_and_effort() {
        let catalog = builtin_presets();
        let registry = crate::ModelRegistry::from_config(crate::ClientConfig {
            providers: catalog.providers,
        })
        .unwrap();
        let listings = registry.available_models();
        let openai = listings
            .iter()
            .find(|item| item.profile_name == "openai" && item.request_model == "gpt-5.6-sol")
            .unwrap();
        let chatgpt = listings
            .iter()
            .find(|item| {
                item.profile_name == "openai-chatgpt" && item.request_model == "gpt-5.6-sol"
            })
            .unwrap();
        assert_eq!(
            openai.metadata.pricing.as_ref().unwrap().billing_mode,
            ModelBillingMode::PerToken
        );
        assert_eq!(
            chatgpt.metadata.pricing.as_ref().unwrap().billing_mode,
            ModelBillingMode::Subscription
        );
        assert_eq!(
            openai.reasoning.levels,
            ["low", "medium", "high", "xhigh", "max"]
        );
        assert!(openai.reasoning.can_disable);
        assert_eq!(chatgpt.reasoning.levels, openai.reasoning.levels);
    }
}

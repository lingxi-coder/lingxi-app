//! Built-in provider presets: vendored models.dev metadata + hand-authored
//! routing (base URL, protocol, auth, credential). The routing table is the
//! source of truth for wire/auth and overrides the snapshot's advisory `api`.

use crate::catalog::map::{to_model_profile, to_pricing};
use crate::catalog::models_dev::ProviderSlice;
use crate::{
    AuthStrategy, CredentialConfig, PricingCatalog, ProtocolFamily, ProviderId, ProviderProfile,
};

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
    /// Embedded models.dev slice JSON.
    slice_json: &'static str,
}

const OPENROUTER: &str = include_str!("../../data/models-dev/openrouter.json");
const DEEPSEEK: &str = include_str!("../../data/models-dev/deepseek.json");
const GLM_CODING: &str = include_str!("../../data/models-dev/zhipuai-coding-plan.json");
const ZAI: &str = include_str!("../../data/models-dev/zai.json");
const OPENAI: &str = include_str!("../../data/models-dev/openai.json");
const GITHUB_COPILOT: &str = include_str!("../../data/models-dev/github-copilot.json");

fn presets() -> Vec<Preset> {
    vec![
        Preset {
            profile_name: "openrouter",
            base_url: "https://openrouter.ai/api/v1",
            protocol: ProtocolFamily::OpenAiChat,
            auth: AuthStrategy::ApiKey,
            provider_id: ProviderId::OpenAICompatible { name: "openrouter".to_string() },
            credential_env: Some("OPENROUTER_API_KEY"),
            slice_json: OPENROUTER,
        },
        Preset {
            profile_name: "deepseek",
            base_url: "https://api.deepseek.com",
            protocol: ProtocolFamily::OpenAiChat,
            auth: AuthStrategy::ApiKey,
            provider_id: ProviderId::OpenAICompatible { name: "deepseek".to_string() },
            credential_env: Some("DEEPSEEK_API_KEY"),
            slice_json: DEEPSEEK,
        },
        // GLM coding plan: Anthropic-compatible endpoint (reuses AnthropicMessagesCodec).
        // The snapshot's api points at /api/coding/paas/v4 (OpenAI-style); we override.
        Preset {
            profile_name: "glm-coding",
            base_url: "https://open.bigmodel.cn/api/anthropic",
            protocol: ProtocolFamily::AnthropicMessages,
            auth: AuthStrategy::ApiKey,
            provider_id: ProviderId::Custom { name: "glm-coding".to_string() },
            credential_env: Some("ZHIPU_API_KEY"),
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
            provider_id: ProviderId::OpenAICompatible { name: "zai".to_string() },
            credential_env: Some("ZAI_API_KEY"),
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
            slice_json: OPENAI,
        },
        // GitHub Copilot: OpenAI-compatible wire; GitHub OAuth token used
        // directly as the bearer via AuthStrategy::CopilotBearer (no exchange).
        Preset {
            profile_name: "github-copilot",
            base_url: "https://api.githubcopilot.com",
            protocol: ProtocolFamily::OpenAiChat,
            auth: AuthStrategy::CopilotBearer,
            provider_id: ProviderId::OpenAICompatible { name: "github-copilot".to_string() },
            credential_env: Some("GITHUB_TOKEN"),
            slice_json: GITHUB_COPILOT,
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

    for preset in presets() {
        let slice: ProviderSlice = serde_json::from_str(preset.slice_json)
            .unwrap_or_else(|e| panic!("vendored slice {} parse: {e}", preset.profile_name));

        let mut models = Vec::with_capacity(slice.models.len());
        for model in slice.models.values() {
            models.push(to_model_profile(model));
            if let Some(price) = to_pricing(model) {
                pricing = pricing.with_price(preset.provider_id.clone(), model.id.clone(), price);
            }
        }
        providers.push(ProviderProfile {
            provider_id: preset.provider_id.clone(),
            profile_name: preset.profile_name.to_string(),
            base_url: preset.base_url.to_string(),
            protocol: preset.protocol.clone(),
            auth: preset.auth.clone(),
            credential: match preset.credential_env {
                Some(var) => CredentialConfig::Env { var: var.to_string() },
                None => CredentialConfig::Static { id: preset.profile_name.to_string() },
            },
            models,
            pricing: crate::config::PricingConfig::default(),
            // main-only fields: catalog presets are all OpenAI/Anthropic-style
            // (no AwsSigV4 / AzureOpenAi), so both default to None.
            signing: None,
            azure: None,
        });
    }

    BuiltinCatalog { providers, pricing }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_preset_yields_expected_model_counts() {
        let catalog = builtin_presets();
        assert_eq!(catalog.providers.len(), 6);
        let count = |name: &str| {
            catalog
                .providers
                .iter()
                .find(|p| p.profile_name == name)
                .map_or(0, |p| p.models.len())
        };
        // Exact counts guard against a truncated/partial re-vendor of a slice.
        assert_eq!(count("openrouter"), 337);
        assert_eq!(count("deepseek"), 4);
        assert_eq!(count("glm-coding"), 6);
        assert_eq!(count("zai"), 13);
        assert_eq!(count("openai"), 50);
        assert_eq!(count("github-copilot"), 23);
        let openai = catalog
            .providers
            .iter()
            .find(|p| p.profile_name == "openai")
            .expect("openai preset present");
        assert_eq!(openai.protocol, ProtocolFamily::OpenAiResponses);
        assert_eq!(openai.provider_id, ProviderId::OpenAI);
        assert_eq!(openai.base_url, "https://api.openai.com/v1");
    }
}

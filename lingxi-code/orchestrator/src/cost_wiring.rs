//! Translation helpers between `api_client::types::UsageApi` and
//! `cost::Usage` plus model-string → `ProviderId` resolution.
//!
//! Used by M6-06 to feed `MessageResponse.usage` into `CostTracker`.

use api_client::types::UsageApi;
use cost::pricing::ProviderId;
use cost::usage::{TokenUsage, Usage};
use cost::ModelRef;
use providers::ModelSpec;

/// Translate an API-client `UsageApi` into the cost crate's `Usage` shape.
///
/// Maps Anthropic's `cache_creation_input_tokens` to `TokenUsage::cache_write`
/// and `cache_read_input_tokens` to `TokenUsage::cache_read`.
/// Reasoning-output and server-tool-use are dropped in v0.7.0 — non-streaming
/// `messages.create` does not emit those counters.
#[must_use]
pub(crate) fn usage_api_to_cost_usage(api: &UsageApi) -> Usage {
    Usage {
        tokens: TokenUsage {
            input: api.input_tokens,
            output: api.output_tokens,
            cache_write: api.cache_creation_input_tokens,
            cache_read: api.cache_read_input_tokens,
            reasoning_output: 0,
        },
        server_tool_use: None,
        speed: None,
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

/// Resolve a model-name string to its `ProviderId` by parsing the
/// `provider/model` prefix (bare / `claude-*` → Anthropic, for back-compat).
#[must_use]
pub(crate) fn provider_from_model(model: &str) -> ProviderId {
    provider_id_for_profile(&ModelSpec::parse(model).profile)
}

/// Build a fully-qualified [`ModelRef`] from a model string: the prefix selects
/// the provider, and the local model id (prefix stripped) is what the price
/// catalog is keyed on. `claude-*` / bare strings keep the full string as the
/// model id, so Anthropic cost attribution is byte-identical to before.
#[must_use]
pub(crate) fn model_ref_from_string(model: &str) -> ModelRef {
    ModelRef {
        provider: provider_from_model(model),
        model: ModelSpec::parse(model).model,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_tokens_one_to_one() {
        let api = UsageApi {
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_input_tokens: 20,
            cache_read_input_tokens: 10,
        };
        let u = usage_api_to_cost_usage(&api);
        assert_eq!(u.tokens.input, 100);
        assert_eq!(u.tokens.output, 50);
        assert_eq!(u.tokens.cache_write, 20);
        assert_eq!(u.tokens.cache_read, 10);
        assert_eq!(u.tokens.reasoning_output, 0);
        assert!(u.server_tool_use.is_none());
        assert!(u.speed.is_none());
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

//! Translation helpers between `api_client::types::UsageApi` and
//! `cost::Usage` plus model-string → `ProviderId` resolution.
//!
//! Used by M6-06 to feed `MessageResponse.usage` into `CostTracker`.

use api_client::types::UsageApi;
use cost::pricing::ProviderId;
use cost::usage::{TokenUsage, Usage};
use cost::ModelRef;

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

/// Resolve a model-name string to its `ProviderId`.
///
/// v0.7.0: always `Anthropic` (the only provider lingxi-cli wires).
/// M7 will expand to prefix-match `gpt-*` → `OpenAI`, `gemini-*` → `GoogleGemini`,
/// etc. The `model` argument is kept so the future expansion does not
/// need a signature change.
#[must_use]
pub(crate) fn provider_from_model(_model: &str) -> ProviderId {
    ProviderId::Anthropic
}

/// Build a fully-qualified [`ModelRef`] from a model string.
#[must_use]
pub(crate) fn model_ref_from_string(model: &str) -> ModelRef {
    ModelRef {
        provider: provider_from_model(model),
        model: model.to_string(),
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
    fn provider_always_anthropic_in_v070() {
        assert_eq!(
            provider_from_model("claude-opus-4-7"),
            ProviderId::Anthropic
        );
        assert_eq!(provider_from_model("gpt-5"), ProviderId::Anthropic);
        assert_eq!(provider_from_model(""), ProviderId::Anthropic);
    }

    #[test]
    fn model_ref_carries_provider_and_string() {
        let mr = model_ref_from_string("claude-opus-4-7");
        assert_eq!(mr.provider, ProviderId::Anthropic);
        assert_eq!(mr.model, "claude-opus-4-7");
    }
}

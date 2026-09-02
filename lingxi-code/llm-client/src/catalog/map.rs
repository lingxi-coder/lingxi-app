//! Map a models.dev [`Model`] to llm-client's [`ModelProfile`] + optional
//! [`TokenPricing`]. Routing/auth is handled by the preset table, not here.

use crate::catalog::models_dev::Model;
use crate::{Capabilities, ModelProfile, TokenPricing};
use platform_api::{ModelBillingMode, ModelMetadata, ModelPricing, ModelPricingTier};

/// Map a models.dev model to a [`ModelProfile`]. `request_model` and
/// `billing_model` are both the wire id; `display_model` is the human name.
#[must_use]
pub fn to_model_profile(model: &Model) -> ModelProfile {
    ModelProfile {
        display_model: model.name.clone(),
        request_model: model.id.clone(),
        billing_model: model.id.clone(),
        aliases: Vec::new(),
        description: model.description.clone(),
        metadata: to_metadata(model),
        capabilities: to_capabilities(model),
    }
}

/// Map provider-published facts without inventing values for absent fields.
#[must_use]
pub fn to_metadata(model: &Model) -> ModelMetadata {
    let limit = model.limit;
    ModelMetadata {
        family: model.family.clone(),
        status: model.status.clone().or_else(|| {
            model
                .experimental
                .is_some_and(std::convert::identity)
                .then(|| "experimental".to_string())
        }),
        release_date: model.release_date.clone(),
        last_updated: model.last_updated.clone(),
        knowledge_cutoff: model.knowledge.clone(),
        input_modalities: model
            .modalities
            .as_ref()
            .map(|modalities| modalities.input.clone())
            .unwrap_or_default(),
        output_modalities: model
            .modalities
            .as_ref()
            .map(|modalities| modalities.output.clone())
            .unwrap_or_default(),
        context_window_tokens: limit.map(|value| value.context).filter(|value| *value > 0),
        max_input_tokens: limit
            .and_then(|value| value.input)
            .filter(|value| *value > 0),
        max_output_tokens: limit.map(|value| value.output).filter(|value| *value > 0),
        open_weights: model.open_weights,
        attachments: model.attachment,
        temperature_control: model.temperature,
        pricing: model.cost.as_ref().map(|cost| ModelPricing {
            billing_mode: if cost.input == 0.0 && cost.output == 0.0 {
                ModelBillingMode::Free
            } else {
                ModelBillingMode::PerToken
            },
            input_per_million: Some(cost.input),
            output_per_million: Some(cost.output),
            cache_read_per_million: cost.cache_read,
            cache_write_per_million: cost.cache_write,
            reasoning_per_million: cost.reasoning,
            tiers: cost
                .tiers
                .iter()
                .filter(|tier| tier.tier.kind == "context")
                .map(|tier| ModelPricingTier {
                    context_threshold_tokens: tier.tier.size,
                    input_per_million: Some(tier.input),
                    output_per_million: Some(tier.output),
                    cache_read_per_million: tier.cache_read,
                    cache_write_per_million: tier.cache_write,
                    reasoning_per_million: tier.reasoning,
                })
                .collect(),
            source: Some("modelsDev".to_string()),
        }),
    }
}

/// Derive capabilities. Streaming is always supported by these providers; vision
/// and documents come from input modalities.
#[must_use]
pub fn to_capabilities(model: &Model) -> Capabilities {
    let has = |m: &str| {
        model
            .modalities
            .as_ref()
            .is_some_and(|x| x.input.iter().any(|i| i == m))
    };
    Capabilities {
        streaming: true,
        tools: model.tool_call,
        vision: has("image"),
        documents: has("pdf") || has("file"),
        reasoning: model.reasoning,
        structured_output: model.structured_output,
    }
}

/// Map costs to [`TokenPricing`]. models.dev costs are already per-million
/// tokens. Returns `None` when the model carries no cost block (unpriced);
/// an all-zero cost block maps to a real zero price (free, but priced).
#[must_use]
pub fn to_pricing(model: &Model) -> Option<TokenPricing> {
    model.cost.as_ref().map(|c| TokenPricing {
        input_per_million: c.input,
        output_per_million: c.output,
        cache_write_per_million: c.cache_write.unwrap_or(0.0),
        cache_read_per_million: c.cache_read.unwrap_or(0.0),
        reasoning_per_million: c.reasoning.unwrap_or(0.0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::models_dev::ProviderSlice;

    const DEEPSEEK: &str = include_str!("../../data/models-dev/deepseek.json");
    const ZAI: &str = include_str!("../../data/models-dev/zai.json");

    fn deepseek() -> ProviderSlice {
        serde_json::from_str(DEEPSEEK).unwrap()
    }

    #[test]
    fn maps_id_name_and_caps() {
        let slice = deepseek();
        let model = slice.models.values().next().unwrap();
        let profile = to_model_profile(model);
        assert_eq!(profile.request_model, model.id);
        assert_eq!(profile.billing_model, model.id);
        assert_eq!(profile.display_model, model.name);
        assert!(profile.capabilities.streaming);
        assert_eq!(profile.capabilities.tools, model.tool_call);
    }

    #[test]
    fn image_input_is_marked_as_vision_and_multimodal() {
        let slice = deepseek();
        let vision = slice
            .models
            .get("deepseek-v4-flash-vision-exp")
            .expect("current DeepSeek vision model");
        let profile = to_model_profile(vision);
        assert!(profile.capabilities.vision);
        assert!(profile.capabilities.supports_multimodal());

        let text_only = slice
            .models
            .get("deepseek-v4-flash")
            .expect("current DeepSeek text model");
        assert!(!to_capabilities(text_only).vision);
    }

    #[test]
    fn zai_glm_53_flash_is_native_multimodal() {
        let slice: ProviderSlice = serde_json::from_str(ZAI).unwrap();
        let model = slice
            .models
            .get("glm-5.3-flash")
            .expect("current Z.AI multimodal model");
        let profile = to_model_profile(model);

        assert!(profile.capabilities.vision);
        assert!(profile.capabilities.documents);
        assert!(profile.capabilities.supports_multimodal());
        assert!(profile.capabilities.structured_output);
        assert_eq!(profile.metadata.context_window_tokens, Some(1_000_000));
        assert_eq!(profile.metadata.max_output_tokens, Some(131_072));
    }

    #[test]
    fn priced_model_maps_cost_per_million() {
        let slice = deepseek();
        let model = slice.models.get("deepseek-v4-flash").unwrap();
        let pricing = to_pricing(model).expect("deepseek model is priced");
        let cost = model.cost.as_ref().unwrap();
        assert!((pricing.input_per_million - cost.input).abs() < f64::EPSILON);
        assert!((pricing.output_per_million - cost.output).abs() < f64::EPSILON);
    }

    #[test]
    fn zero_priced_model_is_labeled_free() {
        let slice: ProviderSlice =
            serde_json::from_str(include_str!("../../data/models-dev/openrouter.json")).unwrap();
        let model = slice
            .models
            .get("openrouter/free")
            .expect("official free router is present");

        let metadata = to_metadata(model);
        assert_eq!(
            metadata.pricing.expect("published zero price").billing_mode,
            ModelBillingMode::Free
        );
    }

    #[test]
    fn openrouter_file_input_maps_to_document_capability() {
        let slice: ProviderSlice =
            serde_json::from_str(include_str!("../../data/models-dev/openrouter.json")).unwrap();
        let model = slice
            .models
            .get("anthropic/claude-fable-5.1")
            .expect("latest Fable model is present");

        assert!(to_capabilities(model).documents);
    }

    #[test]
    fn vision_model_maps_catalog_pricing() {
        let slice = deepseek();
        let model = slice.models.get("deepseek-v4-flash-vision-exp").unwrap();
        let pricing = to_pricing(model).expect("current vision model is priced");
        assert!((pricing.input_per_million - 0.14).abs() < f64::EPSILON);
        assert!((pricing.output_per_million - 0.28).abs() < f64::EPSILON);
    }

    #[test]
    fn rich_metadata_preserves_limits_modalities_and_reasoning_price() {
        let slice = deepseek();
        let model = slice.models.get("deepseek-v4-flash-vision-exp").unwrap();
        let metadata = to_metadata(model);
        assert_eq!(metadata.status.as_deref(), Some("beta"));
        assert_eq!(metadata.context_window_tokens, Some(1_000_000));
        assert_eq!(metadata.max_input_tokens, None);
        assert_eq!(metadata.max_output_tokens, Some(384_000));
        assert!(metadata.input_modalities.iter().any(|item| item == "image"));
        let published = metadata.pricing.expect("published price");
        assert_eq!(published.reasoning_per_million, Some(0.28));
        assert_eq!(published.cache_read_per_million, Some(0.0028));

        let effective = to_pricing(model).expect("billable price");
        assert!((effective.reasoning_per_million - 0.28).abs() < f64::EPSILON);
    }

    #[test]
    fn openai_long_context_tier_and_max_input_are_preserved() {
        let slice: ProviderSlice =
            serde_json::from_str(include_str!("../../data/models-dev/openai.json")).unwrap();
        let model = slice
            .models
            .get("gpt-5.6-sol")
            .expect("current openai model");
        let metadata = to_metadata(model);
        assert_eq!(metadata.context_window_tokens, Some(1_050_000));
        assert_eq!(metadata.max_input_tokens, Some(922_000));
        assert_eq!(metadata.max_output_tokens, Some(128_000));
        let pricing = metadata.pricing.expect("published price");
        assert_eq!(pricing.input_per_million, Some(4.0));
        assert_eq!(pricing.output_per_million, Some(20.0));
        assert_eq!(pricing.cache_read_per_million, Some(0.4));
        assert_eq!(pricing.cache_write_per_million, Some(5.0));
        let tier = pricing.tiers.first().expect("long-context tier");
        assert_eq!(tier.context_threshold_tokens, 272_000);
        assert_eq!(tier.input_per_million, Some(8.0));
        assert_eq!(tier.output_per_million, Some(30.0));
        assert_eq!(tier.cache_write_per_million, Some(10.0));
    }
}

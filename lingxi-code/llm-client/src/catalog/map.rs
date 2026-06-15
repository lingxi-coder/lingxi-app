//! Map a models.dev [`Model`] to llm-client's [`ModelProfile`] + optional
//! [`TokenPricing`]. Routing/auth is handled by the preset table, not here.

use crate::catalog::models_dev::Model;
use crate::{Capabilities, ModelProfile, TokenPricing};

/// Map a models.dev model to a [`ModelProfile`]. `request_model` and
/// `billing_model` are both the wire id; `display_model` is the human name.
#[must_use]
pub fn to_model_profile(model: &Model) -> ModelProfile {
    ModelProfile {
        display_model: model.name.clone(),
        request_model: model.id.clone(),
        billing_model: model.id.clone(),
        aliases: Vec::new(),
        capabilities: to_capabilities(model),
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
        documents: has("pdf"),
        reasoning: model.reasoning,
        structured_output: model.structured_output,
    }
}

/// Map costs to [`TokenPricing`]. models.dev costs are already per-million
/// tokens. Returns `None` when the model carries no cost block (unpriced);
/// an all-zero cost block maps to a real zero price (free, but priced).
#[must_use]
pub fn to_pricing(model: &Model) -> Option<TokenPricing> {
    model.cost.map(|c| TokenPricing {
        input_per_million: c.input,
        output_per_million: c.output,
        cache_write_per_million: c.cache_write.unwrap_or(0.0),
        cache_read_per_million: c.cache_read.unwrap_or(0.0),
        reasoning_per_million: 0.0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::models_dev::ProviderSlice;

    const DEEPSEEK: &str = include_str!("../../data/models-dev/deepseek.json");

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
    fn priced_model_maps_cost_per_million() {
        let slice = deepseek();
        // Every deepseek model has a cost block.
        let model = slice.models.values().next().unwrap();
        let pricing = to_pricing(model).expect("deepseek model is priced");
        let cost = model.cost.unwrap();
        assert!((pricing.input_per_million - cost.input).abs() < f64::EPSILON);
        assert!((pricing.output_per_million - cost.output).abs() < f64::EPSILON);
    }
}

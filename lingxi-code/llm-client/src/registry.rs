//! Model registry and route identity resolution.

use crate::{
    reasoning_control_spec, Capabilities, ClientConfig, LlmError, PricingModelRef, ProviderId,
    ReasoningControlSpec, ReasoningTarget,
};
use platform_api::{ModelBillingMode, ModelMetadata, ModelPricing};

/// Model entry exposed to model-picker and listing callers.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelListing {
    /// Explicit provider identity.
    pub provider_id: ProviderId,
    /// Configured provider profile name.
    pub profile_name: String,
    /// Human-facing model label.
    pub display_model: String,
    /// Provider-local model value sent on the wire.
    pub request_model: String,
    /// Model key used by the pricing catalog.
    pub billing_model: String,
    /// Alternate names accepted by registry resolution.
    pub aliases: Vec<String>,
    /// Optional one-line model description (dimmed sub-line in the `/model`
    /// picker); carried through from the configured [`ModelProfile`].
    pub description: Option<String>,
    /// Model capabilities for this listing.
    pub capabilities: Capabilities,
    /// Full provider-specific metadata used by model selectors.
    pub metadata: ModelMetadata,
    /// Exact reasoning controls accepted by this route.
    pub reasoning: ReasoningControlSpec,
    /// Optional Fusion panel/analyst hints.
    pub fusion_hints: Option<platform_api::FusionModelHints>,
    /// Whether THIS ROUTE could serve as the Fusion analyst: the model claims
    /// structured output AND the owning profile's codec can actually encode a
    /// `response_format`. The two are different claims and the weaker one is
    /// not enough — see [`crate::ProtocolFamily::encodes_response_format`].
    pub fusion_analyst_capable: bool,
}

/// Resolved route identity for one requested model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRoute {
    /// Explicit provider identity.
    pub provider_id: ProviderId,
    /// Configured provider profile name.
    pub profile_name: String,
    /// Provider-local model value sent on the wire.
    pub request_model: String,
    /// Human-facing model label.
    pub display_model: String,
    /// Pricing identity for this route.
    pub pricing_model: PricingModelRef,
    /// Model capabilities for this route.
    pub capabilities: Capabilities,
}

/// Resolved main route plus an optional vision delegate route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaRoute {
    /// Main route selected by the caller.
    pub main: ResolvedRoute,
    /// Vision delegate on the same profile, when the main route lacks vision.
    pub vision_delegate: Option<ResolvedRoute>,
}

/// Registry that resolves requested model names to route identities.
#[derive(Debug, Clone)]
pub struct ModelRegistry {
    config: ClientConfig,
}

impl ModelRegistry {
    pub(crate) fn profile_pricing_config(&self, profile: &str) -> Option<crate::PricingConfig> {
        self.config
            .providers
            .iter()
            .find(|provider| provider.profile_name == profile)
            .map(|provider| provider.pricing.clone())
    }

    fn effective_metadata(
        provider: &crate::ProviderProfile,
        model: &crate::ModelProfile,
    ) -> ModelMetadata {
        let mut metadata = model.metadata.clone();
        if let Some((_, override_pricing)) = provider
            .pricing
            .overrides
            .iter()
            .find(|(id, _)| id == &model.display_model || id == &model.request_model)
        {
            if metadata
                .pricing
                .as_ref()
                .and_then(|pricing| pricing.source.as_deref())
                != Some("userOverride")
            {
                metadata.pricing = Some(ModelPricing {
                    billing_mode: ModelBillingMode::PerToken,
                    input_per_million: Some(override_pricing.input_per_million),
                    output_per_million: Some(override_pricing.output_per_million),
                    cache_read_per_million: Some(override_pricing.cache_read_per_million),
                    cache_write_per_million: Some(override_pricing.cache_write_per_million),
                    reasoning_per_million: Some(override_pricing.reasoning_per_million),
                    tiers: Vec::new(),
                    source: Some("userOverride".to_string()),
                });
            }
        } else if provider.pricing.billing_mode != ModelBillingMode::Unknown {
            let pricing = metadata.pricing.get_or_insert_with(ModelPricing::default);
            pricing.billing_mode = provider.pricing.billing_mode;
        }
        metadata
    }

    /// Build a registry from validated client config.
    pub fn from_config(config: ClientConfig) -> Result<Self, LlmError> {
        if config
            .providers
            .iter()
            .any(|provider| provider.models.is_empty())
        {
            return Err(LlmError::InvalidRequest {
                message: "provider profile must declare at least one model".to_string(),
            });
        }

        for provider in &config.providers {
            if let Some(delegate_model) = provider.vision_delegate.as_deref() {
                let delegate = provider
                    .models
                    .iter()
                    .find(|model| {
                        model.display_model == delegate_model
                            || model.request_model == delegate_model
                            || model.aliases.iter().any(|alias| alias == delegate_model)
                    })
                    .ok_or_else(|| LlmError::InvalidRequest {
                        message: format!(
                            "provider profile '{}' sets visionDelegate to {:?}, but that model is not declared on the same profile",
                            provider.profile_name, delegate_model
                        ),
                    })?;
                if provider.models.len() == 1 {
                    return Err(LlmError::InvalidRequest {
                        message: format!(
                            "provider profile '{}' sets visionDelegate to {:?}, but a profile's only model cannot delegate to itself",
                            provider.profile_name, delegate_model
                        ),
                    });
                }
                if !delegate.capabilities.vision {
                    return Err(LlmError::InvalidRequest {
                        message: format!(
                            "provider profile '{}' sets visionDelegate to {:?}, but that model does not advertise vision capability",
                            provider.profile_name, delegate_model
                        ),
                    });
                }
            }
        }

        Ok(Self { config })
    }

    /// Return configured profile/model pairs.
    #[must_use]
    pub fn available_models(&self) -> Vec<ModelListing> {
        self.config
            .providers
            .iter()
            .flat_map(|provider| {
                provider.models.iter().map(|model| ModelListing {
                    provider_id: provider.provider_id.clone(),
                    profile_name: provider.profile_name.clone(),
                    display_model: model.display_model.clone(),
                    request_model: model.request_model.clone(),
                    billing_model: model.billing_model.clone(),
                    aliases: model.aliases.clone(),
                    description: model.description.clone(),
                    capabilities: model.capabilities,
                    metadata: Self::effective_metadata(provider, model),
                    reasoning: reasoning_control_spec(ReasoningTarget {
                        profile_name: Some(provider.profile_name.as_str()),
                        protocol: &provider.protocol,
                        base_url: &provider.base_url,
                        model: &model.request_model,
                    }),
                    fusion_hints: crate::fusion_hints::hints_for(
                        &provider.profile_name,
                        &model.request_model,
                    ),
                    fusion_analyst_capable: model.capabilities.structured_output
                        && provider.protocol.encodes_response_format(),
                })
            })
            .collect()
    }

    /// Resolve a model id, optionally scoped to one provider profile.
    /// `profile = Some(p)` matches only within profile `p` (absent model or
    /// unknown profile → `ModelUnavailable`); `None` matches across all
    /// providers (ambiguous → error).
    pub fn resolve_in(
        &self,
        requested: &str,
        profile: Option<&str>,
    ) -> Result<ResolvedRoute, LlmError> {
        let mut matches = Vec::new();
        for provider in &self.config.providers {
            if let Some(p) = profile {
                if provider.profile_name != p {
                    continue;
                }
            }
            for model in &provider.models {
                let is_match = model.display_model == requested
                    || model.request_model == requested
                    || model.aliases.iter().any(|alias| alias == requested);

                if is_match {
                    matches.push((provider, model));
                }
            }
        }

        // Mobile and other structured clients expose `profile/model` refs so
        // two providers serving the same model remain distinguishable. The
        // provider protocol accepts only its native `model`, however. Prefer an
        // exact model/alias match above (important for legitimate slash-bearing
        // wire ids such as OpenRouter's `openrouter/auto`), then repair a known
        // provider-qualified UI ref at this final routing boundary. This is the
        // last fail-safe if a lifecycle path lets the display ref reach the LLM
        // client without first splitting it.
        if matches.is_empty() {
            if let Some((qualifier, provider_model)) = requested.split_once('/') {
                if profile.is_none_or(|scoped| scoped == qualifier) {
                    for provider in &self.config.providers {
                        if provider.profile_name != qualifier {
                            continue;
                        }
                        for model in &provider.models {
                            let is_match = model.display_model == provider_model
                                || model.request_model == provider_model
                                || model.aliases.iter().any(|alias| alias == provider_model);
                            if is_match {
                                matches.push((provider, model));
                            }
                        }
                    }
                }
            }
        }

        match matches.as_slice() {
            [] => Err(LlmError::ModelUnavailable),
            [(provider, model)] => Ok(ResolvedRoute {
                provider_id: provider.provider_id.clone(),
                profile_name: provider.profile_name.clone(),
                request_model: model.request_model.clone(),
                display_model: model.display_model.clone(),
                pricing_model: PricingModelRef {
                    pricing_provider_id: provider.provider_id.clone(),
                    billing_model: model.billing_model.clone(),
                    request_model: model.request_model.clone(),
                    display_model: model.display_model.clone(),
                },
                capabilities: model.capabilities,
            }),
            multiple => {
                let profiles: Vec<&str> = multiple
                    .iter()
                    .map(|(p, _)| p.profile_name.as_str())
                    .collect();
                let suggestions = profiles
                    .iter()
                    .map(|p| format!("{p}/{requested}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                Err(LlmError::InvalidRequest {
                    message: format!(
                        "model reference '{requested}' is ambiguous across profiles: {} \
                         — qualify it, e.g. {}",
                        profiles.join(", "),
                        suggestions
                    ),
                })
            }
        }
    }

    /// Resolve across all providers (unscoped). Ambiguous ids error.
    pub fn resolve(&self, requested: &str) -> Result<ResolvedRoute, LlmError> {
        self.resolve_in(requested, None)
    }

    /// Resolve a route and, when needed, its same-profile vision delegate.
    pub fn resolve_media_route_in(
        &self,
        requested: &str,
        profile: Option<&str>,
    ) -> Result<MediaRoute, LlmError> {
        let main = self.resolve_in(requested, profile)?;
        let provider = self
            .config
            .providers
            .iter()
            .find(|provider| provider.profile_name == main.profile_name)
            .ok_or(LlmError::ModelUnavailable)?;
        let vision_delegate = if main.capabilities.vision {
            None
        } else {
            provider
                .vision_delegate
                .as_deref()
                .map(|delegate| self.resolve_in(delegate, Some(provider.profile_name.as_str())))
                .transpose()?
        };
        Ok(MediaRoute {
            main,
            vision_delegate,
        })
    }

    /// Resolve a route and optional same-profile vision delegate unscoped.
    pub fn resolve_media_route(&self, requested: &str) -> Result<MediaRoute, LlmError> {
        self.resolve_media_route_in(requested, None)
    }
}

#[cfg(test)]
mod pricing_policy_tests {
    use super::*;

    #[test]
    fn profile_pricing_policy_retains_explicit_overrides_without_metadata_inference() {
        let mut provider = crate::builtin_presets()
            .providers
            .into_iter()
            .next()
            .unwrap();
        provider.profile_name = "captured-profile".into();
        provider.pricing.overrides = vec![(
            "billing-only-alias".into(),
            crate::TokenPricing::input_output(3.0, 7.0),
        )];
        let expected = provider.pricing.clone();
        let registry = ModelRegistry::from_config(crate::ClientConfig {
            providers: vec![provider],
        })
        .unwrap();
        let mut captured = registry.profile_pricing_config("captured-profile").unwrap();
        assert_eq!(captured, expected);
        captured.overrides.clear();
        assert_eq!(
            registry.profile_pricing_config("captured-profile"),
            Some(expected)
        );
        assert!(registry.profile_pricing_config("other-profile").is_none());
    }
}

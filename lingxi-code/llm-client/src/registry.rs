//! Model registry and route identity resolution.

use crate::{Capabilities, ClientConfig, LlmError, PricingModelRef, ProviderId};

/// Model entry exposed to model-picker and listing callers.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// Model capabilities for this listing.
    pub capabilities: Capabilities,
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

/// Registry that resolves requested model names to route identities.
#[derive(Debug, Clone)]
pub struct ModelRegistry {
    config: ClientConfig,
}

impl ModelRegistry {
    /// Build a registry from validated client config.
    pub fn from_config(config: ClientConfig) -> Result<Self, LlmError> {
        if config.providers.iter().any(|provider| provider.models.is_empty()) {
            return Err(LlmError::InvalidRequest {
                message: "provider profile must declare at least one model".to_string(),
            });
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
                    capabilities: model.capabilities,
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
                let profiles: Vec<&str> =
                    multiple.iter().map(|(p, _)| p.profile_name.as_str()).collect();
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
}

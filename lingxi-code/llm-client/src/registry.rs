//! Model registry and route identity resolution.

use crate::{
    reasoning_control_spec, Capabilities, ClientConfig, FailoverTriggers, LlmError, ModelProfile,
    PricingModelRef, ProviderId, ProviderProfile, ReasoningControlSpec, ReasoningTarget,
};
use platform_api::{ModelBillingMode, ModelMetadata, ModelPricing};

/// Model entry exposed to model-picker and listing callers.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelListing {
    /// Explicit provider identity.
    pub provider_id: ProviderId,
    /// Configured provider profile name.
    pub profile_name: String,
    /// Provider GROUP this profile is a connection of. Equals `profile_name`
    /// for a standalone provider, so grouping is always well-defined.
    pub group: String,
    /// This profile's connection id within the group (`"default"` when the
    /// provider declares no connections).
    pub connection_id: String,
    /// Whether this connection is a spare key slot, never offered for selection.
    pub hidden: bool,
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
    /// Sibling connections of the SAME provider group that also serve this
    /// model, in configured order, excluding the one chosen above.
    ///
    /// Empty when the caller pinned a single connection, or when the group has
    /// only one. This is the failover order the drive loop walks: every hop is a
    /// different endpoint/credential for the same logical model, so retrying
    /// along it is transparent to the caller.
    pub connection_chain: Vec<ConnectionHop>,
    /// Which failures move along [`Self::connection_chain`]. Empty unless the
    /// provider opted in, so a route with no connections is untouched.
    pub failover: FailoverTriggers,
}

/// One sibling connection to fall over to, already resolved to its profile and
/// wire model id so no second lookup is needed at failover time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionHop {
    /// Profile name of the connection to try.
    pub profile_name: String,
    /// Provider-local model value to send on that connection.
    pub request_model: String,
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
            // Extra key slots of one connection exist only to be failed over
            // onto; listing them would show the same model several times and
            // let a user "pick" a spare credential.
            .filter(|provider| !provider.connection.hidden)
            .flat_map(|provider| {
                provider.models.iter().map(|model| ModelListing {
                    provider_id: provider.provider_id.clone(),
                    profile_name: provider.profile_name.clone(),
                    group: provider.group().to_string(),
                    connection_id: provider.connection_id().to_string(),
                    hidden: provider.connection.hidden,
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

    /// Resolve a model id, optionally scoped to one provider profile or group.
    ///
    /// `profile = Some(p)` matches only connections whose profile name OR group
    /// is `p` (absent model or unknown profile → `ModelUnavailable`);
    /// `None` matches across all providers.
    ///
    /// Several matches inside ONE provider group are not ambiguous: they are the
    /// group's connections (a domestic and an international endpoint, or two API
    /// keys), which all serve the same model. The first by
    /// [`ProviderProfile::connection_sort_key`] is chosen and the rest are
    /// returned as [`ResolvedRoute::connection_chain`]. Matches spanning several
    /// groups stay ambiguous — that is a genuine "which provider did you mean".
    pub fn resolve_in(
        &self,
        requested: &str,
        profile: Option<&str>,
    ) -> Result<ResolvedRoute, LlmError> {
        let in_scope = |provider: &ProviderProfile| match profile {
            // A group name scopes to every connection in that group, so a
            // session that stored the group-qualified ref the picker showed
            // still routes.
            Some(p) => provider.profile_name == p || provider.group() == p,
            None => true,
        };
        let matches_model = |model: &ModelProfile, wanted: &str| {
            model.display_model == wanted
                || model.request_model == wanted
                || model.aliases.iter().any(|alias| alias == wanted)
        };

        let mut matches = Vec::new();
        for provider in &self.config.providers {
            if !in_scope(provider) {
                continue;
            }
            for model in &provider.models {
                if matches_model(model, requested) {
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
        //
        // The qualifier may name either a single connection (`deepseek:cn`) or a
        // whole group (`deepseek`) — pickers show the group form, so both must
        // route.
        if matches.is_empty() {
            if let Some((qualifier, provider_model)) = requested.split_once('/') {
                if profile.is_none_or(|scoped| scoped == qualifier) {
                    for provider in &self.config.providers {
                        if provider.profile_name != qualifier && provider.group() != qualifier {
                            continue;
                        }
                        for model in &provider.models {
                            if matches_model(model, provider_model) {
                                matches.push((provider, model));
                            }
                        }
                    }
                }
            }
        }

        if matches.is_empty() {
            return Err(LlmError::ModelUnavailable);
        }

        // More than one group in play is a real ambiguity; within one group the
        // extra matches are the failover chain.
        let mut groups: Vec<&str> = matches.iter().map(|(p, _)| p.group()).collect();
        groups.sort_unstable();
        groups.dedup();
        if groups.len() > 1 {
            let suggestions = groups
                .iter()
                .map(|g| format!("{g}/{requested}"))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(LlmError::InvalidRequest {
                message: format!(
                    "model reference '{requested}' is ambiguous across profiles: {} \
                     — qualify it, e.g. {}",
                    groups.join(", "),
                    suggestions
                ),
            });
        }

        matches.sort_by(|(a, _), (b, _)| a.connection_sort_key().cmp(&b.connection_sort_key()));
        let (provider, model) = matches[0];
        // Only fail over between connections billed the same way. A provider can
        // publish the same model on a subscription endpoint AND a pay-per-token
        // one (Zhipu ships exactly this: `glm-coding` and `zai` share eight model
        // ids at different billing modes). Silently moving a rate-limited
        // subscription request onto the metered endpoint would start charging
        // real money for what the user believes their plan covers, so a
        // cross-billing hop has to be an explicit choice, not a failover.
        let billing = provider.pricing.billing_mode;
        let connection_chain = matches[1..]
            .iter()
            .filter(|(p, _)| p.pricing.billing_mode == billing)
            .map(|(p, m)| ConnectionHop {
                profile_name: p.profile_name.clone(),
                request_model: m.request_model.clone(),
            })
            .collect();

        Ok(ResolvedRoute {
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
            connection_chain,
            failover: provider.connection.failover,
        })
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

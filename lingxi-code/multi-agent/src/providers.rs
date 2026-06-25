//! Candidate / arbiter model resolution (design doc §Provider 接入, Phase 4).
//!
//! Multi-agent does **not** call any vendor directly. Every candidate and the
//! arbiter reference a model with the existing `profile/model` form, and that
//! string is resolved through the workspace's `provider-config` → `llm-client`
//! routing: `provider-config::assemble` folds `settings.providers` /
//! `settings.routing` into a [`llm_client::ClientConfig`], and
//! [`llm_client::ModelRegistry`] resolves a requested model (optionally scoped
//! to a profile) into a [`llm_client::ResolvedRoute`] carrying the concrete
//! [`llm_client::ProviderId`] + wire model.
//!
//! This module is the thin bridge: it parses the `profile/model` reference and
//! resolves it against a [`ModelRegistry`], mapping any failure to a fatal
//! [`MultiAgentError::ProviderUnavailable`] (model unconfigured / unreachable →
//! fail fast, per §Error taxonomy "Fatal config error").

use crate::config::AgentEndpoint;
use crate::error::MultiAgentError;
use llm_client::ModelRegistry;
use llm_client::ProviderId;
use llm_client::ResolvedRoute;

/// A candidate (or arbiter) whose `profile/model` reference has been resolved
/// to a concrete provider route. This is the only place the abstract config
/// string becomes a real provider identity.
#[derive(Debug, Clone)]
pub struct ResolvedCandidate {
    /// Stable candidate id from config (e.g. `candidate-a`).
    pub candidate_id: String,
    /// The original `profile/model` reference, retained for telemetry / logs.
    pub model_ref: String,
    /// The resolved route (provider id, wire model, pricing identity).
    pub route: ResolvedRoute,
}

impl ResolvedCandidate {
    /// The concrete provider identity the candidate will run against.
    #[must_use]
    pub fn provider_id(&self) -> &ProviderId {
        &self.route.provider_id
    }

    /// The provider-local model string sent on the wire.
    #[must_use]
    pub fn request_model(&self) -> &str {
        &self.route.request_model
    }
}

/// Split a `profile/model` reference into `(Some(profile), model)`.
///
/// A reference with no `/` is treated as an unscoped model id (`(None, ref)`),
/// matching [`ModelRegistry::resolve`]. Only the FIRST `/` separates the
/// profile from the model, so provider-namespaced ids such as
/// `openrouter/openai/gpt-4o` keep their internal slashes in the model part.
#[must_use]
pub fn split_model_ref(model_ref: &str) -> (Option<&str>, &str) {
    match model_ref.split_once('/') {
        Some((profile, model)) if !profile.is_empty() && !model.is_empty() => {
            (Some(profile), model)
        }
        // No usable separator → resolve unscoped across all profiles.
        _ => (None, model_ref),
    }
}

/// Resolves `profile/model` references against an assembled [`ModelRegistry`].
///
/// Construct this from the same `provider-config`-assembled
/// [`llm_client::ClientConfig`] the main session uses — multi-agent never
/// builds its own provider configuration (design doc §Provider 接入).
#[derive(Debug, Clone)]
pub struct ModelResolver {
    registry: ModelRegistry,
}

impl ModelResolver {
    /// Wrap an already-assembled [`ModelRegistry`].
    #[must_use]
    pub fn new(registry: ModelRegistry) -> Self {
        Self { registry }
    }

    /// Build a resolver directly from a validated [`llm_client::ClientConfig`]
    /// (as produced by `provider-config::assemble().client_config`).
    pub fn from_client_config(
        config: llm_client::ClientConfig,
    ) -> Result<Self, MultiAgentError> {
        let registry = ModelRegistry::from_config(config).map_err(|e| {
            // A malformed client config is a fatal provider problem; surface it
            // before any candidate is launched.
            MultiAgentError::ProviderUnavailable {
                candidate_id: "<config>".to_string(),
                model: String::new(),
                reason: format!("client config invalid: {e}"),
            }
        })?;
        Ok(Self::new(registry))
    }

    /// Resolve one [`AgentEndpoint`]'s `model` reference to a [`ResolvedCandidate`].
    ///
    /// Failure is fatal for that candidate: an unconfigured or unresolvable
    /// model is a [`MultiAgentError::ProviderUnavailable`] (fail fast — the run
    /// must never silently substitute a different model).
    pub fn resolve_endpoint(
        &self,
        endpoint: &AgentEndpoint,
    ) -> Result<ResolvedCandidate, MultiAgentError> {
        let (profile, model) = split_model_ref(&endpoint.model);
        let route = self.registry.resolve_in(model, profile).map_err(|e| {
            MultiAgentError::ProviderUnavailable {
                candidate_id: endpoint.id.clone(),
                model: endpoint.model.clone(),
                reason: e.to_string(),
            }
        })?;
        Ok(ResolvedCandidate {
            candidate_id: endpoint.id.clone(),
            model_ref: endpoint.model.clone(),
            route,
        })
    }

    /// Resolve a bare `profile/model` reference (e.g. the arbiter's `model`)
    /// under an explanatory `owner` id used only for error reporting.
    pub fn resolve_ref(
        &self,
        owner: &str,
        model_ref: &str,
    ) -> Result<ResolvedRoute, MultiAgentError> {
        let (profile, model) = split_model_ref(model_ref);
        self.registry.resolve_in(model, profile).map_err(|e| {
            MultiAgentError::ProviderUnavailable {
                candidate_id: owner.to_string(),
                model: model_ref.to_string(),
                reason: e.to_string(),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm_client::AuthStrategy;
    use llm_client::Capabilities;
    use llm_client::ClientConfig;
    use llm_client::CredentialConfig;
    use llm_client::ModelProfile;
    use llm_client::PricingConfig;
    use llm_client::ProtocolFamily;
    use llm_client::ProviderProfile;

    fn model(display: &str, request: &str) -> ModelProfile {
        ModelProfile {
            display_model: display.to_string(),
            request_model: request.to_string(),
            billing_model: request.to_string(),
            aliases: Vec::new(),
            capabilities: Capabilities::default(),
        }
    }

    fn profile(name: &str, provider_id: ProviderId, models: Vec<ModelProfile>) -> ProviderProfile {
        ProviderProfile {
            provider_id,
            profile_name: name.to_string(),
            base_url: "https://example.test/v1".to_string(),
            protocol: ProtocolFamily::OpenAiChat,
            auth: AuthStrategy::ApiKey,
            credential: CredentialConfig::Static { id: name.to_string() },
            models,
            pricing: PricingConfig::default(),
            signing: None,
            azure: None,
            supports_websockets: false,
            supports_websocket_compression: false,
            websocket_connect_timeout_ms: None,
        }
    }

    fn resolver() -> ModelResolver {
        let cfg = ClientConfig {
            providers: vec![
                profile(
                    "profile-fast",
                    ProviderId::OpenAICompatible { name: "fast".into() },
                    vec![model("model-fast", "model-fast")],
                ),
                profile(
                    "profile-deep",
                    ProviderId::OpenAICompatible { name: "deep".into() },
                    vec![model("model-deep", "model-deep")],
                ),
                profile(
                    "profile-arbiter",
                    ProviderId::OpenAICompatible { name: "arb".into() },
                    vec![model("model-arbiter", "model-arbiter")],
                ),
            ],
        };
        ModelResolver::from_client_config(cfg).expect("valid config")
    }

    #[test]
    fn splits_profile_and_model() {
        assert_eq!(split_model_ref("profile-a/model-a"), (Some("profile-a"), "model-a"));
        // Only the first slash separates the profile; provider-namespaced model
        // ids keep their internal slashes.
        assert_eq!(
            split_model_ref("openrouter/openai/gpt-4o"),
            (Some("openrouter"), "openai/gpt-4o")
        );
        // No usable separator → unscoped.
        assert_eq!(split_model_ref("bare-model"), (None, "bare-model"));
        assert_eq!(split_model_ref("/model"), (None, "/model"));
        assert_eq!(split_model_ref("profile/"), (None, "profile/"));
    }

    #[test]
    fn resolves_endpoint_to_provider_route() {
        let r = resolver();
        let ep = AgentEndpoint {
            id: "fast".into(),
            label: None,
            model: "profile-fast/model-fast".into(),
            role: None,
        };
        let resolved = r.resolve_endpoint(&ep).unwrap();
        assert_eq!(resolved.candidate_id, "fast");
        assert_eq!(resolved.request_model(), "model-fast");
        assert_eq!(
            resolved.provider_id(),
            &ProviderId::OpenAICompatible { name: "fast".into() }
        );
    }

    #[test]
    fn unresolvable_model_is_provider_unavailable() {
        let r = resolver();
        let ep = AgentEndpoint {
            id: "ghost".into(),
            label: None,
            model: "profile-fast/does-not-exist".into(),
            role: None,
        };
        let err = r.resolve_endpoint(&ep).unwrap_err();
        match err {
            MultiAgentError::ProviderUnavailable { candidate_id, model, .. } => {
                assert_eq!(candidate_id, "ghost");
                assert_eq!(model, "profile-fast/does-not-exist");
            }
            other => panic!("expected ProviderUnavailable, got {other:?}"),
        }
    }

    #[test]
    fn resolves_arbiter_ref() {
        let r = resolver();
        let route = r.resolve_ref("arbiter", "profile-arbiter/model-arbiter").unwrap();
        assert_eq!(route.request_model, "model-arbiter");
    }

    #[test]
    fn invalid_client_config_is_fatal() {
        // A provider profile with no models is rejected by ModelRegistry.
        let cfg = ClientConfig {
            providers: vec![profile(
                "empty",
                ProviderId::OpenAICompatible { name: "empty".into() },
                vec![],
            )],
        };
        let err = ModelResolver::from_client_config(cfg).unwrap_err();
        assert!(matches!(err, MultiAgentError::ProviderUnavailable { .. }));
    }
}

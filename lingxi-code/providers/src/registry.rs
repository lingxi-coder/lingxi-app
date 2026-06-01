//! `ProviderRegistry` resolves a model string to a provider via `ModelSpec`,
//! caching one `LlmProvider` per profile. All three v1 codecs (`anthropic`,
//! `openai`/OpenAI-compatible, `gemini`) resolve to live providers.

use crate::anthropic::AnthropicLlmProvider;
use crate::model_spec::ModelSpec;
use crate::profile::{ProviderKind, ProviderProfile};
use crate::provider::LlmProvider;
use api_client::ApiError;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use traits::{HttpError, HttpTransport};

/// A resolved provider plus the provider-local model id (prefix stripped).
pub struct Resolved {
    /// The provider to drive the request.
    pub provider: Arc<dyn LlmProvider>,
    /// Provider-local model id (e.g. `gpt-4o`, `claude-opus-4-7`).
    pub model: String,
}

impl std::fmt::Debug for Resolved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Resolved")
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

/// Routes a model string to a provider. Object-safe so the orchestrator's
/// `ProviderApiAdapter` can hold an `Arc<dyn ModelRouter>`.
pub trait ModelRouter: Send + Sync {
    /// Resolve `model` to a provider + local model id.
    ///
    /// # Errors
    /// Returns an [`ApiError`] if the profile name is unknown.
    fn resolve(&self, model: &str) -> Result<Resolved, ApiError>;

    /// Profile names available for selection (for `/model` listing).
    fn available_profiles(&self) -> Vec<String>;
}

/// Builds and caches providers from a set of profiles, over a single shared
/// transport `T`.
pub struct ProviderRegistry<T: HttpTransport + Send + Sync + 'static> {
    profiles: BTreeMap<String, ProviderProfile>,
    env: BTreeMap<String, String>,
    transport: Arc<T>,
    cache: Mutex<BTreeMap<String, Arc<dyn LlmProvider>>>,
}

impl<T: HttpTransport + Send + Sync + 'static> ProviderRegistry<T> {
    /// Construct from a profile set (built-ins + settings), an env snapshot
    /// (for API-key lookup), and the shared transport.
    #[must_use]
    pub fn new(
        profiles: BTreeMap<String, ProviderProfile>,
        env: BTreeMap<String, String>,
        transport: Arc<T>,
    ) -> Self {
        Self {
            profiles,
            env,
            transport,
            cache: Mutex::new(BTreeMap::new()),
        }
    }

    fn api_key_for(&self, profile: &ProviderProfile) -> String {
        profile
            .api_key_env
            .as_ref()
            .and_then(|var| self.env.get(var))
            .cloned()
            .unwrap_or_default()
    }

    fn build(&self, name: &str, profile: &ProviderProfile) -> Arc<dyn LlmProvider> {
        match profile.kind {
            ProviderKind::Anthropic => {
                let key = self.api_key_for(profile);
                let provider = AnthropicLlmProvider::new(
                    key,
                    profile.base_url.clone(),
                    self.transport.clone(),
                );
                Arc::new(provider) as Arc<dyn LlmProvider>
            }
            ProviderKind::OpenAi => {
                let key = self.api_key_for(profile);
                let auth = if key.is_empty() {
                    crate::auth::Auth::None
                } else {
                    crate::auth::Auth::Bearer(key)
                };
                let authenticator = Arc::new(crate::authenticator::StaticAuth::new(auth))
                    as Arc<dyn crate::authenticator::Authenticator>;
                let codec = crate::openai::OpenAiCodec::new(profile.base_url.clone(), profile.reasoning_effort);
                let id = if name == "openai" {
                    cost::ProviderId::OpenAI
                } else {
                    cost::ProviderId::OpenAICompatible {
                        name: name.to_string(),
                    }
                };
                let client = crate::client::GenericClient::new(
                    codec,
                    authenticator,
                    self.transport.clone(),
                    id,
                    crate::capabilities::Capabilities::openai(),
                );
                Arc::new(client) as Arc<dyn crate::provider::LlmProvider>
            }
            ProviderKind::Gemini => {
                let key = self.api_key_for(profile);
                let auth = if key.is_empty() {
                    crate::auth::Auth::None
                } else {
                    crate::auth::Auth::Header {
                        name: "x-goog-api-key".to_string(),
                        value: key,
                    }
                };
                let authenticator = Arc::new(crate::authenticator::StaticAuth::new(auth))
                    as Arc<dyn crate::authenticator::Authenticator>;
                let codec = crate::gemini::GeminiCodec::new(profile.base_url.clone(), profile.thinking_budget);
                let client = crate::client::GenericClient::new(
                    codec,
                    authenticator,
                    self.transport.clone(),
                    cost::ProviderId::GoogleGemini,
                    crate::capabilities::Capabilities::gemini(),
                );
                Arc::new(client) as Arc<dyn crate::provider::LlmProvider>
            }
        }
    }
}

impl<T: HttpTransport + Send + Sync + 'static> ModelRouter for ProviderRegistry<T> {
    fn resolve(&self, model: &str) -> Result<Resolved, ApiError> {
        let spec = ModelSpec::parse(model);
        let profile = self.profiles.get(&spec.profile).ok_or_else(|| {
            ApiError::Http(HttpError::InvalidRequest(format!(
                "unknown provider profile {:?}; configure it under settings `providers`",
                spec.profile
            )))
        })?;

        // Cache one provider per profile.
        let mut cache = self.cache.lock().expect("registry cache mutex poisoned");
        if let Some(existing) = cache.get(&spec.profile) {
            return Ok(Resolved {
                provider: existing.clone(),
                model: spec.model,
            });
        }
        let provider = self.build(&spec.profile, profile);
        cache.insert(spec.profile.clone(), provider.clone());
        Ok(Resolved {
            provider,
            model: spec.model,
        })
    }

    fn available_profiles(&self) -> Vec<String> {
        self.profiles.keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::builtin_profiles;
    use crate::testutil::MockTransport;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    fn registry(extra: BTreeMap<String, ProviderProfile>) -> ProviderRegistry<MockTransport> {
        let mut profiles = builtin_profiles(Some("https://mock.local".to_string()));
        profiles.extend(extra);
        let mut env = BTreeMap::new();
        env.insert("ANTHROPIC_API_KEY".to_string(), "sk-test".to_string());
        ProviderRegistry::new(profiles, env, Arc::new(MockTransport::responding(200, "")))
    }

    #[test]
    fn resolves_anthropic_and_strips_nothing_for_bare_model() {
        let r = registry(BTreeMap::new());
        let resolved = r.resolve("claude-opus-4-7").expect("anthropic resolves");
        assert_eq!(resolved.model, "claude-opus-4-7");
        assert_eq!(resolved.provider.id(), cost::ProviderId::Anthropic);
    }

    #[test]
    fn resolves_anthropic_for_explicit_prefix_and_strips_it() {
        let r = registry(BTreeMap::new());
        let resolved = r.resolve("anthropic/claude-sonnet-4-6").expect("resolves");
        assert_eq!(resolved.model, "claude-sonnet-4-6");
        assert_eq!(resolved.provider.id(), cost::ProviderId::Anthropic);
    }

    #[test]
    fn openai_profile_resolves_now() {
        let r = registry(BTreeMap::new());
        let resolved = r.resolve("openai/gpt-4o").expect("openai resolves in P3");
        assert_eq!(resolved.model, "gpt-4o");
        assert_eq!(resolved.provider.id(), cost::ProviderId::OpenAI);
    }

    #[test]
    fn custom_openai_compatible_profile_resolves() {
        let mut extra = BTreeMap::new();
        extra.insert(
            "groq".to_string(),
            ProviderProfile {
                kind: ProviderKind::OpenAi,
                base_url: Some("https://api.groq.com/openai/v1".to_string()),
                api_key_env: Some("GROQ_API_KEY".to_string()),
                reasoning_effort: None,
                thinking_budget: None,
            },
        );
        let r = registry(extra);
        let resolved = r.resolve("groq/llama-3.3-70b").expect("groq resolves");
        assert_eq!(resolved.model, "llama-3.3-70b");
        assert_eq!(
            resolved.provider.id(),
            cost::ProviderId::OpenAICompatible {
                name: "groq".to_string()
            }
        );
    }

    #[test]
    fn unknown_profile_errors() {
        let r = registry(BTreeMap::new());
        assert!(r.resolve("nope/x").is_err());
    }

    #[test]
    fn anthropic_provider_is_cached() {
        let r = registry(BTreeMap::new());
        let a = r.resolve("claude-opus-4-7").unwrap();
        let b = r.resolve("claude-opus-4-7").unwrap();
        assert!(
            Arc::ptr_eq(&a.provider, &b.provider),
            "same Arc reused from cache"
        );
    }

    #[test]
    fn available_profiles_lists_builtins() {
        let r = registry(BTreeMap::new());
        let mut got = r.available_profiles();
        got.sort();
        assert_eq!(
            got,
            vec![
                "anthropic".to_string(),
                "gemini".to_string(),
                "openai".to_string()
            ]
        );
    }

    #[test]
    fn gemini_profile_resolves_now() {
        let r = registry(BTreeMap::new());
        let resolved = r
            .resolve("gemini/gemini-2.0-flash")
            .expect("gemini resolves in P4");
        assert_eq!(resolved.model, "gemini-2.0-flash");
        assert_eq!(resolved.provider.id(), cost::ProviderId::GoogleGemini);
    }
}

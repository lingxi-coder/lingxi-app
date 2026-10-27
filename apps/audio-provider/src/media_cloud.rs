//! Trusted profile/account lookup for the common media route selector.

use crate::AudioProviderHost;
use async_trait::async_trait;
use llm_runtime::services::sdk;
use media_api::audio::{
    AudioError, AudioErrorKind, AudioOperationContext, AudioOperationKind, AudioOwner,
};
use media_runtime::audio::HostedCloudRoute;
use media_runtime::{CloudAudioCredentialSource, CloudAudioHost, CloudBinding};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

#[derive(Clone)]
struct SessionHost {
    profile_name: String,
    account_scope: String,
    provider: Arc<AudioProviderHost>,
    binding_generation: u64,
}

struct PinnedCredentialSource {
    provider: Arc<AudioProviderHost>,
    profile_name: String,
    account_scope: String,
}

#[async_trait]
impl CloudAudioCredentialSource for PinnedCredentialSource {
    async fn request_options(
        &self,
        context: &AudioOperationContext,
    ) -> Result<sdk::RequestOptions, AudioError> {
        let credential = self
            .provider
            .credential_for_profile(&self.profile_name)
            .await
            .map_err(AudioProviderCloudRegistry::error)?;
        if context.timeout_budget_ms == Some(0) {
            return Err(AudioError::new(
                AudioErrorKind::Timeout,
                "media operation deadline has elapsed",
            ));
        }
        Ok(sdk::RequestOptions {
            credential: Some(credential),
            account_scope: Some(self.account_scope.clone()),
            total_timeout: context
                .timeout_budget_ms
                .map(std::time::Duration::from_millis),
            ..Default::default()
        })
    }
}

/// App-scoped provider registry used by MediaService's common route selector.
/// Runtime sessions register their exact SDK services and trusted account;
/// explicit profiles can be registered by app settings or credential previews.
pub struct AudioProviderCloudRegistry {
    sessions: RwLock<HashMap<String, SessionHost>>,
    profiles: RwLock<HashMap<String, Arc<AudioProviderHost>>>,
    next_binding_generation: AtomicU64,
    capture_format: &'static str,
}

impl AudioProviderCloudRegistry {
    /// Create a registry for the platform's bounded native recording format.
    #[must_use]
    pub fn new(capture_format: &'static str) -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            profiles: RwLock::new(HashMap::new()),
            next_binding_generation: AtomicU64::new(0),
            capture_format,
        }
    }

    /// Bind one session to its exact profile and usage/account scope.
    pub fn register_session(
        &self,
        session_id: impl Into<String>,
        profile_name: impl Into<String>,
        account_scope: impl Into<String>,
        provider: Arc<AudioProviderHost>,
    ) -> Result<u64, AudioError> {
        let session_id = session_id.into();
        let profile_name = profile_name.into();
        let account_scope = account_scope.into();
        if session_id.trim().is_empty()
            || profile_name.trim().is_empty()
            || account_scope.trim().is_empty()
        {
            return Err(AudioError::new(
                AudioErrorKind::InvalidRequest,
                "media session provider identity is incomplete",
            ));
        }
        let binding_generation = self
            .next_binding_generation
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .ok()
            .and_then(|previous| previous.checked_add(1))
            .ok_or_else(|| {
                AudioError::new(
                    AudioErrorKind::Unavailable,
                    "media session binding generation is exhausted",
                )
            })?;
        self.sessions
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                session_id,
                SessionHost {
                    profile_name,
                    account_scope,
                    provider,
                    binding_generation,
                },
            );
        Ok(binding_generation)
    }

    /// Remove only the session's cloud route binding.
    pub fn unregister_session(&self, session_id: &str) {
        self.sessions
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(session_id);
    }

    /// Remove one engine's binding only if it still owns the current slot for
    /// that session. A delayed close from an old engine cannot erase a newer
    /// engine's provider/profile context.
    pub fn unregister_session_if_generation(&self, session_id: &str, generation: u64) -> bool {
        let mut sessions = self.sessions.write().unwrap_or_else(|e| e.into_inner());
        if sessions
            .get(session_id)
            .is_some_and(|binding| binding.binding_generation == generation)
        {
            sessions.remove(session_id);
            true
        } else {
            false
        }
    }

    /// Resolve the exact provider host bound to the prepared Agent session.
    /// Profile and account scope are checked so concurrent engines cannot
    /// accidentally consume the most recently installed provider slot.
    pub fn provider_for_session(
        &self,
        session_id: &str,
        profile_name: &str,
        account_scope: &str,
    ) -> Result<Arc<AudioProviderHost>, AudioError> {
        let sessions = self.sessions.read().unwrap_or_else(|e| e.into_inner());
        let binding = sessions.get(session_id).ok_or_else(|| {
            AudioError::new(
                AudioErrorKind::Unavailable,
                "the prepared Agent session has no trusted media provider binding",
            )
        })?;
        if binding.profile_name != profile_name || binding.account_scope != account_scope {
            return Err(AudioError::new(
                AudioErrorKind::InvalidRequest,
                "the prepared Agent provider context does not match its bound media route",
            ));
        }
        Ok(binding.provider.clone())
    }

    /// Read the exact selected profile/account identity for one active owner.
    /// This exposes no SDK client, credential, or provider implementation.
    pub fn session_identity(&self, session_id: &str) -> Option<(String, String)> {
        self.sessions
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(session_id)
            .map(|binding| (binding.profile_name.clone(), binding.account_scope.clone()))
    }

    /// Refresh a session's trusted profile binding from the app's actual
    /// provider registry after a session/model transition.
    pub fn bind_session_to_profile(
        &self,
        session_id: impl Into<String>,
        profile_name: impl Into<String>,
        account_scope: impl Into<String>,
    ) -> Result<u64, AudioError> {
        let profile_name = profile_name.into();
        let provider = self
            .profiles
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&profile_name)
            .cloned()
            .ok_or_else(|| {
                AudioError::new(
                    AudioErrorKind::Unavailable,
                    "the selected session audio profile is not configured",
                )
            })?;
        self.register_session(session_id, profile_name, account_scope, provider)
    }

    /// Bind an explicit provider profile to a host-owned SDK service.
    pub fn register_profile(
        &self,
        profile_name: impl Into<String>,
        provider: Arc<AudioProviderHost>,
    ) -> Result<(), AudioError> {
        let profile_name = profile_name.into();
        if profile_name.trim().is_empty() {
            return Err(AudioError::new(
                AudioErrorKind::InvalidRequest,
                "explicit media profile is empty",
            ));
        }
        self.profiles
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(profile_name, provider);
        Ok(())
    }

    /// Remove an explicit profile binding after settings/account changes.
    pub fn unregister_profile(&self, profile_name: &str) {
        self.profiles
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(profile_name);
    }

    fn route_host(
        &self,
        owner: &AudioOwner,
        binding: &CloudBinding,
    ) -> Result<(Arc<AudioProviderHost>, String, String), AudioError> {
        match binding {
            CloudBinding::FollowSession => {
                let AudioOwner::Session { session_id } = owner else {
                    return Err(AudioError::new(
                        AudioErrorKind::Unavailable,
                        "follow-session audio requires a trusted conversation owner",
                    ));
                };
                let route = self
                    .sessions
                    .read()
                    .unwrap_or_else(|e| e.into_inner())
                    .get(session_id)
                    .cloned()
                    .ok_or_else(|| {
                        AudioError::new(
                            AudioErrorKind::Unavailable,
                            "the conversation has no trusted audio profile context",
                        )
                    })?;
                Ok((route.provider, route.profile_name, route.account_scope))
            }
            CloudBinding::ExplicitProfile { profile_name } => {
                if profile_name.trim().is_empty() {
                    return Err(AudioError::new(
                        AudioErrorKind::InvalidRequest,
                        "the persisted v4 cloud binding is invalid",
                    ));
                }
                if let AudioOwner::Session { session_id } = owner {
                    let route = self
                        .sessions
                        .read()
                        .unwrap_or_else(|e| e.into_inner())
                        .get(session_id)
                        .cloned()
                        .ok_or_else(|| {
                            AudioError::new(
                                AudioErrorKind::Unavailable,
                                "the conversation has no trusted audio provider context",
                            )
                        })?;
                    return Ok((
                        route.provider,
                        profile_name.clone(),
                        format!("profile:{profile_name}"),
                    ));
                }
                let provider = self
                    .profiles
                    .read()
                    .unwrap_or_else(|e| e.into_inner())
                    .get(profile_name)
                    .cloned()
                    .ok_or_else(|| {
                        AudioError::new(
                            AudioErrorKind::Unavailable,
                            "the selected media profile is not configured",
                        )
                    })?;
                Ok((
                    provider,
                    profile_name.clone(),
                    format!("profile:{profile_name}"),
                ))
            }
        }
    }

    fn error(value: serde_json::Value) -> AudioError {
        let error = &value["error"];
        let kind = match error["kind"].as_str().unwrap_or_default() {
            "unsupported" => AudioErrorKind::Unsupported,
            "invalid_request" => AudioErrorKind::InvalidRequest,
            "media_too_large" => AudioErrorKind::MediaTooLarge,
            "timeout" => AudioErrorKind::Timeout,
            "cancelled" => AudioErrorKind::Cancelled,
            _ => AudioErrorKind::Unavailable,
        };
        AudioError::new(
            kind,
            error["message"]
                .as_str()
                .unwrap_or("the selected media profile is unavailable"),
        )
    }
}

#[async_trait]
impl CloudAudioHost for AudioProviderCloudRegistry {
    fn resolve(
        &self,
        owner: &AudioOwner,
        binding: &CloudBinding,
    ) -> Result<HostedCloudRoute, AudioError> {
        let (provider, profile_name, account_scope) = self.route_host(owner, binding)?;
        let source = Arc::new(PinnedCredentialSource {
            provider: provider.clone(),
            profile_name: profile_name.clone(),
            account_scope: account_scope.clone(),
        });
        Ok(HostedCloudRoute::new(
            provider.client_snapshot(),
            profile_name,
            account_scope,
            uuid::Uuid::new_v4().to_string(),
            source,
        ))
    }

    fn capture_format(&self) -> &str {
        self.capture_format
    }

    fn catalog_supported_operations(&self) -> Vec<AudioOperationKind> {
        let mut routes = Vec::new();
        {
            let sessions = self.sessions.read().unwrap_or_else(|e| e.into_inner());
            routes.extend(sessions.values().map(|route| {
                (
                    route.provider.clone(),
                    route.profile_name.clone(),
                    route.account_scope.clone(),
                )
            }));
        }
        {
            let profiles = self.profiles.read().unwrap_or_else(|e| e.into_inner());
            routes.extend(profiles.iter().map(|(profile, provider)| {
                (
                    provider.clone(),
                    profile.clone(),
                    format!("profile:{profile}"),
                )
            }));
        }
        routes.sort_by(|left, right| (&left.1, &left.2).cmp(&(&right.1, &right.2)));
        routes.dedup_by(|left, right| {
            Arc::ptr_eq(&left.0, &right.0) && left.1 == right.1 && left.2 == right.2
        });

        let mut supported = std::collections::BTreeSet::new();
        for (provider, profile, account_scope) in routes {
            let route = sdk::audio::AudioRoute::new(profile, account_scope);
            let Ok(capabilities) = provider.client_snapshot().audio().capabilities(&route) else {
                continue;
            };
            if capabilities.supports(sdk::audio::AudioOperation::FileTranscription) {
                supported.insert(AudioOperationKind::Listen);
                supported.insert(AudioOperationKind::Transcribe);
            }
            if capabilities.supports(sdk::audio::AudioOperation::Synthesis) {
                supported.insert(AudioOperationKind::Synthesize);
                supported.insert(AudioOperationKind::Speak);
            }
        }
        supported.into_iter().collect()
    }
}

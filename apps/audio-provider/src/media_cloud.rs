//! Trusted profile/account lookup for the common media route selector.

use crate::AudioProviderHost;
use async_trait::async_trait;
use llm_runtime::services::sdk;
use media_api::audio::{
    AudioError, AudioErrorKind, AudioOperationContext, AudioOperationKind, AudioOwner,
};
use media_api::MediaAttachmentId;
use media_runtime::audio::HostedCloudRoute;
use media_runtime::{
    CloudAudioCredentialSource, CloudAudioHost, CloudBinding, MediaAttachmentPermit,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

#[derive(Clone)]
struct SessionHost {
    profile_name: String,
    account_scope: String,
    provider: Arc<AudioProviderHost>,
    binding_generation: u64,
    parent: Option<ParentSessionRoute>,
}

#[derive(Clone)]
struct ParentSessionRoute {
    key: SessionRouteKey,
    generation: u64,
    is_current: orchestrator::ScheduledMediaParentCheck,
}

#[derive(Clone, Eq, Hash, PartialEq)]
struct SessionRouteKey {
    attachment_id: Option<MediaAttachmentId>,
    session_id: String,
}

#[derive(Clone, Eq, Hash, PartialEq)]
struct AttachmentProfileKey {
    attachment_id: MediaAttachmentId,
    profile_name: String,
}

fn session_route_key(owner: &AudioOwner) -> Result<SessionRouteKey, AudioError> {
    let logical_owner = owner.logical_owner();
    if logical_owner.attachment_id().is_some() {
        return Err(AudioError::new(
            AudioErrorKind::InvalidRequest,
            "media provider route received a nested attachment owner",
        ));
    }
    let AudioOwner::Session { session_id } = logical_owner else {
        return Err(AudioError::new(
            AudioErrorKind::Unavailable,
            "cloud audio session route requires a conversation owner",
        ));
    };
    if session_id.trim().is_empty() {
        return Err(AudioError::new(
            AudioErrorKind::InvalidRequest,
            "media session provider identity is incomplete",
        ));
    }
    Ok(SessionRouteKey {
        attachment_id: owner.attachment_id().cloned(),
        session_id: session_id.clone(),
    })
}

fn session_route_key_from_permit(
    permit: &MediaAttachmentPermit,
) -> Result<SessionRouteKey, AudioError> {
    if permit.owner().attachment_id() != Some(permit.attachment_id()) {
        return Err(AudioError::new(
            AudioErrorKind::InvalidRequest,
            "media provider permit does not match its attachment",
        ));
    }
    session_route_key(permit.owner())
}

struct ScheduledRouteGuard {
    sessions: Arc<RwLock<HashMap<SessionRouteKey, SessionHost>>>,
    key: SessionRouteKey,
    generation: u64,
}

impl ScheduledRouteGuard {
    fn ensure_current(&self, context: &AudioOperationContext) -> Result<(), AudioError> {
        if session_route_key(&context.owner)? != self.key {
            return Err(AudioError::new(
                AudioErrorKind::InvalidRequest,
                "scheduled media credential context belongs to another owner",
            ));
        }
        let route = {
            let sessions = self
                .sessions
                .read()
                .unwrap_or_else(|error| error.into_inner());
            AudioProviderCloudRegistry::current_session(&sessions, &self.key).cloned()
        };
        if route.as_ref().is_some_and(|route| {
            route.binding_generation == self.generation
                && AudioProviderCloudRegistry::parent_context_current(route)
        }) {
            Ok(())
        } else {
            Err(AudioError::new(
                AudioErrorKind::Unavailable,
                "scheduled media parent binding is no longer current",
            ))
        }
    }
}

struct PinnedCredentialSource {
    provider: Arc<AudioProviderHost>,
    profile_name: String,
    account_scope: String,
    scheduled_guard: Option<ScheduledRouteGuard>,
}

#[async_trait]
impl CloudAudioCredentialSource for PinnedCredentialSource {
    async fn request_options(
        &self,
        context: &AudioOperationContext,
    ) -> Result<sdk::RequestOptions, AudioError> {
        ensure_current_profile(&self.provider, &self.profile_name)?;
        if let Some(guard) = &self.scheduled_guard {
            guard.ensure_current(context)?;
        }
        let credential = self
            .provider
            .credential_for_profile(&self.profile_name)
            .await
            .map_err(AudioProviderCloudRegistry::error)?;
        ensure_current_profile(&self.provider, &self.profile_name)?;
        if let Some(guard) = &self.scheduled_guard {
            guard.ensure_current(context)?;
        }
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

fn ensure_current_profile(provider: &AudioProviderHost, profile: &str) -> Result<(), AudioError> {
    if provider.profile_is_current(profile) {
        Ok(())
    } else {
        Err(AudioError::new(
            AudioErrorKind::Unavailable,
            "the media provider configuration changed; rebuild or restart this engine before using cloud audio",
        ))
    }
}

/// App-scoped provider registry used by MediaService's common route selector.
/// Session routes are separated by the host-issued attachment ID and logical
/// session ID; legacy unscoped routes occupy a separate namespace.
pub struct AudioProviderCloudRegistry {
    sessions: Arc<RwLock<HashMap<SessionRouteKey, SessionHost>>>,
    profiles: RwLock<HashMap<String, Arc<AudioProviderHost>>>,
    attachment_profiles: RwLock<HashMap<AttachmentProfileKey, Arc<AudioProviderHost>>>,
    next_binding_generation: AtomicU64,
    capture_format: &'static str,
}

impl AudioProviderCloudRegistry {
    /// Create a registry for the platform's bounded native recording format.
    #[must_use]
    pub fn new(capture_format: &'static str) -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            profiles: RwLock::new(HashMap::new()),
            attachment_profiles: RwLock::new(HashMap::new()),
            next_binding_generation: AtomicU64::new(0),
            capture_format,
        }
    }

    /// Bind a legacy unscoped session to its exact profile/account scope.
    /// Attachment-owned engines must use `register_attached_session`; attached
    /// owners never fall back to this separate legacy namespace.
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
        self.register_session_route(
            SessionRouteKey {
                attachment_id: None,
                session_id,
            },
            profile_name,
            account_scope,
            provider,
        )
    }

    /// Bind one host-issued attachment's session to its provider. The permit
    /// fences registration against attachment revocation and supplies the exact
    /// owner identity; session/profile/account strings cannot select an engine.
    pub fn register_attached_session(
        &self,
        permit: &MediaAttachmentPermit,
        profile_name: impl Into<String>,
        account_scope: impl Into<String>,
        provider: Arc<AudioProviderHost>,
    ) -> Result<u64, AudioError> {
        let key = session_route_key_from_permit(permit)?;
        if key.attachment_id.as_ref() != Some(permit.attachment_id()) {
            return Err(AudioError::new(
                AudioErrorKind::InvalidRequest,
                "media provider route does not match its attachment permit",
            ));
        }
        self.register_session_route(key, profile_name.into(), account_scope.into(), provider)
    }

    fn register_session_route(
        &self,
        key: SessionRouteKey,
        profile_name: String,
        account_scope: String,
        provider: Arc<AudioProviderHost>,
    ) -> Result<u64, AudioError> {
        if key.session_id.trim().is_empty()
            || profile_name.trim().is_empty()
            || account_scope.trim().is_empty()
        {
            return Err(AudioError::new(
                AudioErrorKind::InvalidRequest,
                "media session provider identity is incomplete",
            ));
        }
        let binding_generation = self.next_session_generation()?;
        self.sessions
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                key,
                SessionHost {
                    profile_name,
                    account_scope,
                    provider,
                    binding_generation,
                    parent: None,
                },
            );
        Ok(binding_generation)
    }

    fn next_session_generation(&self) -> Result<u64, AudioError> {
        self.next_binding_generation
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
            })
    }

    fn current_session<'a>(
        sessions: &'a HashMap<SessionRouteKey, SessionHost>,
        key: &SessionRouteKey,
    ) -> Option<&'a SessionHost> {
        let route = sessions.get(key)?;
        if let Some(parent) = &route.parent {
            let current_parent = sessions.get(&parent.key)?;
            if current_parent.binding_generation != parent.generation
                || current_parent.parent.is_some()
            {
                return None;
            }
        }
        Some(route)
    }

    // Host callbacks run after releasing the registry lock; an embedder cannot
    // deadlock route updates by re-entering its own session validation.
    fn parent_context_current(route: &SessionHost) -> bool {
        route
            .parent
            .as_ref()
            .is_none_or(|parent| (parent.is_current)())
    }

    /// Bind a freshly forked child to a currently registered parent route.
    /// Both permits are held while the parent session/profile/account is
    /// checked and the independent child route is inserted. The child's route
    /// becomes unusable as soon as the parent route is removed or rebound.
    pub(crate) fn register_scheduled_child_session(
        &self,
        parent_permit: &MediaAttachmentPermit,
        child_permit: &MediaAttachmentPermit,
        profile_name: &str,
        account_scope: &str,
        parent_is_current: orchestrator::ScheduledMediaParentCheck,
    ) -> Result<u64, AudioError> {
        if !parent_is_current() {
            return Err(AudioError::new(
                AudioErrorKind::Unavailable,
                "the scheduled media parent context is no longer current",
            ));
        }
        let parent_key = session_route_key_from_permit(parent_permit)?;
        let child_key = session_route_key_from_permit(child_permit)?;
        if parent_key.attachment_id.is_none()
            || child_key.attachment_id.is_none()
            || parent_key.attachment_id == child_key.attachment_id
            || profile_name.trim().is_empty()
            || account_scope.trim().is_empty()
        {
            return Err(AudioError::new(
                AudioErrorKind::InvalidRequest,
                "scheduled media route identity is incomplete",
            ));
        }
        let generation = self.next_session_generation()?;
        let mut sessions = self.sessions.write().unwrap_or_else(|e| e.into_inner());
        let parent = sessions.get(&parent_key).cloned().ok_or_else(|| {
            AudioError::new(
                AudioErrorKind::Unavailable,
                "the parent session has no trusted media provider binding",
            )
        })?;
        if parent.parent.is_some()
            || parent.profile_name != profile_name
            || parent.account_scope != account_scope
        {
            return Err(AudioError::new(
                AudioErrorKind::InvalidRequest,
                "scheduled media profile does not match the current parent binding",
            ));
        }
        if sessions.contains_key(&child_key) {
            return Err(AudioError::new(
                AudioErrorKind::InvalidRequest,
                "scheduled media child already has a session binding",
            ));
        }
        sessions.insert(
            child_key,
            SessionHost {
                profile_name: parent.profile_name,
                account_scope: parent.account_scope,
                provider: parent.provider,
                binding_generation: generation,
                parent: Some(ParentSessionRoute {
                    key: parent_key,
                    generation: parent.binding_generation,
                    is_current: parent_is_current,
                }),
            },
        );
        Ok(generation)
    }

    /// Remove only a legacy unscoped session's cloud route binding.
    /// This never removes an attachment-owned route with the same session ID.
    pub fn unregister_session(&self, session_id: &str) {
        self.sessions
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&SessionRouteKey {
                attachment_id: None,
                session_id: session_id.to_owned(),
            });
    }

    /// Remove one attachment-owned route without affecting the same logical
    /// session in another engine. Use the generation-aware form for delayed
    /// cleanup callbacks.
    pub fn unregister_attached_session(
        &self,
        attachment_id: &MediaAttachmentId,
        session_id: &str,
    ) -> bool {
        self.sessions
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&SessionRouteKey {
                attachment_id: Some(attachment_id.clone()),
                session_id: session_id.to_owned(),
            })
            .is_some()
    }

    /// Remove only the exact attachment/session slot and registration version.
    pub fn unregister_attached_session_if_generation(
        &self,
        attachment_id: &MediaAttachmentId,
        session_id: &str,
        generation: u64,
    ) -> bool {
        self.unregister_session_route_if_generation(
            &SessionRouteKey {
                attachment_id: Some(attachment_id.clone()),
                session_id: session_id.to_owned(),
            },
            generation,
        )
    }

    fn unregister_session_route_if_generation(
        &self,
        key: &SessionRouteKey,
        generation: u64,
    ) -> bool {
        let mut sessions = self.sessions.write().unwrap_or_else(|e| e.into_inner());
        if sessions
            .get(key)
            .is_some_and(|binding| binding.binding_generation == generation)
        {
            sessions.remove(key);
            true
        } else {
            false
        }
    }

    /// Remove a legacy unscoped binding by its session and generation.
    pub fn unregister_session_if_generation(&self, session_id: &str, generation: u64) -> bool {
        self.unregister_session_route_if_generation(
            &SessionRouteKey {
                attachment_id: None,
                session_id: session_id.to_owned(),
            },
            generation,
        )
    }

    /// Resolve a legacy unscoped route. Attached realtime contexts must use
    /// `provider_for_attached_session` and cannot fall back to this namespace.
    /// Profile/account are checked in addition to the session key.
    pub fn provider_for_session(
        &self,
        session_id: &str,
        profile_name: &str,
        account_scope: &str,
    ) -> Result<Arc<AudioProviderHost>, AudioError> {
        let sessions = self.sessions.read().unwrap_or_else(|e| e.into_inner());
        let binding = sessions
            .get(&SessionRouteKey {
                attachment_id: None,
                session_id: session_id.to_owned(),
            })
            .ok_or_else(|| {
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

    /// Resolve only the route belonging to one host-issued attachment.
    pub fn provider_for_attached_session(
        &self,
        attachment_id: &MediaAttachmentId,
        session_id: &str,
        profile_name: &str,
        account_scope: &str,
    ) -> Result<Arc<AudioProviderHost>, AudioError> {
        let binding = {
            let sessions = self.sessions.read().unwrap_or_else(|e| e.into_inner());
            Self::current_session(
                &sessions,
                &SessionRouteKey {
                    attachment_id: Some(attachment_id.clone()),
                    session_id: session_id.to_owned(),
                },
            )
            .cloned()
        }
        .filter(Self::parent_context_current)
        .ok_or_else(|| {
            AudioError::new(
                AudioErrorKind::Unavailable,
                "the prepared Agent attachment has no trusted media provider binding",
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
        self.session_identity_for_key(&SessionRouteKey {
            attachment_id: None,
            session_id: session_id.to_owned(),
        })
    }

    /// Read a session identity using its full attachment-scoped owner.
    pub fn session_identity_for_owner(&self, owner: &AudioOwner) -> Option<(String, String)> {
        self.session_identity_for_key(&session_route_key(owner).ok()?)
    }

    fn session_identity_for_key(&self, key: &SessionRouteKey) -> Option<(String, String)> {
        let binding = {
            let sessions = self.sessions.read().unwrap_or_else(|e| e.into_inner());
            Self::current_session(&sessions, key).cloned()
        }?;
        Self::parent_context_current(&binding)
            .then_some((binding.profile_name, binding.account_scope))
    }

    /// Refresh a legacy unscoped session from the app-wide profile registry.
    /// Attached engines must use the attachment-scoped profile table below.
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

    /// Refresh one attached session from the host's configured profile registry.
    pub fn bind_attached_session_to_profile(
        &self,
        permit: &MediaAttachmentPermit,
        profile_name: impl Into<String>,
        account_scope: impl Into<String>,
    ) -> Result<u64, AudioError> {
        let profile_name = profile_name.into();
        let key = session_route_key_from_permit(permit)?;
        let provider = self
            .attachment_profiles
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&AttachmentProfileKey {
                attachment_id: permit.attachment_id().clone(),
                profile_name: profile_name.clone(),
            })
            .cloned()
            .ok_or_else(|| {
                AudioError::new(
                    AudioErrorKind::Unavailable,
                    "the selected profile is not configured for this media attachment",
                )
            })?;
        self.register_session_route(key, profile_name, account_scope.into(), provider)
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

    /// Register a provider profile inside one engine attachment. This table is
    /// separate from app-wide explicit profiles so equal profile names in two
    /// engines cannot replace each other's SDK host.
    pub fn register_attached_profile(
        &self,
        permit: &MediaAttachmentPermit,
        profile_name: impl Into<String>,
        provider: Arc<AudioProviderHost>,
    ) -> Result<(), AudioError> {
        let key = session_route_key_from_permit(permit)?;
        let profile_name = profile_name.into();
        if key.attachment_id.as_ref() != Some(permit.attachment_id())
            || profile_name.trim().is_empty()
        {
            return Err(AudioError::new(
                AudioErrorKind::InvalidRequest,
                "attached media provider profile identity is incomplete",
            ));
        }
        self.attachment_profiles
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                AttachmentProfileKey {
                    attachment_id: permit.attachment_id().clone(),
                    profile_name,
                },
                provider,
            );
        Ok(())
    }

    /// Restore the app's configured explicit profiles for a new attachment.
    /// The caller must hold its admission permit for the full registry update.
    pub fn register_attached_shared_profiles(
        &self,
        permit: &MediaAttachmentPermit,
    ) -> Result<(), AudioError> {
        let profiles = self
            .profiles
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        for (profile, provider) in profiles {
            self.register_attached_profile(permit, profile, provider)?;
        }
        Ok(())
    }

    /// Retire all provider routes and profile hosts for one attachment after
    /// its media drain has been confirmed.
    pub fn unregister_attachment(&self, attachment_id: &MediaAttachmentId) -> (usize, usize) {
        let mut sessions = self.sessions.write().unwrap_or_else(|e| e.into_inner());
        let before_sessions = sessions.len();
        sessions.retain(|key, _| key.attachment_id.as_ref() != Some(attachment_id));
        let removed_sessions = before_sessions - sessions.len();
        drop(sessions);

        let mut profiles = self
            .attachment_profiles
            .write()
            .unwrap_or_else(|e| e.into_inner());
        let before_profiles = profiles.len();
        profiles.retain(|key, _| &key.attachment_id != attachment_id);
        (removed_sessions, before_profiles - profiles.len())
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
        let logical_owner = owner.logical_owner();
        if logical_owner.attachment_id().is_some() {
            return Err(AudioError::new(
                AudioErrorKind::InvalidRequest,
                "media provider route received a nested attachment owner",
            ));
        }
        match binding {
            CloudBinding::FollowSession => {
                let key = session_route_key(owner)?;
                let route = {
                    let sessions = self.sessions.read().unwrap_or_else(|e| e.into_inner());
                    Self::current_session(&sessions, &key).cloned()
                }
                .filter(Self::parent_context_current)
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
                if matches!(logical_owner, AudioOwner::Session { .. }) {
                    let key = session_route_key(owner)?;
                    let route = {
                        let sessions = self.sessions.read().unwrap_or_else(|e| e.into_inner());
                        Self::current_session(&sessions, &key).cloned()
                    }
                    .filter(Self::parent_context_current)
                    .ok_or_else(|| {
                        AudioError::new(
                            AudioErrorKind::Unavailable,
                            "the conversation has no trusted audio provider context",
                        )
                    })?;
                    let explicit_scope = format!("profile:{profile_name}");
                    if route.parent.is_some()
                        && (route.profile_name != *profile_name
                            || route.account_scope != explicit_scope)
                    {
                        return Err(AudioError::new(
                            AudioErrorKind::Unavailable,
                            "scheduled media profile is not authorized by the parent route",
                        ));
                    }
                    return Ok((route.provider, profile_name.clone(), explicit_scope));
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

    fn scheduled_route_guard(
        &self,
        owner: &AudioOwner,
        provider: &Arc<AudioProviderHost>,
        profile_name: &str,
        account_scope: &str,
    ) -> Result<Option<ScheduledRouteGuard>, AudioError> {
        if !matches!(owner.logical_owner(), AudioOwner::Session { .. }) {
            return Ok(None);
        }
        let key = session_route_key(owner)?;
        let route = {
            let sessions = self
                .sessions
                .read()
                .unwrap_or_else(|error| error.into_inner());
            Self::current_session(&sessions, &key).cloned()
        }
        .filter(Self::parent_context_current)
        .ok_or_else(|| {
            AudioError::new(
                AudioErrorKind::Unavailable,
                "media session route is no longer current",
            )
        })?;
        if route.parent.is_none() {
            return Ok(None);
        }
        if !Arc::ptr_eq(&route.provider, provider)
            || route.profile_name != profile_name
            || route.account_scope != account_scope
        {
            return Err(AudioError::new(
                AudioErrorKind::Unavailable,
                "scheduled media route changed while being resolved",
            ));
        }
        Ok(Some(ScheduledRouteGuard {
            sessions: self.sessions.clone(),
            key,
            generation: route.binding_generation,
        }))
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
        ensure_current_profile(&provider, &profile_name)?;
        let scheduled_guard =
            self.scheduled_route_guard(owner, &provider, &profile_name, &account_scope)?;
        let source = Arc::new(PinnedCredentialSource {
            provider: provider.clone(),
            profile_name: profile_name.clone(),
            account_scope: account_scope.clone(),
            scheduled_guard,
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

#[cfg(test)]
mod attachment_route_tests {
    use super::*;
    use media_api::{
        MediaCapabilities, MediaContent, MediaError, MediaErrorKind, MediaOperation,
        MediaOperationContext, MediaRef, MediaResult, MediaService,
    };
    use media_runtime::MediaAttachmentHost;
    use std::collections::BTreeMap;

    struct EmptyMediaService;

    #[async_trait]
    impl MediaService for EmptyMediaService {
        fn capabilities(&self) -> MediaCapabilities {
            MediaCapabilities {
                service_epoch: 1,
                support_revision: 1,
                supported_operations: Vec::new(),
                supported_media_kinds: Vec::new(),
                readiness: Vec::new(),
                max_payload_bytes: 1,
            }
        }

        async fn execute(
            &self,
            _context: MediaOperationContext,
            _operation: MediaOperation,
        ) -> Result<MediaResult, MediaError> {
            Err(MediaError::new(
                MediaErrorKind::Unsupported,
                "test media service has no operations",
            ))
        }

        async fn read_media(
            &self,
            _context: MediaOperationContext,
            _media: MediaRef,
        ) -> Result<MediaContent, MediaError> {
            Err(MediaError::new(
                MediaErrorKind::Unsupported,
                "test media service has no stored media",
            ))
        }

        async fn cancel(&self, _context: MediaOperationContext) -> Result<(), MediaError> {
            Ok(())
        }
    }

    fn provider() -> Arc<AudioProviderHost> {
        Arc::new(
            AudioProviderHost::new("{}", "international", None, BTreeMap::new())
                .expect("empty provider host is valid"),
        )
    }

    fn session_owner(session_id: &str) -> AudioOwner {
        AudioOwner::Session {
            session_id: session_id.to_owned(),
        }
    }

    #[derive(Debug)]
    struct TestCatalog(RwLock<llm_runtime::ClientConfig>);

    impl llm_runtime::client::ModelRuntimeSource for TestCatalog {
        fn current(&self, template: &llm_runtime::ModelRuntime) -> Arc<llm_runtime::ModelRuntime> {
            Arc::new(
                template
                    .reconfigured(self.0.read().unwrap().clone())
                    .unwrap(),
            )
        }
    }

    #[derive(Debug)]
    struct TestCredentials;

    impl llm_runtime::CredentialProvider for TestCredentials {
        fn load<'a>(
            &'a self,
            _: &'a llm_runtime::CredentialScope,
        ) -> llm_runtime::BoxFuture<'a, Result<llm_runtime::Credential, llm_runtime::LlmError>>
        {
            Box::pin(async { Ok(llm_runtime::Credential::ApiKey("test-key".into())) })
        }
    }

    #[derive(Debug)]
    struct CatalogChangingCredentials {
        source: Arc<TestCatalog>,
        next: llm_runtime::ClientConfig,
    }

    impl llm_runtime::CredentialProvider for CatalogChangingCredentials {
        fn load<'a>(
            &'a self,
            _: &'a llm_runtime::CredentialScope,
        ) -> llm_runtime::BoxFuture<'a, Result<llm_runtime::Credential, llm_runtime::LlmError>>
        {
            Box::pin(async move {
                *self.source.0.write().unwrap() = self.next.clone();
                Ok(llm_runtime::Credential::ApiKey("test-key".into()))
            })
        }
    }

    struct NoNetwork;
    #[async_trait]
    impl sdk::Transport for NoNetwork {
        async fn send(
            &self,
            _: sdk::HttpRequest,
        ) -> Result<sdk::StreamResponse, sdk::protocol::LlmError> {
            panic!("this test must not dispatch a provider request")
        }
    }

    #[tokio::test]
    async fn follow_session_rejects_stale_parent_and_child_audio_routes() {
        let mut openai = llm_runtime::builtin_presets()
            .providers
            .into_iter()
            .find(|profile| profile.profile_name == "openai")
            .unwrap();
        openai.credential = llm_runtime::CredentialConfig::Static {
            id: "openai".into(),
        };
        let initial = llm_runtime::ClientConfig {
            providers: vec![openai],
        };
        let source = Arc::new(TestCatalog(RwLock::new(initial.clone())));
        let template = llm_runtime::ModelRuntime::from_config(initial.clone())
            .unwrap()
            .with_credential_provider(Arc::new(TestCredentials))
            .with_catalog_source(source.clone());
        let services = template
            .provider_services(sdk::protocol::Region::International, Arc::new(NoNetwork))
            .unwrap();
        let provider = Arc::new(AudioProviderHost::from_provider_services(
            services,
            BTreeMap::from([("openai".into(), "openai".into())]),
        ));
        let registry = AudioProviderCloudRegistry::new("wav");
        let host = MediaAttachmentHost::new();
        let parent = host.attach(Arc::new(EmptyMediaService)).unwrap();
        let child = parent.fork_child().unwrap();
        let parent_permit = parent.authorize_owner(&session_owner("parent")).unwrap();
        let child_permit = child.authorize_owner(&session_owner("child")).unwrap();
        registry
            .register_attached_session(&parent_permit, "openai", "profile:openai", provider)
            .unwrap();
        registry
            .register_scheduled_child_session(
                &parent_permit,
                &child_permit,
                "openai",
                "profile:openai",
                Arc::new(|| true),
            )
            .unwrap();
        let parent_owner = parent_permit.owner().clone();
        let child_owner = child_permit.owner().clone();
        let child_context =
            AudioOperationContext::new(media_api::MediaOperationId::new(1, 1), child_owner.clone());
        let mut next = initial;
        let sdk::protocol::ServiceSetting::Enabled(audio) =
            &mut next.providers[0].wire_profile.as_mut().unwrap().audio
        else {
            panic!("OpenAI audio service is enabled in the preset")
        };
        audio.transcriptions_endpoint = "https://new-audio.example/v1/audio/transcriptions".into();
        audio.translations_endpoint = "https://new-audio.example/v1/audio/translations".into();
        audio.speech_endpoint = Some("https://new-audio.example/v1/audio/speech".into());

        let cached_child = llm_runtime::catalog_task::scope(async {
            let _ = template.current();
            let route = registry
                .resolve(&child_owner, &CloudBinding::FollowSession)
                .unwrap();
            assert!(route.request_options(&child_context).await.is_ok());
            *source.0.write().unwrap() = next;
            // A running turn retains its prior owner-scoped snapshot.
            assert!(registry
                .resolve(&parent_owner, &CloudBinding::FollowSession)
                .is_ok());
            assert!(route.request_options(&child_context).await.is_ok());
            route
        })
        .await;

        // Future operations and an old prepared route both fail before loading
        // credentials or reaching the old endpoint. Registry ownership remains.
        for owner in [&parent_owner, &child_owner] {
            let error = registry
                .resolve(owner, &CloudBinding::FollowSession)
                .err()
                .unwrap();
            assert_eq!(error.kind, AudioErrorKind::Unavailable);
            assert!(error.message.contains("restart this engine"));
        }
        assert_eq!(
            cached_child
                .request_options(&child_context)
                .await
                .err()
                .unwrap()
                .kind,
            AudioErrorKind::Unavailable,
        );
        assert!(registry.session_identity_for_owner(&parent_owner).is_some());
        assert!(registry.session_identity_for_owner(&child_owner).is_some());

        // A newly assembled engine may use the newly published route without
        // replacing either stale attachment's owner binding.
        let rebuilt = llm_runtime::ModelRuntime::from_config(source.0.read().unwrap().clone())
            .unwrap()
            .with_credential_provider(Arc::new(TestCredentials))
            .with_catalog_source(source.clone());
        let services = rebuilt
            .provider_services(sdk::protocol::Region::International, Arc::new(NoNetwork))
            .unwrap();
        let fresh_provider = Arc::new(AudioProviderHost::from_provider_services(
            services,
            BTreeMap::from([("openai".into(), "openai".into())]),
        ));
        let fresh = host.attach(Arc::new(EmptyMediaService)).unwrap();
        let fresh_permit = fresh.authorize_owner(&session_owner("fresh")).unwrap();
        registry
            .register_attached_session(&fresh_permit, "openai", "profile:openai", fresh_provider)
            .unwrap();
        let fresh_route = registry
            .resolve(fresh_permit.owner(), &CloudBinding::FollowSession)
            .unwrap();
        let sdk::protocol::ServiceSetting::Enabled(audio) =
            &fresh_route.snapshot.profile("openai").unwrap().audio
        else {
            panic!("the rebuilt audio route is unavailable")
        };
        assert_eq!(
            audio.transcriptions_endpoint,
            "https://new-audio.example/v1/audio/transcriptions"
        );
        assert_eq!(
            registry
                .resolve(&child_owner, &CloudBinding::FollowSession)
                .err()
                .unwrap()
                .kind,
            AudioErrorKind::Unavailable,
        );
    }

    #[tokio::test]
    async fn catalog_change_while_loading_credential_fails_before_dispatch() {
        let mut openai = llm_runtime::builtin_presets()
            .providers
            .into_iter()
            .find(|profile| profile.profile_name == "openai")
            .unwrap();
        openai.credential = llm_runtime::CredentialConfig::Static {
            id: "openai".into(),
        };
        let initial = llm_runtime::ClientConfig {
            providers: vec![openai],
        };
        let mut next = initial.clone();
        let sdk::protocol::ServiceSetting::Enabled(audio) =
            &mut next.providers[0].wire_profile.as_mut().unwrap().audio
        else {
            panic!("OpenAI audio service is enabled in the preset")
        };
        audio.transcriptions_endpoint = "https://new-audio.example/v1/audio/transcriptions".into();
        audio.translations_endpoint = "https://new-audio.example/v1/audio/translations".into();
        audio.speech_endpoint = Some("https://new-audio.example/v1/audio/speech".into());
        let source = Arc::new(TestCatalog(RwLock::new(initial.clone())));
        let template = llm_runtime::ModelRuntime::from_config(initial)
            .unwrap()
            .with_credential_provider(Arc::new(CatalogChangingCredentials {
                source: source.clone(),
                next,
            }))
            .with_catalog_source(source);
        let services = template
            .provider_services(sdk::protocol::Region::International, Arc::new(NoNetwork))
            .unwrap();
        let provider = Arc::new(AudioProviderHost::from_provider_services(
            services,
            BTreeMap::from([("openai".into(), "openai".into())]),
        ));
        let pinned = PinnedCredentialSource {
            provider,
            profile_name: "openai".into(),
            account_scope: "profile:openai".into(),
            scheduled_guard: None,
        };
        let context = AudioOperationContext::new(
            media_api::MediaOperationId::new(2, 1),
            session_owner("parent"),
        );
        assert_eq!(
            pinned.request_options(&context).await.err().unwrap().kind,
            AudioErrorKind::Unavailable,
        );
    }

    #[test]
    fn scheduled_parent_check_runs_outside_registry_locks() {
        let registry = Arc::new(AudioProviderCloudRegistry::new("wav"));
        let host = MediaAttachmentHost::new();
        let parent = host.attach(Arc::new(EmptyMediaService)).unwrap();
        let child = parent.fork_child().unwrap();
        let parent_owner = session_owner("parent");
        let child_owner = session_owner("child");
        let parent_permit = parent.authorize_owner(&parent_owner).unwrap();
        let child_permit = child.authorize_owner(&child_owner).unwrap();
        registry
            .register_attached_session(&parent_permit, "openai", "profile:openai", provider())
            .unwrap();
        let weak_registry = Arc::downgrade(&registry);
        let check: orchestrator::ScheduledMediaParentCheck = Arc::new(move || {
            let Some(registry) = weak_registry.upgrade() else {
                return false;
            };
            let available = registry.sessions.try_write().is_ok();
            available
        });
        registry
            .register_scheduled_child_session(
                &parent_permit,
                &child_permit,
                "openai",
                "profile:openai",
                check,
            )
            .unwrap();
        assert!(registry
            .provider_for_attached_session(child.id(), "child", "openai", "profile:openai",)
            .is_ok());
    }

    #[test]
    fn identical_session_profile_and_account_are_isolated_by_attachment() {
        let registry = AudioProviderCloudRegistry::new("wav");
        let host = MediaAttachmentHost::new();
        let service: Arc<dyn MediaService> = Arc::new(EmptyMediaService);
        let first = host
            .attach(service.clone())
            .expect("first engine attachment");
        let second = host.attach(service).expect("second engine attachment");
        let first_permit = first
            .authorize_owner(&session_owner("same-session"))
            .expect("first engine route permit");
        let second_permit = second
            .authorize_owner(&session_owner("same-session"))
            .expect("second engine route permit");
        let first_provider = provider();
        let second_provider = provider();
        let legacy_provider = provider();

        registry
            .register_attached_profile(&first_permit, "same-profile", first_provider.clone())
            .expect("first engine profile");
        registry
            .register_attached_profile(&second_permit, "same-profile", second_provider.clone())
            .expect("second engine profile");
        let first_generation = registry
            .bind_attached_session_to_profile(&first_permit, "same-profile", "same-account")
            .expect("first engine route");
        let second_generation = registry
            .bind_attached_session_to_profile(&second_permit, "same-profile", "same-account")
            .expect("second engine route");
        let legacy_generation = registry
            .register_session(
                "same-session",
                "same-profile",
                "same-account",
                legacy_provider.clone(),
            )
            .expect("legacy route occupies its separate unscoped namespace");
        let shared_profile_provider = provider();
        registry
            .register_profile("same-profile", shared_profile_provider.clone())
            .expect("legacy UI profile remains app-shared");

        let (first_route, first_profile, first_account) = registry
            .route_host(first_permit.owner(), &CloudBinding::FollowSession)
            .expect("first attachment routes");
        let (second_route, second_profile, second_account) = registry
            .route_host(second_permit.owner(), &CloudBinding::FollowSession)
            .expect("second attachment routes");
        assert!(Arc::ptr_eq(&first_route, &first_provider));
        assert!(Arc::ptr_eq(&second_route, &second_provider));
        assert_eq!(
            (first_profile.as_str(), first_account.as_str()),
            ("same-profile", "same-account")
        );
        assert_eq!(
            (second_profile.as_str(), second_account.as_str()),
            ("same-profile", "same-account")
        );
        assert!(Arc::ptr_eq(
            &registry
                .provider_for_attached_session(
                    first.id(),
                    "same-session",
                    "same-profile",
                    "same-account",
                )
                .expect("prepared context uses first attachment"),
            &first_provider,
        ));
        assert!(Arc::ptr_eq(
            &registry
                .provider_for_attached_session(
                    second.id(),
                    "same-session",
                    "same-profile",
                    "same-account",
                )
                .expect("prepared context uses second attachment"),
            &second_provider,
        ));
        assert!(Arc::ptr_eq(
            &registry
                .provider_for_session("same-session", "same-profile", "same-account")
                .expect("legacy lookup sees only the unscoped route"),
            &legacy_provider,
        ));
        let (legacy_route, _, _) = registry
            .route_host(&session_owner("same-session"), &CloudBinding::FollowSession)
            .expect("legacy owner sees only its legacy route");
        assert!(Arc::ptr_eq(&legacy_route, &legacy_provider));
        assert!(registry
            .provider_for_attached_session(
                second.id(),
                "same-session",
                "same-profile",
                "other-account",
            )
            .is_err());
        let (first_explicit, _, _) = registry
            .route_host(
                first_permit.owner(),
                &CloudBinding::ExplicitProfile {
                    profile_name: "same-profile".into(),
                },
            )
            .expect("attached explicit profile stays on the first engine provider");
        let (second_explicit, _, _) = registry
            .route_host(
                second_permit.owner(),
                &CloudBinding::ExplicitProfile {
                    profile_name: "same-profile".into(),
                },
            )
            .expect("attached explicit profile stays on the second engine provider");
        assert!(Arc::ptr_eq(&first_explicit, &first_provider));
        assert!(Arc::ptr_eq(&second_explicit, &second_provider));
        let (ui_explicit, _, _) = registry
            .route_host(
                &AudioOwner::Ui {
                    instance_id: "settings-preview".into(),
                },
                &CloudBinding::ExplicitProfile {
                    profile_name: "same-profile".into(),
                },
            )
            .expect("non-session settings preview still uses app-shared profile");
        assert!(Arc::ptr_eq(&ui_explicit, &shared_profile_provider));
        registry.unregister_profile("same-profile");
        let (first_after_shared_removal, _, _) = registry
            .route_host(first_permit.owner(), &CloudBinding::FollowSession)
            .expect("removing the shared profile does not remove active scoped routes");
        assert!(Arc::ptr_eq(&first_after_shared_removal, &first_provider));

        assert!(registry.unregister_attached_session_if_generation(
            first.id(),
            "same-session",
            first_generation,
        ));
        assert!(registry
            .route_host(first_permit.owner(), &CloudBinding::FollowSession)
            .is_err());
        let (second_after_first_cleanup, _, _) = registry
            .route_host(second_permit.owner(), &CloudBinding::FollowSession)
            .expect("first engine cleanup cannot remove second engine route");
        assert!(Arc::ptr_eq(&second_after_first_cleanup, &second_provider));
        assert!(registry.unregister_attached_session_if_generation(
            second.id(),
            "same-session",
            second_generation,
        ));
        assert_eq!(registry.unregister_attachment(first.id()), (0, 1));
        assert!(registry
            .provider_for_session("same-session", "same-profile", "same-account")
            .is_ok());
        assert!(registry.unregister_session_if_generation("same-session", legacy_generation));
    }

    #[test]
    fn stale_generation_and_nested_owner_cannot_select_or_delete_another_route() {
        let registry = AudioProviderCloudRegistry::new("wav");
        let host = MediaAttachmentHost::new();
        let service: Arc<dyn MediaService> = Arc::new(EmptyMediaService);
        let first = host.attach(service.clone()).expect("first attachment");
        let second = host.attach(service).expect("second attachment");
        let first_permit = first
            .authorize_owner(&session_owner("same-session"))
            .unwrap();
        let second_permit = second
            .authorize_owner(&session_owner("same-session"))
            .unwrap();
        let stale_generation = registry
            .register_attached_session(&first_permit, "first-old", "same-account", provider())
            .unwrap();
        let first_current = provider();
        let current_generation = registry
            .register_attached_session(
                &first_permit,
                "first-current",
                "same-account",
                first_current.clone(),
            )
            .unwrap();
        let second_provider = provider();
        let second_generation = registry
            .register_attached_session(
                &second_permit,
                "second-current",
                "same-account",
                second_provider.clone(),
            )
            .unwrap();

        assert!(!registry.unregister_attached_session_if_generation(
            first.id(),
            "same-session",
            stale_generation,
        ));
        let (first_after_stale_cleanup, profile, _) = registry
            .route_host(first_permit.owner(), &CloudBinding::FollowSession)
            .unwrap();
        assert!(Arc::ptr_eq(&first_after_stale_cleanup, &first_current));
        assert_eq!(profile, "first-current");

        let nested = session_owner("same-session")
            .in_attachment(first.id().clone())
            .in_attachment(second.id().clone());
        assert!(registry
            .route_host(&nested, &CloudBinding::FollowSession)
            .is_err());
        assert!(!registry.unregister_attached_session_if_generation(
            first.id(),
            "same-session",
            second_generation,
        ));
        let (second_after_wrong_generation, _, _) = registry
            .route_host(second_permit.owner(), &CloudBinding::FollowSession)
            .unwrap();
        assert!(Arc::ptr_eq(
            &second_after_wrong_generation,
            &second_provider
        ));

        assert!(registry.unregister_attached_session_if_generation(
            first.id(),
            "same-session",
            current_generation,
        ));
        assert!(registry.unregister_attached_session_if_generation(
            second.id(),
            "same-session",
            second_generation,
        ));
        assert_eq!(registry.unregister_attachment(first.id()), (0, 0));
    }
}

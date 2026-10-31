#[cfg(feature = "uniffi")]
use super::callbacks::{
    IosAudioService, IosCamera, IosClipboard, IosDeviceControl, IosEventListener,
    IosListenerBridge, IosLocation, IosMediaDevicePorts, IosNotification, IosPermissionSink,
    IosSecureStorage, IosShare,
};
#[cfg(feature = "uniffi")]
use super::callbacks::{IosAudioServiceBridge, IosCameraBridge, IosMediaDevicePortsBridge};
#[cfg(all(feature = "uniffi", target_os = "ios"))]
use super::callbacks::{
    IosClipboardBridge, IosDeviceControlBridge, IosLocationBridge, IosNotificationBridge,
    IosPermissionSinkBridge, IosSecureStorageBridge, IosShareBridge,
};
#[cfg(all(feature = "uniffi", target_os = "ios"))]
use super::configuration::{
    ios_mobile_config_from_launch_config, ios_project_mount_grants,
    mobile_brand_profile_from_directories, validate_mobile_linux_workspace_config,
};
#[cfg(feature = "uniffi")]
use super::configuration::{ios_project_cwd, IosEngineLaunchConfigFfi, IosMobileLinuxConfigFfi};
#[cfg(all(feature = "uniffi", target_os = "ios"))]
use super::linux_runtime::{
    ios_mobile_linux_runtime, ios_mobile_linux_runtime_with_project_mounts,
};
#[cfg(all(feature = "uniffi", target_os = "ios"))]
use super::linux_types::MobileLinuxRuntimeModeFfi;
#[cfg(feature = "uniffi")]
use client::protocol::text::ClientTextDto;
#[cfg(all(feature = "uniffi", target_os = "ios"))]
use harness_runtime::mobile::MobileConfig;
#[cfg(feature = "uniffi")]
use harness_runtime::mobile::{
    ClientEventListener, MediaAttachmentHandle, MediaAttachmentHost, MediaAttachmentPhase,
    MediaAttachmentRevocation, MobileCronStoreHandle, MobileEngineError, MobileEngineHandle,
    PermissionRequestSink, SessionModeDto,
};
#[cfg(all(feature = "uniffi", target_os = "ios"))]
use lingxi_core::host::Platform;
use lingxi_core::host::{AudioService, CameraControl, SharingService};
use std::sync::Arc;

#[cfg(feature = "uniffi")]
use audio_provider::{
    media_cloud::AudioProviderCloudRegistry,
    media_configuration::audio_routing_settings_from_v4_json_checked, media_host::AppMediaHost,
    media_native::CallbackMediaDevicePorts, media_store::BoundedMediaResourceStore,
    AppMediaConversationAttachment, AudioConfigurationSnapshot, AudioProviderHost,
};
#[cfg(feature = "uniffi")]
use media_api::{MediaDevicePorts, MediaResourceStore, MediaSessionMode};
#[cfg(feature = "uniffi")]
use std::collections::HashMap;
#[cfg(feature = "uniffi")]
use std::sync::Weak;

/// Keeps the latest provider binding alive exactly as long as the engine's
/// listener bridge. Stale teardown cannot remove a newer registry generation.
#[cfg(feature = "uniffi")]
struct MediaAudioSessionBindingGuard {
    registry: Arc<AudioProviderCloudRegistry>,
    attachment: MediaAttachmentHandle,
    binding: std::sync::Mutex<Option<MediaAudioSessionBindingFfi>>,
    refresh: tokio::sync::Mutex<()>,
}

#[cfg(feature = "uniffi")]
impl MediaAudioSessionBindingGuard {
    fn new(registry: Arc<AudioProviderCloudRegistry>, attachment: MediaAttachmentHandle) -> Self {
        Self {
            registry,
            attachment,
            binding: std::sync::Mutex::new(None),
            refresh: tokio::sync::Mutex::new(()),
        }
    }

    fn identity(&self) -> (String, u64) {
        (
            self.attachment.id().host_id(),
            self.attachment.id().generation(),
        )
    }

    fn matches_attachment(&self, binding: &MediaAudioSessionBindingFfi) -> bool {
        let (host_id, generation) = self.identity();
        binding.attachment_host_id == host_id && binding.attachment_generation == generation
    }

    /// The caller holds an attachment admission permit until this update and
    /// the matching registry writes are complete.
    fn update(&self, binding: MediaAudioSessionBindingFfi) -> bool {
        if !self.matches_attachment(&binding) {
            return false;
        }
        replace_attached_audio_session_binding(
            &self.registry,
            self.attachment.id(),
            &self.binding,
            binding,
        );
        true
    }

    fn remove_session_context(&self, binding: &MediaAudioSessionBindingFfi) -> bool {
        if !self.matches_attachment(binding) {
            return false;
        }
        let owner = media_api::MediaOwner::Session {
            session_id: binding.session_id.clone(),
        };
        let Ok(_permit) = self.attachment.authorize_owner(&owner) else {
            // Revoking/Unknown keeps the route alive. The exact confirmed-drain
            // callback releases this host-owned route entry.
            return false;
        };
        let mut current = self
            .binding
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if current.as_ref() != Some(binding) {
            return false;
        }
        let removed = self.registry.unregister_attached_session_if_generation(
            self.attachment.id(),
            &binding.session_id,
            binding.generation,
        );
        if removed {
            *current = None;
        }
        removed
    }

    fn clear(&self) {
        let binding = self
            .binding
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        if let Some(binding) = binding {
            self.remove_session_context(&binding);
        }
    }
}

#[cfg(feature = "uniffi")]
impl Drop for MediaAudioSessionBindingGuard {
    fn drop(&mut self) {
        if self.attachment.phase() == MediaAttachmentPhase::Revoked {
            self.registry.unregister_attachment(self.attachment.id());
            self.binding
                .get_mut()
                .unwrap_or_else(|error| error.into_inner())
                .take();
            return;
        }
        if let Some(binding) = self
            .binding
            .get_mut()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
        {
            // While an attachment is Revoking or Unknown, removal is rejected.
            // It is normally held by `attachment_routes` until confirmed drain.
            self.remove_session_context(&binding);
        }
    }
}

#[cfg(feature = "uniffi")]
fn retain_media_route_guard(
    guard: Option<&Arc<MediaAudioSessionBindingGuard>>,
) -> Option<Arc<dyn std::any::Any + Send + Sync>> {
    guard.map(|guard| guard.clone() as Arc<dyn std::any::Any + Send + Sync>)
}

/// Exact provider-route slot installed by one engine attachment. Stale
/// teardown cannot remove a newer route for the same session ID.
#[cfg(feature = "uniffi")]
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MediaAudioSessionBindingFfi {
    pub attachment_host_id: String,
    pub attachment_generation: u64,
    pub session_id: String,
    pub generation: u64,
}

fn replace_attached_audio_session_binding(
    registry: &AudioProviderCloudRegistry,
    attachment_id: &media_api::MediaAttachmentId,
    current: &std::sync::Mutex<Option<MediaAudioSessionBindingFfi>>,
    binding: MediaAudioSessionBindingFfi,
) {
    let mut current = current.lock().unwrap_or_else(|error| error.into_inner());
    if let Some(previous) = current.replace(binding) {
        registry.unregister_attached_session_if_generation(
            attachment_id,
            &previous.session_id,
            previous.generation,
        );
    }
}

// Keep the unscoped helpers for legacy-only unit coverage. Engine production
// routes use the attachment-aware helpers above.
#[cfg(test)]
fn replace_audio_session_binding(
    registry: &AudioProviderCloudRegistry,
    current: &std::sync::Mutex<Option<MediaAudioSessionBindingFfi>>,
    binding: MediaAudioSessionBindingFfi,
) {
    let mut current = current.lock().unwrap_or_else(|error| error.into_inner());
    if let Some(previous) = current.replace(binding) {
        registry.unregister_session_if_generation(&previous.session_id, previous.generation);
    }
}

#[cfg(test)]
fn clear_audio_session_binding(
    registry: &AudioProviderCloudRegistry,
    current: &std::sync::Mutex<Option<MediaAudioSessionBindingFfi>>,
) {
    let mut current = current.lock().unwrap_or_else(|error| error.into_inner());
    if let Some(previous) = current.take() {
        registry.unregister_session_if_generation(&previous.session_id, previous.generation);
    }
}

/// Create a RAM-only side using the same typed callbacks as the main chat.
#[cfg(feature = "uniffi")]
#[uniffi::export(async_runtime = "tokio")]
pub async fn create_ios_side_chat(
    engine: Arc<MobileEngineHandle>,
    request_id: String,
    initial_input: Option<ClientTextDto>,
    listener: Box<dyn IosEventListener>,
    permissions: Box<dyn IosPermissionSink>,
) -> Result<harness_runtime::mobile::SideChatInfoDto, MobileEngineError> {
    engine
        .create_side_chat(
            request_id,
            initial_input,
            Arc::new(IosListenerBridge {
                inner: listener,
                _media_route_guard: None,
            }),
            Arc::new(super::callbacks::IosPermissionSinkBridge { inner: permissions }),
        )
        .await
}

/// The foreign (Swift) capability objects + config the engine needs to build an
/// `IosPlatform`. `UniFFI` marshals each `Arc<dyn …>` as a callback-interface
/// reference; `app_sandbox_root` is the app container path.
pub struct PlatformImpls {
    /// Swift `CameraControl` impl.
    pub camera: Arc<dyn CameraControl>,
    /// Unified Swift device AudioService.
    pub audio: Arc<dyn AudioService>,
    /// Swift `SharingService` impl.
    pub share: Arc<dyn SharingService>,
    /// Swift Keychain-backed `SecureStorage` impl, if provided. When `None` the
    /// composition root falls back to the non-persisting development stub.
    pub secure_storage: Option<Arc<dyn lingxi_core::host::SecureStorage>>,
    /// The app's writable sandbox container root.
    pub app_sandbox_root: String,
    /// Optional mobile-linux runtime configuration.
    pub mobile_linux: Option<IosMobileLinuxConfigFfi>,
}

/// One explicit app-composed media host shared by native engines and Remote
/// Control. It owns a single media service, audio route registry, device-port
/// callback, and owner-scoped resource store for the iOS process.
#[cfg(feature = "uniffi")]
#[derive(uniffi::Object)]
pub struct IosNativeMediaHost {
    inner: Arc<AppMediaHost>,
    audio: Arc<dyn AudioService>,
    camera: Arc<dyn CameraControl>,
    media_devices: Arc<IosMediaDevicePortsBridge>,
    audio_configuration_json: Arc<std::sync::RwLock<String>>,
    media_attachment_host: MediaAttachmentHost,
    /// Keeps provider registries and attachment handles alive until confirmed
    /// drain. Revoking/Unknown never removes an entry from this map.
    attachment_routes:
        Arc<std::sync::Mutex<HashMap<(String, u64), Arc<MediaAudioSessionBindingGuard>>>>,
    orphaned_attachment_routes:
        Arc<std::sync::Mutex<HashMap<usize, Arc<MediaAudioSessionBindingGuard>>>>,
    route_guards: std::sync::Mutex<HashMap<usize, Weak<MediaAudioSessionBindingGuard>>>,
}

#[cfg(feature = "uniffi")]
#[uniffi::export]
impl IosNativeMediaHost {
    /// Construct the app-scoped host from the app's existing audio, camera,
    /// and shared native media-device callbacks plus current v4 settings.
    #[uniffi::constructor]
    pub fn new(
        audio: Box<dyn IosAudioService>,
        camera: Box<dyn IosCamera>,
        media_devices: Box<dyn IosMediaDevicePorts>,
        media_audio_configuration_json: String,
    ) -> Result<Arc<Self>, MobileEngineError> {
        let native_audio: Arc<dyn harness_runtime::mobile::NativeAudioService> =
            Arc::new(IosAudioServiceBridge { inner: audio });
        let audio = harness_runtime::mobile::from_native_audio_service(native_audio);
        let camera: Arc<dyn CameraControl> = Arc::new(IosCameraBridge { inner: camera });
        let local_modes = media_devices
            .supported_session_modes()
            .into_iter()
            .map(|mode| match mode {
                client::protocol::media::MediaSessionModeDto::Audio => MediaSessionMode::Audio,
                client::protocol::media::MediaSessionModeDto::Video => MediaSessionMode::Video,
                client::protocol::media::MediaSessionModeDto::AudioVideo => {
                    MediaSessionMode::AudioVideo
                }
            })
            .collect();
        let media_devices = Arc::new(IosMediaDevicePortsBridge {
            inner: media_devices,
        });
        let ports: Arc<dyn MediaDevicePorts> =
            Arc::new(CallbackMediaDevicePorts::new(media_devices.clone()));
        let settings = audio_routing_settings_from_v4_json_checked(&media_audio_configuration_json)
            .map_err(|error| {
                MobileEngineError::Internal(format!(
                    "iOS media audio settings are invalid: {error}"
                ))
            })?;
        let cloud_audio = Arc::new(AudioProviderCloudRegistry::new("wav"));
        let capabilities = audio.capabilities();
        let inner = AppMediaHost::new(
            Arc::new(BoundedMediaResourceStore::default()),
            audio.clone(),
            Some(camera.clone()),
            ports,
            local_modes,
            cloud_audio,
            Vec::new(),
            settings,
            capabilities.service_epoch.max(1),
            capabilities.max_payload_bytes.max(1),
        )
        .map_err(|error| MobileEngineError::Internal(error.to_string()))?;
        let audio_configuration_json =
            Arc::new(std::sync::RwLock::new(media_audio_configuration_json));
        Ok(Arc::new(Self {
            inner,
            audio,
            camera,
            media_devices,
            audio_configuration_json,
            media_attachment_host: MediaAttachmentHost::new(),
            attachment_routes: Arc::new(std::sync::Mutex::new(HashMap::new())),
            orphaned_attachment_routes: Arc::new(std::sync::Mutex::new(HashMap::new())),
            route_guards: std::sync::Mutex::new(HashMap::new()),
        }))
    }

    /// Replace v4 route settings without replacing the host or any active
    /// owner-scoped resources.
    pub fn update_audio_settings(
        &self,
        media_audio_configuration_json: String,
    ) -> Result<(), MobileEngineError> {
        let mut current_json = self
            .audio_configuration_json
            .write()
            .unwrap_or_else(|error| error.into_inner());
        self.inner
            .update_audio_routing_v4(&media_audio_configuration_json)
            .map_err(|error| {
                MobileEngineError::Internal(format!(
                    "iOS media audio settings are invalid: {error}"
                ))
            })?;
        *current_json = media_audio_configuration_json;
        Ok(())
    }

    /// Refresh trusted provider/session bindings after an engine attaches or
    /// changes session. Provider credentials remain inside the engine/runtime.
    /// An unconfigured inference route returns `None` without registering a
    /// cloud binding; system and offline audio remain available independently.
    #[uniffi::method(async_runtime = "tokio")]
    pub async fn update_audio_session_context(
        &self,
        engine: Arc<MobileEngineHandle>,
    ) -> Result<Option<MediaAudioSessionBindingFfi>, MobileEngineError> {
        let engine_key = Arc::as_ptr(&engine) as usize;
        let guard = self
            .route_guards
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(&engine_key)
            .and_then(Weak::upgrade)
            .ok_or_else(|| {
                MobileEngineError::Internal(
                    "iOS engine has no attachment issued by this media host".into(),
                )
            })?;
        // Serialize refreshes for this engine so an older unconfigured result
        // cannot retire a newer successful binding.
        let _refresh = guard.refresh.lock().await;
        let context = match engine.current_media_context().await {
            Ok(context) => context,
            Err(_) => {
                guard.clear();
                return Ok(None);
            }
        };
        let owner = media_api::MediaOwner::Session {
            session_id: context.session_id.clone(),
        };
        // Admission fences the complete profile/session registry update against
        // attachment revoke. A stale engine cannot reinstall provider routes.
        let permit = guard
            .attachment
            .authorize_owner(&owner)
            .map_err(|error| MobileEngineError::Internal(error.to_string()))?;
        let session_id = context.session_id.clone();
        let (services, credential_ids, _) = engine.audio_provider_services();
        let provider = Arc::new(AudioProviderHost::from_provider_services(
            services,
            credential_ids.clone(),
        ));
        for profile_name in credential_ids.keys() {
            guard
                .registry
                .register_attached_profile(&permit, profile_name.clone(), provider.clone())
                .map_err(|error| MobileEngineError::Internal(error.to_string()))?;
        }
        let generation = guard
            .registry
            .register_attached_session(
                &permit,
                context.profile_id.clone(),
                context.account_scope.clone(),
                provider.clone(),
            )
            .map_err(|error| MobileEngineError::Internal(error.to_string()))?;
        let (attachment_host_id, attachment_generation) = guard.identity();
        let binding = MediaAudioSessionBindingFfi {
            attachment_host_id,
            attachment_generation,
            session_id,
            generation,
        };
        if !guard.update(binding.clone()) {
            guard.registry.unregister_attached_session_if_generation(
                guard.attachment.id(),
                &binding.session_id,
                binding.generation,
            );
            return Err(MobileEngineError::Internal(
                "iOS media provider route does not match the engine attachment".into(),
            ));
        }
        Ok(Some(binding))
    }

    /// Remove a provider route only when the exact host-issued attachment and
    /// session registration still match. Revoking/Unknown retains the route.
    pub fn remove_audio_session_context(
        &self,
        engine: Arc<MobileEngineHandle>,
        binding: MediaAudioSessionBindingFfi,
    ) {
        let engine_key = Arc::as_ptr(&engine) as usize;
        let route = self
            .route_guards
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(&engine_key)
            .and_then(Weak::upgrade);
        if let Some(route) = route {
            route.remove_session_context(&binding);
        }
    }
}

#[cfg(feature = "uniffi")]
impl IosNativeMediaHost {
    fn create_engine_media_scope(
        &self,
    ) -> Result<
        (
            Arc<MediaAudioSessionBindingGuard>,
            Arc<dyn harness_runtime::mobile::MediaConversationAttachment>,
        ),
        MobileEngineError,
    > {
        harness_runtime::mobile::initialize_mobile_lifecycle_runtime()
            .map_err(MobileEngineError::Internal)?;
        let (attachment, registry) = self
            .inner
            .create_mobile_media_attachment(&self.media_attachment_host)
            .map_err(|error| MobileEngineError::Internal(error.to_string()))?;
        let binding = Arc::new(MediaAudioSessionBindingGuard::new(
            registry.clone(),
            attachment.clone(),
        ));
        let key = binding.identity();
        let inserted = {
            let mut routes = self
                .attachment_routes
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            match routes.entry(key) {
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(binding.clone());
                    true
                }
                std::collections::hash_map::Entry::Occupied(_) => false,
            }
        };
        if !inserted {
            // The host promises unique generations. A collision is fatal and
            // must not select or overwrite an existing engine's route.
            self.orphaned_attachment_routes
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(Arc::as_ptr(&binding) as usize, binding.clone());
            self.revoke_unbuilt_attachment(binding);
            return Err(MobileEngineError::Internal(
                "iOS media attachment identity was already registered".into(),
            ));
        }

        let configuration: AudioConfigurationSnapshot = {
            let settings = self.audio_configuration_json.clone();
            Arc::new(move || {
                Ok(settings
                    .read()
                    .unwrap_or_else(|error| error.into_inner())
                    .clone())
            })
        };
        let conversation_attachment =
            AppMediaConversationAttachment::new(registry, attachment.clone(), configuration);
        Ok((binding, conversation_attachment))
    }

    fn attachment_confirmed_callback(
        &self,
        binding: &Arc<MediaAudioSessionBindingGuard>,
    ) -> Arc<dyn Fn() + Send + Sync> {
        let routes = Arc::downgrade(&self.attachment_routes);
        let orphaned_routes = Arc::downgrade(&self.orphaned_attachment_routes);
        // Strongly retain the foreign callback in the drain receipt itself.
        // The receipt is held by the engine lifecycle through late Confirmed;
        // this avoids losing the Swift cleanup callback if the app composition
        // is released while an Unknown drain is still running. The callback
        // does not point back to this host, so this does not form a cycle.
        let media_devices = self.media_devices.clone();
        let key = binding.identity();
        let binding_ptr = Arc::as_ptr(binding) as usize;
        let attachment = binding.attachment.clone();
        let registry = binding.registry.clone();
        Arc::new(move || {
            if attachment.phase() != MediaAttachmentPhase::Revoked {
                return;
            }
            registry.unregister_attachment(attachment.id());
            let (host_id, generation) = key.clone();
            let callback = media_devices.clone();
            if let Err(error) = harness_runtime::mobile::spawn_mobile_lifecycle_task(async move {
                callback.attachment_drained(host_id, generation).await;
            }) {
                tracing::error!(%error, "could not schedule confirmed iOS attachment callback");
                return;
            }
            if let Some(routes) = routes.upgrade() {
                let mut routes = routes.lock().unwrap_or_else(|error| error.into_inner());
                if routes
                    .get(&key)
                    .is_some_and(|current| Arc::as_ptr(current) as usize == binding_ptr)
                {
                    routes.remove(&key);
                }
            }
            if let Some(orphaned_routes) = orphaned_routes.upgrade() {
                orphaned_routes
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .remove(&binding_ptr);
            }
        })
    }

    fn revoke_unbuilt_attachment(&self, binding: Arc<MediaAudioSessionBindingGuard>) {
        binding.attachment.begin_revoke();
        let attachment = binding.attachment.clone();
        let on_confirmed = self.attachment_confirmed_callback(&binding);
        if let Err(error) = harness_runtime::mobile::spawn_mobile_lifecycle_task(async move {
            match attachment.revoke(std::time::Duration::from_secs(5)).await {
                MediaAttachmentRevocation::Confirmed => on_confirmed(),
                MediaAttachmentRevocation::Unknown { reason }
                    if attachment.phase() == MediaAttachmentPhase::Revoking =>
                {
                    tracing::warn!(%reason, "unbuilt iOS media drain remains in flight");
                    if attachment.wait_for_drain_completion().await == MediaAttachmentPhase::Revoked
                    {
                        on_confirmed();
                    }
                }
                MediaAttachmentRevocation::Unknown { reason } => {
                    tracing::error!(%reason, "unbuilt iOS media attachment remains unknown");
                }
            }
        }) {
            // The strong attachment route remains in `attachment_routes`; the
            // failure is visible and no caller may assume cleanup completed.
            tracing::error!(%error, "could not schedule unbuilt iOS media attachment drain");
        }
    }

    fn track_engine_route(
        &self,
        engine: &Arc<MobileEngineHandle>,
        guard: &Arc<MediaAudioSessionBindingGuard>,
    ) {
        let mut guards = self
            .route_guards
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        guards.retain(|_, weak| weak.strong_count() > 0);
        guards.insert(Arc::as_ptr(engine) as usize, Arc::downgrade(guard));
    }

    pub(crate) fn audio_service(&self) -> Arc<dyn AudioService> {
        self.audio.clone()
    }

    pub(crate) fn camera_control(&self) -> Arc<dyn CameraControl> {
        self.camera.clone()
    }

    pub(crate) fn media_service(&self) -> Arc<dyn media_api::MediaService> {
        self.inner.media_service()
    }

    pub(crate) fn device_ports(&self) -> Arc<dyn MediaDevicePorts> {
        self.inner.device_ports()
    }

    pub(crate) fn resource_store(&self) -> Arc<dyn MediaResourceStore> {
        self.inner.resource_store()
    }

    pub(crate) fn cloud_audio_registry(&self) -> Arc<AudioProviderCloudRegistry> {
        self.inner.cloud_audio_registry()
    }

    pub(crate) async fn attach_incoming_session(
        &self,
        context: media_api::MediaOperationContext,
        session: Box<dyn media_api::MediaSession>,
    ) -> Result<media_api::MediaSessionRef, media_api::MediaError> {
        self.inner.attach_incoming_session(context, session).await
    }
}

/// Top-level `UniFFI` constructor: build the mobile engine from the Swift-supplied
/// platform callbacks + event listener. (Under `uniffi`: `#[uniffi::export]`.)
///
/// This is a THIN wrapper: it constructs the iOS-specific `Platform` from the
/// foreign callbacks and then delegates ALL runtime/adapter/listener wiring to
/// the shared [`harness_runtime::mobile::build_mobile_engine`] (F3-04) — so the heavy
/// lifting lives in exactly one place. The returned [`MobileEngineHandle`] owns
/// the tokio runtime + the wired orchestrator + the registered listener.
///
/// On non-iOS hosts this returns [`MobileEngineError::PlatformUnavailable`] —
/// the `IosPlatform` is only linked under `cfg(target_os = "ios")` — so the
/// crate still compiles and the SHARED host is exercised off-device through the
/// test shim (which calls `build_mobile_engine` with a portable fake `Platform`).
#[cfg(feature = "uniffi")]
pub fn build_mobile_engine(
    impls: PlatformImpls,
    listener: Arc<dyn ClientEventListener>,
    permission_sink: Arc<dyn PermissionRequestSink>,
) -> Result<Arc<MobileEngineHandle>, MobileEngineError> {
    #[cfg(target_os = "ios")]
    {
        use platform_ios::{IosPlatform, IosPlatformInputs};
        let linux = impls.mobile_linux.as_ref().ok_or_else(|| {
            MobileEngineError::Internal("iOS requires its Linux runtime user storage".into())
        })?;
        if !matches!(linux.mode, MobileLinuxRuntimeModeFfi::MobileLinux) {
            return Err(MobileEngineError::Internal(
                "iOS conversations require Mobile Linux".into(),
            ));
        }
        let directories =
            harness_runtime::mobile::mobile_user_directories(linux.managed_root.clone())?;
        let brand_profile = mobile_brand_profile_from_directories(
            &directories,
            &linux.brand_id,
            &linux.linux_user,
            &linux.config_dir_name,
        )?;
        directories.prepare()?;
        directories
            .validate_projects_mount(&linux.workspace_host_path, &linux.stable_workspace_id)?;
        let (cwd, logical_cwd) = directories.resolve_cwd(None)?;
        let cfg = MobileConfig {
            brand_runtime_profile: brand_profile.clone(),
            build_info: harness_runtime::mobile::BuildInfo::new(
                env!("CARGO_PKG_VERSION"),
                option_env!("LINGXI_GIT_SHA_SHORT").unwrap_or("unknown"),
            ),
            cwd,
            logical_cwd: Some(logical_cwd),
            lingxi_home: std::path::PathBuf::from(&directories.config_home),
            host_environment: Some(lingxi_core::host::MobileHostEnvironment::new(
                lingxi_core::host::MobileHostOs::Ios,
                None,
                lingxi_core::host::MobileDeviceClass::Unknown,
                lingxi_core::host::MobileExecutionTarget::Unknown,
                lingxi_core::host::MobileLaunchMode::Unknown,
            )),
            // P0.2: production injects the real LINGXI.md hierarchy provider so the
            // orchestrator loads `<cwd>/LINGXI.md` + `<lingxi_home>/LINGXI.md` into
            // its system prompt and `fire_instructions_loaded()` fires over them.
            memory_provider: Some(orchestrator::prompt::real_provider()),
            // The native WebView host renders inline widgets.
            inline_visualization: true,
            // The Apps page and the card under the tool call open local apps.
            local_apps: true,
            ..MobileConfig::default()
        };
        let (workspace_host_path, stable_workspace_id) =
            validate_mobile_linux_workspace_config(&impls.app_sandbox_root, linux)
                .map_err(|error| MobileEngineError::Internal(error.to_string()))?;
        let platform: Arc<dyn Platform> = Arc::new(IosPlatform::new(IosPlatformInputs {
            brand_profile: brand_profile.clone(),
            app_sandbox_root: std::path::PathBuf::from(impls.app_sandbox_root),
            camera: Some(impls.camera),
            audio: Some(impls.audio),
            share: impls.share,
            notifications: None,
            clipboard: None,
            device_status: None,
            haptics: None,
            deep_link: None,
            calendar: None,
            contacts: None,
            secure_storage: impls.secure_storage,
            // This lower-level entry point takes no location impl.
            location: None,
            mobile_linux: ios_mobile_linux_runtime(impls.mobile_linux.as_ref()),
            workspace_host_path: Some(workspace_host_path),
            stable_workspace_id: Some(stable_workspace_id),
            additional_workspace_mounts: Vec::new(),
        }));
        harness_runtime::mobile::build_mobile_engine(cfg, platform, listener, permission_sink)
    }
    #[cfg(not(target_os = "ios"))]
    {
        let _ = (impls, listener, permission_sink);
        Err(MobileEngineError::PlatformUnavailable)
    }
}

/// Foreign-callable constructor for the iOS app (plan M10-P3a).
///
/// Builds a fully-wired [`MobileEngineHandle`] from the Swift-supplied event
/// listener, app-composed media host, other device callbacks, and runtime
/// config. The same opaque media host can be shared with Remote Control. The
/// handle owns its tokio runtime and streams every
/// [`client::protocol::events::ClientEvent`] to `listener.on_event(..)`; the app
/// drives turns via [`MobileEngineHandle::submit`].
///
/// - `api_base`  — Anthropic-compatible base URL (e.g. `https://api.anthropic.com`).
/// - `api_key`   — read by Swift from `ANTHROPIC_API_KEY` / an app setting at
///   runtime. Empty is valid (turns 401 at `run_turn`); never hardcoded here.
/// - `model`     — default model id for new turns.
/// - `app_sandbox_root` — the app container path the engine roots its filesystem
///   + `~/.claude`-equivalent under.
/// - `listener`  — the foreign [`IosEventListener`] the adapter feeds (bridged to
///   the shared [`ClientEventListener`]).
/// - `mobile_linux` — optional iOS mobile-linux config. When selected, the
///   phase-1 build wires a capability/status bridge and a blocked/unavailable
///   runtime stub; it does NOT link GPL runtime code.
///
/// On non-iOS hosts (and the iOS *simulator* IS `target_os = "ios"`, so it takes
/// the real path) this delegates to [`build_mobile_engine`]; off-device it
/// returns [`MobileEngineError::PlatformUnavailable`].
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn build_ios_cron_store(
    app_sandbox_root: String,
    managed_root: String,
    project_cwd: Option<String>,
) -> Result<Arc<MobileCronStoreHandle>, MobileEngineError> {
    use platform_posix_minimal::{PosixClock, PosixFileSystem};

    let directories = harness_runtime::mobile::mobile_user_directories(managed_root.clone())?;
    directories.prepare()?;
    let scheduled_cwd;
    let project_cwd = match project_cwd {
        Some(path) => Some(path),
        None => {
            scheduled_cwd = std::path::PathBuf::from(&directories.scheduled_cwd);
            std::fs::create_dir_all(&scheduled_cwd)
                .map_err(|e| MobileEngineError::Internal(e.to_string()))?;
            Some(scheduled_cwd.to_string_lossy().into_owned())
        }
    };
    let cwd = ios_project_cwd(&managed_root, project_cwd.as_deref())?;
    let app_root = std::path::Path::new(&app_sandbox_root)
        .canonicalize()
        .map_err(|error| {
            MobileEngineError::Internal(format!("iOS app sandbox root is unavailable: {error}"))
        })?;
    Ok(Arc::new(MobileCronStoreHandle::new(
        cwd,
        Arc::new(PosixFileSystem::new(app_root)),
        Arc::new(PosixClock::new()),
    )))
}

/// Select the capabilities before entering the construction closure, which
/// may create an engine and attach its App Agent factory. An App Agent never
/// inherits native media, even when a caller supplies a host.
#[cfg(feature = "uniffi")]
fn with_engine_media_host<Media, Output>(
    app_agent_host: bool,
    media_host: Option<Media>,
    build: impl FnOnce(Option<Media>) -> Result<Output, MobileEngineError>,
) -> Result<Output, MobileEngineError> {
    let media_host = if app_agent_host {
        None
    } else {
        Some(media_host.ok_or_else(|| {
            MobileEngineError::Internal("iOS conversations require a native media host".into())
        })?)
    };
    build(media_host)
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(default(location = None)))]
#[allow(clippy::too_many_arguments)] // FFI constructor: one flat arg per Swift callback.
pub fn build_ios_engine_with_config(
    config: IosEngineLaunchConfigFfi,
    listener: Box<dyn IosEventListener>,
    media_host: Option<Arc<IosNativeMediaHost>>,
    share: Box<dyn IosShare>,
    notifications: Box<dyn IosNotification>,
    clipboard: Box<dyn IosClipboard>,
    permissions: Box<dyn IosPermissionSink>,
    secure_storage: Option<Box<dyn IosSecureStorage>>,
    device_control: Option<Box<dyn IosDeviceControl>>,
    // Defaulted so the two callers that have no location impl (the cron
    // bridge, the engine round-trip test) keep compiling untouched; only the
    // conversation host passes one.
    location: Option<Box<dyn IosLocation>>,
) -> Result<Arc<MobileEngineHandle>, MobileEngineError> {
    with_engine_media_host(config.app_agent_host, media_host, |media_host| {
        #[cfg(target_os = "ios")]
        let route_scope = media_host
            .as_ref()
            .map(|host| host.create_engine_media_scope())
            .transpose()?;
        #[cfg(not(target_os = "ios"))]
        let route_scope: Option<(
            Arc<MediaAudioSessionBindingGuard>,
            Arc<dyn harness_runtime::mobile::MediaConversationAttachment>,
        )> = None;
        let listener_guard = route_scope.as_ref().map(|(guard, _)| guard.clone());
        let listener: Arc<dyn ClientEventListener> = Arc::new(IosListenerBridge {
            inner: listener,
            _media_route_guard: retain_media_route_guard(listener_guard.as_ref()),
        });
        #[cfg(target_os = "ios")]
        let device_control = device_control.map(|inner| Arc::new(IosDeviceControlBridge { inner }));
        #[cfg(not(target_os = "ios"))]
        let _ = device_control;
        #[cfg(target_os = "ios")]
        {
            use platform_ios::{IosPlatform, IosPlatformInputs};
            let audio = media_host.as_ref().map(|host| host.audio_service());
            let camera = media_host.as_ref().map(|host| host.camera_control());
            let device_status = device_control
                .clone()
                .map(|service| service.clone() as Arc<dyn lingxi_core::host::DeviceStatusProvider>);
            let haptics = device_control
                .clone()
                .map(|service| service.clone() as Arc<dyn lingxi_core::host::HapticService>);
            let calendar = device_control
                .clone()
                .map(|service| service.clone() as Arc<dyn lingxi_core::host::CalendarProvider>);
            let contacts = device_control
                .clone()
                .map(|service| service.clone() as Arc<dyn lingxi_core::host::ContactsProvider>);
            let deep_link =
                device_control.map(|service| service as Arc<dyn lingxi_core::host::DeepLinkOpener>);

            // Keep every fallible engine-construction step inside this result so a
            // failure after attachment admission always starts the host-owned drain.
            let build_result = (|| -> Result<Arc<MobileEngineHandle>, MobileEngineError> {
                let mut cfg = ios_mobile_config_from_launch_config(&config)?;
                if let (Some(host), Some((route_guard, conversation_attachment))) =
                    (media_host.as_ref(), route_scope.as_ref())
                {
                    cfg.media_service = Some(route_guard.attachment.service());
                    cfg.media_attachment_handle = Some(route_guard.attachment.clone());
                    cfg.media_attachment_confirmed_callback =
                        Some(host.attachment_confirmed_callback(route_guard));
                    cfg.media_conversation_attachment = Some(conversation_attachment.clone());
                }
                let additional_workspace_mounts = ios_project_mount_grants(&config)?;
                let (workspace_host_path, stable_workspace_id) = match config.mobile_linux.as_ref()
                {
                    Some(linux) => {
                        let (workspace, stable_id) =
                            validate_mobile_linux_workspace_config(&config.app_sandbox_root, linux)
                                .map_err(|error| MobileEngineError::Internal(error.to_string()))?;
                        (Some(workspace), Some(stable_id))
                    }
                    None if config.app_agent_host => (None, None),
                    None => {
                        return Err(MobileEngineError::Internal(
                            "iOS conversations require Mobile Linux".into(),
                        ));
                    }
                };
                let platform: Arc<dyn Platform> = Arc::new(IosPlatform::new(IosPlatformInputs {
                    brand_profile: cfg.brand_runtime_profile.clone(),
                    app_sandbox_root: std::path::PathBuf::from(&config.app_sandbox_root),
                    camera,
                    audio,
                    share: Arc::new(IosShareBridge { inner: share }),
                    notifications: Some(Arc::new(IosNotificationBridge {
                        inner: notifications,
                    })),
                    clipboard: Some(Arc::new(IosClipboardBridge { inner: clipboard })),
                    device_status,
                    haptics,
                    deep_link,
                    calendar,
                    contacts,
                    secure_storage: secure_storage.map(|s| {
                        Arc::new(IosSecureStorageBridge { inner: s })
                            as Arc<dyn lingxi_core::host::SecureStorage>
                    }),
                    location: location.map(|l| {
                        Arc::new(IosLocationBridge { inner: l })
                            as Arc<dyn lingxi_core::host::LocationProvider>
                    }),
                    mobile_linux: ios_mobile_linux_runtime_with_project_mounts(
                        config.mobile_linux.as_ref(),
                        additional_workspace_mounts.clone(),
                    ),
                    workspace_host_path,
                    stable_workspace_id,
                    additional_workspace_mounts,
                }));
                let permission_sink: Arc<dyn PermissionRequestSink> =
                    Arc::new(IosPermissionSinkBridge { inner: permissions });
                let engine = harness_runtime::mobile::build_mobile_engine(
                    cfg,
                    platform,
                    listener,
                    permission_sink,
                )?;
                if let (Some(host), Some((route_guard, _))) =
                    (media_host.as_ref(), route_scope.as_ref())
                {
                    host.track_engine_route(&engine, route_guard);
                    // This UniFFI constructor remains synchronous for its Swift callers.
                    // It runs outside the engine runtime, so use the engine-owned runtime
                    // for the initial trusted provider/session binding.
                    engine
                        .runtime()
                        .block_on(host.update_audio_session_context(engine.clone()))?;
                }
                Ok(engine)
            })();
            match build_result {
                Ok(engine) => Ok(engine),
                Err(error) => {
                    if let (Some(host), Some((route_guard, _))) =
                        (media_host.as_ref(), route_scope.as_ref())
                    {
                        host.revoke_unbuilt_attachment(route_guard.clone());
                    }
                    Err(error)
                }
            }
        }
        #[cfg(not(target_os = "ios"))]
        {
            let _ = (
                config,
                listener,
                media_host,
                route_scope,
                listener_guard,
                share,
                notifications,
                clipboard,
                permissions,
                secure_storage,
                location,
            );
            Err(MobileEngineError::PlatformUnavailable)
        }
    })
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
#[allow(clippy::too_many_arguments)] // FFI constructor: one flat arg per Swift callback.
pub fn build_ios_engine(
    api_base: String,
    api_key: String,
    model: String,
    app_sandbox_root: String,
    listener: Box<dyn IosEventListener>,
    media_host: Arc<IosNativeMediaHost>,
    share: Box<dyn IosShare>,
    notifications: Box<dyn IosNotification>,
    clipboard: Box<dyn IosClipboard>,
    permissions: Box<dyn IosPermissionSink>,
    mobile_linux: Option<IosMobileLinuxConfigFfi>,
    secure_storage: Option<Box<dyn IosSecureStorage>>,
    device_control: Option<Box<dyn IosDeviceControl>>,
) -> Result<Arc<MobileEngineHandle>, MobileEngineError> {
    let (brand_id, linux_user, config_dir_name, native_config_home) =
        if let Some(linux) = mobile_linux.as_ref() {
            let profile = mobile_linux_api::BrandRuntimeProfile::new(
                linux.brand_id.clone(),
                linux.linux_user.clone(),
                linux.config_dir_name.clone(),
            );
            profile
                .validate()
                .map_err(|error| MobileEngineError::Internal(error.to_string()))?;
            let home = profile
                .persistent_home_host_path(std::path::Path::new(&linux.managed_root))
                .join(&profile.config_dir_name);
            (
                profile.brand_id.clone(),
                profile.linux_user.clone(),
                profile.config_dir_name.clone(),
                home.to_string_lossy().into_owned(),
            )
        } else {
            let profile = mobile_linux_api::BrandRuntimeProfile::default();
            let home = profile
                .persistent_home_host_path(std::path::Path::new(&app_sandbox_root))
                .join(&profile.config_dir_name);
            (
                profile.brand_id,
                profile.linux_user,
                profile.config_dir_name,
                home.to_string_lossy().into_owned(),
            )
        };
    build_ios_engine_with_config(
        IosEngineLaunchConfigFfi {
            app_agent_host: false,
            app_version: None,
            brand_id,
            linux_user,
            config_dir_name,
            native_config_home,
            api_base,
            api_key,
            model,
            session_mode: SessionModeDto::Code,
            app_sandbox_root,
            project_cwd: None,
            project_additional_directories: Vec::new(),
            project_mount_grants: Vec::new(),
            provider_config: None,
            mobile_linux,
            vision_delegation_enabled: true,
            physical_memory_bytes: 0,
            host_environment: None,
        },
        listener,
        Some(media_host),
        share,
        notifications,
        clipboard,
        permissions,
        secure_storage,
        device_control,
        None,
    )
}

#[cfg(all(test, feature = "uniffi"))]
mod media_host_policy_tests {
    use super::{retain_media_route_guard, with_engine_media_host, MediaAudioSessionBindingGuard};
    use audio_provider::media_cloud::AudioProviderCloudRegistry;
    use harness_runtime::mobile::{MediaAttachmentHost, MobileEngineError};
    use std::cell::Cell;
    use std::sync::Arc;

    struct EmptyAttachmentMediaService;

    #[async_trait::async_trait]
    impl media_api::MediaService for EmptyAttachmentMediaService {
        fn capabilities(&self) -> media_api::MediaCapabilities {
            media_api::MediaCapabilities {
                service_epoch: 1,
                support_revision: 1,
                supported_operations: Vec::new(),
                supported_media_kinds: Vec::new(),
                readiness: Vec::new(),
                max_payload_bytes: 1024,
            }
        }

        async fn execute(
            &self,
            _context: media_api::MediaOperationContext,
            _operation: media_api::MediaOperation,
        ) -> Result<media_api::MediaResult, media_api::MediaError> {
            Err(media_api::MediaError::new(
                media_api::MediaErrorKind::Unsupported,
                "empty attachment fixture",
            ))
        }

        async fn read_media(
            &self,
            _context: media_api::MediaOperationContext,
            _media: media_api::MediaRef,
        ) -> Result<media_api::MediaContent, media_api::MediaError> {
            Err(media_api::MediaError::new(
                media_api::MediaErrorKind::NotFound,
                "empty attachment fixture",
            ))
        }

        async fn cancel(
            &self,
            _context: media_api::MediaOperationContext,
        ) -> Result<(), media_api::MediaError> {
            Ok(())
        }
    }

    #[test]
    fn listener_guard_retains_the_route_until_listener_release() {
        assert!(retain_media_route_guard(None).is_none());

        let registry = Arc::new(AudioProviderCloudRegistry::new("wav"));
        let attachment_host = MediaAttachmentHost::new();
        let attachment = attachment_host
            .attach(Arc::new(EmptyAttachmentMediaService))
            .unwrap();
        let guard = Arc::new(MediaAudioSessionBindingGuard::new(registry, attachment));
        let weak = Arc::downgrade(&guard);
        let retained = retain_media_route_guard(Some(&guard)).unwrap();
        assert!(retained.is::<MediaAudioSessionBindingGuard>());

        drop(guard);
        assert!(weak.upgrade().is_some());
        drop(retained);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn app_agent_builds_without_media() {
        let builds = Cell::new(0);
        let selected = with_engine_media_host(true, None::<Arc<()>>, |host| {
            builds.set(builds.get() + 1);
            Ok(host)
        })
        .unwrap();
        assert!(selected.is_none());
        assert_eq!(builds.get(), 1);
    }

    #[test]
    fn app_agent_discards_supplied_media_before_building() {
        let host = Arc::new(());
        let registrations = Cell::new(0);
        with_engine_media_host(true, Some(host.clone()), |selected| {
            if selected.is_some() {
                registrations.set(registrations.get() + 1);
            }
            assert!(selected.is_none());
            Ok(())
        })
        .unwrap();
        assert_eq!(registrations.get(), 0);
        assert_eq!(Arc::strong_count(&host), 1);
    }

    #[test]
    fn conversation_without_media_fails_before_factory_side_effects() {
        let factory_calls = Cell::new(0);
        let result = with_engine_media_host(false, None::<Arc<()>>, |_| {
            factory_calls.set(factory_calls.get() + 1);
            Ok(())
        });
        assert!(matches!(result, Err(MobileEngineError::Internal(message))
            if message == "iOS conversations require a native media host"));
        assert_eq!(factory_calls.get(), 0);
    }

    #[test]
    fn conversation_with_media_retains_the_supplied_host() {
        let host = Arc::new(());
        let registrations = Cell::new(0);
        with_engine_media_host(false, Some(host.clone()), |selected| {
            assert!(Arc::ptr_eq(selected.as_ref().unwrap(), &host));
            registrations.set(registrations.get() + 1);
            Ok(())
        })
        .unwrap();
        assert_eq!(registrations.get(), 1);
    }
}

#[cfg(all(test, feature = "uniffi"))]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn clearing_an_audio_binding_preserves_a_newer_engine_owner() {
        let registry = Arc::new(AudioProviderCloudRegistry::new("audio/wav"));
        let provider =
            Arc::new(AudioProviderHost::new("{}", "international", None, BTreeMap::new()).unwrap());
        let current = std::sync::Mutex::new(None);
        let first = registry
            .register_session("session", "first", "profile:first", provider.clone())
            .unwrap();
        replace_audio_session_binding(
            &registry,
            &current,
            MediaAudioSessionBindingFfi {
                attachment_host_id: "00000000-0000-4000-8000-000000000001".into(),
                attachment_generation: 1,
                session_id: "session".into(),
                generation: first,
            },
        );
        clear_audio_session_binding(&registry, &current);
        assert_eq!(registry.session_identity("session"), None);

        let old = registry
            .register_session("session", "old", "profile:old", provider.clone())
            .unwrap();
        replace_audio_session_binding(
            &registry,
            &current,
            MediaAudioSessionBindingFfi {
                attachment_host_id: "00000000-0000-4000-8000-000000000001".into(),
                attachment_generation: 1,
                session_id: "session".into(),
                generation: old,
            },
        );
        registry
            .register_session("session", "new", "profile:new", provider)
            .unwrap();
        clear_audio_session_binding(&registry, &current);
        assert_eq!(
            registry.session_identity("session"),
            Some(("new".into(), "profile:new".into()))
        );
    }

    #[test]
    fn same_session_routes_are_isolated_by_engine_attachment_registry() {
        let first_registry = AudioProviderCloudRegistry::new("audio/wav");
        let second_registry = AudioProviderCloudRegistry::new("audio/wav");
        let provider =
            Arc::new(AudioProviderHost::new("{}", "international", None, BTreeMap::new()).unwrap());
        let first_generation = first_registry
            .register_session("session", "first", "account:first", provider.clone())
            .unwrap();
        let second_generation = second_registry
            .register_session("session", "second", "account:second", provider)
            .unwrap();

        first_registry.unregister_session_if_generation("session", first_generation);

        assert_eq!(first_registry.session_identity("session"), None);
        assert_eq!(
            second_registry.session_identity("session"),
            Some(("second".into(), "account:second".into()))
        );
        assert_eq!(first_generation, second_generation);
    }
}

#[cfg(all(test, feature = "uniffi"))]
mod mobile_media_scope_tests {
    use super::super::callbacks::{CameraFfiError, CapturedImageFfi, IosAudioFfiError};
    use super::*;
    use client::protocol::{audio as audio_dto, media as media_dto};
    use media_api::{
        MediaOperation, MediaOperationContext, MediaOperationId, MediaOperationKind, MediaOwner,
        MediaResult,
    };
    use std::collections::BTreeMap;
    use std::time::Duration;

    struct Audio;
    #[async_trait::async_trait]
    impl IosAudioService for Audio {
        fn capabilities(&self) -> audio_dto::AudioCapabilitySnapshotDto {
            audio_dto::AudioCapabilitySnapshotDto {
                service_epoch: 1,
                support_revision: 1,
                supported_operations: Vec::new(),
                readiness: Vec::new(),
                max_payload_bytes: 1024,
            }
        }
        async fn execute(
            &self,
            request: audio_dto::AudioOperationRequestDto,
        ) -> audio_dto::AudioOperationResultDto {
            match request.operation {
                audio_dto::AudioOperationDto::Status { handle: None } => {
                    audio_dto::AudioOperationResultDto::Status {
                        status: audio_dto::AudioStatusDto {
                            recording: false,
                            playing: false,
                        },
                    }
                }
                _ => panic!("unexpected audio operation in idle media fixture"),
            }
        }
        async fn cancel(&self, _: audio_dto::AudioOperationIdDto) -> Result<(), IosAudioFfiError> {
            Ok(())
        }
    }
    struct Camera;
    #[async_trait::async_trait]
    impl IosCamera for Camera {
        async fn capture_photo(
            &self,
            _: bool,
            _: bool,
        ) -> Result<CapturedImageFfi, CameraFfiError> {
            Err(CameraFfiError::DeviceUnavailable)
        }
        async fn pick_from_library(&self) -> Result<CapturedImageFfi, CameraFfiError> {
            Err(CameraFfiError::DeviceUnavailable)
        }
        async fn capture_photo_sized(
            &self,
            _: bool,
            _: bool,
            _: u32,
            _: f32,
        ) -> Result<CapturedImageFfi, CameraFfiError> {
            Err(CameraFfiError::DeviceUnavailable)
        }
        async fn pick_from_library_sized(
            &self,
            _: u32,
            _: f32,
        ) -> Result<CapturedImageFfi, CameraFfiError> {
            Err(CameraFfiError::DeviceUnavailable)
        }
    }
    struct Devices;
    #[async_trait::async_trait]
    impl IosMediaDevicePorts for Devices {
        fn supported_session_modes(&self) -> Vec<media_dto::MediaSessionModeDto> {
            Vec::new()
        }
        async fn request(
            &self,
            _: media_dto::MediaDeviceRequestEnvelopeDto,
        ) -> media_dto::MediaDeviceResponseDto {
            panic!("this fixture does not open devices")
        }
        async fn attachment_drained(&self, _: String, _: u64) {}
    }

    #[tokio::test]
    async fn native_host_engine_scopes_share_service_without_sharing_owner_routes() {
        let config = r#"{"schemaVersion":4,"recognition":{"source":"automatic","cloud":{"binding":"follow_session"}},"speech":{"source":"automatic","cloud":{"binding":"follow_session"}},"language":"auto","rate":1.0}"#;
        let host = IosNativeMediaHost::new(
            Box::new(Audio),
            Box::new(Camera),
            Box::new(Devices),
            config.into(),
        )
        .unwrap();
        let (first_binding, _) = host.create_engine_media_scope().unwrap();
        let (second_binding, _) = host.create_engine_media_scope().unwrap();
        let first = &first_binding.attachment;
        let second = &second_binding.attachment;
        assert_ne!(first.id(), second.id());
        assert!(Arc::ptr_eq(
            &first_binding.registry,
            &second_binding.registry
        ));
        assert!(Arc::ptr_eq(
            &first_binding.registry,
            &host.inner.cloud_audio_registry()
        ));
        let owner = MediaOwner::Session {
            session_id: "same-session".into(),
        };
        for (generation, attachment) in [(1, first), (2, second)] {
            // Harness mobile assembly binds the platform audio service to the
            // same attachment, even if the engine never uses direct audio.
            let _audio = attachment.audio_service(host.audio_service()).unwrap();
            let service = attachment.service();
            let capabilities = service.capabilities_for(&owner);
            assert!(capabilities.supports(MediaOperationKind::Sources));
            let context = MediaOperationContext::new(
                MediaOperationId::new(generation, capabilities.service_epoch),
                owner.clone(),
            );
            assert!(matches!(
                service
                    .execute(
                        context,
                        MediaOperation::Sources {
                            kind: None,
                            usage: None
                        }
                    )
                    .await,
                Ok(MediaResult::Sources { .. })
            ));
        }
        let provider = || {
            Arc::new(AudioProviderHost::new("{}", "international", None, BTreeMap::new()).unwrap())
        };
        let first_provider = provider();
        let second_provider = provider();
        let registry = first_binding.registry.clone();
        for (attachment, provider) in [(first, &first_provider), (second, &second_provider)] {
            let permit = attachment.authorize_owner(&owner).unwrap();
            registry
                .register_attached_profile(&permit, "openai", provider.clone())
                .unwrap();
            registry
                .bind_attached_session_to_profile(&permit, "openai", "profile:openai")
                .unwrap();
        }
        let callback = host.attachment_confirmed_callback(&first_binding);
        callback();
        assert!(registry
            .provider_for_attached_session(first.id(), "same-session", "openai", "profile:openai")
            .is_ok());
        let (idle, _) = host.create_engine_media_scope().unwrap();
        let _idle_audio = idle.attachment.audio_service(host.audio_service()).unwrap();
        let idle_key = idle.identity();
        let idle_callback = host.attachment_confirmed_callback(&idle);
        assert_eq!(
            idle.attachment.revoke(Duration::from_secs(2)).await,
            MediaAttachmentRevocation::Confirmed
        );
        idle_callback();
        assert!(!host
            .attachment_routes
            .lock()
            .unwrap()
            .contains_key(&idle_key));
        first.begin_revoke();
        callback();
        assert!(registry
            .provider_for_attached_session(first.id(), "same-session", "openai", "profile:openai")
            .is_ok());
        assert_eq!(
            first.revoke(Duration::from_secs(2)).await,
            MediaAttachmentRevocation::Confirmed
        );
        callback();
        assert!(registry
            .provider_for_attached_session(first.id(), "same-session", "openai", "profile:openai")
            .is_err());
        assert!(Arc::ptr_eq(
            &registry
                .provider_for_attached_session(
                    second.id(),
                    "same-session",
                    "openai",
                    "profile:openai"
                )
                .unwrap(),
            &second_provider
        ));
        assert_eq!(
            second.revoke(Duration::from_secs(2)).await,
            MediaAttachmentRevocation::Confirmed
        );
        host.attachment_confirmed_callback(&second_binding)();
    }
}

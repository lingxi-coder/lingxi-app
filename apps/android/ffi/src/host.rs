#[cfg(all(feature = "uniffi", target_os = "android"))]
use super::callbacks::AndroidSecureStorageBridge;
#[cfg(feature = "uniffi")]
use super::callbacks::{
    AndroidAudioService, AndroidAudioServiceBridge, AndroidCamera, AndroidCameraBridge,
    AndroidClipboard, AndroidComputerUseHost, AndroidDeviceControl, AndroidEventListener,
    AndroidGitCredentialProvider, AndroidListenerBridge, AndroidLocation, AndroidNotification,
    AndroidPermissionSink, AndroidSecureStorage, AndroidShare,
};
#[cfg(all(feature = "uniffi", target_os = "android"))]
use super::callbacks::{
    AndroidClipboardBridge, AndroidComputerUseBridge, AndroidDeviceControlBridge,
    AndroidGitCredentialProviderBridge, AndroidLocationBridge, AndroidNotificationBridge,
    AndroidPermissionSinkBridge, AndroidShareBridge,
};
#[cfg(feature = "uniffi")]
use super::callbacks::{AndroidMediaDevicePorts, AndroidMediaDevicePortsBridge};
use super::configuration::AndroidEngineLaunchConfigFfi;
#[cfg(feature = "uniffi")]
use super::configuration::{
    android_project_cwd, AndroidGitConfigFfi, AndroidMobileLinuxConfigFfi, AndroidShellConfigFfi,
};
#[cfg(all(feature = "uniffi", target_os = "android"))]
use super::linux_runtime::prepared_android_mobile_linux_runtime;
#[cfg(feature = "uniffi")]
use client::protocol::text::ClientTextDto;
#[cfg(feature = "uniffi")]
use harness_runtime::mobile::{
    ClientEventListener, MediaAttachmentHandle, MediaAttachmentHost, MediaAttachmentRevocation,
    MobileCronStoreHandle, MobileEngineError, MobileEngineHandle, PermissionRequestSink,
};
#[cfg(all(feature = "uniffi", target_os = "android"))]
use harness_runtime::mobile::{MobileConfig, MobileSessionMode, SessionModeDto};
#[cfg(all(feature = "uniffi", target_os = "android"))]
use lingxi_core::host::Platform;
use lingxi_core::host::{AudioService, CameraControl, SharingService};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock, Weak};

#[cfg(feature = "uniffi")]
use audio_provider::{
    media_cloud::AudioProviderCloudRegistry,
    media_configuration::audio_routing_settings_from_v4_json_checked, media_host::AppMediaHost,
    media_native::CallbackMediaDevicePorts, media_store::BoundedMediaResourceStore,
    AppMediaConversationAttachment, AudioConfigurationSnapshot, AudioProviderHost,
};
#[cfg(feature = "uniffi")]
use media_api::{MediaDevicePorts, MediaResourceStore, MediaSessionMode};

/// Exact provider-route slot installed by one engine attachment. The token is
/// safe to retain and pass back during teardown; stale cleanup cannot remove a
/// later engine's replacement binding for the same session ID.
#[cfg(feature = "uniffi")]
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MediaAudioSessionBindingFfi {
    pub attachment_host_id: String,
    pub attachment_generation: u64,
    pub session_id: String,
    pub generation: u64,
}

/// Opaque app-composed Android media host shared with the Agent runtime and
/// Remote Control. It wraps exactly one app-scoped service, native lease
/// coordinator, and owner-scoped resource store.
#[cfg(feature = "uniffi")]
#[derive(uniffi::Object)]
pub struct AndroidNativeMediaHost {
    inner: Arc<AppMediaHost>,
    audio: Arc<dyn AudioService>,
    camera: Arc<dyn CameraControl>,
    media_audio_configuration_json: Arc<RwLock<String>>,
    media_attachment_host: MediaAttachmentHost,
    /// Strong app-host retention keeps provider routes alive while an engine
    /// attachment is Revoking or Unknown. Confirmed drains remove one exact key.
    attachment_routes: Arc<Mutex<HashMap<(String, u64), Arc<AndroidEngineMediaBinding>>>>,
    engine_bindings: Arc<Mutex<HashMap<usize, Weak<AndroidEngineMediaBinding>>>>,
}

/// Per-engine owner of the exact provider/session registry entry. The listener
/// retained by `MobileEngineHandle` holds this guard, so registry cleanup follows
/// the native engine lifetime rather than a best-effort Kotlin callback.
#[cfg(feature = "uniffi")]
pub(super) struct AndroidEngineMediaBinding {
    registry: Arc<AudioProviderCloudRegistry>,
    attachment: MediaAttachmentHandle,
    engine_bindings: Weak<Mutex<HashMap<usize, Weak<AndroidEngineMediaBinding>>>>,
    engine_key: AtomicUsize,
    update_gate: tokio::sync::Mutex<()>,
    current: Mutex<Option<MediaAudioSessionBindingFfi>>,
}

#[cfg(feature = "uniffi")]
impl AndroidEngineMediaBinding {
    fn new(
        registry: Arc<AudioProviderCloudRegistry>,
        attachment: MediaAttachmentHandle,
        engine_bindings: Weak<Mutex<HashMap<usize, Weak<AndroidEngineMediaBinding>>>>,
    ) -> Self {
        Self {
            registry,
            attachment,
            engine_bindings,
            engine_key: AtomicUsize::new(0),
            update_gate: tokio::sync::Mutex::new(()),
            current: Mutex::new(None),
        }
    }

    fn matches_attachment(&self, binding: &MediaAudioSessionBindingFfi) -> bool {
        binding.attachment_host_id == self.attachment.id().host_id()
            && binding.attachment_generation == self.attachment.id().generation()
    }

    async fn bind_current_context(
        &self,
        engine: &MobileEngineHandle,
    ) -> Result<MediaAudioSessionBindingFfi, MobileEngineError> {
        let _update_guard = self.update_gate.lock().await;
        let context = engine.current_media_context().await?;
        let owner = media_api::MediaOwner::Session {
            session_id: context.session_id.clone(),
        };
        // Keep route registration under the attachment admission fence. If
        // teardown has started, a stale engine cannot reinstall its provider.
        let permit = self
            .attachment
            .authorize_owner(&owner)
            .map_err(|error| MobileEngineError::Internal(error.to_string()))?;
        let (services, credential_ids, _) = engine.audio_provider_services();
        let provider = Arc::new(AudioProviderHost::from_provider_services(
            services,
            credential_ids.clone(),
        ));
        for profile_name in credential_ids.keys() {
            self.registry
                .register_attached_profile(&permit, profile_name.clone(), provider.clone())
                .map_err(|error| MobileEngineError::Internal(error.to_string()))?;
        }

        let mut current = self
            .current
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let generation = self
            .registry
            .register_attached_session(&permit, context.profile_id, context.account_scope, provider)
            .map_err(|error| MobileEngineError::Internal(error.to_string()))?;
        let next = MediaAudioSessionBindingFfi {
            attachment_host_id: self.attachment.id().host_id(),
            attachment_generation: self.attachment.id().generation(),
            session_id: context.session_id,
            generation,
        };
        if let Some(previous) = current.replace(next.clone()) {
            self.registry.unregister_attached_session_if_generation(
                self.attachment.id(),
                &previous.session_id,
                previous.generation,
            );
        }
        Ok(next)
    }

    fn remove_session_context(&self, binding: &MediaAudioSessionBindingFfi) -> bool {
        if !self.matches_attachment(binding) {
            return false;
        }
        let owner = media_api::MediaOwner::Session {
            session_id: binding.session_id.clone(),
        };
        // While revocation is in flight (or Unknown), retain the exact route.
        // A confirmed drain's host callback removes this attachment's routes.
        let Ok(_permit) = self.attachment.authorize_owner(&owner) else {
            return false;
        };
        let mut current = self
            .current
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
}

#[cfg(feature = "uniffi")]
impl Drop for AndroidEngineMediaBinding {
    fn drop(&mut self) {
        match self.attachment.phase() {
            harness_runtime::mobile::MediaAttachmentPhase::Revoked => {
                self.registry.unregister_attachment(self.attachment.id());
                self.current
                    .get_mut()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
            }
            harness_runtime::mobile::MediaAttachmentPhase::Active => {
                let current = self
                    .current
                    .get_mut()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
                if let Some(binding) = current {
                    self.registry.unregister_attached_session_if_generation(
                        self.attachment.id(),
                        &binding.session_id,
                        binding.generation,
                    );
                }
            }
            harness_runtime::mobile::MediaAttachmentPhase::Revoking
            | harness_runtime::mobile::MediaAttachmentPhase::Unknown => {
                // Keep provider routes host-owned while drain completion is not confirmed.
            }
        }
        let key = self.engine_key.load(Ordering::Acquire);
        if key != 0 {
            if let Some(bindings) = self.engine_bindings.upgrade() {
                let mut bindings = bindings
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if bindings
                    .get(&key)
                    .is_some_and(|registered| std::ptr::eq(registered.as_ptr(), self))
                {
                    bindings.remove(&key);
                }
            }
        }
    }
}

#[cfg(feature = "uniffi")]
#[uniffi::export]
impl AndroidNativeMediaHost {
    /// Construct the app-scoped media host from native platform callbacks.
    #[uniffi::constructor]
    pub fn new(
        audio: Box<dyn AndroidAudioService>,
        camera: Box<dyn AndroidCamera>,
        media_devices: Box<dyn AndroidMediaDevicePorts>,
        media_audio_configuration_json: String,
    ) -> Result<Arc<Self>, MobileEngineError> {
        let native_audio: Arc<dyn harness_runtime::mobile::NativeAudioService> =
            Arc::new(AndroidAudioServiceBridge { inner: audio });
        let audio = harness_runtime::mobile::from_native_audio_service(native_audio);
        let camera: Arc<dyn CameraControl> = Arc::new(AndroidCameraBridge { inner: camera });
        let local_modes = media_devices
            .device_capabilities()
            .session_modes
            .into_iter()
            .map(|mode| match mode {
                client::protocol::media::MediaSessionModeDto::Audio => MediaSessionMode::Audio,
                client::protocol::media::MediaSessionModeDto::Video => MediaSessionMode::Video,
                client::protocol::media::MediaSessionModeDto::AudioVideo => {
                    MediaSessionMode::AudioVideo
                }
            })
            .collect();
        let callback = Arc::new(AndroidMediaDevicePortsBridge {
            inner: media_devices,
        });
        let ports: Arc<dyn MediaDevicePorts> = Arc::new(CallbackMediaDevicePorts::new(callback));
        let settings = audio_routing_settings_from_v4_json_checked(&media_audio_configuration_json)
            .map_err(MobileEngineError::Internal)?;
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
        inner
            .update_audio_routing_v4(&media_audio_configuration_json)
            .map_err(MobileEngineError::Internal)?;
        let media_audio_configuration_json = Arc::new(RwLock::new(media_audio_configuration_json));
        Ok(Arc::new(Self {
            inner,
            audio,
            camera,
            media_audio_configuration_json,
            media_attachment_host: MediaAttachmentHost::new(),
            attachment_routes: Arc::new(Mutex::new(HashMap::new())),
            engine_bindings: Arc::new(Mutex::new(HashMap::new())),
        }))
    }

    /// Replace saved v4 route settings without rebuilding the service or its
    /// active owner-scoped resources.
    pub fn update_audio_settings(
        &self,
        media_audio_configuration_json: String,
    ) -> Result<(), MobileEngineError> {
        self.inner
            .update_audio_routing_v4(&media_audio_configuration_json)
            .map_err(MobileEngineError::Internal)?;
        *self
            .media_audio_configuration_json
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = media_audio_configuration_json;
        Ok(())
    }

    /// Refresh the trusted provider/profile/account snapshot after the attached
    /// engine changes session or model. The selected credential host stays in
    /// Rust and is pinned by the next media operation.
    #[uniffi::method(async_runtime = "tokio")]
    pub async fn update_audio_session_context(
        &self,
        engine: Arc<MobileEngineHandle>,
    ) -> Result<MediaAudioSessionBindingFfi, MobileEngineError> {
        let binding = self.engine_binding(&engine).ok_or_else(|| {
            MobileEngineError::Internal(
                "Android engine has no app-owned media binding lifecycle".into(),
            )
        })?;
        binding.bind_current_context(&engine).await
    }

    /// Remove one exact engine attachment/session route. The attachment ID
    /// prevents a stale engine's cleanup from touching a route in another
    /// engine's attachment-scoped routes in the shared registry.
    pub fn remove_audio_session_context(&self, binding: MediaAudioSessionBindingFfi) {
        let key = (
            binding.attachment_host_id.clone(),
            binding.attachment_generation,
        );
        let route = self
            .attachment_routes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&key)
            .cloned();
        if let Some(route) = route {
            route.remove_session_context(&binding);
        }
    }
}

#[cfg(feature = "uniffi")]
impl AndroidNativeMediaHost {
    fn create_engine_media_scope(
        &self,
    ) -> Result<
        (
            MediaAttachmentHandle,
            Arc<dyn orchestrator::MediaConversationAttachment>,
            Arc<AndroidEngineMediaBinding>,
        ),
        MobileEngineError,
    > {
        let (attachment, registry) = self
            .inner
            .create_mobile_media_attachment(&self.media_attachment_host)
            .map_err(|error| MobileEngineError::Internal(error.to_string()))?;
        let binding = Arc::new(AndroidEngineMediaBinding::new(
            registry.clone(),
            attachment.clone(),
            Arc::downgrade(&self.engine_bindings),
        ));
        let key = (attachment.id().host_id(), attachment.id().generation());
        let inserted = {
            let mut routes = self
                .attachment_routes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match routes.entry(key) {
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(binding.clone());
                    true
                }
                std::collections::hash_map::Entry::Occupied(_) => false,
            }
        };
        if !inserted {
            return Err(MobileEngineError::Internal(
                "Android media attachment identity was already registered".into(),
            ));
        }

        let configuration = self.media_audio_configuration_json.clone();
        let configuration_snapshot: AudioConfigurationSnapshot = Arc::new(move || {
            Ok(configuration
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone())
        });
        let conversation_attachment = AppMediaConversationAttachment::new(
            registry,
            attachment.clone(),
            configuration_snapshot,
        );
        Ok((attachment, conversation_attachment, binding))
    }

    fn attachment_confirmed_callback(
        &self,
        attachment: &MediaAttachmentHandle,
    ) -> Arc<dyn Fn() + Send + Sync> {
        let routes = Arc::downgrade(&self.attachment_routes);
        let key = (attachment.id().host_id(), attachment.id().generation());
        let attachment = attachment.clone();
        let registry = self.inner.cloud_audio_registry();
        Arc::new(move || {
            if attachment.phase() != harness_runtime::mobile::MediaAttachmentPhase::Revoked {
                return;
            }
            registry.unregister_attachment(attachment.id());
            if let Some(routes) = routes.upgrade() {
                routes
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .remove(&key);
            }
        })
    }

    fn revoke_unbuilt_attachment(&self, binding: Arc<AndroidEngineMediaBinding>) {
        // A failed engine build must not leave an active, unowned attachment.
        binding.attachment.begin_revoke();
        let attachment = binding.attachment.clone();
        let on_confirmed = self.attachment_confirmed_callback(&attachment);
        if let Err(error) = harness_runtime::mobile::spawn_mobile_lifecycle_task(async move {
            match attachment.revoke(std::time::Duration::from_secs(5)).await {
                MediaAttachmentRevocation::Confirmed => on_confirmed(),
                MediaAttachmentRevocation::Unknown { reason }
                    if attachment.phase()
                        == harness_runtime::mobile::MediaAttachmentPhase::Revoking =>
                {
                    // Timeout only ends this observation. Watch the same drain
                    // flight without polling or destroying its executor.
                    tracing::warn!(%reason, "unbuilt Android media drain is still running");
                    if attachment.wait_for_drain_completion().await
                        == harness_runtime::mobile::MediaAttachmentPhase::Revoked
                        && matches!(
                            attachment.revoke(std::time::Duration::from_secs(5)).await,
                            MediaAttachmentRevocation::Confirmed
                        )
                    {
                        on_confirmed();
                    }
                }
                MediaAttachmentRevocation::Unknown { reason } => {
                    tracing::warn!(%reason, "unbuilt Android media attachment remains retained");
                }
            }
        }) {
            tracing::warn!(%error, "could not schedule unbuilt Android media attachment cleanup; admission remains closed");
        }
    }

    fn engine_key(engine: &Arc<MobileEngineHandle>) -> usize {
        Arc::as_ptr(engine) as usize
    }

    fn register_engine_binding(
        &self,
        engine: &Arc<MobileEngineHandle>,
        binding: &Arc<AndroidEngineMediaBinding>,
    ) {
        let key = Self::engine_key(engine);
        binding.engine_key.store(key, Ordering::Release);
        let mut bindings = self
            .engine_bindings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        bindings.retain(|_, binding| binding.strong_count() > 0);
        bindings.insert(key, Arc::downgrade(binding));
    }

    fn engine_binding(
        &self,
        engine: &Arc<MobileEngineHandle>,
    ) -> Option<Arc<AndroidEngineMediaBinding>> {
        let mut bindings = self
            .engine_bindings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let binding = bindings
            .get(&Self::engine_key(engine))
            .and_then(Weak::upgrade);
        if binding.is_none() {
            bindings.remove(&Self::engine_key(engine));
        }
        binding
    }

    pub(crate) fn audio_service(&self) -> Arc<dyn AudioService> {
        self.inner.audio_service()
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

#[cfg(all(feature = "uniffi", target_os = "android"))]
fn selected_mobile_brand_profile(
    directories: &harness_runtime::mobile::MobileUserDirectories,
    brand_id: &str,
    linux_user: &str,
    config_dir_name: &str,
) -> Result<mobile_linux_api::BrandRuntimeProfile, MobileEngineError> {
    let profile = mobile_linux_api::BrandRuntimeProfile::new(brand_id, linux_user, config_dir_name);
    profile
        .validate()
        .map_err(|error| MobileEngineError::Internal(error.to_string()))?;
    if directories.brand_id != brand_id
        || directories.linux_user != linux_user
        || directories.config_dir_name != config_dir_name
    {
        return Err(MobileEngineError::Internal(
            "native MobileUserDirectories and runtime brand profile do not match".into(),
        ));
    }
    Ok(profile)
}

/// Create a RAM-only side using the same typed callbacks as the main chat.
#[cfg(feature = "uniffi")]
#[uniffi::export(async_runtime = "tokio")]
pub async fn create_android_side_chat(
    engine: Arc<MobileEngineHandle>,
    request_id: String,
    initial_input: Option<ClientTextDto>,
    listener: Box<dyn AndroidEventListener>,
    permissions: Box<dyn AndroidPermissionSink>,
) -> Result<harness_runtime::mobile::SideChatInfoDto, MobileEngineError> {
    engine
        .create_side_chat(
            request_id,
            initial_input,
            Arc::new(AndroidListenerBridge {
                inner: listener,
                _media_binding: None,
            }),
            Arc::new(super::callbacks::AndroidPermissionSinkBridge { inner: permissions }),
        )
        .await
}

/// The foreign (Kotlin) capability objects + config needed to build an
/// `AndroidPlatform`. `UniFFI` marshals each `Arc<dyn …>` as a callback-interface
/// reference; `app_files_root` is the app's private files-dir.
pub struct PlatformImpls {
    /// Kotlin `CameraControl` impl (`CameraX`).
    pub camera: Arc<dyn CameraControl>,
    /// Unified Kotlin device AudioService.
    pub audio: Arc<dyn AudioService>,
    /// Kotlin `SharingService` impl (`Intent.ACTION_SEND`).
    pub share: Arc<dyn SharingService>,
    /// The app's private files-dir root.
    pub app_files_root: String,
    /// Optional mobile-linux runtime configuration.
    pub mobile_linux: Option<AndroidMobileLinuxConfigFfi>,
}

/// Build the lightweight Android scheduled-task store without constructing an
/// LLM client or reading any Provider credential. The same managed-workspace
/// validation as [`build_android_engine`] is applied before
/// the handle can read or mutate a task file.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn build_android_cron_store(
    app_files_root: String,
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
    let cwd = android_project_cwd(&managed_root, project_cwd.as_deref())?;
    let app_root = std::path::Path::new(&app_files_root)
        .canonicalize()
        .map_err(|error| {
            MobileEngineError::Internal(format!("Android app files root is unavailable: {error}"))
        })?;
    Ok(Arc::new(MobileCronStoreHandle::new(
        cwd,
        Arc::new(PosixFileSystem::new(app_root)),
        Arc::new(PosixClock::new()),
    )))
}

/// Top-level `UniFFI` constructor: build the mobile engine from the Kotlin-supplied
/// platform callbacks + event listener. (Under `uniffi`: `#[uniffi::export]`.)
///
/// This is a THIN wrapper: it constructs the Android-specific `Platform` from the
/// foreign callbacks and then delegates ALL runtime/adapter/listener wiring to
/// the shared [`harness_runtime::mobile::build_mobile_engine`] (F3-04) — so the heavy
/// lifting lives in exactly one place. The returned [`MobileEngineHandle`] owns
/// the tokio runtime + the wired orchestrator + the registered listener.
///
/// On non-Android hosts this returns [`MobileEngineError::PlatformUnavailable`] —
/// the `AndroidPlatform` is only linked under `cfg(target_os = "android")` — so
/// the crate still compiles and the SHARED host is exercised off-device through
/// the test shim (which calls `build_mobile_engine` with a portable fake
/// `Platform`).
#[cfg(feature = "uniffi")]
pub fn build_mobile_engine(
    impls: PlatformImpls,
    listener: Arc<dyn ClientEventListener>,
    permission_sink: Arc<dyn PermissionRequestSink>,
) -> Result<Arc<MobileEngineHandle>, MobileEngineError> {
    #[cfg(target_os = "android")]
    {
        use platform_android::{AndroidPlatform, AndroidPlatformInputs};
        let mut cfg = MobileConfig {
            build_info: harness_runtime::mobile::BuildInfo::new(
                env!("CARGO_PKG_VERSION"),
                option_env!("LINGXI_GIT_SHA_SHORT").unwrap_or("unknown"),
            ),
            cwd: std::path::PathBuf::from(&impls.app_files_root),
            lingxi_home: std::path::PathBuf::from(&impls.app_files_root).join(branding::DOT_DIR),
            host_environment: Some(lingxi_core::host::MobileHostEnvironment::new(
                lingxi_core::host::MobileHostOs::Android,
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
        let mobile_linux_config = impls.mobile_linux.as_ref().ok_or_else(|| {
            MobileEngineError::Internal("Android PRoot runtime is required".into())
        })?;
        if !matches!(
            mobile_linux_config.mode,
            super::linux_types::MobileLinuxRuntimeModeFfi::MobileLinux
        ) {
            return Err(MobileEngineError::Internal(
                "Android conversations require Mobile Linux".into(),
            ));
        }
        let directories = harness_runtime::mobile::mobile_user_directories(
            mobile_linux_config.managed_root.clone(),
        )?;
        let brand_profile = selected_mobile_brand_profile(
            &directories,
            &mobile_linux_config.brand_id,
            &mobile_linux_config.linux_user,
            &mobile_linux_config.config_dir_name,
        )?;
        directories.prepare()?;
        directories.validate_projects_mount(
            mobile_linux_config
                .workspace_host_path
                .as_deref()
                .unwrap_or(""),
            mobile_linux_config
                .stable_workspace_id
                .as_deref()
                .unwrap_or(""),
        )?;
        let (cwd, logical_cwd) = directories.resolve_cwd(None)?;
        cfg.cwd = cwd;
        cfg.logical_cwd = Some(logical_cwd);
        cfg.lingxi_home = std::path::PathBuf::from(&directories.config_home);
        cfg.brand_runtime_profile = brand_profile.clone();
        let mobile_linux = prepared_android_mobile_linux_runtime(mobile_linux_config)
            .map_err(MobileEngineError::Internal)?;
        let platform: Arc<dyn Platform> = Arc::new(AndroidPlatform::new(AndroidPlatformInputs {
            brand_profile: brand_profile.clone(),
            app_files_root: std::path::PathBuf::from(impls.app_files_root),
            camera: impls.camera,
            audio: impls.audio,
            location: None,
            share: impls.share,
            notifications: None,
            clipboard: None,
            device_status: None,
            haptics: None,
            deep_link: None,
            calendar: None,
            contacts: None,
            mobile_linux,
            mobile_linux_workspace_root: impls
                .mobile_linux
                .as_ref()
                .and_then(|cfg| cfg.workspace_host_path.clone())
                .map(std::path::PathBuf::from),
            mobile_linux_workspace_id: impls
                .mobile_linux
                .as_ref()
                .and_then(|cfg| cfg.stable_workspace_id.clone()),
            mobile_linux_managed_root: impls
                .mobile_linux
                .as_ref()
                .map(|cfg| std::path::PathBuf::from(cfg.managed_root.clone())),
            secure_storage: None,
            android_ui_automation: None,
        }));
        harness_runtime::mobile::build_mobile_engine(cfg, platform, listener, permission_sink)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (impls, listener, permission_sink);
        Err(MobileEngineError::PlatformUnavailable)
    }
}

/// The mobile `Git`-tool registration gate (spec P4 §G5): enabled iff config
/// opts in AND the workspace is a ready directory AND the CA store is reachable.
/// The token is deliberately NOT part of the gate (spec §G5) — a missing token
/// disables only the network ops (clone/fetch/pull), surfaced via `has_token`.
/// Pure; host-testable (NOT `cfg(target_os)`-gated, so the host tests reach it).
///
/// Called from the `cfg(target_os = "android")` branch of
/// [`build_android_engine`]; on the host build the only caller is the unit test,
/// so `allow(dead_code)` there (mirrors the file's other host-unused items).
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
#[must_use]
pub(super) fn android_git_gate(
    enable_git: bool,
    workspace_ready: bool,
    ca_store_reachable: bool,
) -> bool {
    enable_git && workspace_ready && ca_store_reachable
}

#[cfg(feature = "uniffi")]
#[must_use]
pub(super) fn android_guest_shell_enabled(shell: Option<&AndroidShellConfigFfi>) -> bool {
    shell.is_some_and(|settings| {
        settings.enable_shell
            && settings.secrets_in_keystore
            && settings.shell_data_exposure_accepted
    })
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_lines)]
pub fn build_android_engine(
    config: AndroidEngineLaunchConfigFfi,
    listener: Box<dyn AndroidEventListener>,
    media_host: Arc<AndroidNativeMediaHost>,
    share: Box<dyn AndroidShare>,
    location: Box<dyn AndroidLocation>,
    notifications: Box<dyn AndroidNotification>,
    clipboard: Box<dyn AndroidClipboard>,
    permissions: Box<dyn AndroidPermissionSink>,
    computer_use: Option<Box<dyn AndroidComputerUseHost>>,
    shell: Option<AndroidShellConfigFfi>,
    git: Option<AndroidGitConfigFfi>,
    git_credential_provider: Option<Box<dyn AndroidGitCredentialProvider>>,
    secure_storage: Option<Box<dyn AndroidSecureStorage>>,
    device_control: Option<Box<dyn AndroidDeviceControl>>,
) -> Result<Arc<MobileEngineHandle>, MobileEngineError> {
    let AndroidEngineLaunchConfigFfi {
        app_version,
        app_agent_host,
        api_base,
        api_key,
        model,
        session_mode,
        vision_delegation_enabled,
        app_files_root,
        project_cwd,
        provider_config,
        mobile_linux,
        physical_memory_bytes,
        host_environment,
    } = config;
    #[cfg(target_os = "android")]
    let device_control = device_control.map(|inner| Arc::new(AndroidDeviceControlBridge { inner }));
    // Everything after this point that only the `cfg(android)` arm consumes has
    // to be discarded HERE, not bound as `_` in the destructuring above: an `_`
    // there silences the macOS warning by deleting the NAME, and the Android arm
    // — which no gate on this machine compiles — then fails to find it.
    #[cfg(not(target_os = "android"))]
    let _ = (
        app_version,
        app_agent_host,
        project_cwd,
        device_control,
        session_mode,
        vision_delegation_enabled,
        physical_memory_bytes,
        media_host.clone(),
    );
    #[cfg(target_os = "android")]
    {
        use platform_android::{AndroidPlatform, AndroidPlatformInputs};
        harness_runtime::mobile::initialize_mobile_lifecycle_runtime()
            .map_err(MobileEngineError::Internal)?;
        let audio = media_host.audio_service();
        let camera = media_host.camera_control();
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
        // Git key paths remain anchored to the app-private files root.
        let app_files_root_str = app_files_root.clone();
        let linux = mobile_linux.as_ref().ok_or_else(|| {
            MobileEngineError::Internal("Android requires its Linux runtime user storage".into())
        })?;
        if !matches!(
            linux.mode,
            super::linux_types::MobileLinuxRuntimeModeFfi::MobileLinux
        ) {
            return Err(MobileEngineError::Internal(
                "Android conversations require Mobile Linux".into(),
            ));
        }
        let directories =
            harness_runtime::mobile::mobile_user_directories(linux.managed_root.clone())?;
        let brand_profile = selected_mobile_brand_profile(
            &directories,
            &linux.brand_id,
            &linux.linux_user,
            &linux.config_dir_name,
        )?;
        directories.prepare()?;
        directories.validate_projects_mount(
            linux.workspace_host_path.as_deref().unwrap_or(""),
            linux.stable_workspace_id.as_deref().unwrap_or(""),
        )?;
        let (cwd, logical_cwd) = directories.resolve_cwd(project_cwd.as_deref())?;
        let mut cfg = MobileConfig {
            brand_runtime_profile: brand_profile.clone(),
            app_agent_host,
            startup_catalog: app_version.as_ref().map(|version| {
                harness_runtime::StartupCatalog::new(
                    harness_runtime::CatalogPlatform::Android,
                    version,
                    &std::path::PathBuf::from(&directories.config_home),
                )
            }),
            build_info: harness_runtime::mobile::BuildInfo::new(
                env!("CARGO_PKG_VERSION"),
                option_env!("LINGXI_GIT_SHA_SHORT").unwrap_or("unknown"),
            ),
            cwd,
            logical_cwd: Some(logical_cwd),
            lingxi_home: std::path::PathBuf::from(&directories.config_home),
            session_mode: match session_mode {
                SessionModeDto::Chat => MobileSessionMode::Chat,
                SessionModeDto::Code => MobileSessionMode::Code,
            },
            physical_memory_bytes,
            vision_delegation_enabled,
            host_environment: Some(host_environment.map_or_else(
                || {
                    lingxi_core::host::MobileHostEnvironment::new(
                        lingxi_core::host::MobileHostOs::Android,
                        None,
                        lingxi_core::host::MobileDeviceClass::Unknown,
                        lingxi_core::host::MobileExecutionTarget::Unknown,
                        lingxi_core::host::MobileLaunchMode::Unknown,
                    )
                },
                Into::into,
            )),
            // P0.2: production injects the real LINGXI.md hierarchy provider so the
            // orchestrator loads `<cwd>/LINGXI.md` + `<lingxi_home>/LINGXI.md` into
            // its system prompt and `fire_instructions_loaded()` fires over them.
            memory_provider: Some(orchestrator::prompt::real_provider()),
            // The native WebView host renders inline widgets.
            inline_visualization: true,
            // The Apps page and the card under the tool call open local apps.
            local_apps: true,
            media_service: None,
            media_conversation_attachment: None,
            ..MobileConfig::default()
        };
        if !api_base.is_empty() {
            cfg.api_base = api_base;
        }
        cfg.api_key = api_key;
        if !model.is_empty() {
            cfg.default_model = model;
        }
        if let Some(provider_config) = provider_config {
            let (profiles, routing) = harness_runtime::mobile::parse_mobile_provider_config_json(
                &provider_config.provider_profiles_json,
                provider_config.routing_json.as_deref(),
            )?;
            cfg.provider_profiles = profiles;
            cfg.routing = routing;
        }
        if android_guest_shell_enabled(shell.as_ref()) {
            cfg.enable_mobile_linux_shell();
        }
        let mobile_linux_config = mobile_linux.as_ref().ok_or_else(|| {
            MobileEngineError::Internal("Android PRoot runtime is required".into())
        })?;
        let runtime = prepared_android_mobile_linux_runtime(mobile_linux_config)
            .map_err(MobileEngineError::Internal)?;
        let android_platform = AndroidPlatform::new(AndroidPlatformInputs {
            brand_profile: brand_profile.clone(),
            app_files_root: std::path::PathBuf::from(app_files_root),
            camera,
            audio,
            location: Some(Arc::new(AndroidLocationBridge { inner: location })),
            share: Arc::new(AndroidShareBridge { inner: share }),
            notifications: Some(Arc::new(AndroidNotificationBridge {
                inner: notifications,
            })),
            clipboard: Some(Arc::new(AndroidClipboardBridge { inner: clipboard })),
            device_status,
            haptics,
            deep_link,
            calendar,
            contacts,
            mobile_linux: runtime,
            mobile_linux_workspace_root: mobile_linux_config
                .workspace_host_path
                .clone()
                .map(std::path::PathBuf::from),
            mobile_linux_workspace_id: mobile_linux_config.stable_workspace_id.clone(),
            mobile_linux_managed_root: Some(std::path::PathBuf::from(
                &mobile_linux_config.managed_root,
            )),
            secure_storage: secure_storage.map(|storage| {
                Arc::new(AndroidSecureStorageBridge { inner: storage })
                    as Arc<dyn lingxi_core::host::SecureStorage>
            }),
            android_ui_automation: computer_use.map(|host| {
                Arc::new(AndroidComputerUseBridge { inner: host })
                    as Arc<dyn lingxi_core::host::AndroidUiAutomation>
            }),
        });
        if let Some(c) = git {
            let credential_provider: Option<std::sync::Arc<dyn tool_api::GitCredentialProvider>> =
                git_credential_provider.map(|p| {
                    std::sync::Arc::new(AndroidGitCredentialProviderBridge { inner: p })
                        as std::sync::Arc<dyn tool_api::GitCredentialProvider>
                });
            let workspace_ready = std::path::Path::new(&c.workspace_root).is_dir();
            // CA trust store. mbedTLS fails closed against an EMPTY chain, so an
            // empty `ca_cert_dir` means every HTTPS op will fail with an opaque
            // cert error. Treat an empty CA dir as reachable ONLY for local-only
            // use (no network credential provider); if the host wired a
            // credential provider (network intended) but no CA dir, do NOT
            // advertise the git tool as ready — that would be a tool that looks
            // usable yet hard-fails every clone/fetch/pull/push.
            let ca_store_reachable = if c.ca_cert_dir.is_empty() {
                credential_provider.is_none()
            } else {
                std::path::Path::new(&c.ca_cert_dir).exists()
            };
            cfg.android_git = Some(tool_api::MobileGitToolCtx {
                enabled: android_git_gate(c.enable_git, workspace_ready, ca_store_reachable),
                has_token: credential_provider.is_some(),
                workspace_root: c.workspace_root.clone(),
            });
            // Defense-in-depth: SSH key paths are HOST-supplied (not
            // model-supplied), but still validate they stay inside the app
            // sandbox (`app_files_root`) before handing them to libgit2, and use
            // the CANONICAL path that was containment-checked (not the raw
            // string) so the path libssh2 actually opens is exactly the one
            // validated — closing a check-then-use TOCTOU. An empty private path
            // = no SSH. If the private key fails validation, drop ALL ssh fields
            // so an SSH op reports "not configured" rather than using an
            // out-of-sandbox key. The public key (non-secret) is validated the
            // same way; on failure it is dropped to None so libssh2 derives it
            // from the private key, rather than reading an out-of-sandbox file.
            let ssh_root = std::path::Path::new(&app_files_root_str);
            let canonical_priv = if c.ssh_private_key_path.is_empty() {
                None
            } else {
                tool_git_mobile::auth::validate_ssh_key_path(&c.ssh_private_key_path, ssh_root).ok()
            };
            let (ssh_private_key_path, ssh_public_key_path, ssh_known_hosts) =
                if let Some(priv_canon) = canonical_priv {
                    let pub_canon = if c.ssh_public_key_path.is_empty() {
                        None
                    } else {
                        tool_git_mobile::auth::validate_ssh_key_path(
                            &c.ssh_public_key_path,
                            ssh_root,
                        )
                        .ok()
                        .map(|p| p.to_string_lossy().into_owned())
                    };
                    (
                        Some(priv_canon.to_string_lossy().into_owned()),
                        pub_canon,
                        c.ssh_known_hosts_sha256_hex,
                    )
                } else {
                    (None, None, Vec::new())
                };
            cfg.android_git_secret = Some(tool_api::MobileGitSecret {
                credential_provider,
                ca_dir: if c.ca_cert_dir.is_empty() {
                    None
                } else {
                    Some(c.ca_cert_dir)
                },
                ssh_private_key_path,
                ssh_public_key_path,
                ssh_known_hosts_sha256_hex: ssh_known_hosts,
            });
        }

        let platform: Arc<dyn Platform> = Arc::new(android_platform);
        let permission_sink: Arc<dyn PermissionRequestSink> =
            Arc::new(AndroidPermissionSinkBridge { inner: permissions });
        // Local App Agent engines deliberately have no media capability. Every
        // interactive/headless engine receives one app-host-issued attachment,
        // and both its tool service and Conversation adapter use that same ID.
        let media_scope = if app_agent_host {
            None
        } else {
            Some(media_host.create_engine_media_scope()?)
        };
        if let Some((attachment, conversation_attachment, _)) = media_scope.as_ref() {
            cfg.media_service = Some(attachment.service());
            cfg.media_attachment_handle = Some(attachment.clone());
            cfg.media_attachment_confirmed_callback =
                Some(media_host.attachment_confirmed_callback(attachment));
            cfg.media_conversation_attachment = Some(conversation_attachment.clone());
        }
        let media_binding = media_scope.as_ref().map(|(_, _, binding)| binding.clone());
        let listener: Arc<dyn ClientEventListener> = Arc::new(AndroidListenerBridge {
            inner: listener,
            _media_binding: media_binding.clone(),
        });
        let engine = match harness_runtime::mobile::build_mobile_engine(
            cfg,
            platform,
            listener,
            permission_sink,
        ) {
            Ok(engine) => engine,
            Err(error) => {
                if let Some(binding) = media_binding {
                    media_host.revoke_unbuilt_attachment(binding);
                }
                return Err(error);
            }
        };
        if let Some(media_binding) = media_binding {
            media_host.register_engine_binding(&engine, &media_binding);
            // Interactive and scheduled engines establish their trusted route
            // under this attachment before a Conversation handoff can open.
            let _ = engine
                .runtime()
                .block_on(media_host.update_audio_session_context(engine.clone()));
        }
        Ok(engine)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (
            api_base,
            api_key,
            model,
            app_files_root,
            listener,
            media_host,
            share,
            location,
            notifications,
            clipboard,
            permissions,
            computer_use,
            shell,
            git,
            provider_config,
            mobile_linux,
            host_environment,
            git_credential_provider,
            secure_storage,
        );
        Err(MobileEngineError::PlatformUnavailable)
    }
}

#[cfg(all(test, feature = "uniffi"))]
mod mobile_media_scope_tests {
    use super::super::callbacks::{
        AndroidAudioFfiError, AndroidMediaDeviceCapabilities, AndroidMediaDeviceResponse,
        CameraFfiError, CapturedImageFfi,
    };
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
    impl AndroidAudioService for Audio {
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
        async fn cancel(
            &self,
            _: audio_dto::AudioOperationIdDto,
        ) -> Result<(), AndroidAudioFfiError> {
            Ok(())
        }
    }
    struct Camera;
    #[async_trait::async_trait]
    impl AndroidCamera for Camera {
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
    }
    struct Devices;
    #[async_trait::async_trait]
    impl AndroidMediaDevicePorts for Devices {
        fn device_capabilities(&self) -> AndroidMediaDeviceCapabilities {
            AndroidMediaDeviceCapabilities {
                session_modes: Vec::new(),
            }
        }
        async fn request(
            &self,
            _: media_dto::MediaDeviceRequestEnvelopeDto,
        ) -> AndroidMediaDeviceResponse {
            panic!("this fixture does not open devices")
        }
    }

    #[tokio::test]
    async fn native_host_engine_scopes_share_service_without_sharing_owner_routes() {
        let config = r#"{"schemaVersion":4,"recognition":{"source":"automatic","cloud":{"binding":"follow_session"}},"speech":{"source":"automatic","cloud":{"binding":"follow_session"}},"language":"auto","rate":1.0}"#;
        let host = AndroidNativeMediaHost::new(
            Box::new(Audio),
            Box::new(Camera),
            Box::new(Devices),
            config.into(),
        )
        .unwrap();
        let (first, _, first_binding) = host.create_engine_media_scope().unwrap();
        let (second, _, second_binding) = host.create_engine_media_scope().unwrap();
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
        for (generation, attachment) in [(1, &first), (2, &second)] {
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
        for (attachment, provider) in [(&first, &first_provider), (&second, &second_provider)] {
            let permit = attachment.authorize_owner(&owner).unwrap();
            registry
                .register_attached_profile(&permit, "openai", provider.clone())
                .unwrap();
            registry
                .bind_attached_session_to_profile(&permit, "openai", "profile:openai")
                .unwrap();
        }
        let callback = host.attachment_confirmed_callback(&first);
        callback();
        assert!(registry
            .provider_for_attached_session(first.id(), "same-session", "openai", "profile:openai")
            .is_ok());
        let (idle, _, _) = host.create_engine_media_scope().unwrap();
        let _idle_audio = idle.audio_service(host.audio_service()).unwrap();
        let idle_key = (idle.id().host_id(), idle.id().generation());
        let idle_callback = host.attachment_confirmed_callback(&idle);
        assert_eq!(
            idle.revoke(Duration::from_secs(2)).await,
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
        host.attachment_confirmed_callback(&second)();
    }
}

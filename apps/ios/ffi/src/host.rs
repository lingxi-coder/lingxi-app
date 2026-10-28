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
    ClientEventListener, MobileCronStoreHandle, MobileEngineError, MobileEngineHandle,
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
    binding: std::sync::Mutex<Option<MediaAudioSessionBindingFfi>>,
    refresh: tokio::sync::Mutex<()>,
}

#[cfg(feature = "uniffi")]
impl MediaAudioSessionBindingGuard {
    fn new(registry: Arc<AudioProviderCloudRegistry>) -> Self {
        Self {
            registry,
            binding: std::sync::Mutex::new(None),
            refresh: tokio::sync::Mutex::new(()),
        }
    }

    fn update(&self, binding: MediaAudioSessionBindingFfi) {
        let mut current = self
            .binding
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(previous) = current.replace(binding) {
            self.registry
                .unregister_session_if_generation(&previous.session_id, previous.generation);
        }
    }

    fn clear(&self) {
        let mut current = self
            .binding
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(previous) = current.take() {
            self.registry
                .unregister_session_if_generation(&previous.session_id, previous.generation);
        }
    }
}

#[cfg(feature = "uniffi")]
impl Drop for MediaAudioSessionBindingGuard {
    fn drop(&mut self) {
        if let Some(binding) = self
            .binding
            .get_mut()
            .unwrap_or_else(|error| error.into_inner())
            .take()
        {
            self.registry
                .unregister_session_if_generation(&binding.session_id, binding.generation);
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
    pub session_id: String,
    pub generation: u64,
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
    audio_configuration_json: Arc<std::sync::RwLock<String>>,
    conversation_attachment: Arc<AppMediaConversationAttachment>,
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
        let callback = Arc::new(IosMediaDevicePortsBridge {
            inner: media_devices,
        });
        let ports: Arc<dyn MediaDevicePorts> = Arc::new(CallbackMediaDevicePorts::new(callback));
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
        let configuration: AudioConfigurationSnapshot = {
            let settings = audio_configuration_json.clone();
            Arc::new(move || {
                Ok(settings
                    .read()
                    .unwrap_or_else(|error| error.into_inner())
                    .clone())
            })
        };
        let conversation_attachment = AppMediaConversationAttachment::new(
            inner.cloud_audio_registry(),
            inner.media_service(),
            configuration,
        );
        Ok(Arc::new(Self {
            inner,
            audio,
            camera,
            audio_configuration_json,
            conversation_attachment,
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
            .and_then(Weak::upgrade);
        // Serialize refreshes for this engine so an older unconfigured result
        // cannot retire a newer successful binding.
        let _refresh = match guard.as_ref() {
            Some(guard) => Some(guard.refresh.lock().await),
            None => None,
        };
        let context = match engine.current_media_context().await {
            Ok(context) => context,
            Err(_) => {
                if let Some(guard) = guard.as_ref() {
                    guard.clear();
                }
                return Ok(None);
            }
        };
        let session_id = context.session_id.clone();
        let (services, credential_ids, _) = engine.audio_provider_services();
        let provider = Arc::new(AudioProviderHost::from_provider_services(
            services,
            credential_ids.clone(),
        ));
        for profile_name in credential_ids.keys() {
            self.inner
                .cloud_audio_registry()
                .register_profile(profile_name.clone(), provider.clone())
                .map_err(|error| MobileEngineError::Internal(error.to_string()))?;
        }
        let generation = self
            .inner
            .cloud_audio_registry()
            .register_session(
                context.session_id.clone(),
                context.profile_id.clone(),
                context.account_scope.clone(),
                provider.clone(),
            )
            .map_err(|error| MobileEngineError::Internal(error.to_string()))?;
        let binding = MediaAudioSessionBindingFfi {
            session_id,
            generation,
        };
        if let Some(guard) = guard.as_ref() {
            guard.update(binding.clone());
        }
        Ok(Some(binding))
    }

    /// Remove a session's provider route when the Swift owner releases it.
    pub fn remove_audio_session_context(&self, session_id: String, generation: u64) {
        self.inner
            .cloud_audio_registry()
            .unregister_session_if_generation(&session_id, generation);
    }
}

#[cfg(feature = "uniffi")]
impl IosNativeMediaHost {
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

    pub(crate) fn media_conversation_attachment(
        &self,
    ) -> Arc<dyn harness_runtime::mobile::MediaConversationAttachment> {
        self.conversation_attachment.clone()
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
        let route_guard = media_host.as_ref().map(|host| {
            Arc::new(MediaAudioSessionBindingGuard::new(
                host.inner.cloud_audio_registry(),
            ))
        });
        let listener: Arc<dyn ClientEventListener> = Arc::new(IosListenerBridge {
            inner: listener,
            _media_route_guard: retain_media_route_guard(route_guard.as_ref()),
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

            let mut cfg = ios_mobile_config_from_launch_config(&config)?;
            if let Some(host) = media_host.as_ref() {
                cfg.media_service = Some(host.media_service());
                cfg.media_conversation_attachment = Some(host.media_conversation_attachment());
            }
            let additional_workspace_mounts = ios_project_mount_grants(&config)?;
            let (workspace_host_path, stable_workspace_id) = match config.mobile_linux.as_ref() {
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
            if let Some((host, guard)) = media_host.as_ref().zip(route_guard.as_ref()) {
                host.track_engine_route(&engine, guard);
                // This UniFFI constructor remains synchronous for its Swift callers.
                // It runs outside the engine runtime, so use the engine-owned runtime
                // for the initial trusted provider/session binding.
                engine
                    .runtime()
                    .block_on(host.update_audio_session_context(engine.clone()))?;
            }
            Ok(engine)
        }
        #[cfg(not(target_os = "ios"))]
        {
            let _ = (
                config,
                listener,
                media_host,
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
    use harness_runtime::mobile::MobileEngineError;
    use std::cell::Cell;
    use std::sync::Arc;

    #[test]
    fn listener_guard_retains_the_route_until_listener_release() {
        assert!(retain_media_route_guard(None).is_none());

        let registry = Arc::new(AudioProviderCloudRegistry::new("wav"));
        let guard = Arc::new(MediaAudioSessionBindingGuard::new(registry));
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
        let guard = MediaAudioSessionBindingGuard::new(registry.clone());
        let first = registry
            .register_session("session", "first", "profile:first", provider.clone())
            .unwrap();
        guard.update(MediaAudioSessionBindingFfi {
            session_id: "session".into(),
            generation: first,
        });
        guard.clear();
        assert_eq!(registry.session_identity("session"), None);

        let old = registry
            .register_session("session", "old", "profile:old", provider.clone())
            .unwrap();
        guard.update(MediaAudioSessionBindingFfi {
            session_id: "session".into(),
            generation: old,
        });
        registry
            .register_session("session", "new", "profile:new", provider)
            .unwrap();
        guard.clear();
        assert_eq!(
            registry.session_identity("session"),
            Some(("new".into(), "profile:new".into()))
        );
    }
}

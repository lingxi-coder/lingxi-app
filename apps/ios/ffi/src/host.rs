#[cfg(feature = "uniffi")]
use super::callbacks::{
    IosAudioService, IosCamera, IosClipboard, IosDeviceControl, IosEventListener,
    IosListenerBridge, IosLocation, IosNotification, IosPermissionSink, IosSecureStorage, IosShare,
};
#[cfg(all(feature = "uniffi", target_os = "ios"))]
use super::callbacks::{
    IosAudioServiceBridge, IosCameraBridge, IosClipboardBridge, IosDeviceControlBridge,
    IosLocationBridge, IosNotificationBridge, IosPermissionSinkBridge, IosSecureStorageBridge,
    IosShareBridge,
};
#[cfg(all(feature = "uniffi", target_os = "ios"))]
use super::configuration::{
    default_workspace_host_path, ios_mobile_config_from_launch_config,
    validate_mobile_linux_workspace_config,
};
#[cfg(feature = "uniffi")]
use super::configuration::{ios_project_cwd, IosEngineLaunchConfigFfi, IosMobileLinuxConfigFfi};
#[cfg(all(feature = "uniffi", target_os = "ios"))]
use super::linux_runtime::ios_mobile_linux_runtime;
#[cfg(all(feature = "uniffi", target_os = "ios"))]
use super::linux_types::MobileLinuxRuntimeModeFfi;
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
        let cfg = MobileConfig {
            build_info: harness_runtime::mobile::BuildInfo::new(
                env!("CARGO_PKG_VERSION"),
                option_env!("LINGXI_GIT_SHA_SHORT").unwrap_or("unknown"),
            ),
            cwd: std::path::PathBuf::from(&impls.app_sandbox_root),
            lingxi_home: std::path::PathBuf::from(&impls.app_sandbox_root).join(branding::DOT_DIR),
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
            ..MobileConfig::default()
        };
        let (workspace_host_path, stable_workspace_id) = match impls.mobile_linux.as_ref() {
            Some(config) => validate_mobile_linux_workspace_config(&impls.app_sandbox_root, config)
                .map_err(|error| MobileEngineError::Internal(error.to_string()))?,
            None => (
                default_workspace_host_path(&impls.app_sandbox_root),
                "default".to_string(),
            ),
        };
        let platform: Arc<dyn Platform> = Arc::new(IosPlatform::new(IosPlatformInputs {
            app_sandbox_root: std::path::PathBuf::from(impls.app_sandbox_root),
            camera: impls.camera,
            audio: impls.audio,
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
/// listener, app-scoped audio callback, other device callbacks, and runtime
/// config. The handle owns its tokio runtime and streams every
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
    project_cwd: Option<String>,
) -> Result<Arc<MobileCronStoreHandle>, MobileEngineError> {
    use platform_posix_minimal::{PosixClock, PosixFileSystem};

    let scheduled_cwd;
    let project_cwd = match project_cwd {
        Some(path) => Some(path),
        None => {
            scheduled_cwd = std::path::Path::new(&app_sandbox_root).join("scheduled/workspace");
            std::fs::create_dir_all(&scheduled_cwd)
                .map_err(|e| MobileEngineError::Internal(e.to_string()))?;
            Some(scheduled_cwd.to_string_lossy().into_owned())
        }
    };
    let cwd = ios_project_cwd(&app_sandbox_root, project_cwd.as_deref())?;
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

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(default(location = None)))]
#[allow(clippy::too_many_arguments)] // FFI constructor: one flat arg per Swift callback.
pub fn build_ios_engine_with_config(
    config: IosEngineLaunchConfigFfi,
    listener: Box<dyn IosEventListener>,
    audio: Box<dyn IosAudioService>,
    camera: Box<dyn IosCamera>,
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
    let listener: Arc<dyn ClientEventListener> = Arc::new(IosListenerBridge { inner: listener });
    #[cfg(target_os = "ios")]
    let device_control = device_control.map(|inner| Arc::new(IosDeviceControlBridge { inner }));
    #[cfg(not(target_os = "ios"))]
    let _ = device_control;
    #[cfg(target_os = "ios")]
    {
        use platform_ios::{IosPlatform, IosPlatformInputs};
        let native_audio: Arc<dyn harness_runtime::mobile::NativeAudioService> =
            Arc::new(IosAudioServiceBridge { inner: audio });
        let audio = harness_runtime::mobile::from_native_audio_service(native_audio);
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

        let cfg = ios_mobile_config_from_launch_config(&config)?;
        let (workspace_host_path, stable_workspace_id) = match config.mobile_linux.as_ref() {
            Some(mobile_linux) => {
                validate_mobile_linux_workspace_config(&config.app_sandbox_root, mobile_linux)
                    .map_err(|error| MobileEngineError::Internal(error.to_string()))?
            }
            None => (
                default_workspace_host_path(&config.app_sandbox_root),
                "default".to_string(),
            ),
        };
        let platform: Arc<dyn Platform> = Arc::new(IosPlatform::new(IosPlatformInputs {
            app_sandbox_root: std::path::PathBuf::from(&config.app_sandbox_root),
            camera: Arc::new(IosCameraBridge { inner: camera }),
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
            mobile_linux: ios_mobile_linux_runtime(config.mobile_linux.as_ref()),
            workspace_host_path: Some(workspace_host_path),
            stable_workspace_id: Some(stable_workspace_id),
        }));
        let permission_sink: Arc<dyn PermissionRequestSink> =
            Arc::new(IosPermissionSinkBridge { inner: permissions });
        harness_runtime::mobile::build_mobile_engine(cfg, platform, listener, permission_sink)
    }
    #[cfg(not(target_os = "ios"))]
    {
        let _ = (
            config,
            listener,
            audio,
            camera,
            share,
            notifications,
            clipboard,
            permissions,
            secure_storage,
            location,
        );
        Err(MobileEngineError::PlatformUnavailable)
    }
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
    audio: Box<dyn IosAudioService>,
    camera: Box<dyn IosCamera>,
    share: Box<dyn IosShare>,
    notifications: Box<dyn IosNotification>,
    clipboard: Box<dyn IosClipboard>,
    permissions: Box<dyn IosPermissionSink>,
    mobile_linux: Option<IosMobileLinuxConfigFfi>,
    secure_storage: Option<Box<dyn IosSecureStorage>>,
    device_control: Option<Box<dyn IosDeviceControl>>,
) -> Result<Arc<MobileEngineHandle>, MobileEngineError> {
    build_ios_engine_with_config(
        IosEngineLaunchConfigFfi {
            api_base,
            api_key,
            model,
            session_mode: SessionModeDto::Code,
            app_sandbox_root,
            project_cwd: None,
            provider_config: None,
            mobile_linux,
            vision_delegation_enabled: true,
            physical_memory_bytes: 0,
            host_environment: None,
        },
        listener,
        audio,
        camera,
        share,
        notifications,
        clipboard,
        permissions,
        secure_storage,
        device_control,
        None,
    )
}

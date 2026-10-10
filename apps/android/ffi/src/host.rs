#[cfg(all(feature = "uniffi", target_os = "android"))]
use super::callbacks::AndroidSecureStorageBridge;
#[cfg(feature = "uniffi")]
use super::callbacks::{
    AndroidAudioService, AndroidCamera, AndroidClipboard, AndroidComputerUseHost,
    AndroidDeviceControl, AndroidEventListener, AndroidGitCredentialProvider,
    AndroidListenerBridge, AndroidLocation, AndroidNotification, AndroidPermissionSink,
    AndroidSecureStorage, AndroidShare,
};
#[cfg(all(feature = "uniffi", target_os = "android"))]
use super::callbacks::{
    AndroidAudioServiceBridge, AndroidCameraBridge, AndroidClipboardBridge,
    AndroidComputerUseBridge, AndroidDeviceControlBridge, AndroidGitCredentialProviderBridge,
    AndroidLocationBridge, AndroidNotificationBridge, AndroidPermissionSinkBridge,
    AndroidShareBridge,
};
use super::configuration::AndroidEngineLaunchConfigFfi;
#[cfg(feature = "uniffi")]
use super::configuration::{
    android_project_cwd, AndroidGitConfigFfi, AndroidMobileLinuxConfigFfi, AndroidShellConfigFfi,
};
#[cfg(all(feature = "uniffi", target_os = "android"))]
use super::linux_runtime::prepared_android_mobile_linux_runtime;
#[cfg(feature = "uniffi")]
use harness_runtime::mobile::{
    ClientEventListener, MobileCronStoreHandle, MobileEngineError, MobileEngineHandle,
    PermissionRequestSink,
};
#[cfg(all(feature = "uniffi", target_os = "android"))]
use harness_runtime::mobile::{MobileConfig, MobileSessionMode, SessionModeDto};
#[cfg(all(feature = "uniffi", target_os = "android"))]
use lingxi_core::host::Platform;
use lingxi_core::host::{AudioService, CameraControl, SharingService};
use std::sync::Arc;

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
    project_cwd: Option<String>,
) -> Result<Arc<MobileCronStoreHandle>, MobileEngineError> {
    use platform_posix_minimal::{PosixClock, PosixFileSystem};

    let scheduled_cwd;
    let project_cwd = match project_cwd {
        Some(path) => Some(path),
        None => {
            scheduled_cwd = std::path::Path::new(&app_files_root).join("scheduled/workspace");
            std::fs::create_dir_all(&scheduled_cwd)
                .map_err(|e| MobileEngineError::Internal(e.to_string()))?;
            Some(scheduled_cwd.to_string_lossy().into_owned())
        }
    };
    let cwd = android_project_cwd(&app_files_root, project_cwd.as_deref())?;
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
        let cfg = MobileConfig {
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
            ..MobileConfig::default()
        };
        let mobile_linux_config = impls.mobile_linux.as_ref().ok_or_else(|| {
            MobileEngineError::Internal("Android PRoot runtime is required".into())
        })?;
        let mobile_linux = prepared_android_mobile_linux_runtime(mobile_linux_config)
            .map_err(MobileEngineError::Internal)?;
        let platform: Arc<dyn Platform> = Arc::new(AndroidPlatform::new(AndroidPlatformInputs {
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
    audio: Box<dyn AndroidAudioService>,
    camera: Box<dyn AndroidCamera>,
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
        api_base,
        api_key,
        model,
        session_mode,
        vision_delegation_enabled,
        app_files_root,
        project_cwd,
        provider_config,
        mobile_linux,
        local_apps_full_runtime,
        local_apps_runtime_root,
        physical_memory_bytes,
        host_environment,
    } = config;
    let listener: Arc<dyn ClientEventListener> =
        Arc::new(AndroidListenerBridge { inner: listener });
    #[cfg(target_os = "android")]
    let device_control = device_control.map(|inner| Arc::new(AndroidDeviceControlBridge { inner }));
    // Everything after this point that only the `cfg(android)` arm consumes has
    // to be discarded HERE, not bound as `_` in the destructuring above: an `_`
    // there silences the macOS warning by deleting the NAME, and the Android arm
    // — which no gate on this machine compiles — then fails to find it.
    #[cfg(not(target_os = "android"))]
    let _ = (
        project_cwd,
        device_control,
        session_mode,
        vision_delegation_enabled,
        local_apps_full_runtime,
        local_apps_runtime_root,
        physical_memory_bytes,
    );
    #[cfg(target_os = "android")]
    {
        use platform_android::{AndroidPlatform, AndroidPlatformInputs};
        let native_audio: Arc<dyn harness_runtime::mobile::NativeAudioService> =
            Arc::new(AndroidAudioServiceBridge { inner: audio });
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
        // Git key paths remain anchored to the app-private files root.
        let app_files_root_str = app_files_root.clone();
        let cwd = android_project_cwd(&app_files_root, project_cwd.as_deref())?;
        let mut cfg = MobileConfig {
            build_info: harness_runtime::mobile::BuildInfo::new(
                env!("CARGO_PKG_VERSION"),
                option_env!("LINGXI_GIT_SHA_SHORT").unwrap_or("unknown"),
            ),
            cwd,
            lingxi_home: std::path::PathBuf::from(&app_files_root).join(branding::DOT_DIR),
            session_mode: match session_mode {
                SessionModeDto::Chat => MobileSessionMode::Chat,
                SessionModeDto::Code => MobileSessionMode::Code,
            },
            local_apps_full_runtime,
            local_apps_runtime_root: local_apps_runtime_root.map(std::path::PathBuf::from),
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
            app_files_root: std::path::PathBuf::from(app_files_root),
            camera: Arc::new(AndroidCameraBridge { inner: camera }),
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
            cfg.android_git = Some(tool_api::AndroidGitToolCtx {
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
            cfg.android_git_secret = Some(tool_api::AndroidGitSecret {
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
        harness_runtime::mobile::build_mobile_engine(cfg, platform, listener, permission_sink)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (
            api_base,
            api_key,
            model,
            app_files_root,
            listener,
            audio,
            camera,
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

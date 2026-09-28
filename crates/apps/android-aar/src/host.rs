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
use super::linux_runtime::android_mobile_linux_runtime;
#[cfg(all(feature = "uniffi", target_os = "android"))]
use super::linux_types::MobileLinuxRuntimeModeFfi;
#[cfg(feature = "uniffi")]
use harness_runtime::mobile::{
    ClientEventListener, MobileCronStoreHandle, MobileEngineError, MobileEngineHandle,
    PermissionRequestSink, SessionModeDto,
};
#[cfg(all(feature = "uniffi", target_os = "android"))]
use harness_runtime::mobile::{MobileConfig, MobileSessionMode};
#[cfg(all(feature = "uniffi", target_os = "android"))]
use platform_api::Platform;
use platform_api::{AudioService, CameraControl, SharingService};
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
/// validation as [`build_android_engine_with_mobile_linux`] is applied before
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
            host_environment: Some(platform_api::MobileHostEnvironment::new(
                platform_api::MobileHostOs::Android,
                None,
                platform_api::MobileDeviceClass::Unknown,
                platform_api::MobileExecutionTarget::Unknown,
                platform_api::MobileLaunchMode::Unknown,
            )),
            // P0.2: production injects the real LINGXI.md hierarchy provider so the
            // orchestrator loads `<cwd>/LINGXI.md` + `<lingxi_home>/LINGXI.md` into
            // its system prompt and `fire_instructions_loaded()` fires over them.
            memory_provider: Some(orchestrator::prompt::real_provider()),
            ..MobileConfig::default()
        };
        let mobile_linux_mode = impls
            .mobile_linux
            .as_ref()
            .map_or(mobile_linux_api::MobileLinuxRuntimeMode::Legacy, |cfg| {
                cfg.mode.into()
            });
        let platform: Arc<dyn Platform> = Arc::new(AndroidPlatform::new_with_mode(
            AndroidPlatformInputs {
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
                mobile_linux: android_mobile_linux_runtime(impls.mobile_linux.as_ref()),
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
                shell: None,
                secure_storage: None,
                android_ui_automation: None,
            },
            mobile_linux_mode,
        ));
        harness_runtime::mobile::build_mobile_engine(cfg, platform, listener, permission_sink)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (impls, listener, permission_sink);
        Err(MobileEngineError::PlatformUnavailable)
    }
}

/// The mobile `Shell`-tool registration gate (spec r3 §Registration gates +
/// D11, P5b §B2): enabled iff config opts in AND the device probe proves the
/// enforcement we promise AND the bundled mksh+toybox bootstrap succeeded. Pure;
/// host-testable (NOT `cfg(target_os)`-gated, so the host tests reach it). The
/// six conjuncts are `enable_shell` + the D11 secrets gate (config opt-in),
/// capability-available + seccomp-filter + net-deny-verified (probe-proven
/// enforcement), and `bundled_shell_ready` (P5b: the bundled shell was
/// bootstrapped and exec-verified). The bundled conjunct makes the gate
/// fail-closed — if the bundled mksh+toybox cannot be staged/exec'd, the Shell
/// stays absent rather than falling back to the device's system sh.
///
/// Called from the `cfg(target_os = "android")` branch of
/// [`build_android_engine`]; on the host build the only caller is the unit test,
/// so `allow(dead_code)` there (mirrors the file's other host-unused items).
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
// The six conjuncts ARE distinct boolean gate inputs (spec r3 §Registration
// gates + D11 + P5b §B2) named 1:1 at the single call site; a struct/enum would
// obscure the formula, not clarify it.
#[allow(clippy::fn_params_excessive_bools)]
#[must_use]
pub(super) fn android_shell_gate(
    enable_shell: bool,
    secrets_gate_satisfied: bool,
    caps_available: bool,
    seccomp_filter: bool,
    net_deny_verified: bool,
    bundled_shell_ready: bool,
) -> bool {
    enable_shell
        && secrets_gate_satisfied
        && caps_available
        && seccomp_filter
        && net_deny_verified
        && bundled_shell_ready
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

/// P5b bundled-shell bootstrap result — the typed values the android branch of
/// [`build_android_engine`] threads onto `AndroidShellConfig` (so `prepare`
/// targets the bundled mksh + leads PATH with the applet farm) and onto the
/// capability cache + Shell-tool ctx. `Some` ONLY when staging + exec
/// verification fully succeeded (spec P5b §B2 fail-closed).
#[cfg(target_os = "android")]
pub(super) struct BundledShell {
    pub(super) mksh_path: std::path::PathBuf,
    pub(super) mksh_hash: String,
    pub(super) applet_dir: std::path::PathBuf,
    pub(super) mksh_version: Option<String>,
}

/// P5b bundled-shell bootstrap: stage the toybox applet symlink farm in an
/// app-private dir and prove the version-locked bundled mksh+toybox shipped as
/// `libmksh.so`/`libtoybox.so` under `native_library_dir` execve + dispatch
/// end-to-end, then hash `libmksh.so` for the runner's content-identity check
/// (spec P5b §T4b — the recorded hash MUST equal the real sha256 or the runner
/// refuses to exec).
///
/// FAIL-CLOSED (spec P5b §B2): ANY I/O / staging / exec-verification failure
/// returns `None`, which drops the bundled config fields and flips the gate's
/// `bundled_shell_ready` conjunct false — the Shell then stays absent rather
/// than falling back to the device's system sh.
///
/// The applet-resolution mechanism is the symlink farm
/// (`<applet_dir>/<applet>` → `<native_library_dir>/libtoybox.so`): toybox
/// multicall-dispatches on `argv[0]`, so a symlink farm is the ONLY mechanism
/// (command-rewrite does not work). This mirrors the device-proven
/// [`android_bundled_shell_probe`] logic but returns typed values.
#[cfg(target_os = "android")]
pub(super) fn bootstrap_bundled_shell(
    native_library_dir: &str,
    app_files_root: &str,
) -> Option<BundledShell> {
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::symlink;
    use std::path::Path;
    use std::process::Command;

    let nl = Path::new(native_library_dir);
    let mksh = nl.join("libmksh.so");
    let toybox = nl.join("libtoybox.so");
    if !mksh.exists() || !toybox.exists() {
        return None;
    }

    // 1) (re)build the applet symlink farm in an app-private dir. Wipe any stale
    //    farm first so a re-launch always reflects the current bundled toybox.
    let applet_dir = Path::new(app_files_root).join("applet-bin");
    let _ = std::fs::remove_dir_all(&applet_dir);
    std::fs::create_dir_all(&applet_dir).ok()?;
    for applet in platform_android::capabilities::BUNDLED_TOYBOX_APPLETS {
        let link = applet_dir.join(applet);
        // symlink <applet_dir>/<applet> -> <nl>/libtoybox.so; tolerate a
        // pre-existing link (e.g. a racing relaunch) but fail closed otherwise.
        if symlink(&toybox, &link).is_err() && !link.exists() {
            return None;
        }
    }

    // 2) verify bundled exec end-to-end: bare mksh execve, then a toybox applet
    //    resolved via the symlink farm on PATH (proves argv[0] dispatch).
    let mksh_ok = Command::new(&mksh)
        .args(["-c", "echo hi"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("hi"))
        .unwrap_or(false);
    if !mksh_ok {
        return None;
    }
    let applet_ok = Command::new(&mksh)
        .args(["-c", "echo hi | grep hi"])
        .env("PATH", &applet_dir)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("hi"))
        .unwrap_or(false);
    if !applet_ok {
        return None;
    }

    // 3) hash libmksh.so for the runner identity check (must match what
    //    `prepare` records as `BundledHelper.hash`).
    let bytes = std::fs::read(&mksh).ok()?;
    let mksh_hash = hex::encode(Sha256::digest(&bytes));

    // 4) bundled mksh version (best-effort; mksh prints `$KSH_VERSION`).
    let mksh_version = Command::new(&mksh)
        .args(["-c", "echo $KSH_VERSION"])
        .output()
        .ok()
        .and_then(|o| {
            let v = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if v.is_empty() {
                None
            } else {
                Some(v)
            }
        });

    Some(BundledShell {
        mksh_path: mksh,
        mksh_hash,
        applet_dir,
        mksh_version,
    })
}

/// Foreign-callable constructor for the Android app (plan T2.2).
///
/// Builds a fully-wired [`MobileEngineHandle`] from the Kotlin-supplied event
/// listener + speech callbacks + runtime config. The handle owns its tokio
/// runtime and streams every [`client::protocol::events::ClientEvent`] to
/// `listener.on_event(..)`; the app drives turns via
/// [`MobileEngineHandle::submit`]. The `stt` / `tts` callbacks are bridged into
/// the mobile `Platform` so `tool-speech` can route through the device's native
/// recognizer / synthesizer.
///
/// - `api_base`  — Anthropic-compatible base URL.
/// - `api_key`   — read by Kotlin from an app setting at runtime. Empty is valid
///   (turns 401 at `run_turn`); never hardcoded here.
/// - `model`     — default model id for new turns.
/// - `app_files_root` — the app's private files-dir the engine roots its
///   filesystem + `~/.claude`-equivalent under.
/// - `listener`  — the foreign [`AndroidEventListener`] (bridged to the shared
///   [`ClientEventListener`]).
/// - `audio` — the app-scoped native callback implementing
///   [`platform_api::AudioService`] for every audio operation.
/// - `camera` — the foreign camera callback (bridged to
///   [`platform_api::CameraControl`]) so `tool-camera` routes through `CameraX` +
///   the system photo picker.
/// - `share` — the foreign share callback (bridged to
///   [`platform_api::SharingService`]) so `tool-share` routes through the system
///   `Intent.ACTION_SEND` share sheet.
/// - `location` — the foreign one-shot location callback (bridged to
///   [`platform_api::LocationProvider`]) used by approved local-app bridge requests.
/// - `shell` — optional Android sandbox/shell config (spec r3 §Android
///   inputs); `None`/`null` keeps shell support fully absent.
/// - `git` — optional Android Git-tool config (spec P4 §G5 gate + §G3 auth);
///   `None`/`null` keeps Git support fully absent.
/// - `provider_config` — non-secret provider profiles and routing JSON. Secrets
///   are submitted separately through `SetProviderCredential`.
/// - `mobile_linux` — optional Android mobile-linux config. When selected, the
///   phase-1 build wires a capability/status bridge and a blocked/unavailable
///   runtime stub; it does NOT link GPL runtime code.
/// On non-Android hosts this returns [`MobileEngineError::PlatformUnavailable`]
/// (the `AndroidPlatform` is only linked under `cfg(target_os = "android")`).
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
#[allow(clippy::too_many_arguments)]
// FFI constructor: one flat arg per Kotlin callback.
// Single linear constructor body: probe → shell gate → git gate → delegate. The
// per-tool gate blocks (spec r3 §Registration gates + P4 §G5) read most clearly
// inline at the one call site, so the length is intrinsic, not decomposable.
#[allow(clippy::too_many_lines)]
pub fn build_android_engine(
    api_base: String,
    api_key: String,
    model: String,
    app_files_root: String,
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
    build_android_engine_with_mobile_linux(
        AndroidEngineLaunchConfigFfi {
            api_base,
            api_key,
            model,
            session_mode: SessionModeDto::Code,
            vision_delegation_enabled: true,
            app_files_root,
            project_cwd: None,
            provider_config: None,
            mobile_linux: None,
            local_apps_full_runtime: false,
            local_apps_runtime_root: None,
            physical_memory_bytes: 0,
            host_environment: None,
        },
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
        git_credential_provider,
        secure_storage,
        device_control,
    )
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_lines)]
pub fn build_android_engine_with_mobile_linux(
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
            .map(|service| service.clone() as Arc<dyn platform_api::DeviceStatusProvider>);
        let haptics = device_control
            .clone()
            .map(|service| service.clone() as Arc<dyn platform_api::HapticService>);
        let calendar = device_control
            .clone()
            .map(|service| service.clone() as Arc<dyn platform_api::CalendarProvider>);
        let contacts = device_control
            .clone()
            .map(|service| service.clone() as Arc<dyn platform_api::ContactsProvider>);
        let deep_link =
            device_control.map(|service| service as Arc<dyn platform_api::DeepLinkOpener>);
        // P5b: capture the app-private files root as an owned `String` up front —
        // `app_files_root` is consumed below into `AndroidPlatformInputs`, but the
        // bundled-shell bootstrap (which must run BEFORE `shell_cfg` is built +
        // moved) needs it to stage the applet symlink farm under it.
        let app_files_root_str = app_files_root.clone();
        let local_apps_runtime_requested = local_apps_runtime_root.is_some();
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
                    platform_api::MobileHostEnvironment::new(
                        platform_api::MobileHostOs::Android,
                        None,
                        platform_api::MobileDeviceClass::Unknown,
                        platform_api::MobileExecutionTarget::Unknown,
                        platform_api::MobileLaunchMode::Unknown,
                    )
                },
                Into::into,
            )),
            // P0.2: production injects the real LINGXI.md hierarchy provider so the
            // orchestrator loads `<cwd>/LINGXI.md` + `<lingxi_home>/LINGXI.md` into
            // its system prompt and `fire_instructions_loaded()` fires over them.
            memory_provider: Some(orchestrator::prompt::real_provider()),
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
        // P5b-T7: bundled-shell bootstrap MUST run BEFORE `shell_cfg` is built —
        // `shell_cfg` is moved into `AndroidPlatformInputs.shell` (so `prepare`
        // can target the bundled mksh) below, before the capability probe + the
        // registration gate, so its bundled fields have to be set up front. The
        // arg-less `probe_android_capabilities()` cannot know the bundled paths,
        // so this is the only seam that owns them. Fail-closed (§B2): `None` ⇒ no
        // bundled fields ⇒ `bundled_ready=false` ⇒ gate false ⇒ Shell absent (no
        // system-sh fallback). `bundled` stays alive for the later cache + gate
        // reads — the `shell_cfg` builder only `as_ref().map(..)`-clones out of it.
        let bundled = shell
            .as_ref()
            .and_then(|s| bootstrap_bundled_shell(&s.native_library_dir, &app_files_root_str));
        let shell_cfg = shell.map(|s| platform_android::AndroidShellConfig {
            native_library_dir: std::path::PathBuf::from(s.native_library_dir),
            shell_workspace_root: std::path::PathBuf::from(s.shell_workspace_root),
            app_cache_root: std::path::PathBuf::from(s.app_cache_root),
            package_name: s.package_name,
            package_version_code: s.package_version_code,
            app_writable_roots: s.app_writable_roots.into_iter().map(Into::into).collect(),
            enable_shell: s.enable_shell,
            secrets_in_keystore: s.secrets_in_keystore,
            shell_data_exposure_accepted: s.shell_data_exposure_accepted,
            bundled_mksh_path: bundled.as_ref().map(|b| b.mksh_path.clone()),
            bundled_mksh_hash: bundled.as_ref().map(|b| b.mksh_hash.clone()),
            bundled_applet_dir: bundled.as_ref().map(|b| b.applet_dir.clone()),
        });
        // P3-T5: keep a clone of the shell config before it is moved into
        // `AndroidPlatformInputs.shell` — the registration gate (computed below,
        // after the probe) needs `enable_shell` + the D11 secrets gate from it.
        let shell_cfg_for_gate = shell_cfg.clone();
        let mobile_linux_mode = mobile_linux
            .as_ref()
            .map_or(mobile_linux_api::MobileLinuxRuntimeMode::Legacy, |cfg| {
                cfg.mode.into()
            });
        // Local-app generation always needs the verified internal Linux
        // runtime, even when the user-facing terminal remains in Legacy mode.
        // Keep `mobile_linux_mode` unchanged so this does not enable shell/git
        // tools or change the terminal selection.
        let mut local_apps_mobile_linux = mobile_linux.clone();
        if local_apps_runtime_requested {
            if let Some(config) = local_apps_mobile_linux.as_mut() {
                config.mode = MobileLinuxRuntimeModeFfi::MobileLinux;
            }
        }
        let android_platform = AndroidPlatform::new_with_mode(
            AndroidPlatformInputs {
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
                mobile_linux: android_mobile_linux_runtime(local_apps_mobile_linux.as_ref()),
                mobile_linux_workspace_root: mobile_linux
                    .as_ref()
                    .and_then(|cfg| cfg.workspace_host_path.clone())
                    .map(std::path::PathBuf::from),
                mobile_linux_workspace_id: mobile_linux
                    .as_ref()
                    .and_then(|cfg| cfg.stable_workspace_id.clone()),
                mobile_linux_managed_root: mobile_linux
                    .as_ref()
                    .map(|cfg| std::path::PathBuf::from(cfg.managed_root.clone())),
                shell: shell_cfg,
                secure_storage: secure_storage.map(|s| {
                    std::sync::Arc::new(AndroidSecureStorageBridge { inner: s })
                        as std::sync::Arc<dyn platform_api::SecureStorage>
                }),
                android_ui_automation: computer_use.map(|host| {
                    std::sync::Arc::new(AndroidComputerUseBridge { inner: host })
                        as std::sync::Arc<dyn platform_api::AndroidUiAutomation>
                }),
            },
            mobile_linux_mode,
        );

        // D8: run the eager capability probe and populate the SHARED cache
        // BEFORE erasing to `Arc<dyn Platform>` and assembling the (synchronous)
        // tool registry inside `build_mobile_engine`. The probe result drives
        // both `Sandbox::prepare` and the per-plan registration gates, so it MUST
        // be cached before any of those read it.
        //
        // We hold the CONCRETE `AndroidPlatform` here (the shared
        // `build_mobile_engine` takes `Arc<dyn Platform>` and cannot downcast to
        // reach `shell_capability_cache()`), so this is the only seam that owns
        // both the cache and a pre-registration moment.
        //
        // Runtime for the `block_on`: the handle-owned engine runtime is built
        // INSIDE `build_mobile_engine`, so no `tokio::runtime::Handle` exists yet
        // at this point. The probe is independent of the engine runtime, so we
        // spin up a transient current-thread runtime just for this one call and
        // drop it immediately — clean and correct (option (b) per the plan).
        if matches!(
            mobile_linux_mode,
            mobile_linux_api::MobileLinuxRuntimeMode::MobileLinux
        ) {
            if let Some(shell_cfg) = shell_cfg_for_gate.as_ref() {
                cfg.android_shell = Some(tool_api::AndroidShellToolCtx::mobile_linux_guest(
                    shell_cfg.enable_shell && shell_cfg.secrets_gate_satisfied(),
                    vec![
                        "sh".to_string(),
                        "apk".to_string(),
                        "git".to_string(),
                        "python3".to_string(),
                    ],
                    Some("Alpine BusyBox".to_string()),
                ));
            }
        }
        if let Some(cache) = android_platform.shell_capability_cache() {
            let probe_rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| {
                    MobileEngineError::Internal(format!(
                        "capability probe runtime build failed: {e}"
                    ))
                })?;
            let mut caps =
                probe_rt.block_on(platform_android::capabilities::probe_android_capabilities());

            // P5b-T7: fold the bundled-shell bootstrap result into the capabilities
            // so reporting is TRUTHFUL — the arg-less `probe_android_capabilities()`
            // cannot know the bundled paths, so `bundled_shell_exec` is false until
            // proven here. This MUST be applied BEFORE the single `cache.set()`:
            // `CapabilityCache` wraps `OnceLock` (first-write-wins), so a
            // second `set()` after the probe result was already stored would be a
            // silent no-op and the bundled facts (notably `bundled_mksh_version`,
            // which the tool prompt re-reads from the cache) would be lost.
            let bundled_ready = bundled.is_some();
            if bundled_ready {
                caps.bundled_shell_exec = true;
                caps.bundled_mksh_version = bundled.as_ref().and_then(|b| b.mksh_version.clone());
                caps.bundled_applets = platform_android::capabilities::BUNDLED_TOYBOX_APPLETS
                    .iter()
                    .map(|s| (*s).to_string())
                    .collect();
            }
            cache.set(caps);

            // P3-T5 + P5b-T7: compute the Shell-tool registration gate + prompt
            // info from the just-probed capabilities + the bundled bootstrap +
            // the `AndroidShellConfig`, and thread it onto `MobileConfig` for
            // `tool-shell-mobile::register_all` (the registration gate) + the tool
            // prompt. Absent (`None`) whenever no shell config was supplied —
            // shell support then stays fully absent. The bundled conjunct keeps
            // the gate fail-closed (§B2): no bundled bootstrap ⇒ Shell absent.
            if let Some(shell_cfg) = shell_cfg_for_gate {
                let caps = cache.get();
                cfg.android_shell = Some(tool_api::AndroidShellToolCtx::android_legacy(
                    android_shell_gate(
                        shell_cfg.enable_shell,
                        shell_cfg.secrets_gate_satisfied(),
                        caps.available(),
                        caps.seccomp_filter,
                        caps.net_deny_verified,
                        bundled_ready,
                    ),
                    if bundled_ready {
                        caps.bundled_applets.clone()
                    } else {
                        caps.toybox_applets.clone()
                    },
                    if bundled_ready {
                        caps.bundled_mksh_version.clone()
                    } else {
                        caps.system_sh_version.clone()
                    },
                    bundled_ready,
                ));
            }
        }

        // P4-T10: compute the Git-tool registration gate + thread the in-process
        // HTTPS token onto `MobileConfig`. Independent of the capability probe
        // (Git is decoupled from minijail — spec §G5: no sandbox capability in
        // its gate, no exec). Absent (`None`) whenever no git config was supplied
        // — Git support then stays fully absent. The token rides the separate
        // `android_git_secret` field (NOT the broadly-cloned public
        // `AndroidGitToolCtx`, which only exposes `has_token`), so it never
        // enters the public tool carrier.
        if let Some(c) = git {
            // Wrap the host-implemented per-op credential provider (if any) in the
            // bridge onto the shared `tool_api::GitCredentialProvider` seam. The
            // secret is fetched on demand inside libgit2's sync callback, so it is
            // never held resident in the engine between ops.
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

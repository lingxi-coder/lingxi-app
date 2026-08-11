//! `ios-framework` (M8-P12 → M10-F3) — the iOS `UniFFI` packager.
//!
//! This crate is the FFI boundary between the Rust engine and the iOS app. The
//! Swift layer implements the [`traits::CameraControl`] / [`traits::VoiceRecorder`]
//! / [`traits::SharingService`] callback interfaces (see the skeletons under
//! `swift/`), hands them across as a [`PlatformImpls`] record, and Rust uses
//! them to construct an `IosPlatform` and assemble the mobile engine — so Rust
//! drives the device's native capabilities by calling *back* into Swift. That
//! bidirectional flow is the whole point of the `UniFFI` seam.
//!
//! ## Shared session host (F3-04)
//!
//! The real session host — [`MobileEngineHandle`] (owns the handle-owned tokio
//! runtime + the wired `MobileRuntime` + the registered `ClientEventListener`)
//! and its [`MobileEngineError`] — lives in `engine-mobile` and is RE-EXPORTED
//! here, NOT re-derived. That single-source rule (plan F3-04) is what stops iOS
//! and Android from drifting: this crate only adds the iOS-specific
//! `Platform`-construction wrapper around the shared `build_mobile_engine`.
//!
//! ## Inbound command path (F3-05)
//!
//! The async FFI entry point — `MobileEngineHandle::submit(ClientCommand) ->
//! Result<(), ClientError>` (under `uniffi`: `#[uniffi::export(async_runtime =
//! "tokio")]`) — is defined ONCE on the shared host in `engine-mobile` and
//! reaches Swift through the re-exported [`MobileEngineHandle`]. There is no
//! iOS-specific submit body: `SendPrompt` spawns the streaming turn on the
//! handle-owned runtime and returns promptly (results stream via the listener);
//! `Cancel` fires the in-flight token; `ApprovePermission`/`DenyPermission`
//! resolve the parked permission gate.
//!
//! ## Async-over-FFI runtime registration (F3-07)
//!
//! The async exports (`submit`, the F3-07 inspection helpers) cross the FFI seam
//! as `UniFFI` rust-futures. The foreign async executor is NAMED explicitly by the
//! `#[uniffi::export(async_runtime = "tokio")]` attribute on the shared host's
//! `submit` impl (in `engine-mobile`), backed by the workspace `uniffi` dep's
//! `tokio` feature (pinned offline in F3-00). That scaffolding polls every async
//! export on the handle-owned `rt-multi-thread` runtime — the one
//! [`MobileEngineHandle`] owns per governing decision §0.5 (one connection ⇒ one
//! engine host owning one runtime) — so Swift's `async` calls never block the UI
//! thread and resolve on the engine's own runtime. The
//! `async_submit_resolves_on_handle_runtime` host test proves the registration by
//! asserting an async export resolves on exactly that runtime.
//!
//! ## `UniFFI` status
//! The `uniffi` feature (default-on) lights up the real `UniFFI` surface: the
//! re-exported [`MobileEngineHandle`] is a `#[derive(uniffi::Object)]`, the
//! listener a callback interface, the DTOs `UniFFI` types. `engine-mobile` carries
//! the `setup_scaffolding!()`; this crate re-exports it (and adds its own for
//! the iOS-local exports) so the symbols land in the final `staticlib`/cdylib.

#![forbid(unsafe_code)]

use std::sync::Arc;
#[cfg(feature = "uniffi")]
use std::sync::{Mutex as StdMutex, OnceLock};
#[cfg(feature = "uniffi")]
use traits::mobile_linux::MAX_MOBILE_LINUX_EVENT_BATCH;
use traits::{CameraControl, SharingService, VoiceRecorder};
// `Platform` is named only inside the `cfg(target_os = "ios")` constructor body;
// importing it unconditionally warns on the host build, so scope it to iOS.
#[cfg(all(feature = "uniffi", target_os = "ios"))]
use traits::Platform;

// F3-04: the shared session host + its error type are DEFINED ONCE in
// `engine-mobile` and re-exported here. Both FFI packager crates re-export the
// SAME types so iOS and Android cannot drift (plan F3-04). The listener +
// permission-request-sink the constructor takes are likewise re-exported from
// the shared host.
#[cfg(feature = "uniffi")]
pub use engine_mobile::{
    ClientEventListener, CronDueOccurrenceDto, CronFireStatusDto, CronTaskDto, FiredCronJobDto,
    MobileConfig, MobileCronStoreHandle, MobileEngineError, MobileEngineHandle,
    MobileOAuthSessionDto, MobileOAuthStateDto, PermissionRequestSink,
    ProviderConnectionTestDto,
};

/// The foreign (Swift) capability objects + config the engine needs to build an
/// `IosPlatform`. `UniFFI` marshals each `Arc<dyn …>` as a callback-interface
/// reference; `app_sandbox_root` is the app container path.
pub struct PlatformImpls {
    /// Swift `CameraControl` impl.
    pub camera: Arc<dyn CameraControl>,
    /// Swift `VoiceRecorder` impl.
    pub voice: Arc<dyn VoiceRecorder>,
    /// Swift `SharingService` impl.
    pub share: Arc<dyn SharingService>,
    /// Swift Keychain-backed `SecureStorage` impl, if provided. When `None` the
    /// composition root falls back to the non-persisting development stub.
    pub secure_storage: Option<Arc<dyn traits::SecureStorage>>,
    /// The app's writable sandbox container root.
    pub app_sandbox_root: String,
    /// Optional mobile-linux runtime configuration.
    pub mobile_linux: Option<IosMobileLinuxConfigFfi>,
}

/// Mobile Linux runtime mode exposed to the iOS host.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MobileLinuxRuntimeModeFfi {
    Legacy,
    MobileLinux,
}

/// FFI carrier for iOS mobile-linux configuration.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IosMobileLinuxConfigFfi {
    /// Legacy unavailable stub vs mobile-linux backend selection.
    pub mode: MobileLinuxRuntimeModeFfi,
    /// App-private root where rootfs state is managed.
    pub managed_root: String,
    /// Explicit host workspace root exposed to the guest. Must not be the app
    /// sandbox root, `.lingxi`, or any provider/config subtree.
    pub workspace_host_path: String,
    /// Stable guest workspace id used to produce `/workspace/<id>`.
    pub stable_workspace_id: String,
    /// ABI name for the rootfs payload (`arm64` on device).
    pub abi: String,
    /// Expected rootfs version label.
    pub rootfs_version: String,
    /// Expected rootfs archive sha256, if known.
    pub archive_sha256: Option<String>,
    /// Optional path to the written distribution authorization. It is accepted
    /// only when its digest matches the build-pinned
    /// `LINGXI_MOBILE_LINUX_AUTHORIZATION_SHA256`.
    pub authorization_file: Option<String>,
    /// The engine's data root — the SAME directory the engine receives as its
    /// `app_sandbox_root` and hangs `.lingxi`, `apps/`, and `Projects/` off.
    ///
    /// It must be supplied, never derived. This field replaced an inference
    /// that read the root out of `managed_root` by cutting at
    /// `Library/Application Support`: that yields the iOS *container*, while
    /// the engine's root is `<AppSupport>/LingxiCode` — three components
    /// deeper, and not reachable from `mobile-linux/ios-ish` by any rule. The
    /// runtime therefore expected local-app builds at
    /// `<container>/apps/<id>/build/<channel>`, a path nothing writes, so every
    /// build failed its mount check; and `.lingxi` protection guarded
    /// `<container>/.lingxi`, leaving the real credential directory unguarded.
    pub app_sandbox_root: String,
}

/// iOS-provided multi-provider configuration for the mobile engine.
///
/// The JSON strings contain non-secret provider/routing settings only. API
/// keys are sent separately through `SetProviderCredential` and persist in the
/// injected secure storage implementation.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct IosProviderConfigFfi {
    /// JSON object matching the shared `settings.providers` schema.
    pub provider_profiles_json: String,
    /// Optional JSON value matching the shared `settings.routing` schema.
    pub routing_json: Option<String>,
}

/// Compact launch configuration for the extended iOS engine constructor.
///
/// This additive carrier keeps project scoping, provider configuration, and
/// mobile-linux settings marshaled together without breaking the legacy flat
/// `build_ios_engine` entry point.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct IosEngineLaunchConfigFfi {
    pub api_base: String,
    pub api_key: String,
    pub model: String,
    pub app_sandbox_root: String,
    /// Optional managed Project workspace. When present it must resolve to
    /// `<app_sandbox_root>/(P|p)rojects/<lowercase UUID>/workspace`; global
    /// `.lingxi` state continues to live under `app_sandbox_root`.
    pub project_cwd: Option<String>,
    pub provider_config: Option<IosProviderConfigFfi>,
    pub mobile_linux: Option<IosMobileLinuxConfigFfi>,
    /// Compile-time distribution mode: false for Store, true for Full.
    pub local_apps_full_runtime: bool,
    /// Verified read-only local-app runtime bundle staged by the iOS build.
    pub local_apps_runtime_root: Option<String>,
    /// Device physical memory reported by the iOS host.
    pub physical_memory_bytes: u64,
}

/// FFI rootfs lifecycle state.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[derive(Debug, Clone, Copy)]
pub enum MobileLinuxRootfsStateFfi {
    Missing,
    Installing,
    Ready,
    Corrupt,
    Repairing,
    Resetting,
    Unsupported,
    BlockedByLicense,
}

/// FFI capability snapshot for the iOS mobile-linux runtime.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct MobileLinuxCapabilityFfi {
    pub available: bool,
    pub backend: String,
    pub mode: MobileLinuxRuntimeModeFfi,
    pub reason: Option<String>,
    pub streaming_output: bool,
    pub background_processes: bool,
    pub pty: bool,
    pub bind_mounts: bool,
    pub rootfs_integrity: bool,
}

/// FFI rootfs status snapshot for the iOS mobile-linux runtime.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct MobileLinuxStatusFfi {
    pub state: MobileLinuxRootfsStateFfi,
    pub backend: String,
    pub mode: MobileLinuxRuntimeModeFfi,
    pub platform: String,
    pub abi: String,
    pub version: Option<String>,
    pub managed_root: Option<String>,
    pub active_root: Option<String>,
    pub staged_root: Option<String>,
    pub archive_sha256: Option<String>,
    pub installed_size_bytes: Option<u64>,
    pub writable_guest_paths: Vec<String>,
    pub last_error: Option<String>,
}

/// Network policy for a guest command.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[derive(Debug, Clone, Copy)]
pub enum MobileLinuxNetworkPolicyFfi {
    Disabled,
    LoopbackOnly,
    Allowed,
}

/// Mount purpose surfaced to the host UI.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[derive(Debug, Clone, Copy)]
pub enum MobileLinuxMountPurposeFfi {
    Workspace,
    LocalAppBuild,
    Memory,
    Skills,
    Shared,
    External,
    Temp,
}

/// Host→guest bind mount descriptor.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct MobileLinuxMountSpecFfi {
    pub host_path: String,
    pub guest_path: String,
    pub read_only: bool,
    pub purpose: MobileLinuxMountPurposeFfi,
}

/// One-shot guest command request.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct MobileLinuxCommandRequestFfi {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub env: std::collections::HashMap<String, String>,
    pub stdin: Option<String>,
    pub timeout_ms: Option<u64>,
    pub network: MobileLinuxNetworkPolicyFfi,
    pub mounts: Vec<MobileLinuxMountSpecFfi>,
}

/// Completed command result.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct MobileLinuxCommandResultFfi {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub timed_out: bool,
    pub cancelled: bool,
}

/// PTY open request.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct MobileLinuxPtyOpenRequestFfi {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub env: std::collections::HashMap<String, String>,
    pub cols: u16,
    pub rows: u16,
    pub mounts: Vec<MobileLinuxMountSpecFfi>,
}

/// PTY session snapshot returned to the host.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct MobileLinuxPtySessionFfi {
    pub id: String,
    pub available: bool,
    pub detail: Option<String>,
}

/// Background task lifecycle state.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[derive(Debug, Clone, Copy)]
pub enum MobileLinuxTaskStateFfi {
    Running,
    Completed,
    Failed,
    Cancelled,
    Unavailable,
}

/// Background task summary for the host UI.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct MobileLinuxTaskFfi {
    pub id: String,
    pub title: String,
    pub state: MobileLinuxTaskStateFfi,
    pub detail: Option<String>,
}

/// Streaming event kind surfaced to Swift.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[derive(Debug, Clone, Copy)]
pub enum MobileLinuxStreamEventKindFfi {
    StdoutLine,
    StderrChunk,
    Exit,
    Error,
}

/// Streaming source surfaced to Swift.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[derive(Debug, Clone, Copy)]
pub enum MobileLinuxStreamSourceFfi {
    Run,
    Pty,
}

/// Streaming event emitted while a command / PTY is active.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct MobileLinuxStreamEventFfi {
    pub sequence: u64,
    pub task_id: Option<String>,
    pub stream_id: String,
    pub source: MobileLinuxStreamSourceFfi,
    pub kind: MobileLinuxStreamEventKindFfi,
    pub text: Option<String>,
    pub data: Option<Vec<u8>>,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
}

/// Host-visible mobile-linux operation failure.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Error))]
#[derive(Debug, thiserror::Error)]
pub enum MobileLinuxOperationFfiError {
    #[error("unsupported on this platform")]
    Unsupported,
    #[error("runtime unavailable: {message}")]
    Unavailable { message: String },
    #[error("license blocked: {message}")]
    LicenseBlocked { message: String },
    #[error("invalid request: {message}")]
    InvalidRequest { message: String },
    #[error("io error: {message}")]
    Io { message: String },
    #[error("timeout")]
    Timeout,
}

/// Error returned by the host event sink.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Error))]
#[derive(Debug, thiserror::Error)]
pub enum MobileLinuxEventSinkFfiError {
    #[error("event sink rejected update: {message}")]
    Rejected { message: String },
}

/// Top-level `UniFFI` constructor: build the mobile engine from the Swift-supplied
/// platform callbacks + event listener. (Under `uniffi`: `#[uniffi::export]`.)
///
/// This is a THIN wrapper: it constructs the iOS-specific `Platform` from the
/// foreign callbacks and then delegates ALL runtime/adapter/listener wiring to
/// the shared [`engine_mobile::build_mobile_engine`] (F3-04) — so the heavy
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
            cwd: std::path::PathBuf::from(&impls.app_sandbox_root),
            lingxi_home: std::path::PathBuf::from(&impls.app_sandbox_root).join(branding::DOT_DIR),
            // P0.2: production injects the real LINGXI.md hierarchy provider so the
            // orchestrator loads `<cwd>/LINGXI.md` + `<lingxi_home>/LINGXI.md` into
            // its system prompt and `fire_instructions_loaded()` fires over them.
            memory_provider: Some(orchestrator::prompt::real_provider()),
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
            voice: impls.voice,
            share: impls.share,
            stt: None,
            tts: None,
            notifications: None,
            clipboard: None,
            secure_storage: impls.secure_storage,
            // This lower-level entry point takes no location impl, like the
            // stt/tts/notification/clipboard slots above it.
            location: None,
            mobile_linux: ios_mobile_linux_runtime(impls.mobile_linux.as_ref()),
            workspace_host_path: Some(workspace_host_path),
            stable_workspace_id: Some(stable_workspace_id),
        }));
        engine_mobile::build_mobile_engine(cfg, platform, listener, permission_sink)
    }
    #[cfg(not(target_os = "ios"))]
    {
        let _ = (impls, listener, permission_sink);
        Err(MobileEngineError::PlatformUnavailable)
    }
}

#[cfg(feature = "uniffi")]
fn mobile_linux_backend_name(backend: traits::SandboxBackend) -> String {
    match backend {
        traits::SandboxBackend::LinuxNamespaces => "linux-namespaces",
        traits::SandboxBackend::LinuxFirejail => "linux-firejail",
        traits::SandboxBackend::MacOsSandboxExec => "macos-sandbox-exec",
        traits::SandboxBackend::WindowsJobObject => "windows-job-object",
        traits::SandboxBackend::AndroidMinijail => "android-minijail",
        traits::SandboxBackend::AndroidProot => "android-proot",
        traits::SandboxBackend::IosIsh => "ios-ish",
        traits::SandboxBackend::None => "none",
    }
    .to_string()
}

#[cfg(feature = "uniffi")]
fn rootfs_state_to_ffi(state: traits::RootfsState) -> MobileLinuxRootfsStateFfi {
    match state {
        traits::RootfsState::Missing => MobileLinuxRootfsStateFfi::Missing,
        traits::RootfsState::Installing => MobileLinuxRootfsStateFfi::Installing,
        traits::RootfsState::Ready => MobileLinuxRootfsStateFfi::Ready,
        traits::RootfsState::Corrupt => MobileLinuxRootfsStateFfi::Corrupt,
        traits::RootfsState::Repairing => MobileLinuxRootfsStateFfi::Repairing,
        traits::RootfsState::Resetting => MobileLinuxRootfsStateFfi::Resetting,
        traits::RootfsState::Unsupported => MobileLinuxRootfsStateFfi::Unsupported,
        traits::RootfsState::BlockedByLicense => MobileLinuxRootfsStateFfi::BlockedByLicense,
    }
}

#[cfg(feature = "uniffi")]
fn mobile_linux_authorization_verified(path: Option<&String>) -> bool {
    use sha2::{Digest, Sha256};

    let Some(expected) = option_env!("LINGXI_MOBILE_LINUX_AUTHORIZATION_SHA256") else {
        return false;
    };
    if expected.len() != 64
        || !expected
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return false;
    }
    let Some(path) = path else {
        return false;
    };
    let path = std::path::Path::new(path);
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 1024 * 1024 {
        return false;
    }
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    let actual = format!("{:x}", Sha256::digest(bytes));
    actual == expected
}

#[cfg(feature = "uniffi")]
fn default_workspace_host_path(app_sandbox_root: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(app_sandbox_root)
        .join("workspaces")
        .join("default")
}

#[cfg(feature = "uniffi")]
fn is_lowercase_uuid(value: &str) -> bool {
    if value.len() != 36 || value != value.to_ascii_lowercase() {
        return false;
    }
    value.chars().enumerate().all(|(index, character)| {
        if matches!(index, 8 | 13 | 18 | 23) {
            character == '-'
        } else {
            character.is_ascii_hexdigit()
        }
    })
}

#[cfg(feature = "uniffi")]
fn ios_project_cwd(
    app_sandbox_root: &str,
    project_cwd: Option<&str>,
) -> Result<std::path::PathBuf, MobileEngineError> {
    let app_root = std::path::Path::new(app_sandbox_root)
        .canonicalize()
        .map_err(|error| {
            MobileEngineError::Internal(format!("iOS app sandbox root is unavailable: {error}"))
        })?;
    let Some(project_cwd) = project_cwd else {
        return Ok(app_root);
    };
    let workspace = std::path::Path::new(project_cwd)
        .canonicalize()
        .map_err(|error| {
            MobileEngineError::Internal(format!("iOS Project workspace is unavailable: {error}"))
        })?;
    let relative = workspace.strip_prefix(&app_root).map_err(|_| {
        MobileEngineError::Internal(
            "iOS Project workspace must remain inside the app sandbox".to_string(),
        )
    })?;
    let components: Vec<_> = relative
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .collect();
    let is_managed_project = components.len() == 3
        && matches!(components[0], "Projects" | "projects")
        && is_lowercase_uuid(components[1])
        && components[2] == "workspace";
    // v3 local apps: an app's conversation scope roots at
    // `appSandboxRoot/apps/<id>/workspace` — the same shape Swift's
    // `LocalAppWorkspacePath` derives, with the id legality delegated to the
    // engine's own minting rule instead of a twin regex.
    let is_local_app_workspace = components.len() == 3
        && components[0] == "apps"
        && local_apps::ids::is_valid_app_id(components[1])
        && components[2] == "workspace";
    let valid = (is_managed_project || is_local_app_workspace) && workspace.is_dir();
    if !valid {
        return Err(MobileEngineError::Internal(
            "iOS conversation workspace must match \
             appSandboxRoot/Projects/<lowercase UUID>/workspace or \
             appSandboxRoot/apps/<app id>/workspace"
                .to_string(),
        ));
    }
    Ok(workspace)
}

#[cfg(feature = "uniffi")]
fn ios_mobile_config_from_launch_config(
    config: &IosEngineLaunchConfigFfi,
) -> Result<MobileConfig, MobileEngineError> {
    let cwd = ios_project_cwd(&config.app_sandbox_root, config.project_cwd.as_deref())?;
    let mut cfg = MobileConfig {
        cwd,
        lingxi_home: std::path::PathBuf::from(&config.app_sandbox_root).join(branding::DOT_DIR),
        local_apps_full_runtime: config.local_apps_full_runtime,
        local_apps_runtime_root: config
            .local_apps_runtime_root
            .as_ref()
            .map(std::path::PathBuf::from),
        physical_memory_bytes: config.physical_memory_bytes,
        // P0.2: production injects the real LINGXI.md hierarchy provider so the
        // orchestrator loads `<cwd>/LINGXI.md` + `<lingxi_home>/LINGXI.md` into
        // its system prompt and `fire_instructions_loaded()` fires over them.
        memory_provider: Some(orchestrator::prompt::real_provider()),
        ..MobileConfig::default()
    };
    if !config.api_base.is_empty() {
        cfg.api_base = config.api_base.clone();
    }
    cfg.api_key = config.api_key.clone();
    if !config.model.is_empty() {
        cfg.default_model = config.model.clone();
    }
    if let Some(provider_config) = &config.provider_config {
        let (profiles, routing) = engine_mobile::parse_mobile_provider_config_json(
            &provider_config.provider_profiles_json,
            provider_config.routing_json.as_deref(),
        )?;
        cfg.provider_profiles = profiles;
        cfg.routing = routing;
    }
    Ok(cfg)
}

#[cfg(feature = "uniffi")]
fn validate_mobile_linux_workspace_config(
    app_sandbox_root: &str,
    config: &IosMobileLinuxConfigFfi,
) -> Result<(std::path::PathBuf, String), MobileLinuxOperationFfiError> {
    let workspace_host_path = if config.workspace_host_path.trim().is_empty() {
        default_workspace_host_path(app_sandbox_root)
    } else {
        std::path::PathBuf::from(&config.workspace_host_path)
    };
    let workspace_host_path =
        resolve_mobile_linux_security_path(&workspace_host_path, "workspace_host_path")?;
    let sandbox_root = resolve_mobile_linux_security_path(
        std::path::Path::new(app_sandbox_root),
        "app_sandbox_root",
    )?;
    let managed_root = resolve_mobile_linux_security_path(
        std::path::Path::new(&config.managed_root),
        "managed_root",
    )?;
    let lingxi_root =
        resolve_mobile_linux_security_path(&sandbox_root.join(branding::DOT_DIR), ".lingxi root")?;

    if workspace_host_path == sandbox_root
        || workspace_host_path.starts_with(&managed_root)
        || managed_root.starts_with(&workspace_host_path)
        || workspace_host_path.starts_with(&lingxi_root)
        || lingxi_root.starts_with(&workspace_host_path)
    {
        return Err(MobileLinuxOperationFfiError::InvalidRequest {
            message: "workspace_host_path may not target app root, managed_root, or .lingxi"
                .to_string(),
        });
    }

    if mobile_linux_path_contains_protected_config_subtree(&workspace_host_path) {
        return Err(MobileLinuxOperationFfiError::InvalidRequest {
            message: "workspace_host_path may not target provider/config subtrees".to_string(),
        });
    }

    let stable_workspace_id = if config.stable_workspace_id.trim().is_empty() {
        "default".to_string()
    } else {
        config.stable_workspace_id.clone()
    };
    if stable_workspace_id.is_empty()
        || !stable_workspace_id
            .chars()
            .all(|char| char.is_ascii_alphanumeric() || matches!(char, '-' | '_'))
    {
        return Err(MobileLinuxOperationFfiError::InvalidRequest {
            message: "stable_workspace_id contains unsupported characters".to_string(),
        });
    }

    Ok((workspace_host_path, stable_workspace_id))
}

#[cfg(feature = "uniffi")]
fn resolve_mobile_linux_security_path(
    path: &std::path::Path,
    field: &str,
) -> Result<std::path::PathBuf, MobileLinuxOperationFfiError> {
    use std::path::Component;

    if !path.is_absolute() {
        return Err(MobileLinuxOperationFfiError::InvalidRequest {
            message: format!("{field} must be absolute"),
        });
    }

    let mut normalized = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                return Err(MobileLinuxOperationFfiError::InvalidRequest {
                    message: format!("{field} may not contain parent traversal"),
                });
            }
            Component::CurDir => {}
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }

    // `canonicalize` requires the final path to exist, but first launch often
    // validates a workspace before creating it. Resolve the deepest existing
    // ancestor so any symlink already present in the path is still collapsed,
    // then append the missing suffix without reintroducing `..` components.
    let mut existing = normalized.as_path();
    let mut missing = Vec::new();
    loop {
        match std::fs::canonicalize(existing) {
            Ok(mut resolved) => {
                for component in missing.iter().rev() {
                    resolved.push(component);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let Some(name) = existing.file_name() else {
                    return Err(MobileLinuxOperationFfiError::InvalidRequest {
                        message: format!("{field} has no resolvable ancestor"),
                    });
                };
                missing.push(name.to_os_string());
                let Some(parent) = existing.parent() else {
                    return Err(MobileLinuxOperationFfiError::InvalidRequest {
                        message: format!("{field} has no resolvable ancestor"),
                    });
                };
                existing = parent;
            }
            Err(error) => {
                return Err(MobileLinuxOperationFfiError::InvalidRequest {
                    message: format!("{field} cannot be resolved safely: {error}"),
                });
            }
        }
    }
}

#[cfg(feature = "uniffi")]
fn mobile_linux_path_contains_protected_config_subtree(path: &std::path::Path) -> bool {
    let components: Vec<_> = path
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => {
                Some(value.to_string_lossy().to_ascii_lowercase())
            }
            _ => None,
        })
        .collect();
    components
        .iter()
        .any(|component| matches!(component.as_str(), ".lingxi" | "provider" | "providers"))
        || components
            .windows(2)
            .any(|pair| pair[0] == "library" && pair[1] == "preferences")
}

/// The engine data root the runtime enforces its mount rules against.
///
/// Taken verbatim from the caller. The previous version DERIVED it from
/// `managed_root` — see [`IosMobileLinuxConfigFfi::app_sandbox_root`] for what
/// that cost. No derivation can work here: `managed_root` is
/// `<AppSupport>/mobile-linux/ios-ish` and the engine root is
/// `<AppSupport>/LingxiCode`, which share only `<AppSupport>`. The missing
/// component is the app's own name, known to the caller and to nobody else.
///
/// Empty is rejected rather than defaulted. A default here would be a second
/// authority on the same directory, which is precisely how the runtime and the
/// engine came to disagree in the first place.
#[cfg(feature = "uniffi")]
fn resolve_mobile_linux_app_sandbox_root(
    config: &IosMobileLinuxConfigFfi,
) -> Result<std::path::PathBuf, MobileLinuxOperationFfiError> {
    if config.app_sandbox_root.trim().is_empty() {
        return Err(MobileLinuxOperationFfiError::InvalidRequest {
            message: "app_sandbox_root is required and must be the engine's data root".to_string(),
        });
    }
    resolve_mobile_linux_security_path(
        std::path::Path::new(&config.app_sandbox_root),
        "app_sandbox_root",
    )
}

#[cfg(feature = "uniffi")]
fn capability_to_ffi(capability: traits::MobileLinuxCapability) -> MobileLinuxCapabilityFfi {
    MobileLinuxCapabilityFfi {
        available: capability.available,
        backend: mobile_linux_backend_name(capability.backend),
        mode: match capability.mode {
            traits::MobileLinuxRuntimeMode::Legacy => MobileLinuxRuntimeModeFfi::Legacy,
            traits::MobileLinuxRuntimeMode::MobileLinux => MobileLinuxRuntimeModeFfi::MobileLinux,
        },
        reason: capability.reason,
        streaming_output: capability.streaming_output,
        background_processes: capability.background_processes,
        pty: capability.pty,
        bind_mounts: capability.bind_mounts,
        rootfs_integrity: capability.rootfs_integrity,
    }
}

#[cfg(feature = "uniffi")]
fn status_to_ffi(status: traits::RootfsStatus) -> MobileLinuxStatusFfi {
    MobileLinuxStatusFfi {
        state: rootfs_state_to_ffi(status.state),
        backend: mobile_linux_backend_name(status.backend),
        mode: match status.mode {
            traits::MobileLinuxRuntimeMode::Legacy => MobileLinuxRuntimeModeFfi::Legacy,
            traits::MobileLinuxRuntimeMode::MobileLinux => MobileLinuxRuntimeModeFfi::MobileLinux,
        },
        platform: status.platform,
        abi: status.abi,
        version: status.version,
        managed_root: status
            .managed_root
            .map(|p| p.to_string_lossy().into_owned()),
        active_root: status.active_root.map(|p| p.to_string_lossy().into_owned()),
        staged_root: status.staged_root.map(|p| p.to_string_lossy().into_owned()),
        archive_sha256: status.archive_sha256,
        installed_size_bytes: status.installed_size_bytes,
        writable_guest_paths: status.writable_guest_paths,
        last_error: status.last_error,
    }
}

#[cfg(feature = "uniffi")]
fn ios_mobile_linux_status_from_config(
    config: Option<&IosMobileLinuxConfigFfi>,
) -> MobileLinuxStatusFfi {
    match config {
        None => MobileLinuxStatusFfi {
            state: MobileLinuxRootfsStateFfi::Unsupported,
            backend: "ios-posix".to_string(),
            mode: MobileLinuxRuntimeModeFfi::Legacy,
            platform: "ios".to_string(),
            abi: "unknown".to_string(),
            version: None,
            managed_root: None,
            active_root: None,
            staged_root: None,
            archive_sha256: None,
            installed_size_bytes: None,
            writable_guest_paths: vec![],
            last_error: Some("legacy unavailable backend selected".to_string()),
        },
        Some(cfg) if matches!(cfg.mode, MobileLinuxRuntimeModeFfi::Legacy) => {
            MobileLinuxStatusFfi {
                state: MobileLinuxRootfsStateFfi::Unsupported,
                backend: "ios-posix".to_string(),
                mode: MobileLinuxRuntimeModeFfi::Legacy,
                platform: "ios".to_string(),
                abi: cfg.abi.clone(),
                version: Some(cfg.rootfs_version.clone()),
                managed_root: Some(cfg.managed_root.clone()),
                active_root: None,
                staged_root: None,
                archive_sha256: cfg.archive_sha256.clone(),
                installed_size_bytes: None,
                writable_guest_paths: vec![
                    "/root".to_string(),
                    "/tmp".to_string(),
                    "/var/tmp".to_string(),
                    "/workspace".to_string(),
                ],
                last_error: Some("legacy unavailable backend selected".to_string()),
            }
        }
        Some(cfg) => match ios_mobile_linux_runtime(Some(cfg)) {
            Some(runtime) => {
                let probe_rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("ios mobile-linux status runtime");
                match probe_rt.block_on(runtime.rootfs_status()) {
                    Ok(status) => status_to_ffi(status),
                    Err(error) => fallback_ios_mobile_linux_status(cfg, Some(error.to_string())),
                }
            }
            None => fallback_ios_mobile_linux_status(cfg, None),
        },
    }
}

#[cfg(feature = "uniffi")]
fn fallback_ios_mobile_linux_status(
    cfg: &IosMobileLinuxConfigFfi,
    override_error: Option<String>,
) -> MobileLinuxStatusFfi {
    let auth_present = mobile_linux_authorization_verified(cfg.authorization_file.as_ref());
    let (state, message) = if auth_present {
        (
            MobileLinuxRootfsStateFfi::Unsupported,
            "authorization present, but iSH runtime is not linked in this build",
        )
    } else {
        (
            MobileLinuxRootfsStateFfi::BlockedByLicense,
            "missing additional written authorization for PRoot/iSH redistribution",
        )
    };
    MobileLinuxStatusFfi {
        state,
        backend: "ios-ish".to_string(),
        mode: MobileLinuxRuntimeModeFfi::MobileLinux,
        platform: "ios".to_string(),
        abi: cfg.abi.clone(),
        version: Some(cfg.rootfs_version.clone()),
        managed_root: Some(cfg.managed_root.clone()),
        active_root: None,
        staged_root: None,
        archive_sha256: cfg.archive_sha256.clone(),
        installed_size_bytes: None,
        writable_guest_paths: vec![
            "/root".to_string(),
            "/tmp".to_string(),
            "/var/tmp".to_string(),
            "/workspace".to_string(),
        ],
        last_error: Some(override_error.unwrap_or_else(|| message.to_string())),
    }
}

#[cfg(feature = "uniffi")]
fn ios_mobile_linux_runtime(
    config: Option<&IosMobileLinuxConfigFfi>,
) -> Option<Arc<dyn traits::MobileLinuxRuntime>> {
    let cfg = config?;
    if matches!(cfg.mode, MobileLinuxRuntimeModeFfi::Legacy) {
        return None;
    }
    if let Some(runtime) = linked_ios_mobile_linux_runtime(cfg) {
        return Some(runtime);
    }
    let auth_present = mobile_linux_authorization_verified(cfg.authorization_file.as_ref());
    let runtime = if auth_present {
        traits::UnavailableMobileLinuxRuntime::unavailable(
            traits::SandboxBackend::IosIsh,
            traits::MobileLinuxRuntimeMode::MobileLinux,
            "ios",
            cfg.abi.clone(),
            "authorization present, but iSH runtime is not linked in this build",
        )
    } else {
        traits::UnavailableMobileLinuxRuntime::blocked(
            traits::SandboxBackend::IosIsh,
            traits::MobileLinuxRuntimeMode::MobileLinux,
            "ios",
            cfg.abi.clone(),
            "missing additional written authorization for PRoot/iSH redistribution",
        )
    };
    Some(Arc::new(runtime) as Arc<dyn traits::MobileLinuxRuntime>)
}

#[cfg(feature = "uniffi")]
fn linked_ios_mobile_linux_runtime(
    cfg: &IosMobileLinuxConfigFfi,
) -> Option<Arc<dyn traits::MobileLinuxRuntime>> {
    let app_sandbox_root = resolve_mobile_linux_app_sandbox_root(cfg).ok()?;
    let (workspace_host_path, stable_workspace_id) =
        validate_mobile_linux_workspace_config(app_sandbox_root.to_string_lossy().as_ref(), cfg)
            .ok()?;
    Some(platform_ios_ish_runtime::linked_runtime(
        platform_ios_ish_runtime::IosIshRuntimeConfig {
            managed_root: std::path::PathBuf::from(&cfg.managed_root),
            app_sandbox_root,
            workspace_host_path,
            stable_workspace_id,
            abi: cfg.abi.clone(),
            rootfs_version: cfg.rootfs_version.clone(),
            archive_sha256: cfg.archive_sha256.clone(),
            authorization_file: cfg.authorization_file.clone(),
        },
    ))
}

#[cfg(feature = "uniffi")]
fn mount_purpose_from_ffi(value: MobileLinuxMountPurposeFfi) -> traits::MountPurpose {
    match value {
        MobileLinuxMountPurposeFfi::Workspace => traits::MountPurpose::Workspace,
        MobileLinuxMountPurposeFfi::LocalAppBuild => traits::MountPurpose::LocalAppBuild,
        MobileLinuxMountPurposeFfi::Memory => traits::MountPurpose::Memory,
        MobileLinuxMountPurposeFfi::Skills => traits::MountPurpose::Skills,
        MobileLinuxMountPurposeFfi::Shared => traits::MountPurpose::Shared,
        MobileLinuxMountPurposeFfi::External => traits::MountPurpose::External,
        MobileLinuxMountPurposeFfi::Temp => traits::MountPurpose::Temp,
    }
}

#[cfg(feature = "uniffi")]
fn network_policy_from_ffi(value: MobileLinuxNetworkPolicyFfi) -> traits::NetworkPolicy {
    match value {
        MobileLinuxNetworkPolicyFfi::Disabled => traits::NetworkPolicy::Disabled,
        MobileLinuxNetworkPolicyFfi::LoopbackOnly => traits::NetworkPolicy::LoopbackOnly,
        MobileLinuxNetworkPolicyFfi::Allowed => traits::NetworkPolicy::Allowed,
    }
}

#[cfg(feature = "uniffi")]
fn mount_spec_from_ffi(value: MobileLinuxMountSpecFfi) -> traits::MountSpec {
    traits::MountSpec {
        host_path: std::path::PathBuf::from(value.host_path),
        guest_path: value.guest_path,
        read_only: value.read_only,
        purpose: mount_purpose_from_ffi(value.purpose),
    }
}

#[cfg(feature = "uniffi")]
fn command_request_from_ffi(value: MobileLinuxCommandRequestFfi) -> traits::LinuxCommandRequest {
    traits::LinuxCommandRequest {
        command: value.command,
        args: value.args,
        cwd: value.cwd,
        env: value.env.into_iter().collect(),
        stdin: value.stdin,
        timeout_ms: value.timeout_ms,
        network: network_policy_from_ffi(value.network),
        resource_limits: traits::ResourceLimits::default(),
        mounts: value.mounts.into_iter().map(mount_spec_from_ffi).collect(),
    }
}

#[cfg(feature = "uniffi")]
fn pty_request_from_ffi(value: MobileLinuxPtyOpenRequestFfi) -> traits::PtyOpenRequest {
    traits::PtyOpenRequest {
        command: value.command,
        args: value.args,
        cwd: value.cwd,
        env: value.env.into_iter().collect(),
        size: traits::PtySize {
            cols: value.cols,
            rows: value.rows,
        },
        mounts: value.mounts.into_iter().map(mount_spec_from_ffi).collect(),
    }
}

#[cfg(feature = "uniffi")]
fn command_result_to_ffi(value: traits::LinuxCommandResult) -> MobileLinuxCommandResultFfi {
    MobileLinuxCommandResultFfi {
        stdout: value.stdout,
        stderr: value.stderr,
        exit_code: value.exit_code,
        timed_out: value.timed_out,
        cancelled: value.cancelled,
    }
}

#[cfg(feature = "uniffi")]
fn task_snapshot_to_ffi(value: traits::MobileLinuxTaskSnapshot) -> MobileLinuxTaskFfi {
    MobileLinuxTaskFfi {
        id: value.task_id,
        title: value.command,
        state: match value.status {
            traits::MobileLinuxTaskStatus::Queued
            | traits::MobileLinuxTaskStatus::Running
            | traits::MobileLinuxTaskStatus::Backgrounded => MobileLinuxTaskStateFfi::Running,
            traits::MobileLinuxTaskStatus::Completed => MobileLinuxTaskStateFfi::Completed,
            traits::MobileLinuxTaskStatus::Failed | traits::MobileLinuxTaskStatus::TimedOut => {
                MobileLinuxTaskStateFfi::Failed
            }
            traits::MobileLinuxTaskStatus::Cancelled => MobileLinuxTaskStateFfi::Cancelled,
        },
        detail: value.detail,
    }
}

#[cfg(feature = "uniffi")]
/// `None` for events that have no stream representation and must be SKIPPED
/// (not surfaced as a bogus stream event).
fn event_to_ffi(value: traits::MobileLinuxEvent) -> Option<MobileLinuxStreamEventFfi> {
    Some(match value.kind {
        traits::MobileLinuxEventKind::TaskStatusChanged {
            status,
            exit_code,
            detail,
        } => {
            // ONLY terminal statuses read as an exit. This arm used to map
            // EVERY status change to `kind: Exit` — including the `Running`
            // emitted by task CREATION. The PTY task's id IS the session id,
            // so merely opening a terminal planted `{kind: exit, stream:
            // <session>, code: nil}` at the head of the event log; the
            // terminal's first read closed a perfectly healthy shell with
            // "[process exited]" and stopped polling, and every restart died
            // the same way at its own creation event.
            if matches!(
                status,
                traits::MobileLinuxTaskStatus::Queued
                    | traits::MobileLinuxTaskStatus::Running
                    | traits::MobileLinuxTaskStatus::Backgrounded
            ) {
                return None;
            }
            MobileLinuxStreamEventFfi {
                sequence: value.sequence,
                task_id: value.task_id.clone(),
                stream_id: value
                    .task_id
                    .clone()
                    .unwrap_or_else(|| "runtime".to_string()),
                source: MobileLinuxStreamSourceFfi::Run,
                kind: MobileLinuxStreamEventKindFfi::Exit,
                text: detail,
                data: None,
                exit_code,
                timed_out: matches!(status, traits::MobileLinuxTaskStatus::TimedOut),
            }
        }
        traits::MobileLinuxEventKind::StdoutLine { line } => MobileLinuxStreamEventFfi {
            sequence: value.sequence,
            task_id: value.task_id.clone(),
            stream_id: value
                .task_id
                .clone()
                .unwrap_or_else(|| "runtime".to_string()),
            source: MobileLinuxStreamSourceFfi::Run,
            kind: MobileLinuxStreamEventKindFfi::StdoutLine,
            text: Some(line),
            data: None,
            exit_code: None,
            timed_out: false,
        },
        traits::MobileLinuxEventKind::StderrChunk { chunk } => MobileLinuxStreamEventFfi {
            sequence: value.sequence,
            task_id: value.task_id.clone(),
            stream_id: value
                .task_id
                .clone()
                .unwrap_or_else(|| "runtime".to_string()),
            source: MobileLinuxStreamSourceFfi::Run,
            kind: MobileLinuxStreamEventKindFfi::StderrChunk,
            text: Some(String::from_utf8_lossy(&chunk).into_owned()),
            data: Some(chunk),
            exit_code: None,
            timed_out: false,
        },
        traits::MobileLinuxEventKind::PtyOutput { session_id, data } => MobileLinuxStreamEventFfi {
            sequence: value.sequence,
            task_id: value.task_id,
            stream_id: session_id,
            source: MobileLinuxStreamSourceFfi::Pty,
            kind: MobileLinuxStreamEventKindFfi::StdoutLine,
            text: Some(String::from_utf8_lossy(&data).into_owned()),
            data: Some(data),
            exit_code: None,
            timed_out: false,
        },
        traits::MobileLinuxEventKind::PtyClosed {
            session_id,
            exit_code,
            detail,
        } => MobileLinuxStreamEventFfi {
            sequence: value.sequence,
            task_id: value.task_id,
            stream_id: session_id,
            source: MobileLinuxStreamSourceFfi::Pty,
            kind: MobileLinuxStreamEventKindFfi::Exit,
            text: detail,
            data: None,
            exit_code,
            timed_out: false,
        },
        traits::MobileLinuxEventKind::RuntimeError { detail } => MobileLinuxStreamEventFfi {
            sequence: value.sequence,
            task_id: value.task_id,
            stream_id: "runtime".to_string(),
            source: MobileLinuxStreamSourceFfi::Run,
            kind: MobileLinuxStreamEventKindFfi::Error,
            text: Some(detail),
            data: None,
            exit_code: None,
            timed_out: false,
        },
    })
}

#[cfg(feature = "uniffi")]
fn mobile_linux_error_to_ffi(error: traits::MobileLinuxError) -> MobileLinuxOperationFfiError {
    match error {
        traits::MobileLinuxError::Unsupported => MobileLinuxOperationFfiError::Unsupported,
        traits::MobileLinuxError::Unavailable(message) => {
            MobileLinuxOperationFfiError::Unavailable { message }
        }
        traits::MobileLinuxError::LicenseBlocked(message) => {
            MobileLinuxOperationFfiError::LicenseBlocked { message }
        }
        traits::MobileLinuxError::Integrity(message)
        | traits::MobileLinuxError::InvalidRequest(message) => {
            MobileLinuxOperationFfiError::InvalidRequest { message }
        }
        traits::MobileLinuxError::Io(message) => MobileLinuxOperationFfiError::Io { message },
        traits::MobileLinuxError::NetworkPolicyUnavailable(message) => {
            MobileLinuxOperationFfiError::Io {
                message: format!("network_policy_unavailable: {message}"),
            }
        }
        traits::MobileLinuxError::ResourceLimitExceeded(message) => {
            MobileLinuxOperationFfiError::Io {
                message: format!("resource_limit_exceeded: {message}"),
            }
        }
        traits::MobileLinuxError::Timeout => MobileLinuxOperationFfiError::Timeout,
    }
}

#[cfg(feature = "uniffi")]
async fn unavailable_runtime_error(
    runtime: Arc<dyn traits::MobileLinuxRuntime>,
) -> MobileLinuxOperationFfiError {
    match runtime.rootfs_status().await {
        Ok(status) if matches!(status.state, traits::RootfsState::BlockedByLicense) => {
            MobileLinuxOperationFfiError::LicenseBlocked {
                message: status
                    .last_error
                    .unwrap_or_else(|| "mobile-linux runtime blocked by license gate".to_string()),
            }
        }
        Ok(status) => MobileLinuxOperationFfiError::Unavailable {
            message: status
                .last_error
                .unwrap_or_else(|| "mobile-linux runtime unavailable".to_string()),
        },
        Err(error) => mobile_linux_error_to_ffi(error),
    }
}

#[cfg(feature = "uniffi")]
fn probe_runtime(
    config: Option<&IosMobileLinuxConfigFfi>,
) -> Option<Arc<dyn traits::MobileLinuxRuntime>> {
    ios_mobile_linux_runtime(config)
}

// ---------------------------------------------------------------------------
// M10-P3a: the foreign-callable engine constructor.
// ---------------------------------------------------------------------------
//
// `build_mobile_engine` (above) takes an `Arc<dyn Platform>` + the foreign
// callback objects (camera / voice / share) — none of which are UniFFI types —
// so it cannot itself cross the FFI boundary. The Swift app needs SOME exported
// constructor to obtain a `MobileEngineHandle`; the only piece it must supply
// for a text conversation is the `ClientEventListener` (already a UniFFI
// callback interface) — the engine's own `IosPlatform` supplies fs / http /
// clock from `platform-posix-minimal`, and a text turn never touches the
// camera / voice / share device capabilities.
//
// So this thin `#[uniffi::export]` wrapper takes ONLY UniFFI-marshalable inputs
// (the listener + plain config strings), constructs default device-capability
// stubs + a no-op permission sink on the Rust side, threads the runtime config
// (api base / key / model) into a `MobileConfig`, and delegates to the shared
// `build_mobile_engine`. This is ADDITIVE FFI packaging only — it changes no
// engine semantics and touches neither the `traits` crate nor Android.
//
// SECRETS: `api_key` arrives as a parameter the Swift side reads from the
// process environment (`ANTHROPIC_API_KEY`) / an app setting at runtime; it is
// NEVER hardcoded, logged, or persisted here. An empty key is valid — the
// orchestrator only fails at `run_turn` with a 401 (mirrors `MobileConfig`).

/// Device-capability stubs used when the foreign host does not (yet) wire the
/// camera / voice / share callbacks. A text conversation never invokes these;
/// each method returns the trait's "unavailable" error so an accidental call is
/// a clean error rather than a panic. M9 replaces these with the real
/// Swift-backed callback objects threaded through a richer constructor.
///
/// Constructed only on the device/simulator (`target_os = "ios"`) path of
/// [`build_ios_engine`]; on the host bindgen build that path is `cfg`'d out, so
/// the stubs are dead there — `allow(dead_code)` keeps the host build warning-clean.
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
mod stub_capabilities {
    use async_trait::async_trait;
    use traits::{
        CameraControl, CameraError, CapturePhotoOpts, CapturedImage, ShareError, SharePayload,
        ShareResult, SharingService, VoiceError, VoiceRecorder, VoiceRecording, VoiceRecordingOpts,
    };

    /// No-op camera: capture / pick both report the hardware as unavailable.
    pub struct StubCamera;

    #[async_trait]
    impl CameraControl for StubCamera {
        async fn capture_photo(
            &self,
            _opts: CapturePhotoOpts,
        ) -> Result<CapturedImage, CameraError> {
            Err(CameraError::DeviceUnavailable)
        }
        async fn pick_from_library(&self) -> Result<CapturedImage, CameraError> {
            Err(CameraError::DeviceUnavailable)
        }
    }

    /// No-op voice recorder: never records.
    pub struct StubVoice;

    #[async_trait]
    impl VoiceRecorder for StubVoice {
        async fn start_recording(&self, _opts: VoiceRecordingOpts) -> Result<(), VoiceError> {
            Err(VoiceError::Other("voice capture not wired".to_string()))
        }
        async fn stop_recording(&self) -> Result<VoiceRecording, VoiceError> {
            Err(VoiceError::NotRecording)
        }
        async fn is_recording(&self) -> bool {
            false
        }
    }

    /// No-op share service: reports sharing unsupported.
    pub struct StubShare;

    #[async_trait]
    impl SharingService for StubShare {
        async fn share(&self, _payload: SharePayload) -> Result<ShareResult, ShareError> {
            Err(ShareError::Unsupported)
        }
    }
}

/// A [`PermissionRequestSink`] that drops outbound permission requests. Mobile
/// always binds the adapter permission gate; with no foreign permission UI yet,
/// a request that is never answered simply parks the turn (the conversation can
/// still cancel it). Lighting up a real permission dialog is additive: a future
/// constructor will accept a foreign `PermissionRequestSink` callback interface.
///
/// Constructed only on the `target_os = "ios"` path; `allow(dead_code)` on the
/// host bindgen build (where that path is `cfg`'d out).
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
struct NoopPermissionSink;

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl engine_mobile::PermissionRequestSink for NoopPermissionSink {
    async fn emit_request(&self, _request: client_protocol::permission::PermissionRequest) {}
}

// The foreign permission sink (`IosPermissionSink`) is a callback interface
// DEFINED IN THIS CRATE — mirroring `IosEventListener` — so its UniFFI
// `FfiConverter` lands under `ios_framework`'s tag, a prerequisite for naming it
// as a parameter type in `build_ios_engine`. Where the listener carries OUTBOUND
// events, this carries the engine's OUTBOUND permission requests to the Swift
// host's prompt UI; the inbound resolution flows back through
// `MobileEngineHandle::submit(ClientCommand::Approve/DenyPermission)`.
// `IosPermissionSinkBridge` adapts this crate-local interface to the shared
// `engine_mobile::PermissionRequestSink` the engine's adapter gate emits onto.
/// The Swift-implemented permission sink the iOS app registers when it builds the
/// engine. Defined in this crate (not re-used from `engine-mobile`) so its UniFFI
/// converter registers under `ios_framework`'s tag — see [`build_ios_engine`].
/// The host presents a prompt for each request and resolves it by submitting
/// `ClientCommand::ApprovePermission` / `DenyPermission` back through the handle.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait IosPermissionSink: Send + Sync {
    /// Deliver one outbound [`client_protocol::permission::PermissionRequest`] to
    /// the Swift host. Implementations enqueue a prompt and return promptly —
    /// they must not block the engine turn loop; the user's answer comes back via
    /// `MobileEngineHandle::submit`.
    async fn on_request(&self, request: client_protocol::permission::PermissionRequest);
}

/// Adapts the crate-local [`IosPermissionSink`] callback interface to the shared
/// [`PermissionRequestSink`] the engine's adapter gate emits onto. One forwarding
/// hop per request; no transformation. Mirrors [`IosListenerBridge`].
///
/// Constructed only on the `target_os = "ios"` path of [`build_ios_engine`];
/// `allow(dead_code)` on the host bindgen build (where that path is `cfg`'d out).
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
struct IosPermissionSinkBridge {
    inner: Box<dyn IosPermissionSink>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl engine_mobile::PermissionRequestSink for IosPermissionSinkBridge {
    async fn emit_request(&self, request: client_protocol::permission::PermissionRequest) {
        self.inner.on_request(request).await;
    }
}

// The foreign event listener (`IosEventListener`) is a callback interface
// DEFINED IN THIS CRATE so its UniFFI `FfiConverterArc` lands under
// `ios_framework`'s tag — a prerequisite for naming it in a `#[uniffi::export]`
// function here. (The shared `client_adapter::ClientEventListener` registers its
// converter under `client_adapter`'s tag, so it cannot be a parameter type in
// an export from a different crate.) `IosListenerBridge` adapts this crate-local
// interface to the shared `ClientEventListener` the engine actually feeds.
/// The Swift-implemented event listener the iOS app registers when it builds the
/// engine. Defined in this crate (not re-used from `client-adapter`) so its
/// UniFFI converter registers under `ios_framework`'s tag — see [`build_ios_engine`].
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait IosEventListener: Send + Sync {
    /// Deliver one fully-lowered [`client_protocol::events::ClientEvent`] to the
    /// Swift host. Implementations enqueue onto the UI's event stream and return
    /// promptly — they must not block the engine turn loop.
    async fn on_event(&self, event: client_protocol::events::ClientEvent);
}

/// Adapts the crate-local [`IosEventListener`] callback interface to the shared
/// [`ClientEventListener`] the engine's adapter sink expects. One forwarding hop
/// per event; no transformation. (`UniFFI` lifts a `callback_interface` as a
/// `Box<dyn …>`, so the bridge owns the boxed foreign object directly.)
#[cfg(feature = "uniffi")]
struct IosListenerBridge {
    inner: Box<dyn IosEventListener>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl ClientEventListener for IosListenerBridge {
    async fn on_event(&self, event: client_protocol::events::ClientEvent) {
        self.inner.on_event(event).await;
    }
}

// ---------------------------------------------------------------------------
// Device-capability FFI block (iOS parity with android-aar).
// ---------------------------------------------------------------------------
//
// Mirrors `android-aar`'s AndroidStt/AndroidTts/AndroidCamera/AndroidShare/
// AndroidVoice/AndroidNotification/AndroidClipboard callback interfaces + their
// FFI types + engine bridges, s/Android/Ios/ for the interface/bridge names.
// These interfaces are DEFINED IN THIS CRATE (mirroring `IosEventListener`) so
// their UniFFI `FfiConverter`s register under `ios_framework`'s tag — a
// prerequisite for naming them as parameter types in `build_ios_engine`. The
// engine consumes the SHARED `traits::*` seams, so each crate-local interface is
// adapted by a thin bridge struct to its `traits` counterpart.
//
// RETURN SHAPE (UniFFI 0.28.3): async callback-interface methods return
// `Result<T, E>` where `E` is a `#[derive(uniffi::Error)]` enum.

/// FFI error surface for the iOS speech callback interfaces. A flat enum so
/// `UniFFI` can render it for an async `callback_interface` method; the bridge
/// fans it back out onto the richer `traits::SttError` / `traits::TtsError`.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Error))]
#[derive(Debug, thiserror::Error)]
pub enum SpeechFfiError {
    /// The user denied microphone permission (STT only).
    #[error("microphone permission denied")]
    PermissionDenied,
    /// No speech detected before the listen timeout (STT only).
    #[error("no speech detected")]
    NoSpeech,
    /// No usable recognizer / synthesizer on the device.
    #[error("speech service unavailable")]
    Unavailable,
    /// The shared `AVAudioSession` is held by another consumer, exactly as
    /// [`VoiceFfiError::Busy`] reports for recording.
    #[error("audio session busy")]
    Busy,
    /// A transient failure — safe to retry.
    #[error("transient speech error: {message}")]
    Retriable {
        /// Human-readable detail from the native side.
        message: String,
    },
    /// Any other native failure.
    #[error("speech error: {message}")]
    Other {
        /// Human-readable detail from the native side.
        message: String,
    },
}

/// Crate-local foreign callback interface for native speech-to-text — the Swift
/// app implements it over `SFSpeechRecognizer` (opens the live mic, listens for
/// one utterance, returns the final transcript). Bridged to
/// [`traits::SpeechToText`] by [`IosSttBridge`].
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait IosStt: Send + Sync {
    /// Open the mic, listen for a single utterance, and return the recognized
    /// text. `language` is a BCP-47 hint (`None` = device default).
    async fn transcribe(&self, language: Option<String>) -> Result<String, SpeechFfiError>;
}

/// Crate-local foreign callback interface for native text-to-speech — the Swift
/// app implements it over `AVSpeechSynthesizer`, returning 16-bit signed
/// little-endian mono PCM. Bridged to [`traits::TextToSpeech`] by [`IosTtsBridge`].
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait IosTts: Send + Sync {
    /// Synthesize `text` to PCM16 audio at [`TtsAudioFfi::sample_rate_hz`].
    /// `voice` is a provider-specific id (`None` = system default voice).
    async fn synthesize(
        &self,
        text: String,
        voice: Option<String>,
    ) -> Result<TtsAudioFfi, SpeechFfiError>;
}

/// FFI carrier for synthesized audio crossing the callback-interface seam:
/// PCM16 frames + the sample rate the Swift engine produced them at.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct TtsAudioFfi {
    /// Raw PCM16 frames (16-bit signed little-endian, mono).
    pub pcm: Vec<u8>,
    /// Sample rate of `pcm` in Hz.
    pub sample_rate_hz: u32,
}

/// FFI error surface for the iOS share callback interface. A flat enum so `UniFFI`
/// can render it for an async `callback_interface` method; the bridge fans it
/// back out onto the richer [`traits::ShareError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Error))]
#[derive(Debug, thiserror::Error)]
pub enum ShareFfiError {
    /// Sharing is unsupported on this device / for this payload.
    #[error("sharing unsupported")]
    Unsupported,
    /// Any other native failure.
    #[error("share error: {message}")]
    Other {
        /// Human-readable detail from the native side.
        message: String,
    },
}

/// FFI carrier for the outcome of a native share — whether the user completed
/// or dismissed the system share sheet. Mapped to [`traits::ShareResult`].
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[derive(Debug, Clone)]
pub enum ShareResultFfi {
    /// The user completed the share (chose a target app).
    Success,
    /// The user dismissed the share sheet without sharing.
    Cancelled,
}

/// Crate-local foreign callback interface for native sharing — the Swift app
/// implements it over `UIActivityViewController`. Bridged to
/// [`traits::SharingService`] by [`IosShareBridge`]. The payload crosses the
/// seam as three flat optionals (`text` / `url` / `image_bytes`).
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait IosShare: Send + Sync {
    /// Present the native share sheet for the given payload and report whether
    /// the user completed or cancelled it.
    async fn share(
        &self,
        text: Option<String>,
        url: Option<String>,
        image_bytes: Option<Vec<u8>>,
    ) -> Result<ShareResultFfi, ShareFfiError>;
}

/// Adapts the crate-local [`IosShare`] callback interface to the shared
/// [`traits::SharingService`] seam the engine consumes. Destructures
/// [`traits::SharePayload`] into the flat `text` / `url` / `image_bytes` args
/// and fans [`ShareResultFfi`] / [`ShareFfiError`] back out onto
/// [`traits::ShareResult`] / [`traits::ShareError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
struct IosShareBridge {
    inner: Box<dyn IosShare>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl traits::SharingService for IosShareBridge {
    async fn share(
        &self,
        payload: traits::SharePayload,
    ) -> Result<traits::ShareResult, traits::ShareError> {
        let traits::SharePayload {
            text,
            url,
            image_bytes,
        } = payload;
        match self.inner.share(text, url, image_bytes).await {
            Ok(ShareResultFfi::Success) => Ok(traits::ShareResult::Success),
            Ok(ShareResultFfi::Cancelled) => Ok(traits::ShareResult::Cancelled),
            Err(ShareFfiError::Unsupported) => Err(traits::ShareError::Unsupported),
            Err(ShareFfiError::Other { message }) => Err(traits::ShareError::Other(message)),
        }
    }
}

/// FFI error surface for the iOS location callback interface. Flat, like its
/// notification/camera siblings, so `UniFFI` can render it for an async
/// `callback_interface` method.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Error))]
#[derive(Debug, thiserror::Error)]
pub enum LocationFfiError {
    /// The user denied location permission.
    #[error("location permission denied")]
    PermissionDenied,
    /// Location services are off, restricted, or absent.
    #[error("location unavailable")]
    Unavailable,
    /// No fix arrived before the native deadline.
    #[error("location timed out")]
    Timeout,
    /// Any other native failure.
    #[error("location error: {message}")]
    Other {
        /// Human-readable detail from the native side.
        message: String,
    },
}

/// FFI carrier for one resolved location crossing the callback-interface
/// seam. Mapped to [`traits::LocationFix`].
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct LocationFixFfi {
    /// Latitude in decimal degrees (WGS-84).
    pub latitude: f64,
    /// Longitude in decimal degrees (WGS-84).
    pub longitude: f64,
    /// Horizontal accuracy in meters, when the platform reports one.
    pub accuracy_m: Option<f64>,
    /// Fix time, epoch milliseconds.
    pub timestamp_ms: u64,
}

/// Crate-local foreign callback interface for one-shot location — the Swift
/// app implements it over `CLLocationManager`. Bridged to
/// [`traits::LocationProvider`] by [`IosLocationBridge`].
///
/// One-shot only: continuous tracking would need a host-to-page push channel
/// that does not exist yet, and a background-location entitlement nobody has
/// asked for.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait IosLocation: Send + Sync {
    /// Resolve the device's current location once.
    async fn current_location(&self) -> Result<LocationFixFfi, LocationFfiError>;
}

/// Adapts the crate-local [`IosLocation`] callback interface to the shared
/// [`traits::LocationProvider`] seam the engine consumes.
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
struct IosLocationBridge {
    inner: Box<dyn IosLocation>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl traits::LocationProvider for IosLocationBridge {
    async fn current_location(&self) -> Result<traits::LocationFix, traits::LocationError> {
        match self.inner.current_location().await {
            Ok(fix) => Ok(traits::LocationFix {
                latitude: fix.latitude,
                longitude: fix.longitude,
                accuracy_m: fix.accuracy_m,
                timestamp_ms: fix.timestamp_ms,
            }),
            Err(LocationFfiError::PermissionDenied) => Err(traits::LocationError::PermissionDenied),
            Err(LocationFfiError::Unavailable) => Err(traits::LocationError::Unavailable),
            Err(LocationFfiError::Timeout) => Err(traits::LocationError::Timeout),
            Err(LocationFfiError::Other { message }) => Err(traits::LocationError::Other(message)),
        }
    }
}

/// FFI error surface for the iOS notification callback interface. A flat enum so
/// `UniFFI` can render it for an async `callback_interface` method; the bridge
/// fans it back out onto the richer [`traits::NotificationError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Error))]
#[derive(Debug, thiserror::Error)]
pub enum NotificationFfiError {
    /// The user denied notification permission.
    #[error("notification permission denied")]
    PermissionDenied,
    /// Any other native failure.
    #[error("notification error: {message}")]
    Other {
        /// Human-readable detail from the native side.
        message: String,
    },
}

/// Crate-local foreign callback interface for native notifications — the Swift
/// app implements it over `UNUserNotificationCenter`. Bridged to
/// [`traits::NotificationService`] by [`IosNotificationBridge`]. The request
/// crosses the seam as the flat `title` / `body` / `tag` args.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait IosNotification: Send + Sync {
    /// Post a single local notification. `tag` (when present) lets a later post
    /// replace an earlier one (the notification request identifier).
    async fn notify(
        &self,
        title: String,
        body: String,
        tag: Option<String>,
    ) -> Result<(), NotificationFfiError>;
}

/// Adapts the crate-local [`IosNotification`] callback interface to the shared
/// [`traits::NotificationService`] seam the engine consumes. Destructures
/// [`traits::NotificationRequest`] into the flat `title` / `body` / `tag` args
/// and fans [`NotificationFfiError`] back out onto [`traits::NotificationError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
struct IosNotificationBridge {
    inner: Box<dyn IosNotification>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl traits::NotificationService for IosNotificationBridge {
    async fn notify(
        &self,
        req: traits::NotificationRequest,
    ) -> Result<(), traits::NotificationError> {
        let traits::NotificationRequest { title, body, tag } = req;
        match self.inner.notify(title, body, tag).await {
            Ok(()) => Ok(()),
            Err(NotificationFfiError::PermissionDenied) => {
                Err(traits::NotificationError::PermissionDenied)
            }
            Err(NotificationFfiError::Other { message }) => {
                Err(traits::NotificationError::Other(message))
            }
        }
    }
}

/// FFI error surface for the iOS clipboard callback interface. A flat enum so
/// `UniFFI` can render it for an async `callback_interface` method; the bridge
/// fans it back out onto the richer [`traits::ClipboardError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Error))]
#[derive(Debug, thiserror::Error)]
pub enum ClipboardFfiError {
    /// The platform does not support this clipboard operation.
    #[error("clipboard operation unsupported")]
    Unsupported,
    /// Any other native failure.
    #[error("clipboard error: {message}")]
    Other {
        /// Human-readable detail from the native side.
        message: String,
    },
}

/// Crate-local foreign callback interface for native clipboard access — the
/// Swift app implements it over `UIPasteboard` (set via `string =`; get via
/// `string`). Bridged to [`traits::Clipboard`] by [`IosClipboardBridge`].
/// `get_text` returns `None` when the clipboard is empty or holds no text.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait IosClipboard: Send + Sync {
    /// Write plain `text` to the system clipboard.
    async fn set_text(&self, text: String) -> Result<(), ClipboardFfiError>;
    /// Read plain text from the system clipboard. Returns `None` when empty.
    async fn get_text(&self) -> Result<Option<String>, ClipboardFfiError>;
}

/// Adapts the crate-local [`IosClipboard`] callback interface to the shared
/// [`traits::Clipboard`] seam the engine consumes. One forwarding hop per call;
/// maps [`ClipboardFfiError`] back out onto [`traits::ClipboardError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
struct IosClipboardBridge {
    inner: Box<dyn IosClipboard>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl traits::Clipboard for IosClipboardBridge {
    async fn set_text(&self, text: String) -> Result<(), traits::ClipboardError> {
        self.inner
            .set_text(text)
            .await
            .map_err(clipboard_error_from_ffi)
    }
    async fn get_text(&self) -> Result<Option<String>, traits::ClipboardError> {
        self.inner
            .get_text()
            .await
            .map_err(clipboard_error_from_ffi)
    }
}

/// Fan a flat [`ClipboardFfiError`] back out onto the richer
/// [`traits::ClipboardError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
fn clipboard_error_from_ffi(e: ClipboardFfiError) -> traits::ClipboardError {
    match e {
        ClipboardFfiError::Unsupported => traits::ClipboardError::Unsupported,
        ClipboardFfiError::Other { message } => traits::ClipboardError::Other(message),
    }
}

/// FFI error surface for the iOS camera callback interface. A flat enum so
/// `UniFFI` can render it for an async `callback_interface` method; the bridge
/// fans it back out onto the richer [`traits::CameraError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Error))]
#[derive(Debug, thiserror::Error)]
pub enum CameraFfiError {
    /// The user denied camera / photo-library permission.
    #[error("camera permission denied")]
    PermissionDenied,
    /// The user cancelled the capture / picker.
    #[error("camera capture cancelled")]
    Cancelled,
    /// No camera hardware is available.
    #[error("camera device unavailable")]
    DeviceUnavailable,
    /// Any other native failure.
    #[error("camera error: {message}")]
    Other {
        /// Human-readable detail from the native side.
        message: String,
    },
}

/// FFI carrier for a captured (or picked) image crossing the callback-interface
/// seam: JPEG-encoded bytes + the decoded pixel dimensions.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct CapturedImageFfi {
    /// JPEG-encoded image bytes.
    pub jpeg_bytes: Vec<u8>,
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
}

/// Crate-local foreign callback interface for native camera access — the Swift
/// app implements it over `UIImagePickerController` / `PHPickerViewController`.
/// Bridged to [`traits::CameraControl`] by [`IosCameraBridge`].
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait IosCamera: Send + Sync {
    /// Capture a photo with the native camera UI. `front` selects the
    /// front/selfie camera when true (rear when false); `allow_editing`
    /// presents the native edit/crop UI after capture.
    async fn capture_photo(
        &self,
        front: bool,
        allow_editing: bool,
    ) -> Result<CapturedImageFfi, CameraFfiError>;
    /// Pick an existing image from the system photo library.
    async fn pick_from_library(&self) -> Result<CapturedImageFfi, CameraFfiError>;

    /// Capture, then downscale to at most `max_dimension` px on the longer
    /// side and re-encode at `jpeg_quality` (0.0..=1.0).
    ///
    /// The scaling happens natively because Rust ships no image codec here
    /// (the mobile build vendors its dependencies offline), and a
    /// full-resolution 12 MP JPEG is 3-6 MB — far past what a local app's
    /// bridge response, or a provider's vision endpoint, will take.
    async fn capture_photo_sized(
        &self,
        front: bool,
        allow_editing: bool,
        max_dimension: u32,
        jpeg_quality: f32,
    ) -> Result<CapturedImageFfi, CameraFfiError>;

    /// Library pick with the same native downscale contract as
    /// [`IosCamera::capture_photo_sized`].
    async fn pick_from_library_sized(
        &self,
        max_dimension: u32,
        jpeg_quality: f32,
    ) -> Result<CapturedImageFfi, CameraFfiError>;
}

/// Adapts the crate-local [`IosCamera`] callback interface to the shared
/// [`traits::CameraControl`] seam the engine consumes. Maps
/// [`traits::CameraPosition`] onto the flat `front` bool, threads
/// `allow_editing`, and fans [`CameraFfiError`] back out onto
/// [`traits::CameraError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
struct IosCameraBridge {
    inner: Box<dyn IosCamera>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl traits::CameraControl for IosCameraBridge {
    async fn capture_photo(
        &self,
        opts: traits::CapturePhotoOpts,
    ) -> Result<traits::CapturedImage, traits::CameraError> {
        let front = matches!(opts.position, traits::CameraPosition::Front);
        match self.inner.capture_photo(front, opts.allow_editing).await {
            Ok(img) => Ok(captured_image_from_ffi(img)),
            Err(e) => Err(camera_error_from_ffi(e)),
        }
    }
    async fn pick_from_library(&self) -> Result<traits::CapturedImage, traits::CameraError> {
        match self.inner.pick_from_library().await {
            Ok(img) => Ok(captured_image_from_ffi(img)),
            Err(e) => Err(camera_error_from_ffi(e)),
        }
    }
    // Overrides the trait's delegating defaults: on iOS the native side CAN
    // scale, and a local app's bridge budget depends on it doing so.
    async fn capture_photo_sized(
        &self,
        opts: traits::CapturePhotoOpts,
        max_dimension: u32,
        jpeg_quality: f32,
    ) -> Result<traits::CapturedImage, traits::CameraError> {
        let front = matches!(opts.position, traits::CameraPosition::Front);
        match self
            .inner
            .capture_photo_sized(front, opts.allow_editing, max_dimension, jpeg_quality)
            .await
        {
            Ok(img) => Ok(captured_image_from_ffi(img)),
            Err(e) => Err(camera_error_from_ffi(e)),
        }
    }
    async fn pick_from_library_sized(
        &self,
        max_dimension: u32,
        jpeg_quality: f32,
    ) -> Result<traits::CapturedImage, traits::CameraError> {
        match self
            .inner
            .pick_from_library_sized(max_dimension, jpeg_quality)
            .await
        {
            Ok(img) => Ok(captured_image_from_ffi(img)),
            Err(e) => Err(camera_error_from_ffi(e)),
        }
    }
}

/// Convert an FFI [`CapturedImageFfi`] into the shared [`traits::CapturedImage`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
fn captured_image_from_ffi(img: CapturedImageFfi) -> traits::CapturedImage {
    traits::CapturedImage {
        jpeg_bytes: img.jpeg_bytes,
        width: img.width,
        height: img.height,
    }
}

/// Fan a flat [`CameraFfiError`] back out onto the richer [`traits::CameraError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
fn camera_error_from_ffi(e: CameraFfiError) -> traits::CameraError {
    match e {
        CameraFfiError::PermissionDenied => traits::CameraError::PermissionDenied,
        CameraFfiError::Cancelled => traits::CameraError::Cancelled,
        CameraFfiError::DeviceUnavailable => traits::CameraError::DeviceUnavailable,
        CameraFfiError::Other { message } => traits::CameraError::Other(message),
    }
}

/// FFI error surface for the iOS secure-storage callback interface. A flat enum
/// so `UniFFI` can render it for an async `callback_interface` method; the bridge
/// fans it back out onto the richer [`traits::SecureStorageError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Error))]
#[derive(Debug, thiserror::Error)]
pub enum SecureStorageFfiError {
    /// The OS denied access (e.g. Keychain item requires user auth / device unlock).
    #[error("secure storage permission denied: {message}")]
    PermissionDenied {
        /// Human-readable detail from the native side.
        message: String,
    },
    /// The Keychain is currently unusable.
    #[error("secure storage backend unavailable: {message}")]
    BackendUnavailable {
        /// Human-readable detail from the native side.
        message: String,
    },
    /// Any other native failure (non-zero OSStatus, etc.).
    #[error("secure storage io error: {message}")]
    Io {
        /// Human-readable detail from the native side.
        message: String,
    },
}

/// Crate-local foreign callback interface for the native iOS Keychain-backed
/// secure store — the Swift app implements it over `SecItemAdd`/`SecItemCopyMatching`
/// (kSecClass GenericPassword, kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
/// so items are excluded from iCloud/iTunes backups). The engine's serialized
/// `SecureStorageData` crosses the seam as an opaque `blob` keyed by
/// `(service, account)`. Bridged to [`traits::SecureStorage`] by
/// [`IosSecureStorageBridge`].
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait IosSecureStorage: Send + Sync {
    /// Persist `blob` under `(service, account)`, overwriting any existing entry.
    async fn store(
        &self,
        service: String,
        account: String,
        blob: Vec<u8>,
    ) -> Result<(), SecureStorageFfiError>;
    /// Return the blob for `(service, account)`, or `None` if absent.
    async fn retrieve(
        &self,
        service: String,
        account: String,
    ) -> Result<Option<Vec<u8>>, SecureStorageFfiError>;
    /// Remove `(service, account)` (removing a non-existent entry is not an error).
    async fn delete(&self, service: String, account: String) -> Result<(), SecureStorageFfiError>;
    /// List every `account` stored under `service`.
    async fn list(&self, service: String) -> Result<Vec<String>, SecureStorageFfiError>;
}

/// Adapts the crate-local [`IosSecureStorage`] (opaque-blob FFI) to the shared
/// [`traits::SecureStorage`] seam: serde-encodes `SecureStorageData` to a blob on
/// store, decodes on retrieve, and reports the Keychain as an encrypted backend.
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
struct IosSecureStorageBridge {
    inner: Box<dyn IosSecureStorage>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl traits::SecureStorage for IosSecureStorageBridge {
    async fn store(
        &self,
        service: &str,
        account: &str,
        data: protocol::SecureStorageData,
    ) -> Result<(), traits::SecureStorageError> {
        let blob = serde_json::to_vec(&data)
            .map_err(|e| traits::SecureStorageError::Io(format!("serialize: {e}")))?;
        self.inner
            .store(service.to_string(), account.to_string(), blob)
            .await
            .map_err(securestorage_error_from_ffi)
    }
    async fn retrieve(
        &self,
        service: &str,
        account: &str,
    ) -> Result<Option<protocol::SecureStorageData>, traits::SecureStorageError> {
        match self
            .inner
            .retrieve(service.to_string(), account.to_string())
            .await
            .map_err(securestorage_error_from_ffi)?
        {
            Some(blob) => {
                let data = serde_json::from_slice(&blob)
                    .map_err(|e| traits::SecureStorageError::Io(format!("deserialize: {e}")))?;
                Ok(Some(data))
            }
            None => Ok(None),
        }
    }
    async fn delete(&self, service: &str, account: &str) -> Result<(), traits::SecureStorageError> {
        self.inner
            .delete(service.to_string(), account.to_string())
            .await
            .map_err(securestorage_error_from_ffi)
    }
    async fn list(&self, service: &str) -> Result<Vec<String>, traits::SecureStorageError> {
        self.inner
            .list(service.to_string())
            .await
            .map_err(securestorage_error_from_ffi)
    }
    fn is_encrypted(&self) -> bool {
        true
    }
    fn backend(&self) -> traits::SecureStorageBackend {
        traits::SecureStorageBackend::IosKeychain
    }
}

/// Fan a flat [`SecureStorageFfiError`] back out onto [`traits::SecureStorageError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
fn securestorage_error_from_ffi(e: SecureStorageFfiError) -> traits::SecureStorageError {
    match e {
        SecureStorageFfiError::PermissionDenied { message } => {
            traits::SecureStorageError::PermissionDenied(message)
        }
        SecureStorageFfiError::BackendUnavailable { message } => {
            traits::SecureStorageError::BackendUnavailable(message)
        }
        SecureStorageFfiError::Io { message } => traits::SecureStorageError::Io(message),
    }
}

/// FFI error surface for the iOS mic-recorder callback interface. A flat enum so
/// `UniFFI` can render it for an async `callback_interface` method; the bridge
/// fans it back out onto the richer [`traits::VoiceError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Error))]
#[derive(Debug, thiserror::Error)]
pub enum VoiceFfiError {
    /// The user denied microphone permission.
    #[error("microphone permission denied")]
    PermissionDenied,
    /// `stop_recording` was called with no active session.
    #[error("not currently recording")]
    NotRecording,
    /// The shared `AVAudioSession` is held by another consumer (FlowMode's
    /// voice orb, a hold-to-talk capture): the recorder is fine, the session
    /// is not free. Distinct from `Other` so a local app can tell the user
    /// "try again in a moment" instead of surfacing an opaque failure.
    #[error("audio session busy")]
    Busy,
    /// Any other native failure.
    #[error("voice error: {message}")]
    Other {
        /// Human-readable detail from the native side.
        message: String,
    },
}

/// FFI carrier for a finished recording crossing the callback-interface seam:
/// the encoded audio bytes + their MIME type. Mapped to [`traits::VoiceRecording`].
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct VoiceRecordingFfi {
    /// Encoded audio bytes.
    pub audio_bytes: Vec<u8>,
    /// MIME type of `audio_bytes` (e.g. `"audio/m4a"`).
    pub mime_type: String,
}

/// Crate-local foreign callback interface for native mic recording — the Swift
/// app implements it over `AVAudioRecorder`. Bridged to [`traits::VoiceRecorder`]
/// by [`IosVoiceBridge`]. Driven by the engine through `tool-voice`
/// (start/stop/is_recording); the recording opts cross the seam as the flat
/// `sample_rate_hz` / `format` args.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait IosVoice: Send + Sync {
    /// Begin a mic recording session at the given sample rate / container format.
    async fn start_recording(
        &self,
        sample_rate_hz: u32,
        format: String,
    ) -> Result<(), VoiceFfiError>;
    /// Stop the active session and return the captured audio.
    async fn stop_recording(&self) -> Result<VoiceRecordingFfi, VoiceFfiError>;
    /// Whether a recording session is currently active.
    async fn is_recording(&self) -> bool;
}

/// Adapts the crate-local [`IosVoice`] callback interface to the shared
/// [`traits::VoiceRecorder`] seam the engine consumes. Destructures
/// [`traits::VoiceRecordingOpts`] into the flat `sample_rate_hz` / `format`
/// args, converts [`VoiceRecordingFfi`] back to [`traits::VoiceRecording`], and
/// fans [`VoiceFfiError`] back out onto [`traits::VoiceError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
struct IosVoiceBridge {
    inner: Box<dyn IosVoice>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl traits::VoiceRecorder for IosVoiceBridge {
    async fn start_recording(
        &self,
        opts: traits::VoiceRecordingOpts,
    ) -> Result<(), traits::VoiceError> {
        let traits::VoiceRecordingOpts {
            sample_rate_hz,
            format,
        } = opts;
        self.inner
            .start_recording(sample_rate_hz, format)
            .await
            .map_err(voice_error_from_ffi)
    }
    async fn stop_recording(&self) -> Result<traits::VoiceRecording, traits::VoiceError> {
        match self.inner.stop_recording().await {
            Ok(rec) => Ok(traits::VoiceRecording {
                audio_bytes: rec.audio_bytes,
                mime_type: rec.mime_type,
            }),
            Err(e) => Err(voice_error_from_ffi(e)),
        }
    }
    async fn is_recording(&self) -> bool {
        self.inner.is_recording().await
    }
}

/// Fan a flat [`VoiceFfiError`] back out onto the richer [`traits::VoiceError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
fn voice_error_from_ffi(e: VoiceFfiError) -> traits::VoiceError {
    match e {
        VoiceFfiError::PermissionDenied => traits::VoiceError::PermissionDenied,
        VoiceFfiError::NotRecording => traits::VoiceError::NotRecording,
        VoiceFfiError::Busy => traits::VoiceError::Busy,
        VoiceFfiError::Other { message } => traits::VoiceError::Other(message),
    }
}

/// Adapts the crate-local [`IosStt`] callback interface to the shared
/// [`traits::SpeechToText`] seam the engine consumes. One forwarding hop per
/// call; maps [`SpeechFfiError`] onto [`traits::SttError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
struct IosSttBridge {
    inner: Box<dyn IosStt>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl traits::SpeechToText for IosSttBridge {
    async fn transcribe(
        &self,
        opts: traits::SttOpts,
    ) -> Result<traits::SttTranscript, traits::SttError> {
        match self.inner.transcribe(opts.language.clone()).await {
            Ok(text) => Ok(traits::SttTranscript {
                text,
                language: opts.language,
                confidence: None,
            }),
            Err(e) => Err(match e {
                SpeechFfiError::PermissionDenied => traits::SttError::PermissionDenied,
                SpeechFfiError::NoSpeech => traits::SttError::NoSpeech,
                SpeechFfiError::Unavailable => traits::SttError::Unavailable,
                SpeechFfiError::Busy => traits::SttError::Busy,
                SpeechFfiError::Retriable { message } => traits::SttError::Retriable(message),
                SpeechFfiError::Other { message } => traits::SttError::Other(message),
            }),
        }
    }
}

/// Adapts the crate-local [`IosTts`] callback interface to the shared
/// [`traits::TextToSpeech`] seam the engine consumes. Maps [`SpeechFfiError`]
/// onto [`traits::TtsError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
struct IosTtsBridge {
    inner: Box<dyn IosTts>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl traits::TextToSpeech for IosTtsBridge {
    async fn synthesize(
        &self,
        opts: traits::TtsOpts,
    ) -> Result<traits::TtsAudio, traits::TtsError> {
        match self.inner.synthesize(opts.text, opts.voice).await {
            Ok(audio) => Ok(traits::TtsAudio {
                pcm: audio.pcm,
                sample_rate_hz: audio.sample_rate_hz,
            }),
            Err(e) => Err(match e {
                SpeechFfiError::Unavailable => traits::TtsError::Unavailable,
                SpeechFfiError::Retriable { message } | SpeechFfiError::Other { message } => {
                    traits::TtsError::SynthesisFailed(message)
                }
                // STT-only variants are not produced by a TTS impl; fold them
                // into a generic TTS error rather than panic.
                SpeechFfiError::PermissionDenied => {
                    traits::TtsError::Other("permission denied".to_string())
                }
                SpeechFfiError::NoSpeech => traits::TtsError::Other("no speech".to_string()),
                // Audio-session contention is real for playback too, but
                // `TtsError` has no busy variant; keep it recognizable in
                // the message rather than folding it into a bare "other".
                SpeechFfiError::Busy => traits::TtsError::Other("audio session busy".to_string()),
            }),
        }
    }
}

/// Foreign-callable constructor for the iOS app (plan M10-P3a).
///
/// Builds a fully-wired [`MobileEngineHandle`] from the Swift-supplied event
/// listener + runtime config. The handle owns its tokio runtime and streams
/// every [`client_protocol::events::ClientEvent`] to `listener.on_event(..)`;
/// the app drives turns via [`MobileEngineHandle::submit`].
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
    stt: Box<dyn IosStt>,
    tts: Box<dyn IosTts>,
    camera: Box<dyn IosCamera>,
    share: Box<dyn IosShare>,
    voice: Box<dyn IosVoice>,
    notifications: Box<dyn IosNotification>,
    clipboard: Box<dyn IosClipboard>,
    permissions: Box<dyn IosPermissionSink>,
    secure_storage: Option<Box<dyn IosSecureStorage>>,
    // Defaulted so the two callers that have no location impl (the cron
    // bridge, the engine round-trip test) keep compiling untouched; only the
    // conversation host passes one.
    location: Option<Box<dyn IosLocation>>,
) -> Result<Arc<MobileEngineHandle>, MobileEngineError> {
    let listener: Arc<dyn ClientEventListener> = Arc::new(IosListenerBridge { inner: listener });
    #[cfg(target_os = "ios")]
    {
        use platform_ios::{IosPlatform, IosPlatformInputs};

        let cfg = ios_mobile_config_from_launch_config(&config)?;
        // The app generator uses the bundled runtime independently of the
        // user-facing terminal mode. The iOS tool registry has no mobile shell
        // carrier, so selecting this internal runtime cannot expose a shell.
        let mut local_apps_mobile_linux = config.mobile_linux.clone();
        if config.local_apps_runtime_root.is_some() {
            if let Some(runtime) = local_apps_mobile_linux.as_mut() {
                runtime.mode = MobileLinuxRuntimeModeFfi::MobileLinux;
            }
        }
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
            voice: Arc::new(IosVoiceBridge { inner: voice }),
            share: Arc::new(IosShareBridge { inner: share }),
            stt: Some(Arc::new(IosSttBridge { inner: stt })),
            tts: Some(Arc::new(IosTtsBridge { inner: tts })),
            notifications: Some(Arc::new(IosNotificationBridge {
                inner: notifications,
            })),
            clipboard: Some(Arc::new(IosClipboardBridge { inner: clipboard })),
            secure_storage: secure_storage.map(|s| {
                Arc::new(IosSecureStorageBridge { inner: s }) as Arc<dyn traits::SecureStorage>
            }),
            location: location.map(|l| {
                Arc::new(IosLocationBridge { inner: l }) as Arc<dyn traits::LocationProvider>
            }),
            mobile_linux: ios_mobile_linux_runtime(local_apps_mobile_linux.as_ref()),
            workspace_host_path: Some(workspace_host_path),
            stable_workspace_id: Some(stable_workspace_id),
        }));
        let permission_sink: Arc<dyn PermissionRequestSink> =
            Arc::new(IosPermissionSinkBridge { inner: permissions });
        engine_mobile::build_mobile_engine(cfg, platform, listener, permission_sink)
    }
    #[cfg(not(target_os = "ios"))]
    {
        let _ = (
            config,
            listener,
            stt,
            tts,
            camera,
            share,
            voice,
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
    stt: Box<dyn IosStt>,
    tts: Box<dyn IosTts>,
    camera: Box<dyn IosCamera>,
    share: Box<dyn IosShare>,
    voice: Box<dyn IosVoice>,
    notifications: Box<dyn IosNotification>,
    clipboard: Box<dyn IosClipboard>,
    permissions: Box<dyn IosPermissionSink>,
    mobile_linux: Option<IosMobileLinuxConfigFfi>,
    secure_storage: Option<Box<dyn IosSecureStorage>>,
) -> Result<Arc<MobileEngineHandle>, MobileEngineError> {
    build_ios_engine_with_config(
        IosEngineLaunchConfigFfi {
            api_base,
            api_key,
            model,
            app_sandbox_root,
            project_cwd: None,
            provider_config: None,
            mobile_linux,
            local_apps_full_runtime: false,
            local_apps_runtime_root: None,
            physical_memory_bytes: 0,
        },
        listener,
        stt,
        tts,
        camera,
        share,
        voice,
        notifications,
        clipboard,
        permissions,
        secure_storage,
        None,
    )
}

/// Probe the iOS mobile-linux bridge without constructing the full engine.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn probe_ios_mobile_linux(config: Option<IosMobileLinuxConfigFfi>) -> MobileLinuxCapabilityFfi {
    match ios_mobile_linux_runtime(config.as_ref()) {
        Some(runtime) => {
            let probe_rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("ios mobile-linux probe runtime");
            capability_to_ffi(probe_rt.block_on(runtime.probe_capability()))
        }
        None => capability_to_ffi(traits::MobileLinuxCapability {
            available: false,
            backend: traits::SandboxBackend::IosIsh,
            mode: traits::MobileLinuxRuntimeMode::Legacy,
            reason: Some("legacy unavailable backend selected".to_string()),
            streaming_output: false,
            background_processes: false,
            pty: false,
            bind_mounts: false,
            rootfs_integrity: false,
        }),
    }
}

/// Inspect the iOS mobile-linux rootfs state without constructing the full engine.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn ios_mobile_linux_status(config: Option<IosMobileLinuxConfigFfi>) -> MobileLinuxStatusFfi {
    ios_mobile_linux_status_from_config(config.as_ref())
}

/// Phase-1 verify action: returns the current computed iOS mobile-linux status.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn verify_ios_mobile_linux(config: Option<IosMobileLinuxConfigFfi>) -> MobileLinuxStatusFfi {
    ios_mobile_linux_status_from_config(config.as_ref())
}

/// Phase-1 repair action: returns the current computed iOS mobile-linux status.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn repair_ios_mobile_linux(config: Option<IosMobileLinuxConfigFfi>) -> MobileLinuxStatusFfi {
    ios_mobile_linux_status_from_config(config.as_ref())
}

/// Phase-1 reset action: returns the current computed iOS mobile-linux status.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn reset_ios_mobile_linux(config: Option<IosMobileLinuxConfigFfi>) -> MobileLinuxStatusFfi {
    ios_mobile_linux_status_from_config(config.as_ref())
}

/// Streaming sink for command / PTY output.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait IosMobileLinuxEventSink: Send + Sync {
    async fn on_event(
        &self,
        event: MobileLinuxStreamEventFfi,
    ) -> Result<(), MobileLinuxEventSinkFfiError>;
}

#[cfg(feature = "uniffi")]
struct MobileLinuxStreamSinkBridge {
    stream_id: String,
    source: MobileLinuxStreamSourceFfi,
    inner: Box<dyn IosMobileLinuxEventSink>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl traits::ProcessStreamSink for MobileLinuxStreamSinkBridge {
    async fn stdout_line(&self, line: String) -> Result<(), traits::ProcessError> {
        self.inner
            .on_event(MobileLinuxStreamEventFfi {
                sequence: 0,
                task_id: None,
                stream_id: self.stream_id.clone(),
                source: self.source,
                kind: MobileLinuxStreamEventKindFfi::StdoutLine,
                text: Some(line),
                data: None,
                exit_code: None,
                timed_out: false,
            })
            .await
            .map_err(|err| traits::ProcessError::Io(err.to_string()))
    }

    async fn stderr_chunk(&self, chunk: Vec<u8>) -> Result<(), traits::ProcessError> {
        self.inner
            .on_event(MobileLinuxStreamEventFfi {
                sequence: 0,
                task_id: None,
                stream_id: self.stream_id.clone(),
                source: self.source,
                kind: MobileLinuxStreamEventKindFfi::StderrChunk,
                text: Some(String::from_utf8_lossy(&chunk).into_owned()),
                data: Some(chunk),
                exit_code: None,
                timed_out: false,
            })
            .await
            .map_err(|err| traits::ProcessError::Io(err.to_string()))
    }
}

#[cfg_attr(feature = "uniffi", derive(uniffi::Object))]
pub struct IosMobileLinuxRuntimeHandle {
    runtime: Arc<dyn traits::MobileLinuxRuntime>,
}

impl IosMobileLinuxRuntimeHandle {
    fn new(runtime: Arc<dyn traits::MobileLinuxRuntime>) -> Self {
        Self { runtime }
    }

    async fn availability_error(&self) -> MobileLinuxOperationFfiError {
        unavailable_runtime_error(self.runtime.clone()).await
    }
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn create_ios_mobile_linux_runtime(
    config: IosMobileLinuxConfigFfi,
) -> Result<Arc<IosMobileLinuxRuntimeHandle>, MobileLinuxOperationFfiError> {
    if matches!(config.mode, MobileLinuxRuntimeModeFfi::Legacy) {
        return Err(MobileLinuxOperationFfiError::Unavailable {
            message: "legacy unavailable backend selected".to_string(),
        });
    }
    let app_sandbox_root = resolve_mobile_linux_app_sandbox_root(&config)?;
    let _ = validate_mobile_linux_workspace_config(
        app_sandbox_root.to_string_lossy().as_ref(),
        &config,
    )?;
    let runtime =
        probe_runtime(Some(&config)).ok_or_else(|| MobileLinuxOperationFfiError::Unavailable {
            message: "legacy unavailable backend selected".to_string(),
        })?;
    Ok(Arc::new(IosMobileLinuxRuntimeHandle::new(runtime)))
}

#[cfg_attr(feature = "uniffi", uniffi::export(async_runtime = "tokio"))]
impl IosMobileLinuxRuntimeHandle {
    pub async fn capability(&self) -> MobileLinuxCapabilityFfi {
        capability_to_ffi(self.runtime.probe_capability().await)
    }

    pub async fn status(&self) -> Result<MobileLinuxStatusFfi, MobileLinuxOperationFfiError> {
        self.runtime
            .rootfs_status()
            .await
            .map(status_to_ffi)
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn verify_rootfs(
        &self,
    ) -> Result<MobileLinuxStatusFfi, MobileLinuxOperationFfiError> {
        self.runtime
            .verify_rootfs()
            .await
            .map(status_to_ffi)
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn repair_rootfs(
        &self,
    ) -> Result<MobileLinuxStatusFfi, MobileLinuxOperationFfiError> {
        self.runtime
            .repair_rootfs()
            .await
            .map(status_to_ffi)
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn reset_rootfs(&self) -> Result<MobileLinuxStatusFfi, MobileLinuxOperationFfiError> {
        self.runtime
            .reset_rootfs()
            .await
            .map(status_to_ffi)
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn boot(&self) -> Result<MobileLinuxStatusFfi, MobileLinuxOperationFfiError> {
        self.runtime
            .boot()
            .await
            .map(status_to_ffi)
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn shutdown(&self) -> Result<MobileLinuxStatusFfi, MobileLinuxOperationFfiError> {
        self.runtime
            .shutdown()
            .await
            .map_err(mobile_linux_error_to_ffi)?;
        self.runtime
            .rootfs_status()
            .await
            .map(status_to_ffi)
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn run_command(
        &self,
        request: MobileLinuxCommandRequestFfi,
    ) -> Result<MobileLinuxCommandResultFfi, MobileLinuxOperationFfiError> {
        self.runtime
            .run(command_request_from_ffi(request))
            .await
            .map(command_result_to_ffi)
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn run_command_streaming(
        &self,
        request: MobileLinuxCommandRequestFfi,
        sink: Box<dyn IosMobileLinuxEventSink>,
    ) -> Result<MobileLinuxCommandResultFfi, MobileLinuxOperationFfiError> {
        let stream_id = format!(
            "run-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|value| value.as_millis())
                .unwrap_or(0)
        );
        let bridge = Arc::new(MobileLinuxStreamSinkBridge {
            stream_id: stream_id.clone(),
            source: MobileLinuxStreamSourceFfi::Run,
            inner: sink,
        });
        match self
            .runtime
            .run_streaming(command_request_from_ffi(request), bridge.clone())
            .await
        {
            Ok(result) => Ok(command_result_to_ffi(result)),
            Err(error) => {
                let detail = error.to_string();
                let _ = bridge
                    .inner
                    .on_event(MobileLinuxStreamEventFfi {
                        sequence: 0,
                        task_id: None,
                        stream_id,
                        source: MobileLinuxStreamSourceFfi::Run,
                        kind: MobileLinuxStreamEventKindFfi::Error,
                        text: Some(detail.clone()),
                        data: None,
                        exit_code: None,
                        timed_out: matches!(error, traits::MobileLinuxError::Timeout),
                    })
                    .await;
                Err(mobile_linux_error_to_ffi(error))
            }
        }
    }

    pub async fn spawn_task(
        &self,
        request: MobileLinuxCommandRequestFfi,
    ) -> Result<MobileLinuxTaskFfi, MobileLinuxOperationFfiError> {
        self.runtime
            .spawn_background(command_request_from_ffi(request))
            .await
            .map(|handle| MobileLinuxTaskFfi {
                id: handle.id,
                title: "guest-command".to_string(),
                state: MobileLinuxTaskStateFfi::Running,
                detail: None,
            })
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn list_tasks(
        &self,
    ) -> Result<Vec<MobileLinuxTaskFfi>, MobileLinuxOperationFfiError> {
        let capability = self.runtime.probe_capability().await;
        if !capability.available {
            return Err(self.availability_error().await);
        }
        self.runtime
            .list_tasks()
            .await
            .map(|tasks| tasks.into_iter().map(task_snapshot_to_ffi).collect())
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn task_status(
        &self,
        task_id: String,
    ) -> Result<Option<MobileLinuxTaskFfi>, MobileLinuxOperationFfiError> {
        let capability = self.runtime.probe_capability().await;
        if !capability.available {
            return Err(self.availability_error().await);
        }
        self.runtime
            .task_status(&task_id)
            .await
            .map(|task| task.map(task_snapshot_to_ffi))
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn read_events(
        &self,
        after_sequence: Option<u64>,
        limit: Option<u32>,
    ) -> Result<Vec<MobileLinuxStreamEventFfi>, MobileLinuxOperationFfiError> {
        let capability = self.runtime.probe_capability().await;
        if !capability.available {
            return Err(self.availability_error().await);
        }
        self.runtime
            .read_events(
                after_sequence,
                limit.unwrap_or(MAX_MOBILE_LINUX_EVENT_BATCH as u32) as usize,
            )
            .await
            .map(|events| events.into_iter().filter_map(event_to_ffi).collect())
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn kill_task(
        &self,
        task_id: String,
    ) -> Result<MobileLinuxTaskFfi, MobileLinuxOperationFfiError> {
        self.runtime
            .kill(&traits::LinuxProcessHandle {
                id: task_id.clone(),
                enforcement: traits::LinuxEnforcementReceipt::default(),
            })
            .await
            .map_err(mobile_linux_error_to_ffi)?;
        Ok(MobileLinuxTaskFfi {
            id: task_id,
            title: "guest-command".to_string(),
            state: MobileLinuxTaskStateFfi::Cancelled,
            detail: Some("cancelled".to_string()),
        })
    }

    pub async fn open_pty(
        &self,
        request: MobileLinuxPtyOpenRequestFfi,
    ) -> Result<MobileLinuxPtySessionFfi, MobileLinuxOperationFfiError> {
        self.runtime
            .open_pty(pty_request_from_ffi(request))
            .await
            .map(|handle| MobileLinuxPtySessionFfi {
                id: handle.id,
                available: true,
                detail: None,
            })
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn write_pty(
        &self,
        session_id: String,
        data: Vec<u8>,
    ) -> Result<(), MobileLinuxOperationFfiError> {
        self.runtime
            .write_pty(&traits::PtySessionHandle { id: session_id }, data)
            .await
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn resize_pty(
        &self,
        session_id: String,
        cols: u16,
        rows: u16,
    ) -> Result<(), MobileLinuxOperationFfiError> {
        self.runtime
            .resize_pty(
                &traits::PtySessionHandle { id: session_id },
                traits::PtySize { cols, rows },
            )
            .await
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn close_pty(&self, session_id: String) -> Result<(), MobileLinuxOperationFfiError> {
        self.runtime
            .close_pty(&traits::PtySessionHandle { id: session_id })
            .await
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn configure_mounts(
        &self,
        mounts: Vec<MobileLinuxMountSpecFfi>,
    ) -> Result<MobileLinuxStatusFfi, MobileLinuxOperationFfiError> {
        self.runtime
            .configure_mounts(mounts.into_iter().map(mount_spec_from_ffi).collect())
            .await
            .map_err(mobile_linux_error_to_ffi)?;
        self.runtime
            .rootfs_status()
            .await
            .map(status_to_ffi)
            .map_err(mobile_linux_error_to_ffi)
    }
}

#[cfg(feature = "uniffi")]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct IosMobileLinuxCompatKey {
    config: IosMobileLinuxConfigFfi,
    authorization_verified: bool,
}

#[cfg(feature = "uniffi")]
static IOS_MOBILE_LINUX_COMPAT_HANDLES: OnceLock<
    StdMutex<std::collections::HashMap<IosMobileLinuxCompatKey, Arc<IosMobileLinuxRuntimeHandle>>>,
> = OnceLock::new();

#[cfg(feature = "uniffi")]
fn compat_handle(
    config: Option<IosMobileLinuxConfigFfi>,
) -> Result<Arc<IosMobileLinuxRuntimeHandle>, MobileLinuxOperationFfiError> {
    let config = config.ok_or_else(|| MobileLinuxOperationFfiError::Unavailable {
        message: "legacy unavailable backend selected".to_string(),
    })?;
    // The free functions below are retained for source compatibility. PTY and
    // task operations are stateful, so those calls must resolve to the same
    // process-lifetime handle for a given workspace/configuration.
    let handles = IOS_MOBILE_LINUX_COMPAT_HANDLES
        .get_or_init(|| StdMutex::new(std::collections::HashMap::new()));
    let key = IosMobileLinuxCompatKey {
        authorization_verified: mobile_linux_authorization_verified(
            config.authorization_file.as_ref(),
        ),
        config: config.clone(),
    };
    let mut handles = handles
        .lock()
        .map_err(|_| MobileLinuxOperationFfiError::Io {
            message: "iOS mobile-linux compatibility handle cache is poisoned".to_string(),
        })?;
    if let Some(handle) = handles.get(&key) {
        return Ok(handle.clone());
    }
    let handle = create_ios_mobile_linux_runtime(config)?;
    handles.insert(key, handle.clone());
    Ok(handle)
}

/// Boot the selected iOS mobile-linux runtime.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn boot_ios_mobile_linux(
    config: Option<IosMobileLinuxConfigFfi>,
) -> Result<MobileLinuxStatusFfi, MobileLinuxOperationFfiError> {
    let handle = compat_handle(config)?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| MobileLinuxOperationFfiError::Io {
            message: err.to_string(),
        })?;
    rt.block_on(handle.boot())
}

/// Shut down the selected iOS mobile-linux runtime.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn shutdown_ios_mobile_linux(
    config: Option<IosMobileLinuxConfigFfi>,
) -> Result<MobileLinuxStatusFfi, MobileLinuxOperationFfiError> {
    let handle = compat_handle(config)?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| MobileLinuxOperationFfiError::Io {
            message: err.to_string(),
        })?;
    rt.block_on(handle.shutdown())
}

/// Run a one-shot command in the guest runtime.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn run_ios_mobile_linux_command(
    config: Option<IosMobileLinuxConfigFfi>,
    request: MobileLinuxCommandRequestFfi,
) -> Result<MobileLinuxCommandResultFfi, MobileLinuxOperationFfiError> {
    let handle = compat_handle(config)?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| MobileLinuxOperationFfiError::Io {
            message: err.to_string(),
        })?;
    rt.block_on(handle.run_command(request))
}

/// Run a command and stream stdout/stderr to the host sink.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn run_ios_mobile_linux_command_streaming(
    config: Option<IosMobileLinuxConfigFfi>,
    request: MobileLinuxCommandRequestFfi,
    sink: Box<dyn IosMobileLinuxEventSink>,
) -> Result<MobileLinuxCommandResultFfi, MobileLinuxOperationFfiError> {
    let handle = compat_handle(config)?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| MobileLinuxOperationFfiError::Io {
            message: err.to_string(),
        })?;
    rt.block_on(handle.run_command_streaming(request, sink))
}

/// Spawn a background task in the guest runtime.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn spawn_ios_mobile_linux_task(
    config: Option<IosMobileLinuxConfigFfi>,
    request: MobileLinuxCommandRequestFfi,
) -> Result<MobileLinuxTaskFfi, MobileLinuxOperationFfiError> {
    let handle = compat_handle(config)?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| MobileLinuxOperationFfiError::Io {
            message: err.to_string(),
        })?;
    rt.block_on(handle.spawn_task(request))
}

/// List known guest background tasks.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn list_ios_mobile_linux_tasks(
    config: Option<IosMobileLinuxConfigFfi>,
) -> Result<Vec<MobileLinuxTaskFfi>, MobileLinuxOperationFfiError> {
    let handle = compat_handle(config)?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| MobileLinuxOperationFfiError::Io {
            message: err.to_string(),
        })?;
    rt.block_on(handle.list_tasks())
}

/// Kill a guest background task.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn kill_ios_mobile_linux_task(
    config: Option<IosMobileLinuxConfigFfi>,
    task_id: String,
) -> Result<MobileLinuxTaskFfi, MobileLinuxOperationFfiError> {
    let handle = compat_handle(config)?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| MobileLinuxOperationFfiError::Io {
            message: err.to_string(),
        })?;
    rt.block_on(handle.kill_task(task_id))
}

/// Open a PTY session in the guest runtime.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn open_ios_mobile_linux_pty(
    config: Option<IosMobileLinuxConfigFfi>,
    request: MobileLinuxPtyOpenRequestFfi,
) -> Result<MobileLinuxPtySessionFfi, MobileLinuxOperationFfiError> {
    let handle = compat_handle(config)?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| MobileLinuxOperationFfiError::Io {
            message: err.to_string(),
        })?;
    rt.block_on(handle.open_pty(request))
}

/// Poll queued events for a previously spawned task.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn read_ios_mobile_linux_events(
    config: Option<IosMobileLinuxConfigFfi>,
    after_sequence: Option<u64>,
    limit: Option<u32>,
) -> Result<Vec<MobileLinuxStreamEventFfi>, MobileLinuxOperationFfiError> {
    let handle = compat_handle(config)?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| MobileLinuxOperationFfiError::Io {
            message: err.to_string(),
        })?;
    rt.block_on(handle.read_events(after_sequence, limit))
}

/// Read one task snapshot by id.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn ios_mobile_linux_task_status(
    config: Option<IosMobileLinuxConfigFfi>,
    task_id: String,
) -> Result<Option<MobileLinuxTaskFfi>, MobileLinuxOperationFfiError> {
    let handle = compat_handle(config)?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| MobileLinuxOperationFfiError::Io {
            message: err.to_string(),
        })?;
    rt.block_on(handle.task_status(task_id))
}

/// Write bytes to an open PTY session.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn write_ios_mobile_linux_pty(
    config: Option<IosMobileLinuxConfigFfi>,
    session_id: String,
    data: Vec<u8>,
) -> Result<(), MobileLinuxOperationFfiError> {
    let handle = compat_handle(config)?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| MobileLinuxOperationFfiError::Io {
            message: err.to_string(),
        })?;
    rt.block_on(handle.write_pty(session_id, data))
}

/// Resize an open PTY session.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn resize_ios_mobile_linux_pty(
    config: Option<IosMobileLinuxConfigFfi>,
    session_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), MobileLinuxOperationFfiError> {
    let handle = compat_handle(config)?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| MobileLinuxOperationFfiError::Io {
            message: err.to_string(),
        })?;
    rt.block_on(handle.resize_pty(session_id, cols, rows))
}

/// Close an open PTY session.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn close_ios_mobile_linux_pty(
    config: Option<IosMobileLinuxConfigFfi>,
    session_id: String,
) -> Result<(), MobileLinuxOperationFfiError> {
    let handle = compat_handle(config)?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| MobileLinuxOperationFfiError::Io {
            message: err.to_string(),
        })?;
    rt.block_on(handle.close_pty(session_id))
}

/// Replace the current guest mount configuration.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn configure_ios_mobile_linux_mounts(
    config: Option<IosMobileLinuxConfigFfi>,
    mounts: Vec<MobileLinuxMountSpecFfi>,
) -> Result<MobileLinuxStatusFfi, MobileLinuxOperationFfiError> {
    let handle = compat_handle(config)?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| MobileLinuxOperationFfiError::Io {
            message: err.to_string(),
        })?;
    rt.block_on(handle.configure_mounts(mounts))
}

// F3-04: re-export `engine-mobile`'s UniFFI scaffolding so the shared host's FFI
// symbols (the re-exported `MobileEngineHandle` / `MobileEngineError`) land in
// this crate's final library. Under the `uniffi` feature only.
#[cfg(feature = "uniffi")]
uniffi::setup_scaffolding!();

#[cfg(all(test, feature = "uniffi"))]
mod tests {
    /// The root the ios-ish runtime validates local-app mounts against must be
    /// the SAME directory the engine writes local apps into.
    ///
    /// It was not. The engine's data root is whatever Swift's
    /// `appSandboxRoot()` returns — `<AppSupport>/LingxiCode`, which reaches
    /// the engine as `lingxi_home`'s parent — while the runtime INFERRED its
    /// own root from `managed_root` (`<AppSupport>/mobile-linux/ios-ish`) by
    /// cutting everything from `Library/Application Support` onward. The two
    /// answers differ by three components, so `validate_mount` computed an
    /// expected build path that nothing ever writes to and EVERY local-app
    /// build failed on device with "mount host_path must be …".
    ///
    /// The inference cannot be repaired in place: `LingxiCode` is not
    /// derivable from `mobile-linux/ios-ish`. It has to be told.
    #[test]
    fn the_runtime_sandbox_root_is_the_engine_data_root_not_the_container() {
        // The literal shapes both sides produce on device, from
        // `LXISHDefaultWorkspace.managedRootPath()` and
        // `ConversationSourceFactory.appSandboxRoot()`.
        let container = "/private/var/mobile/Containers/Data/Application/203ED8B0";
        let support = format!("{container}/Library/Application Support");
        let engine_data_root = format!("{support}/LingxiCode");

        let config = super::IosMobileLinuxConfigFfi {
            mode: super::MobileLinuxRuntimeModeFfi::MobileLinux,
            managed_root: format!("{support}/mobile-linux/ios-ish"),
            workspace_host_path: format!("{engine_data_root}/workspaces/default"),
            stable_workspace_id: "default".to_string(),
            abi: "arm64".to_string(),
            rootfs_version: "3.20".to_string(),
            archive_sha256: None,
            authorization_file: None,
            app_sandbox_root: engine_data_root.clone(),
        };

        let resolved = super::resolve_mobile_linux_app_sandbox_root(&config)
            .expect("the shipped device paths must resolve");
        assert_eq!(
            resolved,
            std::path::PathBuf::from(&engine_data_root),
            "the runtime must validate mounts against the engine's data root; \
             resolving to {container:?} is what broke every local-app build"
        );

        // The inference this replaced returned exactly `container`. Pinning the
        // rejection keeps a well-meaning "fall back when it's empty" from
        // quietly restoring a second authority on this directory.
        let empty = super::IosMobileLinuxConfigFfi {
            app_sandbox_root: String::new(),
            ..config
        };
        assert!(
            super::resolve_mobile_linux_app_sandbox_root(&empty).is_err(),
            "an absent root must fail loudly, never fall back to a guess"
        );
    }

    /// A task's non-terminal status changes must NEVER surface as stream
    /// events: `event_to_ffi` used to map EVERY `TaskStatusChanged` to
    /// `kind: Exit`, so the `Running` emitted by PTY task CREATION (task id =
    /// session id) closed a freshly opened, healthy terminal with
    /// "[process exited]" on its very first read — and every restart died at
    /// its own creation event the same way.
    #[test]
    fn non_terminal_task_status_events_are_skipped_and_terminal_ones_map_to_exit() {
        let event = |status| traits::MobileLinuxEvent {
            sequence: 1,
            task_id: Some("session-1".to_string()),
            kind: traits::MobileLinuxEventKind::TaskStatusChanged {
                status,
                exit_code: None,
                detail: None,
            },
        };
        for status in [
            traits::MobileLinuxTaskStatus::Queued,
            traits::MobileLinuxTaskStatus::Running,
            traits::MobileLinuxTaskStatus::Backgrounded,
        ] {
            assert!(
                super::event_to_ffi(event(status)).is_none(),
                "{status:?} must not become a stream event"
            );
        }
        let ffi = super::event_to_ffi(event(traits::MobileLinuxTaskStatus::Completed))
            .expect("terminal status maps");
        assert!(matches!(
            ffi.kind,
            super::MobileLinuxStreamEventKindFfi::Exit
        ));
        assert_eq!(ffi.stream_id, "session-1");
        let timed_out = super::event_to_ffi(event(traits::MobileLinuxTaskStatus::TimedOut))
            .expect("terminal status maps");
        assert!(timed_out.timed_out);
    }

    use std::sync::Arc;

    use async_trait::async_trait;
    use client_protocol::events::ClientEvent;
    use client_protocol::permission::PermissionRequest as PermissionRequestDto;
    use engine_mobile::{ClientEventListener, MobileConfig, PermissionRequestSink};
    use tokio::sync::Mutex;
    use traits::{
        CameraControl, Clock, FileSystem, HttpTransport, Platform, ProcessRunner, Sandbox,
        SharingService, VoiceRecorder, WorktreeManager,
    };

    /// Off-device fake [`Platform`] shim (portable `platform-posix-minimal`
    /// handles over a temp root). Lets the SHARED `build_mobile_engine` build a
    /// real handle on CI without an iOS device — exactly the spec §8 "prove from
    /// a Swift unit test", run on the host.
    struct HostFakePlatform {
        fs: Arc<dyn FileSystem>,
        http: Arc<dyn HttpTransport>,
        clock: Arc<dyn Clock>,
        process: Arc<dyn ProcessRunner>,
        sandbox: Arc<dyn Sandbox>,
        worktree: Arc<dyn WorktreeManager>,
    }

    impl HostFakePlatform {
        fn new(root: std::path::PathBuf) -> Self {
            use platform_posix_minimal::{
                PosixClock, PosixFileSystem, PosixHttp, PosixProcess, PosixSandbox, PosixWorktree,
            };
            Self {
                fs: Arc::new(PosixFileSystem::new(root)),
                http: Arc::new(PosixHttp::new()),
                clock: Arc::new(PosixClock::new()),
                process: Arc::new(PosixProcess::new()),
                sandbox: Arc::new(PosixSandbox::new()),
                worktree: Arc::new(PosixWorktree::new()),
            }
        }
    }

    impl Platform for HostFakePlatform {
        fn filesystem(&self) -> Arc<dyn FileSystem> {
            self.fs.clone()
        }
        fn http(&self) -> Arc<dyn HttpTransport> {
            self.http.clone()
        }
        fn clock(&self) -> Arc<dyn Clock> {
            self.clock.clone()
        }
        fn process(&self) -> Arc<dyn ProcessRunner> {
            self.process.clone()
        }
        fn sandbox(&self) -> Arc<dyn Sandbox> {
            self.sandbox.clone()
        }
        fn worktree(&self) -> Arc<dyn WorktreeManager> {
            self.worktree.clone()
        }
        fn camera(&self) -> Option<Arc<dyn CameraControl>> {
            None
        }
        fn voice(&self) -> Option<Arc<dyn VoiceRecorder>> {
            None
        }
        fn share(&self) -> Option<Arc<dyn SharingService>> {
            None
        }
    }

    /// A host-fake [`ClientEventListener`] that records every delivered event.
    #[derive(Default)]
    struct FakeListener {
        received: Mutex<Vec<ClientEvent>>,
    }

    #[async_trait]
    impl ClientEventListener for FakeListener {
        async fn on_event(&self, event: ClientEvent) {
            self.received.lock().await.push(event);
        }
    }

    /// A [`PermissionRequestSink`] that records the gate's outbound requests.
    #[derive(Default)]
    struct RecordingPermissionSink {
        count: std::sync::atomic::AtomicUsize,
    }

    #[async_trait]
    impl PermissionRequestSink for RecordingPermissionSink {
        async fn emit_request(&self, _request: PermissionRequestDto) {
            self.count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    fn build_handle(root: &std::path::Path) -> Arc<engine_mobile::MobileEngineHandle> {
        let platform: Arc<dyn Platform> = Arc::new(HostFakePlatform::new(root.to_path_buf()));
        let listener: Arc<dyn ClientEventListener> = Arc::new(FakeListener::default());
        let perm_sink: Arc<dyn PermissionRequestSink> =
            Arc::new(RecordingPermissionSink::default());
        let cfg = MobileConfig {
            cwd: root.to_path_buf(),
            lingxi_home: root.join(".lingxi"),
            ..MobileConfig::default()
        };
        engine_mobile::build_mobile_engine(cfg, platform, listener, perm_sink)
            .expect("shared build_mobile_engine failed")
    }

    /// F3-04: the re-exported [`MobileEngineHandle`] holds the handle-owned tokio
    /// runtime AND the registered listener AND the connection-scoped permission
    /// gate — the grown-up form of the M8 stub (which held only a `Platform` +
    /// `skill_count`).
    #[test]
    fn handle_holds_runtime_and_listener() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let handle = build_handle(tmp.path());

        // Owns a live tokio runtime — drive a trivial future on it to prove it.
        let two = handle.runtime().block_on(async { 1 + 1 });
        assert_eq!(two, 2);

        // Holds the wired runtime: the orchestrator + the connection-scoped
        // adapter permission gate + the registered listener are all reachable.
        let _orch: Arc<orchestrator::ConversationOrchestrator> =
            handle.inner().orchestrator.clone();
        let _gate = handle.permission_gate();
        let _listener: Arc<dyn ClientEventListener> = handle.listener();

        // The M8 smoke signal reflects the Rust-bundled mobile skill catalog.
        // `create-local-app` is always present so the agent can enter the
        // template-guided, approval-gated local-app workflow offline.
        assert_eq!(handle.skill_count(), 1);
    }

    /// F3-04: `create_session` is no longer the M8 stub (which returned an
    /// `Internal` error). With a real wired runtime it returns the connection's
    /// live session ref.
    #[test]
    fn create_session_no_longer_stubbed() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let handle = build_handle(tmp.path());

        let session = handle
            .create_session("claude-sonnet-4-20250514".to_string())
            .expect("create_session must no longer be stubbed");
        assert_eq!(session, 1);
    }

    #[test]
    fn mobile_linux_legacy_status_reports_the_posix_stub_backend() {
        let status =
            super::ios_mobile_linux_status_from_config(Some(&super::IosMobileLinuxConfigFfi {
                mode: super::MobileLinuxRuntimeModeFfi::Legacy,
                managed_root: "/tmp/mobile-linux".to_string(),
                workspace_host_path: "/tmp/workspaces/default".to_string(),
                stable_workspace_id: "default".to_string(),
                abi: "arm64".to_string(),
                rootfs_version: "v1".to_string(),
                archive_sha256: None,
                authorization_file: None,
                app_sandbox_root: "/tmp".to_string(),
            }));

        assert!(matches!(
            status.mode,
            super::MobileLinuxRuntimeModeFfi::Legacy
        ));
        assert_eq!(status.backend, "ios-posix");
    }

    #[test]
    fn mobile_linux_command_api_reports_unavailable_on_host_without_device_bridge() {
        let err = super::run_ios_mobile_linux_command(
            Some(super::IosMobileLinuxConfigFfi {
                mode: super::MobileLinuxRuntimeModeFfi::MobileLinux,
                managed_root: "/tmp/mobile-linux".to_string(),
                workspace_host_path: "/tmp/workspaces/default".to_string(),
                stable_workspace_id: "default".to_string(),
                abi: "arm64".to_string(),
                rootfs_version: "v1".to_string(),
                archive_sha256: None,
                authorization_file: None,
                app_sandbox_root: "/tmp".to_string(),
            }),
            super::MobileLinuxCommandRequestFfi {
                command: "/bin/sh".to_string(),
                args: vec!["-lc".to_string(), "echo hi".to_string()],
                cwd: None,
                env: std::collections::HashMap::new(),
                stdin: None,
                timeout_ms: Some(1000),
                network: super::MobileLinuxNetworkPolicyFfi::Allowed,
                mounts: vec![],
            },
        )
        .expect_err("command should fail on host without the device bridge");

        assert!(matches!(
            err,
            super::MobileLinuxOperationFfiError::Unavailable { .. }
        ));
    }

    #[test]
    fn mobile_linux_handle_reuses_one_runtime_instance_for_multiple_calls() {
        let handle = super::create_ios_mobile_linux_runtime(super::IosMobileLinuxConfigFfi {
            mode: super::MobileLinuxRuntimeModeFfi::MobileLinux,
            managed_root: "/tmp/mobile-linux".to_string(),
            workspace_host_path: "/tmp/workspaces/default".to_string(),
            stable_workspace_id: "default".to_string(),
            abi: "arm64".to_string(),
            rootfs_version: "v1".to_string(),
            archive_sha256: None,
            authorization_file: None,
            app_sandbox_root: "/tmp".to_string(),
        })
        .expect("handle");

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");

        let first = rt.block_on(handle.status()).expect("first status");
        let second = rt.block_on(handle.status()).expect("second status");
        assert_eq!(first.last_error, second.last_error);
        assert_eq!(first.backend, second.backend);

        let err = rt
            .block_on(handle.open_pty(super::MobileLinuxPtyOpenRequestFfi {
                command: "/bin/sh".to_string(),
                args: vec![],
                cwd: Some("/workspace/default".to_string()),
                env: std::collections::HashMap::new(),
                cols: 80,
                rows: 24,
                mounts: vec![],
            }))
            .expect_err("pty must be unavailable on host/simulator");

        assert!(matches!(
            err,
            super::MobileLinuxOperationFfiError::Unavailable { .. }
        ));
    }

    #[test]
    fn mobile_linux_compat_functions_reuse_handle_for_same_config() {
        let config = super::IosMobileLinuxConfigFfi {
            mode: super::MobileLinuxRuntimeModeFfi::MobileLinux,
            managed_root: "/tmp/mobile-linux-compat".to_string(),
            workspace_host_path: "/tmp/workspaces/compat".to_string(),
            stable_workspace_id: "compat".to_string(),
            abi: "arm64".to_string(),
            rootfs_version: "v1".to_string(),
            archive_sha256: None,
            authorization_file: None,
            app_sandbox_root: "/tmp".to_string(),
        };

        let first = super::compat_handle(Some(config.clone())).expect("first compat handle");
        let second = super::compat_handle(Some(config)).expect("second compat handle");

        assert!(Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn mobile_linux_workspace_id_rejects_guest_path_components() {
        let config = super::IosMobileLinuxConfigFfi {
            mode: super::MobileLinuxRuntimeModeFfi::MobileLinux,
            managed_root: "/tmp/lingxi-app/mobile-linux".to_string(),
            workspace_host_path: "/tmp/lingxi-app/workspaces/project".to_string(),
            stable_workspace_id: "..".to_string(),
            abi: "arm64".to_string(),
            rootfs_version: "v1".to_string(),
            archive_sha256: None,
            authorization_file: None,
            app_sandbox_root: "/tmp/lingxi-app".to_string(),
        };

        assert!(matches!(
            super::validate_mobile_linux_workspace_config("/tmp/lingxi-app", &config),
            Err(super::MobileLinuxOperationFfiError::InvalidRequest { .. })
        ));
    }

    #[test]
    fn mobile_linux_workspace_rejects_parent_traversal_into_protected_roots() {
        let temp = tempfile::tempdir().expect("tempdir");
        let app_root = temp.path().join("app");
        let managed_root = app_root.join("mobile-linux");
        std::fs::create_dir_all(app_root.join(".lingxi")).expect("create protected root");
        std::fs::create_dir_all(&managed_root).expect("create managed root");

        for workspace_host_path in [
            app_root.join("workspaces/default/../.."),
            app_root.join("workspaces/default/../../.lingxi/state"),
            app_root.join("workspaces/default/../../mobile-linux/rootfs"),
            app_root.join("workspaces/default/../../providers/credentials"),
        ] {
            let config = super::IosMobileLinuxConfigFfi {
                mode: super::MobileLinuxRuntimeModeFfi::MobileLinux,
                managed_root: managed_root.to_string_lossy().into_owned(),
                workspace_host_path: workspace_host_path.to_string_lossy().into_owned(),
                stable_workspace_id: "default".to_string(),
                abi: "arm64".to_string(),
                rootfs_version: "v1".to_string(),
                archive_sha256: None,
                authorization_file: None,
                app_sandbox_root: app_root.to_string_lossy().into_owned(),
            };

            assert!(matches!(
                super::validate_mobile_linux_workspace_config(
                    app_root.to_str().expect("utf8 app root"),
                    &config
                ),
                Err(super::MobileLinuxOperationFfiError::InvalidRequest { .. })
            ));
        }
    }

    #[cfg(unix)]
    #[test]
    fn mobile_linux_workspace_rejects_symlink_aliases_to_protected_roots() {
        let temp = tempfile::tempdir().expect("tempdir");
        let app_root = temp.path().join("app");
        let managed_root = app_root.join("mobile-linux");
        let providers_root = app_root.join("providers");
        let aliases_root = app_root.join("workspaces");
        std::fs::create_dir_all(app_root.join(".lingxi")).expect("create lingxi root");
        std::fs::create_dir_all(&managed_root).expect("create managed root");
        std::fs::create_dir_all(&providers_root).expect("create providers root");
        std::fs::create_dir_all(&aliases_root).expect("create aliases root");

        for (name, destination) in [
            ("lingxi-link", app_root.join(".lingxi")),
            ("managed-link", managed_root.clone()),
            ("provider-link", providers_root),
        ] {
            let alias = aliases_root.join(name);
            std::os::unix::fs::symlink(destination, &alias).expect("create protected alias");
            let config = super::IosMobileLinuxConfigFfi {
                mode: super::MobileLinuxRuntimeModeFfi::MobileLinux,
                managed_root: managed_root.to_string_lossy().into_owned(),
                workspace_host_path: alias.join("workspace").to_string_lossy().into_owned(),
                stable_workspace_id: "default".to_string(),
                abi: "arm64".to_string(),
                rootfs_version: "v1".to_string(),
                archive_sha256: None,
                authorization_file: None,
                app_sandbox_root: app_root.to_string_lossy().into_owned(),
            };

            assert!(matches!(
                super::validate_mobile_linux_workspace_config(
                    app_root.to_str().expect("utf8 app root"),
                    &config
                ),
                Err(super::MobileLinuxOperationFfiError::InvalidRequest { .. })
            ));
        }
    }

    #[test]
    fn standalone_mobile_linux_runtime_applies_workspace_boundary_validation() {
        let temp = tempfile::tempdir().expect("tempdir");
        let app_root = temp.path().join("app");
        let managed_root = app_root.join("mobile-linux");
        std::fs::create_dir_all(app_root.join(".lingxi")).expect("create protected root");
        std::fs::create_dir_all(&managed_root).expect("create managed root");
        let config = super::IosMobileLinuxConfigFfi {
            mode: super::MobileLinuxRuntimeModeFfi::MobileLinux,
            managed_root: managed_root.to_string_lossy().into_owned(),
            workspace_host_path: app_root
                .join("workspaces/default/../../.lingxi/state")
                .to_string_lossy()
                .into_owned(),
            stable_workspace_id: "default".to_string(),
            abi: "arm64".to_string(),
            rootfs_version: "v1".to_string(),
            archive_sha256: None,
            authorization_file: None,
            app_sandbox_root: app_root.to_string_lossy().into_owned(),
        };

        assert!(matches!(
            super::create_ios_mobile_linux_runtime(config),
            Err(super::MobileLinuxOperationFfiError::InvalidRequest { .. })
        ));
    }

    #[test]
    fn ios_project_cwd_accepts_managed_project_and_local_app_workspaces() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("lingxi-ios-project-{nonce}"));
        let project_id = "12345678-1234-4abc-8def-1234567890ab";
        let workspace = root.join("Projects").join(project_id).join("workspace");
        std::fs::create_dir_all(&workspace).expect("create project fixture");

        let legacy = super::ios_project_cwd(root.to_str().expect("utf8"), None)
            .expect("missing project cwd preserves the legacy sandbox root");
        assert_eq!(legacy, root.canonicalize().expect("canonical root"));

        let resolved = super::ios_project_cwd(
            root.to_str().expect("utf8"),
            Some(workspace.to_str().expect("utf8")),
        )
        .expect("managed project workspace is accepted");
        assert_eq!(
            resolved,
            workspace.canonicalize().expect("canonical workspace")
        );

        // v3 local apps: `apps/<engine-minted id>/workspace` is a first-class
        // conversation scope — the exact cwd the client hands `prepare()` when
        // it jumps into a freshly created app's init session.
        let app_workspace = root.join("apps").join("9b48dfb5").join("workspace");
        std::fs::create_dir_all(&app_workspace).expect("create app fixture");
        let resolved_app = super::ios_project_cwd(
            root.to_str().expect("utf8"),
            Some(app_workspace.to_str().expect("utf8")),
        )
        .expect("local app workspace is accepted");
        assert_eq!(
            resolved_app,
            app_workspace.canonicalize().expect("canonical app workspace")
        );

        let illegal_app = root.join("apps").join("Bad_ID").join("workspace");
        std::fs::create_dir_all(&illegal_app).expect("create illegal-app fixture");
        assert!(
            super::ios_project_cwd(
                root.to_str().expect("utf8"),
                Some(illegal_app.to_str().expect("utf8")),
            )
            .is_err(),
            "ids the engine could never mint must not become conversation workspaces"
        );

        let malformed = root.join("Projects").join("user-name").join("workspace");
        std::fs::create_dir_all(&malformed).expect("create malformed fixture");
        assert!(
            super::ios_project_cwd(
                root.to_str().expect("utf8"),
                Some(malformed.to_str().expect("utf8")),
            )
            .is_err(),
            "user-controlled names must never become project directories"
        );

        let outside = std::env::temp_dir().join(format!("lingxi-ios-outside-project-{nonce}"));
        std::fs::create_dir_all(&outside).expect("create outside fixture");
        assert!(
            super::ios_project_cwd(
                root.to_str().expect("utf8"),
                Some(outside.to_str().expect("utf8")),
            )
            .is_err(),
            "workspace must stay under the app sandbox"
        );
    }

    #[test]
    fn ios_launch_config_parses_provider_json_and_project_scope() {
        let temp = tempfile::tempdir().expect("tempdir");
        let project_id = "12345678-1234-4abc-8def-1234567890ab";
        let workspace = temp
            .path()
            .join("Projects")
            .join(project_id)
            .join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");

        let cfg = super::ios_mobile_config_from_launch_config(&super::IosEngineLaunchConfigFfi {
            api_base: "https://example.invalid".to_string(),
            api_key: "sk-test".to_string(),
            model: "claude-test".to_string(),
            app_sandbox_root: temp.path().to_string_lossy().into_owned(),
            project_cwd: Some(workspace.to_string_lossy().into_owned()),
            provider_config: Some(super::IosProviderConfigFfi {
                provider_profiles_json:
                    r#"{"openai":{"baseUrl":"https://api.openai.com/v1","wireApi":"responses"}}"#
                        .to_string(),
                routing_json: Some(r#"{"default":"openai"}"#.to_string()),
            }),
            mobile_linux: None,
            local_apps_full_runtime: false,
            local_apps_runtime_root: None,
            physical_memory_bytes: 7 * 1024_u64.pow(3),
        })
        .expect("launch config");

        assert_eq!(
            cfg.cwd,
            workspace.canonicalize().expect("canonical workspace")
        );
        assert_eq!(cfg.api_base, "https://example.invalid");
        assert_eq!(cfg.api_key, "sk-test");
        assert_eq!(cfg.default_model, "claude-test");
        assert_eq!(cfg.physical_memory_bytes, 7 * 1024_u64.pow(3));
        assert_eq!(
            cfg.lingxi_home,
            temp.path().join(branding::DOT_DIR),
            "global state remains rooted at the sandbox"
        );
        let providers = cfg.provider_profiles.expect("provider profiles");
        assert!(providers.contains_key("openai"));
        assert_eq!(
            cfg.routing.expect("routing"),
            serde_json::json!({ "default": "openai" })
        );
    }

    #[tokio::test]
    async fn build_ios_cron_store_round_trips_without_engine() {
        let temp = tempfile::tempdir().expect("tempdir");
        let project_id = "12345678-1234-4abc-8def-1234567890ab";
        let workspace = temp
            .path()
            .join("Projects")
            .join(project_id)
            .join("workspace");
        std::fs::create_dir_all(temp.path().join(branding::DOT_DIR)).expect("state dir");
        std::fs::create_dir_all(&workspace).expect("workspace");

        let store = super::build_ios_cron_store(
            temp.path().to_string_lossy().into_owned(),
            Some(workspace.to_string_lossy().into_owned()),
        )
        .expect("build cron store");

        let created = store
            .create("* * * * *".to_string(), "hello".to_string(), false)
            .await
            .expect("one-shot creation");
        let updated = store
            .update(
                created.id.clone(),
                "*/15 * * * *".to_string(),
                "updated".to_string(),
                true,
            )
            .await
            .expect("recurring update");
        let due = store
            .due_occurrences(updated.next_fire_ms.expect("next fire").saturating_add(1))
            .await;
        assert_eq!(vec![created.id.clone()], vec![due[0].task_id.clone()]);
        assert_eq!(
            Some(updated.next_fire_ms.expect("next fire")),
            store.next_fire_time().await
        );
        assert!(store.delete(created.id).await);
        assert!(store.list().await.is_empty());
    }
}

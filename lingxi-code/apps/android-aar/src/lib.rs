//! `android-aar` (M8-P12 → M10-F3) — the Android `UniFFI` packager.
//!
//! The FFI boundary between the Rust engine and the Android app. The Kotlin
//! layer implements the [`traits::CameraControl`] / [`traits::VoiceRecorder`] /
//! [`traits::SharingService`] callback interfaces (skeletons under `kotlin/`),
//! hands them across as a [`PlatformImpls`] record, and Rust uses them to build
//! an `AndroidPlatform` and assemble the mobile engine — Rust calls *back* into
//! Kotlin for native capabilities.
//!
//! ## Shared session host (F3-04)
//!
//! The real session host — [`MobileEngineHandle`] (owns the handle-owned tokio
//! runtime + the wired `MobileRuntime` + the registered `ClientEventListener`)
//! and its [`MobileEngineError`] — lives in `engine-mobile` and is RE-EXPORTED
//! here, NOT re-derived. That single-source rule (plan F3-04) is what stops iOS
//! and Android from drifting: this crate only adds the Android-specific
//! `Platform`-construction wrapper around the shared `build_mobile_engine`.
//!
//! ## Inbound command path (F3-05)
//!
//! The async FFI entry point — `MobileEngineHandle::submit(ClientCommand) ->
//! Result<(), ClientError>` (under `uniffi`: `#[uniffi::export(async_runtime =
//! "tokio")]`) — is defined ONCE on the shared host in `engine-mobile` and
//! reaches Kotlin through the re-exported [`MobileEngineHandle`]. There is no
//! Android-specific submit body: `SendPrompt` spawns the streaming turn on the
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
//! engine host owning one runtime) — so Kotlin's `suspend` calls never block the
//! main thread and resolve on the engine's own runtime. The
//! `async_submit_resolves_on_handle_runtime` host test proves the registration by
//! asserting an async export resolves on exactly that runtime.
//!
//! ## `UniFFI` status
//! The `uniffi` feature (default-on) lights up the real `UniFFI` surface: the
//! re-exported [`MobileEngineHandle`] is a `#[derive(uniffi::Object)]`, the
//! listener a callback interface, the DTOs `UniFFI` types. `engine-mobile` carries
//! the `setup_scaffolding!()`; this crate re-exports it (and adds its own for
//! the Android-local exports) so the symbols land in the final library.

#![forbid(unsafe_code)]

#[cfg(feature = "uniffi")]
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
#[cfg(feature = "uniffi")]
use traits::mobile_linux::MAX_MOBILE_LINUX_EVENT_BATCH;
use traits::{CameraControl, SharingService, VoiceRecorder};
// `Platform` is named only inside the `cfg(target_os = "android")` constructor
// body; importing it unconditionally warns on the host build, so scope it.
#[cfg(all(feature = "uniffi", target_os = "android"))]
use traits::Platform;

// F3-04: the shared session host + its error type are DEFINED ONCE in
// `engine-mobile` and re-exported here. Both FFI packager crates re-export the
// SAME types so iOS and Android cannot drift (plan F3-04).
#[cfg(feature = "uniffi")]
pub use engine_mobile::{
    ClientEventListener, CronDueOccurrenceDto, CronFireStatusDto, CronTaskDto, FiredCronJobDto,
    MobileConfig, MobileCronStoreHandle, MobileEngineError, MobileEngineHandle,
    PermissionRequestSink, ProviderConnectionTestDto,
};

/// The foreign (Kotlin) capability objects + config needed to build an
/// `AndroidPlatform`. `UniFFI` marshals each `Arc<dyn …>` as a callback-interface
/// reference; `app_files_root` is the app's private files-dir.
pub struct PlatformImpls {
    /// Kotlin `CameraControl` impl (`CameraX`).
    pub camera: Arc<dyn CameraControl>,
    /// Kotlin `VoiceRecorder` impl (`MediaRecorder`).
    pub voice: Arc<dyn VoiceRecorder>,
    /// Kotlin `SharingService` impl (`Intent.ACTION_SEND`).
    pub share: Arc<dyn SharingService>,
    /// The app's private files-dir root.
    pub app_files_root: String,
    /// Optional mobile-linux runtime configuration.
    pub mobile_linux: Option<AndroidMobileLinuxConfigFfi>,
}

/// FFI carrier for the Android shell/sandbox configuration (spec r3 §Android
/// inputs). `None` anywhere upstream keeps shell support fully absent.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct AndroidShellConfigFfi {
    /// `ApplicationInfo.nativeLibraryDir`.
    pub native_library_dir: String,
    /// Directory the shell treats as `$HOME` / workspace.
    pub shell_workspace_root: String,
    /// App cache dir (`$TMPDIR`).
    pub app_cache_root: String,
    /// Application package name.
    pub package_name: String,
    /// `PackageInfo.longVersionCode`.
    pub package_version_code: i64,
    /// filesDir / cacheDir / codeCacheDir / noBackupFilesDir roots.
    pub app_writable_roots: Vec<String>,
    /// Master enable flag.
    pub enable_shell: bool,
    /// D11: host attests secrets are Keystore-backed.
    pub secrets_in_keystore: bool,
    /// D11: explicit user acceptance of data exposure.
    pub shell_data_exposure_accepted: bool,
}

/// FFI carrier for the Android `Git`-tool configuration (spec P4 §G5 gate +
/// §G3 auth). `None`/`null` anywhere upstream keeps Git support fully absent.
///
/// Mirrors [`AndroidShellConfigFfi`]: the Kotlin host supplies the enable flag,
/// the repository workspace root, and the system CA-certificate directory. The
/// secrets themselves (HTTPS token, SSH passphrase) NO LONGER ride this record —
/// they are fetched per-op through the [`AndroidGitCredentialProvider`] callback
/// so no plaintext secret is held resident in the engine between ops.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct AndroidGitConfigFfi {
    /// Master enable flag for the Git tool.
    pub enable_git: bool,
    /// App-private repository root (absolute path); all git ops are anchored here.
    pub workspace_root: String,
    /// System CA-certificate directory for TLS verification. Empty = use the
    /// libgit2/OpenSSL defaults.
    pub ca_cert_dir: String,
    /// Filesystem path to the SSH private key (spec §G7), or empty for
    /// HTTPS-only. Host-supplied; validated to stay inside `app_files_root`
    /// before reaching the engine secret seam (defense-in-depth — see
    /// [`build_android_engine`]'s git mapping).
    pub ssh_private_key_path: String,
    /// Path to the matching SSH public key, or empty (libssh2 derives it from
    /// the private key).
    pub ssh_public_key_path: String,
    /// Pinned SSH host-key fingerprints (lowercase-hex SHA-256). An empty list
    /// rejects every host key (fail-closed).
    pub ssh_known_hosts_sha256_hex: Vec<String>,
}

/// Mobile Linux runtime mode exposed to the Android host.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[derive(Debug, Clone, Copy)]
pub enum MobileLinuxRuntimeModeFfi {
    Legacy,
    MobileLinux,
}

impl From<MobileLinuxRuntimeModeFfi> for traits::MobileLinuxRuntimeMode {
    fn from(value: MobileLinuxRuntimeModeFfi) -> Self {
        match value {
            MobileLinuxRuntimeModeFfi::Legacy => Self::Legacy,
            MobileLinuxRuntimeModeFfi::MobileLinux => Self::MobileLinux,
        }
    }
}

/// FFI carrier for Android mobile-linux configuration.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct AndroidMobileLinuxConfigFfi {
    /// Legacy backend vs mobile-linux backend selection.
    pub mode: MobileLinuxRuntimeModeFfi,
    /// App-private root where rootfs state is managed.
    pub managed_root: String,
    /// Host workspace root exposed to the guest, if any.
    pub workspace_host_path: Option<String>,
    /// Stable workspace identifier used in the guest mount path.
    pub stable_workspace_id: Option<String>,
    /// ABI name for the rootfs payload (`arm64-v8a`, `x86_64`).
    pub abi: String,
    /// Expected rootfs version label.
    pub rootfs_version: String,
    /// Expected rootfs archive sha256, if known.
    pub archive_sha256: Option<String>,
    /// Retained for wire compatibility with phase-1 hosts. GPL distribution is
    /// now an explicit Android product decision, so this path is informational
    /// and is no longer a runtime capability gate.
    pub authorization_file: Option<String>,
}

/// Android-provided multi-provider configuration for the mobile engine.
///
/// The JSON strings contain non-secret provider/routing settings only. API
/// keys are sent separately through `SetProviderCredential` and persist in the
/// injected encrypted `AndroidSecureStorage` implementation.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct AndroidProviderConfigFfi {
    /// JSON object matching the shared `settings.providers` schema.
    pub provider_profiles_json: String,
    /// Optional JSON value matching the shared `settings.routing` schema.
    pub routing_json: Option<String>,
}

/// Compact launch configuration for the extended Android engine constructor.
///
/// Keeping the scalar and record inputs behind one FFI record is intentional:
/// the Android JNA bridge has to pass UniFFI `RustBuffer` values by value, and
/// the former flat constructor exceeded the reliable arm64 calling surface
/// once provider configuration was added.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct AndroidEngineLaunchConfigFfi {
    pub api_base: String,
    pub api_key: String,
    pub model: String,
    pub app_files_root: String,
    /// Optional Android Project workspace. When present it must resolve to
    /// `<app_files_root>/projects/<lowercase UUID>/workspace`; global `.lingxi`
    /// state continues to live under `app_files_root`.
    pub project_cwd: Option<String>,
    pub provider_config: Option<AndroidProviderConfigFfi>,
    pub mobile_linux: Option<AndroidMobileLinuxConfigFfi>,
    /// Compile-time distribution mode: false for Play, true for Direct.
    pub local_apps_full_runtime: bool,
    /// Verified read-only local-app runtime bundle staged under app files.
    pub local_apps_runtime_root: Option<String>,
    /// Device physical memory reported by the Android host.
    pub physical_memory_bytes: u64,
}

#[cfg(feature = "uniffi")]
fn android_project_cwd(
    app_files_root: &str,
    project_cwd: Option<&str>,
) -> Result<std::path::PathBuf, MobileEngineError> {
    let app_root = std::path::Path::new(app_files_root)
        .canonicalize()
        .map_err(|error| {
            MobileEngineError::Internal(format!("Android app files root is unavailable: {error}"))
        })?;
    let Some(project_cwd) = project_cwd else {
        return Ok(app_root);
    };
    let workspace = std::path::Path::new(project_cwd)
        .canonicalize()
        .map_err(|error| {
            MobileEngineError::Internal(format!(
                "Android Project workspace is unavailable: {error}"
            ))
        })?;
    let relative = workspace.strip_prefix(&app_root).map_err(|_| {
        MobileEngineError::Internal(
            "Android Project workspace must remain inside the app files directory".to_string(),
        )
    })?;
    let components: Vec<_> = relative
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .collect();
    let valid = components.len() == 3
        && components[0] == "projects"
        && is_lowercase_uuid(components[1])
        && components[2] == "workspace"
        && workspace.is_dir();
    if !valid {
        return Err(MobileEngineError::Internal(
            "Android Project workspace must match filesDir/projects/<lowercase UUID>/workspace"
                .to_string(),
        ));
    }
    Ok(workspace)
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

/// FFI capability snapshot for the Android mobile-linux runtime.
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

/// FFI rootfs status snapshot for the Android mobile-linux runtime.
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

/// FFI mount purpose for the mobile-linux runtime.
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

/// FFI mount descriptor for guest-visible paths.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct MobileLinuxMountSpecFfi {
    pub host_path: String,
    pub guest_path: String,
    pub read_only: bool,
    pub purpose: MobileLinuxMountPurposeFfi,
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct MobileLinuxEnvEntryFfi {
    pub key: String,
    pub value: String,
}

/// FFI request for a command executed inside the mobile-linux runtime.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct MobileLinuxCommandRequestFfi {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub env: Vec<MobileLinuxEnvEntryFfi>,
    pub stdin: Option<String>,
    pub timeout_ms: Option<u64>,
    pub allow_network: bool,
    pub mounts: Vec<MobileLinuxMountSpecFfi>,
}

/// FFI command result returned by the mobile-linux runtime.
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

/// FFI background-process handle.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct MobileLinuxProcessHandleFfi {
    pub id: String,
}

/// FFI PTY size.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone, Copy)]
pub struct MobileLinuxPtySizeFfi {
    pub cols: u16,
    pub rows: u16,
}

/// FFI PTY open request.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct MobileLinuxPtyOpenRequestFfi {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub env: Vec<MobileLinuxEnvEntryFfi>,
    pub size: MobileLinuxPtySizeFfi,
    pub mounts: Vec<MobileLinuxMountSpecFfi>,
}

/// FFI PTY-session handle.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct MobileLinuxPtySessionHandleFfi {
    pub id: String,
}

/// FFI stream sink for live command output.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait AndroidMobileLinuxStreamSink: Send + Sync {
    async fn stdout_line(&self, line: String) -> Result<(), MobileLinuxApiErrorFfi>;
    async fn stderr_chunk(&self, chunk: Vec<u8>) -> Result<(), MobileLinuxApiErrorFfi>;
}

/// FFI error surface for mobile-linux operations.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Error))]
#[derive(Debug, thiserror::Error)]
pub enum MobileLinuxApiErrorFfi {
    #[error("legacy backend selected")]
    LegacySelected,
    #[error("mobile-linux runtime unavailable: {message}")]
    Unavailable { message: String },
    #[error("mobile-linux runtime license blocked: {message}")]
    LicenseBlocked { message: String },
    #[error("invalid mobile-linux request: {message}")]
    InvalidRequest { message: String },
    #[error("mobile-linux operation failed: {message}")]
    OperationFailed { message: String },
}

/// Event kind emitted by run/PTY streaming APIs.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[derive(Debug, Clone, Copy)]
pub enum MobileLinuxEventKindFfi {
    TaskStatusChanged,
    StdoutLine,
    StderrChunk,
    PtyOutput,
    PtyClosed,
    RuntimeError,
}

/// Streaming/runtime event delivered to the Android host.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct MobileLinuxEventFfi {
    pub sequence: u64,
    pub task_id: Option<String>,
    pub kind: MobileLinuxEventKindFfi,
    pub data: Option<Vec<u8>>,
    pub text: Option<String>,
    pub session_id: Option<String>,
    pub status: Option<MobileLinuxTaskStateFfi>,
    pub exit_code: Option<i32>,
    pub timed_out: Option<bool>,
    pub cancelled: Option<bool>,
    pub detail: Option<String>,
}

/// Crate-local callback interface for mobile-linux events.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait AndroidMobileLinuxEventSink: Send + Sync {
    async fn on_event(&self, event: MobileLinuxEventFfi) -> Result<(), MobileLinuxApiErrorFfi>;
}

/// FFI task kind for host-visible runtime work.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[derive(Debug, Clone, Copy)]
pub enum MobileLinuxTaskKindFfi {
    Command,
    PtySession,
}

/// FFI task status for host-visible runtime work.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[derive(Debug, Clone, Copy)]
pub enum MobileLinuxTaskStateFfi {
    Queued,
    Running,
    Backgrounded,
    Completed,
    Failed,
    Cancelled,
    TimedOut,
}

/// FFI task snapshot. Phase-1 keeps this fail-closed unless a real runtime is linked.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct MobileLinuxTaskSnapshotFfi {
    pub task_id: String,
    pub status: MobileLinuxTaskStateFfi,
    pub command: String,
    pub started_at_ms: Option<u64>,
    pub finished_at_ms: Option<u64>,
    pub exit_code: Option<i32>,
    pub detail: Option<String>,
}

/// Persistent Android mobile-linux runtime handle. Holds one runtime instance so
/// PTY/task/event state survives across FFI calls.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Object))]
pub struct AndroidMobileLinuxRuntimeHandle {
    runtime: Arc<dyn traits::MobileLinuxRuntime>,
}

#[cfg(feature = "uniffi")]
static MOBILE_LINUX_FFI_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Top-level `UniFFI` constructor: build the mobile engine from the Kotlin-supplied
/// platform callbacks + event listener. (Under `uniffi`: `#[uniffi::export]`.)
///
/// This is a THIN wrapper: it constructs the Android-specific `Platform` from the
/// foreign callbacks and then delegates ALL runtime/adapter/listener wiring to
/// the shared [`engine_mobile::build_mobile_engine`] (F3-04) — so the heavy
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
            cwd: std::path::PathBuf::from(&impls.app_files_root),
            lingxi_home: std::path::PathBuf::from(&impls.app_files_root).join(branding::DOT_DIR),
            // P0.2: production injects the real LINGXI.md hierarchy provider so the
            // orchestrator loads `<cwd>/LINGXI.md` + `<lingxi_home>/LINGXI.md` into
            // its system prompt and `fire_instructions_loaded()` fires over them.
            memory_provider: Some(orchestrator::prompt::real_provider()),
            ..MobileConfig::default()
        };
        let mobile_linux_mode = impls
            .mobile_linux
            .as_ref()
            .map_or(traits::MobileLinuxRuntimeMode::Legacy, |cfg| {
                cfg.mode.into()
            });
        let platform: Arc<dyn Platform> = Arc::new(AndroidPlatform::new_with_mode(
            AndroidPlatformInputs {
                app_files_root: std::path::PathBuf::from(impls.app_files_root),
                camera: impls.camera,
                voice: impls.voice,
                location: None,
                share: impls.share,
                stt: None,
                tts: None,
                notifications: None,
                clipboard: None,
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
        engine_mobile::build_mobile_engine(cfg, platform, listener, permission_sink)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (impls, listener, permission_sink);
        Err(MobileEngineError::PlatformUnavailable)
    }
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn build_android_mobile_linux_runtime_handle(
    config: AndroidMobileLinuxConfigFfi,
) -> Result<Arc<AndroidMobileLinuxRuntimeHandle>, MobileLinuxApiErrorFfi> {
    let runtime = require_mobile_linux_runtime(Some(&config))?;
    Ok(Arc::new(AndroidMobileLinuxRuntimeHandle { runtime }))
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
fn configured_workspace_guest_path(cfg: &AndroidMobileLinuxConfigFfi) -> String {
    let workspace_id = cfg
        .stable_workspace_id
        .as_deref()
        .unwrap_or("default")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect::<String>();
    let workspace_id = if workspace_id.is_empty() {
        "default".to_string()
    } else {
        workspace_id
    };
    format!("/workspace/{workspace_id}")
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
fn mount_purpose_to_traits(value: MobileLinuxMountPurposeFfi) -> traits::MountPurpose {
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
fn command_request_to_traits(
    request: MobileLinuxCommandRequestFfi,
) -> Result<traits::LinuxCommandRequest, MobileLinuxApiErrorFfi> {
    let command = request.command.trim();
    if command.is_empty() {
        return Err(MobileLinuxApiErrorFfi::InvalidRequest {
            message: "command must not be empty".to_string(),
        });
    }
    let mounts = request
        .mounts
        .into_iter()
        .map(mount_spec_to_traits)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(traits::LinuxCommandRequest {
        command: command.to_string(),
        args: request.args,
        cwd: request.cwd,
        env: request
            .env
            .into_iter()
            .map(|entry| (entry.key, entry.value))
            .collect(),
        stdin: request.stdin,
        timeout_ms: request.timeout_ms,
        network: if request.allow_network {
            traits::NetworkPolicy::Allowed
        } else {
            traits::NetworkPolicy::Disabled
        },
        resource_limits: traits::ResourceLimits::default(),
        mounts,
    })
}

#[cfg(feature = "uniffi")]
fn command_result_to_ffi(result: traits::LinuxCommandResult) -> MobileLinuxCommandResultFfi {
    MobileLinuxCommandResultFfi {
        stdout: result.stdout,
        stderr: result.stderr,
        exit_code: result.exit_code,
        timed_out: result.timed_out,
        cancelled: result.cancelled,
    }
}

#[cfg(feature = "uniffi")]
fn task_status_to_ffi(status: traits::MobileLinuxTaskStatus) -> MobileLinuxTaskStateFfi {
    match status {
        traits::MobileLinuxTaskStatus::Queued => MobileLinuxTaskStateFfi::Queued,
        traits::MobileLinuxTaskStatus::Running => MobileLinuxTaskStateFfi::Running,
        traits::MobileLinuxTaskStatus::Backgrounded => MobileLinuxTaskStateFfi::Backgrounded,
        traits::MobileLinuxTaskStatus::Completed => MobileLinuxTaskStateFfi::Completed,
        traits::MobileLinuxTaskStatus::Failed => MobileLinuxTaskStateFfi::Failed,
        traits::MobileLinuxTaskStatus::Cancelled => MobileLinuxTaskStateFfi::Cancelled,
        traits::MobileLinuxTaskStatus::TimedOut => MobileLinuxTaskStateFfi::TimedOut,
    }
}

#[cfg(feature = "uniffi")]
fn task_snapshot_to_ffi(task: traits::MobileLinuxTaskSnapshot) -> MobileLinuxTaskSnapshotFfi {
    MobileLinuxTaskSnapshotFfi {
        task_id: task.task_id,
        status: task_status_to_ffi(task.status),
        command: task.command,
        started_at_ms: task.started_at_ms,
        finished_at_ms: task.finished_at_ms,
        exit_code: task.exit_code,
        detail: task.detail,
    }
}

#[cfg(feature = "uniffi")]
fn event_to_ffi(event: traits::MobileLinuxEvent) -> MobileLinuxEventFfi {
    match event.kind {
        traits::MobileLinuxEventKind::TaskStatusChanged {
            status,
            exit_code,
            detail,
        } => MobileLinuxEventFfi {
            sequence: event.sequence,
            task_id: event.task_id,
            kind: MobileLinuxEventKindFfi::TaskStatusChanged,
            data: None,
            text: None,
            session_id: None,
            status: Some(task_status_to_ffi(status)),
            exit_code,
            timed_out: Some(matches!(status, traits::MobileLinuxTaskStatus::TimedOut)),
            cancelled: Some(matches!(status, traits::MobileLinuxTaskStatus::Cancelled)),
            detail,
        },
        traits::MobileLinuxEventKind::StdoutLine { line } => MobileLinuxEventFfi {
            sequence: event.sequence,
            task_id: event.task_id,
            kind: MobileLinuxEventKindFfi::StdoutLine,
            data: None,
            text: Some(line),
            session_id: None,
            status: None,
            exit_code: None,
            timed_out: None,
            cancelled: None,
            detail: None,
        },
        traits::MobileLinuxEventKind::StderrChunk { chunk } => MobileLinuxEventFfi {
            sequence: event.sequence,
            task_id: event.task_id,
            kind: MobileLinuxEventKindFfi::StderrChunk,
            data: Some(chunk),
            text: None,
            session_id: None,
            status: None,
            exit_code: None,
            timed_out: None,
            cancelled: None,
            detail: None,
        },
        traits::MobileLinuxEventKind::PtyOutput { session_id, data } => MobileLinuxEventFfi {
            sequence: event.sequence,
            task_id: event.task_id,
            kind: MobileLinuxEventKindFfi::PtyOutput,
            data: Some(data),
            text: None,
            session_id: Some(session_id),
            status: None,
            exit_code: None,
            timed_out: None,
            cancelled: None,
            detail: None,
        },
        traits::MobileLinuxEventKind::PtyClosed {
            session_id,
            exit_code,
            detail,
        } => MobileLinuxEventFfi {
            sequence: event.sequence,
            task_id: event.task_id,
            kind: MobileLinuxEventKindFfi::PtyClosed,
            data: None,
            text: None,
            session_id: Some(session_id),
            status: None,
            exit_code,
            timed_out: None,
            cancelled: None,
            detail,
        },
        traits::MobileLinuxEventKind::RuntimeError { detail } => MobileLinuxEventFfi {
            sequence: event.sequence,
            task_id: event.task_id,
            kind: MobileLinuxEventKindFfi::RuntimeError,
            data: None,
            text: None,
            session_id: None,
            status: None,
            exit_code: None,
            timed_out: None,
            cancelled: None,
            detail: Some(detail),
        },
    }
}

#[cfg(feature = "uniffi")]
fn mount_spec_to_traits(
    mount: MobileLinuxMountSpecFfi,
) -> Result<traits::MountSpec, MobileLinuxApiErrorFfi> {
    if mount.guest_path.trim().is_empty() || !mount.guest_path.starts_with('/') {
        return Err(MobileLinuxApiErrorFfi::InvalidRequest {
            message: format!("invalid guest mount path: {}", mount.guest_path),
        });
    }
    if mount.host_path.trim().is_empty() {
        return Err(MobileLinuxApiErrorFfi::InvalidRequest {
            message: "host mount path must not be empty".to_string(),
        });
    }
    Ok(traits::MountSpec {
        host_path: std::path::PathBuf::from(mount.host_path),
        guest_path: mount.guest_path,
        read_only: mount.read_only,
        purpose: mount_purpose_to_traits(mount.purpose),
    })
}

#[cfg(feature = "uniffi")]
fn pty_open_request_to_traits(
    request: MobileLinuxPtyOpenRequestFfi,
) -> Result<traits::PtyOpenRequest, MobileLinuxApiErrorFfi> {
    let command = request.command.trim();
    if command.is_empty() {
        return Err(MobileLinuxApiErrorFfi::InvalidRequest {
            message: "pty command must not be empty".to_string(),
        });
    }
    let mounts = request
        .mounts
        .into_iter()
        .map(mount_spec_to_traits)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(traits::PtyOpenRequest {
        command: command.to_string(),
        args: request.args,
        cwd: request.cwd,
        env: request
            .env
            .into_iter()
            .map(|entry| (entry.key, entry.value))
            .collect(),
        size: traits::PtySize {
            cols: request.size.cols,
            rows: request.size.rows,
        },
        mounts,
    })
}

#[cfg(feature = "uniffi")]
fn process_handle_to_ffi(handle: traits::LinuxProcessHandle) -> MobileLinuxProcessHandleFfi {
    MobileLinuxProcessHandleFfi { id: handle.id }
}

#[cfg(feature = "uniffi")]
fn process_handle_to_traits(
    handle: MobileLinuxProcessHandleFfi,
) -> Result<traits::LinuxProcessHandle, MobileLinuxApiErrorFfi> {
    if handle.id.trim().is_empty() {
        return Err(MobileLinuxApiErrorFfi::InvalidRequest {
            message: "process handle id must not be empty".to_string(),
        });
    }
    Ok(traits::LinuxProcessHandle {
        id: handle.id,
        enforcement: traits::LinuxEnforcementReceipt::default(),
    })
}

#[cfg(feature = "uniffi")]
fn pty_handle_to_ffi(handle: traits::PtySessionHandle) -> MobileLinuxPtySessionHandleFfi {
    MobileLinuxPtySessionHandleFfi { id: handle.id }
}

#[cfg(feature = "uniffi")]
fn pty_handle_to_traits(
    handle: MobileLinuxPtySessionHandleFfi,
) -> Result<traits::PtySessionHandle, MobileLinuxApiErrorFfi> {
    if handle.id.trim().is_empty() {
        return Err(MobileLinuxApiErrorFfi::InvalidRequest {
            message: "pty handle id must not be empty".to_string(),
        });
    }
    Ok(traits::PtySessionHandle { id: handle.id })
}

#[cfg(feature = "uniffi")]
fn mobile_linux_error_to_ffi(error: traits::MobileLinuxError) -> MobileLinuxApiErrorFfi {
    match error {
        traits::MobileLinuxError::Unsupported => MobileLinuxApiErrorFfi::Unavailable {
            message: "runtime unsupported on this build".to_string(),
        },
        traits::MobileLinuxError::Unavailable(message) => {
            MobileLinuxApiErrorFfi::Unavailable { message }
        }
        traits::MobileLinuxError::LicenseBlocked(message) => {
            MobileLinuxApiErrorFfi::LicenseBlocked { message }
        }
        traits::MobileLinuxError::InvalidRequest(message) => {
            MobileLinuxApiErrorFfi::InvalidRequest { message }
        }
        traits::MobileLinuxError::Integrity(message) | traits::MobileLinuxError::Io(message) => {
            MobileLinuxApiErrorFfi::OperationFailed { message }
        }
        traits::MobileLinuxError::NetworkPolicyUnavailable(message) => {
            MobileLinuxApiErrorFfi::OperationFailed {
                message: format!("network_policy_unavailable: {message}"),
            }
        }
        traits::MobileLinuxError::ResourceLimitExceeded(message) => {
            MobileLinuxApiErrorFfi::OperationFailed {
                message: format!("resource_limit_exceeded: {message}"),
            }
        }
        traits::MobileLinuxError::Timeout => MobileLinuxApiErrorFfi::OperationFailed {
            message: "timeout".to_string(),
        },
    }
}

#[cfg(feature = "uniffi")]
struct AndroidMobileLinuxEventSinkBridge {
    task_id: String,
    inner: Box<dyn AndroidMobileLinuxEventSink>,
}

#[cfg(feature = "uniffi")]
impl AndroidMobileLinuxEventSinkBridge {
    async fn emit_event(&self, event: MobileLinuxEventFfi) -> Result<(), MobileLinuxApiErrorFfi> {
        self.inner.on_event(event).await
    }
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl traits::ProcessStreamSink for AndroidMobileLinuxEventSinkBridge {
    async fn stdout_line(&self, line: String) -> Result<(), traits::ProcessError> {
        self.inner
            .on_event(MobileLinuxEventFfi {
                sequence: 0,
                task_id: Some(self.task_id.clone()),
                kind: MobileLinuxEventKindFfi::StdoutLine,
                data: None,
                text: Some(line),
                session_id: None,
                status: None,
                exit_code: None,
                timed_out: None,
                cancelled: None,
                detail: None,
            })
            .await
            .map_err(|err| traits::ProcessError::Io(err.to_string()))
    }

    async fn stderr_chunk(&self, chunk: Vec<u8>) -> Result<(), traits::ProcessError> {
        self.inner
            .on_event(MobileLinuxEventFfi {
                sequence: 0,
                task_id: Some(self.task_id.clone()),
                kind: MobileLinuxEventKindFfi::StderrChunk,
                data: Some(chunk),
                text: None,
                session_id: None,
                status: None,
                exit_code: None,
                timed_out: None,
                cancelled: None,
                detail: None,
            })
            .await
            .map_err(|err| traits::ProcessError::Io(err.to_string()))
    }
}

#[cfg(feature = "uniffi")]
fn require_mobile_linux_runtime(
    config: Option<&AndroidMobileLinuxConfigFfi>,
) -> Result<Arc<dyn traits::MobileLinuxRuntime>, MobileLinuxApiErrorFfi> {
    match config {
        None => Err(MobileLinuxApiErrorFfi::LegacySelected),
        Some(cfg) if matches!(cfg.mode, MobileLinuxRuntimeModeFfi::Legacy) => {
            Err(MobileLinuxApiErrorFfi::LegacySelected)
        }
        Some(_) => {
            android_mobile_linux_runtime(config).ok_or(MobileLinuxApiErrorFfi::Unavailable {
                message: "mobile-linux runtime is not wired into this build".to_string(),
            })
        }
    }
}

#[cfg(feature = "uniffi")]
fn block_on_mobile_linux_call<T>(
    config: Option<&AndroidMobileLinuxConfigFfi>,
    op: impl FnOnce(
        Arc<dyn traits::MobileLinuxRuntime>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<T, traits::MobileLinuxError>> + Send>,
    >,
) -> Result<T, MobileLinuxApiErrorFfi> {
    let runtime = require_mobile_linux_runtime(config)?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| MobileLinuxApiErrorFfi::OperationFailed {
            message: err.to_string(),
        })?;
    rt.block_on(op(runtime)).map_err(mobile_linux_error_to_ffi)
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(async_runtime = "tokio"))]
impl AndroidMobileLinuxRuntimeHandle {
    pub async fn capability(&self) -> MobileLinuxCapabilityFfi {
        capability_to_ffi(self.runtime.probe_capability().await)
    }

    pub async fn status(&self) -> Result<MobileLinuxStatusFfi, MobileLinuxApiErrorFfi> {
        self.runtime
            .rootfs_status()
            .await
            .map(status_to_ffi)
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn boot(&self) -> Result<MobileLinuxStatusFfi, MobileLinuxApiErrorFfi> {
        self.runtime
            .boot()
            .await
            .map(status_to_ffi)
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn shutdown(&self) -> Result<(), MobileLinuxApiErrorFfi> {
        self.runtime
            .shutdown()
            .await
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn verify_rootfs(&self) -> Result<MobileLinuxStatusFfi, MobileLinuxApiErrorFfi> {
        self.runtime
            .verify_rootfs()
            .await
            .map(status_to_ffi)
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn repair_rootfs(&self) -> Result<MobileLinuxStatusFfi, MobileLinuxApiErrorFfi> {
        self.runtime
            .repair_rootfs()
            .await
            .map(status_to_ffi)
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn reset_rootfs(&self) -> Result<MobileLinuxStatusFfi, MobileLinuxApiErrorFfi> {
        self.runtime
            .reset_rootfs()
            .await
            .map(status_to_ffi)
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn run_command(
        &self,
        request: MobileLinuxCommandRequestFfi,
    ) -> Result<MobileLinuxCommandResultFfi, MobileLinuxApiErrorFfi> {
        self.runtime
            .run(command_request_to_traits(request)?)
            .await
            .map(command_result_to_ffi)
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn run_command_streaming(
        &self,
        request: MobileLinuxCommandRequestFfi,
        sink: Box<dyn AndroidMobileLinuxEventSink>,
    ) -> Result<MobileLinuxCommandResultFfi, MobileLinuxApiErrorFfi> {
        let task_id = format!(
            "run-{}",
            MOBILE_LINUX_FFI_ID_COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let sink = Arc::new(AndroidMobileLinuxEventSinkBridge {
            task_id,
            inner: sink,
        });
        let result = self
            .runtime
            .run_streaming(command_request_to_traits(request)?, sink.clone())
            .await
            .map_err(mobile_linux_error_to_ffi)?;
        sink.emit_event(MobileLinuxEventFfi {
            sequence: 0,
            task_id: Some(sink.task_id.clone()),
            kind: MobileLinuxEventKindFfi::TaskStatusChanged,
            data: None,
            text: None,
            session_id: None,
            status: Some(if result.cancelled {
                MobileLinuxTaskStateFfi::Cancelled
            } else if result.timed_out {
                MobileLinuxTaskStateFfi::TimedOut
            } else if result.exit_code == 0 {
                MobileLinuxTaskStateFfi::Completed
            } else {
                MobileLinuxTaskStateFfi::Failed
            }),
            exit_code: Some(result.exit_code),
            timed_out: Some(result.timed_out),
            cancelled: Some(result.cancelled),
            detail: None,
        })
        .await?;
        Ok(command_result_to_ffi(result))
    }

    pub async fn spawn_background(
        &self,
        request: MobileLinuxCommandRequestFfi,
    ) -> Result<MobileLinuxProcessHandleFfi, MobileLinuxApiErrorFfi> {
        self.runtime
            .spawn_background(command_request_to_traits(request)?)
            .await
            .map(process_handle_to_ffi)
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn kill_process(
        &self,
        handle: MobileLinuxProcessHandleFfi,
    ) -> Result<(), MobileLinuxApiErrorFfi> {
        self.runtime
            .kill(&process_handle_to_traits(handle)?)
            .await
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn open_pty(
        &self,
        request: MobileLinuxPtyOpenRequestFfi,
    ) -> Result<MobileLinuxPtySessionHandleFfi, MobileLinuxApiErrorFfi> {
        self.runtime
            .open_pty(pty_open_request_to_traits(request)?)
            .await
            .map(pty_handle_to_ffi)
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn write_pty(
        &self,
        handle: MobileLinuxPtySessionHandleFfi,
        input: Vec<u8>,
    ) -> Result<(), MobileLinuxApiErrorFfi> {
        self.runtime
            .write_pty(&pty_handle_to_traits(handle)?, input)
            .await
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn resize_pty(
        &self,
        handle: MobileLinuxPtySessionHandleFfi,
        size: MobileLinuxPtySizeFfi,
    ) -> Result<(), MobileLinuxApiErrorFfi> {
        self.runtime
            .resize_pty(
                &pty_handle_to_traits(handle)?,
                traits::PtySize {
                    cols: size.cols,
                    rows: size.rows,
                },
            )
            .await
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn close_pty(
        &self,
        handle: MobileLinuxPtySessionHandleFfi,
    ) -> Result<(), MobileLinuxApiErrorFfi> {
        self.runtime
            .close_pty(&pty_handle_to_traits(handle)?)
            .await
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn configure_mounts(
        &self,
        mounts: Vec<MobileLinuxMountSpecFfi>,
    ) -> Result<MobileLinuxStatusFfi, MobileLinuxApiErrorFfi> {
        let mounts = mounts
            .into_iter()
            .map(mount_spec_to_traits)
            .collect::<Result<Vec<_>, _>>()?;
        self.runtime
            .configure_mounts(mounts)
            .await
            .map_err(mobile_linux_error_to_ffi)?;
        self.status().await
    }

    pub async fn read_events(
        &self,
        after_sequence: Option<u64>,
        limit: Option<u32>,
    ) -> Result<Vec<MobileLinuxEventFfi>, MobileLinuxApiErrorFfi> {
        let limit = limit
            .map(|value| value as usize)
            .unwrap_or(MAX_MOBILE_LINUX_EVENT_BATCH)
            .min(MAX_MOBILE_LINUX_EVENT_BATCH);
        self.runtime
            .read_events(after_sequence, limit)
            .await
            .map(|events| events.into_iter().map(event_to_ffi).collect())
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn list_tasks(
        &self,
    ) -> Result<Vec<MobileLinuxTaskSnapshotFfi>, MobileLinuxApiErrorFfi> {
        self.runtime
            .list_tasks()
            .await
            .map(|tasks| tasks.into_iter().map(task_snapshot_to_ffi).collect())
            .map_err(mobile_linux_error_to_ffi)
    }

    pub async fn task_status(
        &self,
        task_id: String,
    ) -> Result<Option<MobileLinuxTaskSnapshotFfi>, MobileLinuxApiErrorFfi> {
        if task_id.trim().is_empty() {
            return Err(MobileLinuxApiErrorFfi::InvalidRequest {
                message: "task_id must not be empty".to_string(),
            });
        }
        self.runtime
            .task_status(&task_id)
            .await
            .map(|task| task.map(task_snapshot_to_ffi))
            .map_err(mobile_linux_error_to_ffi)
    }
}

#[cfg(feature = "uniffi")]
fn android_mobile_linux_status_from_config(
    config: Option<&AndroidMobileLinuxConfigFfi>,
) -> MobileLinuxStatusFfi {
    match config {
        None => MobileLinuxStatusFfi {
            state: MobileLinuxRootfsStateFfi::Unsupported,
            backend: "android-minijail".to_string(),
            mode: MobileLinuxRuntimeModeFfi::Legacy,
            platform: "android".to_string(),
            abi: "unknown".to_string(),
            version: None,
            managed_root: None,
            active_root: None,
            staged_root: None,
            archive_sha256: None,
            installed_size_bytes: None,
            writable_guest_paths: vec![],
            last_error: Some("legacy backend selected".to_string()),
        },
        Some(cfg) if matches!(cfg.mode, MobileLinuxRuntimeModeFfi::Legacy) => {
            MobileLinuxStatusFfi {
                state: MobileLinuxRootfsStateFfi::Unsupported,
                backend: "android-minijail".to_string(),
                mode: MobileLinuxRuntimeModeFfi::Legacy,
                platform: "android".to_string(),
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
                    configured_workspace_guest_path(cfg),
                ],
                last_error: Some("legacy backend selected".to_string()),
            }
        }
        Some(cfg) => MobileLinuxStatusFfi {
            state: MobileLinuxRootfsStateFfi::Unsupported,
            backend: "android-proot".to_string(),
            mode: MobileLinuxRuntimeModeFfi::MobileLinux,
            platform: "android".to_string(),
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
                configured_workspace_guest_path(cfg),
            ],
            last_error: Some("Android PRoot runtime is not linked in this build".to_string()),
        },
    }
}

#[cfg(feature = "uniffi")]
fn android_mobile_linux_runtime(
    config: Option<&AndroidMobileLinuxConfigFfi>,
) -> Option<Arc<dyn traits::MobileLinuxRuntime>> {
    let cfg = config?;
    if matches!(cfg.mode, MobileLinuxRuntimeModeFfi::Legacy) {
        return None;
    }
    #[cfg(target_os = "android")]
    {
        use std::collections::HashMap;
        use std::sync::{LazyLock, Mutex};

        static RUNTIMES: LazyLock<Mutex<HashMap<String, Arc<dyn traits::MobileLinuxRuntime>>>> =
            LazyLock::new(|| Mutex::new(HashMap::new()));
        let key = format!(
            "{}|{}|{}|{}",
            cfg.managed_root,
            cfg.abi,
            cfg.rootfs_version,
            cfg.archive_sha256.as_deref().unwrap_or_default(),
        );
        if let Some(runtime) = RUNTIMES
            .lock()
            .expect("Android MobileLinux runtime registry")
            .get(&key)
            .cloned()
        {
            return Some(runtime);
        }
        let runtime = platform_android::AndroidProotRuntime::new(
            platform_android::AndroidProotRuntimeConfig {
                managed_root: std::path::PathBuf::from(&cfg.managed_root),
                abi: cfg.abi.clone(),
                rootfs_version: cfg.rootfs_version.clone(),
                archive_sha256: cfg.archive_sha256.clone(),
            },
        );
        let runtime = Arc::new(runtime) as Arc<dyn traits::MobileLinuxRuntime>;
        RUNTIMES
            .lock()
            .expect("Android MobileLinux runtime registry")
            .insert(key, runtime.clone());
        return Some(runtime);
    }
    #[cfg(not(target_os = "android"))]
    {
        let runtime = traits::UnavailableMobileLinuxRuntime::unavailable(
            traits::SandboxBackend::AndroidProot,
            traits::MobileLinuxRuntimeMode::MobileLinux,
            "android",
            cfg.abi.clone(),
            "Android PRoot runtime requires an Android target",
        );
        Some(Arc::new(runtime) as Arc<dyn traits::MobileLinuxRuntime>)
    }
}

#[cfg(feature = "uniffi")]
fn android_mobile_linux_capability_from_config(
    config: Option<&AndroidMobileLinuxConfigFfi>,
) -> MobileLinuxCapabilityFfi {
    match config {
        None => MobileLinuxCapabilityFfi {
            available: false,
            backend: "android-minijail".to_string(),
            mode: MobileLinuxRuntimeModeFfi::Legacy,
            reason: Some("legacy backend selected".to_string()),
            streaming_output: false,
            background_processes: false,
            pty: false,
            bind_mounts: false,
            rootfs_integrity: false,
        },
        Some(cfg) if matches!(cfg.mode, MobileLinuxRuntimeModeFfi::Legacy) => {
            MobileLinuxCapabilityFfi {
                available: false,
                backend: "android-minijail".to_string(),
                mode: MobileLinuxRuntimeModeFfi::Legacy,
                reason: Some("legacy backend selected".to_string()),
                streaming_output: false,
                background_processes: false,
                pty: false,
                bind_mounts: false,
                rootfs_integrity: false,
            }
        }
        Some(cfg) => MobileLinuxCapabilityFfi {
            available: false,
            backend: "android-proot".to_string(),
            mode: MobileLinuxRuntimeModeFfi::MobileLinux,
            reason: Some("Android PRoot runtime is not linked in this build".to_string()),
            streaming_output: false,
            background_processes: false,
            pty: false,
            bind_mounts: false,
            rootfs_integrity: false,
        },
    }
}

#[cfg(feature = "uniffi")]
fn block_on_mobile_linux_status(
    config: Option<&AndroidMobileLinuxConfigFfi>,
    op: fn(
        Arc<dyn traits::MobileLinuxRuntime>,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<traits::RootfsStatus, traits::MobileLinuxError>>
                + Send,
        >,
    >,
) -> MobileLinuxStatusFfi {
    let Some(runtime) = android_mobile_linux_runtime(config) else {
        return android_mobile_linux_status_from_config(config);
    };
    let fallback = android_mobile_linux_status_from_config(config);
    let Ok(rt) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return fallback;
    };
    match rt.block_on(op(runtime)) {
        Ok(status) => status_to_ffi(status),
        Err(error) => {
            let mut status = fallback;
            status.last_error = Some(error.to_string());
            status
        }
    }
}

#[cfg(feature = "uniffi")]
fn verify_rootfs_op(
    runtime: Arc<dyn traits::MobileLinuxRuntime>,
) -> std::pin::Pin<
    Box<
        dyn std::future::Future<Output = Result<traits::RootfsStatus, traits::MobileLinuxError>>
            + Send,
    >,
> {
    Box::pin(async move { runtime.verify_rootfs().await })
}

#[cfg(feature = "uniffi")]
fn repair_rootfs_op(
    runtime: Arc<dyn traits::MobileLinuxRuntime>,
) -> std::pin::Pin<
    Box<
        dyn std::future::Future<Output = Result<traits::RootfsStatus, traits::MobileLinuxError>>
            + Send,
    >,
> {
    Box::pin(async move { runtime.repair_rootfs().await })
}

#[cfg(feature = "uniffi")]
fn reset_rootfs_op(
    runtime: Arc<dyn traits::MobileLinuxRuntime>,
) -> std::pin::Pin<
    Box<
        dyn std::future::Future<Output = Result<traits::RootfsStatus, traits::MobileLinuxError>>
            + Send,
    >,
> {
    Box::pin(async move { runtime.reset_rootfs().await })
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn android_mobile_linux_capability(
    config: Option<AndroidMobileLinuxConfigFfi>,
) -> MobileLinuxCapabilityFfi {
    android_mobile_linux_capability_from_config(config.as_ref())
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn android_mobile_linux_status(
    config: Option<AndroidMobileLinuxConfigFfi>,
) -> MobileLinuxStatusFfi {
    android_mobile_linux_status_from_config(config.as_ref())
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn android_mobile_linux_verify_rootfs(
    config: Option<AndroidMobileLinuxConfigFfi>,
) -> MobileLinuxStatusFfi {
    block_on_mobile_linux_status(config.as_ref(), verify_rootfs_op)
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn android_mobile_linux_repair_rootfs(
    config: Option<AndroidMobileLinuxConfigFfi>,
) -> MobileLinuxStatusFfi {
    block_on_mobile_linux_status(config.as_ref(), repair_rootfs_op)
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn android_mobile_linux_reset_rootfs(
    config: Option<AndroidMobileLinuxConfigFfi>,
) -> MobileLinuxStatusFfi {
    block_on_mobile_linux_status(config.as_ref(), reset_rootfs_op)
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn android_mobile_linux_boot(
    config: Option<AndroidMobileLinuxConfigFfi>,
) -> Result<MobileLinuxStatusFfi, MobileLinuxApiErrorFfi> {
    block_on_mobile_linux_call(config.as_ref(), |runtime| {
        Box::pin(async move { runtime.boot().await })
    })
    .map(status_to_ffi)
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn android_mobile_linux_shutdown(
    config: Option<AndroidMobileLinuxConfigFfi>,
) -> Result<(), MobileLinuxApiErrorFfi> {
    block_on_mobile_linux_call(config.as_ref(), |runtime| {
        Box::pin(async move {
            runtime.shutdown().await?;
            Ok(())
        })
    })
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn android_mobile_linux_run_command(
    config: Option<AndroidMobileLinuxConfigFfi>,
    request: MobileLinuxCommandRequestFfi,
) -> Result<MobileLinuxCommandResultFfi, MobileLinuxApiErrorFfi> {
    let request = command_request_to_traits(request)?;
    block_on_mobile_linux_call(config.as_ref(), |runtime| {
        Box::pin(async move { runtime.run(request).await })
    })
    .map(command_result_to_ffi)
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn android_mobile_linux_run_command_streaming(
    config: Option<AndroidMobileLinuxConfigFfi>,
    request: MobileLinuxCommandRequestFfi,
    sink: Box<dyn AndroidMobileLinuxEventSink>,
) -> Result<MobileLinuxCommandResultFfi, MobileLinuxApiErrorFfi> {
    let request = command_request_to_traits(request)?;
    let task_id = format!(
        "run-{}",
        MOBILE_LINUX_FFI_ID_COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    block_on_mobile_linux_call(config.as_ref(), |runtime| {
        let sink = std::sync::Arc::new(AndroidMobileLinuxEventSinkBridge {
            task_id: task_id.clone(),
            inner: sink,
        });
        Box::pin(async move {
            let result = runtime.run_streaming(request, sink.clone()).await?;
            sink.emit_event(MobileLinuxEventFfi {
                sequence: 0,
                task_id: Some(task_id),
                kind: MobileLinuxEventKindFfi::TaskStatusChanged,
                data: None,
                text: None,
                session_id: None,
                status: Some(if result.cancelled {
                    MobileLinuxTaskStateFfi::Cancelled
                } else if result.timed_out {
                    MobileLinuxTaskStateFfi::TimedOut
                } else if result.exit_code == 0 {
                    MobileLinuxTaskStateFfi::Completed
                } else {
                    MobileLinuxTaskStateFfi::Failed
                }),
                exit_code: Some(result.exit_code),
                timed_out: Some(result.timed_out),
                cancelled: Some(result.cancelled),
                detail: None,
            })
            .await
            .map_err(|err| traits::MobileLinuxError::Io(err.to_string()))?;
            Ok(result)
        })
    })
    .map(command_result_to_ffi)
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn android_mobile_linux_spawn_background(
    config: Option<AndroidMobileLinuxConfigFfi>,
    request: MobileLinuxCommandRequestFfi,
) -> Result<MobileLinuxProcessHandleFfi, MobileLinuxApiErrorFfi> {
    let request = command_request_to_traits(request)?;
    block_on_mobile_linux_call(config.as_ref(), |runtime| {
        Box::pin(async move { runtime.spawn_background(request).await })
    })
    .map(process_handle_to_ffi)
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn android_mobile_linux_kill_process(
    config: Option<AndroidMobileLinuxConfigFfi>,
    handle: MobileLinuxProcessHandleFfi,
) -> Result<(), MobileLinuxApiErrorFfi> {
    let handle = process_handle_to_traits(handle)?;
    block_on_mobile_linux_call(config.as_ref(), |runtime| {
        Box::pin(async move {
            runtime.kill(&handle).await?;
            Ok(())
        })
    })
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn android_mobile_linux_open_pty(
    config: Option<AndroidMobileLinuxConfigFfi>,
    request: MobileLinuxPtyOpenRequestFfi,
) -> Result<MobileLinuxPtySessionHandleFfi, MobileLinuxApiErrorFfi> {
    let request = pty_open_request_to_traits(request)?;
    block_on_mobile_linux_call(config.as_ref(), |runtime| {
        Box::pin(async move { runtime.open_pty(request).await })
    })
    .map(pty_handle_to_ffi)
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn android_mobile_linux_write_pty(
    config: Option<AndroidMobileLinuxConfigFfi>,
    handle: MobileLinuxPtySessionHandleFfi,
    input: Vec<u8>,
) -> Result<(), MobileLinuxApiErrorFfi> {
    let handle = pty_handle_to_traits(handle)?;
    block_on_mobile_linux_call(config.as_ref(), |runtime| {
        Box::pin(async move {
            runtime.write_pty(&handle, input).await?;
            Ok(())
        })
    })
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn android_mobile_linux_resize_pty(
    config: Option<AndroidMobileLinuxConfigFfi>,
    handle: MobileLinuxPtySessionHandleFfi,
    size: MobileLinuxPtySizeFfi,
) -> Result<(), MobileLinuxApiErrorFfi> {
    let handle = pty_handle_to_traits(handle)?;
    block_on_mobile_linux_call(config.as_ref(), |runtime| {
        Box::pin(async move {
            runtime
                .resize_pty(
                    &handle,
                    traits::PtySize {
                        cols: size.cols,
                        rows: size.rows,
                    },
                )
                .await?;
            Ok(())
        })
    })
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn android_mobile_linux_close_pty(
    config: Option<AndroidMobileLinuxConfigFfi>,
    handle: MobileLinuxPtySessionHandleFfi,
) -> Result<(), MobileLinuxApiErrorFfi> {
    let handle = pty_handle_to_traits(handle)?;
    block_on_mobile_linux_call(config.as_ref(), |runtime| {
        Box::pin(async move {
            runtime.close_pty(&handle).await?;
            Ok(())
        })
    })
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn android_mobile_linux_configure_mounts(
    config: Option<AndroidMobileLinuxConfigFfi>,
    mounts: Vec<MobileLinuxMountSpecFfi>,
) -> Result<MobileLinuxStatusFfi, MobileLinuxApiErrorFfi> {
    let mounts = mounts
        .into_iter()
        .map(mount_spec_to_traits)
        .collect::<Result<Vec<_>, _>>()?;
    block_on_mobile_linux_call(config.as_ref(), |runtime| {
        Box::pin(async move {
            runtime.configure_mounts(mounts).await?;
            runtime.rootfs_status().await
        })
    })
    .map(status_to_ffi)
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn android_mobile_linux_list_tasks(
    config: Option<AndroidMobileLinuxConfigFfi>,
) -> Result<Vec<MobileLinuxTaskSnapshotFfi>, MobileLinuxApiErrorFfi> {
    block_on_mobile_linux_call(config.as_ref(), |runtime| {
        Box::pin(async move { runtime.list_tasks().await })
    })
    .map(|tasks| tasks.into_iter().map(task_snapshot_to_ffi).collect())
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn android_mobile_linux_read_events(
    config: Option<AndroidMobileLinuxConfigFfi>,
    after_sequence: Option<u64>,
    limit: Option<u32>,
) -> Result<Vec<MobileLinuxEventFfi>, MobileLinuxApiErrorFfi> {
    let limit = limit
        .map(|value| value as usize)
        .unwrap_or(MAX_MOBILE_LINUX_EVENT_BATCH)
        .min(MAX_MOBILE_LINUX_EVENT_BATCH);
    block_on_mobile_linux_call(config.as_ref(), |runtime| {
        Box::pin(async move { runtime.read_events(after_sequence, limit).await })
    })
    .map(|events| events.into_iter().map(event_to_ffi).collect())
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn android_mobile_linux_task_status(
    config: Option<AndroidMobileLinuxConfigFfi>,
    task_id: String,
) -> Result<Option<MobileLinuxTaskSnapshotFfi>, MobileLinuxApiErrorFfi> {
    if task_id.trim().is_empty() {
        return Err(MobileLinuxApiErrorFfi::InvalidRequest {
            message: "task_id must not be empty".to_string(),
        });
    }
    block_on_mobile_linux_call(config.as_ref(), |runtime| {
        Box::pin(async move { runtime.task_status(&task_id).await })
    })
    .map(|snapshot| snapshot.map(task_snapshot_to_ffi))
}

// ---------------------------------------------------------------------------
// T2.1 — foreign (Kotlin) speech callback interfaces + their engine bridges.
// ---------------------------------------------------------------------------
//
// The Kotlin layer implements two crate-local async callback interfaces —
// `AndroidStt` (system `SpeechRecognizer`) and `AndroidTts` (system
// `TextToSpeech`) — and hands them across the FFI seam. The engine consumes the
// SHARED `traits::SpeechToText` / `traits::TextToSpeech` seams, so a thin bridge
// struct adapts each crate-local interface to its `traits` counterpart.
//
// These interfaces are DEFINED IN THIS CRATE (mirroring the `IosEventListener`
// pattern) so their UniFFI `FfiConverter`s register under `android_aar`'s tag —
// a prerequisite for naming them as parameter types in a `#[uniffi::export]`
// constructor here.
//
// RETURN SHAPE (UniFFI 0.28.3): async callback-interface methods return
// `Result<T, E>` where `E` is a `#[derive(uniffi::Error)]` enum — this is the
// supported async-callback fallible shape on 0.28. The bridge maps the FFI
// error variants onto the richer `SttError` / `TtsError` (mic-permission →
// `PermissionDenied`, etc.).

/// FFI error surface for the Android speech callback interfaces. A flat enum so
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

/// Crate-local foreign callback interface for native speech-to-text — the Kotlin
/// app implements it over the system `SpeechRecognizer` (opens the live mic,
/// listens for one utterance, returns the final transcript). Bridged to
/// [`traits::SpeechToText`] by [`AndroidSttBridge`].
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait AndroidStt: Send + Sync {
    /// Open the mic, listen for a single utterance, and return the recognized
    /// text. `language` is a BCP-47 hint (`None` = device default).
    async fn transcribe(&self, language: Option<String>) -> Result<String, SpeechFfiError>;
}

/// Crate-local foreign callback interface for native text-to-speech — the Kotlin
/// app implements it over the system `TextToSpeech` engine, returning 16-bit
/// signed little-endian mono PCM. Bridged to [`traits::TextToSpeech`] by
/// [`AndroidTtsBridge`].
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait AndroidTts: Send + Sync {
    /// Synthesize `text` to PCM16 audio at [`TtsAudioFfi::sample_rate_hz`].
    /// `voice` is a provider-specific id (`None` = system default voice).
    async fn synthesize(
        &self,
        text: String,
        voice: Option<String>,
    ) -> Result<TtsAudioFfi, SpeechFfiError>;
}

/// FFI carrier for synthesized audio crossing the callback-interface seam:
/// PCM16 frames + the sample rate the Kotlin engine produced them at.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct TtsAudioFfi {
    /// Raw PCM16 frames (16-bit signed little-endian, mono).
    pub pcm: Vec<u8>,
    /// Sample rate of `pcm` in Hz.
    pub sample_rate_hz: u32,
}

// ---------------------------------------------------------------------------
// Share — foreign (Kotlin) callback interface + its engine bridge.
// ---------------------------------------------------------------------------
//
// Mirrors the AndroidStt/AndroidTts/AndroidCamera pattern: the Kotlin layer
// implements a crate-local async `AndroidShare` callback interface (the system
// `Intent.ACTION_SEND` share sheet) and hands it across the FFI seam. The
// engine consumes the SHARED `traits::SharingService` seam, so
// `AndroidShareBridge` adapts the crate-local interface to its `traits`
// counterpart. The shared `traits::SharePayload` is destructured into the three
// flat `text` / `url` / `image_bytes` args to keep the FFI flat; the bridge
// maps the FFI result/error back onto `traits::ShareResult` / `traits::ShareError`.

/// FFI error surface for the Android share callback interface. A flat enum so
/// `UniFFI` can render it for an async `callback_interface` method; the bridge
/// fans it back out onto the richer [`traits::ShareError`].
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

/// Crate-local foreign callback interface for native sharing — the Kotlin app
/// implements it over the system `Intent.ACTION_SEND` share sheet. Bridged to
/// [`traits::SharingService`] by [`AndroidShareBridge`]. The payload crosses the
/// seam as three flat optionals (`text` / `url` / `image_bytes`).
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait AndroidShare: Send + Sync {
    /// Present the native share sheet for the given payload and report whether
    /// the user completed or cancelled it.
    async fn share(
        &self,
        text: Option<String>,
        url: Option<String>,
        image_bytes: Option<Vec<u8>>,
    ) -> Result<ShareResultFfi, ShareFfiError>;
}

/// Adapts the crate-local [`AndroidShare`] callback interface to the shared
/// [`traits::SharingService`] seam the engine consumes. Destructures
/// [`traits::SharePayload`] into the flat `text` / `url` / `image_bytes` args
/// and fans [`ShareResultFfi`] / [`ShareFfiError`] back out onto
/// [`traits::ShareResult`] / [`traits::ShareError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
struct AndroidShareBridge {
    inner: Box<dyn AndroidShare>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl traits::SharingService for AndroidShareBridge {
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

// ---------------------------------------------------------------------------
// Location — foreign (Kotlin) callback interface + its engine bridge.
// ---------------------------------------------------------------------------

/// FFI error surface for the Android one-shot location callback interface.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Error))]
#[derive(Debug, thiserror::Error)]
pub enum LocationFfiError {
    /// The user denied Android's location runtime permission.
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

/// FFI carrier for one resolved Android location.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct LocationFixFfi {
    /// Latitude in decimal degrees (WGS-84).
    pub latitude: f64,
    /// Longitude in decimal degrees (WGS-84).
    pub longitude: f64,
    /// Horizontal accuracy in meters, when reported by Android.
    pub accuracy_m: Option<f64>,
    /// Fix time, epoch milliseconds.
    pub timestamp_ms: u64,
}

/// Crate-local foreign callback interface for one-shot Android location.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait AndroidLocation: Send + Sync {
    /// Resolve the device's current location once.
    async fn current_location(&self) -> Result<LocationFixFfi, LocationFfiError>;
}

/// Adapts the Kotlin callback onto the shared engine location seam.
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
struct AndroidLocationBridge {
    inner: Box<dyn AndroidLocation>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl traits::LocationProvider for AndroidLocationBridge {
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

// ---------------------------------------------------------------------------
// Notifications — foreign (Kotlin) callback interface + its engine bridge.
// ---------------------------------------------------------------------------
//
// Mirrors the AndroidShare pattern: the Kotlin layer implements a crate-local
// async `AndroidNotification` callback interface (the system
// `NotificationManager`) and hands it across the FFI seam. The engine consumes
// the SHARED `traits::NotificationService` seam, so `AndroidNotificationBridge`
// adapts the crate-local interface to its `traits` counterpart. The shared
// `traits::NotificationRequest` is destructured into the flat `title` / `body`
// / `tag` args to keep the FFI flat; the bridge maps the FFI error back onto
// `traits::NotificationError`. This is ENGINE-DRIVEN by `tool-notification`
// (the model posts a notification) — no user-facing UI affordance.

/// FFI error surface for the Android notification callback interface. A flat
/// enum so `UniFFI` can render it for an async `callback_interface` method; the
/// bridge fans it back out onto the richer [`traits::NotificationError`].
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

/// Crate-local foreign callback interface for native notifications — the Kotlin
/// app implements it over the system `NotificationManager`. Bridged to
/// [`traits::NotificationService`] by [`AndroidNotificationBridge`]. The request
/// crosses the seam as the flat `title` / `body` / `tag` args.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait AndroidNotification: Send + Sync {
    /// Post a single local notification. `tag` (when present) lets a later post
    /// replace an earlier one (the notification id / channel tag).
    async fn notify(
        &self,
        title: String,
        body: String,
        tag: Option<String>,
    ) -> Result<(), NotificationFfiError>;
}

/// Adapts the crate-local [`AndroidNotification`] callback interface to the
/// shared [`traits::NotificationService`] seam the engine consumes.
/// Destructures [`traits::NotificationRequest`] into the flat `title` / `body`
/// / `tag` args and fans [`NotificationFfiError`] back out onto
/// [`traits::NotificationError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
struct AndroidNotificationBridge {
    inner: Box<dyn AndroidNotification>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl traits::NotificationService for AndroidNotificationBridge {
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

// ---------------------------------------------------------------------------
// Clipboard — foreign (Kotlin) callback interface + its engine bridge.
// ---------------------------------------------------------------------------
//
// Mirrors the AndroidNotification pattern: the Kotlin layer implements a
// crate-local async `AndroidClipboard` callback interface (the system
// `ClipboardManager`) and hands it across the FFI seam. The engine consumes
// the SHARED `traits::Clipboard` seam, so `AndroidClipboardBridge` adapts the
// crate-local interface to its `traits` counterpart; the bridge maps the FFI
// error back onto `traits::ClipboardError`. This is ENGINE-DRIVEN by
// `tool-clipboard` (the model reads/writes the pasteboard) — no user-facing UI
// affordance. NOTE Android 10+ restricts clipboard READS to the focused app /
// default IME — when a read is not permitted the Kotlin side returns `None`
// gracefully rather than crashing.

/// FFI error surface for the Android clipboard callback interface. A flat enum
/// so `UniFFI` can render it for an async `callback_interface` method; the bridge
/// fans it back out onto the richer [`traits::ClipboardError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Error))]
#[derive(Debug, thiserror::Error)]
pub enum ClipboardFfiError {
    /// The platform does not support this clipboard operation (e.g. Android
    /// 10+ restricts clipboard reads to the focused app / default IME).
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
/// Kotlin app implements it over the system `ClipboardManager` (set via
/// `ClipData.newPlainText` + `setPrimaryClip`; get via
/// `primaryClip.getItemAt(0).coerceToText`). Bridged to [`traits::Clipboard`]
/// by [`AndroidClipboardBridge`]. `get_text` returns `None` when the clipboard
/// is empty or a read is not permitted by the platform.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait AndroidClipboard: Send + Sync {
    /// Write plain `text` to the system clipboard.
    async fn set_text(&self, text: String) -> Result<(), ClipboardFfiError>;
    /// Read plain text from the system clipboard. Returns `None` when empty or
    /// when a background read is not permitted (Android 10+ restriction).
    async fn get_text(&self) -> Result<Option<String>, ClipboardFfiError>;
}

/// Adapts the crate-local [`AndroidClipboard`] callback interface to the shared
/// [`traits::Clipboard`] seam the engine consumes. One forwarding hop per call;
/// maps [`ClipboardFfiError`] back out onto [`traits::ClipboardError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
struct AndroidClipboardBridge {
    inner: Box<dyn AndroidClipboard>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl traits::Clipboard for AndroidClipboardBridge {
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
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn clipboard_error_from_ffi(e: ClipboardFfiError) -> traits::ClipboardError {
    match e {
        ClipboardFfiError::Unsupported => traits::ClipboardError::Unsupported,
        ClipboardFfiError::Other { message } => traits::ClipboardError::Other(message),
    }
}

// ---------------------------------------------------------------------------
// Secure storage — foreign (Kotlin) Keystore callback interface + engine bridge.
// ---------------------------------------------------------------------------
//
// The Kotlin app implements a crate-local async `AndroidSecureStorage` callback
// interface backed by the Android Keystore / EncryptedSharedPreferences. The
// engine's `protocol::SecureStorageData` is serde-encoded by the bridge into an
// OPAQUE `blob: Vec<u8>` keyed by (service, account); the native side stores /
// returns the blob verbatim (encrypted at rest by the Keystore). The bridge
// adapts it to the shared `traits::SecureStorage` seam and reports
// is_encrypted()=true / backend=AndroidKeystore so OAuth /login can persist.
//
// The bridge struct/impl + its error-fan-out are gated to `target_os =
// "android"` (not just `uniffi`) because they serde-encode through `serde_json`,
// which is a direct dependency only under the android target table — mirroring
// every other `serde_json::` use in this file. The callback interface + its FFI
// error enum stay plain `uniffi` so their converters register on the host
// bindgen build and `AndroidSecureStorage` can be named in `build_android_engine`.

/// FFI error surface for the Android secure-storage callback interface. A flat
/// enum so `UniFFI` can render it for an async `callback_interface` method; the
/// bridge fans it back out onto the richer [`traits::SecureStorageError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Error))]
#[derive(Debug, thiserror::Error)]
pub enum SecureStorageFfiError {
    /// The OS denied access (e.g. Keystore unlock / user-auth required).
    #[error("secure storage permission denied: {message}")]
    PermissionDenied {
        /// Human-readable detail from the native side.
        message: String,
    },
    /// The Keystore/store is currently unusable.
    #[error("secure storage backend unavailable: {message}")]
    BackendUnavailable {
        /// Human-readable detail from the native side.
        message: String,
    },
    /// Any other native failure.
    #[error("secure storage io error: {message}")]
    Io {
        /// Human-readable detail from the native side.
        message: String,
    },
}

/// Crate-local foreign callback interface for the native Android Keystore-backed
/// secure store. The engine's serialized `SecureStorageData` crosses the seam as
/// an opaque `blob` keyed by `(service, account)`; the Kotlin side persists it in
/// the Keystore / EncryptedSharedPreferences. Bridged to [`traits::SecureStorage`]
/// by [`AndroidSecureStorageBridge`].
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait AndroidSecureStorage: Send + Sync {
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

/// Adapts the crate-local [`AndroidSecureStorage`] (opaque-blob FFI) to the
/// shared [`traits::SecureStorage`] seam: serde-encodes `SecureStorageData` to a
/// blob on store, decodes on retrieve, and reports the Keystore as an encrypted
/// backend so the engine persists secrets there.
#[cfg(all(feature = "uniffi", target_os = "android"))]
struct AndroidSecureStorageBridge {
    inner: Box<dyn AndroidSecureStorage>,
}

#[cfg(all(feature = "uniffi", target_os = "android"))]
#[async_trait::async_trait]
impl traits::SecureStorage for AndroidSecureStorageBridge {
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
        traits::SecureStorageBackend::AndroidKeystore
    }
}

/// Fan a flat [`SecureStorageFfiError`] back out onto [`traits::SecureStorageError`].
#[cfg(all(feature = "uniffi", target_os = "android"))]
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

// ---------------------------------------------------------------------------
// Camera — foreign (Kotlin) callback interface + its engine bridge.
// ---------------------------------------------------------------------------
//
// Mirrors the AndroidStt/AndroidTts speech pattern: the Kotlin layer implements
// a crate-local async `AndroidCamera` callback interface (CameraX capture +
// system photo picker) and hands it across the FFI seam. The engine consumes
// the SHARED `traits::CameraControl` seam, so `AndroidCameraBridge` adapts the
// crate-local interface to its `traits` counterpart. Camera position crosses
// the seam as a plain `front: bool` (true = front/selfie, false = rear) to keep
// the FFI flat; the bridge maps it to `traits::CameraPosition`.

/// FFI error surface for the Android camera callback interface. A flat enum so
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

/// Crate-local foreign callback interface for native camera access — the Kotlin
/// app implements it over `CameraX` (capture) and the system photo picker
/// (library). Bridged to [`traits::CameraControl`] by [`AndroidCameraBridge`].
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait AndroidCamera: Send + Sync {
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
}

/// Adapts the crate-local [`AndroidCamera`] callback interface to the shared
/// [`traits::CameraControl`] seam the engine consumes. Maps
/// [`traits::CameraPosition`] onto the flat `front` bool, threads
/// `allow_editing`, and fans [`CameraFfiError`] back out onto
/// [`traits::CameraError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
struct AndroidCameraBridge {
    inner: Box<dyn AndroidCamera>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl traits::CameraControl for AndroidCameraBridge {
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
}

/// Convert an FFI [`CapturedImageFfi`] into the shared [`traits::CapturedImage`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn captured_image_from_ffi(img: CapturedImageFfi) -> traits::CapturedImage {
    traits::CapturedImage {
        jpeg_bytes: img.jpeg_bytes,
        width: img.width,
        height: img.height,
    }
}

/// Fan a flat [`CameraFfiError`] back out onto the richer [`traits::CameraError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn camera_error_from_ffi(e: CameraFfiError) -> traits::CameraError {
    match e {
        CameraFfiError::PermissionDenied => traits::CameraError::PermissionDenied,
        CameraFfiError::Cancelled => traits::CameraError::Cancelled,
        CameraFfiError::DeviceUnavailable => traits::CameraError::DeviceUnavailable,
        CameraFfiError::Other { message } => traits::CameraError::Other(message),
    }
}

// ---------------------------------------------------------------------------
// Voice — foreign (Kotlin) mic-recorder callback interface + its engine bridge.
// ---------------------------------------------------------------------------
//
// Mirrors the AndroidShare/AndroidCamera pattern: the Kotlin layer implements a
// crate-local async `AndroidVoice` callback interface (the system
// `MediaRecorder` capturing the raw mic) and hands it across the FFI seam. The
// engine consumes the SHARED `traits::VoiceRecorder` seam, so
// `AndroidVoiceBridge` adapts the crate-local interface to its `traits`
// counterpart. This is the RAW mic recorder driven by `tool-voice`
// (start/stop/is_recording), distinct from the `AndroidStt` system recognizer.
// `traits::VoiceRecordingOpts` is destructured into the flat `sample_rate_hz` /
// `format` args to keep the FFI flat; the bridge maps the FFI result/error back
// onto `traits::VoiceRecording` / `traits::VoiceError`.

/// FFI error surface for the Android mic-recorder callback interface. A flat
/// enum so `UniFFI` can render it for an async `callback_interface` method; the
/// bridge fans it back out onto the richer [`traits::VoiceError`].
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

/// Crate-local foreign callback interface for native mic recording — the Kotlin
/// app implements it over the system `MediaRecorder`. Bridged to
/// [`traits::VoiceRecorder`] by [`AndroidVoiceBridge`]. Driven by the engine
/// through `tool-voice` (start/stop/is_recording); the recording opts cross the
/// seam as the flat `sample_rate_hz` / `format` args.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait AndroidVoice: Send + Sync {
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

/// Adapts the crate-local [`AndroidVoice`] callback interface to the shared
/// [`traits::VoiceRecorder`] seam the engine consumes. Destructures
/// [`traits::VoiceRecordingOpts`] into the flat `sample_rate_hz` / `format`
/// args, converts [`VoiceRecordingFfi`] back to [`traits::VoiceRecording`], and
/// fans [`VoiceFfiError`] back out onto [`traits::VoiceError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
struct AndroidVoiceBridge {
    inner: Box<dyn AndroidVoice>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl traits::VoiceRecorder for AndroidVoiceBridge {
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
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn voice_error_from_ffi(e: VoiceFfiError) -> traits::VoiceError {
    match e {
        VoiceFfiError::PermissionDenied => traits::VoiceError::PermissionDenied,
        VoiceFfiError::NotRecording => traits::VoiceError::NotRecording,
        VoiceFfiError::Other { message } => traits::VoiceError::Other(message),
    }
}

/// Host-implemented per-op Git credential provider (spec: per-op credential FFI).
/// Called synchronously inside libgit2's credentials callback, once per network
/// op — the host fetches the secret (e.g. from the Android Keystore) on demand so
/// no plaintext secret is held resident in the engine between ops.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
pub trait AndroidGitCredentialProvider: Send + Sync {
    /// HTTPS token (PAT), or `None` for anonymous/public remotes.
    fn https_token(&self) -> Option<String>;
    /// SSH private-key passphrase, or `None` if the key is unencrypted.
    fn ssh_passphrase(&self) -> Option<String>;
}

/// Adapts the crate-local [`AndroidGitCredentialProvider`] callback interface to
/// the shared [`tool_api::GitCredentialProvider`] seam the engine consumes. One
/// forwarding hop per call; both methods are synchronous (libgit2's credentials
/// callback is sync), so no async runtime is involved.
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
struct AndroidGitCredentialProviderBridge {
    inner: Box<dyn AndroidGitCredentialProvider>,
}
#[cfg(feature = "uniffi")]
impl tool_api::GitCredentialProvider for AndroidGitCredentialProviderBridge {
    fn https_token(&self) -> Option<String> {
        self.inner.https_token()
    }
    fn ssh_passphrase(&self) -> Option<String> {
        self.inner.ssh_passphrase()
    }
}

/// Adapts the crate-local [`AndroidStt`] callback interface to the shared
/// [`traits::SpeechToText`] seam the engine consumes. One forwarding hop per
/// call; maps [`SpeechFfiError`] onto [`traits::SttError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
struct AndroidSttBridge {
    inner: Box<dyn AndroidStt>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl traits::SpeechToText for AndroidSttBridge {
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
                SpeechFfiError::Retriable { message } => traits::SttError::Retriable(message),
                SpeechFfiError::Other { message } => traits::SttError::Other(message),
            }),
        }
    }
}

/// Adapts the crate-local [`AndroidTts`] callback interface to the shared
/// [`traits::TextToSpeech`] seam the engine consumes. Maps [`SpeechFfiError`]
/// onto [`traits::TtsError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
struct AndroidTtsBridge {
    inner: Box<dyn AndroidTts>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl traits::TextToSpeech for AndroidTtsBridge {
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
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// T2.2 — the foreign-callable Android engine constructor.
// ---------------------------------------------------------------------------
//
// Mirrors `ios-framework::build_ios_engine`: a thin `#[uniffi::export]` wrapper
// taking ONLY UniFFI-marshalable inputs (the crate-local event listener + the
// stt/tts callback objects + plain config strings), constructing stub camera /
// voice / share capabilities + a no-op permission sink, threading the runtime
// config into a `MobileConfig`, and delegating to the shared
// `engine_mobile::build_mobile_engine`. ADDITIVE — it does not touch the
// existing non-exported `build_mobile_engine` above, the `traits` crate, or iOS.

/// Device-capability stubs for the camera / voice / share callbacks the Android
/// constructor does not (yet) wire. A text/speech conversation never invokes
/// these; each returns the trait's "unavailable" error. Mirrors
/// `ios-framework`'s `stub_capabilities`.
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
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

/// A [`PermissionRequestSink`] that drops outbound permission requests (mirrors
/// `ios-framework`'s `NoopPermissionSink`). Mobile always binds the adapter
/// permission gate; with no foreign permission UI yet, an unanswered request
/// simply parks the turn (still cancellable).
#[cfg(feature = "uniffi")]
// Reference implementation mirroring `ios-framework`'s `NoopPermissionSink`; the
// Android constructor binds `AndroidPermissionSinkBridge` instead, so this is
// unconstructed on every target — keep it as the documented no-op shape.
#[allow(dead_code)]
struct NoopPermissionSink;

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl engine_mobile::PermissionRequestSink for NoopPermissionSink {
    async fn emit_request(&self, _request: client_protocol::permission::PermissionRequest) {}
}

// ---------------------------------------------------------------------------
// Direct Android Computer Use callback + traits bridge.
// ---------------------------------------------------------------------------

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Error))]
#[derive(Debug, thiserror::Error)]
pub enum AndroidComputerUseFfiError {
    #[error("accessibility service disabled")]
    ServiceDisabled,
    #[error("Computer Use session inactive")]
    SessionInactive,
    #[error("Computer Use permission denied: {message}")]
    PermissionDenied { message: String },
    #[error("target package not allowed: {message}")]
    TargetNotAllowed { message: String },
    #[error("Computer Use tier insufficient: {message}")]
    TierInsufficient { message: String },
    #[error("protected Android surface: {message}")]
    ProtectedSurface { message: String },
    #[error("stale Android node: {message}")]
    StaleNode { message: String },
    #[error("Android Computer Use timeout: {message}")]
    Timeout { message: String },
    #[error("unsupported Android Computer Use operation: {message}")]
    Unsupported { message: String },
    #[error("Android Computer Use error: {message}")]
    Other { message: String },
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct AndroidScreenshotFfi {
    pub width: u32,
    pub height: u32,
    pub png_bytes: Vec<u8>,
}

/// Kotlin-owned Direct-build Computer Use host. JSON is used for the
/// Android-specific tree/action vocabulary so the UniFFI surface stays stable
/// while the strongly typed Rust trait remains the tool contract.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait AndroidComputerUseHost: Send + Sync {
    async fn status_json(&self) -> Result<String, AndroidComputerUseFfiError>;
    async fn request_access_json(
        &self,
        request_json: String,
    ) -> Result<String, AndroidComputerUseFfiError>;
    async fn list_granted_apps_json(&self) -> Result<String, AndroidComputerUseFfiError>;
    async fn screenshot(&self) -> Result<AndroidScreenshotFfi, AndroidComputerUseFfiError>;
    async fn ui_tree_json(&self) -> Result<String, AndroidComputerUseFfiError>;
    async fn find_nodes_json(
        &self,
        query_json: String,
    ) -> Result<String, AndroidComputerUseFfiError>;
    async fn inspect_node_json(
        &self,
        node_id: String,
    ) -> Result<String, AndroidComputerUseFfiError>;
    async fn perform_json(&self, action_json: String)
        -> Result<String, AndroidComputerUseFfiError>;
    async fn wait_for_json(
        &self,
        condition_json: String,
        timeout_ms: u64,
    ) -> Result<String, AndroidComputerUseFfiError>;
    async fn listen_json(&self, request_json: String)
        -> Result<String, AndroidComputerUseFfiError>;
    async fn speak_json(&self, request_json: String) -> Result<String, AndroidComputerUseFfiError>;
    async fn stop_audio(&self) -> Result<(), AndroidComputerUseFfiError>;
    async fn stop(&self) -> Result<(), AndroidComputerUseFfiError>;
}

#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
struct AndroidComputerUseBridge {
    inner: Box<dyn AndroidComputerUseHost>,
}

#[cfg(feature = "uniffi")]
fn computer_use_error_from_ffi(
    error: AndroidComputerUseFfiError,
) -> traits::AndroidAutomationError {
    use traits::AndroidAutomationError as Target;
    match error {
        AndroidComputerUseFfiError::ServiceDisabled => Target::ServiceDisabled,
        AndroidComputerUseFfiError::SessionInactive => Target::SessionInactive,
        AndroidComputerUseFfiError::PermissionDenied { message } => {
            Target::PermissionDenied(message)
        }
        AndroidComputerUseFfiError::TargetNotAllowed { message } => {
            Target::TargetNotAllowed(message)
        }
        AndroidComputerUseFfiError::TierInsufficient { message } => {
            Target::TierInsufficient(message)
        }
        AndroidComputerUseFfiError::ProtectedSurface { message } => {
            Target::ProtectedSurface(message)
        }
        AndroidComputerUseFfiError::StaleNode { message } => Target::StaleNode(message),
        AndroidComputerUseFfiError::Timeout { message } => Target::Timeout(message),
        AndroidComputerUseFfiError::Unsupported { message } => Target::Unsupported(message),
        AndroidComputerUseFfiError::Other { message } => Target::Other(message),
    }
}

#[cfg(feature = "uniffi")]
fn decode_computer_use_json<T: serde::de::DeserializeOwned>(
    value: String,
) -> Result<T, traits::AndroidAutomationError> {
    serde_json::from_str(&value).map_err(|error| {
        traits::AndroidAutomationError::Other(format!("invalid host JSON: {error}"))
    })
}

#[cfg(feature = "uniffi")]
fn encode_computer_use_json<T: serde::Serialize>(
    value: &T,
) -> Result<String, traits::AndroidAutomationError> {
    serde_json::to_string(value).map_err(|error| {
        traits::AndroidAutomationError::Other(format!("cannot encode host JSON: {error}"))
    })
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl traits::AndroidUiAutomation for AndroidComputerUseBridge {
    async fn status(
        &self,
    ) -> Result<traits::AndroidAutomationStatus, traits::AndroidAutomationError> {
        let value = self
            .inner
            .status_json()
            .await
            .map_err(computer_use_error_from_ffi)?;
        decode_computer_use_json(value)
    }

    async fn request_access(
        &self,
        request: traits::AndroidAccessRequest,
    ) -> Result<Vec<traits::AndroidAppInfo>, traits::AndroidAutomationError> {
        let request = encode_computer_use_json(&request)?;
        let value = self
            .inner
            .request_access_json(request)
            .await
            .map_err(computer_use_error_from_ffi)?;
        decode_computer_use_json(value)
    }

    async fn list_granted_apps(
        &self,
    ) -> Result<Vec<traits::AndroidAppInfo>, traits::AndroidAutomationError> {
        let value = self
            .inner
            .list_granted_apps_json()
            .await
            .map_err(computer_use_error_from_ffi)?;
        decode_computer_use_json(value)
    }

    async fn screenshot(
        &self,
    ) -> Result<traits::AndroidScreenshot, traits::AndroidAutomationError> {
        let value = self
            .inner
            .screenshot()
            .await
            .map_err(computer_use_error_from_ffi)?;
        Ok(traits::AndroidScreenshot {
            width: value.width,
            height: value.height,
            png_bytes: value.png_bytes,
        })
    }

    async fn ui_tree(&self) -> Result<traits::AndroidUiSnapshot, traits::AndroidAutomationError> {
        let value = self
            .inner
            .ui_tree_json()
            .await
            .map_err(computer_use_error_from_ffi)?;
        decode_computer_use_json(value)
    }

    async fn find_nodes(
        &self,
        query: traits::AndroidNodeQuery,
    ) -> Result<Vec<traits::AndroidUiNode>, traits::AndroidAutomationError> {
        let query = encode_computer_use_json(&query)?;
        let value = self
            .inner
            .find_nodes_json(query)
            .await
            .map_err(computer_use_error_from_ffi)?;
        decode_computer_use_json(value)
    }

    async fn inspect_node(
        &self,
        node_id: String,
    ) -> Result<traits::AndroidUiNode, traits::AndroidAutomationError> {
        let value = self
            .inner
            .inspect_node_json(node_id)
            .await
            .map_err(computer_use_error_from_ffi)?;
        decode_computer_use_json(value)
    }

    async fn perform(
        &self,
        action: traits::AndroidAction,
    ) -> Result<traits::AndroidActionResult, traits::AndroidAutomationError> {
        let action = encode_computer_use_json(&action)?;
        let value = self
            .inner
            .perform_json(action)
            .await
            .map_err(computer_use_error_from_ffi)?;
        decode_computer_use_json(value)
    }

    async fn wait_for(
        &self,
        condition: traits::AndroidWaitCondition,
        timeout_ms: u64,
    ) -> Result<traits::AndroidActionResult, traits::AndroidAutomationError> {
        let condition = encode_computer_use_json(&condition)?;
        let value = self
            .inner
            .wait_for_json(condition, timeout_ms)
            .await
            .map_err(computer_use_error_from_ffi)?;
        decode_computer_use_json(value)
    }

    async fn listen(
        &self,
        request: traits::AndroidAudioListenRequest,
    ) -> Result<traits::AndroidAudioTranscript, traits::AndroidAutomationError> {
        let request = encode_computer_use_json(&request)?;
        let value = self
            .inner
            .listen_json(request)
            .await
            .map_err(computer_use_error_from_ffi)?;
        decode_computer_use_json(value)
    }

    async fn speak(
        &self,
        request: traits::AndroidAudioSpeakRequest,
    ) -> Result<traits::AndroidAudioSpeakResult, traits::AndroidAutomationError> {
        let request = encode_computer_use_json(&request)?;
        let value = self
            .inner
            .speak_json(request)
            .await
            .map_err(computer_use_error_from_ffi)?;
        decode_computer_use_json(value)
    }

    async fn stop_audio(&self) -> Result<(), traits::AndroidAutomationError> {
        self.inner
            .stop_audio()
            .await
            .map_err(computer_use_error_from_ffi)
    }

    async fn stop(&self) -> Result<(), traits::AndroidAutomationError> {
        self.inner.stop().await.map_err(computer_use_error_from_ffi)
    }
}

/// The Kotlin-implemented permission sink the Android app registers when it builds
/// the engine. Defined in THIS crate (not re-used from `engine-mobile`) so its
/// `UniFFI` converter registers under `android_aar`'s tag — a prerequisite for
/// naming it as a parameter type in [`build_android_engine`]. Mirrors
/// `AndroidEventListener`: where the listener carries OUTBOUND events, this carries
/// the engine's OUTBOUND permission requests to the Kotlin host's prompt UI; the
/// inbound resolution flows back through
/// `MobileEngineHandle::submit(ClientCommand::Approve/DenyPermission)`.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait AndroidPermissionSink: Send + Sync {
    /// Deliver one outbound [`client_protocol::permission::PermissionRequest`] to
    /// the Kotlin host. Implementations enqueue a prompt and return promptly —
    /// they must not block the engine turn loop; the user's answer comes back via
    /// `MobileEngineHandle::submit`.
    async fn on_request(&self, request: client_protocol::permission::PermissionRequest);
}

/// Adapts the crate-local [`AndroidPermissionSink`] callback interface to the
/// shared [`PermissionRequestSink`] the engine's adapter gate emits onto. One
/// forwarding hop per request; no transformation. Mirrors [`AndroidListenerBridge`].
///
/// Constructed only on the `target_os = "android"` path of
/// [`build_android_engine`]; `allow(dead_code)` on the host bindgen build (where
/// that path is `cfg`'d out).
#[cfg(feature = "uniffi")]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
struct AndroidPermissionSinkBridge {
    inner: Box<dyn AndroidPermissionSink>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl engine_mobile::PermissionRequestSink for AndroidPermissionSinkBridge {
    async fn emit_request(&self, request: client_protocol::permission::PermissionRequest) {
        self.inner.on_request(request).await;
    }
}

/// The Kotlin-implemented event listener the Android app registers when it builds
/// the engine. Defined in THIS crate (not re-used from `client-adapter`) so its
/// `UniFFI` converter registers under `android_aar`'s tag — a prerequisite for
/// naming it as a parameter type in [`build_android_engine`]. Mirrors
/// `ios-framework::IosEventListener`.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
#[async_trait::async_trait]
pub trait AndroidEventListener: Send + Sync {
    /// Deliver one fully-lowered [`client_protocol::events::ClientEvent`] to the
    /// Kotlin host. Implementations enqueue onto the UI's event stream and return
    /// promptly — they must not block the engine turn loop.
    async fn on_event(&self, event: client_protocol::events::ClientEvent);
}

/// Adapts the crate-local [`AndroidEventListener`] callback interface to the
/// shared [`ClientEventListener`] the engine's adapter sink expects. One
/// forwarding hop per event; no transformation.
#[cfg(feature = "uniffi")]
struct AndroidListenerBridge {
    inner: Box<dyn AndroidEventListener>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl ClientEventListener for AndroidListenerBridge {
    async fn on_event(&self, event: client_protocol::events::ClientEvent) {
        self.inner.on_event(event).await;
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
fn android_shell_gate(
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
fn android_git_gate(enable_git: bool, workspace_ready: bool, ca_store_reachable: bool) -> bool {
    enable_git && workspace_ready && ca_store_reachable
}

/// P5b bundled-shell bootstrap result — the typed values the android branch of
/// [`build_android_engine`] threads onto `AndroidShellConfig` (so `prepare`
/// targets the bundled mksh + leads PATH with the applet farm) and onto the
/// capability cache + Shell-tool ctx. `Some` ONLY when staging + exec
/// verification fully succeeded (spec P5b §B2 fail-closed).
#[cfg(target_os = "android")]
struct BundledShell {
    mksh_path: std::path::PathBuf,
    mksh_hash: String,
    applet_dir: std::path::PathBuf,
    mksh_version: Option<String>,
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
fn bootstrap_bundled_shell(native_library_dir: &str, app_files_root: &str) -> Option<BundledShell> {
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
/// runtime and streams every [`client_protocol::events::ClientEvent`] to
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
/// - `stt` / `tts` — the foreign speech callbacks (bridged to
///   [`traits::SpeechToText`] / [`traits::TextToSpeech`]).
/// - `camera` — the foreign camera callback (bridged to
///   [`traits::CameraControl`]) so `tool-camera` routes through `CameraX` +
///   the system photo picker.
/// - `share` — the foreign share callback (bridged to
///   [`traits::SharingService`]) so `tool-share` routes through the system
///   `Intent.ACTION_SEND` share sheet.
/// - `location` — the foreign one-shot location callback (bridged to
///   [`traits::LocationProvider`]) used by approved local-app bridge requests.
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
    stt: Box<dyn AndroidStt>,
    tts: Box<dyn AndroidTts>,
    camera: Box<dyn AndroidCamera>,
    share: Box<dyn AndroidShare>,
    voice: Box<dyn AndroidVoice>,
    location: Box<dyn AndroidLocation>,
    notifications: Box<dyn AndroidNotification>,
    clipboard: Box<dyn AndroidClipboard>,
    permissions: Box<dyn AndroidPermissionSink>,
    computer_use: Option<Box<dyn AndroidComputerUseHost>>,
    shell: Option<AndroidShellConfigFfi>,
    git: Option<AndroidGitConfigFfi>,
    git_credential_provider: Option<Box<dyn AndroidGitCredentialProvider>>,
    secure_storage: Option<Box<dyn AndroidSecureStorage>>,
) -> Result<Arc<MobileEngineHandle>, MobileEngineError> {
    build_android_engine_with_mobile_linux(
        AndroidEngineLaunchConfigFfi {
            api_base,
            api_key,
            model,
            app_files_root,
            project_cwd: None,
            provider_config: None,
            mobile_linux: None,
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
        location,
        notifications,
        clipboard,
        permissions,
        computer_use,
        shell,
        git,
        git_credential_provider,
        secure_storage,
    )
}

#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_lines)]
pub fn build_android_engine_with_mobile_linux(
    config: AndroidEngineLaunchConfigFfi,
    listener: Box<dyn AndroidEventListener>,
    stt: Box<dyn AndroidStt>,
    tts: Box<dyn AndroidTts>,
    camera: Box<dyn AndroidCamera>,
    share: Box<dyn AndroidShare>,
    voice: Box<dyn AndroidVoice>,
    location: Box<dyn AndroidLocation>,
    notifications: Box<dyn AndroidNotification>,
    clipboard: Box<dyn AndroidClipboard>,
    permissions: Box<dyn AndroidPermissionSink>,
    computer_use: Option<Box<dyn AndroidComputerUseHost>>,
    shell: Option<AndroidShellConfigFfi>,
    git: Option<AndroidGitConfigFfi>,
    git_credential_provider: Option<Box<dyn AndroidGitCredentialProvider>>,
    secure_storage: Option<Box<dyn AndroidSecureStorage>>,
) -> Result<Arc<MobileEngineHandle>, MobileEngineError> {
    let AndroidEngineLaunchConfigFfi {
        api_base,
        api_key,
        model,
        app_files_root,
        project_cwd,
        provider_config,
        mobile_linux,
        local_apps_full_runtime,
        local_apps_runtime_root,
        physical_memory_bytes,
    } = config;
    let listener: Arc<dyn ClientEventListener> =
        Arc::new(AndroidListenerBridge { inner: listener });
    #[cfg(not(target_os = "android"))]
    let _ = project_cwd;
    #[cfg(target_os = "android")]
    {
        use platform_android::{AndroidPlatform, AndroidPlatformInputs};
        // P5b: capture the app-private files root as an owned `String` up front —
        // `app_files_root` is consumed below into `AndroidPlatformInputs`, but the
        // bundled-shell bootstrap (which must run BEFORE `shell_cfg` is built +
        // moved) needs it to stage the applet symlink farm under it.
        let app_files_root_str = app_files_root.clone();
        let local_apps_runtime_requested = local_apps_runtime_root.is_some();
        let cwd = android_project_cwd(&app_files_root, project_cwd.as_deref())?;
        let mut cfg = MobileConfig {
            cwd,
            lingxi_home: std::path::PathBuf::from(&app_files_root).join(branding::DOT_DIR),
            local_apps_full_runtime,
            local_apps_runtime_root: local_apps_runtime_root.map(std::path::PathBuf::from),
            physical_memory_bytes,
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
            let (profiles, routing) = engine_mobile::parse_mobile_provider_config_json(
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
            .map_or(traits::MobileLinuxRuntimeMode::Legacy, |cfg| {
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
                voice: Arc::new(AndroidVoiceBridge { inner: voice }),
                location: Some(Arc::new(AndroidLocationBridge { inner: location })),
                share: Arc::new(AndroidShareBridge { inner: share }),
                stt: Some(Arc::new(AndroidSttBridge { inner: stt })),
                tts: Some(Arc::new(AndroidTtsBridge { inner: tts })),
                notifications: Some(Arc::new(AndroidNotificationBridge {
                    inner: notifications,
                })),
                clipboard: Some(Arc::new(AndroidClipboardBridge { inner: clipboard })),
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
                        as std::sync::Arc<dyn traits::SecureStorage>
                }),
                android_ui_automation: computer_use.map(|host| {
                    std::sync::Arc::new(AndroidComputerUseBridge { inner: host })
                        as std::sync::Arc<dyn traits::AndroidUiAutomation>
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
            traits::MobileLinuxRuntimeMode::MobileLinux
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
        engine_mobile::build_mobile_engine(cfg, platform, listener, permission_sink)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (
            api_base,
            api_key,
            model,
            app_files_root,
            listener,
            stt,
            tts,
            camera,
            share,
            voice,
            location,
            notifications,
            clipboard,
            permissions,
            computer_use,
            shell,
            git,
            provider_config,
            mobile_linux,
            git_credential_provider,
            secure_storage,
        );
        Err(MobileEngineError::PlatformUnavailable)
    }
}

/// P0a gate probe: run the on-device minijail smoke and return it as JSON
/// (`{"ok":bool,"no_new_privs":bool,"child_exit_zero":bool,"reason":...}`).
/// Keys are serde's default `snake_case` (`SmokeResult` has no `rename_all`).
/// Host builds report the structural reason.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
#[must_use]
pub fn android_sandbox_smoke() -> String {
    #[cfg(target_os = "android")]
    {
        serde_json::to_string(&platform_android_minijail::minijail_smoke())
            .unwrap_or_else(|e| format!("{{\"ok\":false,\"reason\":\"serialize: {e}\"}}"))
    }
    #[cfg(not(target_os = "android"))]
    {
        "{\"ok\":false,\"reason\":\"host build\"}".to_string()
    }
}

/// P2 acceptance probe: exercise the REAL prepare→runner→`run_jailed` path for
/// one `command` rooted at `workspace`, and return the outcome as JSON
/// (`{"stdout","stderr","exit_code","timed_out","enforcement_failed"}`).
///
/// This is NOT a bypass: it constructs a probed [`CapabilityCache`], an
/// [`AndroidMinijailSandbox`] + [`AndroidMinijailProcessRunner`] over that SAME
/// cache, `prepare()`s the command under a deny-net policy, and `run()`s it
/// through the same jailed fork/exec the engine uses. The wall-clock timeout is
/// hardcoded to **2 seconds** so a `sleep 10` probe reliably trips the watchdog
/// (`timed_out=true`) without making the instrumentation test slow.
///
/// `enforcement_failed` is `null` on success; on the host build (no Android
/// device) it is `"host build"` and the rest are empty/zero — so a JVM-host run
/// fails loudly rather than silently passing.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
#[allow(clippy::needless_pass_by_value)] // FFI export: UniFFI marshals owned `String`.
#[must_use]
pub fn android_sandbox_run_probe(command: String, workspace: String) -> String {
    #[cfg(not(target_os = "android"))]
    {
        let _ = (command, workspace);
        "{\"enforcement_failed\":\"host build\"}".to_string()
    }
    #[cfg(target_os = "android")]
    {
        use platform_android::{
            capabilities::{probe_android_capabilities, CapabilityCache},
            AndroidMinijailProcessRunner, AndroidMinijailSandbox, AndroidShellConfig,
        };
        use std::collections::HashMap;
        use traits::{
            NetworkPolicy, ProcessCommand, ProcessRunner, ResourceLimits, Sandbox, SandboxPolicy,
        };

        // The probe runtime: capability probe + the jailed run are independent
        // of the engine runtime, so spin up a transient current-thread runtime
        // and drop it (mirrors `build_android_engine`'s eager-probe seam).
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                return format!("{{\"enforcement_failed\":\"probe runtime build: {e}\"}}");
            }
        };

        // Probe REAL capabilities and share ONE cache across sandbox + runner —
        // exactly as `AndroidPlatform::new` does.
        let cache = Arc::new(CapabilityCache::new());
        cache.set(rt.block_on(probe_android_capabilities()));

        let ws = std::path::PathBuf::from(&workspace);
        let cfg = AndroidShellConfig {
            native_library_dir: ws.join("native-lib"),
            shell_workspace_root: ws.clone(),
            app_cache_root: ws.join("cache"),
            package_name: "com.lingxi.code".to_string(),
            package_version_code: 1,
            app_writable_roots: vec![ws.clone()],
            enable_shell: true,
            secrets_in_keystore: true,
            shell_data_exposure_accepted: true,
            bundled_mksh_path: None,
            bundled_mksh_hash: None,
            bundled_applet_dir: None,
        };
        let sandbox = AndroidMinijailSandbox::new(cfg, cache.clone());
        let runner = AndroidMinijailProcessRunner::new(cache);

        // Deny-net policy (the P2 acceptance default). 2s wall-clock timeout so
        // `sleep 10` trips the watchdog.
        // Request NO filesystem confinement: Android's fs boundary is the app
        // UID, not Landlock (shipping kernels disable it), so `plan_from_policy`
        // rejects any non-empty writable/denied path set. Net-deny is the only
        // active confinement here.
        let policy = SandboxPolicy {
            network: NetworkPolicy::Disabled,
            writable_paths: vec![],
            denied_paths: vec![],
            allow_subprocess: true,
            limits: ResourceLimits::default(),
        };
        let proc_cmd = ProcessCommand {
            command: "/system/bin/sh".to_string(),
            args: vec!["-c".to_string(), command],
            cwd: Some(ws),
            env: HashMap::new(),
            timeout: Some(std::time::Duration::from_secs(2)),
            stdin: None,
        };

        let prepared = match sandbox.prepare(proc_cmd, &policy) {
            Ok(p) => p,
            Err(e) => {
                return format!(
                    "{{\"enforcement_failed\":\"prepare: {}\"}}",
                    e.to_string().replace('"', "'")
                );
            }
        };
        match rt.block_on(runner.run(&prepared)) {
            Ok(out) => serde_json::json!({
                "stdout": out.stdout,
                "stderr": out.stderr,
                "exit_code": out.exit_code,
                "timed_out": out.timed_out,
                "enforcement_failed": serde_json::Value::Null,
            })
            .to_string(),
            Err(e) => format!(
                "{{\"enforcement_failed\":\"run: {}\"}}",
                e.to_string().replace('"', "'")
            ),
        }
    }
}

/// P5c acceptance probe: exercise the REAL prepare→runner→`run_jailed` path for
/// one `command` running through the BUNDLED mksh+toybox (not the device's
/// system sh), and return the outcome as JSON
/// (`{"stdout","stderr","exit_code","timed_out","enforcement_failed"}`).
///
/// This is the on-device end-to-end proof of the P5 bundled-shell chain. It
/// mirrors [`android_sandbox_run_probe`] but first bootstraps the bundled shell
/// via [`bootstrap_bundled_shell`] (stage the toybox applet symlink farm, verify
/// bundled execve + dispatch, sha256 `libmksh.so`), then sets the THREE bundled
/// `AndroidShellConfig` fields from the result. Because those fields are set,
/// `prepare()` selects `ExecTarget::BundledHelper{mksh}` and leads PATH with the
/// applet farm, and the runner content-identity-checks the recorded sha256
/// against the real `libmksh.so` before execve (spec P5b §T4b). So a non-empty
/// `stdout` from a bundled command implicitly proves staging + the hash check +
/// the jailed bundled exec all passed end-to-end.
///
/// The wall-clock timeout is hardcoded to **2 seconds** so a `sleep 10` probe
/// reliably trips the watchdog (`timed_out=true`). The deny-net `SandboxPolicy`
/// is the same one P2 proved (`net_deny_verified`).
///
/// `enforcement_failed` is `null` on success; on the host build (no Android
/// device) it is `"host build"`, and `"bundled bootstrap failed"` if
/// `bootstrap_bundled_shell` returns `None` — so a JVM-host run or a broken
/// bundle fails loudly rather than silently passing.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
#[allow(clippy::needless_pass_by_value)] // FFI export: UniFFI marshals owned `String`.
#[must_use]
pub fn android_bundled_shell_run_probe(
    native_lib_dir: String,
    app_files_root: String,
    command: String,
) -> String {
    #[cfg(not(target_os = "android"))]
    {
        let _ = (native_lib_dir, app_files_root, command);
        "{\"enforcement_failed\":\"host build\"}".to_string()
    }
    #[cfg(target_os = "android")]
    {
        use platform_android::{
            capabilities::{probe_android_capabilities, CapabilityCache},
            AndroidMinijailProcessRunner, AndroidMinijailSandbox, AndroidShellConfig,
        };
        use std::collections::HashMap;
        use traits::{
            NetworkPolicy, ProcessCommand, ProcessRunner, ResourceLimits, Sandbox, SandboxPolicy,
        };

        // Bootstrap the bundled shell: stage the applet symlink farm, verify
        // bundled execve + dispatch, sha256 libmksh.so. `None` = fail-closed.
        let Some(bundled) = bootstrap_bundled_shell(&native_lib_dir, &app_files_root) else {
            return "{\"enforcement_failed\":\"bundled bootstrap failed\"}".to_string();
        };

        // The probe runtime: capability probe + the jailed run are independent
        // of the engine runtime, so spin up a transient current-thread runtime
        // and drop it (mirrors `android_sandbox_run_probe`).
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                return format!("{{\"enforcement_failed\":\"probe runtime build: {e}\"}}");
            }
        };

        // Probe REAL capabilities and share ONE cache across sandbox + runner.
        let cache = Arc::new(CapabilityCache::new());
        cache.set(rt.block_on(probe_android_capabilities()));

        let root = std::path::PathBuf::from(&app_files_root);
        let cfg = AndroidShellConfig {
            native_library_dir: std::path::PathBuf::from(&native_lib_dir),
            shell_workspace_root: root.clone(),
            app_cache_root: root.join("cache"),
            package_name: "com.lingxi.code".to_string(),
            package_version_code: 1,
            app_writable_roots: vec![root.clone()],
            enable_shell: true,
            secrets_in_keystore: true,
            shell_data_exposure_accepted: true,
            // The three bundled fields drive `prepare` onto BundledHelper{mksh}
            // + the applet-farm PATH, and the runner's content-identity check.
            bundled_mksh_path: Some(bundled.mksh_path),
            bundled_mksh_hash: Some(bundled.mksh_hash),
            bundled_applet_dir: Some(bundled.applet_dir),
        };
        let sandbox = AndroidMinijailSandbox::new(cfg, cache.clone());
        let runner = AndroidMinijailProcessRunner::new(cache);

        // Deny-net policy (the P2 acceptance default, same filter P2 proved via
        // `net_deny_verified`). 2s wall-clock timeout so `sleep 10` trips the
        // watchdog. No filesystem confinement (Android's fs boundary is the app
        // UID, not Landlock); net-deny is the only active confinement here.
        let policy = SandboxPolicy {
            network: NetworkPolicy::Disabled,
            writable_paths: vec![],
            denied_paths: vec![],
            allow_subprocess: true,
            limits: ResourceLimits::default(),
        };
        let proc_cmd = ProcessCommand {
            // `prepare` normalizes the system-sh sentinel onto BundledHelper{mksh}
            // because the bundled fields are set.
            command: "/system/bin/sh".to_string(),
            args: vec!["-c".to_string(), command],
            cwd: Some(root),
            env: HashMap::new(),
            timeout: Some(std::time::Duration::from_secs(2)),
            stdin: None,
        };

        let prepared = match sandbox.prepare(proc_cmd, &policy) {
            Ok(p) => p,
            Err(e) => {
                return format!(
                    "{{\"enforcement_failed\":\"prepare: {}\"}}",
                    e.to_string().replace('"', "'")
                );
            }
        };
        match rt.block_on(runner.run(&prepared)) {
            Ok(out) => serde_json::json!({
                "stdout": out.stdout,
                "stderr": out.stderr,
                "exit_code": out.exit_code,
                "timed_out": out.timed_out,
                "enforcement_failed": serde_json::Value::Null,
            })
            .to_string(),
            Err(e) => format!(
                "{{\"enforcement_failed\":\"run: {}\"}}",
                e.to_string().replace('"', "'")
            ),
        }
    }
}

/// P2 acceptance probe: run the REAL capability probe and return the matrix as
/// JSON (`{"net_deny_verified":bool,"seccomp_filter":bool,...}`). The strong
/// proof of net-deny enforcement is `net_deny_verified` — the probe forked a
/// child under the net-deny seccomp filter and observed `socket()` ⇒ `EPERM`.
/// Host builds report `{"probed":false,"net_deny_verified":false}`.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
#[must_use]
pub fn android_sandbox_capabilities() -> String {
    #[cfg(not(target_os = "android"))]
    {
        "{\"probed\":false,\"net_deny_verified\":false,\"reason\":\"host build\"}".to_string()
    }
    #[cfg(target_os = "android")]
    {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => return format!("{{\"probed\":false,\"reason\":\"probe runtime: {e}\"}}"),
        };
        let caps = rt.block_on(platform_android::capabilities::probe_android_capabilities());
        serde_json::json!({
            "probed": caps.probed,
            "minijail_smoke": caps.minijail_smoke,
            "no_new_privs": caps.no_new_privs,
            "seccomp_filter": caps.seccomp_filter,
            "seccomp_tsync": caps.seccomp_tsync,
            "net_deny_verified": caps.net_deny_verified,
            "pgid_kill": caps.pgid_kill,
            "landlock_abi": caps.landlock_abi,
            "system_sh_version": caps.system_sh_version,
            "toybox_applets": caps.toybox_applets,
            "reason": caps.reason,
        })
        .to_string()
    }
}

/// P4d acceptance probe: drive the REAL [`tool_git_mobile::GitTool`] path for one
/// structured git operation, and return the `ToolCallResult` data (or the error)
/// as JSON. This is the on-device proof that libgit2's OpenSSL TLS + the Android
/// system cacerts (`/system/etc/security/cacerts`) verify a real HTTPS clone —
/// the one thing the host `file://` tests (P4b/c) cannot exercise.
///
/// It is NOT a bypass: it builds a [`tool_api::BuiltinToolContext`] with
/// `android_git = Some(AndroidGitToolCtx { enabled: true, has_token: false,
/// workspace_root })` + `android_git_secret = Some(AndroidGitSecret {
/// credential_provider: None, ca_dir: Some(ca_cert_dir), .. })`, constructs the
/// `GitTool`, parses
/// `operation_json` into the tool input `Value`, and runs `GitTool::call(..)` on
/// a transient current-thread runtime — exactly the engine's path. No token is
/// needed: the acceptance clone targets a PUBLIC repo.
///
/// - `operation_json` — the structured tool input (e.g.
///   `{"operation":"clone","repo_url":"https://github.com/.../x.git","repo":"cloned"}`).
/// - `workspace` — the app-private workspace root all ops are anchored under.
/// - `ca_cert_dir` — the system CA-certificate directory for TLS verification
///   (the device passes `/system/etc/security/cacerts`).
///
/// Returns the `ToolCallResult.data` JSON on success, or `{"error": "..."}` on a
/// parse / tool error. On the host build (no Android device) it returns
/// `{"error":"host build"}` so a JVM-host run fails loudly rather than silently
/// passing.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
#[allow(clippy::needless_pass_by_value)] // FFI export: UniFFI marshals owned `String`.
#[must_use]
pub fn android_git_probe(operation_json: String, workspace: String, ca_cert_dir: String) -> String {
    #[cfg(not(target_os = "android"))]
    {
        let _ = (operation_json, workspace, ca_cert_dir);
        "{\"error\":\"host build\"}".to_string()
    }
    #[cfg(target_os = "android")]
    {
        use tool_api::test_support::{fresh_ctx, fresh_tx, shell_test_ctx};
        use tool_api::tool_trait::Tool;
        use tool_api::{AndroidGitSecret, AndroidGitToolCtx};
        use traits::process::ProcessOutput;

        // Parse the structured operation input. A malformed payload is a probe
        // error, not a tool failure.
        let input: serde_json::Value = match serde_json::from_str(&operation_json) {
            Ok(v) => v,
            Err(e) => {
                return format!(
                    "{{\"error\":\"parse operation_json: {}\"}}",
                    e.to_string().replace('"', "'")
                );
            }
        };

        // Build the REAL BuiltinToolContext. The git ops drive libgit2 + the
        // filesystem directly, so the test-support stub fs/process/sandbox
        // handles are inert here; only `android_git` + `android_git_secret`
        // matter. No token (public repo); the CA dir points at the system store.
        let mut ctx = shell_test_ctx(ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        });
        ctx.android_git = Some(AndroidGitToolCtx {
            enabled: true,
            has_token: false,
            workspace_root: workspace,
        });
        ctx.android_git_secret = Some(AndroidGitSecret {
            credential_provider: None,
            ca_dir: if ca_cert_dir.is_empty() {
                None
            } else {
                Some(ca_cert_dir)
            },
            ssh_private_key_path: None,
            ssh_public_key_path: None,
            ssh_known_hosts_sha256_hex: Vec::new(),
        });

        let tool = tool_git_mobile::GitTool::new(ctx);

        // The clone/local ops are sync inside an async `call`; run on a transient
        // current-thread runtime and drop it (mirrors `android_sandbox_run_probe`).
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => return format!("{{\"error\":\"probe runtime build: {e}\"}}"),
        };
        match rt.block_on(tool.call(input, fresh_ctx(), fresh_tx())) {
            Ok(result) => serde_json::to_string(&result.data)
                .unwrap_or_else(|e| format!("{{\"error\":\"serialize: {e}\"}}")),
            Err(e) => format!(
                "{{\"error\":\"{}\"}}",
                e.to_string().replace('"', "'").replace('\n', " ")
            ),
        }
    }
}

/// Authenticated variant of [`android_git_probe`] for the G2 (push), G7 (SSH),
/// and per-op credential-provider device-acceptance tests. Identical to
/// `android_git_probe` except it wires the per-op secret path:
///
/// - `credential_provider` — a host [`AndroidGitCredentialProvider`] bridged onto
///   `tool_api::GitCredentialProvider`; supplies the HTTPS token / SSH passphrase
///   lazily per network op (so a Keystore-backed provider's round-trip is
///   exercised on real ops).
/// - `ssh_private_key_path` / `ssh_public_key_path` — on-device key paths for an
///   SSH remote (G7). `None` keeps HTTPS behavior.
/// - `ssh_known_hosts_sha256_hex` — the pinned host-key SHA-256 hex set (G7
///   strict verification; an empty set makes any SSH op fail closed).
///
/// Returns the op `data` JSON or `{"error": "..."}`; host build →
/// `{"error":"host build"}` so a JVM-host run fails loudly.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
#[allow(clippy::needless_pass_by_value)] // FFI export: UniFFI marshals owned values.
#[must_use]
pub fn android_git_probe_authed(
    operation_json: String,
    workspace: String,
    ca_cert_dir: String,
    credential_provider: Option<Box<dyn AndroidGitCredentialProvider>>,
    ssh_private_key_path: Option<String>,
    ssh_public_key_path: Option<String>,
    ssh_known_hosts_sha256_hex: Vec<String>,
) -> String {
    #[cfg(not(target_os = "android"))]
    {
        let _ = (
            operation_json,
            workspace,
            ca_cert_dir,
            credential_provider,
            ssh_private_key_path,
            ssh_public_key_path,
            ssh_known_hosts_sha256_hex,
        );
        "{\"error\":\"host build\"}".to_string()
    }
    #[cfg(target_os = "android")]
    {
        use tool_api::test_support::{fresh_ctx, fresh_tx, shell_test_ctx};
        use tool_api::tool_trait::Tool;
        use tool_api::{AndroidGitSecret, AndroidGitToolCtx};
        use traits::process::ProcessOutput;

        let input: serde_json::Value = match serde_json::from_str(&operation_json) {
            Ok(v) => v,
            Err(e) => {
                return format!(
                    "{{\"error\":\"parse operation_json: {}\"}}",
                    e.to_string().replace('"', "'")
                );
            }
        };

        // Bridge the host provider onto the engine trait (same as build_android_engine).
        let provider: Option<std::sync::Arc<dyn tool_api::GitCredentialProvider>> =
            credential_provider.map(|p| {
                std::sync::Arc::new(AndroidGitCredentialProviderBridge { inner: p })
                    as std::sync::Arc<dyn tool_api::GitCredentialProvider>
            });

        let mut ctx = shell_test_ctx(ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        });
        ctx.android_git = Some(AndroidGitToolCtx {
            enabled: true,
            has_token: provider.is_some(),
            workspace_root: workspace,
        });
        ctx.android_git_secret = Some(AndroidGitSecret {
            credential_provider: provider,
            ca_dir: if ca_cert_dir.is_empty() {
                None
            } else {
                Some(ca_cert_dir)
            },
            ssh_private_key_path,
            ssh_public_key_path,
            ssh_known_hosts_sha256_hex,
        });

        let tool = tool_git_mobile::GitTool::new(ctx);
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => return format!("{{\"error\":\"probe runtime build: {e}\"}}"),
        };
        match rt.block_on(tool.call(input, fresh_ctx(), fresh_tx())) {
            Ok(result) => serde_json::to_string(&result.data)
                .unwrap_or_else(|e| format!("{{\"error\":\"serialize: {e}\"}}")),
            Err(e) => format!(
                "{{\"error\":\"{}\"}}",
                e.to_string().replace('"', "'").replace('\n', " ")
            ),
        }
    }
}

/// P5a make-or-break gate probe: prove that bundled executables packaged as
/// `lib*.so` under `native_lib_dir` can `execve` under Android 10+ W^X, and
/// decide which toybox applet-resolution mechanism works on-device.
///
/// This is a RAW exec probe — NOT jailed. It only proves the W^X/packaging
/// story (the minijail/deny-net path is unchanged from P2/P3 and proven
/// elsewhere). It returns JSON:
///
/// ```json
/// {"mksh_exec_ok":bool,"applet_symlink_ok":bool,"applet_rewrite_ok":bool,"reason":"..."}
/// ```
///
/// - `mksh_exec_ok`: `<native_lib_dir>/libmksh.so -c 'echo hi'` runs and stdout
///   contains `hi` — proves W^X execve of a bundled executable from
///   nativeLibraryDir works at all.
/// - `applet_symlink_ok`: a symlink `<applet_dir>/grep` → `libtoybox.so`,
///   exec'd as `<applet_dir>/grep foo` over stdin `foo\nbar`, outputs `foo` —
///   proves execve-through-a-symlink-into-nativeLibraryDir + toybox `argv[0]`
///   multicall dispatch under W^X (the PREFERRED applet mechanism for P5b).
/// - `applet_rewrite_ok`: `<native_lib_dir>/libtoybox.so grep foo` over the same
///   stdin outputs `foo` — the command-rewrite FALLBACK mechanism.
///
/// Host builds return `{"error":"host build"}` so a JVM-host run fails loudly
/// rather than silently passing.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
#[allow(clippy::needless_pass_by_value)]
// FFI export: UniFFI marshals owned `String`.
// Single linear probe body (raw mksh exec + symlink-farm + command-rewrite
// applet resolution); the length is intrinsic to the three-mechanism probe, not
// decomposable. Pre-existing P5a debt surfaced by the android-target clippy gate.
#[allow(clippy::too_many_lines)]
#[must_use]
pub fn android_bundled_shell_probe(native_lib_dir: String, applet_dir: String) -> String {
    #[cfg(not(target_os = "android"))]
    {
        let _ = (native_lib_dir, applet_dir);
        "{\"error\":\"host build\"}".to_string()
    }
    #[cfg(target_os = "android")]
    {
        use std::io::Write;
        use std::os::unix::fs::symlink;
        use std::path::Path;
        use std::process::{Command, Stdio};

        let nl = Path::new(&native_lib_dir);
        let mksh = nl.join("libmksh.so");
        let toybox = nl.join("libtoybox.so");
        let mut reason = String::new();

        // (a) RAW mksh execve proof — the make-or-break W^X gate.
        let mksh_exec_ok = match Command::new(&mksh).args(["-c", "echo hi"]).output() {
            Ok(out) => {
                let so = String::from_utf8_lossy(&out.stdout);
                let ok = so.contains("hi");
                if !ok {
                    reason.push_str(&format!(
                        "mksh: status={:?} stdout={:?} stderr={:?}; ",
                        out.status.code(),
                        so,
                        String::from_utf8_lossy(&out.stderr)
                    ));
                }
                ok
            }
            Err(e) => {
                reason.push_str(&format!("mksh spawn: {e}; "));
                false
            }
        };

        // Helper: run a command with stdin "foo\nbar" and assert stdout == "foo".
        let run_grep = |mut cmd: Command, label: &str, reason: &mut String| -> bool {
            cmd.stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    reason.push_str(&format!("{label} spawn: {e}; "));
                    return false;
                }
            };
            if let Some(mut sin) = child.stdin.take() {
                let _ = sin.write_all(b"foo\nbar\n");
            }
            match child.wait_with_output() {
                Ok(out) => {
                    let so = String::from_utf8_lossy(&out.stdout);
                    let ok = so.lines().any(|l| l.trim() == "foo");
                    if !ok {
                        reason.push_str(&format!(
                            "{label}: status={:?} stdout={:?} stderr={:?}; ",
                            out.status.code(),
                            so,
                            String::from_utf8_lossy(&out.stderr)
                        ));
                    }
                    ok
                }
                Err(e) => {
                    reason.push_str(&format!("{label} wait: {e}; "));
                    false
                }
            }
        };

        // (b) Symlink-farm applet resolution (PREFERRED).
        let applet_symlink_ok = {
            let dir = Path::new(&applet_dir);
            let link = dir.join("grep");
            let setup_ok = std::fs::create_dir_all(dir)
                .map_err(|e| reason.push_str(&format!("applet_dir mkdir: {e}; ")))
                .is_ok();
            // Refresh the symlink (ignore a pre-existing one from a warm run).
            let _ = std::fs::remove_file(&link);
            if setup_ok {
                match symlink(&toybox, &link) {
                    Ok(()) => {
                        let mut c = Command::new(&link);
                        c.arg("foo");
                        run_grep(c, "applet_symlink", &mut reason)
                    }
                    Err(e) => {
                        reason.push_str(&format!("symlink: {e}; "));
                        false
                    }
                }
            } else {
                false
            }
        };

        // (c) Command-rewrite applet resolution (FALLBACK).
        let applet_rewrite_ok = {
            let mut c = Command::new(&toybox);
            c.args(["grep", "foo"]);
            run_grep(c, "applet_rewrite", &mut reason)
        };

        serde_json::json!({
            "mksh_exec_ok": mksh_exec_ok,
            "applet_symlink_ok": applet_symlink_ok,
            "applet_rewrite_ok": applet_rewrite_ok,
            "reason": if reason.is_empty() { "ok".to_string() } else { reason },
        })
        .to_string()
    }
}

// F3-04: re-export `engine-mobile`'s UniFFI scaffolding so the shared host's FFI
// symbols (the re-exported `MobileEngineHandle` / `MobileEngineError`) land in
// this crate's final library. Under the `uniffi` feature only.
#[cfg(feature = "uniffi")]
uniffi::setup_scaffolding!();

#[cfg(all(test, feature = "uniffi"))]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use client_protocol::events::ClientEvent;
    use client_protocol::permission::PermissionRequest as PermissionRequestDto;
    use engine_mobile::{ClientEventListener, MobileConfig, PermissionRequestSink};
    use tokio::sync::Mutex;
    use traits::{
        CameraControl, Clock, FileSystem, HttpTransport, LocationProvider, Platform, ProcessRunner,
        Sandbox, SharingService, VoiceRecorder, WorktreeManager,
    };

    struct FakeAndroidLocation {
        failure: Option<&'static str>,
    }

    #[async_trait]
    impl super::AndroidLocation for FakeAndroidLocation {
        async fn current_location(&self) -> Result<super::LocationFixFfi, super::LocationFfiError> {
            match self.failure {
                Some("permission") => Err(super::LocationFfiError::PermissionDenied),
                Some("unavailable") => Err(super::LocationFfiError::Unavailable),
                Some("timeout") => Err(super::LocationFfiError::Timeout),
                Some(message) => Err(super::LocationFfiError::Other {
                    message: message.to_string(),
                }),
                None => Ok(super::LocationFixFfi {
                    latitude: 31.2304,
                    longitude: 121.4737,
                    accuracy_m: Some(20.5),
                    timestamp_ms: 1_753_000_000_000,
                }),
            }
        }
    }

    #[tokio::test]
    async fn android_location_bridge_maps_fix_and_stable_errors() {
        let success = super::AndroidLocationBridge {
            inner: Box::new(FakeAndroidLocation { failure: None }),
        }
        .current_location()
        .await
        .expect("location fix");
        assert_eq!(success.latitude, 31.2304);
        assert_eq!(success.longitude, 121.4737);
        assert_eq!(success.accuracy_m, Some(20.5));
        assert_eq!(success.timestamp_ms, 1_753_000_000_000);

        let permission = super::AndroidLocationBridge {
            inner: Box::new(FakeAndroidLocation {
                failure: Some("permission"),
            }),
        }
        .current_location()
        .await
        .expect_err("permission failure");
        assert!(matches!(
            permission,
            traits::LocationError::PermissionDenied
        ));

        let unavailable = super::AndroidLocationBridge {
            inner: Box::new(FakeAndroidLocation {
                failure: Some("unavailable"),
            }),
        }
        .current_location()
        .await
        .expect_err("unavailable failure");
        assert!(matches!(unavailable, traits::LocationError::Unavailable));

        let timeout = super::AndroidLocationBridge {
            inner: Box::new(FakeAndroidLocation {
                failure: Some("timeout"),
            }),
        }
        .current_location()
        .await
        .expect_err("timeout failure");
        assert!(matches!(timeout, traits::LocationError::Timeout));

        let other = super::AndroidLocationBridge {
            inner: Box::new(FakeAndroidLocation {
                failure: Some("native failure"),
            }),
        }
        .current_location()
        .await
        .expect_err("other failure");
        assert!(matches!(
            other,
            traits::LocationError::Other(message) if message == "native failure"
        ));
    }

    /// Off-device fake [`Platform`] shim (portable `platform-posix-minimal`
    /// handles over a temp root). Lets the SHARED `build_mobile_engine` build a
    /// real handle on CI without an Android device — exactly the spec §8 "prove
    /// from a Kotlin unit test", run on the host.
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

    /// P3-T5 + P5-T7: the Shell-tool registration gate is the conjunction of all
    /// six inputs — it is `true` ONLY when every input is `true`, and `false` if
    /// any single input is `false`. The sixth conjunct (`bundled_shell_ready`,
    /// P5-T7) makes the gate fail-closed when the bundled mksh+toybox bootstrap
    /// did not succeed: no system-sh fallback. Host-testable without a device.
    #[test]
    fn android_shell_gate_is_all_six_conjuncts() {
        use super::android_shell_gate;

        // All six true → enabled.
        assert!(
            android_shell_gate(true, true, true, true, true, true),
            "gate must be enabled when all six conjuncts hold"
        );

        // Each single-false case → disabled. (input index, label) drives the row.
        let cases = [
            (0, "enable_shell"),
            (1, "secrets_gate_satisfied"),
            (2, "caps_available"),
            (3, "seccomp_filter"),
            (4, "net_deny_verified"),
            (5, "bundled_shell_ready"),
        ];
        for (false_idx, label) in cases {
            let mut args = [true; 6];
            args[false_idx] = false;
            assert!(
                !android_shell_gate(args[0], args[1], args[2], args[3], args[4], args[5]),
                "gate must be disabled when {label} is false"
            );
        }
    }

    /// P4-T10: the Git-tool registration gate is the conjunction of all three
    /// inputs — `true` ONLY when every input is `true`, `false` if any single
    /// input is `false`. The token is NOT a gate input (spec §G5). Host-testable.
    #[test]
    fn android_git_gate_is_all_three_conjuncts() {
        use super::android_git_gate;

        // All three true → enabled.
        assert!(
            android_git_gate(true, true, true),
            "gate must be enabled when all three conjuncts hold"
        );

        // Each single-false case → disabled.
        let cases = [
            (0, "enable_git"),
            (1, "workspace_ready"),
            (2, "ca_store_reachable"),
        ];
        for (false_idx, label) in cases {
            let mut args = [true; 3];
            args[false_idx] = false;
            assert!(
                !android_git_gate(args[0], args[1], args[2]),
                "gate must be disabled when {label} is false"
            );
        }
    }

    #[test]
    fn android_project_cwd_accepts_only_managed_workspace_shape() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("lingxi-android-project-{nonce}"));
        let project_id = "12345678-1234-4abc-8def-1234567890ab";
        let workspace = root.join("projects").join(project_id).join("workspace");
        std::fs::create_dir_all(&workspace).expect("create project fixture");

        let legacy = super::android_project_cwd(root.to_str().expect("utf8"), None)
            .expect("missing project cwd preserves the legacy app root");
        assert_eq!(legacy, root.canonicalize().expect("canonical root"));

        let resolved = super::android_project_cwd(
            root.to_str().expect("utf8"),
            Some(workspace.to_str().expect("utf8")),
        )
        .expect("managed project workspace is accepted");
        assert_eq!(
            resolved,
            workspace.canonicalize().expect("canonical workspace")
        );

        let malformed = root.join("projects").join("user-name").join("workspace");
        std::fs::create_dir_all(&malformed).expect("create malformed fixture");
        assert!(
            super::android_project_cwd(
                root.to_str().expect("utf8"),
                Some(malformed.to_str().expect("utf8")),
            )
            .is_err(),
            "user-controlled names must never become project directories"
        );

        let outside = std::env::temp_dir().join(format!("lingxi-outside-project-{nonce}"));
        std::fs::create_dir_all(&outside).expect("create outside fixture");
        assert!(
            super::android_project_cwd(
                root.to_str().expect("utf8"),
                Some(outside.to_str().expect("utf8")),
            )
            .is_err(),
            "workspaces outside filesDir must fail closed"
        );

        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(outside);
    }

    /// P4-T10: the FFI → `AndroidGitToolCtx` mapping yields `enabled = false`
    /// when `enable_git` is false even if the other gate inputs (workspace +
    /// CA store) are satisfied, and regardless of a present credential provider.
    /// Pure-fn level (mirrors the body of [`build_android_engine`]'s git mapping).
    #[test]
    fn ffi_mapping_disabled_when_enable_git_false() {
        use super::android_git_gate;

        let cfg = super::AndroidGitConfigFfi {
            enable_git: false,
            // Use the workspace's own dir so the readiness check would pass.
            workspace_root: env!("CARGO_MANIFEST_DIR").to_string(),
            // Empty CA dir is treated as reachable (libgit2/OpenSSL defaults).
            ca_cert_dir: String::new(),
            ssh_private_key_path: String::new(),
            ssh_public_key_path: String::new(),
            ssh_known_hosts_sha256_hex: Vec::new(),
        };

        let workspace_ready = std::path::Path::new(&cfg.workspace_root).is_dir();
        let ca_store_reachable =
            cfg.ca_cert_dir.is_empty() || std::path::Path::new(&cfg.ca_cert_dir).exists();
        assert!(workspace_ready, "fixture workspace_root must be a real dir");
        assert!(ca_store_reachable, "empty CA dir must count as reachable");

        // Secrets now ride the per-op `AndroidGitCredentialProvider` callback
        // rather than the FFI config; `has_token` reflects whether that provider
        // is present (mirrors `build_android_engine`'s `credential_provider.is_some()`).
        let credential_provider: Option<std::sync::Arc<dyn tool_api::GitCredentialProvider>> = Some(
            std::sync::Arc::new(super::AndroidGitCredentialProviderBridge {
                inner: Box::new(TestCredProvider),
            }),
        );

        let ctx = tool_api::AndroidGitToolCtx {
            enabled: android_git_gate(cfg.enable_git, workspace_ready, ca_store_reachable),
            has_token: credential_provider.is_some(),
            workspace_root: cfg.workspace_root.clone(),
        };

        assert!(
            !ctx.enabled,
            "Git must be disabled when enable_git is false, even with workspace + CA + provider set"
        );
        assert!(
            ctx.has_token,
            "has_token must still reflect a supplied credential provider"
        );
    }

    #[test]
    fn mobile_linux_legacy_config_reports_legacy_status() {
        let status = super::android_mobile_linux_status_from_config(Some(
            &super::AndroidMobileLinuxConfigFfi {
                mode: super::MobileLinuxRuntimeModeFfi::Legacy,
                managed_root: "/tmp/mobile-linux".to_string(),
                workspace_host_path: Some("/tmp/workspaces/default".to_string()),
                stable_workspace_id: Some("default".to_string()),
                abi: "arm64-v8a".to_string(),
                rootfs_version: "v1".to_string(),
                archive_sha256: None,
                authorization_file: None,
            },
        ));
        assert!(matches!(
            status.state,
            super::MobileLinuxRootfsStateFfi::Unsupported
        ));
        assert!(matches!(
            status.mode,
            super::MobileLinuxRuntimeModeFfi::Legacy
        ));
        assert_eq!(status.backend, "android-minijail");
    }

    #[test]
    fn mobile_linux_selected_is_not_blocked_by_distribution_license() {
        let capability = super::android_mobile_linux_capability_from_config(Some(
            &super::AndroidMobileLinuxConfigFfi {
                mode: super::MobileLinuxRuntimeModeFfi::MobileLinux,
                managed_root: "/tmp/mobile-linux".to_string(),
                workspace_host_path: Some("/tmp/workspaces/default".to_string()),
                stable_workspace_id: Some("default".to_string()),
                abi: "arm64-v8a".to_string(),
                rootfs_version: "v1".to_string(),
                archive_sha256: None,
                authorization_file: None,
            },
        ));
        assert!(!capability.available);
        assert!(matches!(
            capability.mode,
            super::MobileLinuxRuntimeModeFfi::MobileLinux
        ));
        let reason = capability
            .reason
            .expect("unavailable capability carries reason");
        assert!(!reason.contains("authorization"));
        assert!(reason.contains("not linked"));
    }

    #[test]
    fn mobile_linux_boot_rejects_legacy_mode() {
        let err = super::android_mobile_linux_boot(Some(super::AndroidMobileLinuxConfigFfi {
            mode: super::MobileLinuxRuntimeModeFfi::Legacy,
            managed_root: "/tmp/mobile-linux".to_string(),
            workspace_host_path: Some("/tmp/workspaces/default".to_string()),
            stable_workspace_id: Some("default".to_string()),
            abi: "arm64-v8a".to_string(),
            rootfs_version: "v1".to_string(),
            archive_sha256: None,
            authorization_file: None,
        }))
        .expect_err("legacy mode must fail closed");
        assert!(matches!(err, super::MobileLinuxApiErrorFfi::LegacySelected));
    }

    #[test]
    fn mobile_linux_run_command_rejects_empty_command() {
        let err = super::android_mobile_linux_run_command(
            Some(super::AndroidMobileLinuxConfigFfi {
                mode: super::MobileLinuxRuntimeModeFfi::MobileLinux,
                managed_root: "/tmp/mobile-linux".to_string(),
                workspace_host_path: Some("/tmp/workspaces/default".to_string()),
                stable_workspace_id: Some("default".to_string()),
                abi: "arm64-v8a".to_string(),
                rootfs_version: "v1".to_string(),
                archive_sha256: None,
                authorization_file: None,
            }),
            super::MobileLinuxCommandRequestFfi {
                command: "   ".to_string(),
                args: vec![],
                cwd: None,
                env: vec![],
                stdin: None,
                timeout_ms: None,
                allow_network: false,
                mounts: vec![],
            },
        )
        .expect_err("empty command must be rejected");
        assert!(matches!(
            err,
            super::MobileLinuxApiErrorFfi::InvalidRequest { .. }
        ));
    }

    /// Minimal host-side [`super::AndroidGitCredentialProvider`] impl for tests:
    /// exercises the bridge onto `tool_api::GitCredentialProvider`.
    struct TestCredProvider;
    impl super::AndroidGitCredentialProvider for TestCredProvider {
        fn https_token(&self) -> Option<String> {
            Some("pat-token".to_string())
        }
        fn ssh_passphrase(&self) -> Option<String> {
            None
        }
    }
}

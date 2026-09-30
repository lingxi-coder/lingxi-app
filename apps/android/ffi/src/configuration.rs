#[cfg(feature = "uniffi")]
use super::linux_types::MobileLinuxRuntimeModeFfi;
#[cfg(feature = "uniffi")]
use harness_runtime::mobile::{MobileEngineError, SessionModeDto};

/// FFI carrier for the Android shell/sandbox configuration (spec r3 §Android
/// inputs). `None` anywhere upstream keeps shell support fully absent.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct AndroidShellConfigFfi {
    /// Whether the product exposes the guest shell tool.
    pub enable_shell: bool,
    /// Whether device secrets are persisted in Android Keystore.
    pub secrets_in_keystore: bool,
    /// Whether the user accepted shell access to workspace data.
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

/// FFI carrier for Android mobile-linux configuration.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct AndroidMobileLinuxConfigFfi {
    /// The Android runtime is always PRoot.
    pub mode: MobileLinuxRuntimeModeFfi,
    /// App-private root where rootfs state is managed.
    pub managed_root: String,
    /// Canonical app sandbox root (`Context.filesDir`) that owns app build roots.
    pub app_sandbox_root: String,
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

/// Stable Android device class supplied by the native host.
#[derive(Debug, Clone, Copy)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
pub enum AndroidDeviceClassFfi {
    /// A handset-sized Android device.
    Phone,
    /// A tablet-sized Android device.
    Tablet,
    /// Native configuration did not expose a reliable classification.
    Unknown,
}

/// Best-effort classification of the Android execution target.
#[derive(Debug, Clone, Copy)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
pub enum AndroidExecutionTargetFfi {
    /// A physical Android device.
    PhysicalDevice,
    /// An Android emulator or another well-known virtual-device image.
    Emulator,
    /// Build facts were unavailable, so no target was inferred.
    Unknown,
}

/// Whether the engine was launched for an interactive or scheduled session.
#[derive(Debug, Clone, Copy)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
pub enum AndroidLaunchModeFfi {
    /// A user-visible interactive conversation engine.
    Interactive,
    /// A transient engine launched by scheduled background work.
    ScheduledHeadless,
}

/// Stable host facts collected by Kotlin when an Android engine is launched.
///
/// Runtime capabilities are deliberately absent: Rust derives those after the
/// mobile runtime probe and registration gates have completed.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct AndroidHostEnvironmentFfi {
    /// Android release/API label, for example `16 (API 36)`.
    pub host_os_version: Option<String>,
    /// Stable phone/tablet classification.
    pub device_class: AndroidDeviceClassFfi,
    /// Best-effort physical/emulator classification.
    pub execution_target: AndroidExecutionTargetFfi,
    /// Interactive foreground vs scheduled headless launch.
    pub launch_mode: AndroidLaunchModeFfi,
}

impl From<AndroidHostEnvironmentFfi>
    for lingxi_core::host::mobile_runtime_environment::MobileHostEnvironment
{
    fn from(value: AndroidHostEnvironmentFfi) -> Self {
        use lingxi_core::host::mobile_runtime_environment::{
            MobileDeviceClass, MobileExecutionTarget, MobileHostEnvironment, MobileHostOs,
            MobileLaunchMode,
        };

        MobileHostEnvironment::new(
            MobileHostOs::Android,
            value.host_os_version,
            match value.device_class {
                AndroidDeviceClassFfi::Phone => MobileDeviceClass::Phone,
                AndroidDeviceClassFfi::Tablet => MobileDeviceClass::Tablet,
                AndroidDeviceClassFfi::Unknown => MobileDeviceClass::Unknown,
            },
            match value.execution_target {
                AndroidExecutionTargetFfi::PhysicalDevice => MobileExecutionTarget::PhysicalDevice,
                AndroidExecutionTargetFfi::Emulator => MobileExecutionTarget::Emulator,
                AndroidExecutionTargetFfi::Unknown => MobileExecutionTarget::Unknown,
            },
            match value.launch_mode {
                AndroidLaunchModeFfi::Interactive => MobileLaunchMode::Interactive,
                AndroidLaunchModeFfi::ScheduledHeadless => MobileLaunchMode::ScheduledHeadless,
            },
        )
    }
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
    pub session_mode: SessionModeDto,
    pub vision_delegation_enabled: bool,
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
    /// Stable native-host facts. Absent for legacy launch entry points.
    pub host_environment: Option<AndroidHostEnvironmentFfi>,
}

#[cfg(feature = "uniffi")]
pub(super) fn android_project_cwd(
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
    let is_managed_project = components.len() == 3
        && components[0] == "projects"
        && is_lowercase_uuid(components[1])
        && components[2] == "workspace";
    // v3 local apps: an app's conversation scope roots at
    // `filesDir/apps/<id>/workspace` — the same shape Kotlin's
    // `LocalAppWorkspace` derives, with the id legality delegated to the
    // engine's own minting rule instead of a twin regex. This MUST stay in
    // lockstep with `ios_project_cwd`: the engine mints and lists app
    // sessions platform-neutrally, so a gate that rejects them here shows the
    // user rows they could never open.
    let is_local_app_workspace = components.len() == 3
        && components[0] == "apps"
        && local_apps::ids::is_valid_app_id(components[1])
        && components[2] == "workspace";
    let is_scheduled_workspace = components == ["scheduled", "workspace"];
    let valid = (is_managed_project || is_local_app_workspace || is_scheduled_workspace)
        && workspace.is_dir();
    if !valid {
        return Err(MobileEngineError::Internal(
            "Android conversation workspace must match \
             filesDir/projects/<lowercase UUID>/workspace or \
             filesDir/apps/<app id>/workspace or \
             filesDir/scheduled/workspace"
                .to_string(),
        ));
    }
    Ok(workspace)
}

#[cfg(feature = "uniffi")]
pub(super) fn is_lowercase_uuid(value: &str) -> bool {
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

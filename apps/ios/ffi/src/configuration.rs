#[cfg(feature = "uniffi")]
use super::linux_types::MobileLinuxOperationFfiError;
#[cfg(feature = "uniffi")]
use super::linux_types::MobileLinuxRuntimeModeFfi;
#[cfg(feature = "uniffi")]
use harness_runtime::mobile::{MobileConfig, MobileEngineError, MobileSessionMode, SessionModeDto};
#[cfg(feature = "uniffi")]
use std::path::{Component, Path, PathBuf};

/// FFI carrier for iOS mobile-linux configuration.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IosMobileLinuxConfigFfi {
    /// Conversations require the mobile-linux backend.
    pub mode: MobileLinuxRuntimeModeFfi,
    /// Brand-owned guest identity copied from `MobileUserDirectories`.
    pub brand_id: String,
    pub linux_user: String,
    pub config_dir_name: String,
    /// App-private root where rootfs state is managed.
    pub managed_root: String,
    /// Explicit host workspace root exposed to the guest. Must not be the app
    /// sandbox root, the brand config directory, or any provider/config subtree.
    pub workspace_host_path: String,
    /// Stable guest workspace id used to produce `/home/<linuxUser>/<brandId>/<id>`.
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
    /// The native engine sandbox root, also used to protect native credentials.
    /// Conversation files live in the Linux account under `managed_root`.
    ///
    /// It must be supplied, never derived. This field replaced an inference
    /// that read the root out of `managed_root` by cutting at
    /// `Library/Application Support`: that yields the iOS *container*, while
    /// the engine's root is `<AppSupport>/LingxiCode` — three components
    /// deeper, and not reachable from `mobile-linux/ios-ish` by any rule. The
    /// runtime therefore guarded the wrong container config directory, leaving the real
    /// credential directory unguarded.
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
    /// Actual native app version; None disables catalog networking for embedders.
    pub app_version: Option<String>,
    /// Standalone Local App model host; disables personal session startup.
    pub app_agent_host: bool,
    /// Native brand identity, shared with `MobileUserDirectories` even when the
    /// Store provider-only Local App host does not initialize Mobile Linux.
    pub brand_id: String,
    pub linux_user: String,
    pub config_dir_name: String,
    /// Native config directory corresponding to the selected brand profile.
    pub native_config_home: String,
    pub api_base: String,
    pub api_key: String,
    pub model: String,
    pub session_mode: SessionModeDto,
    pub vision_delegation_enabled: bool,
    pub app_sandbox_root: String,
    /// Optional cwd inside the Linux user HOME, in guest or native coordinates.
    /// Global configuration and transcripts use that same persistent account.
    pub project_cwd: Option<String>,
    /// Directories registered by the selected native Project. They remain the
    /// file-tool trust catalog; only entries duplicated in
    /// `project_mount_grants` receive an iOS bind mount.
    pub project_additional_directories: Vec<String>,
    /// Exact guest directories the native Project store authorized for
    /// additional iOS bind mounts. This is a separate capability from the
    /// engine's workspace trust/catalog list and is never supplied by a Local
    /// App page.
    pub project_mount_grants: Vec<String>,
    pub provider_config: Option<IosProviderConfigFfi>,
    pub mobile_linux: Option<IosMobileLinuxConfigFfi>,
    /// Device physical memory reported by the iOS host.
    pub physical_memory_bytes: u64,
    /// Stable native host facts used to describe the device execution surface.
    /// Dynamic UI state and model/provider selection deliberately stay out.
    pub host_environment: Option<IosHostEnvironmentFfi>,
}

/// Stable iOS form factor reported once when the engine is constructed.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IosDeviceClassFfi {
    /// iPhone-sized host.
    Phone,
    /// iPad-sized host.
    Tablet,
    /// Native client could not determine the form factor.
    Unknown,
}

/// Whether this iOS host is a physical device or Simulator process.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IosExecutionTargetFfi {
    /// Physical iPhone or iPad.
    PhysicalDevice,
    /// Apple Simulator process.
    Simulator,
    /// Native client could not determine the target.
    Unknown,
}

/// Stable engine launch surface; independent from UIKit foreground state.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IosLaunchModeFfi {
    /// User-visible conversation engine.
    Interactive,
    /// Background scheduler engine without interactive UI.
    ScheduledHeadless,
}

/// Native iOS facts captured once per engine so prompt context stays cacheable.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IosHostEnvironmentFfi {
    /// iOS/iPadOS version reported by UIKit.
    pub os_version: String,
    /// Stable iPhone/iPad form factor.
    pub device_class: IosDeviceClassFfi,
    /// Physical device or Simulator.
    pub execution_target: IosExecutionTargetFfi,
    /// Interactive or scheduled-headless construction path.
    pub launch_mode: IosLaunchModeFfi,
}

#[cfg(feature = "uniffi")]
pub(super) fn mobile_linux_authorization_verified(path: Option<&String>) -> bool {
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
pub(super) fn ios_project_cwd(
    managed_root: &str,
    project_cwd: Option<&str>,
) -> Result<std::path::PathBuf, MobileEngineError> {
    let directories = harness_runtime::mobile::mobile_user_directories(managed_root.to_string())?;
    directories.prepare()?;
    directories.resolve_cwd(project_cwd).map(|(host, _)| host)
}

#[cfg(feature = "uniffi")]
pub(super) fn ios_mobile_config_from_launch_config(
    config: &IosEngineLaunchConfigFfi,
) -> Result<MobileConfig, MobileEngineError> {
    let (brand_profile, config_home, cwd, logical_cwd) = match config.mobile_linux.as_ref() {
        Some(linux) => {
            if !matches!(linux.mode, MobileLinuxRuntimeModeFfi::MobileLinux) {
                return Err(MobileEngineError::Internal(
                    "iOS conversations require Mobile Linux".into(),
                ));
            }
            let directories =
                harness_runtime::mobile::mobile_user_directories(linux.managed_root.clone())?;
            let brand_profile = mobile_brand_profile_from_directories(
                &directories,
                &config.brand_id,
                &config.linux_user,
                &config.config_dir_name,
            )?;
            if linux.brand_id != config.brand_id
                || linux.linux_user != config.linux_user
                || linux.config_dir_name != config.config_dir_name
            {
                return Err(MobileEngineError::Internal(
                    "iOS launch and Mobile Linux brand profiles do not match".into(),
                ));
            }
            let native_home = resolve_mobile_linux_security_path(
                std::path::Path::new(&config.native_config_home),
                "native_config_home",
            )
            .map_err(|error| MobileEngineError::Internal(error.to_string()))?;
            let selected_home = resolve_mobile_linux_security_path(
                std::path::Path::new(&directories.config_home),
                "brand config home",
            )
            .map_err(|error| MobileEngineError::Internal(error.to_string()))?;
            if native_home != selected_home {
                return Err(MobileEngineError::Internal(
                    "native config home does not match MobileUserDirectories".into(),
                ));
            }
            directories.prepare()?;
            directories
                .validate_projects_mount(&linux.workspace_host_path, &linux.stable_workspace_id)?;
            let (cwd, logical_cwd) = directories.resolve_cwd(config.project_cwd.as_deref())?;
            (
                brand_profile,
                std::path::PathBuf::from(&directories.config_home),
                cwd,
                Some(logical_cwd),
            )
        }
        None if config.app_agent_host => {
            let brand_profile = mobile_linux_api::BrandRuntimeProfile::new(
                config.brand_id.clone(),
                config.linux_user.clone(),
                config.config_dir_name.clone(),
            );
            brand_profile
                .validate()
                .map_err(|error| MobileEngineError::Internal(error.to_string()))?;
            let config_home = resolve_mobile_linux_security_path(
                std::path::Path::new(&config.native_config_home),
                "native_config_home",
            )
            .map_err(|error| MobileEngineError::Internal(error.to_string()))?;
            let managed_root = config_home
                .parent()
                .and_then(std::path::Path::parent)
                .and_then(std::path::Path::parent)
                .ok_or_else(|| {
                    MobileEngineError::Internal(
                        "native config home does not have a managed-root profile path".into(),
                    )
                })?;
            let expected_home = resolve_mobile_linux_security_path(
                &brand_profile
                    .persistent_home_host_path(managed_root)
                    .join(&brand_profile.config_dir_name),
                "brand config home",
            )
            .map_err(|error| MobileEngineError::Internal(error.to_string()))?;
            if config_home != expected_home {
                return Err(MobileEngineError::Internal(
                    "native config home does not match the supplied brand profile".into(),
                ));
            }
            (brand_profile, config_home.clone(), config_home, None)
        }
        None => {
            return Err(MobileEngineError::Internal(
                "iOS conversations require Mobile Linux".into(),
            ));
        }
    };
    let mut cfg = MobileConfig {
        brand_runtime_profile: brand_profile,
        app_agent_host: config.app_agent_host,
        startup_catalog: config.app_version.as_ref().map(|version| {
            harness_runtime::StartupCatalog::new(
                harness_runtime::CatalogPlatform::Ios,
                version,
                &config_home,
            )
        }),
        build_info: harness_runtime::mobile::BuildInfo::new(
            env!("CARGO_PKG_VERSION"),
            option_env!("LINGXI_GIT_SHA_SHORT").unwrap_or("unknown"),
        ),
        cwd,
        logical_cwd,
        lingxi_home: config_home,
        session_mode: match config.session_mode {
            SessionModeDto::Chat => MobileSessionMode::Chat,
            SessionModeDto::Code => MobileSessionMode::Code,
        },
        physical_memory_bytes: config.physical_memory_bytes,
        vision_delegation_enabled: config.vision_delegation_enabled,
        host_environment: Some(config.host_environment.as_ref().map_or_else(
            || {
                lingxi_core::host::MobileHostEnvironment::new(
                    lingxi_core::host::MobileHostOs::Ios,
                    None,
                    lingxi_core::host::MobileDeviceClass::Unknown,
                    lingxi_core::host::MobileExecutionTarget::Unknown,
                    lingxi_core::host::MobileLaunchMode::Unknown,
                )
            },
            |environment| {
                lingxi_core::host::MobileHostEnvironment::new(
                    lingxi_core::host::MobileHostOs::Ios,
                    Some(environment.os_version.clone()),
                    match environment.device_class {
                        IosDeviceClassFfi::Phone => lingxi_core::host::MobileDeviceClass::Phone,
                        IosDeviceClassFfi::Tablet => lingxi_core::host::MobileDeviceClass::Tablet,
                        IosDeviceClassFfi::Unknown => lingxi_core::host::MobileDeviceClass::Unknown,
                    },
                    match environment.execution_target {
                        IosExecutionTargetFfi::PhysicalDevice => {
                            lingxi_core::host::MobileExecutionTarget::PhysicalDevice
                        }
                        IosExecutionTargetFfi::Simulator => {
                            lingxi_core::host::MobileExecutionTarget::Simulator
                        }
                        IosExecutionTargetFfi::Unknown => {
                            lingxi_core::host::MobileExecutionTarget::Unknown
                        }
                    },
                    match environment.launch_mode {
                        IosLaunchModeFfi::Interactive => {
                            lingxi_core::host::MobileLaunchMode::Interactive
                        }
                        IosLaunchModeFfi::ScheduledHeadless => {
                            lingxi_core::host::MobileLaunchMode::ScheduledHeadless
                        }
                    },
                )
            },
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
    if !config.api_base.is_empty() {
        cfg.api_base = config.api_base.clone();
    }
    cfg.api_key = config.api_key.clone();
    if !config.model.is_empty() {
        cfg.default_model = config.model.clone();
    }
    if let Some(provider_config) = &config.provider_config {
        let (profiles, routing) = harness_runtime::mobile::parse_mobile_provider_config_json(
            &provider_config.provider_profiles_json,
            provider_config.routing_json.as_deref(),
        )?;
        cfg.provider_profiles = profiles;
        cfg.routing = routing;
    }
    let mobile_linux_selected = config
        .mobile_linux
        .as_ref()
        .is_some_and(|runtime| matches!(runtime.mode, MobileLinuxRuntimeModeFfi::MobileLinux));
    if mobile_linux_selected {
        // Advertise the shell carrier only when the Mobile Linux runtime is the
        // selected terminal; harness-runtime::mobile's capability gate still
        // fails closed when the runtime is unavailable.
        cfg.enable_mobile_linux_shell();
    }
    Ok(cfg)
}

#[cfg(feature = "uniffi")]
pub(super) fn mobile_brand_profile_from_directories(
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

#[cfg(feature = "uniffi")]
pub(super) fn validate_mobile_linux_workspace_config(
    app_sandbox_root: &str,
    config: &IosMobileLinuxConfigFfi,
) -> Result<(std::path::PathBuf, String), MobileLinuxOperationFfiError> {
    let workspace_host_path = std::path::PathBuf::from(&config.workspace_host_path);
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
    let brand_profile = mobile_linux_api::BrandRuntimeProfile::new(
        config.brand_id.clone(),
        config.linux_user.clone(),
        config.config_dir_name.clone(),
    );
    brand_profile
        .validate()
        .map_err(|error| MobileLinuxOperationFfiError::InvalidRequest {
            message: error.to_string(),
        })?;
    let brand_config_home = resolve_mobile_linux_security_path(
        &brand_profile
            .persistent_home_host_path(&managed_root)
            .join(&config.config_dir_name),
        "brand config root",
    )?;

    let persistent_workspace = mobile_linux_core::is_persistent_user_workspace(
        &managed_root,
        &brand_profile,
        &mobile_linux_api::MountSpec {
            host_path: workspace_host_path.clone(),
            guest_path: mobile_linux_api::guest_paths::workspace_for_profile(
                &brand_profile,
                &config.stable_workspace_id,
            ),
            read_only: false,
            purpose: mobile_linux_api::MountPurpose::Workspace,
        },
    );
    if workspace_host_path == sandbox_root
        || (workspace_host_path.starts_with(&managed_root) && !persistent_workspace)
        || managed_root.starts_with(&workspace_host_path)
        || workspace_host_path.starts_with(&brand_config_home)
        || brand_config_home.starts_with(&workspace_host_path)
    {
        return Err(MobileLinuxOperationFfiError::InvalidRequest {
            message:
                "workspace_host_path may not target app root, managed_root, or brand config home"
                    .to_string(),
        });
    }

    if mobile_linux_path_contains_protected_config_subtree(
        &workspace_host_path,
        &config.config_dir_name,
    ) {
        return Err(MobileLinuxOperationFfiError::InvalidRequest {
            message: "workspace_host_path may not target provider/config subtrees".to_string(),
        });
    }

    let stable_workspace_id = config.stable_workspace_id.clone();
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

/// Resolve native Project-authorized extra guest directories to exact host
/// mounts. The primary Projects mount is created separately by `platform-ios`;
/// this adapter adds only individual directories under the selected brand's
/// user home and keeps the app's config tree fenced except for an exact Local
/// App source below `.apps/`.
#[cfg(feature = "uniffi")]
pub(super) fn ios_project_mount_grants(
    config: &IosEngineLaunchConfigFfi,
) -> Result<Vec<mobile_linux_api::MountSpec>, MobileEngineError> {
    let Some(linux) = config.mobile_linux.as_ref() else {
        if config.project_mount_grants.is_empty() {
            return Ok(Vec::new());
        }
        return Err(MobileEngineError::Internal(
            "project mount grants require Mobile Linux".into(),
        ));
    };
    if config.project_mount_grants.is_empty() {
        return Ok(Vec::new());
    }
    if !matches!(linux.mode, MobileLinuxRuntimeModeFfi::MobileLinux) {
        return Err(MobileEngineError::Internal(
            "project mount grants require Mobile Linux mode".into(),
        ));
    }
    let directories = harness_runtime::mobile::mobile_user_directories(linux.managed_root.clone())?;
    directories.prepare()?;
    let profile = mobile_brand_profile_from_directories(
        &directories,
        &config.brand_id,
        &config.linux_user,
        &config.config_dir_name,
    )?;
    let primary_workspace_guest =
        mobile_linux_api::guest_paths::workspace_for_profile(&profile, "projects");
    let (_, project_cwd_guest) = directories.resolve_cwd(config.project_cwd.as_deref())?;
    validate_project_mount_grants(
        &directories,
        &primary_workspace_guest,
        &project_cwd_guest.to_string_lossy(),
        &config.project_additional_directories,
        &config.project_mount_grants,
    )
    .map_err(MobileEngineError::Internal)
}

#[cfg(feature = "uniffi")]
fn validate_project_mount_grants(
    directories: &harness_runtime::mobile::MobileUserDirectories,
    primary_workspace_guest: &str,
    project_cwd_guest: &str,
    project_additional_directories: &[String],
    grants: &[String],
) -> Result<Vec<mobile_linux_api::MountSpec>, String> {
    if grants.len() > 64 {
        return Err("project mount grants exceed the directory limit".into());
    }
    let guest_home = Path::new(&directories.guest_home);
    let config_guest = Path::new(&directories.config_guest_path);
    let apps_guest = Path::new(&directories.apps_guest_path);
    let primary_guest = Path::new(primary_workspace_guest);
    let home_host = Path::new(&directories.home)
        .canonicalize()
        .map_err(|error| format!("resolve native user home: {error}"))?;
    let config_host = Path::new(&directories.config_home)
        .canonicalize()
        .map_err(|error| format!("resolve native config home: {error}"))?;
    let apps_host = Path::new(&directories.apps)
        .canonicalize()
        .map_err(|error| format!("resolve Local App source root: {error}"))?;
    let primary_host = Path::new(&directories.projects)
        .canonicalize()
        .map_err(|error| format!("resolve primary Projects mount: {error}"))?;

    let mut seen_guest = Vec::<PathBuf>::new();
    let mut seen_host = Vec::<PathBuf>::new();
    let mut mounts = Vec::with_capacity(grants.len());
    let mut project_authorized = std::collections::HashSet::new();
    project_authorized.insert(project_cwd_guest.to_string());
    for directory in project_additional_directories {
        if !is_normalized_absolute_guest_path(directory)
            || !Path::new(directory).starts_with(guest_home)
        {
            return Err("native Project additional directory is outside its brand home".into());
        }
        project_authorized.insert(directory.clone());
    }
    for raw in grants {
        if raw.len() > 4096 || !is_normalized_absolute_guest_path(raw) {
            return Err("project mount grant must be a bounded normalized guest path".into());
        }
        let guest = Path::new(raw);
        if !project_authorized.contains(raw) {
            return Err("project mount grant is not registered in the native Project".into());
        }
        let relative = guest
            .strip_prefix(guest_home)
            .map_err(|_| "project mount grant is outside the selected brand's guest home")?;
        if relative.as_os_str().is_empty() {
            return Err("project mount grant may not expose the whole user home".into());
        }
        if paths_overlap(guest, primary_guest) {
            return Err("project mount grant overlaps the primary Projects mount".into());
        }

        let under_apps = guest.starts_with(apps_guest) && guest != apps_guest;
        if guest == apps_guest || apps_guest.starts_with(guest) {
            return Err(
                "project mount grant may not expose the Local App root or its parent".into(),
            );
        }
        if paths_overlap(guest, config_guest) && !under_apps {
            return Err("project mount grant overlaps the protected brand config tree".into());
        }

        let host = canonical_directory_without_symlinks(&home_host, relative)?;
        if paths_overlap(&host, &primary_host) {
            return Err("project mount grant overlaps the primary Projects mount".into());
        }
        let under_apps_host = host.starts_with(&apps_host) && host != apps_host;
        if paths_overlap(&host, &config_host) && !under_apps_host {
            return Err("project mount grant overlaps the protected native config tree".into());
        }
        if apps_host.starts_with(&host) {
            return Err(
                "project mount grant may not expose the Local App root or its parent".into(),
            );
        }
        if seen_guest
            .iter()
            .any(|existing| paths_overlap(existing, guest))
            || seen_host
                .iter()
                .any(|existing| paths_overlap(existing, &host))
        {
            return Err("project mount grants may not duplicate or overlap".into());
        }
        seen_guest.push(guest.to_path_buf());
        seen_host.push(host.clone());
        mounts.push(mobile_linux_api::MountSpec {
            host_path: host,
            guest_path: raw.clone(),
            read_only: false,
            purpose: mobile_linux_api::MountPurpose::External,
        });
    }
    Ok(mounts)
}

#[cfg(feature = "uniffi")]
fn is_normalized_absolute_guest_path(raw: &str) -> bool {
    raw.starts_with('/')
        && raw.len() > 1
        && !raw.ends_with('/')
        && !raw.contains('\0')
        && raw
            .split('/')
            .skip(1)
            .all(|component| !component.is_empty() && component != "." && component != "..")
        && Path::new(raw)
            .components()
            .all(|component| matches!(component, Component::RootDir | Component::Normal(_)))
}

#[cfg(feature = "uniffi")]
fn canonical_directory_without_symlinks(root: &Path, relative: &Path) -> Result<PathBuf, String> {
    let root = root
        .canonicalize()
        .map_err(|error| format!("resolve user-home root: {error}"))?;
    let mut path = root.clone();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err("project mount grant contains a non-normal path component".into());
        };
        path.push(component);
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|error| format!("resolve project mount directory: {error}"))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err("project mount path must be a real directory without symlinks".into());
        }
    }
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("resolve canonical project mount: {error}"))?;
    if !canonical.starts_with(&root) {
        return Err("project mount escaped the selected native user home".into());
    }
    Ok(canonical)
}

#[cfg(feature = "uniffi")]
fn paths_overlap(left: &Path, right: &Path) -> bool {
    left == right || left.starts_with(right) || right.starts_with(left)
}

#[cfg(feature = "uniffi")]
pub(super) fn resolve_mobile_linux_security_path(
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
pub(super) fn mobile_linux_path_contains_protected_config_subtree(
    path: &std::path::Path,
    config_dir_name: &str,
) -> bool {
    let components: Vec<_> = path
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => {
                Some(value.to_string_lossy().to_ascii_lowercase())
            }
            _ => None,
        })
        .collect();
    components.iter().any(|component| {
        component == config_dir_name || matches!(component.as_str(), "provider" | "providers")
    }) || components
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
pub(super) fn resolve_mobile_linux_app_sandbox_root(
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

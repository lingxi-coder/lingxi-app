#[cfg(feature = "uniffi")]
use super::linux_types::MobileLinuxOperationFfiError;
#[cfg(feature = "uniffi")]
use super::linux_types::MobileLinuxRuntimeModeFfi;
#[cfg(feature = "uniffi")]
use harness_runtime::mobile::{MobileConfig, MobileEngineError, MobileSessionMode, SessionModeDto};

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
    pub session_mode: SessionModeDto,
    pub vision_delegation_enabled: bool,
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
pub(super) fn default_workspace_host_path(app_sandbox_root: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(app_sandbox_root)
        .join("workspaces")
        .join("default")
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

#[cfg(feature = "uniffi")]
pub(super) fn ios_project_cwd(
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
        && local_app_contracts::ids::is_valid_app_id(components[1])
        && components[2] == "workspace";
    let is_scheduled_workspace = components == ["scheduled", "workspace"];
    let valid = (is_managed_project || is_local_app_workspace || is_scheduled_workspace)
        && workspace.is_dir();
    if !valid {
        return Err(MobileEngineError::Internal(
            "iOS conversation workspace must match \
             appSandboxRoot/Projects/<lowercase UUID>/workspace or \
             appSandboxRoot/apps/<app id>/workspace or \
             appSandboxRoot/scheduled/workspace"
                .to_string(),
        ));
    }
    Ok(workspace)
}

#[cfg(feature = "uniffi")]
pub(super) fn ios_mobile_config_from_launch_config(
    config: &IosEngineLaunchConfigFfi,
) -> Result<MobileConfig, MobileEngineError> {
    let cwd = ios_project_cwd(&config.app_sandbox_root, config.project_cwd.as_deref())?;
    let mut cfg = MobileConfig {
        build_info: harness_runtime::mobile::BuildInfo::new(
            env!("CARGO_PKG_VERSION"),
            option_env!("LINGXI_GIT_SHA_SHORT").unwrap_or("unknown"),
        ),
        cwd,
        lingxi_home: std::path::PathBuf::from(&config.app_sandbox_root).join(branding::DOT_DIR),
        session_mode: match config.session_mode {
            SessionModeDto::Chat => MobileSessionMode::Chat,
            SessionModeDto::Code => MobileSessionMode::Code,
        },
        local_apps_full_runtime: config.local_apps_full_runtime,
        local_apps_runtime_root: config
            .local_apps_runtime_root
            .as_ref()
            .map(std::path::PathBuf::from),
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
    let mobile_linux_selected = config.mobile_linux.as_ref().is_some_and(|runtime| {
        matches!(runtime.mode, MobileLinuxRuntimeModeFfi::MobileLinux)
            || config.local_apps_runtime_root.is_some()
    });
    if mobile_linux_selected {
        // Local-app generation always runs in the bundled Mobile Linux runtime,
        // even when the user-facing terminal stays in Legacy mode. Advertise
        // the matching shell carrier so agents can invoke the bundled npm/Vite
        // toolchain; harness-runtime::mobile's capability gate still fails closed when
        // the runtime is unavailable.
        cfg.enable_mobile_linux_shell();
    }
    Ok(cfg)
}

#[cfg(feature = "uniffi")]
pub(super) fn validate_mobile_linux_workspace_config(
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
pub(super) fn mobile_linux_path_contains_protected_config_subtree(path: &std::path::Path) -> bool {
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

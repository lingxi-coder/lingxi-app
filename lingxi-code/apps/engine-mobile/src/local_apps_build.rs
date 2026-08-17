//! The local-app BUILD CORE: template scaffolding plus the fixed offline
//! Vite build, extracted from the generation pipeline so that
//! scaffold/build no longer belongs to the LLM-generation executor.

use crate::local_apps_host::LocalAppsHostBroker;
use local_apps::{AppDataStore, AppError, AppLayout, AppManifest};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::AsyncWriteExt;
use traits::{
    LinuxCommandRequest, MobileLinuxRuntime, MountPurpose, MountSpec, NetworkPolicy, ResourceLimits,
};

const BUILD_TIMEOUT_MS: u64 = 30 * 60 * 1_000;
const LOW_MEMORY_BUILD_BUDGET_MB: u32 = 2_048;
const MID_MEMORY_BUILD_BUDGET_MB: u32 = 3_072;
const HIGH_MEMORY_BUILD_BUDGET_MB: u32 = 4_096;
const DEPENDENCY_READY_WAIT_TIMEOUT_MS: u64 = 120_000;
const MAX_BUILD_LOG_BYTES: u64 = 1 * 1024 * 1024;
const BUILD_PROVENANCE_FILE: &str = "build.json";
/// Vite's default deployment directory, relative to the isolated project root.
pub(crate) const VITE_OUTPUT_DIR: &str = "dist";
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LocalAppBuildTarget {
    ViteReactStaticV1,
}

macro_rules! embedded_template_file {
    ($path:literal) => {
        (
            $path,
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../local-apps/templates/vite-react-static-v1/",
                $path
            )) as &[u8],
        )
    };
}

pub(crate) const VITE_LOCKED_FILES: &[(&str, &[u8])] = &[
    embedded_template_file!(".gitignore"),
    embedded_template_file!("package.json"),
    embedded_template_file!("pnpm-lock.yaml"),
    embedded_template_file!("pnpm-workspace.yaml"),
    embedded_template_file!("components.json"),
    embedded_template_file!("jsconfig.json"),
    embedded_template_file!("index.html"),
    embedded_template_file!("vite.config.mjs"),
    embedded_template_file!(".lingxi/source-policy.json"),
    embedded_template_file!("lib/lingxi-bridge.js"),
    embedded_template_file!("lib/device-context.js"),
    embedded_template_file!("lib/platform-adapter.js"),
    embedded_template_file!("lib/lingxi-provider.jsx"),
    embedded_template_file!("styles/foundation.css"),
];

const SOURCE_FILES: &[(&str, &[u8])] = &[
    embedded_template_file!("app/main.jsx"),
    embedded_template_file!("app/app.jsx"),
    embedded_template_file!("app/providers.jsx"),
    embedded_template_file!("app/error-boundary.jsx"),
    embedded_template_file!("app/globals.css"),
    embedded_template_file!("app/screens/home-screen.jsx"),
    embedded_template_file!("app/screens/component-lab.jsx"),
    embedded_template_file!("components/ui/accordion.jsx"),
    embedded_template_file!("components/ui/alert-dialog.jsx"),
    embedded_template_file!("components/ui/alert.jsx"),
    embedded_template_file!("components/ui/aspect-ratio.jsx"),
    embedded_template_file!("components/ui/avatar.jsx"),
    embedded_template_file!("components/ui/badge.jsx"),
    embedded_template_file!("components/ui/breadcrumb.jsx"),
    embedded_template_file!("components/ui/button-group.jsx"),
    embedded_template_file!("components/ui/button.jsx"),
    embedded_template_file!("components/ui/card.jsx"),
    embedded_template_file!("components/ui/checkbox.jsx"),
    embedded_template_file!("components/ui/collapsible.jsx"),
    embedded_template_file!("components/ui/context-menu.jsx"),
    embedded_template_file!("components/ui/dialog.jsx"),
    embedded_template_file!("components/ui/dropdown-menu.jsx"),
    embedded_template_file!("components/ui/empty.jsx"),
    embedded_template_file!("components/ui/field.jsx"),
    embedded_template_file!("components/ui/hover-card.jsx"),
    embedded_template_file!("components/ui/input-group.jsx"),
    embedded_template_file!("components/ui/input.jsx"),
    embedded_template_file!("components/ui/item.jsx"),
    embedded_template_file!("components/ui/kbd.jsx"),
    embedded_template_file!("components/ui/label.jsx"),
    embedded_template_file!("components/ui/pagination.jsx"),
    embedded_template_file!("components/ui/popover.jsx"),
    embedded_template_file!("components/ui/progress.jsx"),
    embedded_template_file!("components/ui/radio-group.jsx"),
    embedded_template_file!("components/ui/scroll-area.jsx"),
    embedded_template_file!("components/ui/select.jsx"),
    embedded_template_file!("components/ui/separator.jsx"),
    embedded_template_file!("components/ui/sheet.jsx"),
    embedded_template_file!("components/ui/sidebar.jsx"),
    embedded_template_file!("components/ui/skeleton.jsx"),
    embedded_template_file!("components/ui/slider.jsx"),
    embedded_template_file!("components/ui/sonner.jsx"),
    embedded_template_file!("components/ui/spinner.jsx"),
    embedded_template_file!("components/ui/switch.jsx"),
    embedded_template_file!("components/ui/table.jsx"),
    embedded_template_file!("components/ui/tabs.jsx"),
    embedded_template_file!("components/ui/textarea.jsx"),
    embedded_template_file!("components/ui/toggle-group.jsx"),
    embedded_template_file!("components/ui/toggle.jsx"),
    embedded_template_file!("components/ui/tooltip.jsx"),
    embedded_template_file!("src/hooks/use-mobile.js"),
    embedded_template_file!("lib/query-client.js"),
    embedded_template_file!("lib/utils.js"),
    embedded_template_file!("src/stores/app-store.js"),
    embedded_template_file!("public/.gitkeep"),
];

pub(crate) fn detect_build_target(layout: &AppLayout) -> Result<LocalAppBuildTarget, AppError> {
    let workspace = layout.root().join(layout.workspace_rel());
    if workspace.join("next.config.mjs").is_file() {
        return Err(AppError::StorageCorrupt(
            "workspace contains legacy Next build configuration; local apps now support Vite only"
                .into(),
        ));
    }
    Ok(LocalAppBuildTarget::ViteReactStaticV1)
}

/// The subset of [`VITE_LOCKED_FILES`] the host re-pins from its compiled-in
/// bytes before every build.
///
/// The repository-verified Vite scaffold is the single source of truth for the
/// build infrastructure. Editable application code lives in `app/`, `src/`,
/// `components/`, `styles/`, and non-host-managed files under `lib/`.
fn repinned_host_managed_files(_target: LocalAppBuildTarget) -> &'static [&'static str] {
    &[
        ".gitignore",
        "package.json",
        "pnpm-lock.yaml",
        "pnpm-workspace.yaml",
        "components.json",
        "jsconfig.json",
        "index.html",
        "vite.config.mjs",
        ".lingxi/source-policy.json",
        "lib/lingxi-bridge.js",
        "lib/device-context.js",
        "lib/platform-adapter.js",
        "lib/lingxi-provider.jsx",
        "styles/foundation.css",
    ]
}

fn build_locked_files(_target: LocalAppBuildTarget) -> &'static [&'static str] {
    &[
        ".gitignore",
        "package.json",
        "pnpm-lock.yaml",
        "pnpm-workspace.yaml",
        "components.json",
        "jsconfig.json",
        "index.html",
        "vite.config.mjs",
        "lib/lingxi-bridge.js",
        "lib/device-context.js",
        "lib/platform-adapter.js",
        "lib/lingxi-provider.jsx",
        "styles/foundation.css",
    ]
}

/// Rewrite every host-managed file from its compiled-in template unless it
/// already matches byte for byte. Called at the top of each build — the
/// enforcement point behind the workspace contract the prompts describe.
///
/// `.lingxi/source-policy.json` DECLARES `host_managed_paths`; restoring here
/// makes the build the actual enforcement point instead of leaving that
/// contract in prompt text only.
pub(crate) fn restore_host_managed_files(
    workspace: &Path,
    target: LocalAppBuildTarget,
) -> Result<(), AppError> {
    let managed = repinned_host_managed_files(target);
    for (relative, bytes) in VITE_LOCKED_FILES
        .iter()
        .filter(|(relative, _)| managed.contains(relative))
    {
        let path = ensure_safe_file_parent(workspace, relative)?;
        let is_matching_regular_file = std::fs::symlink_metadata(&path)
            .ok()
            .filter(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
            .and_then(|_| std::fs::read(&path).ok())
            .is_some_and(|current| current.as_slice() == *bytes);
        if is_matching_regular_file {
            continue;
        }
        tracing::warn!(
            file = %relative,
            "host-managed workspace file diverged from its pinned template; restoring before the build"
        );
        write_file(workspace, relative, bytes, true)?;
    }
    Ok(())
}

pub(crate) fn build_memory_budget_mb(physical_memory_bytes: u64) -> u32 {
    let gib = 1024_u64.pow(3);
    if physical_memory_bytes >= 8 * gib {
        HIGH_MEMORY_BUILD_BUDGET_MB
    } else if physical_memory_bytes >= 6 * gib {
        MID_MEMORY_BUILD_BUDGET_MB
    } else {
        LOW_MEMORY_BUILD_BUDGET_MB
    }
}

pub(crate) fn node_old_space_mb(build_budget_mb: u32) -> u32 {
    build_budget_mb.saturating_mul(3) / 4
}

fn build_tool_executable(build_guest_path: &str) -> String {
    format!("{build_guest_path}/node_modules/vite/bin/vite.js")
}

fn fixed_vite_build_args(build_memory_mb: u32, executable: String, out_dir: String) -> Vec<String> {
    vec![
        format!(
            "--max-old-space-size={}",
            node_old_space_mb(build_memory_mb)
        ),
        executable,
        "build".into(),
        "--outDir".into(),
        out_dir,
        "--emptyOutDir".into(),
    ]
}

fn local_app_build_mount(app_id: &str, channel: &str, build_root: &Path) -> MountSpec {
    MountSpec {
        host_path: build_root.to_path_buf(),
        guest_path: traits::mobile_linux::guest_paths::local_app_build_project(app_id, channel),
        read_only: false,
        purpose: MountPurpose::LocalAppBuild,
    }
}

/// The scaffold/build half of the old generation executor: everything needed
/// to lay down the template workspace and run the fixed offline build, with
/// no dependency on the LLM pipeline.
pub(crate) struct LocalAppBuilder<'a> {
    pub(crate) mobile_linux: Option<Arc<dyn MobileLinuxRuntime>>,
    pub(crate) host: &'a LocalAppsHostBroker,
}

/// Materialize the repository-verified Vite scaffold directly into a fresh
/// workspace so app creation does not depend on `npm create`.
pub(crate) fn scaffold_workspace(layout: &AppLayout) -> Result<(), AppError> {
    layout.initialize()?;
    let workspace = layout.root().join(layout.workspace_rel());
    for (relative, bytes) in VITE_LOCKED_FILES {
        write_file(&workspace, relative, bytes, true)?;
    }
    for (relative, bytes) in SOURCE_FILES {
        write_file(&workspace, relative, bytes, false)?;
    }
    Ok(())
}

impl LocalAppBuilder<'_> {
    /// The build cannot start without the mobile Node runtime.
    fn assert_build_runtime_available(&self) -> Result<(), AppError> {
        if self.mobile_linux.is_none() {
            return Err(AppError::NotYetAvailable(
                "the verified mobile Node runtime is unavailable in this build".into(),
            ));
        }
        Ok(())
    }

    async fn run_fixed_build(
        &self,
        layout: &AppLayout,
        workspace: &Path,
        output_rel: &str,
    ) -> Result<(), AppError> {
        let runtime = self.mobile_linux.as_ref().ok_or_else(|| {
            AppError::NotYetAvailable(
                "the verified mobile Node runtime is unavailable in this build".into(),
            )
        })?;
        let build_channel = "store";
        let build_mount = local_app_build_mount(layout.app_id(), build_channel, workspace);
        let project_guest_path = build_mount.guest_path.clone();
        let tool_name = "Vite";
        let mut environment = BTreeMap::new();
        let build_state_root = workspace.join(".lingxi-build-state");
        let home_root = build_state_root.join("home");
        let temp_root = build_state_root.join("tmp");
        let xdg_cache_root = build_state_root.join("xdg-cache");
        let xdg_config_root = build_state_root.join("xdg-config");
        let xdg_data_root = build_state_root.join("xdg-data");
        // Vite's state is disposable. Reset each build-private directory so
        // repeated builds cannot accumulate cache/temp files beside the app.
        for directory in [
            &home_root,
            &temp_root,
            &xdg_cache_root,
            &xdg_config_root,
            &xdg_data_root,
        ] {
            match std::fs::symlink_metadata(directory) {
                Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                    std::fs::remove_dir_all(directory).map_err(|error| {
                        AppError::Io(format!("reset build state directory: {error}"))
                    })?;
                }
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    std::fs::remove_file(directory).map_err(|error| {
                        AppError::Io(format!("remove build state symlink: {error}"))
                    })?;
                }
                Ok(_) => {
                    std::fs::remove_file(directory).map_err(|error| {
                        AppError::Io(format!("remove build state entry: {error}"))
                    })?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(AppError::Io(format!(
                        "inspect build state directory: {error}"
                    )))
                }
            }
            std::fs::create_dir_all(directory)
                .map_err(|error| AppError::Io(format!("create build state directory: {error}")))?;
        }
        let executable = build_tool_executable(&project_guest_path);
        let workspace_vite = workspace.join("node_modules/vite/bin/vite.js");
        match std::fs::symlink_metadata(&workspace_vite) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {}
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(AppError::InvalidRequest(
                    "workspace Vite executable must not be a symlink".into(),
                ));
            }
            Ok(_) => {
                return Err(AppError::InvalidRequest(
                    "workspace Vite executable must be a regular file".into(),
                ));
            }
            Err(error) => {
                return Err(AppError::Io(format!(
                    "inspect workspace Vite executable: {error}"
                )));
            }
        }
        let guest_build_state_root = format!("{project_guest_path}/.lingxi-build-state");
        let guest_home_root = format!("{guest_build_state_root}/home");
        let guest_temp_root = format!("{guest_build_state_root}/tmp");
        let guest_xdg_cache_root = format!("{guest_build_state_root}/xdg-cache");
        let guest_xdg_config_root = format!("{guest_build_state_root}/xdg-config");
        let guest_xdg_data_root = format!("{guest_build_state_root}/xdg-data");
        environment.insert("NODE_ENV".into(), "production".into());
        environment.insert("HOME".into(), guest_home_root);
        environment.insert("TMPDIR".into(), guest_temp_root.clone());
        environment.insert("TMP".into(), guest_temp_root.clone());
        environment.insert("TEMP".into(), guest_temp_root);
        environment.insert("XDG_CACHE_HOME".into(), guest_xdg_cache_root);
        environment.insert("XDG_CONFIG_HOME".into(), guest_xdg_config_root);
        environment.insert("XDG_DATA_HOME".into(), guest_xdg_data_root);
        let build_memory_mb = build_memory_budget_mb(self.host.physical_memory_bytes());
        let resource_limits = ResourceLimits {
            max_memory_mb: Some(build_memory_mb),
            ..ResourceLimits::default()
        };
        let mounts = vec![build_mount];
        let request = LinuxCommandRequest {
            command: "/usr/bin/node".into(),
            args: fixed_vite_build_args(build_memory_mb, executable, output_rel.to_string()),
            cwd: Some(project_guest_path),
            env: environment,
            stdin: None,
            timeout_ms: Some(BUILD_TIMEOUT_MS),
            network: NetworkPolicy::Disabled,
            resource_limits,
            mounts,
        };
        let result = runtime
            .run_isolated(request)
            .await
            .map_err(|error| AppError::Io(format!("fixed {tool_name} build failed: {error}")))?;
        result
            .enforcement
            .ensure_for(NetworkPolicy::Disabled, resource_limits)
            .map_err(|error| AppError::Io(error.to_string()))?;
        append_build_log(layout, false, &result.stdout, &result.stderr).await?;
        if result.timed_out || result.cancelled || result.exit_code != 0 {
            return Err(AppError::Io(format!(
                "fixed {tool_name} build exited {} (timed_out={}, cancelled={}): {}",
                result.exit_code,
                result.timed_out,
                result.cancelled,
                bounded_log(&result.stderr)
            )));
        }
        Ok(())
    }

    pub(crate) async fn run_vite_build(
        &self,
        layout: &AppLayout,
        workspace: &Path,
        output_rel: &str,
    ) -> Result<(), AppError> {
        self.run_fixed_build(layout, workspace, output_rel).await
    }

    /// Build the app from the workspace's own dependency tree and promote only
    /// the validated output into the public build root. The workspace is the
    /// only app-owned build mount; dependencies must not be supplied by a
    /// nested or shared runtime mount.
    pub(crate) async fn build_workspace(&self, layout: &AppLayout) -> Result<(), AppError> {
        // Dependency installation is host-owned and runs through the same
        // isolated mobile runtime/mount as Vite. Wait before taking the build
        // locks so an installer can never wait on a lock held by this build.
        self.assert_build_runtime_available()?;
        let dependency = match self
            .host
            .ensure_dependency_install(layout.app_id(), true)
            .await
        {
            Ok(dependency) if dependency.state == local_apps::AppDependencyState::Ready => {
                dependency
            }
            Ok(dependency) => {
                return Err(AppError::NotYetAvailable(
                    dependency.last_error.unwrap_or_else(|| {
                        format!("workspace dependencies are {}", dependency.state)
                    }),
                ));
            }
            // Unit-level builders can intentionally omit the service and
            // provide a prepared Vite marker. Production profiles always
            // attach the service, so this compatibility path does not bypass
            // dependency state in a running app.
            Err(error) if error.contains("service is still starting") => {
                await_workspace_dependencies(
                    &layout.root().join(layout.workspace_rel()),
                    std::time::Duration::from_millis(DEPENDENCY_READY_WAIT_TIMEOUT_MS),
                )
                .await?;
            }
            Err(error) => return Err(AppError::NotYetAvailable(error)),
        };
        let build_lock = self.host.build_lock();
        let _build_guard = build_lock.lock().await;
        // The process-global guard serializes Node builds across every broker
        // in this process. The per-app storage lock extends exclusion for this
        // app across engine processes and the native delete path, which
        // otherwise could rename the app while promotion is between its two
        // directory renames.
        let _process_build_guard =
            local_apps::storage::lock_app_build(layout.root(), layout.app_id())?;
        let workspace = layout.root().join(layout.workspace_rel());
        let target = detect_build_target(layout)?;
        // Re-pin the host-managed files from the compiled-in templates on
        // EVERY build before Vite touches the workspace.
        restore_host_managed_files(&workspace, target)?;
        await_workspace_dependencies(
            &workspace,
            std::time::Duration::from_millis(DEPENDENCY_READY_WAIT_TIMEOUT_MS),
        )
        .await?;
        let build_root = layout.root().join(layout.build_rel(false));
        recover_build_promotion(&build_root)?;
        let build_key = workspace_build_key(&workspace, &dependency)?;
        if build_cache_hit(&workspace, &build_root, &build_key)? {
            return Ok(());
        }
        let artifact_root = workspace_build_artifact_root(&workspace);
        let artifact_output_rel = workspace_build_output_rel();
        let build_result = async {
            let prepare_artifact_root = artifact_root.clone();
            tokio::task::spawn_blocking(move || remove_path_if_exists(&prepare_artifact_root))
                .await
                .map_err(|error| {
                    AppError::Io(format!("build preparation worker failed: {error}"))
                })??;
            self.run_vite_build(layout, &workspace, &artifact_output_rel)
                .await?;
            let validate_artifact_root = artifact_root.clone();
            let validate_build_root = build_root.clone();
            let workspace_for_publish = workspace.clone();
            let build_key_for_publish = build_key.clone();
            tokio::task::spawn_blocking(move || {
                validate_build_output(&validate_artifact_root)?;
                prune_staging_root_for_publish(&validate_artifact_root)?;
                promote_build_root(&validate_artifact_root, &validate_build_root)?;
                write_build_provenance(&workspace_for_publish, &build_key_for_publish)
            })
            .await
            .map_err(|error| AppError::Io(format!("build promotion worker failed: {error}")))?
        }
        .await;
        if build_result.is_err() {
            let _ = std::fs::remove_dir_all(&artifact_root);
        }
        build_result
    }
}

fn workspace_build_artifact_root(workspace: &Path) -> PathBuf {
    workspace.join(".lingxi-build-state").join("build-output")
}

fn workspace_build_output_rel() -> String {
    format!(".lingxi-build-state/build-output/{VITE_OUTPUT_DIR}")
}

fn workspace_build_key(
    workspace: &Path,
    dependency: &local_apps::AppDependencyRecord,
) -> Result<String, AppError> {
    let mut files = Vec::new();
    collect_workspace_files(workspace, &mut files)?;
    files.sort();
    let mut hasher = Sha256::new();
    for path in files {
        let relative = path
            .strip_prefix(workspace)
            .map_err(|error| AppError::Io(format!("derive build key path: {error}")))?;
        hasher.update(relative.to_string_lossy().as_bytes());
        hasher.update([0]);
        hasher.update(std::fs::read(&path).map_err(|error| {
            AppError::Io(format!("read build key input {}: {error}", path.display()))
        })?);
        hasher.update([0]);
    }
    hasher.update(b"template=vite-react-static-v1\0");
    hasher.update(
        dependency
            .lockfile_sha256
            .as_deref()
            .unwrap_or("legacy-lockfile"),
    );
    hasher.update([0]);
    hasher.update(
        dependency
            .toolchain_key
            .as_deref()
            .unwrap_or("legacy-toolchain"),
    );
    Ok(format!("{:x}", hasher.finalize()))
}

fn collect_workspace_files(current: &Path, files: &mut Vec<PathBuf>) -> Result<(), AppError> {
    for entry in std::fs::read_dir(current).map_err(|error| {
        AppError::Io(format!(
            "read build key directory {}: {error}",
            current.display()
        ))
    })? {
        let entry =
            entry.map_err(|error| AppError::Io(format!("read build key entry: {error}")))?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if matches!(
            name.as_ref(),
            ".git" | ".lingxi-build-state" | "node_modules" | "dist"
        ) {
            continue;
        }
        let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
            AppError::Io(format!(
                "inspect build key input {}: {error}",
                path.display()
            ))
        })?;
        if metadata.file_type().is_symlink() {
            return Err(AppError::InvalidRequest(format!(
                "build key input must not be a symlink: {}",
                path.display()
            )));
        }
        if metadata.is_dir() {
            collect_workspace_files(&path, files)?;
        } else if metadata.is_file() {
            files.push(path);
        }
    }
    Ok(())
}

fn build_provenance_path(workspace: &Path) -> PathBuf {
    workspace
        .join(".lingxi-build-state")
        .join(BUILD_PROVENANCE_FILE)
}

fn build_cache_hit(workspace: &Path, build_root: &Path, build_key: &str) -> Result<bool, AppError> {
    let index = build_root.join(VITE_OUTPUT_DIR).join("index.html");
    let index_metadata = match std::fs::symlink_metadata(&index) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(AppError::Io(format!("inspect cached build: {error}"))),
    };
    if !index_metadata.is_file() || index_metadata.file_type().is_symlink() {
        return Ok(false);
    }
    let provenance = match std::fs::read_to_string(build_provenance_path(workspace)) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(AppError::Io(format!("read build provenance: {error}"))),
    };
    let value: Value = serde_json::from_str(&provenance)
        .map_err(|error| AppError::Io(format!("parse build provenance: {error}")))?;
    Ok(value.get("buildKey").and_then(Value::as_str) == Some(build_key))
}

fn write_build_provenance(workspace: &Path, build_key: &str) -> Result<(), AppError> {
    let path = build_provenance_path(workspace);
    let parent = path
        .parent()
        .ok_or_else(|| AppError::Io("build provenance has no parent".into()))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| AppError::Io(format!("create build provenance directory: {error}")))?;
    let temp = parent.join(format!(".{BUILD_PROVENANCE_FILE}.tmp-{}", now_stamp()));
    let body = serde_json::to_vec_pretty(&json!({"buildKey": build_key}))
        .map_err(|error| AppError::Io(format!("serialize build provenance: {error}")))?;
    std::fs::write(&temp, body)
        .map_err(|error| AppError::Io(format!("write build provenance: {error}")))?;
    std::fs::rename(&temp, &path).map_err(|error| {
        let _ = std::fs::remove_file(&temp);
        AppError::Io(format!("publish build provenance: {error}"))
    })
}

fn now_stamp() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default()
}

async fn await_workspace_dependencies(
    workspace: &Path,
    timeout_duration: tokio::time::Duration,
) -> Result<(), AppError> {
    let vite = workspace.join("node_modules/vite/bin/vite.js");
    let start = tokio::time::Instant::now();
    loop {
        match std::fs::symlink_metadata(&vite) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                return Ok(());
            }
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(AppError::InvalidRequest(
                    "workspace Vite executable must not be a symlink".into(),
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(AppError::Io(format!(
                    "inspect workspace Vite executable: {error}"
                )));
            }
        }
        if start.elapsed() >= timeout_duration {
            return Err(AppError::NotYetAvailable(format!(
                "app dependencies are not ready yet; waited {} ms for workspace/node_modules/vite/bin/vite.js",
                timeout_duration.as_millis()
            )));
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}

fn remove_path_if_exists(path: &Path) -> Result<(), AppError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            std::fs::remove_dir_all(path)
                .map_err(|error| AppError::Io(format!("remove stale build output: {error}")))
        }
        Ok(_) => std::fs::remove_file(path)
            .map_err(|error| AppError::Io(format!("remove stale build output file: {error}"))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(AppError::Io(format!(
            "inspect stale build output path: {error}"
        ))),
    }
}

fn validate_build_output(staging_root: &Path) -> Result<(), AppError> {
    let output_root = staging_root.join(VITE_OUTPUT_DIR);
    let index = output_root.join("index.html");
    if !index.is_file() {
        return Err(AppError::Io(format!(
            "fixed Vite build produced no {VITE_OUTPUT_DIR}/index.html in {}",
            staging_root.display()
        )));
    }
    validate_no_symlinks(&output_root)?;
    Ok(())
}

fn validate_no_symlinks(root: &Path) -> Result<(), AppError> {
    for entry in std::fs::read_dir(root)
        .map_err(|error| AppError::Io(format!("read build output: {error}")))?
    {
        let entry =
            entry.map_err(|error| AppError::Io(format!("read build output entry: {error}")))?;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|error| AppError::Io(format!("inspect build output: {error}")))?;
        if metadata.file_type().is_symlink() {
            return Err(AppError::InvalidRequest(format!(
                "build output must not contain symlinks: {}",
                path.display()
            )));
        }
        if metadata.is_dir() {
            validate_no_symlinks(&path)?;
        }
    }
    Ok(())
}

/// Recover the only non-atomic window in the directory promotion protocol.
///
/// `promote_build_root` first renames the last-good output to a sibling
/// backup, then renames the validated staging tree into the public `build/`
/// path. If the process dies between those renames, the next build must put
/// the last-good output back before creating another staging tree. Stale
/// staging/backup siblings are private names generated by this module, so
/// sweeping only those prefixes cannot touch app-owned source files.
fn recover_build_promotion(build_root: &Path) -> Result<(), AppError> {
    let Some(parent) = build_root.parent() else {
        return Ok(());
    };
    let Some(file_name) = build_root.file_name().and_then(|name| name.to_str()) else {
        return Err(AppError::Io(format!(
            "build root {} is not valid UTF-8",
            build_root.display()
        )));
    };
    let backup_prefix = format!(".{file_name}.previous-");
    let staging_prefix = format!(".{file_name}.staging-");
    let entries = match std::fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(AppError::Io(format!(
                "scan interrupted build promotion: {error}"
            )))
        }
    };
    let mut backups = Vec::new();
    let mut staging = Vec::new();
    for entry in entries {
        let entry =
            entry.map_err(|error| AppError::Io(format!("scan build promotion: {error}")))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.starts_with(&backup_prefix) {
            backups.push(entry.path());
        } else if name.starts_with(&staging_prefix) {
            staging.push(entry.path());
        }
    }
    backups.sort();
    let has_build = build_root.exists();
    if !has_build {
        if let Some(last_good) = backups.pop() {
            std::fs::rename(&last_good, build_root).map_err(|error| {
                AppError::Io(format!(
                    "restore interrupted build output {}: {error}",
                    last_good.display()
                ))
            })?;
        }
    }
    for path in backups.into_iter().chain(staging) {
        remove_promotion_artifact(&path)?;
    }
    Ok(())
}

fn remove_promotion_artifact(path: &Path) -> Result<(), AppError> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| AppError::Io(format!("inspect build promotion artifact: {error}")))?;
    let result = if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
    result.map_err(|error| {
        AppError::Io(format!(
            "remove build promotion artifact {}: {error}",
            path.display()
        ))
    })
}

fn promote_build_root(staging_root: &Path, build_root: &Path) -> Result<(), AppError> {
    let parent = build_root.parent().ok_or_else(|| {
        AppError::Io(format!("build root {} has no parent", build_root.display()))
    })?;
    let file_name = build_root
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            AppError::Io(format!(
                "build root {} is not valid UTF-8",
                build_root.display()
            ))
        })?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let backup = parent.join(format!(".{file_name}.previous-{stamp}"));
    let had_current = build_root.exists();
    if had_current {
        std::fs::rename(build_root, &backup)
            .map_err(|error| AppError::Io(format!("stage previous build output: {error}")))?;
    }
    if let Err(error) = std::fs::rename(staging_root, build_root) {
        if had_current {
            let _ = std::fs::rename(&backup, build_root);
        }
        return Err(AppError::Io(format!("promote build output: {error}")));
    }
    if had_current {
        if let Err(error) = std::fs::remove_dir_all(&backup) {
            tracing::warn!(error = %error, path = %backup.display(), "could not remove previous build backup");
        }
    }
    Ok(())
}

pub(crate) async fn migrate_manifest_with_approval(
    host: &LocalAppsHostBroker,
    layout: &AppLayout,
    manifest: &AppManifest,
) -> Result<(), AppError> {
    let preview = preview_manifest_migration(layout.clone(), manifest.clone()).await?;
    let allow_destructive = if preview.destructive {
        host.approve_destructive_manifest_migration(manifest.app_id.as_str(), &preview)
            .await
            .map_err(AppError::InvalidRequest)?;
        true
    } else {
        false
    };
    apply_manifest_migration(
        layout.clone(),
        manifest.clone(),
        allow_destructive,
        allow_destructive.then_some(preview),
    )
    .await
}

async fn preview_manifest_migration(
    layout: AppLayout,
    manifest: AppManifest,
) -> Result<local_apps::DataMigrationPreview, AppError> {
    tokio::task::spawn_blocking(move || {
        let store = AppDataStore::open(layout)?;
        store.preview_migration(&manifest)
    })
    .await
    .map_err(|error| AppError::Io(format!("manifest migration worker failed: {error}")))?
}

async fn apply_manifest_migration(
    layout: AppLayout,
    manifest: AppManifest,
    allow_destructive: bool,
    approved_preview: Option<local_apps::DataMigrationPreview>,
) -> Result<(), AppError> {
    tokio::task::spawn_blocking(move || {
        let mut store = AppDataStore::open(layout)?;
        if let Some(approved_preview) = approved_preview {
            let current_preview = store.preview_migration(&manifest)?;
            if current_preview != approved_preview {
                return Err(AppError::WorkflowStateInvalid(
                    "destructive data migration changed while waiting for approval; retry generation"
                        .into(),
                ));
            }
        }
        store.migrate_manifest(&manifest, allow_destructive, now_ms())?;
        Ok::<_, AppError>(())
    })
    .await
    .map_err(|error| AppError::Io(format!("manifest migration worker failed: {error}")))?
}

async fn append_build_log(
    layout: &AppLayout,
    full: bool,
    stdout: &str,
    stderr: &str,
) -> Result<(), AppError> {
    let log_dir = layout.root().join(layout.logs_rel());
    tokio::fs::create_dir_all(&log_dir)
        .await
        .map_err(|error| AppError::Io(format!("create build log directory: {error}")))?;
    let log_path = log_dir.join("build.log");
    let channel = if full { "full" } else { "store" };
    let body = format!(
        "\n=== {channel} build ===\nstdout:\n{}\nstderr:\n{}\n",
        bounded_log(stdout),
        bounded_log(stderr)
    );
    // Keep diagnostics useful without allowing repeated builds to grow the
    // app directory forever. The file is host-owned and can be safely
    // truncated between builds; the current invocation is always retained.
    if let Ok(metadata) = tokio::fs::metadata(&log_path).await {
        if metadata.len().saturating_add(body.len() as u64) > MAX_BUILD_LOG_BYTES {
            tokio::fs::write(&log_path, b"build log rotated\n")
                .await
                .map_err(|error| AppError::Io(format!("rotate build log: {error}")))?;
        }
    }
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .await
        .map_err(|error| AppError::Io(format!("open build log: {error}")))?;
    file.write_all(body.as_bytes())
        .await
        .map_err(|error| AppError::Io(format!("write build log: {error}")))
}

pub(crate) fn write_file(
    root: &Path,
    relative: &str,
    bytes: &[u8],
    overwrite: bool,
) -> Result<(), AppError> {
    let path = ensure_safe_file_parent(root, relative)?;
    if let Ok(metadata) = std::fs::symlink_metadata(&path) {
        if !overwrite && metadata.is_file() && !metadata.file_type().is_symlink() {
            return Ok(());
        }
        if metadata.file_type().is_symlink() {
            std::fs::remove_file(&path).map_err(|error| {
                AppError::Io(format!(
                    "remove symlinked template file {relative}: {error}"
                ))
            })?;
        } else if metadata.is_dir() {
            std::fs::remove_dir_all(&path).map_err(|error| {
                AppError::Io(format!(
                    "remove directory template path {relative}: {error}"
                ))
            })?;
        } else if !metadata.is_file() {
            std::fs::remove_file(&path).map_err(|error| {
                AppError::Io(format!("remove non-file template path {relative}: {error}"))
            })?;
        }
    }
    let parent = path
        .parent()
        .ok_or_else(|| AppError::Io(format!("template path {relative} has no parent")))?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| AppError::Io(format!("template path {relative} is not valid UTF-8")))?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let temp_path = parent.join(format!(".{file_name}.tmp-{stamp}"));
    std::fs::write(&temp_path, bytes)
        .map_err(|error| AppError::Io(format!("write temp template file {relative}: {error}")))?;
    std::fs::rename(&temp_path, &path).map_err(|error| {
        let _ = std::fs::remove_file(&temp_path);
        AppError::Io(format!("replace template file {relative}: {error}"))
    })
}

fn ensure_safe_file_parent(root: &Path, relative: &str) -> Result<PathBuf, AppError> {
    let relative_path = Path::new(relative);
    if relative_path.is_absolute()
        || relative_path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(AppError::InvalidRequest(format!(
            "template path must be a normalized relative path: {relative}"
        )));
    }

    let root_metadata = std::fs::symlink_metadata(root)
        .map_err(|error| AppError::Io(format!("inspect template root: {error}")))?;
    if !root_metadata.is_dir() || root_metadata.file_type().is_symlink() {
        return Err(AppError::InvalidRequest(format!(
            "template root must be a real directory: {}",
            root.display()
        )));
    }

    let mut current = root.to_path_buf();
    if let Some(parent) = relative_path.parent() {
        for component in parent.components() {
            let Component::Normal(name) = component else {
                unreachable!("relative path was validated above")
            };
            current.push(name);
            match std::fs::symlink_metadata(&current) {
                Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
                Ok(_) => {
                    std::fs::remove_file(&current).map_err(|error| {
                        AppError::Io(format!(
                            "remove unsafe template parent {}: {error}",
                            current.display()
                        ))
                    })?;
                    std::fs::create_dir(&current).map_err(|error| {
                        AppError::Io(format!(
                            "create template parent {}: {error}",
                            current.display()
                        ))
                    })?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    std::fs::create_dir(&current).map_err(|error| {
                        AppError::Io(format!(
                            "create template parent {}: {error}",
                            current.display()
                        ))
                    })?;
                }
                Err(error) => {
                    return Err(AppError::Io(format!(
                        "inspect template parent {}: {error}",
                        current.display()
                    )));
                }
            }
        }
    }
    Ok(root.join(relative_path))
}

fn write_host_managed_build_files(
    build_root: &Path,
    target: LocalAppBuildTarget,
) -> Result<(), AppError> {
    let managed = build_locked_files(target);
    for (relative, bytes) in VITE_LOCKED_FILES
        .iter()
        .filter(|(relative, _)| managed.contains(relative))
    {
        write_file(build_root, relative, bytes, true)?;
    }
    Ok(())
}

fn replace_build_source(
    workspace: &Path,
    build_root: &Path,
    target: LocalAppBuildTarget,
) -> Result<(), AppError> {
    if build_root.exists() {
        std::fs::remove_dir_all(build_root)
            .map_err(|error| AppError::Io(format!("clear build directory: {error}")))?;
    }
    std::fs::create_dir_all(build_root)
        .map_err(|error| AppError::Io(format!("create build directory: {error}")))?;
    write_host_managed_build_files(build_root, target)?;
    copy_workspace_tree(workspace, workspace, build_root, target)
}

fn copy_workspace_tree(
    workspace: &Path,
    current: &Path,
    destination: &Path,
    target: LocalAppBuildTarget,
) -> Result<(), AppError> {
    for entry in std::fs::read_dir(current)
        .map_err(|error| AppError::Io(format!("read generation source: {error}")))?
    {
        let entry = entry.map_err(|error| AppError::Io(format!("read source entry: {error}")))?;
        let path = entry.path();
        let relative = path
            .strip_prefix(workspace)
            .map_err(|_| AppError::InvalidRequest("build source escaped workspace".into()))?;
        let destination_path = destination.join(relative);
        copy_workspace_entry(workspace, &path, &destination_path, relative, target)?;
    }
    Ok(())
}

fn copy_workspace_contents(
    workspace: &Path,
    current: &Path,
    destination: &Path,
    target: LocalAppBuildTarget,
) -> Result<(), AppError> {
    for entry in std::fs::read_dir(current)
        .map_err(|error| AppError::Io(format!("read generation source: {error}")))?
    {
        let entry = entry.map_err(|error| AppError::Io(format!("read source entry: {error}")))?;
        let path = entry.path();
        let relative = path
            .strip_prefix(workspace)
            .map_err(|_| AppError::InvalidRequest("build source escaped workspace".into()))?;
        let destination_path = destination.join(entry.file_name());
        copy_workspace_entry(workspace, &path, &destination_path, relative, target)?;
    }
    Ok(())
}

fn copy_workspace_entry(
    workspace: &Path,
    source: &Path,
    destination: &Path,
    relative: &Path,
    target: LocalAppBuildTarget,
) -> Result<(), AppError> {
    if should_skip_workspace_path(relative, target) {
        return Ok(());
    }
    let kind = std::fs::symlink_metadata(source)
        .map_err(|error| AppError::Io(format!("inspect source entry: {error}")))?;
    if kind.file_type().is_symlink() {
        return Err(AppError::InvalidRequest(format!(
            "source symlink is forbidden: {}",
            relative.display()
        )));
    } else if kind.is_dir() {
        std::fs::create_dir_all(destination)
            .map_err(|error| AppError::Io(format!("create build source directory: {error}")))?;
        copy_workspace_contents(workspace, source, destination, target)?;
    } else if kind.is_file() {
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| AppError::Io(format!("create build source parent: {error}")))?;
        }
        std::fs::copy(source, destination)
            .map_err(|error| AppError::Io(format!("copy build source: {error}")))?;
    }
    Ok(())
}

fn should_skip_workspace_path(relative: &Path, target: LocalAppBuildTarget) -> bool {
    if repinned_host_managed_files(target).contains(&relative.to_string_lossy().as_ref()) {
        return true;
    }
    matches!(
        relative
            .components()
            .next()
            .and_then(|component| component.as_os_str().to_str()),
        Some(".git" | ".lingxi" | ".lingxi-build-state" | "dist" | "node_modules")
    ) || relative == Path::new("LINGXI.md")
}

fn prune_staging_root_for_publish(staging_root: &Path) -> Result<(), AppError> {
    for entry in std::fs::read_dir(staging_root)
        .map_err(|error| AppError::Io(format!("read staged build root: {error}")))?
    {
        let entry =
            entry.map_err(|error| AppError::Io(format!("read staged build entry: {error}")))?;
        if entry.file_name() == VITE_OUTPUT_DIR {
            continue;
        }
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|error| AppError::Io(format!("inspect staged publish entry: {error}")))?;
        if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
            std::fs::remove_dir_all(&path)
                .map_err(|error| AppError::Io(format!("prune staged build directory: {error}")))?;
        } else {
            std::fs::remove_file(&path)
                .map_err(|error| AppError::Io(format!("prune staged build file: {error}")))?;
        }
    }
    Ok(())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

fn bounded_log(value: &str) -> String {
    value.chars().take(4_000).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Mutex;
    use traits::mobile_linux::{guest_paths, map_guest_path_to_host};
    use traits::{
        LinuxCommandResult, MobileLinuxCapability, MobileLinuxError, MobileLinuxRuntimeMode,
        MobileLinuxTaskSnapshot, PtyOpenRequest, PtySessionHandle, PtySize, RootfsState,
        RootfsStatus, SandboxBackend,
    };

    struct RecordingIsolatedRuntime {
        requests: Mutex<Vec<LinuxCommandRequest>>,
        isolated_runs: AtomicUsize,
    }

    impl RecordingIsolatedRuntime {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                requests: Mutex::new(Vec::new()),
                isolated_runs: AtomicUsize::new(0),
            })
        }

        async fn recorded_request(&self) -> LinuxCommandRequest {
            self.requests
                .lock()
                .await
                .last()
                .cloned()
                .expect("recorded request")
        }
    }

    #[async_trait]
    impl MobileLinuxRuntime for RecordingIsolatedRuntime {
        fn backend(&self) -> SandboxBackend {
            SandboxBackend::IosIsh
        }

        fn mode(&self) -> MobileLinuxRuntimeMode {
            MobileLinuxRuntimeMode::MobileLinux
        }

        async fn probe_capability(&self) -> MobileLinuxCapability {
            MobileLinuxCapability {
                available: true,
                backend: self.backend(),
                mode: self.mode(),
                reason: None,
                streaming_output: false,
                background_processes: false,
                pty: false,
                bind_mounts: true,
                rootfs_integrity: false,
            }
        }

        async fn boot(&self) -> Result<RootfsStatus, MobileLinuxError> {
            self.rootfs_status().await
        }

        async fn shutdown(&self) -> Result<(), MobileLinuxError> {
            Ok(())
        }

        async fn run(
            &self,
            _request: LinuxCommandRequest,
        ) -> Result<LinuxCommandResult, MobileLinuxError> {
            Err(MobileLinuxError::Unsupported)
        }

        async fn run_isolated(
            &self,
            request: LinuxCommandRequest,
        ) -> Result<LinuxCommandResult, MobileLinuxError> {
            self.isolated_runs.fetch_add(1, Ordering::SeqCst);
            self.requests.lock().await.push(request.clone());
            let mount = request.mounts.first().ok_or_else(|| {
                MobileLinuxError::InvalidRequest("missing LocalAppBuild mount".into())
            })?;
            let out_dir = request
                .args
                .windows(2)
                .find_map(|window| (window[0] == "--outDir").then(|| window[1].clone()))
                .ok_or_else(|| MobileLinuxError::InvalidRequest("missing --outDir".into()))?;
            let guest_output = format!(
                "{}/{}",
                request
                    .cwd
                    .clone()
                    .unwrap_or_else(|| mount.guest_path.clone()),
                out_dir
            );
            let host_output =
                map_guest_path_to_host(&guest_output, &request.mounts).ok_or_else(|| {
                    MobileLinuxError::InvalidRequest(
                        "outDir is outside the mounted workspace".into(),
                    )
                })?;
            fs::create_dir_all(host_output.join("assets")).map_err(|error| {
                MobileLinuxError::Io(format!("create fake build output: {error}"))
            })?;
            fs::write(host_output.join("index.html"), "<html>fresh</html>").map_err(|error| {
                MobileLinuxError::Io(format!("write fake build output: {error}"))
            })?;
            fs::write(host_output.join("assets/app.js"), "console.log('ok');")
                .map_err(|error| MobileLinuxError::Io(format!("write fake asset: {error}")))?;
            Ok(LinuxCommandResult {
                stdout: "ok".into(),
                stderr: String::new(),
                exit_code: 0,
                timed_out: false,
                cancelled: false,
                enforcement: traits::LinuxEnforcementReceipt {
                    network_policy_enforced: true,
                    memory_limit_enforced: true,
                },
            })
        }

        async fn spawn_background(
            &self,
            _request: LinuxCommandRequest,
        ) -> Result<traits::LinuxProcessHandle, MobileLinuxError> {
            Err(MobileLinuxError::Unsupported)
        }

        async fn kill(&self, _handle: &traits::LinuxProcessHandle) -> Result<(), MobileLinuxError> {
            Err(MobileLinuxError::Unsupported)
        }

        async fn open_pty(
            &self,
            _request: PtyOpenRequest,
        ) -> Result<PtySessionHandle, MobileLinuxError> {
            Err(MobileLinuxError::Unsupported)
        }

        async fn write_pty(
            &self,
            _handle: &PtySessionHandle,
            _input: Vec<u8>,
        ) -> Result<(), MobileLinuxError> {
            Err(MobileLinuxError::Unsupported)
        }

        async fn resize_pty(
            &self,
            _handle: &PtySessionHandle,
            _size: PtySize,
        ) -> Result<(), MobileLinuxError> {
            Err(MobileLinuxError::Unsupported)
        }

        async fn close_pty(&self, _handle: &PtySessionHandle) -> Result<(), MobileLinuxError> {
            Err(MobileLinuxError::Unsupported)
        }

        async fn rootfs_status(&self) -> Result<RootfsStatus, MobileLinuxError> {
            Ok(RootfsStatus {
                state: RootfsState::Ready,
                backend: self.backend(),
                mode: self.mode(),
                platform: "ios".into(),
                abi: "arm64".into(),
                version: None,
                managed_root: None,
                active_root: None,
                staged_root: None,
                archive_sha256: None,
                installed_size_bytes: None,
                writable_guest_paths: guest_paths::writable_roots()
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
                last_error: None,
            })
        }

        async fn verify_rootfs(&self) -> Result<RootfsStatus, MobileLinuxError> {
            self.rootfs_status().await
        }

        async fn repair_rootfs(&self) -> Result<RootfsStatus, MobileLinuxError> {
            self.rootfs_status().await
        }

        async fn reset_rootfs(&self) -> Result<RootfsStatus, MobileLinuxError> {
            self.rootfs_status().await
        }

        async fn configure_mounts(
            &self,
            _mounts: Vec<traits::MountSpec>,
        ) -> Result<(), MobileLinuxError> {
            Ok(())
        }

        async fn list_tasks(&self) -> Result<Vec<MobileLinuxTaskSnapshot>, MobileLinuxError> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn empty_workspaces_use_vite_and_reject_legacy_next_markers() {
        let root = tempfile::tempdir().expect("tempdir");
        let layout = AppLayout::new(root.path(), "aaaa1111").expect("layout");
        layout.initialize().expect("initialize");
        assert_eq!(
            detect_build_target(&layout).expect("empty workspace defaults to Vite"),
            LocalAppBuildTarget::ViteReactStaticV1
        );

        let workspace = layout.root().join(layout.workspace_rel());
        fs::write(workspace.join("next.config.mjs"), "export default {};").expect("Next marker");
        let error = detect_build_target(&layout).expect_err("legacy Next marker must be rejected");
        assert!(
            error
                .to_string()
                .contains("local apps now support Vite only"),
            "{error:?}"
        );
        fs::remove_file(workspace.join("next.config.mjs")).expect("remove Next marker");
        fs::write(workspace.join("vite.config.mjs"), "export default {};").expect("Vite marker");
        assert_eq!(
            detect_build_target(&layout).expect("Vite marker selects Vite"),
            LocalAppBuildTarget::ViteReactStaticV1
        );
        fs::remove_file(workspace.join("vite.config.mjs")).expect("remove Vite marker");
        fs::write(workspace.join("vite.config.js"), "export default {};").expect("Vite JS marker");
        assert_eq!(
            detect_build_target(&layout).expect("Vite JS marker selects Vite"),
            LocalAppBuildTarget::ViteReactStaticV1
        );
    }

    #[test]
    fn build_source_pins_host_managed_files_and_excludes_workspace_metadata() {
        let root = tempfile::tempdir().expect("tempdir");
        let workspace = root.path().join("workspace");
        let output = root.path().join("build");
        fs::create_dir_all(workspace.join("node_modules/vite/bin")).expect("node_modules");
        fs::create_dir_all(workspace.join(".lingxi")).expect("lingxi metadata");
        fs::create_dir_all(workspace.join("lib")).expect("lib");
        fs::create_dir_all(workspace.join("app")).expect("app");
        fs::write(
            workspace.join("node_modules/vite/bin/vite.js"),
            "#!/usr/bin/env node\n",
        )
        .expect("vite cli");
        fs::write(workspace.join(".lingxi/source-policy.json"), "{}").expect("metadata");
        fs::write(workspace.join("LINGXI.md"), "workspace context").expect("lingxi context");
        fs::write(workspace.join("lib/lingxi-bridge.js"), "tampered").expect("bridge");
        fs::write(workspace.join("package.json"), "{\"tampered\":true}").expect("package");
        fs::write(workspace.join("app/main.jsx"), "export default 'custom';").expect("source");

        replace_build_source(&workspace, &output, LocalAppBuildTarget::ViteReactStaticV1)
            .expect("copy source");
        assert_eq!(
            fs::read_to_string(output.join("app/main.jsx")).unwrap(),
            "export default 'custom';"
        );
        assert_eq!(
            fs::read(output.join("package.json")).unwrap(),
            VITE_LOCKED_FILES
                .iter()
                .find(|(relative, _)| *relative == "package.json")
                .map(|(_, bytes)| *bytes)
                .unwrap()
        );
        assert_eq!(
            fs::read(output.join("lib/lingxi-bridge.js")).unwrap(),
            VITE_LOCKED_FILES
                .iter()
                .find(|(relative, _)| *relative == "lib/lingxi-bridge.js")
                .map(|(_, bytes)| *bytes)
                .unwrap()
        );
        assert!(!output.join("node_modules").exists());
        assert!(!output.join(".lingxi").exists());
        assert!(!output.join("LINGXI.md").exists());
    }

    #[test]
    fn build_source_omits_the_generated_vite_output_directory() {
        let root = tempfile::tempdir().expect("tempdir");
        let workspace = root.path().join("workspace");
        let output = root.path().join("build");
        fs::create_dir_all(workspace.join("dist/assets")).expect("current Vite output");
        fs::write(workspace.join("dist/assets/current.js"), "generated")
            .expect("current output asset");
        fs::create_dir_all(workspace.join("src")).expect("source");
        fs::write(workspace.join("src/main.jsx"), "export default null;").expect("source file");

        replace_build_source(&workspace, &output, LocalAppBuildTarget::ViteReactStaticV1)
            .expect("copy source");

        assert!(output.join("src/main.jsx").is_file());
        assert!(!output.join("dist").exists());
    }

    #[tokio::test]
    async fn dependency_wait_reports_not_ready_when_workspace_vite_is_missing() {
        let root = tempfile::tempdir().expect("tempdir");
        let workspace = root.path().join("workspace");
        fs::create_dir_all(&workspace).expect("workspace");

        let error = await_workspace_dependencies(&workspace, std::time::Duration::from_millis(1))
            .await
            .expect_err("missing workspace node_modules must block the build");

        assert!(
            matches!(error, AppError::NotYetAvailable(_)),
            "unexpected error: {error:?}"
        );
        assert!(
            error
                .to_string()
                .contains("workspace/node_modules/vite/bin/vite.js"),
            "{error:?}"
        );
    }

    #[test]
    fn successful_build_promotion_replaces_output_without_deleting_stage_first() {
        let root = tempfile::tempdir().expect("tempdir");
        let build_root = root.path().join("store");
        let staging_root = root.path().join(".store.staging-test");
        fs::create_dir_all(build_root.join(VITE_OUTPUT_DIR)).expect("current output");
        fs::write(build_root.join(VITE_OUTPUT_DIR).join("index.html"), "old").expect("old output");
        fs::create_dir_all(staging_root.join(VITE_OUTPUT_DIR)).expect("staged output");
        fs::write(staging_root.join(VITE_OUTPUT_DIR).join("index.html"), "new")
            .expect("new output");

        promote_build_root(&staging_root, &build_root).expect("promote output");

        assert_eq!(
            fs::read_to_string(build_root.join(VITE_OUTPUT_DIR).join("index.html")).unwrap(),
            "new"
        );
        assert!(!staging_root.exists());
    }

    #[test]
    fn interrupted_build_promotion_restores_last_good_output_and_cleans_artifacts() {
        let root = tempfile::tempdir().expect("tempdir");
        let build_root = root.path().join("store");
        let backup = root.path().join(".store.previous-123");
        let staging = root.path().join(".store.staging-456");
        fs::create_dir_all(backup.join(VITE_OUTPUT_DIR)).expect("backup output");
        fs::write(backup.join(VITE_OUTPUT_DIR).join("index.html"), "last-good")
            .expect("backup html");
        fs::create_dir_all(staging.join(VITE_OUTPUT_DIR)).expect("staging output");
        fs::write(staging.join(VITE_OUTPUT_DIR).join("index.html"), "partial")
            .expect("staging html");

        recover_build_promotion(&build_root).expect("recover interrupted promotion");

        assert_eq!(
            fs::read_to_string(build_root.join(VITE_OUTPUT_DIR).join("index.html")).unwrap(),
            "last-good"
        );
        assert!(!backup.exists());
        assert!(!staging.exists());
    }

    #[test]
    fn vite_build_uses_the_project_local_cli_path() {
        assert_eq!(
            build_tool_executable("/var/lingxi/local-app-build/aaaa1111/store/project"),
            "/var/lingxi/local-app-build/aaaa1111/store/project/node_modules/vite/bin/vite.js"
        );
    }

    #[test]
    fn prune_staging_root_publishes_only_dist() {
        let root = tempfile::tempdir().expect("tempdir");
        let staging = root.path().join("staging");
        fs::create_dir_all(staging.join("dist/assets")).expect("dist");
        fs::create_dir_all(staging.join("node_modules/vite")).expect("deps");
        fs::create_dir_all(staging.join("app")).expect("app");
        fs::write(staging.join("dist/index.html"), "<html/>").expect("index");
        fs::write(staging.join("app/main.jsx"), "export default null;").expect("source");

        prune_staging_root_for_publish(&staging).expect("prune staging");

        assert!(staging.join("dist/index.html").is_file());
        assert!(!staging.join("node_modules").exists());
        assert!(!staging.join("app").exists());
    }

    #[test]
    fn fixed_vite_build_forces_the_canonical_dist_output() {
        assert_eq!(
            fixed_vite_build_args(
                2_048,
                "/runtime/vite.js".into(),
                ".lingxi-build-state/build-output/dist".into()
            ),
            [
                "--max-old-space-size=1536",
                "/runtime/vite.js",
                "build",
                "--outDir",
                ".lingxi-build-state/build-output/dist",
                "--emptyOutDir",
            ]
        );
    }

    #[test]
    fn local_app_build_mount_uses_the_isolated_project_as_its_command_root() {
        let root = tempfile::tempdir().expect("tempdir");
        let staging = root.path().join("build/.store.staging-123");
        let mount = local_app_build_mount("aaaa1111", "store", &staging);

        assert_eq!(mount.host_path, staging);
        assert_eq!(
            mount.guest_path,
            "/var/lingxi/local-app-build/aaaa1111/store/project"
        );
        assert!(!mount.read_only);
        assert_eq!(mount.purpose, MountPurpose::LocalAppBuild);
    }

    #[test]
    fn scaffold_workspace_materializes_the_pinned_template_in_place() {
        let root = tempfile::tempdir().expect("tempdir");
        let layout = AppLayout::new(root.path(), "aaaa1111").expect("layout");

        scaffold_workspace(&layout).expect("scaffold workspace");

        let workspace = layout.root().join(layout.workspace_rel());
        assert!(workspace.join(".gitignore").is_file());
        assert!(workspace.join("package.json").is_file());
        assert!(workspace.join("pnpm-lock.yaml").is_file());
        assert!(workspace.join("pnpm-workspace.yaml").is_file());
        assert!(!workspace.join("package-lock.json").exists());
        assert!(workspace.join("index.html").is_file());
        assert!(workspace.join("vite.config.mjs").is_file());
        assert!(workspace.join("components.json").is_file());
        assert!(workspace.join("jsconfig.json").is_file());
        assert!(workspace.join("app/main.jsx").is_file());
        assert!(workspace.join("app/globals.css").is_file());
        assert!(workspace.join("app/providers.jsx").is_file());
        assert!(workspace.join("app/screens/component-lab.jsx").is_file());
        assert!(workspace.join("components/ui/button.jsx").is_file());
        assert!(workspace.join("styles/foundation.css").is_file());
        assert!(workspace.join(".lingxi/source-policy.json").is_file());
        let gitignore = fs::read_to_string(workspace.join(".gitignore")).expect("gitignore");
        assert!(gitignore.lines().any(|line| line == "node_modules/"));
        assert!(gitignore.lines().any(|line| line == "dist/"));
        assert!(!gitignore.lines().any(|line| line == "package-lock.json"));
    }

    #[cfg(unix)]
    #[test]
    fn scaffold_workspace_replaces_host_managed_symlinks_with_regular_files() {
        let root = tempfile::tempdir().expect("tempdir");
        let layout = AppLayout::new(root.path(), "aaaa1111").expect("layout");
        layout.initialize().expect("initialize");
        let workspace = layout.root().join(layout.workspace_rel());
        let outside = root.path().join("outside-package.json");
        fs::write(&outside, "{\"outside\":true}").expect("outside file");
        std::os::unix::fs::symlink(&outside, workspace.join("package.json"))
            .expect("symlink package.json");
        scaffold_workspace(&layout).expect("scaffold workspace");

        let metadata = fs::symlink_metadata(workspace.join("package.json")).expect("metadata");
        assert!(metadata.is_file());
        assert!(!metadata.file_type().is_symlink());
        assert_eq!(
            fs::read_to_string(&outside).expect("outside"),
            "{\"outside\":true}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn restore_replaces_matching_host_managed_symlinks_without_following_them() {
        let root = tempfile::tempdir().expect("tempdir");
        let workspace = root.path().join("workspace");
        fs::create_dir_all(&workspace).expect("workspace");
        let pinned_package = VITE_LOCKED_FILES
            .iter()
            .find(|(relative, _)| *relative == "package.json")
            .map(|(_, bytes)| *bytes)
            .expect("pinned package bytes");
        let outside = root.path().join("outside-package.json");
        fs::write(&outside, pinned_package).expect("outside file");
        std::os::unix::fs::symlink(&outside, workspace.join("package.json"))
            .expect("symlink package.json");

        restore_host_managed_files(&workspace, LocalAppBuildTarget::ViteReactStaticV1)
            .expect("restore host-managed files");

        let metadata = fs::symlink_metadata(workspace.join("package.json")).expect("metadata");
        assert!(metadata.is_file());
        assert!(!metadata.file_type().is_symlink());
        assert_eq!(
            fs::read(workspace.join("package.json")).unwrap(),
            pinned_package
        );
        assert_eq!(fs::read(&outside).unwrap(), pinned_package);
    }

    #[cfg(unix)]
    #[test]
    fn restore_replaces_symlinked_host_managed_parent_without_writing_outside() {
        let root = tempfile::tempdir().expect("tempdir");
        let workspace = root.path().join("workspace");
        let outside_lib = root.path().join("outside-lib");
        fs::create_dir_all(&workspace).expect("workspace");
        fs::create_dir_all(&outside_lib).expect("outside lib");
        let pinned_bridge = VITE_LOCKED_FILES
            .iter()
            .find(|(relative, _)| *relative == "lib/lingxi-bridge.js")
            .map(|(_, bytes)| *bytes)
            .expect("pinned bridge bytes");
        fs::write(outside_lib.join("lingxi-bridge.js"), pinned_bridge).expect("outside bridge");
        std::os::unix::fs::symlink(&outside_lib, workspace.join("lib"))
            .expect("symlink lib parent");

        restore_host_managed_files(&workspace, LocalAppBuildTarget::ViteReactStaticV1)
            .expect("restore host-managed files");

        let lib_metadata = fs::symlink_metadata(workspace.join("lib")).expect("lib metadata");
        assert!(lib_metadata.is_dir());
        assert!(!lib_metadata.file_type().is_symlink());
        assert_eq!(
            fs::read(workspace.join("lib/lingxi-bridge.js")).unwrap(),
            pinned_bridge
        );
        assert_eq!(
            fs::read(outside_lib.join("lingxi-bridge.js")).unwrap(),
            pinned_bridge,
            "restoring the workspace must not rewrite a symlink target outside it"
        );
    }

    /// The build still promotes only validated output. If the runtime itself is
    /// unavailable, the live build output must survive untouched.
    #[tokio::test]
    async fn an_unavailable_runtime_fails_before_the_build_root_is_destroyed() {
        let root = tempfile::tempdir().expect("tempdir");
        let layout = AppLayout::new(root.path(), "aaaa1111").expect("layout");
        layout.initialize().expect("initialize");
        let workspace = layout.root().join(layout.workspace_rel());
        fs::write(workspace.join("vite.config.mjs"), "export default {};").expect("Vite marker");

        // The output the preview server is serving right now.
        let build_root = layout.root().join(layout.build_rel(false));
        let served_index = build_root.join(VITE_OUTPUT_DIR).join("index.html");
        fs::create_dir_all(served_index.parent().expect("dist dir")).expect("dist dir");
        fs::write(&served_index, "<html>live</html>").expect("served index");

        let broker = LocalAppsHostBroker::new(
            root.path().to_path_buf(),
            client_adapter::MockSink::arc(),
            None,
            false,
            None,
        );
        let builder = LocalAppBuilder {
            mobile_linux: None,
            host: broker.as_ref(),
        };

        let error = builder
            .build_workspace(&layout)
            .await
            .expect_err("an unavailable runtime must fail the build");
        assert!(
            matches!(error, AppError::NotYetAvailable(_)),
            "unexpected error: {error:?}"
        );
        assert!(
            served_index.is_file(),
            "the live preview output must survive a build that never started"
        );
    }

    /// `build_workspace` is the enforcement point: host-managed infrastructure
    /// is restored while editable app source survives.
    #[tokio::test]
    async fn the_build_re_pins_host_managed_infrastructure_without_touching_app_owned_files() {
        let root = tempfile::tempdir().expect("tempdir");
        let layout = AppLayout::new(root.path(), "aaaa1111").expect("layout");
        scaffold_workspace(&layout).expect("scaffold workspace");
        let workspace = layout.root().join(layout.workspace_rel());
        let pinned_bridge = VITE_LOCKED_FILES
            .iter()
            .find(|(relative, _)| *relative == "lib/lingxi-bridge.js")
            .map(|(_, bytes)| *bytes)
            .expect("pinned bridge bytes");
        let pinned_gitignore = VITE_LOCKED_FILES
            .iter()
            .find(|(relative, _)| *relative == ".gitignore")
            .map(|(_, bytes)| *bytes)
            .expect("pinned gitignore bytes");
        let pinned_package = VITE_LOCKED_FILES
            .iter()
            .find(|(relative, _)| *relative == "package.json")
            .map(|(_, bytes)| *bytes)
            .expect("pinned package bytes");
        let pinned_index = VITE_LOCKED_FILES
            .iter()
            .find(|(relative, _)| *relative == "index.html")
            .map(|(_, bytes)| *bytes)
            .expect("pinned index bytes");

        fs::write(
            workspace.join("lib/lingxi-bridge.js"),
            "export const lingxi = { exfiltrate: true };",
        )
        .expect("tampered bridge");
        fs::write(workspace.join(".gitignore"), b"").expect("tampered gitignore");
        fs::write(workspace.join("package.json"), br#"{"tampered":true}"#).expect("package.json");
        fs::write(
            workspace.join("index.html"),
            b"<!doctype html><div>tampered</div>",
        )
        .expect("index.html");
        fs::write(
            workspace.join("app/main.jsx"),
            "export default 'custom main';",
        )
        .expect("main.jsx");
        fs::write(
            workspace.join("app/globals.css"),
            "body { color: hotpink; }",
        )
        .expect("globals.css");
        fs::create_dir_all(workspace.join("node_modules/vite/bin")).expect("node_modules");
        fs::write(
            workspace.join("node_modules/vite/bin/vite.js"),
            "#!/usr/bin/env node\n",
        )
        .expect("workspace vite");

        let broker = LocalAppsHostBroker::new(
            root.path().to_path_buf(),
            client_adapter::MockSink::arc(),
            None,
            false,
            None,
        );
        let runtime: Arc<dyn MobileLinuxRuntime> =
            Arc::new(traits::UnavailableMobileLinuxRuntime::unavailable(
                traits::SandboxBackend::IosIsh,
                traits::MobileLinuxRuntimeMode::MobileLinux,
                "ios",
                "arm64",
                "test runtime never executes node",
            ));
        let builder = LocalAppBuilder {
            mobile_linux: Some(runtime),
            host: broker.as_ref(),
        };

        // The preflight passes, so preparation runs; only `node` itself fails.
        builder
            .build_workspace(&layout)
            .await
            .expect_err("the stub runtime cannot run the build tool");

        assert_eq!(
            fs::read(workspace.join("lib/lingxi-bridge.js")).expect("bridge"),
            pinned_bridge,
            "a model-rewritten bridge must be restored before the build"
        );
        assert_eq!(
            fs::read(workspace.join(".gitignore")).expect("gitignore"),
            pinned_gitignore,
            "the dependency and build-output exclusions must be restored before the build"
        );
        assert!(workspace.join(".lingxi/source-policy.json").is_file());
        assert_eq!(
            fs::read(workspace.join("package.json")).expect("package.json"),
            pinned_package,
            "host-managed package.json must be restored before the build"
        );
        assert_eq!(
            fs::read(workspace.join("index.html")).expect("index.html"),
            pinned_index,
            "host-managed index.html must be restored before the build"
        );
        assert_eq!(
            fs::read_to_string(workspace.join("app/main.jsx")).expect("main.jsx"),
            "export default 'custom main';"
        );
        assert_eq!(
            fs::read_to_string(workspace.join("app/globals.css")).expect("globals.css"),
            "body { color: hotpink; }"
        );
    }

    #[tokio::test]
    async fn a_failed_build_cleans_up_its_staging_directory() {
        let root = tempfile::tempdir().expect("tempdir");
        let layout = AppLayout::new(root.path(), "aaaa1111").expect("layout");
        scaffold_workspace(&layout).expect("scaffold workspace");
        let workspace = layout.root().join(layout.workspace_rel());
        fs::create_dir_all(workspace.join("node_modules/vite/bin")).expect("node_modules");
        fs::write(
            workspace.join("node_modules/vite/bin/vite.js"),
            "#!/usr/bin/env node\n",
        )
        .expect("workspace vite");

        let broker = LocalAppsHostBroker::new(
            root.path().to_path_buf(),
            client_adapter::MockSink::arc(),
            None,
            false,
            None,
        );
        let runtime: Arc<dyn MobileLinuxRuntime> =
            Arc::new(traits::UnavailableMobileLinuxRuntime::unavailable(
                traits::SandboxBackend::IosIsh,
                traits::MobileLinuxRuntimeMode::MobileLinux,
                "ios",
                "arm64",
                "test runtime never executes node",
            ));
        let builder = LocalAppBuilder {
            mobile_linux: Some(runtime),
            host: broker.as_ref(),
        };

        builder
            .build_workspace(&layout)
            .await
            .expect_err("the stub runtime cannot run the build tool");

        assert!(
            !workspace_build_artifact_root(&workspace).exists(),
            "failed builds must not leave private build artifacts behind"
        );
    }

    #[tokio::test]
    async fn build_runs_from_the_workspace_mount_and_promotes_private_output() {
        let root = tempfile::tempdir().expect("tempdir");
        let layout = AppLayout::new(root.path(), "aaaa1111").expect("layout");
        scaffold_workspace(&layout).expect("scaffold workspace");
        let workspace = layout.root().join(layout.workspace_rel());
        fs::create_dir_all(workspace.join("node_modules/vite/bin")).expect("node_modules");
        fs::write(
            workspace.join("node_modules/vite/bin/vite.js"),
            "#!/usr/bin/env node\n",
        )
        .expect("workspace vite");

        let build_root = layout.root().join(layout.build_rel(false));
        fs::create_dir_all(build_root.join(VITE_OUTPUT_DIR)).expect("old dist");
        fs::write(
            build_root.join(VITE_OUTPUT_DIR).join("index.html"),
            "<html>old</html>",
        )
        .expect("old output");

        let runtime = RecordingIsolatedRuntime::new();
        let broker = LocalAppsHostBroker::new(
            root.path().to_path_buf(),
            client_adapter::MockSink::arc(),
            Some(runtime.clone() as Arc<dyn MobileLinuxRuntime>),
            false,
            None,
        );
        let builder = LocalAppBuilder {
            mobile_linux: Some(runtime.clone() as Arc<dyn MobileLinuxRuntime>),
            host: broker.as_ref(),
        };

        builder
            .build_workspace(&layout)
            .await
            .expect("workspace build");

        let request = runtime.recorded_request().await;
        assert_eq!(runtime.isolated_runs.load(Ordering::SeqCst), 1);
        assert_eq!(request.mounts.len(), 1);
        assert_eq!(request.mounts[0].host_path, workspace);
        assert_eq!(
            request.mounts[0].guest_path,
            guest_paths::local_app_build_project("aaaa1111", "store")
        );
        assert_eq!(
            request.cwd.as_deref(),
            Some(request.mounts[0].guest_path.as_str())
        );
        assert!(request.args.windows(2).any(|window| {
            window[0] == "--outDir" && window[1] == ".lingxi-build-state/build-output/dist"
        }));
        assert!(
            fs::read_to_string(build_root.join(VITE_OUTPUT_DIR).join("index.html"))
                .expect("promoted index")
                .contains("fresh")
        );
        assert!(
            !workspace_build_artifact_root(&workspace).exists(),
            "private build output should be consumed by promotion"
        );
    }

    #[test]
    fn the_locked_bridge_exposes_the_native_data_wire_contract() {
        let bridge = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../local-apps/templates/vite-react-static-v1/lib/lingxi-bridge.js"
        ));
        for anchor in [
            "records[].document",
            "export async function upsertRecord",
            "kind: \"upsert\"",
            "recordId",
            "document",
            "export async function deleteRecord",
            "kind: \"delete\"",
        ] {
            assert!(bridge.contains(anchor), "bridge missing {anchor}");
        }
    }

    #[test]
    fn platform_adapter_declares_distinct_phone_and_tablet_presentations() {
        let adapter = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../local-apps/templates/vite-react-static-v1/lib/platform-adapter.js"
        ));
        let foundation = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../local-apps/templates/vite-react-static-v1/styles/foundation.css"
        ));
        let vite_config = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../local-apps/templates/vite-react-static-v1/vite.config.mjs"
        ));
        for marker in [
            "ios:iphone",
            "android:phone",
            "ios:tablet",
            "android:tablet",
            "44pt",
            "48dp",
            "sidebar-or-split-view",
            "rail-and-adaptive-pane",
        ] {
            assert!(adapter.contains(marker), "adapter missing {marker}");
        }
        assert!(
            vite_config.contains("outDir: \"dist\""),
            "the pinned Vite config must use Vite's official dist output"
        );
        assert!(
            !vite_config.contains("/opt/lingxi/local-app-runtime"),
            "the pinned Vite config must not depend on a shared runtime mount"
        );
        assert!(
            !vite_config.contains("NODE_PATH"),
            "the pinned Vite config must not rely on NODE_PATH"
        );
        assert!(foundation.contains("[data-platform^=\"ios:\"]"));
        assert!(foundation.contains("[data-platform^=\"android:\"]"));
        assert!(foundation.contains("--safe-area-top: env(safe-area-inset-top"));
        assert!(foundation.contains("prefers-reduced-motion"));
    }
}

//! The local-app BUILD CORE: template scaffolding plus the fixed offline
//! Vite build, extracted from the generation pipeline so that
//! scaffold/build no longer belongs to the LLM-generation executor.

use crate::local_apps_host::LocalAppsHostBroker;
use local_apps::{AppDataStore, AppError, AppLayout, AppManifest};
use std::collections::{BTreeMap, HashSet};
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
const RUNTIME_SEED_WAIT_TIMEOUT_MS: u64 = 120_000;
/// Vite's default deployment directory, relative to the isolated project root.
pub(crate) const VITE_OUTPUT_DIR: &str = "dist";
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LocalAppBuildTarget {
    ViteReactStaticV1,
}

pub(crate) const VITE_LOCKED_FILES: &[(&str, &[u8])] = &[
    (
        "package.json",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../local-apps/templates/vite-react-static-v1/package.json"
        )),
    ),
    (
        "package-lock.json",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../local-apps/templates/vite-react-static-v1/package-lock.json"
        )),
    ),
    (
        "index.html",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../local-apps/templates/vite-react-static-v1/index.html"
        )),
    ),
    (
        "vite.config.mjs",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../local-apps/templates/vite-react-static-v1/vite.config.mjs"
        )),
    ),
    (
        ".lingxi/source-policy.json",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../local-apps/templates/vite-react-static-v1/.lingxi/source-policy.json"
        )),
    ),
    (
        "lib/lingxi-bridge.js",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../local-apps/templates/vite-react-static-v1/lib/lingxi-bridge.js"
        )),
    ),
    (
        "lib/device-context.js",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../local-apps/templates/vite-react-static-v1/lib/device-context.js"
        )),
    ),
    (
        "lib/platform-adapter.js",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../local-apps/templates/vite-react-static-v1/lib/platform-adapter.js"
        )),
    ),
];

const SOURCE_FILES: &[(&str, &[u8])] = &[
    (
        "app/main.jsx",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../local-apps/templates/vite-react-static-v1/app/main.jsx"
        )),
    ),
    (
        "app/globals.css",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../local-apps/templates/vite-react-static-v1/app/globals.css"
        )),
    ),
    ("public/.gitkeep", b""),
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
        "package.json",
        "package-lock.json",
        "index.html",
        "vite.config.mjs",
        ".lingxi/source-policy.json",
        "lib/lingxi-bridge.js",
        "lib/device-context.js",
        "lib/platform-adapter.js",
    ]
}

fn build_locked_files(_target: LocalAppBuildTarget) -> &'static [&'static str] {
    &[
        "package.json",
        "package-lock.json",
        "index.html",
        "vite.config.mjs",
        "lib/lingxi-bridge.js",
        "lib/device-context.js",
        "lib/platform-adapter.js",
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

fn fixed_vite_build_args(build_memory_mb: u32, executable: String) -> Vec<String> {
    vec![
        format!(
            "--max-old-space-size={}",
            node_old_space_mb(build_memory_mb)
        ),
        executable,
        "build".into(),
        "--outDir".into(),
        VITE_OUTPUT_DIR.into(),
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
    /// The two preconditions a fixed build cannot start without: the mobile
    /// Node runtime and the verified runtime snapshot. Checked up front by
    /// [`Self::build_workspace`] so a doomed build never reaches the
    /// staging `replace_build_source` step, and re-checked inside
    /// [`Self::run_fixed_build`] where the values are actually used.
    fn assert_build_runtime_available(&self) -> Result<(), AppError> {
        if self.mobile_linux.is_none() {
            return Err(AppError::NotYetAvailable(
                "the verified mobile Node runtime is unavailable in this build".into(),
            ));
        }
        self.host
            .configured_runtime_root()
            .map_err(AppError::NotYetAvailable)?;
        Ok(())
    }

    async fn run_fixed_build(&self, layout: &AppLayout, build_root: &Path) -> Result<(), AppError> {
        let runtime = self.mobile_linux.as_ref().ok_or_else(|| {
            AppError::NotYetAvailable(
                "the verified mobile Node runtime is unavailable in this build".into(),
            )
        })?;
        let build_channel = "store";
        let build_mount = local_app_build_mount(layout.app_id(), build_channel, build_root);
        let project_guest_path = build_mount.guest_path.clone();
        let tool_name = "Vite";
        let mut environment = BTreeMap::new();
        let build_state_root = build_root.join(".lingxi-build-state");
        let home_root = build_state_root.join("home");
        let temp_root = build_state_root.join("tmp");
        let xdg_cache_root = build_state_root.join("xdg-cache");
        let xdg_config_root = build_state_root.join("xdg-config");
        let xdg_data_root = build_state_root.join("xdg-data");
        let npm_cache_root = build_state_root.join("npm-cache");
        for directory in [
            &home_root,
            &temp_root,
            &xdg_cache_root,
            &xdg_config_root,
            &xdg_data_root,
            &npm_cache_root,
        ] {
            std::fs::create_dir_all(directory)
                .map_err(|error| AppError::Io(format!("create build state directory: {error}")))?;
        }
        let executable = build_tool_executable(&project_guest_path);
        let staged_vite = build_root.join("node_modules/vite/bin/vite.js");
        match std::fs::symlink_metadata(&staged_vite) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {}
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(AppError::InvalidRequest(
                    "staged Vite executable must not be a symlink".into(),
                ));
            }
            Ok(_) => {
                return Err(AppError::InvalidRequest(
                    "staged Vite executable must be a regular file".into(),
                ));
            }
            Err(error) => {
                return Err(AppError::Io(format!(
                    "inspect staged Vite executable: {error}"
                )));
            }
        }
        let guest_build_state_root = format!("{project_guest_path}/.lingxi-build-state");
        let guest_home_root = format!("{guest_build_state_root}/home");
        let guest_temp_root = format!("{guest_build_state_root}/tmp");
        let guest_xdg_cache_root = format!("{guest_build_state_root}/xdg-cache");
        let guest_xdg_config_root = format!("{guest_build_state_root}/xdg-config");
        let guest_xdg_data_root = format!("{guest_build_state_root}/xdg-data");
        let guest_npm_cache_root = format!("{guest_build_state_root}/npm-cache");
        environment.insert("NODE_ENV".into(), "production".into());
        environment.insert("HOME".into(), guest_home_root);
        environment.insert("TMPDIR".into(), guest_temp_root.clone());
        environment.insert("TMP".into(), guest_temp_root.clone());
        environment.insert("TEMP".into(), guest_temp_root);
        environment.insert("XDG_CACHE_HOME".into(), guest_xdg_cache_root);
        environment.insert("XDG_CONFIG_HOME".into(), guest_xdg_config_root);
        environment.insert("XDG_DATA_HOME".into(), guest_xdg_data_root);
        environment.insert("NPM_CONFIG_CACHE".into(), guest_npm_cache_root.clone());
        environment.insert("npm_config_cache".into(), guest_npm_cache_root);
        let build_memory_mb = build_memory_budget_mb(self.host.physical_memory_bytes());
        let resource_limits = ResourceLimits {
            max_memory_mb: Some(build_memory_mb),
            ..ResourceLimits::default()
        };
        let mounts = vec![build_mount];
        let request = LinuxCommandRequest {
            command: "/usr/bin/node".into(),
            args: fixed_vite_build_args(build_memory_mb, executable),
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
        build_root: &Path,
    ) -> Result<(), AppError> {
        self.run_fixed_build(layout, build_root).await
    }

    /// Materialize the workspace, including app dependencies, into the build root
    /// and run the fixed offline Vite build. The resulting directory is the only
    /// app-owned build mount; dependencies must not be supplied by a nested mount.
    pub(crate) async fn build_workspace(&self, layout: &AppLayout) -> Result<(), AppError> {
        let build_lock = self.host.build_lock();
        let _build_guard = build_lock.lock().await;
        // The process-global guard serializes memory-heavy dependency copies
        // and Node builds across every broker in this process. The per-app
        // storage lock extends exclusion for this app across engine processes
        // and the native delete path, which otherwise could rename the app
        // while promotion is between its two directory renames.
        let _process_build_guard =
            local_apps::storage::lock_app_build(layout.root(), layout.app_id())?;
        let workspace = layout.root().join(layout.workspace_rel());
        // Availability FIRST. The build is staged separately and promoted
        // only after Vite succeeds, but these gates still avoid creating
        // throwaway staging trees for a runtime that cannot build.
        self.assert_build_runtime_available()?;
        let target = detect_build_target(layout)?;
        let runtime_root = self
            .host
            .await_fixed_runtime_root(std::time::Duration::from_millis(
                RUNTIME_SEED_WAIT_TIMEOUT_MS,
            ))
            .await
            .map_err(AppError::NotYetAvailable)?;
        // Re-pin the host-managed files from the compiled-in templates on
        // EVERY build, before anything is copied into the build root.
        restore_host_managed_files(&workspace, target)?;
        let build_root = layout.root().join(layout.build_rel(false));
        recover_build_promotion(&build_root)?;
        let staging_root = build_staging_root(&build_root)?;
        let build_result = async {
            let prepare_workspace = workspace.clone();
            let prepare_staging_root = staging_root.clone();
            let prepare_runtime_root = runtime_root.clone();
            tokio::task::spawn_blocking(move || {
                replace_build_source(&prepare_workspace, &prepare_staging_root, target)?;
                materialize_runtime_dependencies(&prepare_runtime_root, &prepare_staging_root)
            })
            .await
            .map_err(|error| AppError::Io(format!("build preparation worker failed: {error}")))??;
            self.run_vite_build(layout, &staging_root).await?;
            let validate_staging_root = staging_root.clone();
            let validate_build_root = build_root.clone();
            tokio::task::spawn_blocking(move || {
                validate_build_output(&validate_staging_root)?;
                prune_staging_root_for_publish(&validate_staging_root)?;
                promote_build_root(&validate_staging_root, &validate_build_root)
            })
            .await
            .map_err(|error| AppError::Io(format!("build promotion worker failed: {error}")))?
        }
        .await;
        if build_result.is_err() {
            let _ = std::fs::remove_dir_all(&staging_root);
        }
        build_result
    }
}

fn build_staging_root(build_root: &Path) -> Result<PathBuf, AppError> {
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
    let staging = parent.join(format!(".{file_name}.staging-{stamp}"));
    if staging.exists() {
        std::fs::remove_dir_all(&staging)
            .map_err(|error| AppError::Io(format!("clear stale build staging: {error}")))?;
    }
    Ok(staging)
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
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_dir.join("build.log"))
        .await
        .map_err(|error| AppError::Io(format!("open build log: {error}")))?;
    let channel = if full { "full" } else { "store" };
    let body = format!(
        "\n=== {channel} build ===\nstdout:\n{}\nstderr:\n{}\n",
        bounded_log(stdout),
        bounded_log(stderr)
    );
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
        Some(".git" | ".lingxi" | "dist" | "node_modules")
    ) || relative == Path::new("LINGXI.md")
}

fn materialize_runtime_dependencies(
    runtime_root: &Path,
    build_root: &Path,
) -> Result<(), AppError> {
    let dependency_root = runtime_root.join("node_modules");
    let vite = dependency_root.join("vite/bin/vite.js");
    let metadata = std::fs::symlink_metadata(&vite)
        .map_err(|error| AppError::Io(format!("inspect runtime Vite executable: {error}")))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(AppError::InvalidRequest(
            "verified runtime Vite executable must be a regular file".into(),
        ));
    }
    let canonical_root = std::fs::canonicalize(&dependency_root)
        .map_err(|error| AppError::Io(format!("resolve runtime dependency root: {error}")))?;
    let destination = build_root.join("node_modules");
    std::fs::create_dir_all(&destination)
        .map_err(|error| AppError::Io(format!("create staged dependency root: {error}")))?;
    let mut visited = HashSet::new();
    copy_dependency_contents(
        &canonical_root,
        &dependency_root,
        &destination,
        &mut visited,
    )
}

fn copy_dependency_contents(
    canonical_root: &Path,
    current: &Path,
    destination: &Path,
    visited: &mut HashSet<PathBuf>,
) -> Result<(), AppError> {
    let canonical_current = std::fs::canonicalize(current)
        .map_err(|error| AppError::Io(format!("resolve dependency directory: {error}")))?;
    if !visited.insert(canonical_current.clone()) {
        return Err(AppError::InvalidRequest(format!(
            "runtime dependency snapshot contains a directory cycle: {}",
            canonical_current.display()
        )));
    }
    for entry in std::fs::read_dir(current)
        .map_err(|error| AppError::Io(format!("read dependency snapshot: {error}")))?
    {
        let entry =
            entry.map_err(|error| AppError::Io(format!("read dependency entry: {error}")))?;
        let path = entry.path();
        let destination_path = destination.join(entry.file_name());
        copy_dependency_entry(canonical_root, &path, &destination_path, visited)?;
    }
    visited.remove(&canonical_current);
    Ok(())
}

fn copy_dependency_entry(
    canonical_root: &Path,
    source: &Path,
    destination: &Path,
    visited: &mut HashSet<PathBuf>,
) -> Result<(), AppError> {
    let metadata = std::fs::symlink_metadata(source)
        .map_err(|error| AppError::Io(format!("inspect dependency snapshot: {error}")))?;
    if metadata.file_type().is_symlink() {
        let resolved = std::fs::canonicalize(source).map_err(|error| {
            AppError::InvalidRequest(format!(
                "resolve runtime dependency symlink {}: {error}",
                source.display()
            ))
        })?;
        if !resolved.starts_with(canonical_root) {
            return Err(AppError::InvalidRequest(format!(
                "runtime dependency symlink escapes node_modules: {}",
                source.display()
            )));
        }
        let resolved_metadata = std::fs::symlink_metadata(&resolved).map_err(|error| {
            AppError::Io(format!(
                "inspect resolved runtime dependency {}: {error}",
                source.display()
            ))
        })?;
        if resolved_metadata.is_dir() {
            std::fs::create_dir_all(destination)
                .map_err(|error| AppError::Io(format!("create dependency directory: {error}")))?;
            copy_dependency_contents(canonical_root, &resolved, destination, visited)?;
        } else if resolved_metadata.is_file() {
            if let Some(parent) = destination.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| AppError::Io(format!("create dependency parent: {error}")))?;
            }
            std::fs::copy(&resolved, destination)
                .map_err(|error| AppError::Io(format!("copy dependency file: {error}")))?;
        } else {
            return Err(AppError::InvalidRequest(format!(
                "runtime dependency symlink target is not a regular file or directory: {}",
                source.display()
            )));
        }
    } else if metadata.is_dir() {
        std::fs::create_dir_all(destination)
            .map_err(|error| AppError::Io(format!("create dependency directory: {error}")))?;
        copy_dependency_contents(canonical_root, source, destination, visited)?;
    } else if metadata.is_file() {
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| AppError::Io(format!("create dependency parent: {error}")))?;
        }
        std::fs::copy(source, destination)
            .map_err(|error| AppError::Io(format!("copy dependency file: {error}")))?;
    }
    Ok(())
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
    use std::fs;

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

    #[cfg(unix)]
    #[test]
    fn runtime_dependency_snapshot_materializes_internal_symlinks() {
        let root = tempfile::tempdir().expect("tempdir");
        let runtime_root = root.path().join("runtime");
        let output = root.path().join("build");
        let vite = runtime_root.join("node_modules/vite/bin/vite.js");
        let bin = runtime_root.join("node_modules/.bin/vite");
        fs::create_dir_all(vite.parent().expect("vite bin parent")).expect("node_modules");
        fs::create_dir_all(bin.parent().expect("node_modules bin parent")).expect("bin dir");
        fs::write(&vite, "#!/usr/bin/env node\n").expect("vite cli");
        std::os::unix::fs::symlink("../vite/bin/vite.js", &bin).expect("vite bin symlink");

        materialize_runtime_dependencies(&runtime_root, &output).expect("stage dependencies");

        let materialized = output.join("node_modules/.bin/vite");
        assert!(materialized.is_file());
        assert!(!materialized
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            fs::read_to_string(materialized).unwrap(),
            "#!/usr/bin/env node\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn runtime_dependency_snapshot_rejects_directory_cycles() {
        let root = tempfile::tempdir().expect("tempdir");
        let runtime_root = root.path().join("runtime");
        let output = root.path().join("build");
        let vite = runtime_root.join("node_modules/vite/bin/vite.js");
        let cycle = runtime_root.join("node_modules/loop");
        fs::create_dir_all(vite.parent().expect("vite bin parent")).expect("node_modules");
        fs::write(&vite, "#!/usr/bin/env node\n").expect("vite cli");
        std::os::unix::fs::symlink(".", &cycle).expect("cycle symlink");

        let error = materialize_runtime_dependencies(&runtime_root, &output)
            .expect_err("cyclic dependency snapshot must be rejected");

        assert!(
            error
                .to_string()
                .contains("runtime dependency snapshot contains a directory cycle"),
            "{error:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn runtime_dependency_snapshot_rejects_symlinks_that_escape_node_modules() {
        let root = tempfile::tempdir().expect("tempdir");
        let runtime_root = root.path().join("runtime");
        let output = root.path().join("build");
        let vite = runtime_root.join("node_modules/vite/bin/vite.js");
        let outside = root.path().join("outside.js");
        let escaped = runtime_root.join("node_modules/.bin/escaped");
        fs::create_dir_all(vite.parent().expect("vite bin parent")).expect("node_modules");
        fs::create_dir_all(escaped.parent().expect("escaped parent")).expect("bin dir");
        fs::write(&vite, "#!/usr/bin/env node\n").expect("vite cli");
        fs::write(&outside, "console.log('outside')\n").expect("outside file");
        std::os::unix::fs::symlink(&outside, &escaped).expect("escaped symlink");

        let error = materialize_runtime_dependencies(&runtime_root, &output)
            .expect_err("escaping dependency symlink must be rejected");

        assert!(
            error
                .to_string()
                .contains("runtime dependency symlink escapes node_modules"),
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
            fixed_vite_build_args(2_048, "/runtime/vite.js".into()),
            [
                "--max-old-space-size=1536",
                "/runtime/vite.js",
                "build",
                "--outDir",
                "dist",
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
        assert!(workspace.join("package.json").is_file());
        assert!(workspace.join("index.html").is_file());
        assert!(workspace.join("vite.config.mjs").is_file());
        assert!(workspace.join("app/main.jsx").is_file());
        assert!(workspace.join("app/globals.css").is_file());
        assert!(workspace.join(".lingxi/source-policy.json").is_file());
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

    /// The build now uses a separate staging root and promotes it only after
    /// Vite succeeds. Tapping Rebuild while the runtime bundle is unavailable
    /// must still fail before creating staging work.
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

    /// A runtime that passes [`LocalAppBuilder::assert_build_runtime_available`]
    /// (so `build_workspace` runs its whole preparation) and then refuses the
    /// actual `node` invocation.
    fn staged_runtime_root(root: &Path) -> std::path::PathBuf {
        let runtime_root = root.join("runtime");
        let vite = runtime_root.join("node_modules/vite/bin/vite.js");
        fs::create_dir_all(vite.parent().expect("bin parent")).expect("runtime bin dir");
        fs::write(&vite, "#!/usr/bin/env node\n").expect("runtime bin");
        runtime_root
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

        let broker = LocalAppsHostBroker::new(
            root.path().to_path_buf(),
            client_adapter::MockSink::arc(),
            None,
            false,
            Some(staged_runtime_root(root.path())),
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

        let broker = LocalAppsHostBroker::new(
            root.path().to_path_buf(),
            client_adapter::MockSink::arc(),
            None,
            false,
            Some(staged_runtime_root(root.path())),
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

        let build_root = root.path().join(layout.build_rel(false));
        let parent = build_root.parent().expect("build root parent");
        let staging_prefix = format!(
            ".{}{}",
            build_root
                .file_name()
                .and_then(|name| name.to_str())
                .expect("build root name"),
            ".staging-"
        );
        let leftovers: Vec<_> = fs::read_dir(parent)
            .expect("read build parent")
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| name.starts_with(&staging_prefix))
            .collect();
        assert!(
            leftovers.is_empty(),
            "failed builds must not leave staging roots behind: {leftovers:?}"
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
        let css = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../local-apps/templates/vite-react-static-v1/app/globals.css"
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
        for selector in [
            "[data-platform=\"ios:iphone\"]",
            "[data-platform=\"android:phone\"]",
            "[data-platform=\"ios:ipad\"]",
            "[data-platform=\"android:tablet\"]",
        ] {
            assert!(
                css.contains(selector),
                "CSS missing platform selector {selector}"
            );
        }
        assert!(css.contains("var(--safe-area-top"));
        assert!(css.contains("prefers-reduced-motion"));
    }
}

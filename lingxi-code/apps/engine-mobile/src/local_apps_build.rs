//! The local-app BUILD CORE: template scaffolding plus the fixed offline
//! Vite build, extracted from the generation pipeline so that
//! scaffold/build no longer belongs to the LLM-generation executor.

use crate::local_apps_host::LocalAppsHostBroker;
use local_apps::{AppDataStore, AppError, AppLayout, AppManifest};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
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
const LOCAL_APP_BUILD_GUEST_ROOT: &str = traits::mobile_linux::guest_paths::LOCAL_APP_BUILD_ROOT;
const SHARED_VITE_EXECUTABLE: &str = "/opt/lingxi/local-app-runtime/node_modules/vite/bin/vite.js";
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
        "app/main.jsx",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../local-apps/templates/vite-react-static-v1/app/main.jsx"
        )),
    ),
];

const SOURCE_FILES: &[(&str, &[u8])] = &[
    (
        "app/globals.css",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../local-apps/templates/vite-react-static-v1/app/globals.css"
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
/// v3 hands the source root to the official `create-vite` CLI and lets the
/// Dependencies phase run `npm install`, so `package.json`,
/// `package-lock.json`, `index.html`, `vite.config.*` and the entry module are
/// now legitimately app-owned and CANNOT be re-pinned. What stays host-owned
/// is exactly what `.lingxi/source-policy.json` calls host-managed AND the
/// host can reproduce byte-for-byte: the bridge (the app's only door to host
/// data) and the policy file itself.
fn repinned_host_managed_files(_target: LocalAppBuildTarget) -> &'static [&'static str] {
    &[".lingxi/source-policy.json", "lib/lingxi-bridge.js"]
}

/// Rewrite every host-managed file from its compiled-in template unless it
/// already matches byte for byte. Called at the top of each build — the
/// enforcement point behind the workspace contract the prompts describe.
///
/// `.lingxi/source-policy.json` DECLARES `host_managed_paths`, but nothing
/// reads that declaration: the generate/repair agent holds ordinary
/// Write/Edit on the workspace, so the contract lives in prompts, and a prompt
/// is not an enforcement point. Restoring here means a redefined
/// `lib/lingxi-bridge.js` can never reach a built app — the build silently
/// un-does the edit instead of shipping a bridge the model rewrote under the
/// app's own feet.
pub(crate) fn restore_host_managed_files(
    workspace: &Path,
    target: LocalAppBuildTarget,
) -> Result<(), AppError> {
    let managed = repinned_host_managed_files(target);
    for (relative, bytes) in VITE_LOCKED_FILES
        .iter()
        .filter(|(relative, _)| managed.contains(relative))
    {
        let path = workspace.join(relative);
        let current = std::fs::read(&path).ok();
        if current.as_deref() == Some(*bytes) {
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

fn build_tool_executable(
    _target: LocalAppBuildTarget,
    workspace: &Path,
    app_node_modules_guest: &str,
) -> String {
    let app_local_vite = workspace.join("node_modules/vite/bin/vite.js");
    match std::fs::symlink_metadata(app_local_vite) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            format!("{app_node_modules_guest}/vite/bin/vite.js")
        }
        _ => SHARED_VITE_EXECUTABLE.into(),
    }
}

/// The scaffold/build half of the old generation executor: everything needed
/// to lay down the template workspace and run the fixed offline build, with
/// no dependency on the LLM pipeline.
pub(crate) struct LocalAppBuilder<'a> {
    pub(crate) mobile_linux: Option<Arc<dyn MobileLinuxRuntime>>,
    pub(crate) host: &'a LocalAppsHostBroker,
}

impl LocalAppBuilder<'_> {
    /// The two preconditions a fixed build cannot start without: the mobile
    /// Node runtime and the staged runtime mount. Checked up front by
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
            .fixed_runtime_mount()
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
        let build_guest_path = local_app_build_guest_path(layout.app_id(), build_channel);
        let tool_name = "Vite";
        let mut environment = BTreeMap::new();
        let workspace = layout.root().join(layout.workspace_rel());
        let app_node_modules = workspace.join("node_modules");
        let app_node_modules_guest = format!("{build_guest_path}/node_modules");
        let app_node_modules_available = match std::fs::symlink_metadata(&app_node_modules) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(AppError::InvalidRequest(
                    "workspace node_modules must not be a symlink".into(),
                ));
            }
            Ok(metadata) if metadata.is_dir() => true,
            Ok(_) => {
                return Err(AppError::InvalidRequest(
                    "workspace node_modules must be a directory".into(),
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => {
                return Err(AppError::Io(format!(
                    "inspect workspace node_modules: {error}"
                )));
            }
        };
        let executable = build_tool_executable(
            LocalAppBuildTarget::ViteReactStaticV1,
            &workspace,
            &app_node_modules_guest,
        );
        environment.insert(
            "NODE_PATH".into(),
            if app_node_modules_available {
                format!("{app_node_modules_guest}:/opt/lingxi/local-app-runtime/node_modules")
            } else {
                "/opt/lingxi/local-app-runtime/node_modules".into()
            },
        );
        if app_node_modules_available {
            environment.insert(
                "LINGXI_APP_NODE_MODULES".into(),
                app_node_modules_guest.clone(),
            );
        }
        environment.insert("NODE_ENV".into(), "production".into());
        let build_memory_mb = build_memory_budget_mb(self.host.physical_memory_bytes());
        let resource_limits = ResourceLimits {
            max_memory_mb: Some(build_memory_mb),
            ..ResourceLimits::default()
        };
        let mut mounts = vec![MountSpec {
            host_path: build_root.to_path_buf(),
            guest_path: build_guest_path.clone(),
            read_only: false,
            purpose: MountPurpose::LocalAppBuild,
        }];
        if app_node_modules_available {
            mounts.push(MountSpec {
                host_path: app_node_modules,
                guest_path: app_node_modules_guest,
                read_only: true,
                purpose: MountPurpose::External,
            });
        }
        mounts.push(
            self.host
                .fixed_runtime_mount()
                .map_err(AppError::NotYetAvailable)?,
        );
        let request = LinuxCommandRequest {
            command: "/usr/bin/node".into(),
            args: vec![
                format!(
                    "--max-old-space-size={}",
                    node_old_space_mb(build_memory_mb)
                ),
                executable,
                "build".into(),
            ],
            cwd: Some(build_guest_path.clone()),
            env: environment,
            stdin: None,
            timeout_ms: Some(BUILD_TIMEOUT_MS),
            network: NetworkPolicy::Disabled,
            resource_limits,
            mounts,
        };
        let result = runtime
            .run(request)
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

    /// Prepare the repository-verified Vite project used only when the
    /// official `create-vite` command cannot run because the guest has no
    /// network. The normal path never copies these files into the source
    /// root: the workflow runs the official CLI first through Shell.
    pub(crate) async fn prepare_offline_vite_fallback(
        &self,
        layout: &AppLayout,
    ) -> Result<(), AppError> {
        layout.initialize()?;
        let workspace = layout.root().join(layout.workspace_rel());
        let fallback_root = workspace.join(".lingxi").join("vite-fallback");
        for (relative, bytes) in VITE_LOCKED_FILES {
            if *relative == ".lingxi/source-policy.json" {
                write_file(&workspace, relative, bytes, true)?;
            } else {
                write_file(&fallback_root, relative, bytes, true)?;
            }
        }
        for (relative, bytes) in SOURCE_FILES {
            write_file(&fallback_root, relative, bytes, true)?;
        }
        Ok(())
    }

    /// Copy the workspace into the build root and run the fixed offline
    /// Vite build.
    pub(crate) async fn build_workspace(&self, layout: &AppLayout) -> Result<(), AppError> {
        let build_lock = self.host.build_lock(layout.app_id()).await;
        let _build_guard = build_lock.lock().await;
        // The in-memory guard coalesces builds within this engine. The
        // storage lock extends the same exclusion across engine processes and
        // the native delete path, which otherwise could rename the app while
        // promotion is between its two directory renames.
        let _process_build_guard =
            local_apps::storage::lock_app_build(layout.root(), layout.app_id())?;
        let workspace = layout.root().join(layout.workspace_rel());
        // Availability FIRST. The build is staged separately and promoted
        // only after Vite succeeds, but these gates still avoid creating
        // throwaway staging trees for a runtime that cannot build.
        self.assert_build_runtime_available()?;
        let target = detect_build_target(layout)?;
        // Re-pin the host-managed files from the compiled-in templates on
        // EVERY build, before anything is copied into the build root.
        restore_host_managed_files(&workspace, target)?;
        let build_root = layout.root().join(layout.build_rel(false));
        recover_build_promotion(&build_root)?;
        let staging_root = build_staging_root(&build_root)?;
        let build_result = async {
            replace_build_source(&workspace, &staging_root)?;
            self.run_vite_build(layout, &staging_root).await?;
            validate_build_output(&staging_root)?;
            promote_build_root(&staging_root, &build_root)
        }
        .await;
        if build_result.is_err() {
            let _ = std::fs::remove_dir_all(&staging_root);
        }
        build_result
    }
}

fn local_app_build_guest_path(app_id: &str, channel: &str) -> String {
    format!("{LOCAL_APP_BUILD_GUEST_ROOT}/{app_id}/{channel}")
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
    let index = staging_root.join("out/index.html");
    if !index.is_file() {
        return Err(AppError::Io(format!(
            "fixed Vite build produced no out/index.html in {}",
            staging_root.display()
        )));
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
    let path = root.join(relative);
    if !overwrite && path.exists() {
        return Ok(());
    }
    let parent = path
        .parent()
        .ok_or_else(|| AppError::Io(format!("template path {relative} has no parent")))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| AppError::Io(format!("create template directory: {error}")))?;
    std::fs::write(&path, bytes)
        .map_err(|error| AppError::Io(format!("write template file {relative}: {error}")))
}

fn replace_build_source(workspace: &Path, build_root: &Path) -> Result<(), AppError> {
    if build_root.exists() {
        std::fs::remove_dir_all(build_root)
            .map_err(|error| AppError::Io(format!("clear build directory: {error}")))?;
    }
    std::fs::create_dir_all(build_root)
        .map_err(|error| AppError::Io(format!("create build directory: {error}")))?;
    copy_tree(workspace, workspace, build_root)
}

fn copy_tree(workspace: &Path, current: &Path, destination: &Path) -> Result<(), AppError> {
    for entry in std::fs::read_dir(current)
        .map_err(|error| AppError::Io(format!("read generation source: {error}")))?
    {
        let entry = entry.map_err(|error| AppError::Io(format!("read source entry: {error}")))?;
        let path = entry.path();
        if entry.file_name() == "node_modules" || entry.file_name() == ".git" {
            continue;
        }
        let relative = path
            .strip_prefix(workspace)
            .map_err(|_| AppError::InvalidRequest("build source escaped workspace".into()))?;
        if relative
            .components()
            .next()
            .is_some_and(|component| component.as_os_str() == ".git")
        {
            continue;
        }
        let kind = entry
            .file_type()
            .map_err(|error| AppError::Io(format!("inspect source entry: {error}")))?;
        if kind.is_symlink() {
            return Err(AppError::InvalidRequest(format!(
                "source symlink is forbidden: {}",
                relative.display()
            )));
        }
        let target = destination.join(relative);
        if kind.is_dir() {
            std::fs::create_dir_all(&target)
                .map_err(|error| AppError::Io(format!("create build source directory: {error}")))?;
            copy_tree(workspace, &path, destination)?;
        } else if kind.is_file() {
            std::fs::copy(&path, &target)
                .map_err(|error| AppError::Io(format!("copy build source: {error}")))?;
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
    fn build_source_never_copies_workspace_node_modules() {
        let root = tempfile::tempdir().expect("tempdir");
        let workspace = root.path().join("workspace");
        let output = root.path().join("build");
        fs::create_dir_all(workspace.join("node_modules/example")).expect("node_modules");
        fs::write(workspace.join("node_modules/example/package.json"), "{}").expect("package");
        fs::write(workspace.join("app.jsx"), "export default null;").expect("source");

        replace_build_source(&workspace, &output).expect("copy source");
        assert!(output.join("app.jsx").is_file());
        assert!(!output.join("node_modules").exists());
    }

    #[test]
    fn successful_build_promotion_replaces_output_without_deleting_stage_first() {
        let root = tempfile::tempdir().expect("tempdir");
        let build_root = root.path().join("store");
        let staging_root = root.path().join(".store.staging-test");
        fs::create_dir_all(build_root.join("out")).expect("current output");
        fs::write(build_root.join("out/index.html"), "old").expect("old output");
        fs::create_dir_all(staging_root.join("out")).expect("staged output");
        fs::write(staging_root.join("out/index.html"), "new").expect("new output");

        promote_build_root(&staging_root, &build_root).expect("promote output");

        assert_eq!(
            fs::read_to_string(build_root.join("out/index.html")).unwrap(),
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
        fs::create_dir_all(backup.join("out")).expect("backup output");
        fs::write(backup.join("out/index.html"), "last-good").expect("backup html");
        fs::create_dir_all(staging.join("out")).expect("staging output");
        fs::write(staging.join("out/index.html"), "partial").expect("staging html");

        recover_build_promotion(&build_root).expect("recover interrupted promotion");

        assert_eq!(
            fs::read_to_string(build_root.join("out/index.html")).unwrap(),
            "last-good"
        );
        assert!(!backup.exists());
        assert!(!staging.exists());
    }

    #[test]
    fn vite_build_prefers_the_app_local_cli_when_dependencies_are_installed() {
        let root = tempfile::tempdir().expect("tempdir");
        let workspace = root.path().join("workspace");
        let local_vite = workspace.join("node_modules/vite/bin/vite.js");
        fs::create_dir_all(local_vite.parent().expect("Vite bin parent"))
            .expect("create app-local Vite package");
        fs::write(&local_vite, "export {};").expect("write app-local Vite CLI");

        assert_eq!(
            build_tool_executable(
                LocalAppBuildTarget::ViteReactStaticV1,
                &workspace,
                "/var/lingxi/local-app-build/aaaa1111/store/node_modules",
            ),
            "/var/lingxi/local-app-build/aaaa1111/store/node_modules/vite/bin/vite.js"
        );
    }

    #[test]
    fn vite_build_uses_the_verified_shared_cli_without_an_app_local_install() {
        let root = tempfile::tempdir().expect("tempdir");
        let workspace = root.path().join("workspace");
        fs::create_dir_all(&workspace).expect("workspace");

        assert_eq!(
            build_tool_executable(
                LocalAppBuildTarget::ViteReactStaticV1,
                &workspace,
                "/var/lingxi/local-app-build/aaaa1111/store/node_modules",
            ),
            "/opt/lingxi/local-app-runtime/node_modules/vite/bin/vite.js"
        );
    }

    #[tokio::test]
    async fn offline_vite_fallback_is_staged_outside_the_source_root() {
        let root = tempfile::tempdir().expect("tempdir");
        let layout = AppLayout::new(root.path(), "aaaa1111").expect("layout");
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

        builder
            .prepare_offline_vite_fallback(&layout)
            .await
            .expect("prepare fallback");

        let workspace = layout.root().join(layout.workspace_rel());
        assert!(!workspace.join("package.json").exists());
        assert!(!workspace.join("index.html").exists());
        assert!(workspace
            .join(".lingxi/vite-fallback/package.json")
            .is_file());
        assert!(workspace
            .join(".lingxi/vite-fallback/vite.config.mjs")
            .is_file());
        assert!(workspace.join(".lingxi/source-policy.json").is_file());
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
        let served_index = build_root.join("out/index.html");
        fs::create_dir_all(served_index.parent().expect("out dir")).expect("out dir");
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

    /// `.lingxi/source-policy.json` DECLARES the host-managed paths but is
    /// read by nothing, and the generate/repair agent holds ordinary
    /// Write/Edit on the workspace. The build is the enforcement point: a
    /// redefined bridge never reaches a built app. The app-owned files the
    /// official Vite CLI and `npm install` produce must NOT be clobbered.
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

    /// `.lingxi/source-policy.json` DECLARES the host-managed paths but is
    /// read by nothing, and the generate/repair agent holds ordinary
    /// Write/Edit on the workspace. `build_workspace` is the enforcement
    /// point: a redefined bridge never reaches a built app. The app-owned
    /// files the official Vite CLI and `npm install` produce must NOT be
    /// clobbered.
    #[tokio::test]
    async fn the_build_re_pins_the_bridge_without_touching_app_owned_files() {
        let root = tempfile::tempdir().expect("tempdir");
        let layout = AppLayout::new(root.path(), "aaaa1111").expect("layout");
        layout.initialize().expect("initialize");
        let workspace = layout.root().join(layout.workspace_rel());
        let pinned_bridge = VITE_LOCKED_FILES
            .iter()
            .find(|(relative, _)| *relative == "lib/lingxi-bridge.js")
            .map(|(_, bytes)| *bytes)
            .expect("pinned bridge bytes");

        fs::write(workspace.join("vite.config.mjs"), "export default {};").expect("Vite marker");
        fs::create_dir_all(workspace.join("lib")).expect("lib");
        fs::write(
            workspace.join("lib/lingxi-bridge.js"),
            "export const lingxi = { exfiltrate: true };",
        )
        .expect("tampered bridge");
        // Whatever the official CLI + `npm install` produced.
        let app_package_json = br#"{"name":"scaffolded","dependencies":{"lucide-react":"^0.1.0"}}"#;
        fs::write(workspace.join("package.json"), app_package_json).expect("app package.json");
        fs::write(
            workspace.join("index.html"),
            b"<!doctype html><div id=root>",
        )
        .expect("app index.html");

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
            app_package_json,
            "v3 lets the Dependencies phase own package.json"
        );
        assert_eq!(
            fs::read(workspace.join("index.html")).expect("index.html"),
            b"<!doctype html><div id=root>",
            "v3 preserves the official Vite index.html"
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
            vite_config.contains("require.resolve(\"@tailwindcss/vite\", { paths: [appModules] })")
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

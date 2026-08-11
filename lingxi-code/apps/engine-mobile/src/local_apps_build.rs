//! The local-app BUILD CORE: template scaffolding plus the fixed offline
//! Next/Vite builds, extracted from the generation pipeline so that
//! scaffold/build no longer belongs to the LLM-generation executor.

use crate::local_apps_host::LocalAppsHostBroker;
use local_apps::{AppDataStore, AppError, AppLayout, AppManifest};
use std::collections::BTreeMap;
use std::path::Path;
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
const LOCAL_APP_BUILD_GUEST_ROOT: &str = "/var/lingxi/local-app-build";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LocalAppBuildTarget {
    NextStaticV1,
    ViteReactStaticV1,
}

pub(crate) const LOCKED_FILES: &[(&str, &[u8])] = &[
    (
        "package.json",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../local-apps/templates/next-static-v1/package.json"
        )),
    ),
    (
        "package-lock.json",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../local-apps/templates/next-static-v1/package-lock.json"
        )),
    ),
    (
        "next.config.mjs",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../local-apps/templates/next-static-v1/next.config.mjs"
        )),
    ),
    // LOCKED rather than a one-time scaffold file: this is the app's only
    // door to every host capability, so a revision must be able to pick up
    // new helpers. `scaffold_workspace` overwrites LOCKED files on every job
    // kind (the upgrade path an existing app needs), and `build_workspace`
    // re-pins them from these bytes before every build, so a model cannot
    // quietly redefine the bridge under the app's own feet.
    (
        "lib/lingxi-bridge.js",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../local-apps/templates/next-static-v1/lib/lingxi-bridge.js"
        )),
    ),
    (
        "app/layout.jsx",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../local-apps/templates/next-static-v1/app/layout.jsx"
        )),
    ),
    (
        "app/page.jsx",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../local-apps/templates/next-static-v1/app/page.jsx"
        )),
    ),
];

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
        "lib/lingxi-bridge.js",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../local-apps/templates/next-static-v1/lib/lingxi-bridge.js"
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
            "/../../local-apps/templates/next-static-v1/app/globals.css"
        )),
    ),
    (
        "components/AppShell.jsx",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../local-apps/templates/next-static-v1/components/AppShell.jsx"
        )),
    ),
    ("public/.gitkeep", b""),
];

pub(crate) fn detect_build_target(layout: &AppLayout) -> Result<LocalAppBuildTarget, AppError> {
    let workspace = layout.root().join(layout.workspace_rel());
    let has_next = workspace.join("next.config.mjs").is_file();
    let has_vite = workspace.join("vite.config.mjs").is_file();
    match (has_next, has_vite) {
        (true, false) => Ok(LocalAppBuildTarget::NextStaticV1),
        (false, true) | (false, false) => Ok(LocalAppBuildTarget::ViteReactStaticV1),
        (true, true) => Err(AppError::StorageCorrupt(
            "workspace contains both Next and Vite build configurations".into(),
        )),
    }
}

pub(crate) fn locked_files(target: LocalAppBuildTarget) -> &'static [(&'static str, &'static [u8])] {
    match target {
        LocalAppBuildTarget::NextStaticV1 => LOCKED_FILES,
        LocalAppBuildTarget::ViteReactStaticV1 => VITE_LOCKED_FILES,
    }
}

/// Rewrite every locked file from its compiled-in template unless it already
/// matches byte for byte. Called at the top of each build — the enforcement
/// point behind the workspace contract the prompts describe.
pub(crate) fn restore_locked_files(
    workspace: &Path,
    target: LocalAppBuildTarget,
) -> Result<(), AppError> {
    for (relative, bytes) in locked_files(target) {
        let path = workspace.join(relative);
        let current = std::fs::read(&path).ok();
        if current.as_deref() == Some(*bytes) {
            continue;
        }
        tracing::warn!(
            file = %relative,
            "locked workspace file diverged from its pinned template; restoring before the build"
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
    /// destructive `replace_build_source` step, and re-checked inside
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

    async fn run_fixed_build(
        &self,
        layout: &AppLayout,
        target: LocalAppBuildTarget,
        full: bool,
    ) -> Result<(), AppError> {
        let runtime = self.mobile_linux.as_ref().ok_or_else(|| {
            AppError::NotYetAvailable(
                "the verified mobile Node runtime is unavailable in this build".into(),
            )
        })?;
        let build_root = layout.root().join(layout.build_rel(full));
        let build_channel = if full { "full" } else { "store" };
        let build_guest_path = local_app_build_guest_path(layout.app_id(), build_channel);
        let (tool_name, executable, mut environment) = match target {
            LocalAppBuildTarget::NextStaticV1 => (
                "Next",
                "/opt/lingxi/local-app-runtime/node_modules/next/dist/bin/next",
                BTreeMap::from([(
                    "LINGXI_APP_OUTPUT".into(),
                    if full { "server" } else { "export" }.into(),
                )]),
            ),
            LocalAppBuildTarget::ViteReactStaticV1 => (
                "Vite",
                "/opt/lingxi/local-app-runtime/node_modules/vite/bin/vite.js",
                BTreeMap::new(),
            ),
        };
        environment.insert(
            "NODE_PATH".into(),
            "/opt/lingxi/local-app-runtime/node_modules".into(),
        );
        environment.insert("NODE_ENV".into(), "production".into());
        let build_memory_mb = build_memory_budget_mb(self.host.physical_memory_bytes());
        let resource_limits = ResourceLimits {
            max_memory_mb: Some(build_memory_mb),
            ..ResourceLimits::default()
        };
        let request = LinuxCommandRequest {
            command: "/usr/bin/node".into(),
            args: vec![
                format!(
                    "--max-old-space-size={}",
                    node_old_space_mb(build_memory_mb)
                ),
                executable.into(),
                "build".into(),
            ],
            cwd: Some(build_guest_path.clone()),
            env: environment,
            stdin: None,
            timeout_ms: Some(BUILD_TIMEOUT_MS),
            network: NetworkPolicy::Disabled,
            resource_limits,
            mounts: vec![
                MountSpec {
                    host_path: build_root,
                    guest_path: build_guest_path,
                    read_only: false,
                    purpose: MountPurpose::LocalAppBuild,
                },
                self.host
                    .fixed_runtime_mount()
                    .map_err(AppError::NotYetAvailable)?,
            ],
        };
        let result = runtime
            .run(request)
            .await
            .map_err(|error| AppError::Io(format!("fixed {tool_name} build failed: {error}")))?;
        result
            .enforcement
            .ensure_for(NetworkPolicy::Disabled, resource_limits)
            .map_err(|error| AppError::Io(error.to_string()))?;
        append_build_log(layout, full, &result.stdout, &result.stderr).await?;
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

    pub(crate) async fn run_next_build(
        &self,
        layout: &AppLayout,
        full: bool,
    ) -> Result<(), AppError> {
        self.run_fixed_build(layout, LocalAppBuildTarget::NextStaticV1, full)
            .await
    }

    pub(crate) async fn run_vite_build(&self, layout: &AppLayout) -> Result<(), AppError> {
        self.run_fixed_build(layout, LocalAppBuildTarget::ViteReactStaticV1, false)
            .await
    }

    /// Lay down the template workspace: initialize the layout, write the
    /// LOCKED files (always overwritten) and the SOURCE files (overwritten
    /// only when `overwrite_sources` is true — i.e. an initial generation).
    pub(crate) async fn scaffold_workspace(
        &self,
        layout: &AppLayout,
        overwrite_sources: bool,
    ) -> Result<(), AppError> {
        layout.initialize()?;
        let target = detect_build_target(layout)?;
        let workspace = layout.root().join(layout.workspace_rel());
        for (relative, bytes) in locked_files(target) {
            write_file(&workspace, relative, bytes, true)?;
        }
        for (relative, bytes) in SOURCE_FILES {
            write_file(&workspace, relative, bytes, overwrite_sources)?;
        }
        Ok(())
    }

    /// Copy the workspace into the build root(s) and run the fixed offline
    /// build for the detected target.
    pub(crate) async fn build_workspace(&self, layout: &AppLayout) -> Result<(), AppError> {
        let workspace = layout.root().join(layout.workspace_rel());
        // Availability FIRST. `replace_build_source` below deletes the build
        // root — which holds the `out/` directory the preview server is
        // serving right now — so running it before these gates would destroy
        // a working app for a build that could never have started (runtime
        // bundle still downloading, or evicted).
        self.assert_build_runtime_available()?;
        let target = detect_build_target(layout)?;
        // Re-pin the locked files from the compiled-in templates on EVERY
        // build. The agent-driven v3 flow has no source validator (the
        // generation pipeline that enforced the writable roots was deleted),
        // so the workspace contract lives in prompts — and a prompt is not an
        // enforcement point. Restoring here means an edited `lingxi-bridge.js`
        // (the app's only door to host data), `vite.config.mjs` (whose
        // `build.outDir` is the only reason output lands where the preview
        // server looks), `package.json`, `index.html` or `app/main.jsx` can
        // never reach a built app: the build silently un-does the edit
        // instead of shipping a redefined bridge.
        restore_locked_files(&workspace, target)?;
        match target {
            LocalAppBuildTarget::ViteReactStaticV1 => {
                let build_root = layout.root().join(layout.build_rel(false));
                replace_build_source(&workspace, &build_root)?;
                self.run_vite_build(layout).await?;
            }
            LocalAppBuildTarget::NextStaticV1 => {
                let build_modes: &[bool] = if self.host.full_runtime_enabled() {
                    &[false, true]
                } else {
                    &[false]
                };
                for &full in build_modes {
                    let build_root = layout.root().join(layout.build_rel(full));
                    replace_build_source(&workspace, &build_root)?;
                    self.run_next_build(layout, full).await?;
                }
            }
        }
        Ok(())
    }
}

fn local_app_build_guest_path(app_id: &str, channel: &str) -> String {
    format!("{LOCAL_APP_BUILD_GUEST_ROOT}/{app_id}/{channel}")
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

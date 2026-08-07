//! Concrete mobile executor for the fixed local-app generation pipeline.

use crate::local_apps_host::LocalAppsHostBroker;
use async_trait::async_trait;
use client_adapter::ClientEventSink;
use client_protocol::events::ClientEvent;
use client_protocol::local_apps::{AppEventDto, AppGenerationJobDto, AppGenerationJobStateDto};
use local_apps::{
    load_manifest, save_manifest, AppDataStore, AppError, AppGenerationExecutor, AppLayout,
    AppManifest, AppService, DataCollectionSchema, DesignValue, GenerationJob,
    GenerationJobObserver, GenerationJobStatus, GenerationRequest, GenerationRequestKind,
    WorkspaceSourcePolicy,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::AsyncWriteExt;
use traits::{LinuxCommandRequest, MobileLinuxRuntime, MountPurpose, MountSpec, NetworkPolicy};

const BUILD_TIMEOUT_MS: u64 = 180_000;
const LOCAL_APP_BUILD_GUEST_ROOT: &str = "/var/lingxi/local-app-build";

const LOCKED_FILES: &[(&str, &[u8])] = &[
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
];

const SOURCE_FILES: &[(&str, &[u8])] = &[
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
    (
        "lib/lingxi-bridge.js",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../local-apps/templates/next-static-v1/lib/lingxi-bridge.js"
        )),
    ),
    ("public/.gitkeep", b""),
];

pub(crate) struct MobileAppGenerationExecutor {
    mobile_linux: Option<Arc<dyn MobileLinuxRuntime>>,
    host: Arc<LocalAppsHostBroker>,
    service: OnceLock<Arc<AppService>>,
}

impl MobileAppGenerationExecutor {
    pub(crate) fn new(
        mobile_linux: Option<Arc<dyn MobileLinuxRuntime>>,
        host: Arc<LocalAppsHostBroker>,
    ) -> Arc<Self> {
        Arc::new(Self {
            mobile_linux,
            host,
            service: OnceLock::new(),
        })
    }

    pub(crate) fn attach_service(&self, service: Arc<AppService>) -> Result<(), Arc<AppService>> {
        self.service.set(service)
    }

    fn service(&self) -> Result<Arc<AppService>, AppError> {
        self.service
            .get()
            .cloned()
            .ok_or_else(|| AppError::Io("generation service is not attached".into()))
    }

    async fn reconcile_manifest(
        &self,
        request: &GenerationRequest,
        layout: &AppLayout,
    ) -> Result<(), AppError> {
        let service = self.service()?;
        let record = service.record(&request.key.app_id).await?;
        let draft = service.draft(&request.key.app_id).await?;
        let mut manifest = load_manifest(layout)?;
        manifest.name = record.name;
        manifest.revision = request.key.revision;
        if let Some(DesignValue::DataFieldList(fields)) = draft.fields.get("collection_fields") {
            if manifest.collections.is_empty() {
                // TODO(local-apps#questionnaire, Task 9): the per-template
                // collection id ("records"/"items"/"entries"/"submissions")
                // this used to pick lost its input — `AppRecord` no longer
                // carries a template — and `AppManifest::for_new_app` now
                // starts every app with zero collections regardless (Task 2).
                // Task 9 replaces this whole reconciliation with the real
                // `AppPlan.collections` the LLM plan step authors (validated
                // by `questionnaire::validate_plan`); until then a single
                // fixed id keeps this path a legal, non-fabricated manifest.
                manifest.collections.push(DataCollectionSchema {
                    id: "records".into(),
                    name: "App Data".into(),
                    fields: fields.clone(),
                });
            } else if let Some(collection) = manifest.collections.first_mut() {
                collection.fields = fields.clone();
            }
        }
        if let Some(DesignValue::DomainList(domains)) = draft.fields.get("network_domains") {
            manifest.allowed_domains = domains.clone();
        }
        manifest.validate()?;
        migrate_manifest_with_approval(&self.host, layout, &manifest).await?;
        save_manifest(layout, &manifest)
    }

    async fn run_next_build(&self, layout: &AppLayout, full: bool) -> Result<(), AppError> {
        let runtime = self.mobile_linux.as_ref().ok_or_else(|| {
            AppError::NotYetAvailable(
                "the verified mobile Node runtime is unavailable in this build".into(),
            )
        })?;
        let build_root = layout.root().join(layout.build_rel(full));
        let build_channel = if full { "full" } else { "store" };
        let build_guest_path = local_app_build_guest_path(layout.app_id(), build_channel);
        let request = LinuxCommandRequest {
            command: "/usr/bin/node".into(),
            args: vec![
                "/opt/lingxi/local-app-runtime/node_modules/next/dist/bin/next".into(),
                "build".into(),
            ],
            cwd: Some(build_guest_path.clone()),
            env: [
                (
                    "LINGXI_APP_OUTPUT".into(),
                    if full { "server" } else { "export" }.into(),
                ),
                (
                    "NODE_PATH".into(),
                    "/opt/lingxi/local-app-runtime/node_modules".into(),
                ),
            ]
            .into_iter()
            .collect(),
            stdin: None,
            timeout_ms: Some(BUILD_TIMEOUT_MS),
            // See local_apps_host.rs: the shipped mobile runtimes accept only
            // `Allowed` and reject the request outright otherwise.
            network: NetworkPolicy::Allowed,
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
            .map_err(|error| AppError::Io(format!("fixed Next build failed: {error}")))?;
        append_build_log(layout, full, &result.stdout, &result.stderr).await?;
        if result.timed_out || result.cancelled || result.exit_code != 0 {
            return Err(AppError::Io(format!(
                "fixed Next build exited {} (timed_out={}, cancelled={}): {}",
                result.exit_code,
                result.timed_out,
                result.cancelled,
                bounded_log(&result.stderr)
            )));
        }
        Ok(())
    }
}

fn local_app_build_guest_path(app_id: &str, channel: &str) -> String {
    format!("{LOCAL_APP_BUILD_GUEST_ROOT}/{app_id}/{channel}")
}

async fn migrate_manifest_with_approval(
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

#[async_trait]
impl AppGenerationExecutor for MobileAppGenerationExecutor {
    async fn prepare_scaffold(
        &self,
        request: &GenerationRequest,
        layout: &AppLayout,
    ) -> Result<(), AppError> {
        layout.initialize()?;
        let workspace = layout.root().join(layout.workspace_rel());
        for (relative, bytes) in LOCKED_FILES {
            write_file(&workspace, relative, bytes, true)?;
        }
        for (relative, bytes) in SOURCE_FILES {
            let overwrite = request.kind == GenerationRequestKind::Initial;
            write_file(&workspace, relative, bytes, overwrite)?;
        }
        self.reconcile_manifest(request, layout).await
    }

    async fn generate_source(
        &self,
        request: &GenerationRequest,
        _layout: &AppLayout,
    ) -> Result<(), AppError> {
        if request.kind == GenerationRequestKind::Restore {
            return Ok(());
        }
        // TODO(local-apps#questionnaire, Task 9): this used to render
        // `components/AppShell.jsx` from a fixed per-`AppTemplateKind`
        // scaffold (`render_app_shell_source` / `APP_SHELL_TEMPLATE` /
        // `default_collection_fields`, deleted in Task 2 along with the core
        // `AppTemplateKind` they switched on). Task 9 replaces this step with
        // the real LLM source-writing call. Failing loudly here — instead of
        // either a silent no-op (a `generating` app that never leaves that
        // state) or a fabricated scaffold — makes the gap self-describing:
        // any generation job now fails typed at this stage until Task 9
        // lands.
        Err(AppError::NotYetAvailable(
            "local-app source generation is not yet wired to the LLM (Task 9 replaces the \
             deleted template-driven AppShell scaffold with a real generation call)"
                .into(),
        ))
    }

    async fn source_policy(
        &self,
        _request: &GenerationRequest,
        _layout: &AppLayout,
    ) -> Result<WorkspaceSourcePolicy, AppError> {
        let locked_files = LOCKED_FILES
            .iter()
            .map(|(relative, bytes)| {
                (
                    PathBuf::from(relative),
                    format!("{:x}", Sha256::digest(bytes)),
                )
            })
            .collect::<BTreeMap<_, _>>();
        Ok(WorkspaceSourcePolicy { locked_files })
    }

    async fn validate_source(
        &self,
        _request: &GenerationRequest,
        _layout: &AppLayout,
    ) -> Result<(), AppError> {
        Ok(())
    }

    async fn build(
        &self,
        _request: &GenerationRequest,
        layout: &AppLayout,
    ) -> Result<(), AppError> {
        let workspace = layout.root().join(layout.workspace_rel());
        let build_modes: &[bool] = if self.host.full_runtime_enabled() {
            // Full still proves static-export compatibility before producing
            // the server build used at runtime.
            &[false, true]
        } else {
            &[false]
        };
        for &full in build_modes {
            let build_root = layout.root().join(layout.build_rel(full));
            replace_build_source(&workspace, &build_root)?;
            self.run_next_build(layout, full).await?;
        }
        Ok(())
    }

    async fn start_preview(
        &self,
        request: &GenerationRequest,
        _layout: &AppLayout,
    ) -> Result<Option<String>, AppError> {
        let value = self
            .host
            .manage_runtime_value(serde_json::json!({
                "app_id": request.key.app_id,
                "action": "start",
            }))
            .await
            .map_err(AppError::Io)?;
        Ok(value
            .get("url")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string))
    }
}

pub(crate) struct ClientGenerationJobObserver {
    root: PathBuf,
    sink: Arc<dyn ClientEventSink>,
}

impl ClientGenerationJobObserver {
    pub(crate) fn new(root: PathBuf, sink: Arc<dyn ClientEventSink>) -> Arc<Self> {
        Arc::new(Self { root, sink })
    }
}

#[async_trait]
impl GenerationJobObserver for ClientGenerationJobObserver {
    async fn on_job_changed(&self, job: GenerationJob) {
        if let Err(error) = append_generation_log(&self.root, &job).await {
            tracing::warn!(app_id = %job.key.app_id, %error, "failed to append local-app generation log");
        }
        self.sink
            .emit(ClientEvent::AppEvent {
                event: AppEventDto::AppGenerationJobChanged {
                    job: lower_job(job),
                },
            })
            .await;
    }
}

async fn append_generation_log(root: &Path, job: &GenerationJob) -> Result<(), AppError> {
    let layout = AppLayout::new(root, &job.key.app_id)?;
    let log_dir = layout.root().join(layout.logs_rel());
    tokio::fs::create_dir_all(&log_dir)
        .await
        .map_err(|error| AppError::Io(format!("create generation log directory: {error}")))?;
    let path = log_dir.join("generation.log");
    if tokio::fs::metadata(&path)
        .await
        .is_ok_and(|metadata| metadata.len() >= 1024 * 1024)
    {
        let _ = tokio::fs::rename(&path, log_dir.join("generation.log.1")).await;
    }
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
        .map_err(|error| AppError::Io(format!("open generation log: {error}")))?;
    let line = serde_json::to_string(&serde_json::json!({
        "updatedAtMs": job.updated_at_ms,
        "revision": job.key.revision,
        "continuationSeq": job.key.continuation_seq,
        "attempt": job.attempt,
        "state": job.status,
        "previewUrl": &job.preview_url,
        "error": &job.last_error,
    }))
    .map_err(|error| AppError::Io(format!("serialize generation log: {error}")))?;
    file.write_all(format!("{line}\n").as_bytes())
        .await
        .map_err(|error| AppError::Io(format!("write generation log: {error}")))
}

pub(crate) fn lower_job(job: GenerationJob) -> AppGenerationJobDto {
    let (state, percent) = match job.status {
        GenerationJobStatus::Queued => (AppGenerationJobStateDto::Queued, Some(0)),
        GenerationJobStatus::Scaffolding => (AppGenerationJobStateDto::Scaffolding, Some(5)),
        GenerationJobStatus::Generating => (AppGenerationJobStateDto::Generating, Some(25)),
        GenerationJobStatus::Validating => (AppGenerationJobStateDto::Validating, Some(50)),
        GenerationJobStatus::Building => (AppGenerationJobStateDto::Building, Some(70)),
        GenerationJobStatus::StartingPreview => {
            (AppGenerationJobStateDto::StartingPreview, Some(90))
        }
        GenerationJobStatus::AwaitingPreviewApproval => {
            (AppGenerationJobStateDto::AwaitingApproval, Some(100))
        }
        GenerationJobStatus::Completed => (AppGenerationJobStateDto::Succeeded, Some(100)),
        GenerationJobStatus::Failed | GenerationJobStatus::Retryable => {
            (AppGenerationJobStateDto::Failed, None)
        }
    };
    AppGenerationJobDto {
        id: format!(
            "{}:{}:{}",
            job.key.app_id, job.key.revision, job.key.continuation_seq
        ),
        app_id: job.key.app_id,
        revision: job.key.revision,
        continuation_seq: job.key.continuation_seq,
        state,
        percent,
        detail: job.last_error.or(job.preview_url),
        log_rel: Some("logs/generation.log".into()),
        updated_at_ms: job.updated_at_ms,
    }
}

fn write_file(root: &Path, relative: &str, bytes: &[u8], overwrite: bool) -> Result<(), AppError> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use client_adapter::MockSink;
    use client_protocol::events::ClientEvent;
    use client_protocol::local_apps::{
        AppAuthorizationDecisionDto, AppCapabilityKindDto, AppEventDto,
    };
    use local_apps::load_permissions;
    use std::sync::Mutex as StdMutex;
    use tokio::time::{sleep, Duration};
    use traits::{
        LinuxCommandResult, LinuxProcessHandle, MobileLinuxCapability, MobileLinuxError,
        MobileLinuxRuntimeMode, MobileLinuxTaskSnapshot, MountPurpose, PtyOpenRequest,
        PtySessionHandle, PtySize, RootfsState, RootfsStatus, SandboxBackend,
    };

    /// Captures the one `run` request `run_next_build` issues. Every other
    /// entry point is unreachable from that path and stays `Unsupported`.
    #[derive(Default)]
    struct RecordingMobileLinuxRuntime {
        request: StdMutex<Option<LinuxCommandRequest>>,
    }

    impl RecordingMobileLinuxRuntime {
        fn recorded(&self) -> LinuxCommandRequest {
            self.request
                .lock()
                .expect("recorded request")
                .clone()
                .expect("run was called")
        }

        fn rootfs(&self) -> RootfsStatus {
            RootfsStatus {
                state: RootfsState::Ready,
                backend: self.backend(),
                mode: self.mode(),
                platform: "test".into(),
                abi: "test".into(),
                version: None,
                managed_root: None,
                active_root: None,
                staged_root: None,
                archive_sha256: None,
                installed_size_bytes: None,
                writable_guest_paths: vec![],
                last_error: None,
            }
        }
    }

    #[async_trait]
    impl MobileLinuxRuntime for RecordingMobileLinuxRuntime {
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
                background_processes: true,
                pty: false,
                bind_mounts: true,
                rootfs_integrity: false,
            }
        }

        async fn boot(&self) -> Result<RootfsStatus, MobileLinuxError> {
            Ok(self.rootfs())
        }

        async fn shutdown(&self) -> Result<(), MobileLinuxError> {
            Ok(())
        }

        async fn run(
            &self,
            request: LinuxCommandRequest,
        ) -> Result<LinuxCommandResult, MobileLinuxError> {
            // The shipped runtimes reject a denied policy before they boot, so
            // record what the caller asked for rather than silently accepting it.
            *self.request.lock().expect("recorded request") = Some(request);
            Ok(LinuxCommandResult {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: 0,
                timed_out: false,
                cancelled: false,
            })
        }

        async fn spawn_background(
            &self,
            _request: LinuxCommandRequest,
        ) -> Result<LinuxProcessHandle, MobileLinuxError> {
            Err(MobileLinuxError::Unsupported)
        }

        async fn kill(&self, _handle: &LinuxProcessHandle) -> Result<(), MobileLinuxError> {
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
            Ok(self.rootfs())
        }

        async fn verify_rootfs(&self) -> Result<RootfsStatus, MobileLinuxError> {
            Ok(self.rootfs())
        }

        async fn repair_rootfs(&self) -> Result<RootfsStatus, MobileLinuxError> {
            Ok(self.rootfs())
        }

        async fn reset_rootfs(&self) -> Result<RootfsStatus, MobileLinuxError> {
            Ok(self.rootfs())
        }

        async fn configure_mounts(&self, _mounts: Vec<MountSpec>) -> Result<(), MobileLinuxError> {
            Ok(())
        }

        async fn list_tasks(&self) -> Result<Vec<MobileLinuxTaskSnapshot>, MobileLinuxError> {
            Ok(Vec::new())
        }

        async fn task_status(
            &self,
            _task_id: &str,
        ) -> Result<Option<MobileLinuxTaskSnapshot>, MobileLinuxError> {
            Ok(None)
        }
    }

    #[tokio::test]
    async fn next_build_request_uses_a_policy_the_mobile_runtimes_accept() {
        let root = tempfile::tempdir().unwrap();
        let runtime_root = root.path().join("runtime-root");
        let next_bin = runtime_root.join("node_modules/next/dist/bin/next");
        std::fs::create_dir_all(next_bin.parent().unwrap()).unwrap();
        std::fs::write(&next_bin, b"#!/bin/sh\n").unwrap();
        let host = LocalAppsHostBroker::new(
            root.path().to_path_buf(),
            MockSink::arc(),
            None,
            true,
            Some(runtime_root),
        );
        let runtime = Arc::new(RecordingMobileLinuxRuntime::default());
        let executor = MobileAppGenerationExecutor::new(Some(runtime.clone()), host);
        let layout = AppLayout::new(root.path(), "abcd1234").unwrap();

        executor.run_next_build(&layout, false).await.unwrap();

        assert!(matches!(
            runtime.recorded().network,
            traits::NetworkPolicy::Allowed
        ));
        assert_eq!(
            runtime.recorded().cwd.as_deref(),
            Some("/var/lingxi/local-app-build/abcd1234/store")
        );
        assert!(matches!(
            runtime
                .recorded()
                .mounts
                .first()
                .map(|mount| &mount.purpose),
            Some(MountPurpose::LocalAppBuild)
        ));
        assert_eq!(
            runtime
                .recorded()
                .mounts
                .first()
                .map(|mount| mount.guest_path.as_str()),
            Some("/var/lingxi/local-app-build/abcd1234/store")
        );
    }

    #[test]
    fn bundled_policy_locks_dependency_files() {
        let policy = LOCKED_FILES
            .iter()
            .map(|(path, bytes)| (PathBuf::from(path), format!("{:x}", Sha256::digest(bytes))))
            .collect::<BTreeMap<_, _>>();
        assert!(policy.contains_key(Path::new("package.json")));
        assert!(policy.contains_key(Path::new("package-lock.json")));
    }

    // The five tests that used to live here (`*_does_not_corrupt_the_shell`,
    // `generated_{dashboard,crud,content,form}_source_uses_*`) exercised
    // `render_app_shell_source` / `default_collection_fields` directly — the
    // per-`AppTemplateKind` scaffold renderer deleted in Task 2 (fix-forward
    // for the engine-mobile build) alongside the core `AppTemplateKind` it
    // switched on. There is no smaller-scope replacement to assert against:
    // Task 9 owns writing the real (LLM-driven) equivalent and its tests.
    // `generate_source_reports_not_yet_available_until_task_9_wires_the_llm`
    // below covers the NEW behavior at the level that still exists —
    // `generate_source` failing typed — so app creation exercised by other
    // tests fails loudly instead of silently wedging.

    /// `generate_source` (the `AppGenerationExecutor` step that used to
    /// render `components/AppShell.jsx` from the deleted template scaffold)
    /// now fails typed `NotYetAvailable` for every non-restore job, since
    /// Task 9 has not wired the real LLM source-writing call yet.
    #[tokio::test]
    async fn generate_source_reports_not_yet_available_until_task_9_wires_the_llm() {
        let root = tempfile::tempdir().unwrap();
        let service = Arc::new(
            AppService::load(
                root.path(),
                Arc::new(local_apps::test_support::FixedClock::new(1)),
                Arc::new(local_apps::NoopContinuationSink),
                Arc::new(local_apps::NoopAppEventObserver),
            )
            .await
            .expect("load service"),
        );
        let record = service
            .create_app(Some("Habits"), "a habit tracker", None)
            .await
            .expect("create app");
        let record = local_apps::test_support::advance_to_collecting_spec(&service, &record.id)
            .await;
        let interaction = service
            .open_designer(&record.id)
            .await
            .expect("open designer");
        local_apps::test_support::stamp_fresh_plan(&service, &record.id).await;
        service
            .confirm_design(&record.id, &interaction.interaction_id, 0)
            .await
            .expect("confirm design");

        let host =
            LocalAppsHostBroker::new(root.path().to_path_buf(), MockSink::arc(), None, false, None);
        let executor = MobileAppGenerationExecutor::new(None, host);
        executor
            .attach_service(service.clone())
            .map_err(|_| "service already attached")
            .unwrap();
        let layout = AppLayout::new(root.path().to_path_buf(), record.id.clone()).unwrap();
        let key = local_apps::GenerationJobKey {
            app_id: record.id.clone(),
            revision: 0,
            continuation_seq: 1,
        };
        let request = GenerationRequest {
            key: key.clone(),
            kind: GenerationRequestKind::Initial,
            prompt: None,
        };
        let error = executor
            .generate_source(&request, &layout)
            .await
            .expect_err("generate_source must fail until Task 9 wires the LLM");
        assert_eq!(error.code(), local_apps::AppErrorCode::NotYetAvailable);
        assert!(
            format!("{error}").contains("Task 9"),
            "the failure must name the task that fills the gap: {error}"
        );

        // A restore job is NOT source generation — it must stay a no-op.
        let restore_request = GenerationRequest {
            key,
            kind: GenerationRequestKind::Restore,
            prompt: None,
        };
        executor
            .generate_source(&restore_request, &layout)
            .await
            .expect("a restore job must not hit the not-yet-available gate");
    }

    #[tokio::test]
    async fn destructive_manifest_migration_uses_one_shot_approval_without_persisting_grant() {
        let root = tempfile::tempdir().unwrap();
        let layout = AppLayout::new(root.path(), "abcd1234").unwrap();
        let mut store = AppDataStore::open(layout.clone()).unwrap();
        let old_manifest = manifest_with_score();
        store.migrate_manifest(&old_manifest, false, 1).unwrap();
        drop(store);

        let sink = MockSink::arc();
        let host =
            LocalAppsHostBroker::new(root.path().to_path_buf(), sink.clone(), None, false, None);
        let approver = tokio::spawn(spawn_capability_resolution(
            sink.clone(),
            host.clone(),
            AppAuthorizationDecisionDto::AllowAlways,
            None,
        ));

        migrate_manifest_with_approval(host.as_ref(), &layout, &manifest_without_score())
            .await
            .unwrap();
        approver.await.unwrap();

        let events = sink.events().await;
        assert_eq!(events.len(), 1);
        let request = capability_request(&events[0]).unwrap();
        assert_eq!(request.capability, AppCapabilityKindDto::DataMutation);
        assert!(request.reason.contains("exact migration attempt only"));
        assert!(request.reason.contains("score"));

        let preview = AppDataStore::open(layout.clone())
            .unwrap()
            .preview_migration(&manifest_without_score())
            .unwrap();
        assert!(!preview.destructive);
        assert_eq!(
            load_permissions(&layout)
                .unwrap()
                .always_allowed_capabilities
                .len(),
            0
        );
    }

    #[tokio::test]
    async fn destructive_manifest_migration_denial_leaves_database_unchanged() {
        let root = tempfile::tempdir().unwrap();
        let layout = AppLayout::new(root.path(), "abcd1234").unwrap();
        let mut store = AppDataStore::open(layout.clone()).unwrap();
        let old_manifest = manifest_with_score();
        store.migrate_manifest(&old_manifest, false, 1).unwrap();
        drop(store);

        let sink = MockSink::arc();
        let host =
            LocalAppsHostBroker::new(root.path().to_path_buf(), sink.clone(), None, false, None);
        let approver = tokio::spawn(spawn_capability_resolution(
            sink.clone(),
            host.clone(),
            AppAuthorizationDecisionDto::Deny,
            None,
        ));

        let error =
            migrate_manifest_with_approval(host.as_ref(), &layout, &manifest_without_score())
                .await
                .unwrap_err();
        approver.await.unwrap();

        assert!(
            matches!(error, AppError::InvalidRequest(message) if message == "user denied destructive manifest migration")
        );
        let preview = AppDataStore::open(layout.clone())
            .unwrap()
            .preview_migration(&manifest_without_score())
            .unwrap();
        assert!(preview.destructive);
        assert_eq!(
            load_permissions(&layout)
                .unwrap()
                .always_allowed_capabilities
                .len(),
            0
        );
    }

    #[tokio::test]
    async fn destructive_manifest_approval_is_bound_to_the_previewed_attempt() {
        let root = tempfile::tempdir().unwrap();
        let layout = AppLayout::new(root.path(), "abcd1234").unwrap();
        let mut store = AppDataStore::open(layout.clone()).unwrap();
        let old_manifest = manifest_with_score();
        store.migrate_manifest(&old_manifest, false, 1).unwrap();
        drop(store);

        let sink = MockSink::arc();
        let host =
            LocalAppsHostBroker::new(root.path().to_path_buf(), sink.clone(), None, false, None);
        let approver = tokio::spawn(spawn_capability_resolution(
            sink.clone(),
            host.clone(),
            AppAuthorizationDecisionDto::AllowOnce,
            Some((layout.clone(), manifest_with_added_note())),
        ));

        let error =
            migrate_manifest_with_approval(host.as_ref(), &layout, &manifest_without_score())
                .await
                .unwrap_err();
        approver.await.unwrap();

        assert!(
            matches!(error, AppError::WorkflowStateInvalid(message) if message.contains("changed while waiting for approval"))
        );
    }

    async fn spawn_capability_resolution(
        sink: Arc<MockSink>,
        host: Arc<LocalAppsHostBroker>,
        decision: AppAuthorizationDecisionDto,
        pre_resolution_migration: Option<(AppLayout, AppManifest)>,
    ) {
        let request = wait_for_capability_request(&sink).await;
        if let Some((layout, manifest)) = pre_resolution_migration {
            let mut store = AppDataStore::open(layout).unwrap();
            store.migrate_manifest(&manifest, false, 2).unwrap();
        }
        assert!(host.resolve_capability(&request.request_id, decision).await);
    }

    async fn wait_for_capability_request(
        sink: &MockSink,
    ) -> client_protocol::local_apps::AppCapabilityRequestDto {
        loop {
            if let Some(request) = sink
                .events()
                .await
                .into_iter()
                .find_map(|event| capability_request(&event).cloned())
            {
                return request;
            }
            sleep(Duration::from_millis(10)).await;
        }
    }

    fn capability_request(
        event: &ClientEvent,
    ) -> Option<&client_protocol::local_apps::AppCapabilityRequestDto> {
        match event {
            ClientEvent::AppEvent {
                event: AppEventDto::AppCapabilityRequested { request },
            } => Some(request),
            _ => None,
        }
    }

    fn manifest_with_score() -> AppManifest {
        AppManifest {
            schema_version: local_apps::APPS_SCHEMA_VERSION,
            app_id: "abcd1234".into(),
            revision: 1,
            name: "Habits".into(),
            collections: vec![local_apps::DataCollectionSchema {
                id: "items".into(),
                name: "Items".into(),
                fields: vec![
                    data_field("title", local_apps::DataFieldKind::Text),
                    data_field("score", local_apps::DataFieldKind::Integer),
                ],
            }],
            allowed_domains: vec![],
        }
    }

    fn manifest_without_score() -> AppManifest {
        let mut manifest = manifest_with_score();
        manifest.revision = 2;
        manifest.collections[0]
            .fields
            .retain(|field| field.id != "score");
        manifest
    }

    fn manifest_with_added_note() -> AppManifest {
        let mut manifest = manifest_with_score();
        manifest.revision = 2;
        manifest.collections[0]
            .fields
            .push(data_field("note", local_apps::DataFieldKind::LongText));
        manifest
    }

    fn data_field(id: &str, kind: local_apps::DataFieldKind) -> local_apps::DataFieldSchema {
        local_apps::DataFieldSchema {
            id: id.into(),
            label: id.to_string(),
            kind,
            required: false,
            enum_options: vec![],
        }
    }
}

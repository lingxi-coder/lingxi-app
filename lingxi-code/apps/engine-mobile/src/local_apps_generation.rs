//! Concrete mobile executor for the fixed local-app generation pipeline.

use crate::local_apps_host::LocalAppsHostBroker;
#[cfg(test)]
use crate::local_apps_llm::LocalAppsLlm;
use crate::local_apps_llm::SourceRequest;
use crate::local_apps_profile::SharedLlm;
use crate::local_apps_sources::{FileWrite, MAX_GENERATED_TOTAL_BYTES};
use async_trait::async_trait;
use client_adapter::ClientEventSink;
use client_protocol::events::ClientEvent;
use client_protocol::local_apps::{AppEventDto, AppGenerationJobDto, AppGenerationJobStateDto};
use local_apps::{
    load_manifest, save_manifest, validate_workspace_source, AppDataStore, AppError,
    AppGenerationExecutor, AppLayout, AppManifest, AppService, GenerationJob,
    GenerationJobObserver, GenerationJobStatus, GenerationRequest, GenerationRequestKind,
    WorkspaceSourcePolicy, WRITABLE_ROOTS,
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
    llm: Arc<SharedLlm>,
}

impl MobileAppGenerationExecutor {
    pub(crate) fn new(
        mobile_linux: Option<Arc<dyn MobileLinuxRuntime>>,
        host: Arc<LocalAppsHostBroker>,
        llm: Arc<SharedLlm>,
    ) -> Arc<Self> {
        Arc::new(Self {
            mobile_linux,
            host,
            service: OnceLock::new(),
            llm,
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
        // The manifest's collections/domains come from the validated
        // `AppPlan` the user confirmed at the designer gate
        // (`questionnaire::validate_plan`), not from raw questionnaire
        // answers — the plan is the LLM's derivation from those answers,
        // and it is the only thing the human actually approved. A missing
        // plan means generation was reached without ever clearing the
        // designer gate, which is a workflow bug, not a legal "no data"
        // app — `generate_source` (a sibling `AppGenerationExecutor`
        // method run in the same pipeline) fails the identical way for the
        // identical reason.
        let plan = draft.plan.ok_or_else(|| {
            AppError::WorkflowStateInvalid(
                "manifest reconciliation requires a confirmed plan".into(),
            )
        })?;
        let mut manifest = load_manifest(layout)?;
        manifest.name = record.name;
        manifest.revision = request.key.revision;
        manifest.collections = plan.collections;
        manifest.allowed_domains = plan.domains;
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
        layout: &AppLayout,
    ) -> Result<(), AppError> {
        // A restore reuses Git-restored source as-is: re-validating and
        // rebuilding it is the whole job. Spending an LLM round trip here
        // would be both wasteful (nothing changed) and wrong (it would ask
        // the model to redo work a human already approved).
        if request.kind == GenerationRequestKind::Restore {
            return Ok(());
        }
        let service = self.service()?;
        let record = service.record(&request.key.app_id).await?;
        let draft = service.draft(&request.key.app_id).await?;
        let plan = draft.plan.clone().ok_or_else(|| {
            AppError::WorkflowStateInvalid("generation requires a confirmed plan".into())
        })?;
        let workspace = layout.root().join(layout.workspace_rel());

        // A revision hands the model the tree it is editing — writes are an
        // overlay (see `write_file(.., true)` below), not a replace-all, so
        // the model needs to see what already exists to know what NOT to
        // resend.
        let (existing, existing_note) = if request.kind == GenerationRequestKind::Revision {
            read_generated_tree(&workspace)?
        } else {
            (Vec::new(), None)
        };

        let mut source_request = SourceRequest {
            brief: record.brief.clone(),
            plan,
            answers: draft.fields.clone(),
            existing,
            existing_note,
            revision_prompt: request.prompt.clone(),
            validator_feedback: None,
        };

        // One initial attempt plus at most two repairs. `validate_workspace_source`'s
        // own error text is fed back as `validator_feedback` so the model sees
        // exactly what it broke — far more useful than asking it to guess again
        // from scratch.
        const MAX_ATTEMPTS: usize = 3;
        let mut last_error = None;
        for attempt in 0..MAX_ATTEMPTS {
            let writes = self.llm.current().generate_sources(&source_request).await?;
            // Overlay write, never a clear-then-write: the model names only
            // the files it wants to create or replace, everything else in the
            // workspace stays untouched. A "move the search box" edit should
            // cost one file, not a full re-emission of the app — and a model
            // that forgets to mention a file must not silently delete it.
            for write in &writes {
                write_file(&workspace, &write.path, write.contents.as_bytes(), true)?;
            }
            let policy = self.source_policy(request, layout).await?;
            match validate_workspace_source(layout, &policy) {
                Ok(()) => return Ok(()),
                Err(error) => {
                    let message = format!("{error}");
                    source_request.validator_feedback = Some(message);
                    last_error = Some(error);
                }
            }
            // The write above landed on disk BEFORE validation ran, so a
            // rejected attempt's bytes are real workspace content by now —
            // for every job kind, not only `Revision`: an `Initial` job's
            // `existing` started empty, but after this failure the
            // workspace no longer matches that empty snapshot. Refresh from
            // disk before the next attempt so the model sees what it
            // actually broke, not a stale pre-attempt tree that contradicts
            // `validator_feedback` (and would read as "nothing to fix").
            // Skipped on the last attempt: no further call will read it.
            if attempt + 1 < MAX_ATTEMPTS {
                let (refreshed, note) = read_generated_tree(&workspace)?;
                source_request.existing = refreshed;
                source_request.existing_note = note;
            }
        }
        Err(last_error.unwrap_or_else(|| {
            AppError::Io("source generation exhausted its repair attempts".into())
        }))
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

/// Walk the five writable roots and collect every current source file, so a
/// revision pass sees what already exists before the model edits it.
/// Bounded by [`MAX_GENERATED_TOTAL_BYTES`]: once the running total would
/// exceed the budget, the (path-sorted) remainder is left out. The second
/// return value, when `Some`, names how many files were omitted — the caller
/// folds it into the model's PROMPT TEXT (`SourceRequest::existing_note`),
/// never a fabricated [`FileWrite`], because a placeholder entry describing
/// the omission would read to the model as a real file that exists in the
/// workspace.
fn read_generated_tree(workspace: &Path) -> Result<(Vec<FileWrite>, Option<String>), AppError> {
    let mut collected: Vec<(String, Vec<u8>)> = Vec::new();
    for root in WRITABLE_ROOTS {
        let root_path = workspace.join(root);
        if root_path.is_dir() {
            collect_generated_files(workspace, &root_path, &mut collected)?;
        }
    }
    collected.sort_by(|a, b| a.0.cmp(&b.0));

    let total_files = collected.len();
    let mut cutoff = total_files;
    let mut running_bytes = 0usize;
    for (index, (_, bytes)) in collected.iter().enumerate() {
        if running_bytes.saturating_add(bytes.len()) > MAX_GENERATED_TOTAL_BYTES {
            cutoff = index;
            break;
        }
        running_bytes += bytes.len();
    }

    let omitted = total_files - cutoff;
    let files = collected
        .into_iter()
        .take(cutoff)
        .map(|(path, bytes)| FileWrite {
            path,
            contents: String::from_utf8_lossy(&bytes).into_owned(),
        })
        .collect();
    let note = (omitted > 0).then(|| {
        format!(
            "现有源码树超过了 {MAX_GENERATED_TOTAL_BYTES} 字节的读取预算，按路径字典序\
             截断，有 {omitted} 个文件的内容未在上面展示。它们仍然存在于工作区里，没有\
             被删除——只是这次没有塞进 prompt，不要假设它们不存在。"
        )
    });
    Ok((files, note))
}

/// Recursive `read_dir` walk collecting `(workspace-relative POSIX path,
/// bytes)` for every regular file under `current`. Mirrors
/// [`validate_workspace_source`]'s own walk (symlinks skipped, not
/// followed) rather than trusting arbitrary workspace content.
fn collect_generated_files(
    workspace: &Path,
    current: &Path,
    out: &mut Vec<(String, Vec<u8>)>,
) -> Result<(), AppError> {
    for entry in std::fs::read_dir(current)
        .map_err(|error| AppError::Io(format!("read generated tree: {error}")))?
    {
        let entry = entry
            .map_err(|error| AppError::Io(format!("read generated tree entry: {error}")))?;
        let path = entry.path();
        let kind = entry
            .file_type()
            .map_err(|error| AppError::Io(format!("inspect generated entry: {error}")))?;
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            collect_generated_files(workspace, &path, out)?;
            continue;
        }
        if !kind.is_file() {
            continue;
        }
        let relative = path
            .strip_prefix(workspace)
            .map_err(|_| AppError::InvalidRequest("generated file escaped workspace".into()))?;
        let bytes = std::fs::read(&path)
            .map_err(|error| AppError::Io(format!("read {}: {error}", path.display())))?;
        out.push((relative.to_string_lossy().replace('\\', "/"), bytes));
    }
    Ok(())
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
    use crate::local_apps_llm::test_support::ScriptedModel;
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
        let llm = Arc::new(SharedLlm::new(Arc::new(LocalAppsLlm::new(ScriptedModel::new(
            Vec::new(),
        )))));
        let executor = MobileAppGenerationExecutor::new(Some(runtime.clone()), host, llm);
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
    // switched on. There is no smaller-scope replacement to assert against.
    // Task 9 replaced the typed `NotYetAvailable` stub that stood in their
    // place with the real LLM-driven `generate_source` — the tests below
    // exercise it via a `GenerationHarness` wrapping a `ScriptedModel`.

    /// Everything one `generate_source` test needs: a loaded `AppService`
    /// with a confirmed plan, a `MobileAppGenerationExecutor` wired to a
    /// `ScriptedModel`, and a scaffolded workspace (`prepare_scaffold` has
    /// already run, matching the real pipeline's call order — without it
    /// `validate_workspace_source`'s locked-file hash check has nothing to
    /// compare against).
    struct GenerationHarness {
        // Kept alive for the harness's lifetime; the workspace lives under it.
        _root: tempfile::TempDir,
        executor: Arc<MobileAppGenerationExecutor>,
        layout: AppLayout,
        model: Arc<ScriptedModel>,
        app_id: String,
    }

    impl GenerationHarness {
        fn initial_request(&self) -> GenerationRequest {
            GenerationRequest {
                key: local_apps::GenerationJobKey {
                    app_id: self.app_id.clone(),
                    revision: 0,
                    continuation_seq: 1,
                },
                kind: GenerationRequestKind::Initial,
                prompt: None,
            }
        }

        async fn read(&self, relative: &str) -> Option<String> {
            let path = self
                .layout
                .root()
                .join(self.layout.workspace_rel())
                .join(relative);
            tokio::fs::read_to_string(path).await.ok()
        }

        /// Write a file directly into the workspace, standing in for content
        /// a previous generation left behind — used to prove a revision's
        /// overlay write leaves files the model did not mention untouched.
        async fn seed(&self, relative: &str, contents: &str) {
            let workspace = self.layout.root().join(self.layout.workspace_rel());
            write_file(&workspace, relative, contents.as_bytes(), true).expect("seed file");
        }

        fn model_calls(&self) -> usize {
            self.model.call_count()
        }

        fn prompt_at(&self, index: usize) -> String {
            self.model.prompt_at(index)
        }
    }

    async fn generation_harness(
        responses: Vec<Result<serde_json::Value, AppError>>,
    ) -> GenerationHarness {
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
        let model = ScriptedModel::new(responses);
        let llm = Arc::new(SharedLlm::new(Arc::new(LocalAppsLlm::new(model.clone()))));
        let executor = MobileAppGenerationExecutor::new(None, host, llm);
        executor
            .attach_service(service.clone())
            .map_err(|_| "service already attached")
            .unwrap();
        let layout = AppLayout::new(root.path().to_path_buf(), record.id.clone()).unwrap();

        let scaffold_request = GenerationRequest {
            key: local_apps::GenerationJobKey {
                app_id: record.id.clone(),
                revision: 0,
                continuation_seq: 1,
            },
            kind: GenerationRequestKind::Initial,
            prompt: None,
        };
        executor
            .prepare_scaffold(&scaffold_request, &layout)
            .await
            .expect("scaffold prepared");

        GenerationHarness {
            _root: root,
            executor,
            layout,
            model,
            app_id: record.id,
        }
    }

    #[tokio::test]
    async fn generate_source_writes_what_the_model_returned() {
        let harness = generation_harness(vec![Ok(serde_json::json!({
            "files": [{"path": "app/page.jsx", "contents": "export default function P(){return <div/>}"}]
        }))])
        .await;

        harness
            .executor
            .generate_source(&harness.initial_request(), &harness.layout)
            .await
            .expect("generation succeeds");

        let written = harness.read("app/page.jsx").await.expect("the file landed");
        assert!(written.contains("export default function P"));
    }

    #[tokio::test]
    async fn a_restore_job_never_calls_the_model() {
        let harness = generation_harness(Vec::new()).await;
        let mut request = harness.initial_request();
        request.kind = GenerationRequestKind::Restore;

        harness
            .executor
            .generate_source(&request, &harness.layout)
            .await
            .expect("restore reuses existing source");

        assert_eq!(harness.model_calls(), 0, "a restore must not spend an LLM round trip");
    }

    #[tokio::test]
    async fn a_validation_failure_is_fed_back_and_the_second_attempt_can_succeed() {
        let harness = generation_harness(vec![
            // First attempt carries `eval` — the validator will reject it.
            Ok(serde_json::json!({
                "files": [{"path": "app/page.jsx", "contents": "export const x = eval('1')"}]
            })),
            // Second attempt is clean.
            Ok(serde_json::json!({
                "files": [{"path": "app/page.jsx", "contents": "export default function P(){return null}"}]
            })),
        ])
        .await;

        harness
            .executor
            .generate_source(&harness.initial_request(), &harness.layout)
            .await
            .expect("the repair pass succeeds");

        assert_eq!(harness.model_calls(), 2, "exactly one repair round trip");
        let second = harness.prompt_at(1);
        assert!(
            second.contains("eval"),
            "the validator's own words must reach the repair pass: {second}"
        );
    }

    /// The rejected write from attempt 1 lands on disk BEFORE validation
    /// runs. If the repair pass's `existing` tree is never refreshed, attempt
    /// 2 sees the CLEAN pre-attempt snapshot while `validator_feedback` talks
    /// about bytes it can't see — the consistent reading is "nothing to fix",
    /// so the model re-emits nothing and the poisoned file survives forever.
    /// This is `Initial`, not `Revision`, on purpose: it is the sharper case
    /// (the pre-fix code never populated `existing` at all for an initial
    /// job), and it is the common case a first-ever generation attempt hits.
    #[tokio::test]
    async fn a_repair_pass_sees_the_rejected_attempts_bytes_not_a_stale_snapshot() {
        let harness = generation_harness(vec![
            // Attempt 1: a forbidden `fetch(` call. Rejected, but written first.
            Ok(serde_json::json!({
                "files": [{"path": "app/page.jsx", "contents": "export const marker = fetch('https://evil.example')"}]
            })),
            Ok(serde_json::json!({
                "files": [{"path": "app/page.jsx", "contents": "export default function P(){return null}"}]
            })),
        ])
        .await;

        harness
            .executor
            .generate_source(&harness.initial_request(), &harness.layout)
            .await
            .expect("the repair pass succeeds");

        let second = harness.prompt_at(1);
        assert!(
            second.contains("evil.example"),
            "the repair prompt must show the REJECTED bytes actually on disk, not a stale \
             pre-attempt snapshot that contradicts the validator feedback: {second}"
        );
    }

    #[tokio::test]
    async fn three_consecutive_validation_failures_give_up() {
        let dirty = || {
            Ok(serde_json::json!({
                "files": [{"path": "app/page.jsx", "contents": "export const x = eval('1')"}]
            }))
        };
        let harness = generation_harness(vec![dirty(), dirty(), dirty()]).await;

        let error = harness
            .executor
            .generate_source(&harness.initial_request(), &harness.layout)
            .await
            .expect_err("the repair loop is bounded");

        assert_eq!(
            harness.model_calls(),
            3,
            "one initial attempt plus at most two repairs — never an unbounded loop"
        );
        assert!(
            format!("{error}").contains("eval"),
            "exhausting the repair loop must surface the LAST REAL validator error, \
             not a generic 'gave up' message: {error}"
        );
    }

    #[tokio::test]
    async fn a_revision_job_passes_the_prompt_and_the_existing_tree_to_the_model() {
        let harness = generation_harness(vec![Ok(serde_json::json!({
            "files": [{"path": "app/page.jsx", "contents": "export default function P(){return null}"}]
        }))])
        .await;
        harness.seed("components/Old.jsx", "export const Old = 1").await;

        let mut request = harness.initial_request();
        request.kind = GenerationRequestKind::Revision;
        request.prompt = Some("把搜索框挪到顶部".into());

        harness
            .executor
            .generate_source(&request, &harness.layout)
            .await
            .expect("revision succeeds");

        let prompt = harness.prompt_at(0);
        assert!(prompt.contains("把搜索框挪到顶部"), "the user's words: {prompt}");
        assert!(prompt.contains("components/Old.jsx"), "the existing tree: {prompt}");
    }

    #[tokio::test]
    async fn a_revision_leaves_files_the_model_did_not_mention_untouched() {
        let harness = generation_harness(vec![Ok(serde_json::json!({
            "files": [{"path": "app/page.jsx", "contents": "export default function P(){return null}"}]
        }))])
        .await;
        harness.seed("components/Keep.jsx", "export const Keep = 1").await;

        let mut request = harness.initial_request();
        request.kind = GenerationRequestKind::Revision;
        request.prompt = Some("把搜索框挪到顶部".into());

        harness
            .executor
            .generate_source(&request, &harness.layout)
            .await
            .expect("revision succeeds");

        assert_eq!(
            harness.read("components/Keep.jsx").await.as_deref(),
            Some("export const Keep = 1"),
            "writes are an overlay — a one-line change must not require re-emitting the whole app"
        );
    }

    #[test]
    fn read_generated_tree_truncates_over_budget_and_notes_the_omission() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        // Five 1 MiB files comfortably exceed `MAX_GENERATED_TOTAL_BYTES` (4 MiB) once combined.
        for index in 0..5 {
            let bytes = vec![b'x'; 1024 * 1024];
            write_file(&workspace, &format!("app/p{index}.jsx"), &bytes, true).unwrap();
        }

        let (files, note) = read_generated_tree(&workspace).expect("walk the workspace");

        // 5 files of 1 MiB each sum to 5 MiB against a 4 MiB budget: exactly
        // 4 fit (running total after the 4th is 4 MiB, still <= budget; the
        // 5th would push it to 5 MiB) — so exactly 1 file must be omitted.
        // Asserting the exact count (rather than `note.contains('4')`, which
        // any `Some(note)` satisfies once the budget constant itself
        // contains a '4') is what actually pins the truncation math.
        assert_eq!(files.len(), 4, "exactly one of the five files must be cut");
        assert_eq!(
            note.as_deref(),
            Some(
                "现有源码树超过了 4194304 字节的读取预算，按路径字典序\
                 截断，有 1 个文件的内容未在上面展示。它们仍然存在于工作区里，没有\
                 被删除——只是这次没有塞进 prompt，不要假设它们不存在。"
            ),
            "the note must name exactly how many files were left out"
        );
    }

    fn plan_collection(
        id: &str,
        fields: Vec<local_apps::DataFieldSchema>,
    ) -> local_apps::DataCollectionSchema {
        local_apps::DataCollectionSchema {
            id: id.into(),
            name: id.into(),
            fields,
        }
    }

    fn plan_with(
        collections: Vec<local_apps::DataCollectionSchema>,
        domains: Vec<String>,
    ) -> local_apps::AppPlan {
        local_apps::AppPlan {
            collections,
            capabilities: Vec::new(),
            domains,
            summary: "a plan the user confirmed".into(),
        }
    }

    /// A loaded service plus a `MobileAppGenerationExecutor` wired to its own
    /// `MockSink`-backed host — like [`generation_harness`], but stops
    /// BEFORE `prepare_scaffold` and hands the caller everything
    /// (`service`, `sink`, `layout`) needed to stamp a specific plan, run
    /// `prepare_scaffold` explicitly, and — for a destructive migration —
    /// resolve the capability gate concurrently.
    struct ManifestHarness {
        _root: tempfile::TempDir,
        service: Arc<AppService>,
        sink: Arc<MockSink>,
        executor: Arc<MobileAppGenerationExecutor>,
        layout: AppLayout,
        app_id: String,
    }

    impl ManifestHarness {
        fn request(&self, revision: u64, continuation_seq: u64) -> GenerationRequest {
            GenerationRequest {
                key: local_apps::GenerationJobKey {
                    app_id: self.app_id.clone(),
                    revision,
                    continuation_seq,
                },
                kind: GenerationRequestKind::Initial,
                prompt: None,
            }
        }
    }

    async fn manifest_harness() -> ManifestHarness {
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
        let sink = MockSink::arc();
        let host =
            LocalAppsHostBroker::new(root.path().to_path_buf(), sink.clone(), None, false, None);
        let llm = Arc::new(SharedLlm::new(Arc::new(LocalAppsLlm::new(ScriptedModel::new(
            Vec::new(),
        )))));
        let executor = MobileAppGenerationExecutor::new(None, host, llm);
        executor
            .attach_service(service.clone())
            .map_err(|_| "service already attached")
            .unwrap();
        let layout = AppLayout::new(root.path().to_path_buf(), record.id.clone()).unwrap();
        ManifestHarness {
            _root: root,
            service,
            sink,
            executor,
            layout,
            app_id: record.id,
        }
    }

    /// F1: `reconcile_manifest` must source `collections`/`allowed_domains`
    /// from the user-confirmed `AppPlan`, not from questionnaire-answer
    /// field ids that no longer exist. Drives the real designer gate
    /// (`open_designer` → `stamp_plan` → `confirm_design`) rather than
    /// hand-building an `AppManifest`, so this fails if reconciliation ever
    /// stops reading `draft.plan` again.
    #[tokio::test]
    async fn reconcile_manifest_writes_the_confirmed_plans_collections_and_domains() {
        let h = manifest_harness().await;
        local_apps::test_support::advance_to_collecting_spec(&h.service, &h.app_id).await;
        let interaction = h
            .service
            .open_designer(&h.app_id)
            .await
            .expect("open designer");
        let plan = plan_with(
            vec![plan_collection(
                "notes",
                vec![
                    data_field("title", local_apps::DataFieldKind::Text),
                    data_field("body", local_apps::DataFieldKind::LongText),
                ],
            )],
            vec!["api.example.com".into()],
        );
        local_apps::test_support::stamp_plan(&h.service, &h.app_id, plan.clone()).await;
        h.service
            .confirm_design(&h.app_id, &interaction.interaction_id, 0)
            .await
            .expect("confirm design");

        h.executor
            .prepare_scaffold(&h.request(0, 1), &h.layout)
            .await
            .expect("scaffold and manifest reconciliation");

        let manifest = load_manifest(&h.layout).expect("manifest persisted");
        assert_eq!(
            manifest.collections, plan.collections,
            "the confirmed plan's collections must reach the manifest — otherwise every \
             queryCollection/mutateCollection call fails with \"not declared by the app manifest\""
        );
        assert_eq!(
            manifest.allowed_domains, plan.domains,
            "the confirmed plan's domains must reach the manifest — otherwise every \
             requestNetwork call is rejected"
        );
    }

    /// F1: generation reaching `reconcile_manifest` with no confirmed plan
    /// at all is a workflow bug (the designer gate was never cleared), not
    /// a legal "no data" app — it must fail closed with a clear error
    /// rather than silently writing an empty manifest, exactly like its
    /// sibling `generate_source` already does for the same precondition.
    #[tokio::test]
    async fn reconcile_manifest_fails_closed_without_a_confirmed_plan() {
        let h = manifest_harness().await;
        // No `advance_to_collecting_spec` / `stamp_plan` / `confirm_design`
        // at all — `draft.plan` is `None` exactly as `AppState::create`
        // leaves it.

        let error = h
            .executor
            .prepare_scaffold(&h.request(0, 1), &h.layout)
            .await
            .expect_err("no confirmed plan must not produce an empty-but-legal manifest");

        assert!(
            matches!(&error, AppError::WorkflowStateInvalid(message) if message.contains("confirmed plan")),
            "unexpected error: {error}"
        );
        assert!(
            load_manifest(&h.layout)
                .expect("create_app already wrote the initial empty manifest")
                .collections
                .is_empty(),
            "a failed reconciliation must not leave a fabricated non-empty manifest behind"
        );
    }

    /// F1 second-order effect: once collections are real, a revision whose
    /// plan drops a collection field takes `migrate_manifest_with_approval`'s
    /// DESTRUCTIVE path (a real schema shrink, not a hand-built
    /// `AppManifest` bypassing `reconcile_manifest` the way
    /// `manifest_with_score`/`manifest_without_score` do below). It must
    /// still gate on human approval and land the shrunk schema once granted.
    #[tokio::test]
    async fn a_revision_that_shrinks_a_plans_collection_takes_the_destructive_path_and_survives_approval(
    ) {
        let h = manifest_harness().await;
        local_apps::test_support::advance_to_collecting_spec(&h.service, &h.app_id).await;
        let interaction = h
            .service
            .open_designer(&h.app_id)
            .await
            .expect("open designer");
        let wide_plan = plan_with(
            vec![plan_collection(
                "notes",
                vec![
                    data_field("title", local_apps::DataFieldKind::Text),
                    data_field("body", local_apps::DataFieldKind::LongText),
                ],
            )],
            Vec::new(),
        );
        local_apps::test_support::stamp_plan(&h.service, &h.app_id, wide_plan).await;
        h.service
            .confirm_design(&h.app_id, &interaction.interaction_id, 0)
            .await
            .expect("confirm design");
        h.executor
            .prepare_scaffold(&h.request(0, 1), &h.layout)
            .await
            .expect("initial scaffold — additive, no approval needed");
        assert_eq!(
            h.sink.events().await.len(),
            0,
            "the FIRST reconciliation is purely additive over an empty manifest — it must not \
             ask for destructive-migration approval"
        );

        // A later round re-plans the SAME collection with `body` dropped —
        // exactly the shrink F1 flags as needing real (not synthetic) cover.
        let narrow_plan = plan_with(
            vec![plan_collection(
                "notes",
                vec![data_field("title", local_apps::DataFieldKind::Text)],
            )],
            Vec::new(),
        );
        local_apps::test_support::stamp_plan(&h.service, &h.app_id, narrow_plan.clone()).await;

        let approver = tokio::spawn(spawn_capability_resolution(
            h.sink.clone(),
            h.executor.host.clone(),
            AppAuthorizationDecisionDto::AllowAlways,
            None,
        ));
        h.executor
            .prepare_scaffold(&h.request(1, 2), &h.layout)
            .await
            .expect("the shrink succeeds once the destructive migration is approved");
        approver.await.unwrap();

        let manifest = load_manifest(&h.layout).expect("manifest persisted");
        assert_eq!(
            manifest.collections, narrow_plan.collections,
            "the approved shrink must actually land"
        );
        let events = h.sink.events().await;
        assert_eq!(
            events
                .iter()
                .filter(|event| capability_request(event).is_some())
                .count(),
            1,
            "the shrink must have gone through exactly one destructive-migration approval"
        );
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

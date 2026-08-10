//! Durable, single-concurrency local-app generation coordinator.
//!
//! Human-gate continuations are persisted as generation jobs before the
//! continuation delivery succeeds. The worker owns the fixed pipeline and
//! delegates only the platform-specific Node/Next operations to
//! [`AppGenerationExecutor`]. Jobs interrupted by process death are surfaced
//! as retryable on the next attach; they are never silently regenerated.

use crate::continuation::ContinuationSink;
use crate::error::AppError;
use crate::manifest::{AppLayout, GENERATION_JOBS_FILE};
use crate::service::AppService;
use crate::source_validator::{validate_workspace_source, WorkspaceSourcePolicy};
use crate::types::{
    AppCheckpoint, AppCheckpointKind, AppContinuation, AppContinuationKind, AppGenerationProgress,
    AppWorkflowState, APPS_SCHEMA_VERSION,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::UNIX_EPOCH;
use tokio::sync::{mpsc, Mutex};
use traits::rooted_fs::{self, AtomicWriteOptions};
use traits::{Clock, FsError};

const MAX_GENERATION_JOBS_BYTES: u64 = 4 * 1024 * 1024;

/// Durable identity of one continuation-driven generation attempt.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationJobKey {
    /// App being generated.
    pub app_id: String,
    /// Design revision used as generator input.
    pub revision: u64,
    /// Continuation sequence that requested this generation.
    pub continuation_seq: u64,
}

/// Whether the job creates the first scaffold or revises existing source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GenerationRequestKind {
    /// Initial generation after design confirmation.
    Initial,
    /// Revision after preview feedback.
    Revision,
    /// Revalidate and rebuild Git-restored source without regenerating it.
    Restore,
}

/// Durable stage of one generation job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GenerationJobStatus {
    /// Persisted and waiting for the sole worker.
    Queued,
    /// Creating or reconciling the fixed scaffold.
    Scaffolding,
    /// Agent/source generation is running.
    Generating,
    /// Static policy and source validation is running.
    Validating,
    /// Fixed Next production/export build is running.
    Building,
    /// Preview runtime startup/health check is running.
    StartingPreview,
    /// Build succeeded and the user preview gate is open.
    AwaitingPreviewApproval,
    /// The preview was approved.
    Completed,
    /// A stage failed and can be explicitly retried.
    Failed,
    /// An active job was interrupted by application shutdown and can be retried.
    Retryable,
}

impl GenerationJobStatus {
    /// A stage the sole worker owns: the job is either queued for it or
    /// running inside it, and the app workspace is its to mutate.
    fn is_active(self) -> bool {
        matches!(
            self,
            Self::Queued
                | Self::Scaffolding
                | Self::Generating
                | Self::Validating
                | Self::Building
                | Self::StartingPreview
        )
    }

    fn was_interrupted(self) -> bool {
        self.is_active()
    }
}

/// Persisted generation job exposed to native clients.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationJob {
    /// Persistence schema.
    pub schema_version: u32,
    /// Stable dedup key.
    pub key: GenerationJobKey,
    /// Initial generation or revision.
    pub kind: GenerationRequestKind,
    /// Revision prompt for revision jobs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// Durable stage.
    pub status: GenerationJobStatus,
    /// Number of worker attempts, starting at one.
    pub attempt: u32,
    /// Preview URL returned by the runtime executor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_url: Option<String>,
    /// Last actionable failure or interruption reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// Creation time, epoch milliseconds.
    pub created_at_ms: u64,
    /// Last durable mutation time, epoch milliseconds.
    pub updated_at_ms: u64,
}

/// Immutable input handed to each platform-specific pipeline stage.
#[derive(Debug, Clone)]
pub struct GenerationRequest {
    /// Stable dedup key.
    pub key: GenerationJobKey,
    /// Initial generation or revision.
    pub kind: GenerationRequestKind,
    /// User's revision feedback, when present.
    pub prompt: Option<String>,
}

/// Platform-specific fixed generation/build operations.
///
/// Implementations must not install dependencies. They operate only within
/// the validated [`AppLayout`] and are invoked sequentially by one worker.
#[async_trait]
pub trait AppGenerationExecutor: Send + Sync {
    /// Create/reconcile the locked Next scaffold.
    async fn prepare_scaffold(
        &self,
        request: &GenerationRequest,
        layout: &AppLayout,
    ) -> Result<(), AppError>;

    /// Generate constrained application source.
    async fn generate_source(
        &self,
        request: &GenerationRequest,
        layout: &AppLayout,
    ) -> Result<(), AppError>;

    /// Return immutable scaffold hashes used by the native validator. The
    /// policy must lock `package.json` and the npm lockfile.
    async fn source_policy(
        &self,
        request: &GenerationRequest,
        layout: &AppLayout,
    ) -> Result<WorkspaceSourcePolicy, AppError>;

    /// Run static source/policy validation.
    async fn validate_source(
        &self,
        request: &GenerationRequest,
        layout: &AppLayout,
    ) -> Result<(), AppError>;

    /// Run the fixed production/static build.
    async fn build(&self, request: &GenerationRequest, layout: &AppLayout) -> Result<(), AppError>;

    /// Start and health-check the preview, returning its loopback URL.
    async fn start_preview(
        &self,
        request: &GenerationRequest,
        layout: &AppLayout,
    ) -> Result<Option<String>, AppError>;
}

/// Observer seam for persisted job-state changes.
#[async_trait]
pub trait GenerationJobObserver: Send + Sync {
    /// Called after the changed job is durably written.
    async fn on_job_changed(&self, job: GenerationJob);
}

/// Observer that ignores job changes.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopGenerationJobObserver;

#[async_trait]
impl GenerationJobObserver for NoopGenerationJobObserver {
    async fn on_job_changed(&self, _job: GenerationJob) {}
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GenerationJobsFile {
    schema_version: u32,
    jobs: Vec<GenerationJob>,
}

/// Persistent coordinator that also serves as the `AppService` continuation
/// sink. Construct it first, pass the same `Arc` as the sink to
/// `AppService::load`, then call [`Self::attach_service`].
pub struct AppGenerationCoordinator {
    root: PathBuf,
    clock: Arc<dyn Clock>,
    executor: Arc<dyn AppGenerationExecutor>,
    observer: Arc<dyn GenerationJobObserver>,
    service: OnceLock<Arc<AppService>>,
    jobs: Mutex<BTreeMap<GenerationJobKey, GenerationJob>>,
    persist_order: Mutex<()>,
    /// Per-app guard over destructive whole-workspace writes. The active-job
    /// check in [`Self::restore_checkpoint`] is a read-then-act: only holding
    /// this across the `git reset --hard` excludes a job enqueued in that
    /// window, whose build walks the very tree the reset is deleting.
    workspace_locks: Mutex<BTreeMap<String, Arc<Mutex<()>>>>,
    tx: mpsc::UnboundedSender<GenerationJobKey>,
    receiver: Mutex<Option<mpsc::UnboundedReceiver<GenerationJobKey>>>,
}

impl AppGenerationCoordinator {
    /// Create a coordinator with a no-op job observer.
    #[must_use]
    pub fn new(
        root: impl Into<PathBuf>,
        clock: Arc<dyn Clock>,
        executor: Arc<dyn AppGenerationExecutor>,
    ) -> Arc<Self> {
        Self::new_with_observer(root, clock, executor, Arc::new(NoopGenerationJobObserver))
    }

    /// Create a coordinator whose durable job changes are forwarded to an
    /// embedding-specific observer (for example the client protocol bridge).
    #[must_use]
    pub fn new_with_observer(
        root: impl Into<PathBuf>,
        clock: Arc<dyn Clock>,
        executor: Arc<dyn AppGenerationExecutor>,
        observer: Arc<dyn GenerationJobObserver>,
    ) -> Arc<Self> {
        let (tx, receiver) = mpsc::unbounded_channel();
        Arc::new(Self {
            root: root.into(),
            clock,
            executor,
            observer,
            service: OnceLock::new(),
            jobs: Mutex::new(BTreeMap::new()),
            persist_order: Mutex::new(()),
            workspace_locks: Mutex::new(BTreeMap::new()),
            tx,
            receiver: Mutex::new(Some(receiver)),
        })
    }

    /// Attach the service, load all durable jobs, convert interrupted or
    /// orphaned generating/validating states to explicit retryable failures,
    /// and start the sole sequential worker.
    pub async fn attach_service(
        self: &Arc<Self>,
        service: Arc<AppService>,
    ) -> Result<(), AppError> {
        self.service.set(service.clone()).map_err(|_| {
            AppError::InvalidRequest("generation service is already attached".into())
        })?;
        let records = service.records().await;
        let mut recovered = BTreeMap::new();
        for record in &records {
            let layout = AppLayout::new(self.root.clone(), record.id.clone())?;
            for mut job in load_jobs(&layout)? {
                if job.key.app_id != record.id || job.schema_version != APPS_SCHEMA_VERSION {
                    return Err(AppError::StorageCorrupt(format!(
                        "generation job identity/schema mismatch for app {}",
                        record.id
                    )));
                }
                if job.status.was_interrupted() {
                    job.status = GenerationJobStatus::Retryable;
                    job.last_error = Some("generation was interrupted; retry explicitly".into());
                    job.updated_at_ms = self.now_ms();
                }
                if recovered.insert(job.key.clone(), job).is_some() {
                    return Err(AppError::StorageCorrupt(format!(
                        "duplicate generation job key for app {}",
                        record.id
                    )));
                }
            }
        }
        *self.jobs.lock().await = recovered;

        for record in records {
            let has_recoverable_job = self.jobs.lock().await.values().any(|job| {
                job.key.app_id == record.id
                    && matches!(
                        job.status,
                        GenerationJobStatus::Retryable | GenerationJobStatus::Failed
                    )
            });
            if !has_recoverable_job
                && matches!(
                    record.workflow_state,
                    AppWorkflowState::Generating | AppWorkflowState::Validating
                )
            {
                let draft = service.draft(&record.id).await?;
                let now = self.now_ms();
                let key = GenerationJobKey {
                    app_id: record.id.clone(),
                    revision: draft.confirmed_revision.unwrap_or(draft.revision),
                    continuation_seq: 0,
                };
                let job = GenerationJob {
                    schema_version: APPS_SCHEMA_VERSION,
                    key: key.clone(),
                    kind: GenerationRequestKind::Initial,
                    prompt: None,
                    status: GenerationJobStatus::Retryable,
                    attempt: 0,
                    preview_url: None,
                    last_error: Some(
                        "workflow was active without a durable generation job; retry explicitly"
                            .into(),
                    ),
                    created_at_ms: now,
                    updated_at_ms: now,
                };
                self.jobs.lock().await.insert(key, job.clone());
                self.persist_and_notify(&record.id, Some(job)).await?;
            } else {
                self.persist_and_notify(&record.id, None).await?;
            }
            match record.workflow_state {
                AppWorkflowState::Generating => {
                    service
                        .generation_failed(
                            &record.id,
                            Some("generation was interrupted and can be retried".into()),
                        )
                        .await?;
                }
                AppWorkflowState::Validating => {
                    service
                        .validation_failed(
                            &record.id,
                            Some("validation was interrupted and can be retried".into()),
                        )
                        .await?;
                }
                _ => {}
            }
        }

        let receiver = self.receiver.lock().await.take().ok_or_else(|| {
            AppError::InvalidRequest("generation worker is already running".into())
        })?;
        let coordinator = Arc::clone(self);
        tokio::spawn(async move { coordinator.worker_loop(receiver).await });
        Ok(())
    }

    /// Snapshot jobs for one app, newest first.
    pub async fn jobs_for_app(&self, app_id: &str) -> Result<Vec<GenerationJob>, AppError> {
        crate::ids::validate_app_id(app_id)?;
        let mut jobs: Vec<_> = self
            .jobs
            .lock()
            .await
            .values()
            .filter(|job| job.key.app_id == app_id)
            .cloned()
            .collect();
        jobs.sort_by(|a, b| {
            b.updated_at_ms
                .cmp(&a.updated_at_ms)
                .then_with(|| b.key.cmp(&a.key))
        });
        Ok(jobs)
    }

    /// Explicitly retry the newest failed/interrupted job for an app.
    /// Re-queue an app's failed generation job.
    ///
    /// `prompt` is the user's own words, sent from the failure screen. It
    /// replaces the job's prompt so the next attempt is a CONVERSATION rather
    /// than a blind replay: it reaches the model as
    /// `SourceRequest::revision_prompt`, the same channel a revision uses.
    /// `None` retries unchanged — a plain "try again" must not erase the
    /// prompt a revision job already carried.
    pub async fn retry_app(
        &self,
        app_id: &str,
        prompt: Option<String>,
    ) -> Result<GenerationJob, AppError> {
        let service = self.attached_service()?;
        let candidate = self
            .jobs_for_app(app_id)
            .await?
            .into_iter()
            .find(|job| {
                matches!(
                    job.status,
                    GenerationJobStatus::Failed | GenerationJobStatus::Retryable
                )
            })
            .ok_or_else(|| {
                AppError::NotFound(format!("no retryable generation job for {app_id}"))
            })?;
        match service.record(app_id).await?.workflow_state {
            AppWorkflowState::GenerationFailed => service.retry_generation(app_id).await?,
            AppWorkflowState::ValidationFailed => service.begin_revision(app_id).await?,
            AppWorkflowState::Generating
            | AppWorkflowState::Validating
            | AppWorkflowState::Revising => {}
            state => {
                return Err(AppError::WorkflowStateInvalid(format!(
                    "cannot retry generation while app {app_id} is {state}"
                )));
            }
        }
        let job = self
            .update_job(&candidate.key, |job, now| {
                job.status = GenerationJobStatus::Queued;
                job.attempt = job.attempt.saturating_add(1);
                job.last_error = None;
                job.preview_url = None;
                if let Some(prompt) = prompt.clone() {
                    job.prompt = Some(prompt);
                }
                job.updated_at_ms = now;
            })
            .await?;
        self.tx
            .send(candidate.key)
            .map_err(|_| AppError::Io("generation worker is unavailable".into()))?;
        Ok(job)
    }

    /// Queue validation/build of a restored workspace. Source generation is
    /// deliberately skipped so Git is the source of truth for the restored
    /// revision while the `SQLite` database remains untouched.
    pub async fn enqueue_restored_rebuild(&self, app_id: &str) -> Result<GenerationJob, AppError> {
        let service = self.attached_service()?;
        let revision = service.draft(app_id).await?.revision;
        service.begin_restore_rebuild(app_id).await?;
        let continuation_seq = self
            .jobs_for_app(app_id)
            .await?
            .into_iter()
            .map(|job| job.key.continuation_seq)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        let continuation = AppContinuation {
            seq: continuation_seq,
            app_id: app_id.to_string(),
            kind: AppContinuationKind::RevisionRequested,
            payload: serde_json::json!({ "reason": "checkpoint_restore" }),
            created_at_ms: self.now_ms(),
        };
        self.enqueue_generation(
            &continuation,
            GenerationRequestKind::Restore,
            revision,
            Some("Rebuild restored checkpoint".into()),
        )
        .await?;
        self.jobs_for_app(app_id)
            .await?
            .into_iter()
            .find(|job| job.key.continuation_seq == continuation_seq)
            .ok_or_else(|| AppError::Io("restore rebuild job disappeared after enqueue".into()))
    }

    /// Restore a checkpoint and queue the rebuild that must follow it.
    ///
    /// The `git reset --hard` is destructive and cannot be undone from the
    /// client, so both preconditions of the rebuild are checked BEFORE it
    /// rather than afterwards by `begin_restore_rebuild`.
    pub async fn restore_checkpoint(
        &self,
        app_id: &str,
        checkpoint_id: &str,
    ) -> Result<(AppCheckpoint, GenerationJob), AppError> {
        let service = self.attached_service()?;
        let state = service.record(app_id).await?.workflow_state;
        if state != AppWorkflowState::Ready {
            return Err(AppError::WorkflowStateInvalid(format!(
                "cannot restore a checkpoint while app {app_id} is {state}"
            )));
        }
        if self
            .jobs_for_app(app_id)
            .await?
            .iter()
            .any(|job| job.status.is_active())
        {
            return Err(AppError::WorkflowStateInvalid(format!(
                "cannot restore a checkpoint while a generation job for app {app_id} is running"
            )));
        }
        let workspace = self.workspace_lock(app_id).await;
        let Ok(_guard) = workspace.try_lock() else {
            return Err(AppError::WorkflowStateInvalid(format!(
                "cannot restore a checkpoint while a generation job for app {app_id} is running"
            )));
        };
        let safety = service.restore_checkpoint(app_id, checkpoint_id).await?;
        let job = self.enqueue_restored_rebuild(app_id).await?;
        Ok((safety, job))
    }

    /// The `app_id`-keyed workspace guard, created on first use.
    async fn workspace_lock(&self, app_id: &str) -> Arc<Mutex<()>> {
        self.workspace_locks
            .lock()
            .await
            .entry(app_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    fn attached_service(&self) -> Result<Arc<AppService>, AppError> {
        self.service
            .get()
            .cloned()
            .ok_or_else(|| AppError::Io("generation service is not attached".into()))
    }

    fn now_ms(&self) -> u64 {
        self.clock
            .now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| {
                u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
            })
    }

    async fn worker_loop(self: Arc<Self>, mut receiver: mpsc::UnboundedReceiver<GenerationJobKey>) {
        while let Some(key) = receiver.recv().await {
            if let Err(error) = self.run_job(&key).await {
                tracing::error!(app_id = %key.app_id, revision = key.revision, error = %error, "local app generation failed");
                if let Err(persist_error) = self.fail_job(&key, error.to_string()).await {
                    tracing::error!(app_id = %key.app_id, error = %persist_error, "failed to persist local app generation failure");
                }
            }
        }
    }

    async fn run_job(&self, key: &GenerationJobKey) -> Result<(), AppError> {
        let service = self.attached_service()?;
        let job = self
            .jobs
            .lock()
            .await
            .get(key)
            .cloned()
            .ok_or_else(|| AppError::NotFound(format!("generation job {key:?}")))?;
        if job.status != GenerationJobStatus::Queued {
            return Ok(());
        }
        let request = GenerationRequest {
            key: key.clone(),
            kind: job.kind,
            prompt: job.prompt.clone(),
        };
        let layout = AppLayout::new(self.root.clone(), key.app_id.clone())?;
        // Held for the whole pipeline: every stage below reads or rewrites the
        // workspace tree a concurrent checkpoint restore would hard-reset.
        let workspace = self.workspace_lock(&key.app_id).await;
        let _workspace_guard = workspace.lock().await;

        self.set_stage(key, GenerationJobStatus::Scaffolding, "scaffold", 5)
            .await?;
        self.executor.prepare_scaffold(&request, &layout).await?;
        if job.kind == GenerationRequestKind::Initial
            && service.git_version_control_enabled(&key.app_id).await?
        {
            service
                .create_checkpoint(
                    &key.app_id,
                    AppCheckpointKind::ScaffoldCreated,
                    "Scaffold created",
                )
                .await?;
        }

        self.set_stage(key, GenerationJobStatus::Generating, "generate", 25)
            .await?;
        self.executor.generate_source(&request, &layout).await?;
        match service.record(&key.app_id).await?.workflow_state {
            AppWorkflowState::Generating => service.generation_complete(&key.app_id).await?,
            AppWorkflowState::Revising => service.revision_ready(&key.app_id).await?,
            AppWorkflowState::Validating => {}
            state => {
                return Err(AppError::WorkflowStateInvalid(format!(
                    "generation job cannot enter validation while app {} is {state}",
                    key.app_id
                )));
            }
        }

        self.set_stage(key, GenerationJobStatus::Validating, "validate", 50)
            .await?;
        let policy = self.executor.source_policy(&request, &layout).await?;
        validate_workspace_source(&layout, &policy)?;
        self.executor.validate_source(&request, &layout).await?;
        self.set_stage(key, GenerationJobStatus::Building, "build", 70)
            .await?;
        self.executor.build(&request, &layout).await?;
        if service.git_version_control_enabled(&key.app_id).await? {
            service
                .create_checkpoint(
                    &key.app_id,
                    AppCheckpointKind::GenerationValidated,
                    "Generation validated",
                )
                .await?;
        }

        self.set_stage(
            key,
            GenerationJobStatus::StartingPreview,
            "start_preview",
            90,
        )
        .await?;
        let preview_url = self.executor.start_preview(&request, &layout).await?;
        service.validation_passed(&key.app_id).await?;
        let job = self
            .update_job(key, move |job, now| {
                job.status = GenerationJobStatus::AwaitingPreviewApproval;
                job.preview_url = preview_url;
                job.last_error = None;
                job.updated_at_ms = now;
            })
            .await?;
        service
            .report_generation_progress(AppGenerationProgress {
                app_id: key.app_id.clone(),
                stage: "awaiting_preview_approval".into(),
                percent: Some(100),
                detail: job.preview_url.clone(),
            })
            .await?;
        Ok(())
    }

    async fn set_stage(
        &self,
        key: &GenerationJobKey,
        status: GenerationJobStatus,
        stage: &str,
        percent: u8,
    ) -> Result<(), AppError> {
        let service = self.attached_service()?;
        self.update_job(key, |job, now| {
            job.status = status;
            job.updated_at_ms = now;
        })
        .await?;
        service
            .report_generation_progress(AppGenerationProgress {
                app_id: key.app_id.clone(),
                stage: stage.to_string(),
                percent: Some(percent),
                detail: None,
            })
            .await
    }

    async fn fail_job(&self, key: &GenerationJobKey, detail: String) -> Result<(), AppError> {
        let service = self.attached_service()?;
        match service.record(&key.app_id).await?.workflow_state {
            AppWorkflowState::Generating => {
                service
                    .generation_failed(&key.app_id, Some(detail.clone()))
                    .await?;
            }
            AppWorkflowState::Revising => {
                service.revision_ready(&key.app_id).await?;
                service
                    .validation_failed(&key.app_id, Some(detail.clone()))
                    .await?;
            }
            AppWorkflowState::Validating => {
                service
                    .validation_failed(&key.app_id, Some(detail.clone()))
                    .await?;
            }
            _ => {}
        }
        self.update_job(key, move |job, now| {
            job.status = GenerationJobStatus::Failed;
            job.last_error = Some(detail);
            job.updated_at_ms = now;
        })
        .await?;
        Ok(())
    }

    async fn update_job(
        &self,
        key: &GenerationJobKey,
        update: impl FnOnce(&mut GenerationJob, u64),
    ) -> Result<GenerationJob, AppError> {
        let now = self.now_ms();
        let changed = {
            let mut jobs = self.jobs.lock().await;
            let job = jobs
                .get_mut(key)
                .ok_or_else(|| AppError::NotFound(format!("generation job {key:?}")))?;
            update(job, now);
            job.clone()
        };
        self.persist_and_notify(&key.app_id, Some(changed.clone()))
            .await?;
        Ok(changed)
    }

    async fn persist_and_notify(
        &self,
        app_id: &str,
        changed: Option<GenerationJob>,
    ) -> Result<(), AppError> {
        let persist_guard = self.persist_order.lock().await;
        let snapshot: Vec<_> = self
            .jobs
            .lock()
            .await
            .values()
            .filter(|job| job.key.app_id == app_id)
            .cloned()
            .collect();
        let layout = AppLayout::new(self.root.clone(), app_id.to_string())?;
        save_jobs(&layout, &snapshot)?;
        drop(persist_guard);
        if let Some(job) = changed {
            self.observer.on_job_changed(job).await;
        }
        Ok(())
    }

    async fn enqueue_generation(
        &self,
        continuation: &AppContinuation,
        kind: GenerationRequestKind,
        revision: u64,
        prompt: Option<String>,
    ) -> Result<(), AppError> {
        let key = GenerationJobKey {
            app_id: continuation.app_id.clone(),
            revision,
            continuation_seq: continuation.seq,
        };
        let now = self.now_ms();
        let job = GenerationJob {
            schema_version: APPS_SCHEMA_VERSION,
            key: key.clone(),
            kind,
            prompt,
            status: GenerationJobStatus::Queued,
            attempt: 1,
            preview_url: None,
            last_error: None,
            created_at_ms: now,
            updated_at_ms: now,
        };
        let inserted = {
            let mut jobs = self.jobs.lock().await;
            if jobs.contains_key(&key) {
                false
            } else {
                jobs.insert(key.clone(), job.clone());
                true
            }
        };
        if !inserted {
            self.persist_and_notify(&key.app_id, None).await?;
            let queued = self
                .jobs
                .lock()
                .await
                .get(&key)
                .is_some_and(|job| job.status == GenerationJobStatus::Queued);
            if queued {
                self.tx
                    .send(key)
                    .map_err(|_| AppError::Io("generation worker is unavailable".into()))?;
            }
            return Ok(());
        }
        if let Err(error) = self.persist_and_notify(&key.app_id, Some(job)).await {
            self.jobs.lock().await.remove(&key);
            return Err(error);
        }
        self.tx
            .send(key)
            .map_err(|_| AppError::Io("generation worker is unavailable".into()))
    }

    async fn complete_preview(&self, continuation: &AppContinuation) -> Result<(), AppError> {
        let revision = continuation
            .payload
            .get("revision")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| {
                AppError::InvalidRequest("preview continuation has no numeric revision".into())
            })?;
        let jobs = self.jobs_for_app(&continuation.app_id).await?;
        // The approved revision is the draft revision the preview gate was
        // opened at, which can legally be NEWER than the key the job was
        // minted with: `Revising` is a draft-editable state, so an edit made
        // while the job runs bumps the draft without touching the job. Fall
        // back to the job actually sitting at the preview gate — an exact
        // match here would return `NotFound` forever and, because redelivery
        // always re-picks the queue head, starve every later continuation of
        // this app behind it.
        let candidate = jobs
            .iter()
            .find(|job| job.key.revision == revision)
            .or_else(|| {
                jobs.iter()
                    .filter(|job| job.status == GenerationJobStatus::AwaitingPreviewApproval)
                    .max_by_key(|job| job.key.continuation_seq)
            })
            .cloned()
            .ok_or_else(|| {
                AppError::NotFound(format!(
                    "no generation job for approved revision {revision}"
                ))
            })?;
        if candidate.status != GenerationJobStatus::Completed {
            self.update_job(&candidate.key, |job, now| {
                job.status = GenerationJobStatus::Completed;
                job.updated_at_ms = now;
            })
            .await?;
        }
        let service = self.attached_service()?;
        if service
            .git_version_control_enabled(&continuation.app_id)
            .await?
        {
            let checkpoints = service.list_checkpoints(&continuation.app_id).await?;
            let already_recorded = checkpoints.iter().any(|checkpoint| {
                checkpoint.kind == AppCheckpointKind::PreviewApproved
                    && checkpoint.created_at_ms >= candidate.created_at_ms
            });
            if !already_recorded {
                service
                    .create_checkpoint(
                        &continuation.app_id,
                        AppCheckpointKind::PreviewApproved,
                        "Preview approved",
                    )
                    .await?;
            }
            let user_approved = checkpoints.iter().any(|checkpoint| {
                checkpoint.kind == AppCheckpointKind::UserApproved
                    && checkpoint.created_at_ms >= candidate.created_at_ms
            });
            if !user_approved {
                service
                    .create_checkpoint(
                        &continuation.app_id,
                        AppCheckpointKind::UserApproved,
                        "User approved",
                    )
                    .await?;
            }
        }
        Ok(())
    }
}

#[async_trait]
impl ContinuationSink for AppGenerationCoordinator {
    async fn deliver(&self, app_id: &str, continuation: &AppContinuation) -> Result<(), AppError> {
        if app_id != continuation.app_id {
            return Err(AppError::InvalidRequest(
                "continuation app id does not match delivery app id".into(),
            ));
        }
        let service = self.attached_service()?;
        match continuation.kind {
            AppContinuationKind::DesignConfirmed => {
                let revision = continuation
                    .payload
                    .get("revision")
                    .and_then(serde_json::Value::as_u64)
                    .ok_or_else(|| {
                        AppError::InvalidRequest(
                            "design continuation has no numeric revision".into(),
                        )
                    })?;
                self.enqueue_generation(
                    continuation,
                    GenerationRequestKind::Initial,
                    revision,
                    None,
                )
                .await
            }
            AppContinuationKind::RevisionRequested => {
                let revision = service.draft(app_id).await?.revision;
                let prompt = continuation
                    .payload
                    .get("prompt")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);
                self.enqueue_generation(
                    continuation,
                    GenerationRequestKind::Revision,
                    revision,
                    prompt,
                )
                .await
            }
            AppContinuationKind::PreviewConfirmed => self.complete_preview(continuation).await,
            AppContinuationKind::DesignCancelled => Ok(()),
        }
    }
}

fn save_jobs(layout: &AppLayout, jobs: &[GenerationJob]) -> Result<(), AppError> {
    layout.initialize()?;
    let file = GenerationJobsFile {
        schema_version: APPS_SCHEMA_VERSION,
        jobs: jobs.to_vec(),
    };
    let mut body = serde_json::to_vec_pretty(&file)
        .map_err(|error| AppError::Io(format!("serialize {GENERATION_JOBS_FILE}: {error}")))?;
    body.push(b'\n');
    if body.len() as u64 > MAX_GENERATION_JOBS_BYTES {
        return Err(AppError::InvalidRequest(format!(
            "{GENERATION_JOBS_FILE} is {} bytes (limit {MAX_GENERATION_JOBS_BYTES})",
            body.len()
        )));
    }
    rooted_fs::atomic_write(
        layout.root(),
        &layout.generation_jobs_rel(),
        &body,
        AtomicWriteOptions::default(),
    )
    .map_err(|error| AppError::from_fs("write generation jobs", &error))
}

fn load_jobs(layout: &AppLayout) -> Result<Vec<GenerationJob>, AppError> {
    let body = match rooted_fs::read_to_string_limited(
        layout.root(),
        &layout.generation_jobs_rel(),
        MAX_GENERATION_JOBS_BYTES,
    ) {
        Ok(body) => body,
        Err(FsError::NotFound(_)) => return Ok(Vec::new()),
        Err(error) => return Err(AppError::from_fs("read generation jobs", &error)),
    };
    let file: GenerationJobsFile = serde_json::from_str(&body)
        .map_err(|error| AppError::StorageCorrupt(format!("generation jobs: {error}")))?;
    if file.schema_version != APPS_SCHEMA_VERSION {
        return Err(AppError::StorageCorrupt(format!(
            "generation jobs schemaVersion {} is unsupported",
            file.schema_version
        )));
    }
    Ok(file.jobs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::NoopAppEventObserver;
    use crate::storage;
    use crate::test_support::{advance_to_collecting_spec, stamp_fresh_plan, FixedClock};
    use crate::types::{AppDesignPatch, AppDesignPatchOp, DesignValue};
    use sha2::Digest;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::time::{sleep, Duration};

    #[derive(Default)]
    struct RecordingExecutor {
        active: AtomicUsize,
        max_active: AtomicUsize,
    }

    impl RecordingExecutor {
        async fn enter(&self, layout: &AppLayout, filename: &str) -> Result<(), AppError> {
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_active.fetch_max(active, Ordering::SeqCst);
            sleep(Duration::from_millis(2)).await;
            std::fs::write(
                layout.root().join(layout.workspace_rel()).join(filename),
                "ok",
            )
            .map_err(|error| AppError::Io(error.to_string()))?;
            self.active.fetch_sub(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[async_trait]
    impl AppGenerationExecutor for RecordingExecutor {
        async fn prepare_scaffold(
            &self,
            _: &GenerationRequest,
            layout: &AppLayout,
        ) -> Result<(), AppError> {
            self.enter(layout, "package.json").await?;
            self.enter(layout, "package-lock.json").await
        }
        async fn generate_source(
            &self,
            _: &GenerationRequest,
            layout: &AppLayout,
        ) -> Result<(), AppError> {
            let app = layout.root().join(layout.workspace_rel()).join("app");
            std::fs::create_dir_all(&app).map_err(|error| AppError::Io(error.to_string()))?;
            self.enter(layout, "app/page.js").await
        }
        async fn source_policy(
            &self,
            _: &GenerationRequest,
            _: &AppLayout,
        ) -> Result<WorkspaceSourcePolicy, AppError> {
            Ok(WorkspaceSourcePolicy {
                locked_files: BTreeMap::from([
                    (
                        PathBuf::from("package.json"),
                        format!("{:x}", sha2::Sha256::digest(b"ok")),
                    ),
                    (
                        PathBuf::from("package-lock.json"),
                        format!("{:x}", sha2::Sha256::digest(b"ok")),
                    ),
                ]),
            })
        }
        async fn validate_source(
            &self,
            _: &GenerationRequest,
            layout: &AppLayout,
        ) -> Result<(), AppError> {
            self.enter(layout, "validated").await
        }
        async fn build(&self, _: &GenerationRequest, layout: &AppLayout) -> Result<(), AppError> {
            self.enter(layout, "built").await
        }
        async fn start_preview(
            &self,
            _: &GenerationRequest,
            _: &AppLayout,
        ) -> Result<Option<String>, AppError> {
            Ok(Some("http://127.0.0.1:41000".into()))
        }
    }

    async fn wait_for_status(
        coordinator: &AppGenerationCoordinator,
        app_id: &str,
        status: GenerationJobStatus,
    ) {
        for _ in 0..1_000 {
            if coordinator
                .jobs_for_app(app_id)
                .await
                .unwrap()
                .iter()
                .any(|job| job.status == status)
            {
                return;
            }
            sleep(Duration::from_millis(5)).await;
        }
        panic!(
            "job did not reach {status:?}: {:?}",
            coordinator.jobs_for_app(app_id).await.unwrap()
        );
    }

    #[tokio::test]
    async fn continuation_is_durable_deduped_and_runs_fixed_pipeline() {
        let root = tempfile::tempdir().unwrap();
        let clock = Arc::new(FixedClock::new(1_000));
        let executor = Arc::new(RecordingExecutor::default());
        let coordinator =
            AppGenerationCoordinator::new(root.path(), clock.clone(), executor.clone());
        let service = Arc::new(
            AppService::load(
                root.path(),
                clock,
                coordinator.clone(),
                Arc::new(NoopAppEventObserver),
            )
            .await
            .unwrap(),
        );
        coordinator.attach_service(service.clone()).await.unwrap();
        let app = service
            .create_app(Some("Tasks"), "a test app", None)
            .await
            .unwrap();
        let app = advance_to_collecting_spec(&service, &app.id).await;
        service
            .update_draft(
                &app.id,
                0,
                &AppDesignPatch {
                    ops: vec![AppDesignPatchOp::Set {
                        field_id: "purpose".into(),
                        value: DesignValue::ShortText("Track tasks".into()),
                    }],
                    note: None,
                },
            )
            .await
            .unwrap();
        let gate = service.open_designer(&app.id).await.unwrap();
        stamp_fresh_plan(&service, &app.id).await;
        service
            .confirm_design(&app.id, &gate.interaction_id, 1)
            .await
            .unwrap();
        wait_for_status(
            &coordinator,
            &app.id,
            GenerationJobStatus::AwaitingPreviewApproval,
        )
        .await;
        let jobs = coordinator.jobs_for_app(&app.id).await.unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(executor.max_active.load(Ordering::SeqCst), 1);
        let persisted = load_jobs(&AppLayout::new(root.path(), &app.id).unwrap()).unwrap();
        assert_eq!(persisted.len(), 1);
    }

    /// The preview gate opens at the CURRENT draft revision, which need not
    /// be the revision the running job was keyed with: `Revising` is a
    /// draft-editable state, so an edit landing while the job runs bumps the
    /// draft and leaves the job behind. The approval must still resolve that
    /// job — an error here is never retried into success (redelivery always
    /// re-picks the queue head), so it would starve every later continuation
    /// of this app and leave the job at the gate forever.
    #[tokio::test]
    async fn preview_confirmed_at_a_newer_revision_still_completes_the_job_at_the_gate() {
        let root = tempfile::tempdir().unwrap();
        let clock = Arc::new(FixedClock::new(1_000));
        let executor = Arc::new(RecordingExecutor::default());
        let coordinator =
            AppGenerationCoordinator::new(root.path(), clock.clone(), executor.clone());
        let service = Arc::new(
            AppService::load(
                root.path(),
                clock,
                coordinator.clone(),
                Arc::new(NoopAppEventObserver),
            )
            .await
            .unwrap(),
        );
        coordinator.attach_service(service.clone()).await.unwrap();
        let app = service
            .create_app(Some("Tasks"), "a test app", None)
            .await
            .unwrap();
        let app = advance_to_collecting_spec(&service, &app.id).await;
        let gate = service.open_designer(&app.id).await.unwrap();
        stamp_fresh_plan(&service, &app.id).await;
        service
            .confirm_design(&app.id, &gate.interaction_id, 0)
            .await
            .unwrap();
        wait_for_status(
            &coordinator,
            &app.id,
            GenerationJobStatus::AwaitingPreviewApproval,
        )
        .await;
        let job_revision = coordinator.jobs_for_app(&app.id).await.unwrap()[0].key.revision;

        coordinator
            .deliver(
                &app.id,
                &AppContinuation {
                    seq: 99,
                    app_id: app.id.clone(),
                    kind: AppContinuationKind::PreviewConfirmed,
                    payload: serde_json::json!({ "revision": job_revision + 1 }),
                    created_at_ms: 2_000,
                },
            )
            .await
            .expect("an approval past the job's revision must still be deliverable");

        let jobs = coordinator.jobs_for_app(&app.id).await.unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].status, GenerationJobStatus::Completed);
        let kinds: Vec<_> = service
            .list_checkpoints(&app.id)
            .await
            .unwrap()
            .into_iter()
            .map(|checkpoint| checkpoint.kind)
            .collect();
        assert!(kinds.contains(&AppCheckpointKind::PreviewApproved), "{kinds:?}");
        assert!(kinds.contains(&AppCheckpointKind::UserApproved), "{kinds:?}");
    }

    /// The active-job census in `restore_checkpoint` is a read-then-act, so a
    /// job enqueued after it is only excluded by the workspace guard. With a
    /// Ready app and no job in the census, holding the guard must still refuse
    /// — before `AppCheckpointStore::restore` can hard-reset the tree (a bogus
    /// checkpoint id would otherwise fail with NotFound, not this error).
    /// Everything a `RecordingExecutor` does, except source generation, which
    /// fails the way a model that will not produce usable files does.
    struct FailsSourceGeneration(RecordingExecutor);

    #[async_trait]
    impl AppGenerationExecutor for FailsSourceGeneration {
        async fn prepare_scaffold(
            &self,
            request: &GenerationRequest,
            layout: &AppLayout,
        ) -> Result<(), AppError> {
            self.0.prepare_scaffold(request, layout).await
        }

        async fn generate_source(
            &self,
            _: &GenerationRequest,
            _: &AppLayout,
        ) -> Result<(), AppError> {
            Err(AppError::LlmOutputRejected(
                "`lib/image-utils.js` has no contents".into(),
            ))
        }

        async fn source_policy(
            &self,
            request: &GenerationRequest,
            layout: &AppLayout,
        ) -> Result<WorkspaceSourcePolicy, AppError> {
            self.0.source_policy(request, layout).await
        }

        async fn validate_source(
            &self,
            request: &GenerationRequest,
            layout: &AppLayout,
        ) -> Result<(), AppError> {
            self.0.validate_source(request, layout).await
        }

        async fn build(
            &self,
            request: &GenerationRequest,
            layout: &AppLayout,
        ) -> Result<(), AppError> {
            self.0.build(request, layout).await
        }

        async fn start_preview(
            &self,
            request: &GenerationRequest,
            layout: &AppLayout,
        ) -> Result<Option<String>, AppError> {
            self.0.start_preview(request, layout).await
        }
    }

    /// A retry carries the user's own words into the next attempt.
    ///
    /// This is what makes a failed generation a conversation rather than a
    /// dead end: the words land on the job as `prompt`, which the worker hands
    /// the generator as `SourceRequest::revision_prompt` — the same channel a
    /// revision uses. A retry with no words must leave any prompt the job
    /// already carried alone, or a plain "try again" on a revision job would
    /// silently discard what the user asked for the first time.
    #[tokio::test]
    async fn a_retry_carries_the_users_words_and_a_wordless_one_preserves_them() {
        let root = tempfile::tempdir().unwrap();
        let clock = Arc::new(FixedClock::new(1_000));
        let executor = Arc::new(FailsSourceGeneration(RecordingExecutor::default()));
        let coordinator =
            AppGenerationCoordinator::new(root.path(), clock.clone(), executor.clone());
        let service = Arc::new(
            AppService::load(
                root.path(),
                clock,
                coordinator.clone(),
                Arc::new(NoopAppEventObserver),
            )
            .await
            .unwrap(),
        );
        coordinator.attach_service(service.clone()).await.unwrap();
        let app = service
            .create_app(Some("Tasks"), "a test app", None)
            .await
            .unwrap();
        let app = advance_to_collecting_spec(&service, &app.id).await;
        let gate = service.open_designer(&app.id).await.unwrap();
        stamp_fresh_plan(&service, &app.id).await;
        service
            .confirm_design(&app.id, &gate.interaction_id, 0)
            .await
            .unwrap();
        wait_for_status(&coordinator, &app.id, GenerationJobStatus::Failed).await;

        let retried = coordinator
            .retry_app(&app.id, Some("配色再淡一点".into()))
            .await
            .expect("a failed job is retryable");
        assert_eq!(
            retried.prompt.as_deref(),
            Some("配色再淡一点"),
            "the user's words must reach the job the worker will run"
        );

        wait_for_status(&coordinator, &app.id, GenerationJobStatus::Failed).await;
        let wordless = coordinator
            .retry_app(&app.id, None)
            .await
            .expect("a wordless retry is still a retry");
        assert_eq!(
            wordless.prompt.as_deref(),
            Some("配色再淡一点"),
            "a retry with no words must not erase the words already on the job"
        );
    }

    #[tokio::test]
    async fn restore_is_refused_while_the_workspace_guard_is_held() {
        let root = tempfile::tempdir().unwrap();
        let clock = Arc::new(FixedClock::new(1_000));
        let executor = Arc::new(RecordingExecutor::default());
        let coordinator =
            AppGenerationCoordinator::new(root.path(), clock.clone(), executor.clone());
        let service = Arc::new(
            AppService::load(
                root.path(),
                clock,
                coordinator.clone(),
                Arc::new(NoopAppEventObserver),
            )
            .await
            .unwrap(),
        );
        coordinator.attach_service(service.clone()).await.unwrap();
        let app = service
            .create_app(Some("Tasks"), "a test app", None)
            .await
            .unwrap();
        let app = advance_to_collecting_spec(&service, &app.id).await;
        let gate = service.open_designer(&app.id).await.unwrap();
        stamp_fresh_plan(&service, &app.id).await;
        service
            .confirm_design(&app.id, &gate.interaction_id, 0)
            .await
            .unwrap();
        wait_for_status(
            &coordinator,
            &app.id,
            GenerationJobStatus::AwaitingPreviewApproval,
        )
        .await;
        let preview = service
            .pending_interaction(&app.id)
            .await
            .unwrap()
            .expect("preview gate");
        service
            .confirm_preview(&app.id, &preview.interaction_id, preview.revision)
            .await
            .unwrap();
        assert_eq!(
            service.record(&app.id).await.unwrap().workflow_state,
            AppWorkflowState::Ready
        );

        let workspace = coordinator.workspace_lock(&app.id).await;
        let guard = workspace.lock().await;
        let error = coordinator
            .restore_checkpoint(&app.id, "no-such-checkpoint")
            .await
            .unwrap_err();
        assert!(
            matches!(&error, AppError::WorkflowStateInvalid(detail)
                if detail.contains("generation job")),
            "{error:?}"
        );
        drop(guard);
    }

    #[tokio::test]
    async fn interrupted_job_becomes_retryable_and_active_workflow_becomes_failed() {
        let root = tempfile::tempdir().unwrap();
        let clock = Arc::new(FixedClock::new(1_000));
        let sink = Arc::new(crate::continuation::NoopContinuationSink);
        let service = Arc::new(
            AppService::load(
                root.path(),
                clock.clone(),
                sink,
                Arc::new(NoopAppEventObserver),
            )
            .await
            .unwrap(),
        );
        let app = service
            .create_app(Some("Interrupted"), "a test app", None)
            .await
            .unwrap();
        let app = advance_to_collecting_spec(&service, &app.id).await;
        service
            .update_draft(
                &app.id,
                0,
                &AppDesignPatch {
                    ops: vec![AppDesignPatchOp::Set {
                        field_id: "purpose".into(),
                        value: DesignValue::ShortText("Metrics".into()),
                    }],
                    note: None,
                },
            )
            .await
            .unwrap();
        let gate = service.open_designer(&app.id).await.unwrap();
        stamp_fresh_plan(&service, &app.id).await;
        service
            .confirm_design(&app.id, &gate.interaction_id, 1)
            .await
            .unwrap();
        let layout = AppLayout::new(root.path(), &app.id).unwrap();
        save_jobs(
            &layout,
            &[GenerationJob {
                schema_version: APPS_SCHEMA_VERSION,
                key: GenerationJobKey {
                    app_id: app.id.clone(),
                    revision: 1,
                    continuation_seq: 1,
                },
                kind: GenerationRequestKind::Initial,
                prompt: None,
                status: GenerationJobStatus::Building,
                attempt: 1,
                preview_url: None,
                last_error: None,
                created_at_ms: 1_000,
                updated_at_ms: 1_000,
            }],
        )
        .unwrap();
        let coordinator = AppGenerationCoordinator::new(
            root.path(),
            clock,
            Arc::new(RecordingExecutor::default()),
        );
        coordinator.attach_service(service.clone()).await.unwrap();
        assert_eq!(
            coordinator.jobs_for_app(&app.id).await.unwrap()[0].status,
            GenerationJobStatus::Retryable
        );
        assert_eq!(
            service.record(&app.id).await.unwrap().workflow_state,
            AppWorkflowState::GenerationFailed
        );
    }

    #[test]
    fn durable_key_is_the_required_triple() {
        let a = GenerationJobKey {
            app_id: "app".into(),
            revision: 1,
            continuation_seq: 2,
        };
        let mut b = a.clone();
        assert_eq!(a, b);
        b.continuation_seq += 1;
        assert_ne!(a, b);
    }

    #[test]
    fn generation_file_is_app_scoped() {
        let root = tempfile::tempdir().unwrap();
        let layout = AppLayout::new(root.path(), "app-test").unwrap();
        layout.initialize().unwrap();
        save_jobs(&layout, &[]).unwrap();
        assert!(root
            .path()
            .join(storage::app_dir_rel("app-test"))
            .join(GENERATION_JOBS_FILE)
            .is_file());
    }
}

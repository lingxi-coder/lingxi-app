//! [`AppService`] — the single source of truth for local apps.
//!
//! Owns the in-memory aggregates, all persistence (rebuilding fully from disk
//! at construction), and typed domain-event emission. Every mutation follows
//! clone → transition → persist → commit → emit, so a failed persist never
//! leaves memory ahead of disk, and observers never run while the state lock
//! is held. A separate emission-order lock spans every commit → emit window
//! (and snapshot emissions), so concurrent commands can never deliver events
//! out of commit order — a stale snapshot is never delivered after a newer
//! mutation's events.
//!
//! Event DELIVERY runs in spawned emission tasks, never in the caller's
//! future: a mutating call commits and hands its already-held emission-order
//! guard (plus the events) to a `tokio::spawn`ed task, so cancelling the
//! caller between commit and emission cannot lose events, and an observer
//! that blocks cannot wedge the mutating caller. Ordering still holds because
//! the guard is ACQUIRED in the caller before the commit (tokio's `Mutex` is
//! FIFO-fair, so guards are granted in request order — this fairness is
//! load-bearing) and only RELEASED by the emission task after delivery.
//! Observers must not call back into emitting paths; a task-local reentrancy
//! guard turns that programming error into a loud panic instead of a silent
//! permanent deadlock. All blocking multi-fsync storage I/O runs on the
//! blocking pool (`spawn_blocking`) over owned clones, never on the async
//! executor threads.

use crate::checkpoints::AppCheckpointStore;
use crate::error::AppError;
use crate::events::{AppEvent, AppEventObserver};
use crate::ids;
use crate::manifest::{save_manifest, AppLayout, AppManifest};
use crate::permissions::{save_permissions, save_workspace_permission_settings, AppPermissions};
use crate::state::AppState;
use crate::storage;
use crate::types::{
    AppCheckpoint, AppCheckpointKind, AppDependencyRecord, AppDependencyState, AppRecord,
    AppRuntimeMode, AppRuntimeRecord, AppRuntimeState, AppWorkflowState,
};
use std::collections::{BTreeSet, HashMap};
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::UNIX_EPOCH;
use tokio::sync::{Mutex, OwnedMutexGuard};
use traits::Clock;

tokio::task_local! {
    /// Set for the duration of every event-delivery task; its presence at an
    /// emission-order acquisition site proves an observer called back into an
    /// emitting path (which would self-deadlock the FIFO emission queue).
    static DELIVERING_EVENTS: ();
}

/// Bound on fresh-id collisions before giving up (practically unreachable
/// with 8 random hex chars).
const MAX_ID_MINT_ATTEMPTS: usize = 32;

// ── Input caps ───────────────────────────────────────────────────────────────
// The service is the single enforcement point for client-supplied payload
// sizes. Every violation fails typed `invalid_request` BEFORE anything is
// cloned, persisted, or re-emitted — index.json and the full `AppsChanged`
// record set are rewritten on every mutation, so an uncapped value would be
// amplified on every subsequent edit of any app. String caps are UTF-8 bytes
// (what memory/disk/the wire actually pay).

/// Maximum app name length in bytes (after trimming).
pub const MAX_NAME_BYTES: usize = 200;
/// Maximum app `brief` length in bytes (after trimming). Generous relative to
/// [`MAX_NAME_BYTES`] — the brief is prose the agent reads for context, not a
/// label.
pub const MAX_BRIEF_BYTES: usize = 4_000;
/// Maximum provider-qualified workflow model reference length in bytes.
pub const MAX_WORKFLOW_MODEL_BYTES: usize = 512;
/// Maximum `conversation_id` length in bytes.
pub const MAX_CONVERSATION_ID_BYTES: usize = 128;
/// Maximum text value length in bytes (also the runtime `last_error` cap).
pub const MAX_TEXT_VALUE_BYTES: usize = 20_000;

fn ensure_within(what: &str, len: usize, max: usize) -> Result<(), AppError> {
    if len <= max {
        Ok(())
    } else {
        Err(AppError::InvalidRequest(format!(
            "{what} is {len} bytes (limit {max})"
        )))
    }
}

/// Marker appended when a runtime `last_error` had to be truncated.
const LAST_ERROR_TRUNCATION_MARKER: &str = "… [truncated]";

/// Cap a runtime `last_error` at [`MAX_TEXT_VALUE_BYTES`] by TRUNCATING on a
/// char boundary and appending [`LAST_ERROR_TRUNCATION_MARKER`].
///
/// Every sibling cap REJECTS oversized input (`ensure_within`) because the
/// caller supplied the value and can shrink and retry. `last_error` is
/// different: it REPORTS a runtime failure that already happened — rejecting
/// the report would lose the failure evidence entirely (and leave the
/// runtime record lying about its state), so this one seam truncates instead.
fn truncate_last_error(last_error: Option<String>) -> Option<String> {
    last_error.map(|text| {
        if text.len() <= MAX_TEXT_VALUE_BYTES {
            return text;
        }
        let mut cut = MAX_TEXT_VALUE_BYTES - LAST_ERROR_TRUNCATION_MARKER.len();
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        let mut truncated = text[..cut].to_string();
        truncated.push_str(LAST_ERROR_TRUNCATION_MARKER);
        truncated
    })
}

/// Single source of truth for local apps (data model, runtime state machine,
/// storage, git checkpoints).
pub struct AppService {
    /// `pub(crate)` (not private) solely so [`crate::test_support`]'s
    /// cross-crate test fixtures can splice fixture state onto disk without
    /// a public API surface — production code outside this module has no
    /// business touching it.
    pub(crate) root: PathBuf,
    clock: Arc<dyn Clock>,
    observer: Arc<dyn AppEventObserver>,
    /// `Arc` so completion tasks can hold an [`OwnedMutexGuard`] across a
    /// caller-cancellation boundary (see the module doc). `pub(crate)` for
    /// the same test-fixture reason as `root`.
    pub(crate) state: Arc<Mutex<Vec<AppState>>>,
    /// Serializes every commit → emit window (and snapshot emissions).
    /// Acquired in the CALLER before `state` (so guard-grant order == commit
    /// order — tokio's FIFO-fair `Mutex` is load-bearing here) and released
    /// by the spawned emission task after the events are delivered; the
    /// `state` lock itself is never held while observers run.
    emit_order: Arc<Mutex<()>>,
    /// Every app id this instance ever DELETED. Merged with the live ids
    /// into the `known_ids` set of [`storage::save_index_preserving`], so
    /// the instance stays authoritative for its own deletions (no
    /// resurrection from disk) while foreign-process entries survive its
    /// index rewrites. Plain sync mutex: locked only for short synchronous
    /// sections, never across an await.
    retired_ids: Arc<std::sync::Mutex<BTreeSet<String>>>,
    dependencies: Arc<Mutex<HashMap<String, AppDependencyRecord>>>,
}

impl AppService {
    /// Rebuild the service from disk alone. `root` is the per-profile data
    /// root (the engine derives it from its config; tests inject a tempdir);
    /// app state lives under `<root>/apps/`. The directory is created when
    /// missing; corrupt state fails with `storage_corrupt` instead of being
    /// silently reset. A store torn by a crash between the per-app batch and
    /// the index rewrite is repaired forward here (see the [`crate::storage`]
    /// module doc).
    pub async fn load(
        root: impl Into<PathBuf>,
        clock: Arc<dyn Clock>,
        observer: Arc<dyn AppEventObserver>,
    ) -> Result<Self, AppError> {
        let root = root.into();
        let apps = {
            let root = root.clone();
            Self::run_blocking(move || {
                std::fs::create_dir_all(&root).map_err(|error| {
                    AppError::Io(format!("create data root {}: {error}", root.display()))
                })?;
                // Owner-only root, matching the repo's private-state
                // convention (everything BELOW is already 0o700 via
                // `rooted_fs`; `create_dir_all` alone would leave the root
                // itself at the umask default, typically world-listable).
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))
                        .map_err(|error| {
                            AppError::Io(format!("restrict data root {}: {error}", root.display()))
                        })?;
                }
                storage::load_all(&root)
            })
            .await?
        };
        let dependencies = apps
            .iter()
            .map(|app| {
                storage::load_dependency_record(&root, &app.record)
                    .map(|dependency| (app.record.id.clone(), dependency))
            })
            .collect::<Result<HashMap<_, _>, _>>()?;
        Ok(Self {
            root,
            clock,
            observer,
            state: Arc::new(Mutex::new(apps)),
            emit_order: Arc::new(Mutex::new(())),
            retired_ids: Arc::new(std::sync::Mutex::new(BTreeSet::new())),
            dependencies: Arc::new(Mutex::new(dependencies)),
        })
    }

    /// Run one blocking storage closure on the blocking pool. A join failure
    /// (the blocking task panicked or the runtime is shutting down) surfaces
    /// as `Io` — the closure itself reports its own typed errors.
    async fn run_blocking<T: Send + 'static>(
        work: impl FnOnce() -> Result<T, AppError> + Send + 'static,
    ) -> Result<T, AppError> {
        tokio::task::spawn_blocking(work)
            .await
            .map_err(|error| AppError::Io(format!("storage task failed: {error}")))?
    }

    fn now_ms(&self) -> u64 {
        self.clock
            .now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| {
                u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
            })
    }

    fn position(apps: &[AppState], app_id: &str) -> Result<usize, AppError> {
        apps.iter()
            .position(|app| app.record.id == app_id)
            .ok_or_else(|| AppError::NotFound(format!("app {app_id} does not exist")))
    }

    /// Join the FIFO emission queue. PANICS (all builds — a loud panic beats
    /// a silent permanent deadlock) when called from inside an event-delivery
    /// task: an observer calling back into an emitting path would wait on the
    /// very guard its own delivery task holds.
    async fn acquire_emit_order(&self) -> OwnedMutexGuard<()> {
        assert!(
            DELIVERING_EVENTS.try_with(|()| ()).is_err(),
            "AppEventObserver re-entered an AppService emitting path during event \
             delivery; observers must never call back into the service's emitting \
             methods (this would self-deadlock the emission queue)"
        );
        Arc::clone(&self.emit_order).lock_owned().await
    }

    /// Deliver `events` in a spawned emission task that owns the
    /// emission-order guard, releasing it only after the last observer call —
    /// the caller's future can be dropped at any point without losing the
    /// events or breaking cross-command ordering (module doc).
    fn spawn_emission(
        observer: Arc<dyn AppEventObserver>,
        order: OwnedMutexGuard<()>,
        events: Vec<AppEvent>,
    ) {
        if events.is_empty() {
            drop(order);
            return;
        }
        tokio::spawn(DELIVERING_EVENTS.scope((), async move {
            for event in events {
                observer.on_event(event).await;
            }
            drop(order);
        }));
    }

    /// Barrier for tests and embedders: resolves once every emission task
    /// spawned by PREVIOUSLY COMPLETED calls has delivered its events (it
    /// simply waits for its own turn in the FIFO emission queue). Panics if
    /// called from inside an observer (same reentrancy rule as every
    /// emitting path).
    pub async fn flush_events(&self) {
        drop(self.acquire_emit_order().await);
    }

    /// Run one transition against a clone of the aggregate, persist on
    /// success (changed per-app documents first — in
    /// [`storage::APP_DOC_WRITE_ORDER`] order — index last), then commit to
    /// memory and hand the events to a spawned emission task. The
    /// emission-order guard is acquired before the state lock and travels
    /// with the events, so events reach observers in commit order even under
    /// concurrent commands; the state lock itself is never held while
    /// observers run. On failure nothing is persisted or committed; failure
    /// events still emit. Persist + commit + emission hand-off run in a
    /// spawned completion task, so dropping the caller's future mid-call
    /// either aborts BEFORE any side effect or changes nothing about the
    /// mutation completing, committing, and emitting.
    ///
    /// Only documents that actually changed are rewritten — the load-time
    /// repair contract depends on the ORDER of the writes that happen, not on
    /// every document being rewritten — and the index rewrite is skipped when
    /// the record is unchanged.
    ///
    /// EXACT durability guarantee, and its residual windows, of a call that
    /// returned `Err`:
    ///
    /// - **Mid-batch write failure**: the batch fails between atomic
    ///   per-document writes, so a canonical PREFIX of the changed documents
    ///   holds new content. A compensating rollback rewrites that prefix's
    ///   ORIGINAL documents (reverse order — see
    ///   [`Self::rollback_original_docs`]); when it fully succeeds, disk is
    ///   byte-identical to the pre-transition state and a reload agrees with
    ///   the reported failure.
    /// - **Index write failure after a full batch**: same compensating
    ///   rollback over ALL changed documents, so the mirror-wins repair at
    ///   the next load does not commit the transition the caller saw fail.
    /// - **Residual window (honest)**: if a rollback write ITSELF fails
    ///   (logged), the rollback stops there, leaving new content in exactly a
    ///   canonical prefix of the changed documents — indistinguishable from
    ///   a mid-batch crash, which load-time repair resolves by rolling the
    ///   transition FORWARD. Until that next load (or the next successful
    ///   write of those documents) disk stays ahead of the failure the
    ///   caller observed.
    async fn with_app<T: Send + 'static>(
        &self,
        app_id: &str,
        op: impl FnOnce(&mut AppState, u64) -> (Result<T, AppError>, Vec<AppEvent>),
    ) -> Result<T, AppError> {
        let order = self.acquire_emit_order().await;
        // Timestamp read AFTER joining the FIFO queue: guard-grant order ==
        // commit order, so `updated_at_ms`/`created_at_ms` are monotonic
        // across commits instead of rewinding when a later-stamped caller
        // wins the lock first.
        let now = self.now_ms();
        let mut apps = Arc::clone(&self.state).lock_owned().await;
        let idx = Self::position(&apps, app_id)?;
        let mut working = apps[idx].clone();
        let (result, events) = op(&mut working, now);
        let value = match result {
            Ok(value) => value,
            Err(error) => {
                drop(apps);
                Self::spawn_emission(Arc::clone(&self.observer), order, events);
                return Err(error);
            }
        };
        let changed = Self::changed_doc_steps(&apps[idx], &working);
        let index = if working.record == apps[idx].record {
            None
        } else {
            let mut records: Vec<AppRecord> = apps.iter().map(|app| app.record.clone()).collect();
            records[idx] = working.record.clone();
            Some((records, self.known_ids(&apps)))
        };
        let root = self.root.clone();
        let original = apps[idx].clone();
        let observer = Arc::clone(&self.observer);
        // Completion task: owns both guards; runs persist → commit → emission
        // hand-off to completion even if the caller's future is dropped
        // (there is no await between this spawn and the caller's return, so
        // cancellation can only land before any side effect or after the
        // task exists).
        let completion = tokio::spawn(async move {
            let persisted = {
                let working = working.clone();
                Self::run_blocking(move || {
                    Self::persist_mutation(&root, &original, &working, &changed, index.as_ref())
                })
                .await
            };
            match persisted {
                Ok(()) => {
                    apps[idx] = working;
                    drop(apps);
                    Self::spawn_emission(observer, order, events);
                    Ok(value)
                }
                Err(error) => Err(error),
            }
        });
        completion
            .await
            .map_err(|error| AppError::Io(format!("mutation completion task failed: {error}")))?
    }

    /// The `known_ids` set for [`storage::save_index_preserving`]: every id
    /// this instance currently holds plus every id it ever deleted.
    fn known_ids(&self, apps: &[AppState]) -> BTreeSet<String> {
        let mut known: BTreeSet<String> = apps.iter().map(|app| app.record.id.clone()).collect();
        if let Ok(retired) = self.retired_ids.lock() {
            known.extend(retired.iter().cloned());
        }
        known
    }

    /// Blocking persistence of one committed mutation: changed per-app
    /// documents in canonical order, then (when the record changed) the
    /// locked, foreign-preserving index write. Any failure triggers the
    /// compensating rollback over exactly the steps whose NEW documents
    /// landed, then surfaces the original error. See [`Self::with_app`] for
    /// the guarantee this implements.
    fn persist_mutation(
        root: &std::path::Path,
        original: &AppState,
        working: &AppState,
        changed: &[storage::AppDocWriteStep],
        index: Option<&(Vec<AppRecord>, BTreeSet<String>)>,
    ) -> Result<(), AppError> {
        if let Err(failure) = storage::save_app_files_steps(root, working, changed) {
            Self::rollback_original_docs(root, original, &changed[..failure.written]);
            return Err(failure.error);
        }
        if let Some((records, known)) = index {
            if let Err(error) = storage::save_index_preserving(root, records, known) {
                Self::rollback_original_docs(root, original, changed);
                return Err(error);
            }
        }
        Ok(())
    }

    /// Compensating rollback: rewrite the ORIGINAL documents of exactly the
    /// steps whose NEW versions landed, in REVERSE canonical order, stopping
    /// at the first rollback failure.
    ///
    /// WHY reverse, why stop (both load-bearing):
    ///
    /// - A forward mid-batch crash always leaves NEW content in a canonical
    ///   PREFIX of the write order — the only shape load-time repair has
    ///   arms for. Rolling back in REVERSE restores originals from the END,
    ///   so dying (or stopping) mid-rollback still leaves new content in a
    ///   canonical prefix: the mirror (last in forward order) is reverted
    ///   FIRST, meaning mirror-wins repair adopts the OLD record and any
    ///   still-new earlier documents are exactly the shapes the repair
    ///   already handles. A FORWARD rollback dying mid-way would instead
    ///   leave old-prefix/new-suffix — the mirror still ahead over
    ///   already-reverted documents — a hybrid repair has no arm for (a
    ///   durable wedge).
    /// - Stopping at the first rollback failure preserves the same prefix
    ///   property; continuing past it could revert an EARLIER document while
    ///   a later one stays new, manufacturing the same repair-less hybrid.
    fn rollback_original_docs(
        root: &std::path::Path,
        original: &AppState,
        written: &[storage::AppDocWriteStep],
    ) {
        for step in written.iter().rev() {
            if let Err(failure) =
                storage::save_app_files_steps(root, original, std::slice::from_ref(step))
            {
                tracing::warn!(
                    app_id = %original.record.id,
                    step = ?step,
                    error = %failure.error,
                    "compensating rollback stopped at a failing step; disk keeps a \
                     canonical new-content prefix ahead of memory until the next \
                     load repairs the transition forward"
                );
                return;
            }
        }
    }

    /// The subsequence of [`storage::APP_DOC_WRITE_ORDER`] whose documents
    /// differ between `committed` and `working` — the only writes a mutation
    /// needs (relative order preserved; see [`Self::with_app`]).
    ///
    /// DELIBERATE tradeoff, not an oversight: the full-aggregate clone in
    /// `with_app` is LOAD-BEARING (the compensating rollback needs the
    /// pristine original), and this structural equality is bounded by the
    /// service input caps; dirty flags or copy-on-write would buy speed at
    /// the cost of a second source of truth for "what changed" inside the
    /// crash-repair contract.
    fn changed_doc_steps(
        committed: &AppState,
        working: &AppState,
    ) -> Vec<storage::AppDocWriteStep> {
        storage::APP_DOC_WRITE_ORDER
            .iter()
            .copied()
            .filter(|step| match step {
                storage::AppDocWriteStep::Runtime => committed.runtime != working.runtime,
                storage::AppDocWriteStep::MetadataMirror => committed.record != working.record,
            })
            .collect()
    }

    /// Every app record, in stored order.
    pub async fn list_apps(&self) -> Vec<AppRecord> {
        let apps = self.state.lock().await;
        apps.iter().map(|app| app.record.clone()).collect()
    }

    /// Snapshot the record list and emit it as [`AppEvent::AppsChanged`] —
    /// the engine's `ListApps` / post-mutation snapshot path. The
    /// emission-order lock spans the snapshot AND its delivery, so a stale
    /// snapshot can never be delivered after a newer mutation's events.
    /// Returns the snapshotted records.
    pub async fn announce_apps(&self) -> Vec<AppRecord> {
        let order = self.acquire_emit_order().await;
        let records: Vec<AppRecord> = {
            let apps = self.state.lock().await;
            apps.iter().map(|app| app.record.clone()).collect()
        };
        Self::spawn_emission(
            Arc::clone(&self.observer),
            order,
            vec![AppEvent::AppsChanged {
                apps: records.clone(),
            }],
        );
        records
    }

    /// The record of one app.
    pub async fn record(&self, app_id: &str) -> Result<AppRecord, AppError> {
        let apps = self.state.lock().await;
        let idx = Self::position(&apps, app_id)?;
        Ok(apps[idx].record.clone())
    }

    /// Snapshot every app record without emitting an event. Embedders use
    /// this for durable worker recovery where a client snapshot would be an
    /// unrelated side effect.
    pub async fn records(&self) -> Vec<AppRecord> {
        self.state
            .lock()
            .await
            .iter()
            .map(|app| app.record.clone())
            .collect()
    }

    /// The runtime record of one app.
    pub async fn runtime_record(&self, app_id: &str) -> Result<AppRuntimeRecord, AppError> {
        let apps = self.state.lock().await;
        let idx = Self::position(&apps, app_id)?;
        Ok(apps[idx].runtime.clone())
    }

    /// The dependency-install record of one app.
    pub async fn dependency_record(&self, app_id: &str) -> Result<AppDependencyRecord, AppError> {
        let apps = self.state.lock().await;
        let _ = Self::position(&apps, app_id)?;
        let dependencies = self.dependencies.lock().await;
        dependencies.get(app_id).cloned().ok_or_else(|| {
            AppError::StorageCorrupt(format!("app {app_id} is missing dependencies.json state"))
        })
    }

    async fn update_dependency_record(
        &self,
        app_id: &str,
        op: impl FnOnce(&mut AppDependencyRecord, u64) -> Result<(), AppError>,
    ) -> Result<AppDependencyRecord, AppError> {
        let now = self.now_ms();
        let apps = self.state.lock().await;
        let _ = Self::position(&apps, app_id)?;
        let mut dependencies = self.dependencies.lock().await;
        let mut working = dependencies.get(app_id).cloned().ok_or_else(|| {
            AppError::StorageCorrupt(format!("app {app_id} is missing dependencies.json state"))
        })?;
        op(&mut working, now)?;
        let root = self.root.clone();
        let persisted = working.clone();
        Self::run_blocking(move || storage::save_dependency_record(&root, &persisted)).await?;
        dependencies.insert(app_id.to_string(), working.clone());
        Ok(working)
    }

    /// Move one app back to the dependency queue.
    pub async fn queue_dependency_install(
        &self,
        app_id: &str,
    ) -> Result<AppDependencyRecord, AppError> {
        self.update_dependency_record(app_id, |dependency, now| {
            dependency.state = AppDependencyState::Queued;
            dependency.last_error = None;
            dependency.updated_at_ms = now;
            Ok(())
        })
        .await
    }

    /// Mark that the host has started an install attempt.
    pub async fn start_dependency_install(
        &self,
        app_id: &str,
    ) -> Result<AppDependencyRecord, AppError> {
        self.update_dependency_record(app_id, |dependency, now| {
            if matches!(dependency.state, AppDependencyState::Installing) {
                return Err(AppError::InvalidRequest(format!(
                    "app {app_id} dependency install is already running"
                )));
            }
            dependency.state = AppDependencyState::Installing;
            dependency.install_attempts = dependency.install_attempts.saturating_add(1);
            dependency.last_error = None;
            dependency.updated_at_ms = now;
            Ok(())
        })
        .await
    }

    /// Mark one app's dependencies ready.
    pub async fn complete_dependency_install(
        &self,
        app_id: &str,
    ) -> Result<AppDependencyRecord, AppError> {
        self.complete_dependency_install_with_metadata(app_id, None, None)
            .await
    }

    /// Mark dependencies ready and persist the exact lock/toolchain identity
    /// that produced the app-local tree. Older records remain readable and
    /// are deliberately treated as stale by the mobile host.
    pub async fn complete_dependency_install_with_metadata(
        &self,
        app_id: &str,
        lockfile_sha256: Option<String>,
        toolchain_key: Option<String>,
    ) -> Result<AppDependencyRecord, AppError> {
        self.update_dependency_record(app_id, |dependency, now| {
            dependency.state = AppDependencyState::Ready;
            dependency.last_error = None;
            dependency.lockfile_sha256 = lockfile_sha256;
            dependency.toolchain_key = toolchain_key;
            dependency.updated_at_ms = now;
            Ok(())
        })
        .await
    }

    /// Persist a failed dependency install attempt.
    pub async fn fail_dependency_install(
        &self,
        app_id: &str,
        last_error: impl Into<String>,
    ) -> Result<AppDependencyRecord, AppError> {
        let last_error = last_error.into();
        ensure_within(
            "dependency last_error",
            last_error.len(),
            MAX_TEXT_VALUE_BYTES,
        )?;
        self.update_dependency_record(app_id, move |dependency, now| {
            dependency.state = AppDependencyState::Failed;
            dependency.last_error = Some(last_error);
            dependency.updated_at_ms = now;
            Ok(())
        })
        .await
    }

    /// Git-backed checkpoints of one app, newest first.
    pub async fn list_checkpoints(&self, app_id: &str) -> Result<Vec<AppCheckpoint>, AppError> {
        if !self.git_version_control_enabled(app_id).await? {
            return Ok(Vec::new());
        }
        let layout = AppLayout::new(self.root.clone(), app_id.to_string())?;
        Self::run_blocking(move || AppCheckpointStore::new(&layout).list()).await
    }

    /// Whether this app opted into Git-backed source version control.
    pub async fn git_version_control_enabled(&self, app_id: &str) -> Result<bool, AppError> {
        Ok(self.record(app_id).await?.git_enabled)
    }

    /// Stamp the app `ready` — the host calls this after the first successful
    /// offline build (v3: the objective "this app has runnable output"
    /// signal; there is no preview-approval gate anymore). Idempotent: an
    /// already-ready app is a no-op with no event.
    /// Pin the app's init session (bare uuid). Set-once: the anchor is the
    /// app's durable "set-up conversation" identity, so a second call with a
    /// DIFFERENT id is rejected (idempotent for the same id). The engine
    /// calls this right after minting the session (create) or backfilling a
    /// missing anchor at boot. Announces `AppsChanged` so clients learn the
    /// pin without a details round-trip.
    pub async fn set_init_session(&self, app_id: &str, session_id: &str) -> Result<(), AppError> {
        let session_id = session_id.to_string();
        self.with_app(app_id, move |app, now| match &app.record.init_session_id {
            Some(existing) if *existing == session_id => (Ok(()), Vec::new()),
            Some(existing) => (
                Err(AppError::InvalidRequest(format!(
                    "app {} already has init session {existing}",
                    app.record.id
                ))),
                Vec::new(),
            ),
            None => {
                app.record.init_session_id = Some(session_id.clone());
                app.record.updated_at_ms = now;
                (Ok(()), Vec::new())
            }
        })
        .await?;
        self.announce_apps().await;
        Ok(())
    }

    pub async fn mark_ready(&self, app_id: &str) -> Result<(), AppError> {
        self.with_app(app_id, |app, now| {
            if app.record.workflow_state == AppWorkflowState::Ready {
                return (Ok(()), Vec::new());
            }
            app.record.workflow_state = AppWorkflowState::Ready;
            app.record.updated_at_ms = now;
            (
                Ok(()),
                vec![AppEvent::WorkflowChanged {
                    app_id: app.record.id.clone(),
                    state: app.record.workflow_state,
                    detail: None,
                }],
            )
        })
        .await
    }

    /// Commit the current workspace as a retained Git checkpoint and emit its
    /// domain event after the durable reference has been written.
    pub async fn create_checkpoint(
        &self,
        app_id: &str,
        kind: AppCheckpointKind,
        label: &str,
    ) -> Result<AppCheckpoint, AppError> {
        if !self.git_version_control_enabled(app_id).await? {
            return Err(AppError::NotYetAvailable(
                "Git version control is disabled for this app".into(),
            ));
        }
        {
            let apps = self.state.lock().await;
            Self::position(&apps, app_id)?;
        }
        let order = self.acquire_emit_order().await;
        let layout = AppLayout::new(self.root.clone(), app_id.to_string())?;
        let label = label.to_string();
        let now = self.now_ms();
        let checkpoint = Self::run_blocking(move || {
            let _build_lock = storage::lock_app_build(layout.root(), layout.app_id())?;
            AppCheckpointStore::new(&layout).create(kind, &label, now)
        })
        .await?;
        Self::spawn_emission(
            Arc::clone(&self.observer),
            order,
            vec![AppEvent::CheckpointCreated {
                app_id: app_id.to_string(),
                checkpoint: checkpoint.clone(),
            }],
        );
        Ok(checkpoint)
    }

    /// Restore only the app workspace to a retained checkpoint. A durable
    /// `pre_restore` checkpoint is created first; data/runtime/build paths sit
    /// outside the Git repository and are never reset.
    pub async fn restore_checkpoint(
        &self,
        app_id: &str,
        checkpoint_id: &str,
    ) -> Result<AppCheckpoint, AppError> {
        if !self.git_version_control_enabled(app_id).await? {
            return Err(AppError::NotYetAvailable(
                "Git version control is disabled for this app".into(),
            ));
        }
        {
            let apps = self.state.lock().await;
            Self::position(&apps, app_id)?;
        }
        let order = self.acquire_emit_order().await;
        let layout = AppLayout::new(self.root.clone(), app_id.to_string())?;
        let checkpoint_id = checkpoint_id.to_string();
        let now = self.now_ms();
        let safety = Self::run_blocking(move || {
            let _build_lock = storage::lock_app_build(layout.root(), layout.app_id())?;
            AppCheckpointStore::new(&layout).restore(&checkpoint_id, now)
        })
        .await?;
        Self::spawn_emission(
            Arc::clone(&self.observer),
            order,
            vec![AppEvent::CheckpointCreated {
                app_id: app_id.to_string(),
                checkpoint: safety.clone(),
            }],
        );
        Ok(safety)
    }

    /// Create a new app record (workflow starts in `draft`) and its on-disk
    /// layout, then announce the new list via `AppsChanged`.
    ///
    /// `name` is optional: an app can be created from a one-line `brief`
    /// alone, with the conversation agent renaming it later. A blank or
    /// absent `name` gets a PLACEHOLDER — the brief's first 24 **chars**,
    /// not bytes: truncating a CJK brief on a byte boundary would slice a
    /// multi-byte codepoint in half and produce invalid UTF-8 — so the app
    /// always has a non-empty, presentable name.
    pub async fn create_app(
        &self,
        name: Option<&str>,
        brief: &str,
        conversation_id: Option<String>,
    ) -> Result<AppRecord, AppError> {
        self.create_app_with_initializer(name, brief, conversation_id, |_| async { Ok(()) })
            .await
    }

    /// Create a new app record and run a pre-commit initializer after the
    /// private on-disk skeleton exists but BEFORE the index/memory/event
    /// commit makes the app visible.
    pub async fn create_app_with_initializer<F, Fut>(
        &self,
        name: Option<&str>,
        brief: &str,
        conversation_id: Option<String>,
        initializer: F,
    ) -> Result<AppRecord, AppError>
    where
        F: FnOnce(AppRecord) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), AppError>> + Send + 'static,
    {
        self.create_app_with_git_and_workflow_model_and_initializer(
            name,
            brief,
            conversation_id,
            crate::types::DEFAULT_GIT_VERSION_CONTROL,
            None,
            initializer,
        )
        .await
    }

    /// Create a new app with an explicit Git version-control choice.
    pub async fn create_app_with_git(
        &self,
        name: Option<&str>,
        brief: &str,
        conversation_id: Option<String>,
        git_enabled: bool,
    ) -> Result<AppRecord, AppError> {
        self.create_app_with_git_and_workflow_model_and_initializer(
            name,
            brief,
            conversation_id,
            git_enabled,
            None,
            |_| async { Ok(()) },
        )
        .await
    }

    /// Create a new app with explicit Git and app-build model choices.
    pub async fn create_app_with_git_and_workflow_model(
        &self,
        name: Option<&str>,
        brief: &str,
        conversation_id: Option<String>,
        git_enabled: bool,
        workflow_model: Option<&str>,
    ) -> Result<AppRecord, AppError> {
        self.create_app_with_git_and_workflow_model_and_initializer(
            name,
            brief,
            conversation_id,
            git_enabled,
            workflow_model,
            |_| async { Ok(()) },
        )
        .await
    }

    /// Create a new app with explicit Git/model choices and a pre-commit
    /// initializer that can materialize host-owned scaffold before the app
    /// becomes visible to reloads, snapshots, or observers.
    pub async fn create_app_with_git_and_workflow_model_and_initializer<F, Fut>(
        &self,
        name: Option<&str>,
        brief: &str,
        conversation_id: Option<String>,
        git_enabled: bool,
        workflow_model: Option<&str>,
        initializer: F,
    ) -> Result<AppRecord, AppError>
    where
        F: FnOnce(AppRecord) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), AppError>> + Send + 'static,
    {
        let trimmed_brief = brief.trim();
        if trimmed_brief.is_empty() {
            return Err(AppError::InvalidRequest(
                "app brief must not be empty".into(),
            ));
        }
        ensure_within("app brief", trimmed_brief.len(), MAX_BRIEF_BYTES)?;
        let name = match name
            .map(str::trim)
            .filter(|candidate| !candidate.is_empty())
        {
            Some(candidate) => {
                ensure_within("app name", candidate.len(), MAX_NAME_BYTES)?;
                candidate.to_string()
            }
            None => trimmed_brief.chars().take(24).collect(),
        };
        if let Some(conversation_id) = &conversation_id {
            ensure_within(
                "conversation id",
                conversation_id.len(),
                MAX_CONVERSATION_ID_BYTES,
            )?;
        }
        let workflow_model = workflow_model
            .map(str::trim)
            .filter(|model| !model.is_empty())
            .map(|model| {
                ensure_within("workflow model", model.len(), MAX_WORKFLOW_MODEL_BYTES)?;
                Ok::<_, AppError>(model.to_string())
            })
            .transpose()?;
        let brief = trimmed_brief.to_string();
        let order = self.acquire_emit_order().await;
        // After the queue join, for commit-order-monotonic timestamps (see
        // `with_app`).
        let now = self.now_ms();
        let mut apps = Arc::clone(&self.state).lock_owned().await;
        let existing_ids: Vec<String> = apps.iter().map(|app| app.record.id.clone()).collect();
        let existing_records: Vec<AppRecord> = apps.iter().map(|app| app.record.clone()).collect();
        let known = self.known_ids(&apps);
        let root = self.root.clone();
        let observer = Arc::clone(&self.observer);
        let dependencies = Arc::clone(&self.dependencies);
        // Completion task (see with_app): mint + persist + commit + emission
        // hand-off survive the caller's future being dropped.
        let completion = tokio::spawn(async move {
            let app = Self::run_blocking({
                let root = root.clone();
                move || {
                    let id = Self::mint_app_id(&root, &existing_ids)?;
                    let mut app = AppState::create_with_git(
                        id,
                        name,
                        brief,
                        conversation_id,
                        git_enabled,
                        now,
                    );
                    let app_id = app.record.id.clone();
                    app.record.workflow_model = workflow_model;
                    let layout = AppLayout::new(root.clone(), app.record.id.clone())?;
                    let prepared: Result<AppState, AppError> = (|| {
                        // Per-app files first; the index entry is the commit
                        // point, and the initializer must run on the pinned
                        // scaffold before that point.
                        storage::save_app_files(&root, &app)?;
                        layout.initialize()?;
                        let manifest = AppManifest::for_new_app(
                            app.record.id.clone(),
                            app.record.name.clone(),
                        );
                        save_manifest(&layout, &manifest)?;
                        save_permissions(&layout, &AppPermissions::default())?;
                        save_workspace_permission_settings(&layout)?;
                        storage::save_dependency_record(
                            &root,
                            &storage::default_dependency_record(&app.record.id, now),
                        )?;
                        Ok(app)
                    })();
                    if let Err(ref error) = prepared {
                        Self::best_effort_cleanup_uncommitted_create(
                            &root,
                            &app_id,
                            "skeleton preparation",
                            &error.to_string(),
                        );
                    }
                    prepared
                }
            })
            .await?;
            let record = app.record.clone();
            let initializer_result = match tokio::spawn(initializer(record.clone())).await {
                Ok(result) => result,
                Err(error) => Err(AppError::Io(format!(
                    "create initializer task failed: {error}"
                ))),
            };
            if let Err(error) = initializer_result {
                Self::cleanup_uncommitted_create(
                    root.clone(),
                    record.id.clone(),
                    "initializer",
                    &error,
                )
                .await;
                return Err(error);
            }
            let mut records = existing_records;
            records.push(record.clone());
            if let Err(error) = Self::run_blocking({
                let root = root.clone();
                let records = records.clone();
                let known = known.clone();
                move || storage::save_index_preserving(&root, &records, &known)
            })
            .await
            {
                Self::cleanup_uncommitted_create(
                    root.clone(),
                    record.id.clone(),
                    "index commit",
                    &error,
                )
                .await;
                return Err(error);
            }
            apps.push(app);
            dependencies.lock().await.insert(
                record.id.clone(),
                storage::default_dependency_record(&record.id, now),
            );
            drop(apps);
            Self::spawn_emission(
                observer,
                order,
                vec![AppEvent::AppsChanged { apps: records }],
            );
            Ok(record)
        });
        completion
            .await
            .map_err(|error| AppError::Io(format!("create completion task failed: {error}")))?
    }

    fn best_effort_cleanup_uncommitted_create(
        root: &std::path::Path,
        app_id: &str,
        stage: &str,
        failure: &str,
    ) {
        if let Err(cleanup_error) = storage::delete_app_dir(root, app_id) {
            tracing::warn!(
                app_id,
                stage,
                failure,
                cleanup_error = %cleanup_error,
                "failed to clean up an uncommitted app after create failure"
            );
        }
    }

    async fn cleanup_uncommitted_create(
        root: PathBuf,
        app_id: String,
        stage: &'static str,
        failure: &AppError,
    ) {
        let failure = failure.to_string();
        let cleanup_app_id = app_id.clone();
        let cleanup_failure = failure.clone();
        let cleanup = Self::run_blocking(move || {
            Self::best_effort_cleanup_uncommitted_create(
                &root,
                &cleanup_app_id,
                stage,
                &cleanup_failure,
            );
            Ok(())
        })
        .await;
        if let Err(cleanup_join) = cleanup {
            tracing::warn!(
                app_id,
                stage,
                failure,
                cleanup_join = %cleanup_join,
                "failed to run cleanup for an uncommitted app after create failure"
            );
        }
    }

    /// Mint a fresh app id that collides with neither the in-memory list nor
    /// ANYTHING still on disk — a live `apps/<id>` entry (e.g. an orphan a
    /// failed removal left behind) or a `.trash` tombstone of an in-flight
    /// deletion. Disk presence must be checked because ids are minted from
    /// memory while directories die asynchronously: without it a create
    /// racing a stale removal could adopt (and then lose) the dying
    /// directory.
    fn mint_app_id(root: &std::path::Path, existing: &[String]) -> Result<String, AppError> {
        for _ in 0..MAX_ID_MINT_ATTEMPTS {
            let id = ids::generate_app_id();
            if existing.iter().any(|existing| *existing == id) {
                continue;
            }
            if storage::app_id_present_on_disk(root, &id) {
                continue;
            }
            return Ok(id);
        }
        Err(AppError::Io("failed to mint a unique app id".into()))
    }

    /// Delete an app: index entry first (commit point), then the contained
    /// `apps/<id>` directory via rename-to-trash + removal as best-effort
    /// cleanup — once the index rewrite has committed (and `AppsChanged`
    /// announced it), a directory-removal failure is logged and the leftover
    /// (an orphan dir or a trash tombstone, both invisible to index-driven
    /// loads; tombstones are swept at the next load) never mis-reports the
    /// committed deletion as failed. Refused while the runtime record says
    /// the app is starting/running/stopping.
    pub async fn delete_app(&self, app_id: &str) -> Result<(), AppError> {
        ids::validate_app_id(app_id)?;
        let order = self.acquire_emit_order().await;
        let mut apps = Arc::clone(&self.state).lock_owned().await;
        let idx = Self::position(&apps, app_id)?;
        let runtime_state = apps[idx].runtime.state;
        if matches!(
            runtime_state,
            AppRuntimeState::Starting | AppRuntimeState::Running | AppRuntimeState::Stopping
        ) {
            return Err(AppError::RuntimeBusy(format!(
                "app {app_id} runtime is {runtime_state}; stop it before deleting"
            )));
        }
        let records: Vec<AppRecord> = apps
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != idx)
            .map(|(_, app)| app.record.clone())
            .collect();
        let known = self.known_ids(&apps);
        let root = self.root.clone();
        let observer = Arc::clone(&self.observer);
        let retired = Arc::clone(&self.retired_ids);
        let dependencies = Arc::clone(&self.dependencies);
        let app_id = app_id.to_string();
        let completion = tokio::spawn(async move {
            {
                let root = root.clone();
                let records = records.clone();
                Self::run_blocking(move || storage::save_index_preserving(&root, &records, &known))
                    .await?;
            }
            // The instance stays authoritative for this id forever: a future
            // index write must not resurrect it from a foreign copy.
            if let Ok(mut retired) = retired.lock() {
                retired.insert(app_id.clone());
            }
            apps.remove(idx);
            dependencies.lock().await.remove(&app_id);
            drop(apps);
            Self::spawn_emission(
                observer,
                order,
                vec![AppEvent::AppsChanged { apps: records }],
            );
            let removal = Self::run_blocking(move || storage::delete_app_dir(&root, &app_id)).await;
            if let Err(error) = removal {
                tracing::warn!(
                    error = %error,
                    "deleted app's directory could not be fully removed; leaving \
                     orphan/tombstone for the load-time sweep"
                );
            }
            Ok(())
        });
        completion
            .await
            .map_err(|error| AppError::Io(format!("delete completion task failed: {error}")))?
    }

    /// Update the runtime record (spec §C transition table). Emits
    /// `RuntimeChanged` and persists `runtime.json`. A port, once assigned,
    /// is never reassigned. An oversized `last_error` is TRUNCATED, not
    /// rejected (see [`truncate_last_error`]).
    ///
    /// ⚠️ GATE: before wiring a live process manager onto this method,
    /// replace [`crate::storage`]'s unconditional load-time reconciliation
    /// (`reconcile_runtime_at_load` — it assumes NO runtime survives a
    /// restart and stamps every busy record failed/stopped). With dev
    /// servers that outlive the engine process it would mis-fail live
    /// runtimes, unpin their ports, and let `delete_app` pull a workspace
    /// out from under a running server.
    pub async fn update_runtime_record(
        &self,
        app_id: &str,
        state: AppRuntimeState,
        port: Option<u16>,
        pid: Option<u32>,
        last_error: Option<String>,
    ) -> Result<AppRuntimeRecord, AppError> {
        let last_error = truncate_last_error(last_error);
        self.with_app(app_id, move |app, now| {
            match app.set_runtime(state, port, pid, last_error, now) {
                Ok(()) => {
                    let runtime = app.runtime.clone();
                    let event = AppEvent::RuntimeChanged {
                        app_id: app.record.id.clone(),
                        runtime: runtime.clone(),
                    };
                    (Ok(runtime), vec![event])
                }
                Err(error) => (Err(error), Vec::new()),
            }
        })
        .await
    }

    /// Persist the Store/Play vs Full/Direct runtime mode selected by the
    /// native distribution before starting a loopback server.
    pub async fn set_runtime_mode(
        &self,
        app_id: &str,
        mode: AppRuntimeMode,
    ) -> Result<AppRuntimeRecord, AppError> {
        self.with_app(app_id, move |app, now| {
            app.set_runtime_mode(mode, now);
            let runtime = app.runtime.clone();
            (
                Ok(runtime.clone()),
                vec![AppEvent::RuntimeChanged {
                    app_id: app.record.id.clone(),
                    runtime,
                }],
            )
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::AppErrorCode;
    use crate::events::{NoopAppEventObserver, RecordingAppEventObserver};
    use crate::test_support::FixedClock;
    use crate::types::AppDependencyState;
    use crate::types::AppWorkflowState;
    use std::path::Path;
    use std::time::Duration;
    use tokio::time::timeout;

    struct Harness {
        service: AppService,
        observer: Arc<RecordingAppEventObserver>,
    }

    impl Harness {
        /// Flush the async emission queue, then drain the observed events.
        /// Event delivery runs in spawned emission tasks, so every take must
        /// wait for completed deliveries first.
        async fn take_events(&self) -> Vec<AppEvent> {
            self.service.flush_events().await;
            self.observer.take()
        }
    }

    async fn harness(root: &Path) -> Harness {
        let observer = Arc::new(RecordingAppEventObserver::new());
        let service = AppService::load(
            root,
            Arc::new(FixedClock::new(1_700_000_000_000)),
            Arc::clone(&observer) as Arc<dyn AppEventObserver>,
        )
        .await
        .unwrap();
        Harness { service, observer }
    }

    /// A ready [`AppService`] over its own throwaway directory, for tests
    /// that only need `service.*` calls and don't care about the observer
    /// (see [`harness`] for those). The directory is leaked (never cleaned
    /// up — `TempDir::into_path` disarms its drop-time removal) so it stays
    /// alive for the rest of the test process; harmless in a test binary.
    async fn test_service() -> AppService {
        let root = tempfile::tempdir().unwrap().into_path();
        harness(&root).await.service
    }

    /// Rebuild a fresh [`AppService`] over the SAME on-disk root as
    /// `service`, standing in for a process restart. `root` is
    /// `pub(crate)`-visible for exactly this reason (module doc).
    async fn reload_service(service: &AppService) -> AppService {
        AppService::load(
            service.root.clone(),
            Arc::new(FixedClock::new(1_700_000_000_000)),
            Arc::new(NoopAppEventObserver) as Arc<dyn AppEventObserver>,
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn create_list_and_reload_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let record = h
            .service
            .create_app(
                Some("  Habit Tracker  "),
                "a test app",
                Some("conv-1".into()),
            )
            .await
            .unwrap();
        assert_eq!(record.name, "Habit Tracker", "name is trimmed");
        assert_eq!(record.workflow_state, AppWorkflowState::Draft);
        assert_eq!(
            record.workspace_rel,
            format!("apps/{}/workspace", record.id)
        );
        let settings: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(
                dir.path()
                    .join(&record.workspace_rel)
                    .join(".lingxi/settings.local.json"),
            )
            .expect("create writes workspace-local permission settings"),
        )
        .expect("workspace-local permission settings are valid JSON");
        assert_eq!(
            settings["permissions"]["allow"],
            serde_json::json!(["Read(./**)", "Edit(./**)"])
        );
        assert!(ids::is_valid_app_id(&record.id));
        assert_eq!(h.service.list_apps().await, vec![record.clone()]);
        let events = h.take_events().await;
        assert!(matches!(&events[..], [AppEvent::AppsChanged { apps }] if apps.len() == 1));

        // Rebuild from disk alone: everything survives byte-identically.
        drop(h);
        let h2 = harness(dir.path()).await;
        assert_eq!(h2.service.list_apps().await, vec![record.clone()]);
        assert_eq!(
            h2.service.runtime_record(&record.id).await.unwrap().state,
            AppRuntimeState::Stopped
        );
    }

    #[tokio::test]
    async fn create_treats_a_blank_name_as_absent_and_uses_a_placeholder() {
        // A blank `name` is not an error: an app can be created from a brief
        // alone, so a whitespace-only `name` is filtered exactly like `None`
        // and falls back to the brief-derived placeholder.
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let record = h
            .service
            .create_app(Some("   "), "a test app", None)
            .await
            .expect("a blank name is treated as absent, not rejected");
        assert!(!record.name.trim().is_empty());
        assert_eq!(h.service.list_apps().await, vec![record]);
    }

    #[tokio::test]
    async fn unknown_app_is_not_found_everywhere() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let missing = "beadfeed";
        assert_eq!(
            h.service.record(missing).await.unwrap_err().code(),
            AppErrorCode::NotFound
        );
        assert_eq!(
            h.service.mark_ready(missing).await.unwrap_err().code(),
            AppErrorCode::NotFound
        );
        assert_eq!(
            h.service
                .update_runtime_record(missing, AppRuntimeState::Starting, None, None, None)
                .await
                .unwrap_err()
                .code(),
            AppErrorCode::NotFound
        );
        assert_eq!(
            h.service.delete_app(missing).await.unwrap_err().code(),
            AppErrorCode::NotFound
        );
        assert_eq!(
            h.service
                .list_checkpoints(missing)
                .await
                .unwrap_err()
                .code(),
            AppErrorCode::NotFound
        );
    }

    #[tokio::test]
    async fn mark_ready_stamps_ready_emits_once_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let record = h
            .service
            .create_app(Some("Ready"), "a test app", None)
            .await
            .unwrap();
        assert_eq!(record.workflow_state, AppWorkflowState::Draft);
        let _ = h.take_events().await;

        h.service.mark_ready(&record.id).await.unwrap();
        assert_eq!(
            h.service.record(&record.id).await.unwrap().workflow_state,
            AppWorkflowState::Ready
        );
        assert_eq!(
            h.take_events().await,
            vec![AppEvent::WorkflowChanged {
                app_id: record.id.clone(),
                state: AppWorkflowState::Ready,
                detail: None,
            }]
        );

        // Idempotent: a second call is Ok with NO event.
        h.service.mark_ready(&record.id).await.unwrap();
        assert!(h.take_events().await.is_empty());

        // The stamp is persisted: a fresh process still sees `ready`.
        let reloaded = reload_service(&h.service).await;
        assert_eq!(
            reloaded.record(&record.id).await.unwrap().workflow_state,
            AppWorkflowState::Ready
        );
    }

    #[tokio::test]
    async fn delete_app_removes_record_and_directory() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let keep = h
            .service
            .create_app(Some("Keep"), "a test app", None)
            .await
            .unwrap();
        let gone = h
            .service
            .create_app(Some("Gone"), "a test app", None)
            .await
            .unwrap();
        let _ = h.take_events().await;
        h.service.delete_app(&gone.id).await.unwrap();
        assert_eq!(h.service.list_apps().await, vec![keep.clone()]);
        assert!(!dir.path().join("apps").join(&gone.id).exists());
        assert!(dir.path().join("apps").join(&keep.id).is_dir());
        let events = h.take_events().await;
        assert!(matches!(&events[..], [AppEvent::AppsChanged { apps }] if apps.len() == 1));
        // Survives reload.
        drop(h);
        let h2 = harness(dir.path()).await;
        assert_eq!(h2.service.list_apps().await, vec![keep]);
    }

    #[tokio::test]
    async fn delete_rejects_traversal_ids_and_busy_runtimes() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let err = h.service.delete_app("../../etc").await.unwrap_err();
        assert_eq!(err.code(), AppErrorCode::InvalidRequest);

        let record = h
            .service
            .create_app(Some("Busy"), "a test app", None)
            .await
            .unwrap();
        h.service
            .update_runtime_record(
                &record.id,
                AppRuntimeState::Starting,
                Some(3001),
                None,
                None,
            )
            .await
            .unwrap();
        let err = h.service.delete_app(&record.id).await.unwrap_err();
        assert_eq!(err.code(), AppErrorCode::RuntimeBusy);
        assert_eq!(h.service.list_apps().await.len(), 1);
    }

    /// Injection mechanism: plant a DIRECTORY at a document's final path.
    /// `atomic_write` refuses to rename onto a non-regular file for EVERY
    /// uid (root included — unlike the old chmod probes, which were vacuous
    /// under root/CI), so the write fails deterministically. Returns the
    /// displaced document body for [`unsquat_document`].
    fn squat_document(path: &std::path::Path) -> String {
        let original = std::fs::read_to_string(path).unwrap();
        std::fs::remove_file(path).unwrap();
        std::fs::create_dir(path).unwrap();
        original
    }

    /// Undo [`squat_document`]: drop the squatting directory (removing any
    /// orphan temp files a failed write left inside it) and restore the
    /// original document body.
    fn unsquat_document(path: &std::path::Path, original: &str) {
        std::fs::remove_dir_all(path).unwrap();
        std::fs::write(path, original).unwrap();
    }

    #[tokio::test]
    async fn delete_app_commits_even_when_directory_removal_fails() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let record = h
            .service
            .create_app(Some("Stuck"), "a test app", None)
            .await
            .unwrap();
        let _ = h.take_events().await;
        let app_dir = dir.path().join("apps").join(&record.id);
        // A regular FILE squatting on `apps/.trash` makes the commit-point
        // rename of the trash-based removal fail for EVERY uid
        // (creating/renaming through a file path is impossible even for
        // root) — no chmod probe, no vacuous early return.
        std::fs::write(dir.path().join("apps/.trash"), b"squat").unwrap();

        // The deletion still succeeds — the index rewrite is the commit
        // point; directory removal is best-effort cleanup.
        h.service.delete_app(&record.id).await.unwrap();
        assert!(h.service.list_apps().await.is_empty());
        let events = h.take_events().await;
        assert!(matches!(&events[..], [AppEvent::AppsChanged { apps }] if apps.is_empty()));
        // The orphan directory is left behind but invisible to a reload.
        assert!(app_dir.exists());
        drop(h);
        let h2 = harness(dir.path()).await;
        assert!(h2.service.list_apps().await.is_empty());
        // The orphan's id can never be re-minted while its directory exists.
        assert!(storage::app_id_present_on_disk(dir.path(), &record.id));
    }

    /// The emission-order lock spans every snapshot AND its delivery, so
    /// under concurrent creates a delivered `AppsChanged` can never show
    /// FEWER apps than one delivered before it (events arrive in commit
    /// order).
    #[tokio::test]
    async fn concurrent_snapshots_are_delivered_in_commit_order() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let service = Arc::new(h.service);
        for _ in 0..10 {
            let snapshotter = {
                let service = Arc::clone(&service);
                tokio::spawn(async move {
                    service.announce_apps().await;
                })
            };
            let creator = {
                let service = Arc::clone(&service);
                tokio::spawn(async move {
                    service
                        .create_app(Some("Race"), "a test app", None)
                        .await
                        .unwrap();
                })
            };
            let (a, b) = tokio::join!(snapshotter, creator);
            a.unwrap();
            b.unwrap();
        }
        service.flush_events().await;
        let mut last_len = 0usize;
        for event in h.observer.take() {
            if let AppEvent::AppsChanged { apps } = event {
                assert!(
                    apps.len() >= last_len,
                    "a snapshot delivered after a commit must not show fewer apps \
                     ({} after {last_len})",
                    apps.len()
                );
                last_len = apps.len();
            }
        }
        assert_eq!(last_len, 10, "every create's announcement was delivered");
    }

    #[tokio::test]
    async fn runtime_updates_persist_and_emit() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let record = h
            .service
            .create_app(Some("R"), "a test app", None)
            .await
            .unwrap();
        let _ = h.take_events().await;
        let runtime = h
            .service
            .update_runtime_record(
                &record.id,
                AppRuntimeState::Starting,
                Some(3010),
                Some(9),
                None,
            )
            .await
            .unwrap();
        assert_eq!(runtime.state, AppRuntimeState::Starting);
        assert_eq!(runtime.port, Some(3010));
        let events = h.take_events().await;
        assert_eq!(
            events,
            vec![AppEvent::RuntimeChanged {
                app_id: record.id.clone(),
                runtime: runtime.clone(),
            }]
        );
        // Illegal transition is refused and not persisted.
        let err = h
            .service
            .update_runtime_record(&record.id, AppRuntimeState::Stopped, None, None, None)
            .await
            .unwrap_err();
        assert_eq!(err.code(), AppErrorCode::InvalidRequest);
        drop(h);
        // A reload is a fresh process: no runtime process outlives the
        // engine, so a busy state found at load is a crash leftover and is
        // reconciled — it must NOT survive as `starting`.
        let h2 = harness(dir.path()).await;
        let reloaded = h2.service.runtime_record(&record.id).await.unwrap();
        assert_eq!(reloaded.state, AppRuntimeState::Failed);
        assert_eq!(
            reloaded.last_error.as_deref(),
            Some("reconciled at load: no live runtime manager")
        );
        assert_eq!(
            reloaded.port,
            Some(3010),
            "the port pin survives reconciliation"
        );
    }

    #[tokio::test]
    async fn set_runtime_mode_persists_and_emits() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let record = h
            .service
            .create_app(Some("Mode"), "a test app", None)
            .await
            .unwrap();
        let _ = h.take_events().await;
        let runtime = h
            .service
            .set_runtime_mode(&record.id, AppRuntimeMode::StaticExport)
            .await
            .unwrap();
        assert_eq!(runtime.mode, Some(AppRuntimeMode::StaticExport));
        assert_eq!(
            h.take_events().await,
            vec![AppEvent::RuntimeChanged {
                app_id: record.id.clone(),
                runtime,
            }]
        );
        let reloaded = reload_service(&h.service).await;
        assert_eq!(
            reloaded.runtime_record(&record.id).await.unwrap().mode,
            Some(AppRuntimeMode::StaticExport)
        );
    }

    /// The data root itself is owner-only on unix, matching the repo's
    /// private-state convention (the interior was already 0o700 via
    /// `rooted_fs`; `create_dir_all` alone left the root at the umask
    /// default).
    #[cfg(unix)]
    #[tokio::test]
    async fn data_root_is_owner_only_on_unix() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("profile-data");
        let _h = harness(&root).await;
        let mode = std::fs::metadata(&root).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "data root must be owner-only");
    }

    #[tokio::test]
    async fn checkpoints_are_empty_for_a_new_app() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let record = h
            .service
            .create_app(Some("C"), "a test app", None)
            .await
            .unwrap();
        assert!(h
            .service
            .list_checkpoints(&record.id)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn git_version_control_choice_persists_and_disables_checkpoints() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let record = h
            .service
            .create_app_with_git(Some("No Git"), "an app without Git", None, false)
            .await
            .unwrap();

        assert!(!record.git_enabled);
        assert!(!h
            .service
            .git_version_control_enabled(&record.id)
            .await
            .unwrap());
        assert!(h
            .service
            .list_checkpoints(&record.id)
            .await
            .unwrap()
            .is_empty());
        let error = h
            .service
            .create_checkpoint(
                &record.id,
                AppCheckpointKind::UserApproved,
                "must not create",
            )
            .await
            .unwrap_err();
        assert_eq!(error.code(), AppErrorCode::NotYetAvailable);

        let reloaded = reload_service(&h.service).await;
        assert!(!reloaded.record(&record.id).await.unwrap().git_enabled);
    }

    /// The store's own record mirror lives inside `apps/<id>/workspace`,
    /// which is exactly the tree a checkpoint restore hard-resets. Nothing
    /// rewrites the mirror after a restore — persistence compares in-memory
    /// `committed` against in-memory `working`, so a mirror rewound ON DISK
    /// is invisible to it — which is why the restore must not be able to
    /// rewind it in the first place. Asserted through a real reload from
    /// disk (mirror-wins repair would adopt a rewound mirror).
    #[tokio::test]
    async fn restore_checkpoint_does_not_rewind_the_record_mirror_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let record = h
            .service
            .create_app(Some("Restorable"), "a test app", None)
            .await
            .unwrap();
        // Checkpoint while the app is still `draft`…
        let first = h
            .service
            .create_checkpoint(
                &record.id,
                AppCheckpointKind::ScaffoldCreated,
                "Scaffold created",
            )
            .await
            .unwrap();
        // …then advance the record past the checkpoint.
        h.service.mark_ready(&record.id).await.unwrap();

        h.service
            .restore_checkpoint(&record.id, &first.id)
            .await
            .unwrap();

        drop(h);
        let h2 = harness(dir.path()).await;
        assert_eq!(
            h2.service.record(&record.id).await.unwrap().workflow_state,
            AppWorkflowState::Ready,
            "the restore must not rewind the record mirror to its checkpoint-era state"
        );
    }

    /// The same contract for the population that actually has data: an app
    /// whose checkpoint history was written before the service documents
    /// were excluded. Its checkpoint tree still carries `.lingxi/app.json`,
    /// so the restore's hard reset would check that blob back out — and
    /// nothing rewrites the mirror afterwards, so the loss would be silent
    /// and permanent. The fixture commits the documents with the pre-fix
    /// `add_all(["*"])` semantics, which is the only way to reach the state.
    #[tokio::test]
    async fn restoring_a_legacy_checkpoint_does_not_rewind_the_record_mirror() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let record = h
            .service
            .create_app(Some("Legacy"), "a test app", None)
            .await
            .unwrap();
        let workspace = dir.path().join(&record.workspace_rel);
        let legacy = crate::checkpoints::seed_legacy_checkpoint(&workspace, 1_000);

        h.service.mark_ready(&record.id).await.unwrap();

        h.service
            .restore_checkpoint(&record.id, &legacy)
            .await
            .unwrap();

        drop(h);
        let h2 = harness(dir.path()).await;
        assert_eq!(
            h2.service.record(&record.id).await.unwrap().workflow_state,
            AppWorkflowState::Ready,
            "a legacy checkpoint's tracked .lingxi blobs must not rewind the mirror"
        );
    }

    #[tokio::test]
    async fn create_app_enforces_name_and_conversation_caps() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let err = h
            .service
            .create_app(Some(&"x".repeat(MAX_NAME_BYTES + 1)), "a test app", None)
            .await
            .unwrap_err();
        assert_eq!(err.code(), AppErrorCode::InvalidRequest);
        let err = h
            .service
            .create_app(
                Some("A"),
                "a test app",
                Some("c".repeat(MAX_CONVERSATION_ID_BYTES + 1)),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code(), AppErrorCode::InvalidRequest);
        assert!(
            h.service.list_apps().await.is_empty(),
            "nothing was created"
        );
        // Exactly at the cap is fine.
        h.service
            .create_app(
                Some(&"x".repeat(MAX_NAME_BYTES)),
                "a test app",
                Some("c".repeat(MAX_CONVERSATION_ID_BYTES)),
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn create_app_persists_workflow_model_in_app_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let record = h
            .service
            .create_app_with_git_and_workflow_model(
                Some("Habits"),
                "Track daily habits",
                None,
                true,
                Some("deepseek/deepseek-v4-flash"),
            )
            .await
            .unwrap();
        assert_eq!(
            record.workflow_model.as_deref(),
            Some("deepseek/deepseek-v4-flash")
        );

        let metadata = std::fs::read_to_string(
            dir.path()
                .join(&record.workspace_rel)
                .join(".lingxi/app.json"),
        )
        .unwrap();
        let metadata: serde_json::Value = serde_json::from_str(&metadata).unwrap();
        assert_eq!(
            metadata
                .pointer("/app/workflowModel")
                .and_then(serde_json::Value::as_str),
            Some("deepseek/deepseek-v4-flash")
        );

        drop(h);
        let reloaded = harness(dir.path()).await;
        assert_eq!(
            reloaded
                .service
                .record(&record.id)
                .await
                .unwrap()
                .workflow_model
                .as_deref(),
            Some("deepseek/deepseek-v4-flash")
        );
    }

    #[tokio::test]
    async fn dependency_records_persist_their_state_machine() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let record = h
            .service
            .create_app(Some("Deps"), "a test app", None)
            .await
            .expect("create");

        let queued = h
            .service
            .dependency_record(&record.id)
            .await
            .expect("queued");
        assert_eq!(queued.state, AppDependencyState::Queued);
        assert_eq!(queued.install_attempts, 0);

        let installing = h
            .service
            .start_dependency_install(&record.id)
            .await
            .expect("installing");
        assert_eq!(installing.state, AppDependencyState::Installing);
        assert_eq!(installing.install_attempts, 1);

        let failed = h
            .service
            .fail_dependency_install(&record.id, "offline")
            .await
            .expect("failed");
        assert_eq!(failed.state, AppDependencyState::Failed);
        assert_eq!(failed.last_error.as_deref(), Some("offline"));

        h.service
            .queue_dependency_install(&record.id)
            .await
            .expect("requeue");
        let ready = h
            .service
            .complete_dependency_install(&record.id)
            .await
            .expect("ready");
        assert_eq!(ready.state, AppDependencyState::Ready);

        let reloaded = reload_service(&h.service).await;
        assert_eq!(
            reloaded
                .dependency_record(&record.id)
                .await
                .expect("reloaded")
                .state,
            AppDependencyState::Ready
        );
    }

    /// Direct coverage for `brief`'s validation (empty-after-trim rejected,
    /// `MAX_BRIEF_BYTES` enforced, exactly-at-cap accepted, and the trimmed
    /// value is what's persisted).
    #[tokio::test]
    async fn create_app_enforces_brief_caps() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        // Empty after trim is rejected, independent of `name`.
        let err = h
            .service
            .create_app(Some("A"), "   ", None)
            .await
            .unwrap_err();
        assert_eq!(err.code(), AppErrorCode::InvalidRequest);
        // Over the cap is rejected.
        let err = h
            .service
            .create_app(Some("A"), &"x".repeat(MAX_BRIEF_BYTES + 1), None)
            .await
            .unwrap_err();
        assert_eq!(err.code(), AppErrorCode::InvalidRequest);
        assert!(
            h.service.list_apps().await.is_empty(),
            "nothing was created"
        );
        // Exactly at the cap is fine, and leading/trailing whitespace is
        // trimmed the same way `name` is before persisting.
        let record = h
            .service
            .create_app(
                Some("A"),
                &format!("  {}  ", "x".repeat(MAX_BRIEF_BYTES)),
                None,
            )
            .await
            .unwrap();
        assert_eq!(record.brief, "x".repeat(MAX_BRIEF_BYTES));
    }

    #[tokio::test]
    async fn create_app_without_a_name_uses_a_placeholder() {
        let service = test_service().await;
        let record = service
            .create_app(None, "一个记事本 app", None)
            .await
            .expect("brief alone is enough to create");
        assert_eq!(record.brief, "一个记事本 app");
        assert!(
            !record.name.trim().is_empty(),
            "a placeholder name is always present"
        );
        assert_eq!(record.workflow_state, AppWorkflowState::Draft);
    }

    /// The placeholder-name rule cuts at 24 CHARS, not 24 bytes — a byte
    /// truncation of a CJK brief would slice a multi-byte codepoint in half.
    /// `"一个记事本 app"` above (8 chars, 19 bytes — under BOTH a 24-char and
    /// a 24-byte cut) can't tell the two implementations apart; this uses a
    /// brief long enough, in an ALL-multi-byte script, that a byte-boundary
    /// bug would produce a visibly different (or panicking) result.
    #[tokio::test]
    async fn create_app_placeholder_name_truncates_by_char_not_by_byte() {
        let service = test_service().await;
        let brief = "记".repeat(30);
        let record = service
            .create_app(None, &brief, None)
            .await
            .expect("create");
        assert_eq!(
            record.name.chars().count(),
            24,
            "the placeholder is exactly 24 CHARACTERS"
        );
        assert_eq!(record.name, "记".repeat(24));
        assert_eq!(
            record.name.len(),
            24 * "记".len(),
            "24 three-byte chars is 72 bytes, not 24 — a byte-boundary cut \
             would have stopped after 8 whole chars"
        );
    }

    #[tokio::test]
    async fn create_app_rejects_an_empty_brief() {
        let service = test_service().await;
        service
            .create_app(Some("Notes"), "   ", None)
            .await
            .expect_err("an empty brief is rejected");
    }

    #[tokio::test]
    async fn create_app_library_is_not_artificially_capped() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        for i in 0..101 {
            h.service
                .create_app(Some(&format!("App {i}")), "a test app", None)
                .await
                .unwrap();
        }
        assert_eq!(h.service.list_apps().await.len(), 101);
    }

    #[tokio::test]
    async fn failed_create_initializer_is_invisible_after_reload_and_emits_no_apps_changed() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let failed_app_id = Arc::new(std::sync::Mutex::new(None::<String>));
        let failed_app_id_for_initializer = Arc::clone(&failed_app_id);

        let error = h
            .service
            .create_app_with_initializer(Some("Ghost"), "a test app", None, move |record| {
                let failed_app_id = Arc::clone(&failed_app_id_for_initializer);
                async move {
                    *failed_app_id.lock().expect("lock app id") = Some(record.id);
                    Err(AppError::Io("initializer failed".into()))
                }
            })
            .await
            .expect_err("initializer failure must fail the create");

        let failed_app_id = failed_app_id
            .lock()
            .expect("lock app id")
            .clone()
            .expect("initializer observed a minted id");
        assert_eq!(error.code(), AppErrorCode::Io, "{error}");
        assert!(
            h.service.list_apps().await.is_empty(),
            "failed create must not publish an in-memory app"
        );
        assert!(
            h.take_events().await.is_empty(),
            "failed create must not emit AppsChanged"
        );
        assert!(
            !dir.path().join("apps/index.json").exists(),
            "the commit-point index must stay absent when the initializer fails"
        );
        assert!(
            !dir.path().join("apps").join(&failed_app_id).exists(),
            "failed create must clean the exact unindexed app directory"
        );

        let reloaded = reload_service(&h.service).await;
        assert!(
            reloaded.list_apps().await.is_empty(),
            "reload must not discover the failed create"
        );
    }

    #[tokio::test]
    async fn panicking_create_initializer_cleans_the_uncommitted_app() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let failed_app_id = Arc::new(std::sync::Mutex::new(None::<String>));
        let failed_app_id_for_initializer = Arc::clone(&failed_app_id);

        let error = h
            .service
            .create_app_with_initializer(Some("Panic"), "a test app", None, move |record| {
                let failed_app_id = Arc::clone(&failed_app_id_for_initializer);
                async move {
                    *failed_app_id.lock().expect("lock app id") = Some(record.id);
                    panic!("initializer panicked");
                }
            })
            .await
            .expect_err("initializer panic must fail the create");

        let failed_app_id = failed_app_id
            .lock()
            .expect("lock app id")
            .clone()
            .expect("initializer observed a minted id");
        assert_eq!(error.code(), AppErrorCode::Io, "{error}");
        assert!(h.service.list_apps().await.is_empty());
        assert!(h.take_events().await.is_empty());
        assert!(!dir.path().join("apps/index.json").exists());
        assert!(
            !dir.path().join("apps").join(&failed_app_id).exists(),
            "initializer panic must clean the exact unindexed app directory"
        );

        let reloaded = reload_service(&h.service).await;
        assert!(reloaded.list_apps().await.is_empty());
    }

    #[tokio::test]
    async fn dropping_create_after_the_initializer_starts_still_commits_and_emits() {
        let dir = tempfile::tempdir().unwrap();
        let Harness { service, observer } = harness(dir.path()).await;
        let service = Arc::new(service);
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let observed_id = Arc::new(std::sync::Mutex::new(None::<String>));

        let create = tokio::spawn({
            let service = Arc::clone(&service);
            let started = Arc::clone(&started);
            let release = Arc::clone(&release);
            let observed_id = Arc::clone(&observed_id);
            async move {
                service
                    .create_app_with_initializer(Some("Abort"), "a test app", None, move |record| {
                        let started = Arc::clone(&started);
                        let release = Arc::clone(&release);
                        let observed_id = Arc::clone(&observed_id);
                        async move {
                            *observed_id.lock().expect("lock app id") = Some(record.id);
                            started.notify_one();
                            release.notified().await;
                            Ok(())
                        }
                    })
                    .await
            }
        });

        started.notified().await;
        create.abort();
        let _ = create.await;
        release.notify_one();

        let committed_id = {
            let observed_id = Arc::clone(&observed_id);
            timeout(Duration::from_secs(5), async move {
                loop {
                    if let Some(app_id) = observed_id.lock().expect("lock app id").clone() {
                        break app_id;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("initializer observed a minted id")
        };

        timeout(Duration::from_secs(5), async {
            loop {
                if service
                    .list_apps()
                    .await
                    .iter()
                    .any(|record| record.id == committed_id)
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("completion task commits after caller cancellation");
        service.flush_events().await;

        let apps = service.list_apps().await;
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].id, committed_id);
        assert_eq!(
            observer.take(),
            vec![AppEvent::AppsChanged { apps: apps.clone() }]
        );

        let reloaded = reload_service(service.as_ref()).await;
        assert_eq!(reloaded.list_apps().await, apps);
    }

    /// When the per-app batch lands but the index write fails, the
    /// compensating rollback rewrites the ORIGINAL documents so a reload
    /// agrees with the failure the caller saw — the mirror-wins repair must
    /// NOT commit the transition the user was told failed. Injection is a
    /// directory squat on `apps/index.json`: it defeats root, where the old
    /// chmod probe returned vacuously.
    #[tokio::test]
    async fn failed_index_write_rolls_back_disk_so_reload_agrees_with_the_failure() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let record = h
            .service
            .create_app(Some("Roll"), "a test app", None)
            .await
            .unwrap();
        let _ = h.take_events().await;

        // Make ONLY the index write fail: a directory squatting on
        // index.json blocks the rename for every uid; per-app documents in
        // the subdirectories stay writable.
        let index_path = dir.path().join("apps/index.json");
        let index_before = squat_document(&index_path);
        let mirror_path = dir
            .path()
            .join("apps")
            .join(&record.id)
            .join("workspace/.lingxi/app.json");
        let mirror_before = std::fs::read_to_string(&mirror_path).unwrap();

        let err = h.service.mark_ready(&record.id).await.unwrap_err();
        // A squatted document path is store tampering, typed storage_corrupt
        // (the write-side twin of the load-side squat contract).
        assert_eq!(err.code(), AppErrorCode::StorageCorrupt, "{err}");
        // Memory rolled back: the record still reads `draft`, and no success
        // event leaked out.
        assert_eq!(
            h.service.record(&record.id).await.unwrap().workflow_state,
            AppWorkflowState::Draft
        );
        assert!(
            h.take_events().await.is_empty(),
            "a failed mutation emits nothing"
        );
        // DISK rolled back too: the compensating rollback restored the
        // mirror byte-for-byte.
        assert_eq!(
            std::fs::read_to_string(&mirror_path).unwrap(),
            mirror_before,
            "the mirror must be rolled back to its pre-transition bytes"
        );

        unsquat_document(&index_path, &index_before);

        // Disk agrees with the reported failure: the reload does NOT
        // resurrect the mark_ready, and the mutation succeeds when retried.
        drop(h);
        let h2 = harness(dir.path()).await;
        assert_eq!(
            h2.service.record(&record.id).await.unwrap().workflow_state,
            AppWorkflowState::Draft
        );
        h2.service.mark_ready(&record.id).await.unwrap();
        assert_eq!(
            h2.service.record(&record.id).await.unwrap().workflow_state,
            AppWorkflowState::Ready
        );
    }

    /// A mutation only rewrites the documents it changed — a runtime-only
    /// update must succeed even when the record mirror cannot be written,
    /// and must not rewrite the index. Injection is a directory squat on
    /// `app.json`: it blocks that document for EVERY uid, so the test
    /// asserts under root too.
    #[tokio::test]
    async fn runtime_only_mutation_skips_untouched_documents() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let record = h
            .service
            .create_app(Some("Lean"), "a test app", None)
            .await
            .unwrap();
        let index_path = dir.path().join("apps/index.json");
        let index_before = std::fs::read_to_string(&index_path).unwrap();

        // Block app.json with a squatting directory: atomic writes onto it
        // become impossible (for any uid).
        let mirror_path = dir
            .path()
            .join("apps")
            .join(&record.id)
            .join("workspace/.lingxi/app.json");
        let mirror_before = squat_document(&mirror_path);

        // The runtime-only mutation does not touch that file: it must
        // SUCCEED and persist runtime.json.
        h.service
            .update_runtime_record(
                &record.id,
                AppRuntimeState::Starting,
                Some(3020),
                Some(7),
                None,
            )
            .await
            .unwrap();
        let runtime_body = std::fs::read_to_string(
            dir.path()
                .join("apps")
                .join(&record.id)
                .join("runtime.json"),
        )
        .unwrap();
        assert!(runtime_body.contains("\"starting\""), "{runtime_body}");
        assert!(runtime_body.contains("3020"), "{runtime_body}");
        // The record did not change, so the index was not rewritten either.
        assert_eq!(std::fs::read_to_string(&index_path).unwrap(), index_before);

        // Control probe: a mutation that DOES touch app.json fails, proving
        // the squat actually blocks that document.
        let err = h.service.mark_ready(&record.id).await.unwrap_err();
        assert_eq!(err.code(), AppErrorCode::StorageCorrupt, "{err}");

        unsquat_document(&mirror_path, &mirror_before);

        // The runtime change survives a reload (reconciled at load, the
        // pinned port proves the write landed).
        drop(h);
        let h2 = harness(dir.path()).await;
        let reloaded = h2.service.runtime_record(&record.id).await.unwrap();
        assert_eq!(reloaded.port, Some(3020));
        assert_eq!(
            reloaded.state,
            AppRuntimeState::Failed,
            "reconciled at load"
        );
    }

    /// A crash while the runtime was busy no longer wedges `delete_app` —
    /// the busy state is reconciled at the next load.
    #[tokio::test]
    async fn delete_app_works_after_a_crash_left_the_runtime_busy() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let record = h
            .service
            .create_app(Some("Busy2"), "a test app", None)
            .await
            .unwrap();
        h.service
            .update_runtime_record(
                &record.id,
                AppRuntimeState::Starting,
                Some(3005),
                Some(1),
                None,
            )
            .await
            .unwrap();
        // While the process lives, the busy runtime still blocks deletion.
        let err = h.service.delete_app(&record.id).await.unwrap_err();
        assert_eq!(err.code(), AppErrorCode::RuntimeBusy);
        drop(h); // crash

        let h2 = harness(dir.path()).await;
        let reconciled = h2.service.runtime_record(&record.id).await.unwrap();
        assert_eq!(reconciled.state, AppRuntimeState::Failed);
        h2.service.delete_app(&record.id).await.unwrap();
        assert!(h2.service.list_apps().await.is_empty());
    }

    /// `last_error` is the one persisted string that TRUNCATES instead of
    /// rejecting — refusing a runtime failure report would lose the evidence
    /// entirely.
    #[tokio::test]
    async fn oversized_runtime_last_error_is_truncated_not_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let record = h
            .service
            .create_app(Some("Err"), "a test app", None)
            .await
            .unwrap();
        let _ = h.take_events().await;

        let huge = "e".repeat(MAX_TEXT_VALUE_BYTES + 500);
        let runtime = h
            .service
            .update_runtime_record(
                &record.id,
                AppRuntimeState::Starting,
                Some(3001),
                None,
                Some(huge),
            )
            .await
            .unwrap();
        let stored = runtime.last_error.clone().expect("error kept");
        assert!(
            stored.len() <= MAX_TEXT_VALUE_BYTES,
            "{} bytes",
            stored.len()
        );
        assert!(stored.ends_with("… [truncated]"), "{stored:?}");
        assert!(stored.starts_with("eee"), "the report's head is kept");
        // The event carries exactly what was persisted.
        assert_eq!(
            h.take_events().await,
            vec![AppEvent::RuntimeChanged {
                app_id: record.id.clone(),
                runtime: runtime.clone(),
            }]
        );

        // Multi-byte text truncates on a char boundary (no panic, valid text).
        let multi = "é".repeat(MAX_TEXT_VALUE_BYTES); // 2 bytes per char
        let runtime = h
            .service
            .update_runtime_record(
                &record.id,
                AppRuntimeState::Starting,
                None,
                None,
                Some(multi),
            )
            .await
            .unwrap();
        let stored = runtime.last_error.expect("error kept");
        assert!(stored.len() <= MAX_TEXT_VALUE_BYTES);
        assert!(stored.ends_with("… [truncated]"));

        // Exactly at the cap passes through untouched.
        let exact = "x".repeat(MAX_TEXT_VALUE_BYTES);
        let runtime = h
            .service
            .update_runtime_record(
                &record.id,
                AppRuntimeState::Starting,
                None,
                None,
                Some(exact.clone()),
            )
            .await
            .unwrap();
        assert_eq!(runtime.last_error.as_deref(), Some(exact.as_str()));
    }

    /// Two service instances over one root coordinate through the locked,
    /// foreign-preserving index transactions — each instance's creates
    /// survive the other's writes, and a delete stays authoritative without
    /// resurrecting or touching the other instance's app.
    #[tokio::test]
    async fn two_service_instances_preserve_each_others_index_entries() {
        let dir = tempfile::tempdir().unwrap();
        let a = harness(dir.path()).await;
        let b = harness(dir.path()).await; // loaded before app1 exists
        let app1 = a
            .service
            .create_app(Some("From A"), "a test app", None)
            .await
            .unwrap();
        // B has never seen app1; its index write must preserve it.
        let app2 = b
            .service
            .create_app(Some("From B"), "a test app", None)
            .await
            .unwrap();
        let on_disk = storage::load_all(dir.path()).unwrap();
        let mut ids_on_disk: Vec<&str> = on_disk.iter().map(|app| app.record.id.as_str()).collect();
        ids_on_disk.sort_unstable();
        let mut expected = [app1.id.as_str(), app2.id.as_str()];
        expected.sort_unstable();
        assert_eq!(ids_on_disk, expected, "both instances' creates survive");

        // A deletes ITS app: app1 must not resurrect, app2 must survive.
        a.service.delete_app(&app1.id).await.unwrap();
        let after = storage::load_all(dir.path()).unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].record.id, app2.id, "B's app is untouched");
        assert_eq!(after[0].record.name, "From B");
        // …and stays deleted across A's next index write.
        let app3 = a
            .service
            .create_app(Some("A again"), "a test app", None)
            .await
            .unwrap();
        let final_state = storage::load_all(dir.path()).unwrap();
        let mut final_ids: Vec<&str> = final_state
            .iter()
            .map(|app| app.record.id.as_str())
            .collect();
        final_ids.sort_unstable();
        let mut expected = [app2.id.as_str(), app3.id.as_str()];
        expected.sort_unstable();
        assert_eq!(
            final_ids, expected,
            "app1 never resurrects; app2 still survives"
        );
    }

    /// Await `future`, converting a panic anywhere in its polls into an
    /// `Err(message)` — a dependency-free async `catch_unwind` (no `unsafe`:
    /// the future is boxed, so `as_mut().poll` needs no manual pin
    /// projection).
    async fn catch_panic_message<F: std::future::Future>(future: F) -> Result<F::Output, String> {
        let mut future = Box::pin(future);
        std::future::poll_fn(move |context| {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                future.as_mut().poll(context)
            })) {
                Ok(std::task::Poll::Pending) => std::task::Poll::Pending,
                Ok(std::task::Poll::Ready(value)) => std::task::Poll::Ready(Ok(value)),
                Err(payload) => std::task::Poll::Ready(Err(payload
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_string()))
                    .unwrap_or_else(|| "panicked with a non-string payload".to_string()))),
            }
        })
        .await
    }

    /// An observer calling back into an emitting path PANICS with a clear
    /// message (caught here inside the observer) instead of silently
    /// deadlocking the emission queue forever.
    #[tokio::test]
    async fn reentrant_observer_panics_instead_of_deadlocking() {
        use async_trait::async_trait;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::OnceLock;

        struct ReentrantObserver {
            service: OnceLock<Arc<AppService>>,
            fired: AtomicBool,
            outcome: std::sync::Mutex<Option<String>>,
        }
        #[async_trait]
        impl AppEventObserver for ReentrantObserver {
            async fn on_event(&self, _event: AppEvent) {
                if self.fired.swap(true, Ordering::SeqCst) {
                    return;
                }
                let Some(service) = self.service.get() else {
                    return;
                };
                let message = match catch_panic_message(service.announce_apps()).await {
                    Err(panic_message) => panic_message,
                    Ok(_) => "the reentrant call did NOT panic".to_string(),
                };
                *self.outcome.lock().unwrap() = Some(message);
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let observer = Arc::new(ReentrantObserver {
            service: OnceLock::new(),
            fired: AtomicBool::new(false),
            outcome: std::sync::Mutex::new(None),
        });
        let service = Arc::new(
            AppService::load(
                dir.path(),
                Arc::new(FixedClock::new(1_700_000_000_000)),
                Arc::clone(&observer) as Arc<dyn AppEventObserver>,
            )
            .await
            .unwrap(),
        );
        observer.service.set(Arc::clone(&service)).ok().unwrap();

        // The create's emission triggers the observer's reentrant call.
        service
            .create_app(Some("Reenter"), "a test app", None)
            .await
            .unwrap();
        service.flush_events().await; // completes — the queue is NOT deadlocked
        let outcome = observer
            .outcome
            .lock()
            .unwrap()
            .clone()
            .expect("observer ran");
        assert!(
            outcome.contains("re-entered"),
            "the reentrancy guard must panic with its message, got: {outcome}"
        );
    }

    /// A caller future dropped around a mutation can no longer lose the
    /// mutation's events — whenever the mutation committed, its event is
    /// delivered; when it did not commit, nothing is.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dropped_caller_never_loses_a_committed_mutations_events() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let record = h
            .service
            .create_app(Some("Drop"), "a test app", None)
            .await
            .unwrap();
        let _ = h.take_events().await;
        let observer = Arc::clone(&h.observer);
        let service = Arc::new(h.service);
        for i in 0..20u32 {
            // Current runtime state is always Stopped or Failed here, both
            // legal sources for Starting.
            let call = {
                let service = Arc::clone(&service);
                let app_id = record.id.clone();
                tokio::spawn(async move {
                    service
                        .update_runtime_record(
                            &app_id,
                            AppRuntimeState::Starting,
                            Some(3999),
                            Some(7),
                            None,
                        )
                        .await
                })
            };
            if i % 2 == 0 {
                tokio::task::yield_now().await; // let some iterations commit
            }
            call.abort();
            let _ = call.await;
            service.flush_events().await;
            let committed = service.runtime_record(&record.id).await.unwrap().state
                == AppRuntimeState::Starting;
            let events = observer.take();
            if committed {
                assert!(
                    events.iter().any(|event| matches!(
                        event,
                        AppEvent::RuntimeChanged { runtime, .. }
                            if runtime.state == AppRuntimeState::Starting
                    )),
                    "iteration {i}: committed mutation lost its event: {events:?}"
                );
                // Reset for the next iteration (Starting -> Failed is legal).
                service
                    .update_runtime_record(
                        &record.id,
                        AppRuntimeState::Failed,
                        None,
                        None,
                        Some("reset".into()),
                    )
                    .await
                    .unwrap();
                service.flush_events().await;
                let _ = observer.take();
            } else {
                assert!(
                    events.is_empty(),
                    "iteration {i}: an uncommitted call must emit nothing: {events:?}"
                );
            }
        }
    }
}

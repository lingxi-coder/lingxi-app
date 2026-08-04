//! Atomic on-disk storage for local apps (spec §D).
//!
//! Layout under the injected data root:
//!
//! ```text
//! apps/index.json                                  — { schemaVersion, apps }
//! apps/<app-id>/runtime.json                       — AppRuntimeRecord
//! apps/<app-id>/interactions.json                  — AppInteractions
//! apps/<app-id>/workspace/.lingxi/app.json         — { schemaVersion, app } mirror
//! apps/<app-id>/workspace/.lingxi/design-spec.json — AppDesignDraft
//! ```
//!
//! Every write goes through [`traits::rooted_fs::atomic_write`] (same-directory
//! temp file + rename, no symlink traversal below the root), so a crash can
//! only ever leave an orphan `*.tmp-<pid>-<seq>` file behind — loaders address
//! exact file names and therefore ignore orphans naturally. Every document
//! carries `schemaVersion`; enumeration is index-driven (an app directory not
//! listed in `index.json` is invisible), which makes `index.json` the commit
//! point for creation (per-app files first, index last) and deletion (index
//! entry first, directory last).
//!
//! For MUTATIONS of a listed app the per-app batch is the effective commit
//! point instead: the batch lands `design-spec.json`, `interactions.json`,
//! `runtime.json` and finally the `app.json` record mirror — the order is
//! defined ONCE as [`APP_DOC_WRITE_ORDER`] and executed by
//! [`save_app_files_steps`] — and only then does the caller rewrite the
//! index. The repair contract depends on that ORDER, not on completeness: a
//! writer may skip documents that did not change (the service's mutation path
//! does), but the writes that DO happen must follow the canonical sequence,
//! and crash tests replay true prefixes of it. A crash inside that window
//! would otherwise strand the store torn — e.g. the index still says
//! `awaiting_spec_confirmation` while `interactions.json` already consumed
//! the gate and queued the `design_confirmed` continuation, leaving an
//! unsatisfiable confirmation gate and a phantom continuation. [`load_all`]
//! therefore reconciles on load: a diverged `app.json` mirror (written after
//! every other per-app document) supersedes the index record, and the gate
//! invariants of `interactions.json` (pending presence + newest undelivered
//! continuation) roll a mid-batch tear forward. Repairs are persisted through
//! the normal write path before the load returns.
//!
//! Cross-process coordination: `apps/index.json` is read-modify-written under
//! the advisory `apps/index.lock` file lock ([`lock_exclusive`]-style, the
//! same pattern sibling crates use for shared spool files). [`load_all`] holds
//! it across the whole load (read + repair persists) and
//! [`save_index_preserving`] holds it across its re-read + merge + write, so
//! two service instances over one root cannot clobber each other's index
//! entries. Deletion is rename-to-trash: `apps/<id>` is atomically renamed
//! into `apps/.trash/<id>-<nonce>` (the commit point AND the tombstone —
//! `rename` never follows the final component, and a racing create cannot
//! collide while either the live dir or the tombstone exists), then removed;
//! leftovers in `apps/.trash` are swept best-effort at the next load.
//! [`load_all`] itself is index-driven and never enumerates `apps/`, so
//! `.trash` is invisible to it beyond the sweep.

use crate::error::AppError;
use crate::ids;
use crate::state::AppState;
use crate::types::{
    AppContinuationKind, AppDesignDraft, AppInteractionKind, AppInteractionRequest,
    AppInteractions, AppRecord, AppRuntimeRecord, AppRuntimeState, AppWorkflowState,
    APPS_SCHEMA_VERSION,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use traits::rooted_fs::{self, AtomicWriteOptions};
use traits::FsError;

/// Upper bound on any single persisted document, enforced in BOTH directions:
/// loads refuse to slurp a larger file (typed `storage_corrupt` — the file is
/// out of contract), and [`save_app_files_steps`]/`write_doc` refuse to
/// produce one (typed `invalid_request`, checked BEFORE any temp file is
/// written). The service-level input caps keep ordinary documents far below
/// this bound, but they do NOT arithmetically guarantee it — e.g. a maximal
/// draft (256 fields × 20 000-byte values) whose text is dominated by
/// JSON-escaped characters serializes well past 8 MiB — so the write seam is
/// where the invariant is enforced: nothing this module persists can later
/// fail its own load on size.
pub const MAX_DOC_BYTES: u64 = 8 * 1024 * 1024;

/// Directory under the data root holding all app state.
pub const APPS_DIR: &str = "apps";
/// Index document listing every app.
pub const INDEX_FILE: &str = "index.json";
/// Advisory lock file serializing every `apps/index.json` read-modify-write
/// transaction across processes (and across service instances in one
/// process). A runtime artifact, not a persisted document — loaders never
/// read it.
pub const INDEX_LOCK_FILE: &str = "index.lock";
/// Tombstone directory for deleted app dirs (`apps/.trash`). Never a legal
/// app id (ids cannot start with `.`), invisible to the index-driven
/// [`load_all`], swept best-effort at load.
pub const TRASH_DIR: &str = ".trash";
/// Per-app runtime record.
pub const RUNTIME_FILE: &str = "runtime.json";
/// Per-app interaction + continuation store.
pub const INTERACTIONS_FILE: &str = "interactions.json";
/// Per-app workspace directory (the future Next.js project root).
pub const WORKSPACE_DIR: &str = "workspace";
/// App-scoped state directory inside the workspace (`.lingxi`).
pub const APP_STATE_DIR: &str = branding::DOT_DIR;
/// App-scoped metadata mirror inside `workspace/.lingxi/`.
pub const APP_METADATA_FILE: &str = "app.json";
/// Design draft inside `workspace/.lingxi/`.
pub const DESIGN_SPEC_FILE: &str = "design-spec.json";

/// The whole `apps/index.json` document.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppIndexFile {
    /// Persisted schema version ([`APPS_SCHEMA_VERSION`]).
    pub schema_version: u32,
    /// Every app, in creation order.
    pub apps: Vec<AppRecord>,
}

/// The `workspace/.lingxi/app.json` app-scoped metadata mirror.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppMetadataFile {
    /// Persisted schema version ([`APPS_SCHEMA_VERSION`]).
    pub schema_version: u32,
    /// Mirror of this app's index record.
    pub app: AppRecord,
}

/// Root-relative path of `apps/index.json`.
#[must_use]
pub fn index_rel() -> PathBuf {
    PathBuf::from(APPS_DIR).join(INDEX_FILE)
}

/// Root-relative path of the advisory index lock (`apps/index.lock`).
#[must_use]
pub fn index_lock_rel() -> PathBuf {
    PathBuf::from(APPS_DIR).join(INDEX_LOCK_FILE)
}

/// Root-relative path of the deletion tombstone dir (`apps/.trash`).
#[must_use]
pub fn trash_dir_rel() -> PathBuf {
    PathBuf::from(APPS_DIR).join(TRASH_DIR)
}

/// Take the advisory exclusive lock guarding `apps/index.json`
/// read-modify-write transactions. BLOCKS until acquired; callers must not
/// nest it (a second acquisition from the same process deadlocks — `flock`
/// excludes across open descriptions, not just across processes).
fn lock_index(root: &Path) -> Result<rooted_fs::RootedFileLock, AppError> {
    rooted_fs::lock_exclusive(
        root,
        &index_lock_rel(),
        rooted_fs::PRIVATE_DIR_MODE,
        rooted_fs::PRIVATE_FILE_MODE,
    )
    .map_err(|error| AppError::from_fs("lock apps index", &error))
}

/// Root-relative path of `apps/<id>`.
#[must_use]
pub fn app_dir_rel(app_id: &str) -> PathBuf {
    PathBuf::from(APPS_DIR).join(app_id)
}

/// Root-relative path of `apps/<id>/runtime.json`.
#[must_use]
pub fn runtime_rel(app_id: &str) -> PathBuf {
    app_dir_rel(app_id).join(RUNTIME_FILE)
}

/// Root-relative path of `apps/<id>/interactions.json`.
#[must_use]
pub fn interactions_rel(app_id: &str) -> PathBuf {
    app_dir_rel(app_id).join(INTERACTIONS_FILE)
}

/// Root-relative path of `apps/<id>/workspace`.
#[must_use]
pub fn workspace_dir_rel(app_id: &str) -> PathBuf {
    app_dir_rel(app_id).join(WORKSPACE_DIR)
}

/// The `workspace_rel` string stored on [`AppRecord`]: always forward-slash
/// `apps/<id>/workspace`, independent of platform.
#[must_use]
pub fn workspace_rel_str(app_id: &str) -> String {
    format!("{APPS_DIR}/{app_id}/{WORKSPACE_DIR}")
}

/// Root-relative path of `apps/<id>/workspace/.lingxi/app.json`.
#[must_use]
pub fn metadata_rel(app_id: &str) -> PathBuf {
    workspace_dir_rel(app_id)
        .join(APP_STATE_DIR)
        .join(APP_METADATA_FILE)
}

/// Root-relative path of `apps/<id>/workspace/.lingxi/design-spec.json`.
#[must_use]
pub fn design_spec_rel(app_id: &str) -> PathBuf {
    workspace_dir_rel(app_id)
        .join(APP_STATE_DIR)
        .join(DESIGN_SPEC_FILE)
}

/// Serialize a document the way the repo persists native-feature JSON:
/// pretty 2-space indent plus a single trailing newline.
fn serialize_doc<T: Serialize>(rel: &Path, value: &T) -> Result<String, AppError> {
    let mut body = serde_json::to_string_pretty(value)
        .map_err(|error| AppError::Io(format!("serialize {}: {error}", rel.display())))?;
    body.push('\n');
    Ok(body)
}

fn write_doc<T: Serialize>(root: &Path, rel: &Path, value: &T) -> Result<(), AppError> {
    let body = serialize_doc(rel, value)?;
    // Enforce the load-side size contract at the write seam, BEFORE any temp
    // file is created: a document the loader would refuse must never reach
    // disk, where it would brick the whole store at the next load. The
    // mutation fails typed instead, and the caller's memory rollback keeps
    // memory == disk.
    let within = u64::try_from(body.len()).is_ok_and(|len| len <= MAX_DOC_BYTES);
    if !within {
        return Err(AppError::InvalidRequest(format!(
            "document {} would exceed the durable size limit: {} bytes (limit {MAX_DOC_BYTES})",
            rel.display(),
            body.len()
        )));
    }
    rooted_fs::atomic_write(root, rel, body.as_bytes(), AtomicWriteOptions::default())
        .map_err(|error| write_error(rel, &error))
}

/// Map a write failure. Every path this module writes is derived from a
/// pre-validated app id (never a raw caller path), so a containment violation
/// here — a symlink or DIRECTORY squatting on a document's final path, which
/// `atomic_write` refuses before renaming — is store tampering, not caller
/// error: it maps to `storage_corrupt`, mirroring [`load_read_error`]'s
/// treatment of the same squat on the read side. Everything else stays `Io`.
fn write_error(rel: &Path, error: &FsError) -> AppError {
    match error {
        FsError::OutsideWorkspace(_) => AppError::StorageCorrupt(format!(
            "{} is squatted by a non-regular file (symlink or directory on a \
             document path); refusing to write through it",
            rel.display()
        )),
        other => AppError::from_fs(&format!("write {}", rel.display()), other),
    }
}

/// Map a read failure in the LOAD path. Tampering with the store's documents
/// is storage corruption, not caller error: a missing file, an
/// over-[`MAX_DOC_BYTES`] body, non-UTF-8 bytes, and a symlink (or other
/// non-regular file) squatting on a document path are all `storage_corrupt`
/// — for a *listed* app every document must exist as a well-formed regular
/// file within the size contract. `invalid_request` stays reserved for
/// genuinely caller-supplied bad paths outside the load path (see
/// [`AppError::from_fs`]); genuine I/O failures stay `Io`.
fn load_read_error(rel: &Path, error: &FsError) -> AppError {
    match error {
        FsError::NotFound(_) => AppError::StorageCorrupt(format!("{} is missing", rel.display())),
        FsError::TooLarge { actual, limit } => AppError::StorageCorrupt(format!(
            "{} is {actual} bytes (limit {limit})",
            rel.display()
        )),
        FsError::BinaryFile(_) => {
            AppError::StorageCorrupt(format!("{} is not valid UTF-8", rel.display()))
        }
        FsError::OutsideWorkspace(_) => AppError::StorageCorrupt(format!(
            "{} is not a regular file contained in the store (symlink or special file \
             squatting on a document path)",
            rel.display()
        )),
        other => AppError::from_fs(&format!("read {}", rel.display()), other),
    }
}

/// Read + strictly parse one schema-versioned document; every out-of-contract
/// shape fails typed `storage_corrupt` (see [`load_read_error`]).
fn read_doc<T: DeserializeOwned>(root: &Path, rel: &Path) -> Result<T, AppError> {
    let body = rooted_fs::read_to_string_limited(root, rel, MAX_DOC_BYTES)
        .map_err(|error| load_read_error(rel, &error))?;
    serde_json::from_str(&body)
        .map_err(|error| AppError::StorageCorrupt(format!("{}: {error}", rel.display())))
}

fn ensure_schema_version(rel: &Path, found: u32) -> Result<(), AppError> {
    if found == APPS_SCHEMA_VERSION {
        Ok(())
    } else {
        Err(AppError::StorageCorrupt(format!(
            "{}: unsupported schemaVersion {found} (expected {APPS_SCHEMA_VERSION})",
            rel.display()
        )))
    }
}

/// Load every app from disk. A missing index means an empty store; a corrupt
/// index or per-app document fails with `storage_corrupt` rather than
/// silently dropping apps.
///
/// A store torn by a crash between the per-app batch and the index rewrite is
/// detected here (mirror/index divergence, gate invariants — see the module
/// doc) and repaired forward, and a runtime record stranded busy by a crash
/// is reconciled (phase 1 has no process manager, so nothing can still be
/// starting/running/stopping — see [`reconcile_runtime_at_load`]); both are
/// persisted before returning.
pub fn load_all(root: &Path) -> Result<Vec<AppState>, AppError> {
    // The whole load — initial read, torn-commit repairs, and their index
    // rewrite — is one read-modify-write transaction under the advisory
    // index lock (finding 9), so a concurrent instance's save cannot
    // interleave with the repair writes. Everything below writes the index
    // through the RAW `save_index` (this transaction already holds the lock;
    // re-acquiring would self-deadlock). The lock file lives inside the
    // store, so an absent root/`apps/` skeleton is created first (a fresh,
    // empty store).
    std::fs::create_dir_all(root.join(APPS_DIR))
        .map_err(|error| AppError::Io(format!("create {}: {error}", root.display())))?;
    let _lock = lock_index(root)?;
    sweep_trash(root);
    let index_rel = index_rel();
    let body = match rooted_fs::read_to_string_limited(root, &index_rel, MAX_DOC_BYTES) {
        Ok(body) => body,
        Err(FsError::NotFound(_)) => return Ok(Vec::new()),
        Err(error) => return Err(load_read_error(&index_rel, &error)),
    };
    let index: AppIndexFile = serde_json::from_str(&body)
        .map_err(|error| AppError::StorageCorrupt(format!("apps/index.json: {error}")))?;
    ensure_schema_version(&index_rel, index.schema_version)?;

    let mut apps = Vec::with_capacity(index.apps.len());
    let mut seen_ids = BTreeSet::new();
    let mut any_repaired = false;
    for record in index.apps {
        // A hostile id in a tampered index must never turn into a path.
        if !ids::is_valid_app_id(&record.id) {
            return Err(AppError::StorageCorrupt(format!(
                "apps/index.json lists invalid app id {:?}",
                record.id
            )));
        }
        // A duplicated id would alias one directory across two entries and
        // detonate later (deleting one strands the twin over a removed
        // directory); fail loudly at load time like an invalid id.
        if !seen_ids.insert(record.id.clone()) {
            return Err(AppError::StorageCorrupt(format!(
                "apps/index.json lists app id {:?} more than once",
                record.id
            )));
        }
        // Finding 7: `workspace_rel` carries the documented invariant
        // `apps/<id>/workspace` — validated for exact equality BEFORE any
        // per-app document is read, so a tampered value is rejected instead
        // of being laundered back into the index by a later repair rewrite.
        ensure_workspace_rel("apps/index.json", &record)?;
        let draft_rel = design_spec_rel(&record.id);
        let draft: AppDesignDraft = read_doc(root, &draft_rel)?;
        ensure_schema_version(&draft_rel, draft.schema_version)?;

        let interactions = load_interactions(root, &record.id)?;

        let runtime_rel = runtime_rel(&record.id);
        let runtime: AppRuntimeRecord = read_doc(root, &runtime_rel)?;
        ensure_schema_version(&runtime_rel, runtime.schema_version)?;
        // Finding 6: the embedded owner id must match the app the document
        // belongs to, like the mirror id below.
        if runtime.app_id != record.id {
            return Err(AppError::StorageCorrupt(format!(
                "{} claims app id {:?} but belongs to app {:?}",
                runtime_rel.display(),
                runtime.app_id,
                record.id
            )));
        }

        let metadata_rel = metadata_rel(&record.id);
        let mirror: AppMetadataFile = read_doc(root, &metadata_rel)?;
        ensure_schema_version(&metadata_rel, mirror.schema_version)?;
        if mirror.app.id != record.id {
            return Err(AppError::StorageCorrupt(format!(
                "{} mirrors app id {:?} but the index lists {:?}",
                metadata_rel.display(),
                mirror.app.id,
                record.id
            )));
        }
        // Finding 7 (mirror side): the mirror record can supersede the index
        // record in torn-commit repair, so its `workspace_rel` is validated
        // BEFORE repair may adopt (and re-persist) it.
        ensure_workspace_rel(&metadata_rel.display().to_string(), &mirror.app)?;

        let mut app = AppState {
            record,
            draft,
            interactions,
            runtime,
        };
        let repaired = repair_torn_commit(&mut app, &mirror.app);
        if repaired {
            tracing::warn!(
                app_id = %app.record.id,
                repaired_state = %app.record.workflow_state,
                "local-apps store was torn by a crash mid-commit; repaired forward"
            );
        }
        let reconciled = reconcile_runtime_at_load(&mut app);
        if repaired {
            save_app_files(root, &app)?;
            any_repaired = true;
        } else if reconciled {
            save_runtime(root, &app.record.id, &app.runtime)?;
        }
        apps.push(app);
    }
    if any_repaired {
        let records: Vec<AppRecord> = apps.iter().map(|app| app.record.clone()).collect();
        save_index(root, &records)?;
    }
    Ok(apps)
}

/// Enforce the documented `workspace_rel` invariant — EXACT equality with
/// `apps/<id>/workspace` (finding 7). Applied to index records and the
/// `app.json` mirror at load, so repair can never launder a tampered value
/// back into the index: nothing is adopted or re-persisted unvalidated.
fn ensure_workspace_rel(source: &str, record: &AppRecord) -> Result<(), AppError> {
    let expected = workspace_rel_str(&record.id);
    if record.workspace_rel == expected {
        Ok(())
    } else {
        Err(AppError::StorageCorrupt(format!(
            "{source}: app {:?} workspaceRel {:?} violates the invariant {expected:?}",
            record.id, record.workspace_rel
        )))
    }
}

/// Read one app's `interactions.json` straight from disk (strict parse,
/// schema check, and [`validate_interactions_invariants`]). Besides
/// [`load_all`], the service's redelivery path re-reads the document through
/// this to MERGE with memory before persisting, so a continuation that
/// reached disk but was rolled back out of memory is never clobbered.
pub fn load_interactions(root: &Path, app_id: &str) -> Result<AppInteractions, AppError> {
    let rel = interactions_rel(app_id);
    let interactions: AppInteractions = read_doc(root, &rel)?;
    ensure_schema_version(&rel, interactions.schema_version)?;
    validate_interactions_invariants(&rel, app_id, &interactions)?;
    Ok(interactions)
}

/// Pin the continuation-counter invariants that [`crate::state`]'s mint logic
/// guarantees (a fresh store starts at `next_seq` 1; seqs are minted at
/// `next_seq` which then increments; a delivery removes the entry and
/// advances `last_delivered_seq` in the same atomic write): every undelivered
/// seq lies strictly between `last_delivered_seq` (exclusive) and `next_seq`
/// (exclusive), seqs are strictly increasing in queue order (hence unique),
/// `next_seq` is never 0 and always exceeds `last_delivered_seq` — the
/// validator rejects NOW any state whose NEXT mint would violate these rules
/// (finding 5: validate-now instead of accept-then-brick; a persisted
/// `next_seq` of 0 would mint seq 0 and only the FOLLOWING load would
/// refuse the store). Every embedded `app_id` (pending gate, undelivered
/// entries) must name the owning app (finding 6). A violating document did
/// not come from a legal writer and fails typed `storage_corrupt` ("violates
/// invariants") instead of feeding garbage into torn-commit repair or
/// redelivery.
fn validate_interactions_invariants(
    rel: &Path,
    app_id: &str,
    interactions: &AppInteractions,
) -> Result<(), AppError> {
    let corrupt = |detail: String| {
        AppError::StorageCorrupt(format!(
            "{} violates continuation invariants: {detail}",
            rel.display()
        ))
    };
    if interactions.next_seq == 0 {
        return Err(corrupt(
            "nextSeq 0 is below the mint floor (a fresh store starts at 1); the next \
             mint would persist seq 0 and brick the following load"
                .to_string(),
        ));
    }
    if interactions.next_seq <= interactions.last_delivered_seq {
        return Err(corrupt(format!(
            "nextSeq {} does not exceed lastDeliveredSeq {} (the next mint would land \
             at or below the delivered bound)",
            interactions.next_seq, interactions.last_delivered_seq
        )));
    }
    if let Some(pending) = &interactions.pending {
        if pending.app_id != app_id {
            return Err(corrupt(format!(
                "pending gate claims app id {:?} but belongs to app {app_id:?}",
                pending.app_id
            )));
        }
    }
    let mut previous: Option<u64> = None;
    for continuation in &interactions.undelivered {
        if continuation.app_id != app_id {
            return Err(corrupt(format!(
                "undelivered seq {} claims app id {:?} but belongs to app {app_id:?}",
                continuation.seq, continuation.app_id
            )));
        }
        if continuation.seq <= interactions.last_delivered_seq {
            return Err(corrupt(format!(
                "undelivered seq {} is not above lastDeliveredSeq {}",
                continuation.seq, interactions.last_delivered_seq
            )));
        }
        if continuation.seq >= interactions.next_seq {
            return Err(corrupt(format!(
                "undelivered seq {} was never minted (nextSeq {})",
                continuation.seq, interactions.next_seq
            )));
        }
        if let Some(previous) = previous {
            if continuation.seq <= previous {
                return Err(corrupt(format!(
                    "undelivered seqs are not strictly increasing ({previous} then {})",
                    continuation.seq
                )));
            }
        }
        previous = Some(continuation.seq);
    }
    Ok(())
}

/// Reconcile a runtime record stranded busy by a crash. Phase 1 has no
/// process manager, so at load time nothing can genuinely still be
/// starting/running/stopping: `stopping` settles to `stopped` (the shutdown
/// it was waiting for cannot outlive the process), and `starting`/`running`
/// become `failed` with a `last_error` explaining the reconciliation —
/// otherwise the record would claim a live runtime forever and e.g.
/// `delete_app` would refuse with `runtime_busy` with no path out. Returns
/// `true` when the record changed (the caller persists).
///
/// ⚠️ PHASE-4 GATE (the twin of the warning on
/// `AppService::update_runtime_record`): this unconditional stranding policy
/// is CORRECT ONLY while no runtime process can outlive the engine. The
/// phase that introduces a live process manager MUST replace it with a
/// liveness-aware reconciliation, or loads will stamp genuinely running dev
/// servers `failed` and un-guard deletion.
fn reconcile_runtime_at_load(app: &mut AppState) -> bool {
    let reconciled_state = match app.runtime.state {
        AppRuntimeState::Stopping => AppRuntimeState::Stopped,
        AppRuntimeState::Starting | AppRuntimeState::Running => AppRuntimeState::Failed,
        AppRuntimeState::Stopped | AppRuntimeState::Failed => return false,
    };
    tracing::warn!(
        app_id = %app.record.id,
        stranded_state = %app.runtime.state,
        reconciled_state = %reconciled_state,
        "runtime record was stranded busy by a crash; reconciled at load"
    );
    if reconciled_state == AppRuntimeState::Failed {
        app.runtime.last_error = Some("reconciled at load: no live runtime manager".to_string());
    }
    app.runtime.state = reconciled_state;
    true
}

/// Reconcile one loaded app against a crash torn between the per-app batch
/// and the index rewrite (or inside the batch). Returns `true` when anything
/// was repaired (the caller persists).
///
/// Layer 1 — whole-batch tear: the `app.json` mirror is written LAST in
/// [`save_app_files`] and BEFORE the index, so a mirror/index divergence
/// proves the batch committed while the index write was lost. The mirror
/// record (same batch as `interactions.json` / `design-spec.json`) wins.
///
/// Layer 2 — mid-batch tear (`interactions.json` committed, mirror not):
/// `interactions.json` is authoritative for the gate. A gate-consuming
/// transition always clears `pending` AND queues its continuation in the same
/// atomic write, so:
/// - an awaiting state with `pending: None` rolls forward along the newest
///   undelivered continuation (`design_confirmed` → generating,
///   `design_cancelled` → `collecting_spec`, `preview_confirmed` → ready,
///   `revision_requested` → revising); with no evidence (hand-tampering) the
///   gate is re-armed with a fresh pending interaction so it stays
///   satisfiable;
/// - a gate-opening transition (`open_designer` / `validation_passed`) that
///   committed `pending` but not the state rolls the state forward to its
///   awaiting state;
/// - `ready` with a newest undelivered `revision_requested` is a torn
///   `request_revision` and rolls forward to revising.
fn repair_torn_commit(app: &mut AppState, mirror: &AppRecord) -> bool {
    let mut repaired = false;
    if *mirror != app.record {
        app.record = mirror.clone();
        repaired = true;
    }
    if resolve_gate_evidence(app) {
        repaired = true;
    }
    repaired
}

/// THE gate-vs-evidence rule table, defined exactly once (finding 3b):
/// reconcile `app.record.workflow_state` against the gate/continuation
/// evidence in `app.interactions`, mutating the aggregate in place. Shared by
/// [`repair_torn_commit`] (load-time torn-commit repair) and the service's
/// redelivery merge (which, after adopting disk's proof that an armed gate
/// was consumed, must resolve the resulting state by the SAME rules — never
/// a duplicated table). Returns `true` when anything was repaired (state
/// rolled forward or a gate re-armed); the caller persists.
pub(crate) fn resolve_gate_evidence(app: &mut AppState) -> bool {
    // Evidence: the newest continuation that is genuinely undelivered (a
    // stale entry from a crash between deliver and dequeue is not evidence).
    let newest_undelivered = app
        .interactions
        .undelivered
        .iter()
        .filter(|c| c.seq > app.interactions.last_delivered_seq)
        .max_by_key(|c| c.seq)
        .map(|c| c.kind);
    let pending_kind = app.interactions.pending.as_ref().map(|p| p.kind);

    // Both `newest_undelivered` matches below are deliberately exhaustive
    // over `AppContinuationKind` with NO catch-all arm: adding a kind must
    // force a compile-time decision about what it proves at each gate — a
    // silent `_` would actively mis-repair (re-arm over real evidence, or
    // worse) for the new kind.
    let repaired_state = match (app.record.workflow_state, pending_kind) {
        (AppWorkflowState::AwaitingSpecConfirmation, None) => Some(match newest_undelivered {
            Some(AppContinuationKind::DesignConfirmed) => AppWorkflowState::Generating,
            Some(AppContinuationKind::DesignCancelled) => AppWorkflowState::CollectingSpec,
            // Preview-stage evidence says nothing about the DESIGNER gate;
            // with no usable evidence (hand-tampering) the gate is re-armed.
            Some(
                AppContinuationKind::PreviewConfirmed | AppContinuationKind::RevisionRequested,
            )
            | None => {
                rearm_gate(app, AppInteractionKind::Designer);
                AppWorkflowState::AwaitingSpecConfirmation
            }
        }),
        (AppWorkflowState::AwaitingPreviewConfirmation, None) => Some(match newest_undelivered {
            Some(AppContinuationKind::PreviewConfirmed) => AppWorkflowState::Ready,
            Some(AppContinuationKind::RevisionRequested) => AppWorkflowState::Revising,
            // Designer-stage evidence says nothing about the PREVIEW gate;
            // with no usable evidence (hand-tampering) the gate is re-armed.
            Some(AppContinuationKind::DesignConfirmed | AppContinuationKind::DesignCancelled)
            | None => {
                rearm_gate(app, AppInteractionKind::Preview);
                AppWorkflowState::AwaitingPreviewConfirmation
            }
        }),
        // `open_designer` accepts GenerationFailed as well as CollectingSpec
        // (state.rs), so a crash between the interactions.json and the mirror
        // writes can leave either source state with the gate already armed.
        (
            AppWorkflowState::CollectingSpec | AppWorkflowState::GenerationFailed,
            Some(AppInteractionKind::Designer),
        ) => Some(AppWorkflowState::AwaitingSpecConfirmation),
        (AppWorkflowState::Validating, Some(AppInteractionKind::Preview)) => {
            Some(AppWorkflowState::AwaitingPreviewConfirmation)
        }
        (AppWorkflowState::Ready, None)
            if newest_undelivered == Some(AppContinuationKind::RevisionRequested) =>
        {
            Some(AppWorkflowState::Revising)
        }
        _ => None,
    };
    if let Some(state) = repaired_state {
        app.record.workflow_state = state;
        return true;
    }
    false
}

/// Re-arm an unsatisfiable gate whose consumption left no evidence (only
/// reachable through hand-tampering): a fresh pending interaction of `kind`
/// at the current revision, timestamped with the record's last mutation.
fn rearm_gate(app: &mut AppState, kind: AppInteractionKind) {
    app.interactions.pending = Some(AppInteractionRequest {
        interaction_id: ids::generate_interaction_id(),
        app_id: app.record.id.clone(),
        kind,
        revision: app.draft.revision,
        created_at_ms: app.record.updated_at_ms,
    });
}

/// Atomically replace `apps/index.json` with `records` — RAW, whole-index
/// authority, no lock of its own. Legal callers either already hold the
/// index lock for a wider transaction ([`load_all`]'s repair rewrite) or are
/// single-writer seeds (tests, fixtures). The service's cross-instance index
/// transactions go through [`save_index_preserving`] instead.
pub fn save_index(root: &Path, records: &[AppRecord]) -> Result<(), AppError> {
    let doc = AppIndexFile {
        schema_version: APPS_SCHEMA_VERSION,
        apps: records.to_vec(),
    };
    write_doc(root, &index_rel(), &doc)
}

/// One locked `apps/index.json` read-modify-write transaction (finding 9):
/// under the advisory index lock, re-read the disk index and write `records`
/// PLUS every disk entry whose id is in neither `records` nor `known_ids` —
/// an app created by a foreign process/instance this writer has never seen
/// (preserved verbatim, with a warning). For ids the writer HAS seen
/// (`known_ids` = every id it ever loaded, created or deleted) it stays
/// authoritative: an id it deleted is absent from `records` and is NOT
/// resurrected from disk.
///
/// A disk index that exists but cannot be read/parsed is logged and treated
/// as empty — this write then repairs it (foreign entries in the unreadable
/// document are unrecoverable either way).
pub fn save_index_preserving(
    root: &Path,
    records: &[AppRecord],
    known_ids: &BTreeSet<String>,
) -> Result<(), AppError> {
    let _lock = lock_index(root)?;
    let disk: Vec<AppRecord> =
        match rooted_fs::read_to_string_limited(root, &index_rel(), MAX_DOC_BYTES) {
            Ok(body) => match serde_json::from_str::<AppIndexFile>(&body) {
                Ok(index) => index.apps,
                Err(error) => {
                    tracing::warn!(
                        error = %error,
                        "apps/index.json on disk is unparseable during a locked index write; \
                         rewriting it from this instance's records"
                    );
                    Vec::new()
                }
            },
            Err(FsError::NotFound(_)) => Vec::new(),
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "apps/index.json on disk is unreadable during a locked index write; \
                     rewriting it from this instance's records"
                );
                Vec::new()
            }
        };
    let mut merged = records.to_vec();
    for entry in disk {
        if !known_ids.contains(&entry.id) && !merged.iter().any(|r| r.id == entry.id) {
            tracing::warn!(
                app_id = %entry.id,
                "preserving a foreign-process app index entry this instance has never seen"
            );
            merged.push(entry);
        }
    }
    save_index(root, &merged)
}

/// One per-app persisted document, as a step of [`APP_DOC_WRITE_ORDER`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppDocWriteStep {
    /// `workspace/.lingxi/design-spec.json` ([`save_draft`]).
    DesignSpec,
    /// `apps/<id>/interactions.json` ([`save_interactions`]).
    Interactions,
    /// `apps/<id>/runtime.json` ([`save_runtime`]).
    Runtime,
    /// `workspace/.lingxi/app.json` record mirror — always LAST;
    /// [`load_all`]'s torn-commit repair depends on it superseding the index.
    MetadataMirror,
}

/// THE canonical per-app write order, defined exactly once.
/// [`save_app_files`] executes this sequence in full; partial writers (the
/// service's changed-docs mutation path, crash tests replaying a mid-batch
/// tear) must pass subsequences of it to [`save_app_files_steps`] — the
/// repair contract in [`load_all`] depends on the ORDER of the writes that
/// happen, not on every document being rewritten.
pub const APP_DOC_WRITE_ORDER: [AppDocWriteStep; 4] = [
    AppDocWriteStep::DesignSpec,
    AppDocWriteStep::Interactions,
    AppDocWriteStep::Runtime,
    AppDocWriteStep::MetadataMirror,
];

/// The true prefix of [`APP_DOC_WRITE_ORDER`] up to and including `last` —
/// what a crash inside [`save_app_files`] leaves behind (a mid-batch crash
/// always commits a prefix of the canonical sequence, never a reordering).
/// Crash tests replay these instead of hand-picking an assumed order.
#[must_use]
pub fn write_order_prefix_through(last: AppDocWriteStep) -> &'static [AppDocWriteStep] {
    let position = APP_DOC_WRITE_ORDER
        .iter()
        .position(|step| *step == last)
        .expect("every step appears in APP_DOC_WRITE_ORDER");
    &APP_DOC_WRITE_ORDER[..=position]
}

/// Failure from [`save_app_files_steps`]: the underlying error plus how far
/// the batch got. Each step is one atomic write, so the batch always fails
/// BETWEEN steps: the first `written` steps of the requested slice hold their
/// NEW documents on disk, the failing step and everything after it are
/// untouched. Callers use `written` to compensate precisely (the service's
/// mid-batch rollback rewrites exactly the succeeded prefix's originals —
/// finding on `with_app`).
#[derive(Debug)]
pub struct AppBatchWriteFailure {
    /// How many leading steps of the requested slice were durably written
    /// before the failure (their NEW documents are on disk).
    pub written: usize,
    /// The failing step's error.
    pub error: AppError,
}

impl AppBatchWriteFailure {
    /// The failing step's error, discarding the progress report.
    #[must_use]
    pub fn into_error(self) -> AppError {
        self.error
    }
}

/// Atomically persist the given per-app documents for `app`, in the given
/// order. [`save_app_files`] passes the full [`APP_DOC_WRITE_ORDER`]; the
/// service's mutation path passes the subsequence of steps whose documents
/// actually changed; crash tests pass [`write_order_prefix_through`] prefixes
/// to model a mid-batch crash. Does NOT touch the index — callers write the
/// index last (the creation commit point; for mutations a lost index write
/// is repaired forward on load, see the module doc). On failure the error
/// reports WHICH prefix of `steps` already landed (see
/// [`AppBatchWriteFailure`]).
pub fn save_app_files_steps(
    root: &Path,
    app: &AppState,
    steps: &[AppDocWriteStep],
) -> Result<(), AppBatchWriteFailure> {
    let id = &app.record.id;
    for (written, step) in steps.iter().enumerate() {
        let result = match step {
            AppDocWriteStep::DesignSpec => save_draft(root, id, &app.draft),
            AppDocWriteStep::Interactions => save_interactions(root, id, &app.interactions),
            AppDocWriteStep::Runtime => save_runtime(root, id, &app.runtime),
            AppDocWriteStep::MetadataMirror => {
                let mirror = AppMetadataFile {
                    schema_version: APPS_SCHEMA_VERSION,
                    app: app.record.clone(),
                };
                write_doc(root, &metadata_rel(id), &mirror)
            }
        };
        if let Err(error) = result {
            return Err(AppBatchWriteFailure { written, error });
        }
    }
    Ok(())
}

/// Atomically persist every per-app document in [`APP_DOC_WRITE_ORDER`]
/// (draft, interactions, runtime, metadata mirror — the mirror LAST).
pub fn save_app_files(root: &Path, app: &AppState) -> Result<(), AppError> {
    save_app_files_steps(root, app, &APP_DOC_WRITE_ORDER).map_err(AppBatchWriteFailure::into_error)
}

/// Atomically persist `workspace/.lingxi/design-spec.json`.
pub fn save_draft(root: &Path, app_id: &str, draft: &AppDesignDraft) -> Result<(), AppError> {
    write_doc(root, &design_spec_rel(app_id), draft)
}

/// Atomically persist `apps/<id>/interactions.json`.
pub fn save_interactions(
    root: &Path,
    app_id: &str,
    interactions: &AppInteractions,
) -> Result<(), AppError> {
    write_doc(root, &interactions_rel(app_id), interactions)
}

/// Atomically persist `apps/<id>/runtime.json`.
pub fn save_runtime(root: &Path, app_id: &str, runtime: &AppRuntimeRecord) -> Result<(), AppError> {
    write_doc(root, &runtime_rel(app_id), runtime)
}

/// Remove `apps/<id>`: rename-to-trash first (the commit point — see
/// [`trash_app_dir`]), then recursively remove the trash entry. A removal
/// failure AFTER the rename still leaves the id fully out of the `apps/`
/// namespace; the leftover trash entry is swept at the next load.
pub fn delete_app_dir(root: &Path, app_id: &str) -> Result<(), AppError> {
    match trash_app_dir(root, app_id)? {
        None => Ok(()),
        Some(trash_path) => std::fs::remove_dir_all(&trash_path)
            .map_err(|error| AppError::Io(format!("remove {}: {error}", trash_path.display()))),
    }
}

/// COMMIT POINT of app-directory deletion: atomically rename `apps/<id>` into
/// `apps/.trash/<id>-<8-hex nonce>` and return the trash path (`None` when
/// there was nothing to move — the dir is absent, or a squatting
/// symlink/file was removed as the link itself, never followed).
///
/// Why rename instead of `remove_dir_all` in place (TOCTOU finding):
/// `remove_dir_all` re-resolves every intermediate component on each step of
/// its traversal, while `rename` never follows the FINAL component of either
/// path — one atomic metadata operation moves the whole tree out of the
/// `apps/` namespace. The rename is also the tombstone: `mint_app_id`
/// re-checks disk (live dir OR trash entry), so a racing create can never
/// adopt a directory a stale removal is still tearing down.
///
/// The id is re-validated (grammar forbids separators and `..`), the join is
/// lexically checked, and `apps/`, `apps/.trash` and `apps/<id>` must be
/// real directories.
pub fn trash_app_dir(root: &Path, app_id: &str) -> Result<Option<PathBuf>, AppError> {
    ids::validate_app_id(app_id)?;
    let apps_rel = PathBuf::from(APPS_DIR);
    let apps_dir = rooted_fs::checked_join(root, &apps_rel)
        .map_err(|error| AppError::from_fs("delete app dir", &error))?;
    match std::fs::symlink_metadata(&apps_dir) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(AppError::Io(format!(
                "stat {}: {error}",
                apps_dir.display()
            )));
        }
        Ok(meta) if !meta.is_dir() => {
            return Err(AppError::StorageCorrupt(format!(
                "{} is not a real directory",
                apps_dir.display()
            )));
        }
        Ok(_) => {}
    }

    let target = rooted_fs::checked_join(root, &app_dir_rel(app_id))
        .map_err(|error| AppError::from_fs("delete app dir", &error))?;
    match std::fs::symlink_metadata(&target) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(AppError::Io(format!("stat {}: {error}", target.display()))),
        Ok(meta) if meta.is_dir() => {
            let trash_dir = apps_dir.join(TRASH_DIR);
            std::fs::create_dir_all(&trash_dir).map_err(|error| {
                AppError::Io(format!("create {}: {error}", trash_dir.display()))
            })?;
            // A file or symlink squatting on `.trash` must never receive the
            // rename (a symlink would be followed by the create/rename above
            // it); insist on a real directory like `apps/` itself.
            match std::fs::symlink_metadata(&trash_dir) {
                Ok(meta) if meta.is_dir() => {}
                Ok(_) => {
                    return Err(AppError::StorageCorrupt(format!(
                        "{} is not a real directory",
                        trash_dir.display()
                    )));
                }
                Err(error) => {
                    return Err(AppError::Io(format!(
                        "stat {}: {error}",
                        trash_dir.display()
                    )));
                }
            }
            let trash_path = trash_dir.join(format!("{app_id}-{}", ids::generate_app_id()));
            std::fs::rename(&target, &trash_path).map_err(|error| {
                AppError::Io(format!(
                    "rename {} -> {}: {error}",
                    target.display(),
                    trash_path.display()
                ))
            })?;
            Ok(Some(trash_path))
        }
        // A symlink (or stray file) squatting on the app path is removed as
        // the link itself; its target is never followed.
        Ok(_) => {
            std::fs::remove_file(&target)
                .map_err(|error| AppError::Io(format!("remove {}: {error}", target.display())))?;
            Ok(None)
        }
    }
}

/// True when `app_id` is still present ON DISK — a live `apps/<id>` entry (of
/// any file type) or a `.trash/<id>-<nonce>` tombstone. `mint_app_id`
/// consults this in addition to the in-memory list so a freshly-deleted (or
/// orphaned) directory can never be adopted by a same-id create racing a
/// stale removal. Read failures conservatively report "absent" (minting must
/// not wedge on an unreadable trash dir; the collision odds of a random
/// 8-hex id are negligible).
#[must_use]
pub fn app_id_present_on_disk(root: &Path, app_id: &str) -> bool {
    if !ids::is_valid_app_id(app_id) {
        return false;
    }
    let Ok(dir) = rooted_fs::checked_join(root, &app_dir_rel(app_id)) else {
        return false;
    };
    if std::fs::symlink_metadata(&dir).is_ok() {
        return true;
    }
    let Ok(trash) = rooted_fs::checked_join(root, &trash_dir_rel()) else {
        return false;
    };
    let prefix = format!("{app_id}-");
    match std::fs::read_dir(&trash) {
        Ok(entries) => entries
            .flatten()
            .any(|entry| entry.file_name().to_string_lossy().starts_with(&prefix)),
        Err(_) => false,
    }
}

/// Best-effort sweep of `apps/.trash` (leftovers from removals that failed
/// or were interrupted after their commit-point rename). Failures are logged
/// and NEVER fail the load.
fn sweep_trash(root: &Path) {
    let Ok(trash) = rooted_fs::checked_join(root, &trash_dir_rel()) else {
        return;
    };
    let entries = match std::fs::read_dir(&trash) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            tracing::warn!(error = %error, "apps/.trash is unreadable; sweep skipped");
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let removal = match entry.file_type() {
            Ok(kind) if kind.is_dir() => std::fs::remove_dir_all(&path),
            _ => std::fs::remove_file(&path),
        };
        if let Err(error) = removal {
            tracing::warn!(
                path = %path.display(),
                error = %error,
                "trash entry could not be swept; leaving it for the next load"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::AppErrorCode;
    use crate::types::AppTemplateKind;

    fn new_app(id: &str) -> AppState {
        AppState::create(
            id.into(),
            format!("App {id}"),
            AppTemplateKind::CrudTracker,
            Some("conv-9".into()),
            1_700_000_000_000,
        )
    }

    fn save_full(root: &Path, apps: &[AppState]) {
        for app in apps {
            save_app_files(root, app).unwrap();
        }
        let records: Vec<_> = apps.iter().map(|a| a.record.clone()).collect();
        save_index(root, &records).unwrap();
    }

    #[test]
    fn round_trips_full_store() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = new_app("aaaa1111");
        app.open_designer("int-1".into(), 5).unwrap();
        let apps = vec![app, new_app("bbbb2222")];
        save_full(dir.path(), &apps);

        let loaded = load_all(dir.path()).unwrap();
        assert_eq!(loaded, apps);
        // Files live where the spec says.
        assert!(dir.path().join("apps/index.json").is_file());
        assert!(dir.path().join("apps/aaaa1111/runtime.json").is_file());
        assert!(dir.path().join("apps/aaaa1111/interactions.json").is_file());
        assert!(dir
            .path()
            .join("apps/aaaa1111/workspace/.lingxi/app.json")
            .is_file());
        assert!(dir
            .path()
            .join("apps/aaaa1111/workspace/.lingxi/design-spec.json")
            .is_file());
        // Persisted documents are pretty-printed with a trailing newline.
        let body = std::fs::read_to_string(dir.path().join("apps/index.json")).unwrap();
        assert!(body.starts_with("{\n"));
        assert!(body.ends_with("}\n"));
        assert!(body.contains("\"schemaVersion\": 1"));
    }

    #[test]
    fn missing_index_is_empty_store() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load_all(dir.path()).unwrap(), Vec::new());
        // Root without even the apps/ dir is fine too.
        assert_eq!(
            load_all(&dir.path().join("nested-missing")).unwrap(),
            Vec::new()
        );
    }

    #[test]
    fn corrupt_index_is_storage_corrupt_not_silent_reset() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("apps")).unwrap();
        std::fs::write(dir.path().join("apps/index.json"), "{ not json").unwrap();
        let err = load_all(dir.path()).unwrap_err();
        assert_eq!(err.code(), AppErrorCode::StorageCorrupt);
    }

    #[test]
    fn unsupported_schema_version_is_storage_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("apps")).unwrap();
        std::fs::write(
            dir.path().join("apps/index.json"),
            "{\"schemaVersion\": 99, \"apps\": []}\n",
        )
        .unwrap();
        let err = load_all(dir.path()).unwrap_err();
        assert_eq!(err.code(), AppErrorCode::StorageCorrupt);
    }

    #[test]
    fn hostile_app_id_in_index_never_becomes_a_path() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("apps")).unwrap();
        let body = serde_json::json!({
            "schemaVersion": 1,
            "apps": [{
                "id": "../../escape",
                "name": "evil",
                "template": "dashboard",
                "createdAtMs": 1,
                "updatedAtMs": 1,
                "workflowState": "collecting_spec",
                "workspaceRel": "apps/../../escape/workspace"
            }]
        });
        std::fs::write(dir.path().join("apps/index.json"), body.to_string()).unwrap();
        let err = load_all(dir.path()).unwrap_err();
        assert_eq!(err.code(), AppErrorCode::StorageCorrupt);
    }

    /// Finding 7: `workspace_rel` itself is validated against its documented
    /// invariant (`apps/<id>/workspace`, exactly) — a hostile value behind a
    /// perfectly VALID app id is rejected on THIS field, before any per-app
    /// document is read, so repair can never launder it back into the index.
    #[test]
    fn hostile_workspace_rel_with_valid_id_is_rejected_on_that_field() {
        for hostile in [
            "apps/../../escape/workspace",
            "apps/zzzz9999/workspace", // someone ELSE's workspace
            "workspace",
            "/etc",
            "apps/aaaa1111/workspace/", // trailing separator — not exact
        ] {
            let dir = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(dir.path().join("apps")).unwrap();
            let body = serde_json::json!({
                "schemaVersion": 1,
                "apps": [{
                    "id": "aaaa1111",
                    "name": "sneaky",
                    "template": "dashboard",
                    "createdAtMs": 1,
                    "updatedAtMs": 1,
                    "workflowState": "collecting_spec",
                    "workspaceRel": hostile
                }]
            });
            std::fs::write(dir.path().join("apps/index.json"), body.to_string()).unwrap();
            let err = load_all(dir.path()).unwrap_err();
            assert_eq!(err.code(), AppErrorCode::StorageCorrupt, "{hostile}");
            assert!(
                err.to_string().contains("workspaceRel"),
                "rejection must pin the workspaceRel field: {err}"
            );
        }
    }

    /// Finding 7 (mirror side): the `app.json` mirror can supersede the
    /// index record in repair, so its `workspace_rel` is validated BEFORE
    /// adoption — repair never persists a value it did not validate.
    #[test]
    fn tampered_mirror_workspace_rel_is_storage_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let app = new_app("qqqq7777");
        save_full(dir.path(), std::slice::from_ref(&app));
        let mirror_path = dir.path().join("apps/qqqq7777/workspace/.lingxi/app.json");
        let body = std::fs::read_to_string(&mirror_path).unwrap();
        std::fs::write(
            &mirror_path,
            body.replace("apps/qqqq7777/workspace", "apps/../../escape/workspace"),
        )
        .unwrap();
        let err = load_all(dir.path()).unwrap_err();
        assert_eq!(err.code(), AppErrorCode::StorageCorrupt, "{err}");
        assert!(err.to_string().contains("workspaceRel"), "{err}");
    }

    /// Finding 5: a persisted `nextSeq` of 0 (or one at/below
    /// `lastDeliveredSeq`) would mint an invalid seq and brick the FOLLOWING
    /// load — the validator rejects it NOW instead of accept-then-brick.
    #[test]
    fn next_seq_zero_is_storage_corrupt_at_load_not_after_the_next_mint() {
        // The finding's exact tamper: a fresh, otherwise-legal store whose
        // nextSeq was reset to 0.
        let dir = tempfile::tempdir().unwrap();
        let app = new_app("rrrr8888");
        save_full(dir.path(), std::slice::from_ref(&app));
        let mut interactions = app.interactions.clone();
        interactions.next_seq = 0;
        save_interactions(dir.path(), "rrrr8888", &interactions).unwrap();
        let err = load_all(dir.path()).unwrap_err();
        assert_eq!(err.code(), AppErrorCode::StorageCorrupt, "{err}");
        assert!(
            err.to_string().contains("violates continuation invariants"),
            "{err}"
        );
        assert!(err.to_string().contains("mint floor"), "{err}");

        // And the equal-counters variant: the next mint would land AT the
        // delivered bound.
        let dir = tempfile::tempdir().unwrap();
        let mut app = new_app("rrrr8888");
        app.open_designer("int-1".into(), 2).unwrap();
        app.cancel_design(3).unwrap();
        app.interactions.undelivered.clear();
        app.interactions.last_delivered_seq = 1; // delivered
        save_full(dir.path(), std::slice::from_ref(&app));
        let mut interactions = app.interactions.clone();
        interactions.next_seq = 1; // == lastDeliveredSeq
        save_interactions(dir.path(), "rrrr8888", &interactions).unwrap();
        let err = load_all(dir.path()).unwrap_err();
        assert_eq!(err.code(), AppErrorCode::StorageCorrupt, "{err}");
        assert!(err.to_string().contains("does not exceed"), "{err}");
    }

    /// Finding 6: every embedded `app_id` — the pending gate's, each
    /// undelivered continuation's, and the runtime record's — must name the
    /// owning app; a mismatch is `storage_corrupt` like the mirror id.
    #[test]
    fn embedded_app_id_mismatches_are_storage_corrupt() {
        // interactions.pending.app_id
        let dir = tempfile::tempdir().unwrap();
        let mut app = new_app("ssss1111");
        app.open_designer("int-1".into(), 2).unwrap();
        save_full(dir.path(), std::slice::from_ref(&app));
        let mut interactions = app.interactions.clone();
        interactions.pending.as_mut().unwrap().app_id = "tttt2222".into();
        save_interactions(dir.path(), "ssss1111", &interactions).unwrap();
        let err = load_all(dir.path()).unwrap_err();
        assert_eq!(err.code(), AppErrorCode::StorageCorrupt, "pending: {err}");
        assert!(
            err.to_string().contains("pending gate claims app id"),
            "{err}"
        );

        // undelivered[*].app_id
        let dir = tempfile::tempdir().unwrap();
        let mut app = new_app("ssss1111");
        app.open_designer("int-1".into(), 2).unwrap();
        app.cancel_design(3).unwrap();
        save_full(dir.path(), std::slice::from_ref(&app));
        let mut interactions = app.interactions.clone();
        interactions.undelivered[0].app_id = "tttt2222".into();
        save_interactions(dir.path(), "ssss1111", &interactions).unwrap();
        let err = load_all(dir.path()).unwrap_err();
        assert_eq!(
            err.code(),
            AppErrorCode::StorageCorrupt,
            "undelivered: {err}"
        );
        assert!(err.to_string().contains("claims app id"), "{err}");

        // runtime.app_id
        let dir = tempfile::tempdir().unwrap();
        let app = new_app("ssss1111");
        save_full(dir.path(), std::slice::from_ref(&app));
        let mut runtime = app.runtime.clone();
        runtime.app_id = "tttt2222".into();
        save_runtime(dir.path(), "ssss1111", &runtime).unwrap();
        let err = load_all(dir.path()).unwrap_err();
        assert_eq!(err.code(), AppErrorCode::StorageCorrupt, "runtime: {err}");
        assert!(err.to_string().contains("claims app id"), "{err}");
    }

    /// Finding 10: the rename into `apps/.trash` is the deletion's commit
    /// point — after it, the id is fully out of the `apps/` namespace (a
    /// same-id create gets a FRESH directory; nothing is adopted) while the
    /// content awaits removal under the trash path, and the tombstone still
    /// blocks re-minting the id.
    #[test]
    fn trash_rename_commits_the_deletion_before_removal() {
        let dir = tempfile::tempdir().unwrap();
        let app = new_app("uuuu3333");
        save_full(dir.path(), std::slice::from_ref(&app));
        let marker = dir.path().join("apps/uuuu3333/workspace/user-data.txt");
        std::fs::write(&marker, b"precious").unwrap();

        let trash_path = trash_app_dir(dir.path(), "uuuu3333")
            .unwrap()
            .expect("a live dir moves to trash");
        // Committed: the live path is gone even though nothing was removed
        // yet; the content sits intact under the returned trash path.
        assert!(!dir.path().join("apps/uuuu3333").exists());
        assert!(trash_path.join("workspace/user-data.txt").is_file());
        // The tombstone still pins the id against re-minting…
        assert!(app_id_present_on_disk(dir.path(), "uuuu3333"));
        // …but a same-id create-after-delete writes a FRESH workspace with
        // no trace of the trashed content (no orphan adoption).
        let recreated = new_app("uuuu3333");
        save_app_files(dir.path(), &recreated).unwrap();
        assert!(!dir
            .path()
            .join("apps/uuuu3333/workspace/user-data.txt")
            .exists());
        assert!(dir
            .path()
            .join("apps/uuuu3333/workspace/.lingxi/app.json")
            .is_file());
        // The old content is still only in the trash, and the slow removal
        // targets the nonce'd trash path — never the recreated dir.
        std::fs::remove_dir_all(&trash_path).unwrap();
        assert!(dir
            .path()
            .join("apps/uuuu3333/workspace/.lingxi/app.json")
            .is_file());
    }

    /// Finding 10: `.trash` leftovers are swept (best-effort) at the next
    /// load, and the sweep never fails the load; `load_all` itself ignores
    /// `.trash` entirely (enumeration is index-driven).
    #[test]
    fn trash_leftovers_are_swept_at_load() {
        let dir = tempfile::tempdir().unwrap();
        let keep = new_app("vvvv4444");
        let gone = new_app("wwww5555");
        save_app_files(dir.path(), &keep).unwrap();
        save_app_files(dir.path(), &gone).unwrap();
        save_index(dir.path(), &[keep.record.clone()]).unwrap();
        // A removal that renamed but never finished deleting.
        let trash_path = trash_app_dir(dir.path(), "wwww5555").unwrap().unwrap();
        assert!(trash_path.exists());
        assert!(app_id_present_on_disk(dir.path(), "wwww5555"));

        let loaded = load_all(dir.path()).unwrap();
        assert_eq!(
            loaded,
            vec![keep],
            ".trash must be invisible to enumeration"
        );
        assert!(!trash_path.exists(), "the leftover tombstone is swept");
        assert!(!app_id_present_on_disk(dir.path(), "wwww5555"));
    }

    /// Finding 10: `app_id_present_on_disk` truth table — live dir, trash
    /// tombstone, absent.
    #[test]
    fn app_id_disk_presence_covers_live_and_trash_entries() {
        let dir = tempfile::tempdir().unwrap();
        let app = new_app("xxxx6666");
        save_full(dir.path(), std::slice::from_ref(&app));
        assert!(app_id_present_on_disk(dir.path(), "xxxx6666"), "live dir");
        assert!(!app_id_present_on_disk(dir.path(), "yyyy7777"), "absent");
        let trash_path = trash_app_dir(dir.path(), "xxxx6666").unwrap().unwrap();
        assert!(
            app_id_present_on_disk(dir.path(), "xxxx6666"),
            "trash tombstone still pins the id"
        );
        // Another id sharing a PREFIX is not confused with the tombstone.
        assert!(!app_id_present_on_disk(dir.path(), "xxxx666"));
        std::fs::remove_dir_all(trash_path).unwrap();
        assert!(
            !app_id_present_on_disk(dir.path(), "xxxx6666"),
            "fully gone"
        );
    }

    /// Finding 9: `save_index_preserving` keeps foreign-process entries this
    /// writer has never seen, stays authoritative for known ids (deletions
    /// included), and never duplicates.
    #[test]
    fn save_index_preserving_merges_foreign_entries_and_honors_deletions() {
        let dir = tempfile::tempdir().unwrap();
        let ours = new_app("aaaa1111");
        let foreign = new_app("bbbb2222");
        let deleted = new_app("cccc3333");
        // Disk currently lists the foreign app and one we are deleting.
        save_index(
            dir.path(),
            &[foreign.record.clone(), deleted.record.clone()],
        )
        .unwrap();
        // We know about `ours` (writing it) and `deleted` (we deleted it);
        // the foreign entry is unknown to us and must survive.
        let known: BTreeSet<String> = [ours.record.id.clone(), deleted.record.id.clone()].into();
        save_index_preserving(dir.path(), std::slice::from_ref(&ours.record), &known).unwrap();

        let body = std::fs::read_to_string(dir.path().join("apps/index.json")).unwrap();
        let index: AppIndexFile = serde_json::from_str(&body).unwrap();
        let ids: Vec<&str> = index.apps.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["aaaa1111", "bbbb2222"],
            "ours first, foreign preserved, our deletion NOT resurrected"
        );
    }

    #[test]
    fn duplicate_app_id_in_index_is_storage_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let app = new_app("hhhh8888");
        save_full(dir.path(), std::slice::from_ref(&app));
        // A restored/merged backup (or hand edit) lists the same id twice.
        save_index(dir.path(), &[app.record.clone(), app.record.clone()]).unwrap();
        let err = load_all(dir.path()).unwrap_err();
        assert_eq!(err.code(), AppErrorCode::StorageCorrupt);
    }

    #[test]
    fn listed_app_with_missing_document_is_storage_corrupt() {
        for missing in [
            "apps/cccc3333/interactions.json",
            "apps/cccc3333/workspace/.lingxi/app.json",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let app = new_app("cccc3333");
            save_full(dir.path(), &[app]);
            std::fs::remove_file(dir.path().join(missing)).unwrap();
            let err = load_all(dir.path()).unwrap_err();
            assert_eq!(err.code(), AppErrorCode::StorageCorrupt, "{missing}");
        }
    }

    /// Every per-app document's `schemaVersion` guard must fire — not just the
    /// index-level one. A future v2 document loaded by a v1 binary must fail
    /// typed instead of being silently field-misinterpreted.
    #[test]
    fn unsupported_per_app_doc_schema_version_is_storage_corrupt() {
        for doc in [
            "apps/iiii9999/workspace/.lingxi/design-spec.json",
            "apps/iiii9999/interactions.json",
            "apps/iiii9999/runtime.json",
            "apps/iiii9999/workspace/.lingxi/app.json",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let app = new_app("iiii9999");
            save_full(dir.path(), &[app]);
            let path = dir.path().join(doc);
            let body = std::fs::read_to_string(&path).unwrap();
            assert!(
                body.contains("\"schemaVersion\": 1"),
                "{doc} must carry the schema version"
            );
            std::fs::write(
                &path,
                body.replacen("\"schemaVersion\": 1", "\"schemaVersion\": 99", 1),
            )
            .unwrap();
            let err = load_all(dir.path()).unwrap_err();
            assert_eq!(err.code(), AppErrorCode::StorageCorrupt, "{doc}");
            assert!(
                err.to_string().contains("unsupported schemaVersion 99"),
                "{doc}: {err}"
            );
        }
    }

    /// A document above [`MAX_DOC_BYTES`] is out of contract: it must fail
    /// typed `storage_corrupt` instead of being slurped unbounded into memory.
    #[test]
    fn oversized_document_is_storage_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let app = new_app("jjjj0000");
        save_full(dir.path(), &[app]);
        let path = dir.path().join("apps/jjjj0000/runtime.json");
        let oversized = vec![b' '; usize::try_from(MAX_DOC_BYTES).unwrap() + 1];
        std::fs::write(&path, oversized).unwrap();
        let err = load_all(dir.path()).unwrap_err();
        assert_eq!(err.code(), AppErrorCode::StorageCorrupt);
        assert!(err.to_string().contains("limit"), "{err}");
    }

    #[test]
    fn orphan_temp_files_are_ignored_and_index_survives_partial_write() {
        let dir = tempfile::tempdir().unwrap();
        let apps = vec![new_app("dddd4444")];
        save_full(dir.path(), &apps);
        // Simulate a crashed atomic write: an orphan temp with garbage next to
        // the real index (rooted_fs names temps `<final>.tmp-<pid>-<seq>`),
        // plus one inside the app directory.
        std::fs::write(
            dir.path().join("apps/index.json.tmp-1234-7"),
            "GARBAGE-PARTIAL-WRITE",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("apps/dddd4444/interactions.json.tmp-1-1"),
            "{\"half\":",
        )
        .unwrap();
        let loaded = load_all(dir.path()).unwrap();
        assert_eq!(loaded, apps);
        // A later write still lands atomically despite the orphans.
        save_index(dir.path(), &[loaded[0].record.clone()]).unwrap();
        assert_eq!(load_all(dir.path()).unwrap(), apps);
    }

    #[test]
    fn delete_app_dir_removes_only_the_contained_dir() {
        let dir = tempfile::tempdir().unwrap();
        let apps = vec![new_app("eeee5555"), new_app("ffff6666")];
        save_full(dir.path(), &apps);
        delete_app_dir(dir.path(), "eeee5555").unwrap();
        assert!(!dir.path().join("apps/eeee5555").exists());
        assert!(dir.path().join("apps/ffff6666/runtime.json").is_file());
        // Deleting a missing dir is a no-op.
        delete_app_dir(dir.path(), "eeee5555").unwrap();
    }

    #[test]
    fn delete_app_dir_rejects_traversal_ids() {
        let dir = tempfile::tempdir().unwrap();
        for bad in ["../evil", "a/b", "..", "UPPER", ""] {
            let err = delete_app_dir(dir.path(), bad).unwrap_err();
            assert_eq!(err.code(), AppErrorCode::InvalidRequest, "id {bad:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn delete_app_dir_removes_a_planted_symlink_without_following_it() {
        let dir = tempfile::tempdir().unwrap();
        let victim = tempfile::tempdir().unwrap();
        std::fs::write(victim.path().join("precious.txt"), "keep me").unwrap();
        std::fs::create_dir_all(dir.path().join("apps")).unwrap();
        std::os::unix::fs::symlink(victim.path(), dir.path().join("apps/gggg7777")).unwrap();
        delete_app_dir(dir.path(), "gggg7777").unwrap();
        assert!(!dir.path().join("apps/gggg7777").exists());
        assert!(
            victim.path().join("precious.txt").is_file(),
            "symlink target must survive"
        );
    }

    /// The write seam enforces the SAME size bound loads do: a document the
    /// loader would refuse never reaches disk, so an over-cap mutation fails
    /// typed and the store on disk stays loadable.
    #[test]
    fn oversized_write_fails_typed_and_leaves_the_store_loadable() {
        use crate::types::DesignValue;
        let dir = tempfile::tempdir().unwrap();
        let mut app = new_app("kkkk1111");
        save_full(dir.path(), std::slice::from_ref(&app));

        let over = usize::try_from(MAX_DOC_BYTES).unwrap() + 1;
        app.draft
            .fields
            .insert("huge".into(), DesignValue::LongText("x".repeat(over)));
        let err = save_draft(dir.path(), "kkkk1111", &app.draft).unwrap_err();
        assert_eq!(err.code(), AppErrorCode::InvalidRequest);
        assert!(
            err.to_string().contains("durable size limit"),
            "unexpected message: {err}"
        );

        // Disk was never touched: the store still loads cleanly and holds the
        // committed (empty) draft.
        let loaded = load_all(dir.path()).unwrap();
        assert_eq!(loaded.len(), 1);
        assert!(loaded[0].draft.fields.is_empty());
    }

    /// Finding 6: every continuation-counter invariant violation class fails
    /// `storage_corrupt` at load instead of feeding garbage into repair or
    /// redelivery.
    #[test]
    fn interactions_counter_invariant_violations_are_storage_corrupt() {
        type Tamper = Box<dyn Fn(&mut AppInteractions)>;
        let base = || {
            let mut app = new_app("llll2222");
            app.open_designer("int-1".into(), 1_700_000_000_001)
                .unwrap();
            app.cancel_design(1_700_000_000_002).unwrap();
            app
        };
        let cases: Vec<(&str, Tamper)> = vec![
            (
                "undelivered seq not above lastDeliveredSeq",
                Box::new(|ints| ints.last_delivered_seq = 1),
            ),
            (
                "undelivered seq at/above nextSeq (never minted)",
                Box::new(|ints| ints.undelivered[0].seq = 9),
            ),
            (
                "undelivered seqs not strictly increasing",
                Box::new(|ints| {
                    let duplicate = ints.undelivered[0].clone();
                    ints.undelivered.push(duplicate);
                    ints.next_seq = 3;
                }),
            ),
            (
                "nextSeq trails lastDeliveredSeq",
                Box::new(|ints| {
                    ints.undelivered.clear();
                    ints.next_seq = 3;
                    ints.last_delivered_seq = 5;
                }),
            ),
        ];
        for (label, tamper) in cases {
            let dir = tempfile::tempdir().unwrap();
            let app = base();
            save_full(dir.path(), std::slice::from_ref(&app));
            let mut interactions = app.interactions.clone();
            tamper(&mut interactions);
            save_interactions(dir.path(), "llll2222", &interactions).unwrap();
            let err = load_all(dir.path()).unwrap_err();
            assert_eq!(err.code(), AppErrorCode::StorageCorrupt, "{label}: {err}");
            assert!(
                err.to_string().contains("violates continuation invariants"),
                "{label}: {err}"
            );
        }
        // The untampered base state passes the validation.
        let dir = tempfile::tempdir().unwrap();
        let app = base();
        save_full(dir.path(), std::slice::from_ref(&app));
        assert_eq!(load_all(dir.path()).unwrap(), vec![app]);
    }

    #[test]
    fn write_order_is_the_canonical_four_step_sequence() {
        assert_eq!(
            APP_DOC_WRITE_ORDER,
            [
                AppDocWriteStep::DesignSpec,
                AppDocWriteStep::Interactions,
                AppDocWriteStep::Runtime,
                AppDocWriteStep::MetadataMirror,
            ],
            "the canonical order is part of the repair contract"
        );
        assert_eq!(
            write_order_prefix_through(AppDocWriteStep::Interactions),
            &APP_DOC_WRITE_ORDER[..2]
        );
        assert_eq!(
            write_order_prefix_through(AppDocWriteStep::MetadataMirror),
            &APP_DOC_WRITE_ORDER[..]
        );
    }

    /// Behavioral pin of the write ORDER (not just the const): failing one
    /// document's write mid-batch must leave exactly the canonical prefix
    /// updated. Reordering `save_app_files` breaks this test.
    #[cfg(unix)]
    #[test]
    fn save_app_files_executes_the_canonical_write_order() {
        let dir = tempfile::tempdir().unwrap();
        let before = new_app("mmmm3333");
        save_full(dir.path(), std::slice::from_ref(&before));

        // A distinct after-state in every document (the draft revision is
        // poked directly; only the serialized difference matters here).
        let mut after = before.clone();
        after.draft.revision = 1;
        after
            .open_designer("int-order".into(), 1_700_000_000_100)
            .unwrap();
        after
            .set_runtime(
                AppRuntimeState::Starting,
                Some(3999),
                Some(7),
                None,
                1_700_000_000_101,
            )
            .unwrap();

        let squat = |rel: PathBuf| {
            let path = dir.path().join(&rel);
            std::fs::remove_file(&path).unwrap();
            std::os::unix::fs::symlink("/dev/null", &path).unwrap();
        };
        let unsquat = |rel: PathBuf, body: &str| {
            let path = dir.path().join(&rel);
            std::fs::remove_file(&path).unwrap();
            std::fs::write(&path, body).unwrap();
        };
        let doc = |rel: PathBuf| std::fs::read_to_string(dir.path().join(rel)).unwrap();

        // Fail at interactions.json: design-spec (earlier) must be updated,
        // runtime + mirror (later) must not.
        let interactions_before = doc(interactions_rel("mmmm3333"));
        squat(interactions_rel("mmmm3333"));
        save_app_files(dir.path(), &after).unwrap_err();
        assert!(
            doc(design_spec_rel("mmmm3333")).contains("\"revision\": 1"),
            "design-spec is written before interactions"
        );
        assert!(
            !doc(runtime_rel("mmmm3333")).contains("starting"),
            "runtime must not be written before interactions"
        );
        assert!(
            !doc(metadata_rel("mmmm3333")).contains("awaiting_spec_confirmation"),
            "the mirror must not be written before interactions"
        );
        unsquat(interactions_rel("mmmm3333"), &interactions_before);

        // Fail at runtime.json: interactions (earlier) updated, mirror not.
        squat(runtime_rel("mmmm3333"));
        save_app_files(dir.path(), &after).unwrap_err();
        assert!(
            doc(interactions_rel("mmmm3333")).contains("int-order"),
            "interactions are written before runtime"
        );
        assert!(
            !doc(metadata_rel("mmmm3333")).contains("awaiting_spec_confirmation"),
            "the mirror must be the LAST write"
        );
    }

    /// Finding 8: a runtime record stranded busy by a crash is reconciled at
    /// load (phase 1 has no process manager) and the reconciliation is
    /// persisted.
    #[test]
    fn stranded_busy_runtime_states_are_reconciled_at_load() {
        use crate::types::AppRuntimeState::{Failed, Running, Starting, Stopped, Stopping};
        let cases = [
            (Starting, Failed, true),
            (Running, Failed, true),
            (Stopping, Stopped, false),
        ];
        for (stranded, expected, expect_error) in cases {
            let dir = tempfile::tempdir().unwrap();
            let mut app = new_app("nnnn4444");
            // Walk legal runtime transitions up to the stranded state.
            app.set_runtime(Starting, Some(3123), Some(42), None, 2)
                .unwrap();
            if matches!(stranded, Running | Stopping) {
                app.set_runtime(Running, None, Some(42), None, 3).unwrap();
            }
            if stranded == Stopping {
                app.set_runtime(Stopping, None, Some(42), None, 4).unwrap();
            }
            save_full(dir.path(), std::slice::from_ref(&app));

            let loaded = load_all(dir.path()).unwrap();
            assert_eq!(loaded[0].runtime.state, expected, "{stranded}");
            assert_eq!(
                loaded[0].runtime.last_error.as_deref(),
                expect_error.then_some("reconciled at load: no live runtime manager"),
                "{stranded}"
            );
            assert_eq!(loaded[0].runtime.port, Some(3123), "port pin survives");
            // The reconciliation is persisted: a second load sees the exact
            // same (already consistent) state.
            assert_eq!(load_all(dir.path()).unwrap(), loaded, "{stranded}");
            let body =
                std::fs::read_to_string(dir.path().join("apps/nnnn4444/runtime.json")).unwrap();
            assert!(
                body.contains(&format!("\"{expected}\"")),
                "{stranded}: {body}"
            );
        }
        // stopped / failed records are untouched (no rewrite).
        let dir = tempfile::tempdir().unwrap();
        let app = new_app("nnnn4444");
        save_full(dir.path(), std::slice::from_ref(&app));
        let before =
            std::fs::read_to_string(dir.path().join("apps/nnnn4444/runtime.json")).unwrap();
        assert_eq!(load_all(dir.path()).unwrap(), vec![app]);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("apps/nnnn4444/runtime.json")).unwrap(),
            before,
            "a settled runtime record must load read-only"
        );
    }

    /// Finding 11: a non-UTF-8 document is store tampering/corruption, not a
    /// generic I/O failure.
    #[test]
    fn non_utf8_document_is_storage_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let app = new_app("oooo5555");
        save_full(dir.path(), std::slice::from_ref(&app));
        std::fs::write(
            dir.path().join("apps/oooo5555/runtime.json"),
            [0xff, 0xfe, 0x00, 0x01],
        )
        .unwrap();
        let err = load_all(dir.path()).unwrap_err();
        assert_eq!(err.code(), AppErrorCode::StorageCorrupt, "{err}");
        assert!(err.to_string().contains("not valid UTF-8"), "{err}");

        // Same contract for the index document itself.
        std::fs::write(dir.path().join("apps/index.json"), [0xff, 0xfe]).unwrap();
        let err = load_all(dir.path()).unwrap_err();
        assert_eq!(err.code(), AppErrorCode::StorageCorrupt, "{err}");
    }

    /// Finding 11: a symlink squatting on a document path is store tampering
    /// (`storage_corrupt`), not a malformed caller request.
    #[cfg(unix)]
    #[test]
    fn symlink_squatting_on_a_document_path_is_storage_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let victim = tempfile::tempdir().unwrap();
        std::fs::write(victim.path().join("payload.json"), "{}").unwrap();
        let app = new_app("pppp6666");
        save_full(dir.path(), std::slice::from_ref(&app));
        let target = dir.path().join("apps/pppp6666/interactions.json");
        std::fs::remove_file(&target).unwrap();
        std::os::unix::fs::symlink(victim.path().join("payload.json"), &target).unwrap();
        let err = load_all(dir.path()).unwrap_err();
        assert_eq!(err.code(), AppErrorCode::StorageCorrupt, "{err}");
        assert!(err.to_string().contains("regular file"), "{err}");
    }
}

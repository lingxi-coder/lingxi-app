//! Atomic on-disk storage for local apps (spec §D).
//!
//! Layout under the injected data root:
//!
//! ```text
//! apps/index.json                          — { schemaVersion, apps }
//! apps/<app-id>/runtime.json               — AppRuntimeRecord
//! apps/<app-id>/dependencies.json          — AppDependencyRecord
//! apps/<app-id>/workspace/.lingxi/app.json — { schemaVersion, app } mirror
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
//! point instead: the batch lands `runtime.json` and finally the `app.json`
//! record mirror — the order is defined ONCE as [`APP_DOC_WRITE_ORDER`] and
//! executed by [`save_app_files_steps`] — and only then does the caller
//! rewrite the index. The repair contract depends on that ORDER, not on
//! completeness: a writer may skip documents that did not change (the
//! service's mutation path does), but the writes that DO happen must follow
//! the canonical sequence. [`load_all`] reconciles on load: a diverged
//! `app.json` mirror (written after every other per-app document) supersedes
//! the index record. Repairs are persisted through the normal write path
//! before the load returns.
//!
//! Legacy pipeline documents (`interactions.json`, `design-spec.json`) from
//! pre-v3 stores are simply IGNORED — never read, never deleted. That, plus
//! the legacy-state serde aliases on
//! [`crate::types::AppWorkflowState`], IS the on-disk migration.
//!
//! Cross-process coordination: `apps/index.json` is read-modify-written under
//! the advisory `apps/index.lock` file lock ([`lock_exclusive`]-style, the
//! same pattern sibling crates use for shared spool files). [`load_all`] holds
//! it across the whole load (read + repair persists) and
//! [`save_index_preserving`] holds it across its re-read + merge + write, so
//! two service instances over one root cannot clobber each other's index
//! entries. Per-app build promotion, checkpoint mutation, and deletion share
//! `apps/<id>/build.lock`, while host-owned background task claims use the
//! independent `apps/<id>/background.lock`, so a second engine process cannot
//! execute the same durable flow concurrently. Deletion is
//! rename-to-trash: `apps/<id>` is atomically renamed
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
    AppDependencyRecord, AppDependencyState, AppRecord, AppRuntimeRecord, AppRuntimeState,
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
/// written). The write seam is where the invariant is enforced: nothing this
/// module persists can later fail its own load on size.
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
/// Per-app advisory lock serializing build promotion and physical deletion.
/// This is a runtime artifact, not a persisted document.
pub const BUILD_LOCK_FILE: &str = "build.lock";
/// Per-app advisory lock serializing host-owned background task claim and
/// terminal state transitions across foreground/headless engine instances.
/// This is a runtime artifact, not a persisted document.
pub const BACKGROUND_LOCK_FILE: &str = "background.lock";
/// Tombstone directory for deleted app dirs (`apps/.trash`). Never a legal
/// app id (ids cannot start with `.`), invisible to the index-driven
/// [`load_all`], swept best-effort at load.
pub const TRASH_DIR: &str = ".trash";
/// Per-app runtime record.
pub const RUNTIME_FILE: &str = "runtime.json";
/// Per-app dependency-install record.
pub const DEPENDENCY_FILE: &str = "dependencies.json";
/// Per-app workspace directory (the Next.js project root).
pub const WORKSPACE_DIR: &str = "workspace";
/// App-scoped state directory inside the workspace (`.lingxi`).
pub const APP_STATE_DIR: &str = branding::DOT_DIR;
/// App-scoped metadata mirror inside `workspace/.lingxi/`.
pub const APP_METADATA_FILE: &str = "app.json";

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

/// Root-relative path of the per-app build/deletion lock.
#[must_use]
pub fn build_lock_rel(app_id: &str) -> PathBuf {
    app_dir_rel(app_id).join(BUILD_LOCK_FILE)
}

/// Root-relative path of the per-app background execution lock.
#[must_use]
pub fn background_lock_rel(app_id: &str) -> PathBuf {
    app_dir_rel(app_id).join(BACKGROUND_LOCK_FILE)
}

/// Take the advisory lock shared by local-app builds and physical deletion.
///
/// Callers must hold this lock for the complete operation that mutates or
/// removes an app's workspace/build tree. The app directory must already
/// exist; unlike the index lock, this helper deliberately does not create an
/// app directory as a side effect.
pub fn lock_app_build(root: &Path, app_id: &str) -> Result<rooted_fs::RootedFileLock, AppError> {
    ids::validate_app_id(app_id)?;
    let app_dir = rooted_fs::checked_join(root, &app_dir_rel(app_id))
        .map_err(|error| AppError::from_fs("lock app build", &error))?;
    match std::fs::symlink_metadata(&app_dir) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => {
            return Err(AppError::StorageCorrupt(format!(
                "{} is not a real directory",
                app_dir.display()
            )));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(AppError::NotFound(format!("app {app_id}")));
        }
        Err(error) => {
            return Err(AppError::Io(format!(
                "inspect {} before locking: {error}",
                app_dir.display()
            )));
        }
    }
    rooted_fs::lock_exclusive(
        root,
        &build_lock_rel(app_id),
        rooted_fs::PRIVATE_DIR_MODE,
        rooted_fs::PRIVATE_FILE_MODE,
    )
    .map_err(|error| match error {
        FsError::NotFound(_) => AppError::NotFound(format!("app {app_id}")),
        other => AppError::from_fs("lock app build", &other),
    })
}

/// Take the advisory lock shared by host-owned background execution.
///
/// This lock is independent from `build.lock`: a long-running headless step
/// must not block an unrelated build, while two engine instances must never
/// claim the same durable task.
pub fn lock_app_background(
    root: &Path,
    app_id: &str,
) -> Result<rooted_fs::RootedFileLock, AppError> {
    ids::validate_app_id(app_id)?;
    let app_dir = rooted_fs::checked_join(root, &app_dir_rel(app_id))
        .map_err(|error| AppError::from_fs("lock app background", &error))?;
    match std::fs::symlink_metadata(&app_dir) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => {
            return Err(AppError::StorageCorrupt(format!(
                "{} is not a real directory",
                app_dir.display()
            )));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(AppError::NotFound(format!("app {app_id}")));
        }
        Err(error) => {
            return Err(AppError::Io(format!(
                "inspect {} before locking: {error}",
                app_dir.display()
            )));
        }
    }
    rooted_fs::lock_exclusive(
        root,
        &background_lock_rel(app_id),
        rooted_fs::PRIVATE_DIR_MODE,
        rooted_fs::PRIVATE_FILE_MODE,
    )
    .map_err(|error| match error {
        FsError::NotFound(_) => AppError::NotFound(format!("app {app_id}")),
        other => AppError::from_fs("lock app background", &other),
    })
}

fn lock_app_build_if_present(
    root: &Path,
    app_id: &str,
) -> Result<Option<rooted_fs::RootedFileLock>, AppError> {
    ids::validate_app_id(app_id)?;
    let app_dir = rooted_fs::checked_join(root, &app_dir_rel(app_id))
        .map_err(|error| AppError::from_fs("lock app deletion", &error))?;
    match std::fs::symlink_metadata(&app_dir) {
        Ok(metadata) if metadata.is_dir() => match lock_app_build(root, app_id) {
            Ok(lock) => Ok(Some(lock)),
            Err(AppError::NotFound(_)) => Ok(None),
            Err(error) => Err(error),
        },
        Ok(_) => Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(AppError::Io(format!(
            "inspect {} before deleting: {error}",
            app_dir.display()
        ))),
    }
}

fn lock_app_background_if_present(
    root: &Path,
    app_id: &str,
) -> Result<Option<rooted_fs::RootedFileLock>, AppError> {
    ids::validate_app_id(app_id)?;
    let app_dir = rooted_fs::checked_join(root, &app_dir_rel(app_id))
        .map_err(|error| AppError::from_fs("lock app background deletion", &error))?;
    match std::fs::symlink_metadata(&app_dir) {
        Ok(metadata) if metadata.is_dir() => match lock_app_background(root, app_id) {
            Ok(lock) => Ok(Some(lock)),
            Err(AppError::NotFound(_)) => Ok(None),
            Err(error) => Err(error),
        },
        Ok(_) => Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(AppError::Io(format!(
            "inspect {} before deleting: {error}",
            app_dir.display()
        ))),
    }
}

/// Root-relative path of `apps/<id>/runtime.json`.
#[must_use]
pub fn runtime_rel(app_id: &str) -> PathBuf {
    app_dir_rel(app_id).join(RUNTIME_FILE)
}

/// Root-relative path of `apps/<id>/dependencies.json`.
#[must_use]
pub fn dependency_rel(app_id: &str) -> PathBuf {
    app_dir_rel(app_id).join(DEPENDENCY_FILE)
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

/// The dependency record used for a newly-created app or a legacy app whose
/// dependency file predates this record.
#[must_use]
pub fn default_dependency_record(app_id: &str, now_ms: u64) -> AppDependencyRecord {
    AppDependencyRecord {
        schema_version: APPS_SCHEMA_VERSION,
        app_id: app_id.to_string(),
        state: AppDependencyState::Queued,
        lockfile_sha256: None,
        toolchain_key: None,
        install_attempts: 0,
        last_error: None,
        updated_at_ms: now_ms,
    }
}

fn derived_dependency_record(root: &Path, record: &AppRecord) -> AppDependencyRecord {
    let vite = root
        .join(workspace_dir_rel(&record.id))
        .join("node_modules/vite/bin/vite.js");
    let state = match std::fs::symlink_metadata(&vite) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            AppDependencyState::Ready
        }
        _ => AppDependencyState::Queued,
    };
    AppDependencyRecord {
        state,
        ..default_dependency_record(&record.id, record.updated_at_ms)
    }
}

/// Root-relative path of `apps/<id>/workspace/.lingxi/app.json`.
#[must_use]
pub fn metadata_rel(app_id: &str) -> PathBuf {
    workspace_dir_rel(app_id)
        .join(APP_STATE_DIR)
        .join(APP_METADATA_FILE)
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
/// detected here (mirror/index divergence — see the module doc) and repaired
/// forward, and a runtime record stranded busy by a crash is reconciled (no
/// runtime process outlives the engine — see [`reconcile_runtime_at_load`]);
/// both are persisted before returning.
///
/// Legacy pipeline documents (`interactions.json`, `design-spec.json`) are
/// NOT read — stale files from old stores are simply ignored on disk.
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
    let index: AppIndexFile = serde_json::from_str(&body).map_err(|error| {
        // A template-era `apps/index.json` fails to parse (it lacks the
        // now-required `brief` field) like any other shape drift — but unlike
        // an ordinary corruption, this ISN'T a bug to report, it's an
        // intentionally unreadable legacy format: give a message that says so
        // instead of leaking the raw serde path. The check is on the raw
        // bytes, not the (already-failed) typed value, so it fires
        // regardless of where in the document the old `"template"` field
        // happened to sit.
        if body.as_bytes().windows(10).any(|w| w == b"\"template\"") {
            AppError::StorageCorrupt(
                "此版本不再支持模版时代的 app 记录（apps/index.json 含 template 字段）；\
                 请删除 apps/ 目录后重新创建应用 / no longer supports template-era app records"
                    .into(),
            )
        } else {
            AppError::StorageCorrupt(format!("apps/index.json: {error}"))
        }
    })?;
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

        let mut app = AppState { record, runtime };
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

/// Load one app's dependency record. Missing files are derived for backwards
/// compatibility and become durable the next time the host updates them.
pub fn load_dependency_record(
    root: &Path,
    record: &AppRecord,
) -> Result<AppDependencyRecord, AppError> {
    let dependency_rel = dependency_rel(&record.id);
    match rooted_fs::read_to_string_limited(root, &dependency_rel, MAX_DOC_BYTES) {
        Ok(body) => {
            let dependency: AppDependencyRecord = serde_json::from_str(&body).map_err(|error| {
                AppError::StorageCorrupt(format!("{}: {error}", dependency_rel.display()))
            })?;
            ensure_schema_version(&dependency_rel, dependency.schema_version)?;
            if dependency.app_id != record.id {
                return Err(AppError::StorageCorrupt(format!(
                    "{} claims app id {:?} but belongs to app {:?}",
                    dependency_rel.display(),
                    dependency.app_id,
                    record.id
                )));
            }
            Ok(dependency)
        }
        Err(FsError::NotFound(_)) => Ok(derived_dependency_record(root, record)),
        Err(error) => Err(load_read_error(&dependency_rel, &error)),
    }
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

/// Reconcile a runtime record stranded busy by a crash. No runtime process
/// outlives the engine, so at load time nothing can genuinely still be
/// starting/running/stopping: `stopping` settles to `stopped` (the shutdown
/// it was waiting for cannot outlive the process), and `starting`/`running`
/// become `failed` with a `last_error` explaining the reconciliation —
/// otherwise the record would claim a live runtime forever and e.g.
/// `delete_app` would refuse with `runtime_busy` with no path out. Returns
/// `true` when the record changed (the caller persists).
///
/// ⚠️ GATE (the twin of the warning on
/// `AppService::update_runtime_record`): this unconditional stranding policy
/// is CORRECT ONLY while no runtime process can outlive the engine. A future
/// live process manager MUST replace it with a liveness-aware
/// reconciliation, or loads will stamp genuinely running dev servers
/// `failed` and un-guard deletion.
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
/// and the index rewrite. Returns `true` when anything was repaired (the
/// caller persists).
///
/// Whole-batch tear: the `app.json` mirror is written LAST in
/// [`save_app_files`] and BEFORE the index, so a mirror/index divergence
/// proves the batch committed while the index write was lost. The mirror
/// record wins.
fn repair_torn_commit(app: &mut AppState, mirror: &AppRecord) -> bool {
    if *mirror != app.record {
        app.record = mirror.clone();
        return true;
    }
    false
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
    let mut merged_ids: BTreeSet<String> = merged.iter().map(|record| record.id.clone()).collect();
    for entry in disk {
        if !known_ids.contains(&entry.id) && merged_ids.insert(entry.id.clone()) {
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
    /// `apps/<id>/runtime.json` ([`save_runtime`]).
    Runtime,
    /// `workspace/.lingxi/app.json` record mirror — always LAST;
    /// [`load_all`]'s torn-commit repair depends on it superseding the index.
    MetadataMirror,
}

/// THE canonical per-app write order, defined exactly once.
/// [`save_app_files`] executes this sequence in full; partial writers (the
/// service's changed-docs mutation path) must pass subsequences of it to
/// [`save_app_files_steps`] — the repair contract in [`load_all`] depends on
/// the ORDER of the writes that happen, not on every document being
/// rewritten.
pub const APP_DOC_WRITE_ORDER: [AppDocWriteStep; 2] =
    [AppDocWriteStep::Runtime, AppDocWriteStep::MetadataMirror];

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
/// actually changed. Does NOT touch the index — callers write the index last
/// (the creation commit point; for mutations a lost index write is repaired
/// forward on load, see the module doc). On failure the error reports WHICH
/// prefix of `steps` already landed (see [`AppBatchWriteFailure`]).
pub fn save_app_files_steps(
    root: &Path,
    app: &AppState,
    steps: &[AppDocWriteStep],
) -> Result<(), AppBatchWriteFailure> {
    let id = &app.record.id;
    for (written, step) in steps.iter().enumerate() {
        let result = match step {
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
/// (runtime, then the metadata mirror — the mirror LAST).
pub fn save_app_files(root: &Path, app: &AppState) -> Result<(), AppError> {
    save_app_files_steps(root, app, &APP_DOC_WRITE_ORDER).map_err(AppBatchWriteFailure::into_error)
}

/// Atomically persist `apps/<id>/runtime.json`.
pub fn save_runtime(root: &Path, app_id: &str, runtime: &AppRuntimeRecord) -> Result<(), AppError> {
    write_doc(root, &runtime_rel(app_id), runtime)
}

/// Atomically persist `apps/<id>/dependencies.json`.
pub fn save_dependency_record(
    root: &Path,
    dependency: &AppDependencyRecord,
) -> Result<(), AppError> {
    write_doc(root, &dependency_rel(&dependency.app_id), dependency)
}

/// Remove `apps/<id>`: serialize against any in-flight build or background
/// task, then
/// rename-to-trash first (the commit point — see
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
    // Keep the lock through the rename commit point. Once the directory is in
    // `.trash`, no build can resolve it through the live app path anymore and
    // the lock can be released safely before the best-effort recursive remove.
    let _background_lock = lock_app_background_if_present(root, app_id)?;
    let _build_lock = lock_app_build_if_present(root, app_id)?;
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
    use crate::types::AppWorkflowState;

    fn new_app(id: &str) -> AppState {
        AppState::create(
            id.into(),
            format!("App {id}"),
            "a test app".into(),
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
        app.set_runtime(
            AppRuntimeState::Starting,
            Some(3010),
            Some(9),
            None,
            1_700_000_000_001,
        )
        .unwrap();
        app.set_runtime(
            AppRuntimeState::Failed,
            None,
            None,
            Some("boot failed".into()),
            1_700_000_000_002,
        )
        .unwrap();
        let apps = vec![app, new_app("bbbb2222")];
        save_full(dir.path(), &apps);

        let loaded = load_all(dir.path()).unwrap();
        assert_eq!(loaded, apps);
        // Files live where the spec says.
        assert!(dir.path().join("apps/index.json").is_file());
        assert!(dir.path().join("apps/aaaa1111/runtime.json").is_file());
        assert!(dir
            .path()
            .join("apps/aaaa1111/workspace/.lingxi/app.json")
            .is_file());
        // Persisted documents are pretty-printed with a trailing newline.
        let body = std::fs::read_to_string(dir.path().join("apps/index.json")).unwrap();
        assert!(body.starts_with("{\n"));
        assert!(body.ends_with("}\n"));
        assert!(body.contains("\"schemaVersion\": 1"));
        assert!(body.contains("\"workflowState\": \"draft\""));
    }

    /// The mirror-wins half of torn-commit repair: a crash after the per-app
    /// batch but before the index rewrite leaves the mirror AHEAD of the
    /// index — the mirror record supersedes the index record and the repair
    /// is persisted (both index and mirror agree on the next load).
    #[test]
    fn diverged_mirror_supersedes_the_index_and_the_repair_persists() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = new_app("cccc3333");
        save_full(dir.path(), std::slice::from_ref(&app));
        // The mutation (mark_ready) committed its batch (runtime + mirror)…
        app.record.workflow_state = AppWorkflowState::Ready;
        app.record.updated_at_ms = 1_700_000_000_500;
        save_app_files(dir.path(), &app).unwrap();
        // …but the index rewrite was lost to a crash: index still says draft.

        let loaded = load_all(dir.path()).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(
            loaded[0].record.workflow_state,
            AppWorkflowState::Ready,
            "the mirror (written after the index) wins"
        );
        assert_eq!(loaded[0].record.updated_at_ms, 1_700_000_000_500);
        // The repair was persisted: the index now agrees.
        let body = std::fs::read_to_string(dir.path().join("apps/index.json")).unwrap();
        assert!(body.contains("\"ready\""), "{body}");
        // A second load needs no repair and sees the same state.
        assert_eq!(load_all(dir.path()).unwrap(), loaded);
    }

    /// The on-disk legacy migration: every pipeline-era `workflowState`
    /// deserializes to `draft` via the serde aliases, and legacy pipeline
    /// documents sitting in the app dir are ignored (not read, not deleted).
    #[test]
    fn legacy_pipeline_states_load_as_draft_and_stale_docs_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let app = new_app("dddd4444");
        save_full(dir.path(), std::slice::from_ref(&app));
        // Rewrite index + mirror with a legacy mid-pipeline state.
        for rel in [
            "apps/index.json",
            "apps/dddd4444/workspace/.lingxi/app.json",
        ] {
            let path = dir.path().join(rel);
            let body = std::fs::read_to_string(&path).unwrap();
            std::fs::write(
                &path,
                body.replace("\"draft\"", "\"awaiting_preview_confirmation\""),
            )
            .unwrap();
        }
        // Plant stale legacy pipeline documents.
        let stale_interactions = dir.path().join("apps/dddd4444/interactions.json");
        std::fs::write(&stale_interactions, "{\"schemaVersion\":1,\"nextSeq\":1}").unwrap();
        let stale_spec = dir
            .path()
            .join("apps/dddd4444/workspace/.lingxi/design-spec.json");
        std::fs::write(&stale_spec, "{\"schemaVersion\":1,\"revision\":3}").unwrap();

        let loaded = load_all(dir.path()).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(
            loaded[0].record.workflow_state,
            AppWorkflowState::Draft,
            "a mid-pipeline legacy state can only mean 'not ready yet'"
        );
        // The stale documents were left alone.
        assert_eq!(
            std::fs::read_to_string(&stale_interactions).unwrap(),
            "{\"schemaVersion\":1,\"nextSeq\":1}"
        );
        assert_eq!(
            std::fs::read_to_string(&stale_spec).unwrap(),
            "{\"schemaVersion\":1,\"revision\":3}"
        );
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
    fn a_template_era_index_reports_a_readable_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("apps")).unwrap();
        std::fs::write(
            dir.path().join("apps/index.json"),
            br#"{"schemaVersion":1,"apps":[{"id":"old","name":"Old","template":"dashboard","createdAtMs":1,"updatedAtMs":1,"workflowState":"ready","workspaceRel":"apps/old/workspace"}]}"#,
        )
        .unwrap();

        let error = load_all(dir.path()).expect_err("a template-era index is not loadable");
        let message = format!("{error}");
        assert!(
            message.contains("不再支持") || message.contains("no longer supports"),
            "the error explains WHY rather than leaking a serde path: {message}"
        );
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
                "brief": "an evil app",
                "createdAtMs": 1,
                "updatedAtMs": 1,
                "workflowState": "draft",
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
                    "brief": "a sneaky app",
                    "createdAtMs": 1,
                    "updatedAtMs": 1,
                    "workflowState": "draft",
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

    /// Finding 6: the runtime record's embedded `app_id` must name the
    /// owning app; a mismatch is `storage_corrupt` like the mirror id.
    #[test]
    fn embedded_app_id_mismatches_are_storage_corrupt() {
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
            "apps/cccc3333/runtime.json",
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
            dir.path().join("apps/dddd4444/runtime.json.tmp-1-1"),
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
        assert!(
            !dir.path().join("apps/eeee5555/build.lock").exists(),
            "the per-app lock is a runtime artifact and must not survive deletion"
        );
        assert!(dir.path().join("apps/ffff6666/runtime.json").is_file());
        // Deleting a missing dir is a no-op.
        delete_app_dir(dir.path(), "eeee5555").unwrap();
    }

    #[test]
    fn app_build_lock_is_confined_to_an_existing_app_directory() {
        let dir = tempfile::tempdir().unwrap();
        let app = new_app("llll2222");
        save_full(dir.path(), std::slice::from_ref(&app));

        let lock = lock_app_build(dir.path(), "llll2222").unwrap();
        assert!(dir.path().join("apps/llll2222/build.lock").is_file());
        drop(lock);

        let err = match lock_app_build(dir.path(), "mmmm3333") {
            Ok(_) => panic!("missing app directory must not be created by locking"),
            Err(error) => error,
        };
        assert_eq!(err.code(), AppErrorCode::NotFound);
        assert!(!dir.path().join("apps/mmmm3333").exists());
    }

    #[test]
    fn app_background_lock_is_confined_to_an_existing_app_directory() {
        let dir = tempfile::tempdir().unwrap();
        let app = new_app("nnnn3333");
        save_full(dir.path(), std::slice::from_ref(&app));

        let lock = lock_app_background(dir.path(), "nnnn3333").unwrap();
        assert!(dir.path().join("apps/nnnn3333/background.lock").is_file());
        drop(lock);

        let err = match lock_app_background(dir.path(), "oooo4444") {
            Ok(_) => panic!("missing app directory must not be created by locking"),
            Err(error) => error,
        };
        assert_eq!(err.code(), AppErrorCode::NotFound);
        assert!(!dir.path().join("apps/oooo4444").exists());
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
        let dir = tempfile::tempdir().unwrap();
        let mut app = new_app("kkkk1111");
        save_full(dir.path(), std::slice::from_ref(&app));

        let over = usize::try_from(MAX_DOC_BYTES).unwrap() + 1;
        app.runtime.last_error = Some("x".repeat(over));
        let err = save_runtime(dir.path(), "kkkk1111", &app.runtime).unwrap_err();
        assert_eq!(err.code(), AppErrorCode::InvalidRequest);
        assert!(
            err.to_string().contains("durable size limit"),
            "unexpected message: {err}"
        );

        // Disk was never touched: the store still loads cleanly and holds the
        // committed (error-free) runtime record.
        let loaded = load_all(dir.path()).unwrap();
        assert_eq!(loaded.len(), 1);
        assert!(loaded[0].runtime.last_error.is_none());
    }

    #[test]
    fn write_order_is_the_canonical_two_step_sequence() {
        assert_eq!(
            APP_DOC_WRITE_ORDER,
            [AppDocWriteStep::Runtime, AppDocWriteStep::MetadataMirror],
            "the canonical order is part of the repair contract"
        );
        assert_eq!(
            write_order_prefix_through(AppDocWriteStep::Runtime),
            &APP_DOC_WRITE_ORDER[..1]
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

        // A distinct after-state in both documents.
        let mut after = before.clone();
        after
            .set_runtime(
                AppRuntimeState::Starting,
                Some(3999),
                Some(7),
                None,
                1_700_000_000_101,
            )
            .unwrap();
        after.record.workflow_state = AppWorkflowState::Ready;
        after.record.updated_at_ms = 1_700_000_000_101;

        // Fail at runtime.json (the FIRST step): the mirror (later) must not
        // be written.
        let runtime_path = dir.path().join(runtime_rel("mmmm3333"));
        std::fs::remove_file(&runtime_path).unwrap();
        std::os::unix::fs::symlink("/dev/null", &runtime_path).unwrap();
        save_app_files(dir.path(), &after).unwrap_err();
        let mirror_body =
            std::fs::read_to_string(dir.path().join(metadata_rel("mmmm3333"))).unwrap();
        assert!(
            !mirror_body.contains("\"ready\""),
            "the mirror must not be written before runtime"
        );
    }

    /// Finding 8: a runtime record stranded busy by a crash is reconciled at
    /// load (no runtime process outlives the engine) and the reconciliation
    /// is persisted.
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
        let target = dir.path().join("apps/pppp6666/runtime.json");
        std::fs::remove_file(&target).unwrap();
        std::os::unix::fs::symlink(victim.path().join("payload.json"), &target).unwrap();
        let err = load_all(dir.path()).unwrap_err();
        assert_eq!(err.code(), AppErrorCode::StorageCorrupt, "{err}");
        assert!(err.to_string().contains("regular file"), "{err}");
    }
}

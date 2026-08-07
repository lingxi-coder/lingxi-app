//! [`AppService`] — the single source of truth for local apps.
//!
//! Owns the in-memory aggregates, all persistence (rebuilding fully from disk
//! at construction), continuation delivery, and typed domain-event emission.
//! Every mutation follows clone → transition → persist → commit → emit, so a
//! failed persist never leaves memory ahead of disk, and observers never run
//! while the state lock is held. A separate emission-order lock spans every
//! commit → emit window (and snapshot emissions), so concurrent commands can
//! never deliver events out of commit order — a stale snapshot is never
//! delivered after a newer mutation's events.
//!
//! Event DELIVERY runs in spawned emission tasks, never in the caller's
//! future: a mutating call commits and hands its already-held emission-order
//! guard (plus the events) to a `tokio::spawn`ed task, so cancelling the
//! caller between commit and emission can no longer lose gate announcements,
//! and an observer that blocks cannot wedge the mutating caller. Ordering
//! still holds because the guard is ACQUIRED in the caller before the commit
//! (tokio's `Mutex` is FIFO-fair, so guards are granted in request order —
//! this fairness is load-bearing) and only RELEASED by the emission task
//! after delivery. Observers must not call back into emitting paths; a
//! task-local reentrancy guard turns that programming error into a loud
//! panic instead of a silent permanent deadlock. All blocking multi-fsync
//! storage I/O runs on the blocking pool (`spawn_blocking`) over owned
//! clones, never on the async executor threads.

use crate::checkpoints::AppCheckpointStore;
use crate::continuation::ContinuationSink;
use crate::error::AppError;
use crate::events::{AppEvent, AppEventObserver};
use crate::ids;
use crate::manifest::{
    save_manifest, validate_domain, validate_identifier, AppLayout, AppManifest,
};
use crate::permissions::{save_permissions, AppPermissions};
use crate::state::{self, AppState};
use crate::storage;
use crate::types::{
    AppCheckpoint, AppCheckpointKind, AppContinuation, AppDesignDraft, AppDesignPatch,
    AppDesignPatchOp, AppDesignSuggestion, AppGenerationProgress, AppInteractionKind,
    AppInteractionRequest, AppInteractions, AppRecord, AppRuntimeMode, AppRuntimeRecord,
    AppRuntimeState, DesignValue,
};
use std::collections::BTreeSet;
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
// sizes (phase 1's producer is the on-device UI; phases 2–3 hand the same
// seam to agent-generated input). Every violation fails typed
// `invalid_request` BEFORE anything is cloned, persisted, or re-emitted —
// index.json and the full `AppsChanged` record set are rewritten on every
// mutation, and `DesignDraftChanged` re-ships the whole field map, so an
// uncapped value would be amplified on every subsequent edit of any app.
// String caps are UTF-8 bytes (what memory/disk/the wire actually pay).

/// Maximum app name length in bytes (after trimming).
pub const MAX_NAME_BYTES: usize = 200;
/// Maximum app `brief` length in bytes (after trimming). Generous relative to
/// [`MAX_NAME_BYTES`] — the brief is prose the LLM reads for context in all
/// three stages, not a label.
pub const MAX_BRIEF_BYTES: usize = 4_000;
/// Maximum `conversation_id` length in bytes.
pub const MAX_CONVERSATION_ID_BYTES: usize = 128;
/// Maximum operations in one design patch.
pub const MAX_PATCH_OPS: usize = 64;
/// Maximum field id length in bytes.
pub const MAX_FIELD_ID_BYTES: usize = 200;
/// Maximum text value length in bytes (short/long text, single choice, color).
pub const MAX_TEXT_VALUE_BYTES: usize = 20_000;
/// Maximum items in one list value (multiple choice, screens, features).
pub const MAX_LIST_ITEMS: usize = 200;
/// Maximum list item length in bytes.
pub const MAX_LIST_ITEM_BYTES: usize = 500;
/// Maximum patch note length in bytes.
pub const MAX_NOTE_BYTES: usize = 2_000;
/// Maximum revision prompt length in bytes.
pub const MAX_PROMPT_BYTES: usize = 20_000;
/// Maximum fields a draft may accumulate across all patches.
pub const MAX_DRAFT_FIELDS: usize = 256;

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

fn validate_design_value(field_id: &str, value: &DesignValue) -> Result<(), AppError> {
    match value {
        DesignValue::ShortText(text)
        | DesignValue::LongText(text)
        | DesignValue::SingleChoice(text)
        | DesignValue::Color(text) => ensure_within(
            &format!("value of field {field_id:?}"),
            text.len(),
            MAX_TEXT_VALUE_BYTES,
        ),
        DesignValue::MultipleChoice(items)
        | DesignValue::ScreenList(items)
        | DesignValue::FeatureList(items) => {
            if items.len() > MAX_LIST_ITEMS {
                return Err(AppError::InvalidRequest(format!(
                    "value of field {field_id:?} has {} items (limit {MAX_LIST_ITEMS})",
                    items.len()
                )));
            }
            for item in items {
                ensure_within(
                    &format!("a list item of field {field_id:?}"),
                    item.len(),
                    MAX_LIST_ITEM_BYTES,
                )?;
            }
            Ok(())
        }
        // The domain list and the data-field ids below are the only draft
        // values the scaffold copies VERBATIM onto the manifest, where
        // `AppManifest::validate` rejects them while the app is already
        // `generating` — a state whose only exit replays the same input. The
        // manifest contract is therefore enforced here, at ingest, where a
        // rejection leaves the draft revision and the workflow state intact.
        DesignValue::DomainList(domains) => {
            if domains.len() > MAX_LIST_ITEMS {
                return Err(AppError::InvalidRequest(format!(
                    "value of field {field_id:?} has {} domains (limit {MAX_LIST_ITEMS})",
                    domains.len()
                )));
            }
            for domain in domains {
                // `validate_domain` caps at 253 bytes, stricter than
                // `MAX_LIST_ITEM_BYTES`, so no separate length check.
                validate_domain(domain)?;
            }
            Ok(())
        }
        DesignValue::DataFieldList(fields) => {
            if fields.len() > MAX_LIST_ITEMS {
                return Err(AppError::InvalidRequest(format!(
                    "value of field {field_id:?} has {} data fields (limit {MAX_LIST_ITEMS})",
                    fields.len()
                )));
            }
            for field in fields {
                // Shape (and the 64-byte cap) per the manifest contract. The
                // label / enum-option / duplicate-id rules are NOT mirrored:
                // the designer patches the whole list on every keystroke, so a
                // field that is still being typed would have its edit rejected
                // mid-flight.
                validate_identifier("data field", &field.id)?;
                ensure_within(
                    &format!("data field label of field {field_id:?}"),
                    field.label.len(),
                    MAX_TEXT_VALUE_BYTES,
                )?;
                for option in &field.enum_options {
                    ensure_within(
                        &format!("enum option of field {field_id:?}"),
                        option.len(),
                        MAX_LIST_ITEM_BYTES,
                    )?;
                }
            }
            Ok(())
        }
        DesignValue::Boolean(_) | DesignValue::Density(_) => Ok(()),
        // TODO(local-apps#questionnaire, Task 3): `Deferred` is the
        // questionnaire-answer sentinel added for the conversational
        // designer; the generic draft-patch pipeline does not understand it
        // yet — wiring it into `AppDesignDraft` is Task 3's job. Reject it
        // here so the invariant "`AppDesignDraft::fields` never holds a
        // `Deferred` value" holds for every WRITE path (`update_draft`,
        // `store_suggestion`, continuation replay). The matching LOAD-path
        // gate is `storage::ensure_no_deferred_design_values` (a hand-edited
        // or newer-build disk document is the other way one could appear);
        // together the two are what let `engine-mobile`'s wire-lowering match
        // treat its `Deferred` arm as unreachable instead of needing a wire
        // representation before Task 3 lands. When Task 3 relaxes this arm,
        // the storage.rs gate must relax in the same change, or a
        // legitimately-saved draft fails to reload on next start.
        DesignValue::Deferred => Err(AppError::InvalidRequest(format!(
            "value of field {field_id:?} cannot be a deferred answer in a draft patch"
        ))),
    }
}

/// Cap one inbound patch (op count, note, field ids, values) — shared by
/// `update_draft` and `store_suggestion` (an applied suggestion re-applies a
/// patch that already passed this gate).
fn validate_patch(patch: &AppDesignPatch) -> Result<(), AppError> {
    if patch.ops.len() > MAX_PATCH_OPS {
        return Err(AppError::InvalidRequest(format!(
            "patch has {} ops (limit {MAX_PATCH_OPS})",
            patch.ops.len()
        )));
    }
    if let Some(note) = &patch.note {
        ensure_within("patch note", note.len(), MAX_NOTE_BYTES)?;
    }
    for op in &patch.ops {
        match op {
            AppDesignPatchOp::Set { field_id, value } => {
                ensure_within(
                    &format!("field id {field_id:?}"),
                    field_id.len(),
                    MAX_FIELD_ID_BYTES,
                )?;
                validate_design_value(field_id, value)?;
            }
            AppDesignPatchOp::Remove { field_id } => {
                ensure_within(
                    &format!("field id {field_id:?}"),
                    field_id.len(),
                    MAX_FIELD_ID_BYTES,
                )?;
            }
        }
    }
    Ok(())
}

/// Post-apply guard shared by `update_draft` / `apply_suggestion`: individual
/// patches are capped, but fields accumulate across patches — an over-limit
/// draft is rejected BEFORE it is persisted (the working clone is discarded).
fn ensure_draft_field_count(draft: &AppDesignDraft) -> Result<(), AppError> {
    if draft.fields.len() <= MAX_DRAFT_FIELDS {
        Ok(())
    } else {
        Err(AppError::InvalidRequest(format!(
            "draft would have {} fields (limit {MAX_DRAFT_FIELDS})",
            draft.fields.len()
        )))
    }
}

/// Outcome of one `apply_suggestion` transition, making the commit/report
/// split explicit (instead of a nested `Ok(Err(..))` puzzle): BOTH variants
/// commit through `with_app`'s success path — `RejectedConsumed` persists
/// the suggestion's consumption while the caller still receives the typed
/// error.
enum SuggestionOutcome {
    /// The patch applied; carries the new draft revision.
    Applied(u64),
    /// The suggestion was consumed WITHOUT applying (over-cap poison, or a
    /// stale `based_on_revision`); carries the typed error the caller sees.
    RejectedConsumed(AppError),
}

/// Single source of truth for local apps (phase 1: data model, state
/// machines, storage, continuations — no runtime, no git).
pub struct AppService {
    root: PathBuf,
    clock: Arc<dyn Clock>,
    sink: Arc<dyn ContinuationSink>,
    observer: Arc<dyn AppEventObserver>,
    /// `Arc` so completion tasks can hold an [`OwnedMutexGuard`] across a
    /// caller-cancellation boundary (see the module doc).
    state: Arc<Mutex<Vec<AppState>>>,
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
}

impl AppService {
    /// Rebuild the service from disk alone. `root` is the per-profile data
    /// root (the engine derives it from its config; tests inject a tempdir);
    /// app state lives under `<root>/apps/`. The directory is created when
    /// missing; corrupt state fails with `storage_corrupt` instead of being
    /// silently reset. A store torn by a crash between the per-app batch and
    /// the index rewrite is repaired forward here (see the [`crate::storage`]
    /// module doc). Before returning, the pending human gate of every app is
    /// re-announced through the observer (see `announce_pending_gates`) so a
    /// persisted `interaction_id` whose original announcement was lost to a
    /// crash — or one freshly minted by a load-time re-arm — still reaches
    /// the client.
    pub async fn load(
        root: impl Into<PathBuf>,
        clock: Arc<dyn Clock>,
        sink: Arc<dyn ContinuationSink>,
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
        let service = Self {
            root,
            clock,
            sink,
            observer,
            state: Arc::new(Mutex::new(apps)),
            emit_order: Arc::new(Mutex::new(())),
            retired_ids: Arc::new(std::sync::Mutex::new(BTreeSet::new())),
        };
        service.announce_pending_gates().await;
        Ok(service)
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

    /// Announce the pending human gate of EVERY app (as `DesignerRequested` /
    /// `PreviewReady`) right after a load. A crash between persisting a gate
    /// and emitting its announcement — or a load-time gate re-arm — would
    /// otherwise leave a pending `interaction_id` no event ever carried,
    /// wedging the client protocol surface (the id gates `confirm_*`).
    /// Emitted unconditionally, not just for re-armed gates: clients treat
    /// gate announcements idempotently, so re-announcing an id they already
    /// know is a no-op.
    /// Re-announce pending gates once observers are attached.
    ///
    /// [`Self::load`] announces too, but it runs BEFORE the host subscribes its
    /// client and domain observers, so those events reach a fanout with no
    /// subscribers and are dropped. After a relaunch with an armed designer
    /// gate that left the client without the pending `interaction_id`, which
    /// gates `confirm_design` — and the UI, seeing no id, would try
    /// `open_designer`, which is illegal from `awaiting_spec_confirmation`.
    /// The app was then unreachable from either side.
    ///
    /// Safe to call repeatedly: announcements are idempotent by design.
    pub async fn resync_pending_gates(&self) {
        self.announce_pending_gates().await;
    }

    async fn announce_pending_gates(&self) {
        let order = self.acquire_emit_order().await;
        let events: Vec<AppEvent> = {
            let apps = self.state.lock().await;
            apps.iter()
                .filter_map(|app| {
                    app.interactions
                        .pending
                        .as_ref()
                        .map(Self::gate_announcement)
                })
                .collect()
        };
        Self::spawn_emission(Arc::clone(&self.observer), order, events);
    }

    /// THE pending-gate announcement shape, defined exactly once: `Designer`
    /// -> `DesignerRequested`, `Preview` -> `PreviewReady` (`url` stays
    /// `None` until phase 4). Shared by the live gate-opening paths
    /// ([`Self::open_designer`], [`Self::validation_passed`]) and the
    /// load-time re-announcement, so the three surfaces can never drift —
    /// when phase 4 adds a real preview url, one edit updates them all.
    fn gate_announcement(pending: &AppInteractionRequest) -> AppEvent {
        match pending.kind {
            AppInteractionKind::Designer => AppEvent::DesignerRequested {
                app_id: pending.app_id.clone(),
                interaction_id: pending.interaction_id.clone(),
                revision: pending.revision,
            },
            AppInteractionKind::Preview => AppEvent::PreviewReady {
                app_id: pending.app_id.clone(),
                interaction_id: pending.interaction_id.clone(),
                revision: pending.revision,
                url: None,
            },
        }
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
    /// events (e.g. `DesignConflict`) still emit. Persist + commit + emission
    /// hand-off run in a spawned completion task, so dropping the caller's
    /// future mid-call either aborts BEFORE any side effect or changes
    /// nothing about the mutation completing, committing, and emitting.
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
    ///   still-new earlier documents are exactly the evidence shapes the
    ///   repair table already handles. A FORWARD rollback dying mid-way
    ///   would instead leave old-prefix/new-suffix — the mirror still ahead
    ///   over already-reverted documents — a hybrid repair has no arm for (a
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
    /// DELIBERATE tradeoffs, not oversights:
    /// - The full-aggregate clone in `with_app` is LOAD-BEARING (the
    ///   compensating rollback needs the pristine original), and this
    ///   structural equality is bounded by the service input caps; dirty
    ///   flags or copy-on-write would buy speed at the cost of a second
    ///   source of truth for "what changed" inside the crash-repair
    ///   contract.
    /// - Draft edits bump `record.updated_at_ms`, so every edit rewrites the
    ///   mirror AND the index. Skipping the index would make mirror/index
    ///   divergence the STEADY state, turning every load into a spurious
    ///   torn-commit repair (and its rewrite) — costlier than the write it
    ///   saves.
    fn changed_doc_steps(
        committed: &AppState,
        working: &AppState,
    ) -> Vec<storage::AppDocWriteStep> {
        storage::APP_DOC_WRITE_ORDER
            .iter()
            .copied()
            .filter(|step| match step {
                storage::AppDocWriteStep::DesignSpec => committed.draft != working.draft,
                storage::AppDocWriteStep::Interactions => {
                    committed.interactions != working.interactions
                }
                storage::AppDocWriteStep::Runtime => committed.runtime != working.runtime,
                storage::AppDocWriteStep::MetadataMirror => committed.record != working.record,
            })
            .collect()
    }

    fn conflict_events(app_id: &str, error: &AppError) -> Vec<AppEvent> {
        if let AppError::RevisionConflict { expected, actual } = error {
            vec![AppEvent::DesignConflict {
                app_id: app_id.to_string(),
                expected_revision: *expected,
                actual_revision: *actual,
            }]
        } else {
            Vec::new()
        }
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

    /// The design draft of one app.
    pub async fn draft(&self, app_id: &str) -> Result<AppDesignDraft, AppError> {
        let apps = self.state.lock().await;
        let idx = Self::position(&apps, app_id)?;
        Ok(apps[idx].draft.clone())
    }

    /// The pending human gate of one app, if any.
    pub async fn pending_interaction(
        &self,
        app_id: &str,
    ) -> Result<Option<AppInteractionRequest>, AppError> {
        let apps = self.state.lock().await;
        let idx = Self::position(&apps, app_id)?;
        Ok(apps[idx].interactions.pending.clone())
    }

    /// The interaction + continuation store of one app.
    pub async fn interactions(&self, app_id: &str) -> Result<AppInteractions, AppError> {
        let apps = self.state.lock().await;
        let idx = Self::position(&apps, app_id)?;
        Ok(apps[idx].interactions.clone())
    }

    /// The runtime record of one app.
    pub async fn runtime_record(&self, app_id: &str) -> Result<AppRuntimeRecord, AppError> {
        let apps = self.state.lock().await;
        let idx = Self::position(&apps, app_id)?;
        Ok(apps[idx].runtime.clone())
    }

    /// Git-backed checkpoints of one app, newest first.
    pub async fn list_checkpoints(&self, app_id: &str) -> Result<Vec<AppCheckpoint>, AppError> {
        {
            let apps = self.state.lock().await;
            Self::position(&apps, app_id)?;
        }
        let layout = AppLayout::new(self.root.clone(), app_id.to_string())?;
        Self::run_blocking(move || AppCheckpointStore::new(&layout).list()).await
    }

    /// Commit the current workspace as a retained Git checkpoint and emit its
    /// domain event after the durable reference has been written.
    pub async fn create_checkpoint(
        &self,
        app_id: &str,
        kind: AppCheckpointKind,
        label: &str,
    ) -> Result<AppCheckpoint, AppError> {
        {
            let apps = self.state.lock().await;
            Self::position(&apps, app_id)?;
        }
        let order = self.acquire_emit_order().await;
        let layout = AppLayout::new(self.root.clone(), app_id.to_string())?;
        let label = label.to_string();
        let now = self.now_ms();
        let checkpoint =
            Self::run_blocking(move || AppCheckpointStore::new(&layout).create(kind, &label, now))
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
        {
            let apps = self.state.lock().await;
            Self::position(&apps, app_id)?;
        }
        let order = self.acquire_emit_order().await;
        let layout = AppLayout::new(self.root.clone(), app_id.to_string())?;
        let checkpoint_id = checkpoint_id.to_string();
        let now = self.now_ms();
        let safety = Self::run_blocking(move || {
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

    /// Create a new app record (workflow starts in `collecting_spec`) and its
    /// on-disk layout, then announce the new list via `AppsChanged`.
    pub async fn create_app(
        &self,
        name: &str,
        brief: &str,
        conversation_id: Option<String>,
    ) -> Result<AppRecord, AppError> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err(AppError::InvalidRequest(
                "app name must not be empty".into(),
            ));
        }
        ensure_within("app name", trimmed.len(), MAX_NAME_BYTES)?;
        let trimmed_brief = brief.trim();
        if trimmed_brief.is_empty() {
            return Err(AppError::InvalidRequest(
                "app brief must not be empty".into(),
            ));
        }
        ensure_within("app brief", trimmed_brief.len(), MAX_BRIEF_BYTES)?;
        if let Some(conversation_id) = &conversation_id {
            ensure_within(
                "conversation id",
                conversation_id.len(),
                MAX_CONVERSATION_ID_BYTES,
            )?;
        }
        let name = trimmed.to_string();
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
        // Completion task (see with_app): mint + persist + commit + emission
        // hand-off survive the caller's future being dropped.
        let completion = tokio::spawn(async move {
            let persisted = Self::run_blocking(move || {
                let id = Self::mint_app_id(&root, &existing_ids)?;
                let app = AppState::create(id, name, brief, conversation_id, now);
                // Per-app files first; the index entry is the commit point.
                storage::save_app_files(&root, &app)?;
                let layout = AppLayout::new(root.clone(), app.record.id.clone())?;
                layout.initialize()?;
                let manifest =
                    AppManifest::for_new_app(app.record.id.clone(), app.record.name.clone());
                save_manifest(&layout, &manifest)?;
                save_permissions(&layout, &AppPermissions::default())?;
                let mut records = existing_records;
                records.push(app.record.clone());
                storage::save_index_preserving(&root, &records, &known)?;
                Ok((app, records))
            })
            .await;
            match persisted {
                Ok((app, records)) => {
                    let record = app.record.clone();
                    apps.push(app);
                    drop(apps);
                    Self::spawn_emission(
                        observer,
                        order,
                        vec![AppEvent::AppsChanged { apps: records }],
                    );
                    Ok(record)
                }
                Err(error) => Err(error),
            }
        });
        completion
            .await
            .map_err(|error| AppError::Io(format!("create completion task failed: {error}")))?
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

    /// Open the designer gate (`collecting_spec | generation_failed ->
    /// awaiting_spec_confirmation`). The returned interaction carries the id
    /// the UI must echo back to [`Self::confirm_design`]. Gate-opening event
    /// pairs are state-change-first everywhere: `WorkflowChanged`, then the
    /// gate announcement (here `DesignerRequested`; `PreviewReady` for
    /// [`Self::validation_passed`]).
    pub async fn open_designer(&self, app_id: &str) -> Result<AppInteractionRequest, AppError> {
        let interaction_id = ids::generate_interaction_id();
        self.with_app(app_id, move |app, now| {
            match app.open_designer(interaction_id, now) {
                Ok(interaction) => {
                    let events = vec![
                        AppEvent::WorkflowChanged {
                            app_id: interaction.app_id.clone(),
                            state: app.record.workflow_state,
                            detail: None,
                        },
                        Self::gate_announcement(&interaction),
                    ];
                    (Ok(interaction), events)
                }
                Err(error) => (Err(error), Vec::new()),
            }
        })
        .await
    }

    /// Apply a user edit at `expected_revision`. A stale revision fails with
    /// `revision_conflict` AND emits `DesignConflict`; the user value is
    /// never silently overwritten.
    pub async fn update_draft(
        &self,
        app_id: &str,
        expected_revision: u64,
        patch: &AppDesignPatch,
    ) -> Result<u64, AppError> {
        validate_patch(patch)?;
        self.with_app(app_id, |app, now| {
            match app.update_draft(expected_revision, patch, now) {
                Ok(revision) => {
                    if let Err(error) = ensure_draft_field_count(&app.draft) {
                        return (Err(error), Vec::new());
                    }
                    (
                        Ok(revision),
                        vec![AppEvent::DesignDraftChanged {
                            app_id: app.record.id.clone(),
                            revision,
                            fields: app.draft.fields.clone(),
                        }],
                    )
                }
                Err(error) => {
                    let events = Self::conflict_events(&app.record.id, &error);
                    (Err(error), events)
                }
            }
        })
        .await
    }

    /// Store an agent suggestion (does not touch fields or revision) and
    /// announce it via `DesignSuggestionAvailable`.
    pub async fn store_suggestion(
        &self,
        app_id: &str,
        patch: AppDesignPatch,
    ) -> Result<AppDesignSuggestion, AppError> {
        validate_patch(&patch)?;
        let suggestion_id = ids::generate_suggestion_id();
        self.with_app(app_id, move |app, now| {
            match app.store_suggestion(suggestion_id, patch, now) {
                Ok(suggestion) => {
                    let event = AppEvent::DesignSuggestionAvailable {
                        app_id: app.record.id.clone(),
                        suggestion_id: suggestion.suggestion_id.clone(),
                        based_on_revision: suggestion.based_on_revision,
                        patch: suggestion.patch.clone(),
                    };
                    (Ok(suggestion), vec![event])
                }
                Err(error) => (Err(error), Vec::new()),
            }
        })
        .await
    }

    /// Dismiss the pending suggestion without mutating the draft revision.
    pub async fn dismiss_suggestion(
        &self,
        app_id: &str,
        suggestion_id: &str,
    ) -> Result<(), AppError> {
        self.with_app(app_id, |app, now| {
            match app.dismiss_suggestion(suggestion_id, now) {
                Ok(()) => (
                    Ok(()),
                    vec![AppEvent::DesignDraftChanged {
                        app_id: app.record.id.clone(),
                        revision: app.draft.revision,
                        fields: app.draft.fields.clone(),
                    }],
                ),
                Err(error) => (Err(error), Vec::new()),
            }
        })
        .await
    }

    /// Apply the stored suggestion at `expected_revision`. Same conflict
    /// semantics as [`Self::update_draft`]; a wrong `suggestion_id` fails
    /// with `interaction_invalid`.
    ///
    /// `based_on_revision` is load-bearing (finding 8): a suggestion whose
    /// `based_on_revision` is not the CURRENT draft revision is refused with
    /// typed `revision_conflict { expected: based_on, actual: current }`,
    /// CONSUMED (the clear is persisted — revisions only grow, so the stale
    /// suggestion could never become applicable), and announced via
    /// `DesignConflict`; the user's newer edits are never overwritten.
    ///
    /// The STORED patch is re-validated against the byte caps before it is
    /// applied: a suggestion planted through a hand-edited store never went
    /// through [`Self::store_suggestion`]'s gate and must not bypass the caps
    /// here. An over-cap suggestion fails typed `invalid_request` AND is
    /// consumed (the clear is persisted) — a poisoned suggestion left pending
    /// would wedge the draft behind the same error forever. The cap check
    /// runs as soon as the addressed suggestion is the pending one,
    /// deliberately ahead of the state/revision gates: poison is disposed of
    /// at first touch.
    pub async fn apply_suggestion(
        &self,
        app_id: &str,
        suggestion_id: &str,
        expected_revision: u64,
    ) -> Result<u64, AppError> {
        let outcome = self
            .with_app(app_id, |app, now| {
                if let Some(pending) = &app.draft.pending_suggestion {
                    if pending.suggestion_id == suggestion_id {
                        if let Err(error) = validate_patch(&pending.patch) {
                            app.draft.pending_suggestion = None;
                            return (Ok(SuggestionOutcome::RejectedConsumed(error)), Vec::new());
                        }
                    }
                }
                let had_pending = app.draft.pending_suggestion.is_some();
                match app.apply_suggestion(suggestion_id, expected_revision, now) {
                    Ok(revision) => {
                        if let Err(error) = ensure_draft_field_count(&app.draft) {
                            return (Err(error), Vec::new());
                        }
                        (
                            Ok(SuggestionOutcome::Applied(revision)),
                            vec![AppEvent::DesignDraftChanged {
                                app_id: app.record.id.clone(),
                                revision,
                                fields: app.draft.fields.clone(),
                            }],
                        )
                    }
                    Err(error) => {
                        let events = Self::conflict_events(&app.record.id, &error);
                        // The stale-`based_on_revision` arm is the only Err
                        // that CONSUMES the suggestion (finding 8); the
                        // consumption must commit while the caller still
                        // sees the typed conflict.
                        if had_pending && app.draft.pending_suggestion.is_none() {
                            return (Ok(SuggestionOutcome::RejectedConsumed(error)), events);
                        }
                        (Err(error), events)
                    }
                }
            })
            .await?;
        match outcome {
            SuggestionOutcome::Applied(revision) => Ok(revision),
            SuggestionOutcome::RejectedConsumed(error) => Err(error),
        }
    }

    /// Confirm the design spec (`awaiting_spec_confirmation -> generating`).
    /// Requires the exact pending designer `interaction_id` and the CURRENT
    /// draft revision; enqueues and (best-effort) delivers a
    /// `design_confirmed` continuation.
    pub async fn confirm_design(
        &self,
        app_id: &str,
        interaction_id: &str,
        revision: u64,
    ) -> Result<(), AppError> {
        self.workflow_step(app_id, None, |app, now| {
            app.confirm_design(interaction_id, revision, now)
                .map(|_continuation| ())
        })
        .await?;
        self.drain_after_gate(app_id).await;
        Ok(())
    }

    /// Cancel out of the design gate (`awaiting_spec_confirmation ->
    /// collecting_spec`); enqueues a `design_cancelled` continuation.
    pub async fn cancel_design(&self, app_id: &str) -> Result<(), AppError> {
        self.workflow_step(app_id, None, |app, now| {
            app.cancel_design(now).map(|_continuation| ())
        })
        .await?;
        self.drain_after_gate(app_id).await;
        Ok(())
    }

    /// `generating -> validating` (driven by the phase-3 generator; exposed
    /// for tests now).
    pub async fn generation_complete(&self, app_id: &str) -> Result<(), AppError> {
        self.workflow_step(app_id, None, AppState::generation_complete)
            .await
    }

    /// `generating -> generation_failed`, with an optional failure detail
    /// surfaced on the `WorkflowChanged` event.
    pub async fn generation_failed(
        &self,
        app_id: &str,
        detail: Option<String>,
    ) -> Result<(), AppError> {
        self.workflow_step(app_id, detail, AppState::generation_failed)
            .await
    }

    /// `generation_failed -> generating`; refused unless the confirmed
    /// revision is still current.
    pub async fn retry_generation(&self, app_id: &str) -> Result<(), AppError> {
        self.workflow_step(app_id, None, AppState::retry_generation)
            .await
    }

    /// `validating -> awaiting_preview_confirmation`; opens the preview gate
    /// and announces it via `PreviewReady` (`url` is `None` until phase 4).
    pub async fn validation_passed(&self, app_id: &str) -> Result<AppInteractionRequest, AppError> {
        let interaction_id = ids::generate_interaction_id();
        self.with_app(app_id, move |app, now| {
            match app.validation_passed(interaction_id, now) {
                Ok(interaction) => {
                    let events = vec![
                        AppEvent::WorkflowChanged {
                            app_id: app.record.id.clone(),
                            state: app.record.workflow_state,
                            detail: None,
                        },
                        Self::gate_announcement(&interaction),
                    ];
                    (Ok(interaction), events)
                }
                Err(error) => (Err(error), Vec::new()),
            }
        })
        .await
    }

    /// `validating -> validation_failed`, with an optional failure detail.
    pub async fn validation_failed(
        &self,
        app_id: &str,
        detail: Option<String>,
    ) -> Result<(), AppError> {
        self.workflow_step(app_id, detail, AppState::validation_failed)
            .await
    }

    /// `validation_failed -> revising`.
    pub async fn begin_revision(&self, app_id: &str) -> Result<(), AppError> {
        self.workflow_step(app_id, None, AppState::begin_revision)
            .await
    }

    /// Confirm the preview (`awaiting_preview_confirmation -> ready`).
    /// Requires the exact pending preview `interaction_id` and the CURRENT
    /// draft revision; enqueues a `preview_confirmed` continuation.
    pub async fn confirm_preview(
        &self,
        app_id: &str,
        interaction_id: &str,
        revision: u64,
    ) -> Result<(), AppError> {
        self.workflow_step(app_id, None, |app, now| {
            app.confirm_preview(interaction_id, revision, now)
                .map(|_continuation| ())
        })
        .await?;
        self.drain_after_gate(app_id).await;
        Ok(())
    }

    /// Ask for a revision from the preview gate or a ready app
    /// (`-> revising`); enqueues a `revision_requested` continuation carrying
    /// the prompt.
    pub async fn request_revision(&self, app_id: &str, prompt: &str) -> Result<(), AppError> {
        ensure_within("revision prompt", prompt.len(), MAX_PROMPT_BYTES)?;
        self.workflow_step(app_id, None, |app, now| {
            app.request_revision(prompt, now).map(|_continuation| ())
        })
        .await?;
        self.drain_after_gate(app_id).await;
        Ok(())
    }

    /// `revising -> validating`.
    pub async fn revision_ready(&self, app_id: &str) -> Result<(), AppError> {
        self.workflow_step(app_id, None, AppState::revision_ready)
            .await
    }

    /// `ready -> revising` for an automatic post-restore validation/build.
    /// Unlike [`Self::request_revision`], this does not enqueue a source-
    /// generation continuation; the coordinator queues a restore build.
    pub async fn begin_restore_rebuild(&self, app_id: &str) -> Result<(), AppError> {
        self.workflow_step(app_id, None, AppState::begin_restore_rebuild)
            .await
    }

    /// THE workflow-transition scaffold: run `step`, emit one
    /// `WorkflowChanged { detail }` on success, nothing on failure. Every
    /// plain transition AND every gate confirmation/cancellation routes
    /// through here (gate methods discard their continuation and follow up
    /// with `drain_after_gate`), so the event shape can never drift between
    /// them.
    async fn workflow_step(
        &self,
        app_id: &str,
        detail: Option<String>,
        step: impl FnOnce(&mut AppState, u64) -> Result<(), AppError>,
    ) -> Result<(), AppError> {
        self.with_app(app_id, move |app, now| match step(app, now) {
            Ok(()) => (
                Ok(()),
                vec![AppEvent::WorkflowChanged {
                    app_id: app.record.id.clone(),
                    state: app.record.workflow_state,
                    detail,
                }],
            ),
            Err(error) => (Err(error), Vec::new()),
        })
        .await
    }

    /// Update the runtime record (spec §C transition table; the phase-4
    /// runtime drives this). Emits `RuntimeChanged` and persists
    /// `runtime.json`. A port, once assigned, is never reassigned. An
    /// oversized `last_error` is TRUNCATED, not rejected (see
    /// [`truncate_last_error`]).
    ///
    /// ⚠️ PHASE-4 GATE: before wiring a live process manager onto this
    /// method, replace [`crate::storage`]'s unconditional load-time
    /// reconciliation (`reconcile_runtime_at_load` — it assumes NO runtime
    /// survives a restart and stamps every busy record failed/stopped).
    /// With dev servers that outlive the engine process it would mis-fail
    /// live runtimes, unpin their ports, and let `delete_app` pull a
    /// workspace out from under a running server.
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

    /// Emit a `GenerationProgress` event for an existing app (phase-3
    /// generator seam; nothing is persisted). `stage`/`detail` are capped
    /// like every sibling text input (rejection loses nothing here — nothing
    /// is persisted) and `percent` must be within 0–100.
    pub async fn report_generation_progress(
        &self,
        progress: AppGenerationProgress,
    ) -> Result<(), AppError> {
        ensure_within(
            "generation progress stage",
            progress.stage.len(),
            MAX_TEXT_VALUE_BYTES,
        )?;
        if let Some(detail) = &progress.detail {
            ensure_within(
                "generation progress detail",
                detail.len(),
                MAX_TEXT_VALUE_BYTES,
            )?;
        }
        if let Some(percent) = progress.percent {
            if percent > 100 {
                return Err(AppError::InvalidRequest(format!(
                    "generation progress percent is {percent} (limit 100)"
                )));
            }
        }
        let order = self.acquire_emit_order().await;
        {
            let apps = self.state.lock().await;
            Self::position(&apps, &progress.app_id)?;
        }
        Self::spawn_emission(
            Arc::clone(&self.observer),
            order,
            vec![AppEvent::GenerationProgress(progress)],
        );
        Ok(())
    }

    /// Deliver queued continuations for one app in seq order.
    ///
    /// Continuations with `seq <= last_delivered_seq` (already delivered
    /// before a crash) are dropped WITHOUT redelivery — the store-side seq
    /// dedup. Each successful delivery advances `last_delivered_seq` and is
    /// persisted before the next attempt, so a crash between deliveries
    /// redelivers at most one continuation (at-least-once). A sink failure
    /// stops the drain, keeps the remainder queued, and surfaces the error.
    /// Returns how many continuations were delivered.
    ///
    /// An in-flight `deliver` holds no locks, so it can RACE `delete_app`
    /// and land after the deletion was announced — the sink contract
    /// requires consumers to treat continuations for unknown/deleted apps
    /// as a no-op (see [`ContinuationSink`]).
    ///
    /// EVERY persist in this loop — the pre-delivery one AND the
    /// post-delivery one — first re-reads the on-disk `interactions.json`
    /// and MERGES it (union by seq) with memory: a continuation that reached
    /// disk but was rolled back out of memory by a failed index write (see
    /// [`Self::with_app`]'s residual window), or that landed on disk while a
    /// delivery was in flight, must be delivered, never clobbered by
    /// persisting a memory-derived view. After every merge, a gate-vs-
    /// evidence contradiction (disk proves memory's armed gate was consumed)
    /// is resolved by the SAME rule table load-time repair uses (see
    /// [`Self::merge_resolve_persist`]).
    pub async fn redeliver_undelivered(&self, app_id: &str) -> Result<usize, AppError> {
        let mut delivered = 0usize;
        loop {
            let next = self.merge_step(app_id, None).await?;
            let Some(continuation) = next else {
                return Ok(delivered);
            };
            self.sink.deliver(app_id, &continuation).await?;
            self.merge_step(app_id, Some(continuation.seq)).await?;
            delivered += 1;
        }
    }

    /// One locked merge-resolve-persist round of the redelivery loop
    /// (`delivered_seq: None` = pre-delivery, `Some(seq)` = post-delivery
    /// bookkeeping for a just-delivered seq). Returns the next continuation
    /// to deliver, if any. Blocking disk I/O runs on the blocking pool over
    /// an owned clone; the merged aggregate is committed back to memory
    /// under the still-held state lock.
    async fn merge_step(
        &self,
        app_id: &str,
        delivered_seq: Option<u64>,
    ) -> Result<Option<AppContinuation>, AppError> {
        let mut apps = self.state.lock().await;
        let idx = Self::position(&apps, app_id)?;
        let root = self.root.clone();
        let working = apps[idx].clone();
        let (merged, next) =
            Self::run_blocking(move || Self::merge_resolve_persist(&root, working, delivered_seq))
                .await?;
        apps[idx] = merged;
        Ok(next)
    }

    /// Merge one app's on-disk `interactions.json` into `app` (union by
    /// seq), fold in an optional just-delivered seq, resolve gate-vs-
    /// evidence contradictions, persist what changed, and return the updated
    /// aggregate plus the next continuation to deliver.
    ///
    /// Contradiction rule (finding 3): `pending` normally follows MEMORY —
    /// the in-process service is authoritative for the gate protocol. But
    /// when disk's `pending` is `None` AND disk has minted seqs memory never
    /// saw (`disk.next_seq > memory.next_seq`), the disk document was
    /// written by a gate-consuming transition that was rolled back out of
    /// memory: the SAME atomic write that minted those seqs also cleared the
    /// gate, so memory's still-armed gate is provably consumed. The merged
    /// aggregate then adopts `pending = None` and runs
    /// [`storage::resolve_gate_evidence`] — the exact rule table load-time
    /// repair uses — so the workflow state follows the evidence
    /// (e.g. `design_confirmed` → `generating`) instead of persisting an
    /// armed-gate + consumption-evidence hybrid that would invite a second,
    /// duplicate confirmation. The changed documents (interactions, and the
    /// record mirror when resolution moved the state) are persisted in
    /// canonical order; the index copy of a resolved record intentionally
    /// waits for the next index write or load-time mirror-wins repair, like
    /// every mirror-ahead window.
    fn merge_resolve_persist(
        root: &std::path::Path,
        mut app: AppState,
        delivered_seq: Option<u64>,
    ) -> Result<(AppState, Option<AppContinuation>), AppError> {
        let app_id = app.record.id.clone();
        let disk = storage::load_interactions(root, &app_id)?;
        let memory_gate_armed = app.interactions.pending.is_some();
        let memory_next_seq = app.interactions.next_seq;
        let record_before = app.record.clone();
        let mut merged = Self::merge_interactions(&app.interactions, &disk);
        if let Some(seq) = delivered_seq {
            if merged.last_delivered_seq < seq {
                merged.last_delivered_seq = seq;
            }
            let last = merged.last_delivered_seq;
            merged.undelivered.retain(|c| c.seq > last);
        }
        state::enforce_undelivered_cap(&app_id, &mut merged);
        app.interactions = merged;
        let disk_proves_gate_consumed =
            memory_gate_armed && disk.pending.is_none() && disk.next_seq > memory_next_seq;
        if disk_proves_gate_consumed {
            app.interactions.pending = None;
            storage::resolve_gate_evidence(&mut app);
        }
        let mut steps = Vec::new();
        if app.interactions != disk {
            steps.push(storage::AppDocWriteStep::Interactions);
        }
        if app.record != record_before {
            steps.push(storage::AppDocWriteStep::MetadataMirror);
        }
        storage::save_app_files_steps(root, &app, &steps)
            .map_err(storage::AppBatchWriteFailure::into_error)?;
        let next = app.interactions.undelivered.first().cloned();
        Ok((app, next))
    }

    /// Union `memory` and `disk` interaction stores by continuation seq: the
    /// queue keeps every seq present on either side (sorted, deduped), both
    /// counters advance to their max, and already-delivered seqs are pruned.
    /// `pending` follows MEMORY — the in-process service is authoritative for
    /// the gate protocol — except when disk PROVES the armed gate was
    /// consumed, which [`Self::merge_resolve_persist`] resolves after the
    /// merge; the merge itself exists so disk-only SEQS survive (see
    /// [`Self::redeliver_undelivered`]).
    fn merge_interactions(memory: &AppInteractions, disk: &AppInteractions) -> AppInteractions {
        let mut merged = memory.clone();
        merged.next_seq = memory.next_seq.max(disk.next_seq);
        merged.last_delivered_seq = memory.last_delivered_seq.max(disk.last_delivered_seq);
        for continuation in &disk.undelivered {
            if !merged.undelivered.iter().any(|c| c.seq == continuation.seq) {
                merged.undelivered.push(continuation.clone());
            }
        }
        merged.undelivered.sort_by_key(|c| c.seq);
        let last = merged.last_delivered_seq;
        merged.undelivered.retain(|c| c.seq > last);
        merged
    }

    /// Deliver queued continuations for EVERY app (engine startup sweep).
    /// Apps deleted mid-sweep are skipped.
    ///
    /// One app's failure must never starve the others: a per-app error is
    /// logged and the sweep CONTINUES through the remaining apps (their
    /// queued continuations still deliver); the failed app's remainder stays
    /// queued for the next sweep. After every app has been visited, the first
    /// error (if any) is returned, else the total delivered count.
    pub async fn redeliver_all_undelivered(&self) -> Result<usize, AppError> {
        let app_ids: Vec<String> = {
            let apps = self.state.lock().await;
            apps.iter().map(|app| app.record.id.clone()).collect()
        };
        let mut delivered = 0usize;
        let mut first_error: Option<AppError> = None;
        for app_id in app_ids {
            match self.redeliver_undelivered(&app_id).await {
                Ok(count) => delivered += count,
                Err(AppError::NotFound(_)) => {}
                Err(error) => {
                    tracing::warn!(
                        app_id,
                        error = %error,
                        "continuation redelivery failed for one app; sweep continues"
                    );
                    first_error.get_or_insert(error);
                }
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(delivered),
        }
    }

    /// Best-effort drain right after a gate enqueued a continuation; failures
    /// stay queued for [`Self::redeliver_undelivered`] (at-least-once).
    async fn drain_after_gate(&self, app_id: &str) {
        if let Err(error) = self.redeliver_undelivered(app_id).await {
            tracing::debug!(
                app_id,
                error = %error,
                "continuation delivery deferred; queued for redelivery"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::continuation::{NoopContinuationSink, RecordingContinuationSink};
    use crate::error::AppErrorCode;
    use crate::events::RecordingAppEventObserver;
    use crate::manifest::{DataFieldKind, DataFieldSchema};
    use crate::test_support::FixedClock;
    use crate::types::{
        AppContinuation, AppContinuationKind, AppDesignPatchOp, AppWorkflowState, DesignValue,
    };
    use std::path::Path;

    struct Harness {
        service: AppService,
        sink: Arc<RecordingContinuationSink>,
        observer: Arc<RecordingAppEventObserver>,
    }

    impl Harness {
        /// Flush the async emission queue, then drain the observed events.
        /// Event delivery runs in spawned emission tasks (finding 11), so
        /// every take must wait for completed deliveries first.
        async fn take_events(&self) -> Vec<AppEvent> {
            self.service.flush_events().await;
            self.observer.take()
        }
    }

    async fn harness(root: &Path) -> Harness {
        let sink = Arc::new(RecordingContinuationSink::new());
        let observer = Arc::new(RecordingAppEventObserver::new());
        let service = AppService::load(
            root,
            Arc::new(FixedClock::new(1_700_000_000_000)),
            Arc::clone(&sink) as Arc<dyn ContinuationSink>,
            Arc::clone(&observer) as Arc<dyn AppEventObserver>,
        )
        .await
        .unwrap();
        Harness {
            service,
            sink,
            observer,
        }
    }

    fn patch(field: &str, text: &str) -> AppDesignPatch {
        AppDesignPatch {
            ops: vec![AppDesignPatchOp::Set {
                field_id: field.into(),
                value: DesignValue::ShortText(text.into()),
            }],
            note: None,
        }
    }

    #[tokio::test]
    async fn create_list_and_reload_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let record = h
            .service
            .create_app(
                "  Habit Tracker  ",
                "a test app",
                Some("conv-1".into()),
            )
            .await
            .unwrap();
        assert_eq!(record.name, "Habit Tracker", "name is trimmed");
        assert_eq!(record.workflow_state, AppWorkflowState::CollectingSpec);
        assert_eq!(
            record.workspace_rel,
            format!("apps/{}/workspace", record.id)
        );
        assert!(ids::is_valid_app_id(&record.id));
        assert_eq!(h.service.list_apps().await, vec![record.clone()]);
        let events = h.take_events().await;
        assert!(matches!(&events[..], [AppEvent::AppsChanged { apps }] if apps.len() == 1));

        // Rebuild from disk alone.
        drop(h);
        let h2 = harness(dir.path()).await;
        assert_eq!(h2.service.list_apps().await, vec![record.clone()]);
        assert_eq!(h2.service.draft(&record.id).await.unwrap().revision, 0);
        assert_eq!(
            h2.service.runtime_record(&record.id).await.unwrap().state,
            AppRuntimeState::Stopped
        );
    }

    #[tokio::test]
    async fn create_rejects_blank_name() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let err = h
            .service
            .create_app("   ", "a test app", None)
            .await
            .unwrap_err();
        assert_eq!(err.code(), AppErrorCode::InvalidRequest);
        assert!(h.service.list_apps().await.is_empty());
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
            h.service.open_designer(missing).await.unwrap_err().code(),
            AppErrorCode::NotFound
        );
        assert_eq!(
            h.service
                .update_draft(missing, 0, &patch("t", "x"))
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
        assert_eq!(
            h.service
                .redeliver_undelivered(missing)
                .await
                .unwrap_err()
                .code(),
            AppErrorCode::NotFound
        );
    }

    #[tokio::test]
    async fn conflicting_update_emits_design_conflict_and_keeps_value() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let record = h
            .service
            .create_app("A", "a test app", None)
            .await
            .unwrap();
        h.service
            .update_draft(&record.id, 0, &patch("title", "Mine"))
            .await
            .unwrap();
        let _ = h.take_events().await;
        let err = h
            .service
            .update_draft(&record.id, 0, &patch("title", "Stale"))
            .await
            .unwrap_err();
        assert_eq!(err.code(), AppErrorCode::RevisionConflict);
        let events = h.take_events().await;
        assert_eq!(
            events,
            vec![AppEvent::DesignConflict {
                app_id: record.id.clone(),
                expected_revision: 0,
                actual_revision: 1,
            }]
        );
        // Value stays; nothing was silently overwritten (also true on disk).
        let draft = h.service.draft(&record.id).await.unwrap();
        assert_eq!(draft.revision, 1);
        assert_eq!(
            draft.fields.get("title"),
            Some(&DesignValue::ShortText("Mine".into()))
        );
        drop(h);
        let h2 = harness(dir.path()).await;
        assert_eq!(h2.service.draft(&record.id).await.unwrap().revision, 1);
    }

    #[tokio::test]
    async fn confirm_requires_exact_interaction_id_and_current_revision() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let record = h
            .service
            .create_app("A", "a test app", None)
            .await
            .unwrap();
        let interaction = h.service.open_designer(&record.id).await.unwrap();
        h.service
            .update_draft(&record.id, 0, &patch("title", "T"))
            .await
            .unwrap();
        // Guessed interaction id fails.
        let err = h
            .service
            .confirm_design(&record.id, "int-guessed", 1)
            .await
            .unwrap_err();
        assert_eq!(err.code(), AppErrorCode::InteractionInvalid);
        // Right id, stale revision fails.
        let err = h
            .service
            .confirm_design(&record.id, &interaction.interaction_id, 0)
            .await
            .unwrap_err();
        assert_eq!(err.code(), AppErrorCode::RevisionConflict);
        // Nothing was consumed or delivered by the failures.
        assert!(h
            .service
            .pending_interaction(&record.id)
            .await
            .unwrap()
            .is_some());
        assert!(h.sink.calls().is_empty());
        // Exact id + current revision succeeds and delivers the continuation.
        h.service
            .confirm_design(&record.id, &interaction.interaction_id, 1)
            .await
            .unwrap();
        assert_eq!(
            h.service.record(&record.id).await.unwrap().workflow_state,
            AppWorkflowState::Generating
        );
        let calls = h.sink.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1.seq, 1);
        assert!(h
            .service
            .interactions(&record.id)
            .await
            .unwrap()
            .undelivered
            .is_empty());
    }

    #[tokio::test]
    async fn delete_app_removes_record_and_directory() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let keep = h
            .service
            .create_app("Keep", "a test app", None)
            .await
            .unwrap();
        let gone = h
            .service
            .create_app("Gone", "a test app", None)
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
            .create_app("Busy", "a test app", None)
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

    /// Finding 13 injection mechanism: plant a DIRECTORY at a document's
    /// final path. `atomic_write` refuses to rename onto a non-regular file
    /// for EVERY uid (root included — unlike the old chmod probes, which
    /// were vacuous under root/CI), so the write fails deterministically.
    /// Returns the displaced document body for [`unsquat_document`].
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
            .create_app("Stuck", "a test app", None)
            .await
            .unwrap();
        let _ = h.take_events().await;
        let app_dir = dir.path().join("apps").join(&record.id);
        // Finding 13 mechanism, inverted for the rename seam: a regular FILE
        // squatting on `apps/.trash` makes the commit-point rename of the
        // trash-based removal fail for EVERY uid (creating/renaming through
        // a file path is impossible even for root) — no chmod probe, no
        // vacuous early return.
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

    #[tokio::test]
    async fn concurrent_snapshot_never_delivers_stale_state_after_a_commit() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let record = h
            .service
            .create_app("Race", "a test app", None)
            .await
            .unwrap();
        let _ = h.take_events().await;
        let service = Arc::new(h.service);
        // Race `AppsChanged` snapshots against workflow toggles; the
        // emission-order lock must make every delivered snapshot agree with
        // the last `WorkflowChanged` delivered before it.
        for _ in 0..25 {
            let snapshotter = {
                let service = Arc::clone(&service);
                tokio::spawn(async move {
                    service.announce_apps().await;
                })
            };
            let toggler = {
                let service = Arc::clone(&service);
                let app_id = record.id.clone();
                tokio::spawn(async move {
                    service.open_designer(&app_id).await.unwrap();
                    service.cancel_design(&app_id).await.unwrap();
                })
            };
            let (a, b) = tokio::join!(snapshotter, toggler);
            a.unwrap();
            b.unwrap();
        }
        service.flush_events().await;
        let mut committed = AppWorkflowState::CollectingSpec;
        for event in h.observer.take() {
            match event {
                AppEvent::WorkflowChanged { state, .. } => committed = state,
                AppEvent::AppsChanged { apps } => {
                    assert_eq!(apps.len(), 1);
                    assert_eq!(
                        apps[0].workflow_state, committed,
                        "a snapshot must agree with the last committed event delivered before it"
                    );
                }
                _ => {}
            }
        }
    }

    #[tokio::test]
    async fn runtime_updates_persist_and_emit() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let record = h
            .service
            .create_app("R", "a test app", None)
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
        // A reload is a fresh process: phase 1 has no process manager, so a
        // busy state found at load is a crash leftover and is reconciled
        // (finding 8) — it must NOT survive as `starting`.
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
    async fn failed_delivery_stays_queued_and_redelivers_once() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        h.sink.set_fail(true);
        let record = h
            .service
            .create_app("Q", "a test app", None)
            .await
            .unwrap();
        let interaction = h.service.open_designer(&record.id).await.unwrap();
        // confirm_design succeeds even though delivery fails (at-least-once).
        h.service
            .confirm_design(&record.id, &interaction.interaction_id, 0)
            .await
            .unwrap();
        let interactions = h.service.interactions(&record.id).await.unwrap();
        assert_eq!(interactions.undelivered.len(), 1);
        assert_eq!(interactions.last_delivered_seq, 0);
        assert!(h.sink.accepted().is_empty());

        // Rebuild from disk with a working sink: exactly one delivery.
        drop(h);
        let h2 = harness(dir.path()).await;
        let queued = h2.service.interactions(&record.id).await.unwrap();
        assert_eq!(queued.undelivered.len(), 1, "undelivered survives restart");
        assert_eq!(
            h2.service.redeliver_undelivered(&record.id).await.unwrap(),
            1
        );
        assert_eq!(h2.sink.accepted().len(), 1);
        assert_eq!(
            h2.service.redeliver_undelivered(&record.id).await.unwrap(),
            0
        );
        assert_eq!(h2.sink.calls().len(), 1, "second sweep delivers nothing");
        let drained = h2.service.interactions(&record.id).await.unwrap();
        assert!(drained.undelivered.is_empty());
        assert_eq!(drained.last_delivered_seq, 1);
    }

    /// The store persists a delivery (queue removal + `last_delivered_seq`
    /// advance) in ONE atomic write, so "seq both delivered and still queued"
    /// is a state no legal writer produces — since finding 6 it fails the
    /// load-time invariant validation as `storage_corrupt` instead of being
    /// silently pruned.
    #[tokio::test]
    async fn delivered_seq_still_queued_on_disk_is_storage_corrupt_at_load() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        h.sink.set_fail(true);
        let record = h
            .service
            .create_app("D", "a test app", None)
            .await
            .unwrap();
        let interaction = h.service.open_designer(&record.id).await.unwrap();
        h.service
            .confirm_design(&record.id, &interaction.interaction_id, 0)
            .await
            .unwrap();
        drop(h);

        // Hand-tamper: seq 1 both marked delivered and still queued.
        let mut interactions: AppInteractions = serde_json::from_str(
            &std::fs::read_to_string(
                dir.path()
                    .join("apps")
                    .join(&record.id)
                    .join("interactions.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(interactions.undelivered.len(), 1);
        interactions.last_delivered_seq = 1;
        storage::save_interactions(dir.path(), &record.id, &interactions).unwrap();

        let Err(err) = AppService::load(
            dir.path(),
            Arc::new(FixedClock::new(1_700_000_000_000)),
            Arc::new(RecordingContinuationSink::new()) as Arc<dyn ContinuationSink>,
            Arc::new(RecordingAppEventObserver::new()) as Arc<dyn AppEventObserver>,
        )
        .await
        else {
            panic!("a store with a delivered seq still queued must fail to load");
        };
        assert_eq!(err.code(), AppErrorCode::StorageCorrupt, "{err}");
        assert!(
            err.to_string().contains("violates continuation invariants"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn sink_failure_mid_sweep_keeps_remainder_queued() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        h.sink.set_fail(true);
        let record = h
            .service
            .create_app("M", "a test app", None)
            .await
            .unwrap();
        // Two continuations: cancel then confirm.
        let i1 = h.service.open_designer(&record.id).await.unwrap();
        assert!(i1.interaction_id.starts_with("int-"));
        h.service.cancel_design(&record.id).await.unwrap();
        let i2 = h.service.open_designer(&record.id).await.unwrap();
        h.service
            .confirm_design(&record.id, &i2.interaction_id, 0)
            .await
            .unwrap();
        assert_eq!(
            h.service
                .interactions(&record.id)
                .await
                .unwrap()
                .undelivered
                .len(),
            2
        );
        let err = h
            .service
            .redeliver_undelivered(&record.id)
            .await
            .unwrap_err();
        assert_eq!(err.code(), AppErrorCode::Io);
        // Working sink drains both, in order, exactly once.
        h.sink.set_fail(false);
        assert_eq!(
            h.service.redeliver_undelivered(&record.id).await.unwrap(),
            2
        );
        let seqs: Vec<u64> = h.sink.accepted().iter().map(|(_, c)| c.seq).collect();
        assert_eq!(seqs, vec![1, 2]);
        assert_eq!(h.service.redeliver_all_undelivered().await.unwrap(), 0);
    }

    /// Cleanup: the data root itself is owner-only on unix, matching the
    /// repo's private-state convention (the interior was already 0o700 via
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
    async fn checkpoints_are_empty_in_phase_one() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let record = h
            .service
            .create_app("C", "a test app", None)
            .await
            .unwrap();
        assert!(h
            .service
            .list_checkpoints(&record.id)
            .await
            .unwrap()
            .is_empty());
    }

    /// The store's own documents live inside `apps/<id>/workspace`, which is
    /// exactly the tree a checkpoint restore hard-resets. Nothing rewrites
    /// the design draft after a restore — persistence compares in-memory
    /// `committed` against in-memory `working`, so a draft rewound ON DISK is
    /// invisible to it — which is why the restore must not be able to rewind
    /// it in the first place. Asserted through a real reload from disk.
    #[tokio::test]
    async fn restore_checkpoint_does_not_rewind_the_design_draft_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let record = h
            .service
            .create_app("Restorable", "a test app", None)
            .await
            .unwrap();
        h.service
            .update_draft(&record.id, 0, &patch("title", "v1"))
            .await
            .unwrap();
        let first = h
            .service
            .create_checkpoint(
                &record.id,
                crate::types::AppCheckpointKind::ScaffoldCreated,
                "Scaffold created",
            )
            .await
            .unwrap();
        h.service
            .update_draft(&record.id, 1, &patch("title", "v2"))
            .await
            .unwrap();
        assert_eq!(h.service.draft(&record.id).await.unwrap().revision, 2);

        h.service
            .restore_checkpoint(&record.id, &first.id)
            .await
            .unwrap();

        drop(h);
        let h2 = harness(dir.path()).await;
        let draft = h2.service.draft(&record.id).await.unwrap();
        assert_eq!(draft.revision, 2);
        assert_eq!(
            draft.fields.get("title"),
            Some(&DesignValue::ShortText("v2".into()))
        );
    }

    /// The same contract for the population that actually has data: an app
    /// whose checkpoint history was written before the service documents were
    /// excluded. Its checkpoint tree still carries `design-spec.json`, so the
    /// restore's hard reset checks that blob back out — and nothing rewrites
    /// the draft afterwards, so the loss is silent and permanent. The fixture
    /// commits the documents with the pre-fix `add_all(["*"])` semantics,
    /// which is the only way to reach the state; a store that builds its own
    /// repository cannot.
    #[tokio::test]
    async fn restoring_a_legacy_checkpoint_does_not_rewind_the_design_draft_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let record = h
            .service
            .create_app("Legacy", "a test app", None)
            .await
            .unwrap();
        h.service
            .update_draft(&record.id, 0, &patch("title", "v1"))
            .await
            .unwrap();
        let workspace = dir.path().join(&record.workspace_rel);
        let legacy = crate::checkpoints::seed_legacy_checkpoint(&workspace, 1_000);

        h.service
            .update_draft(&record.id, 1, &patch("title", "v2"))
            .await
            .unwrap();
        assert_eq!(h.service.draft(&record.id).await.unwrap().revision, 2);

        h.service
            .restore_checkpoint(&record.id, &legacy)
            .await
            .unwrap();

        drop(h);
        let h2 = harness(dir.path()).await;
        let draft = h2.service.draft(&record.id).await.unwrap();
        assert_eq!(draft.revision, 2);
        assert_eq!(
            draft.fields.get("title"),
            Some(&DesignValue::ShortText("v2".into()))
        );
    }

    #[tokio::test]
    async fn generation_progress_requires_existing_app() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let record = h
            .service
            .create_app("P", "a test app", None)
            .await
            .unwrap();
        let _ = h.take_events().await;
        let progress = AppGenerationProgress {
            app_id: record.id.clone(),
            stage: "scaffold".into(),
            percent: Some(10),
            detail: None,
        };
        h.service
            .report_generation_progress(progress.clone())
            .await
            .unwrap();
        assert_eq!(
            h.take_events().await,
            vec![AppEvent::GenerationProgress(progress)]
        );
        let err = h
            .service
            .report_generation_progress(AppGenerationProgress {
                app_id: "beadfeed".into(),
                stage: "scaffold".into(),
                percent: None,
                detail: None,
            })
            .await
            .unwrap_err();
        assert_eq!(err.code(), AppErrorCode::NotFound);
    }

    #[tokio::test]
    async fn sweep_continues_past_one_apps_failing_sink() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        // Queue one undelivered continuation per app (sink down for both).
        h.sink.set_fail(true);
        let a = h
            .service
            .create_app("A", "a test app", None)
            .await
            .unwrap();
        let b = h
            .service
            .create_app("B", "a test app", None)
            .await
            .unwrap();
        for record in [&a, &b] {
            h.service.open_designer(&record.id).await.unwrap();
            h.service.cancel_design(&record.id).await.unwrap();
        }
        // A (first in stored order) keeps failing; B is healthy again.
        h.sink.set_fail(false);
        h.sink.set_fail_for(&a.id, true);
        let err = h.service.redeliver_all_undelivered().await.unwrap_err();
        assert_eq!(err.code(), AppErrorCode::Io, "A's failure is surfaced");
        // B was NOT starved by A's persistent failure.
        let b_ints = h.service.interactions(&b.id).await.unwrap();
        assert!(b_ints.undelivered.is_empty(), "B's continuation delivered");
        assert_eq!(b_ints.last_delivered_seq, 1);
        assert_eq!(
            h.sink
                .accepted()
                .iter()
                .map(|(app_id, _)| app_id.clone())
                .collect::<Vec<_>>(),
            vec![b.id.clone()]
        );
        // A's continuation stays queued and delivers once A recovers.
        assert_eq!(
            h.service
                .interactions(&a.id)
                .await
                .unwrap()
                .undelivered
                .len(),
            1
        );
        h.sink.set_fail_for(&a.id, false);
        assert_eq!(h.service.redeliver_all_undelivered().await.unwrap(), 1);
        assert!(h
            .service
            .interactions(&a.id)
            .await
            .unwrap()
            .undelivered
            .is_empty());
    }

    #[tokio::test]
    async fn failure_details_ride_workflow_changed_events() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let record = h
            .service
            .create_app("F", "a test app", None)
            .await
            .unwrap();
        let gate = h.service.open_designer(&record.id).await.unwrap();
        h.service
            .confirm_design(&record.id, &gate.interaction_id, 0)
            .await
            .unwrap();
        let _ = h.take_events().await;

        h.service
            .generation_failed(&record.id, Some("npm install failed".into()))
            .await
            .unwrap();
        assert_eq!(
            h.take_events().await,
            vec![AppEvent::WorkflowChanged {
                app_id: record.id.clone(),
                state: AppWorkflowState::GenerationFailed,
                detail: Some("npm install failed".into()),
            }],
            "generation_failed must surface the caller's detail"
        );

        h.service.retry_generation(&record.id).await.unwrap();
        h.service.generation_complete(&record.id).await.unwrap();
        let _ = h.take_events().await;
        h.service
            .validation_failed(&record.id, Some("tsc: 3 errors".into()))
            .await
            .unwrap();
        assert_eq!(
            h.take_events().await,
            vec![AppEvent::WorkflowChanged {
                app_id: record.id.clone(),
                state: AppWorkflowState::ValidationFailed,
                detail: Some("tsc: 3 errors".into()),
            }],
            "validation_failed must surface the caller's detail"
        );
    }

    #[tokio::test]
    async fn create_app_enforces_name_and_conversation_caps() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let err = h
            .service
            .create_app(
                &"x".repeat(MAX_NAME_BYTES + 1),
                "a test app",
                None,
            )
            .await
            .unwrap_err();
        assert_eq!(err.code(), AppErrorCode::InvalidRequest);
        let err = h
            .service
            .create_app(
                "A",
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
                &"x".repeat(MAX_NAME_BYTES),
                "a test app",
                Some("c".repeat(MAX_CONVERSATION_ID_BYTES)),
            )
            .await
            .unwrap();
    }

    /// Direct coverage for `brief`'s validation (empty-after-trim rejected,
    /// `MAX_BRIEF_BYTES` enforced, exactly-at-cap accepted, and the trimmed
    /// value is what's persisted) — finding from Task 2 review: every WIRE
    /// path (`host.rs::handle_create_app`, `local_apps_mcp.rs`'s `create`
    /// tool) currently passes an already-trimmed, non-empty, ≤200-byte
    /// `name` as `brief` (Tasks 10/11 have not wired a real `brief` input
    /// yet), so this validation was otherwise unreachable from any live
    /// caller and shipped with no test. Mirrors
    /// `create_app_enforces_name_and_conversation_caps` immediately above.
    #[tokio::test]
    async fn create_app_enforces_brief_caps() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        // Empty after trim is rejected, independent of `name`.
        let err = h
            .service
            .create_app("A", "   ", None)
            .await
            .unwrap_err();
        assert_eq!(err.code(), AppErrorCode::InvalidRequest);
        // Over the cap is rejected.
        let err = h
            .service
            .create_app("A", &"x".repeat(MAX_BRIEF_BYTES + 1), None)
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
            .create_app("A", &format!("  {}  ", "x".repeat(MAX_BRIEF_BYTES)), None)
            .await
            .unwrap();
        assert_eq!(record.brief, "x".repeat(MAX_BRIEF_BYTES));
    }

    #[tokio::test]
    async fn create_app_library_is_not_artificially_capped() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        for i in 0..101 {
            h.service
                .create_app(&format!("App {i}"), "a test app", None)
                .await
                .unwrap();
        }
        assert_eq!(h.service.list_apps().await.len(), 101);
    }

    #[tokio::test]
    async fn dismiss_suggestion_persists_without_changing_revision_and_refreshes_draft() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let record = h
            .service
            .create_app("Suggestion", "a test app", None)
            .await
            .unwrap();
        h.take_events().await;
        let suggestion = h
            .service
            .store_suggestion(&record.id, patch("accent", "#3366ff"))
            .await
            .unwrap();
        h.take_events().await;
        let wrong = h
            .service
            .dismiss_suggestion(&record.id, "wrong")
            .await
            .unwrap_err();
        assert_eq!(wrong.code(), AppErrorCode::InteractionInvalid);
        h.service
            .dismiss_suggestion(&record.id, &suggestion.suggestion_id)
            .await
            .unwrap();
        let draft = h.service.draft(&record.id).await.unwrap();
        assert_eq!(draft.revision, 0);
        assert!(draft.pending_suggestion.is_none());
        assert!(matches!(
            h.take_events().await.as_slice(),
            [AppEvent::DesignDraftChanged { revision: 0, .. }]
        ));
        drop(h);
        let reloaded = harness(dir.path()).await;
        assert!(reloaded
            .service
            .draft(&record.id)
            .await
            .unwrap()
            .pending_suggestion
            .is_none());
    }

    #[tokio::test]
    async fn draft_patches_enforce_size_caps() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let record = h
            .service
            .create_app("Caps", "a test app", None)
            .await
            .unwrap();
        let set_op = |field: &str, text: &str| AppDesignPatchOp::Set {
            field_id: field.into(),
            value: DesignValue::ShortText(text.into()),
        };
        let oversized: Vec<AppDesignPatch> = vec![
            // Too many ops.
            AppDesignPatch {
                ops: (0..=MAX_PATCH_OPS)
                    .map(|i| set_op(&format!("f{i}"), "v"))
                    .collect(),
                note: None,
            },
            // Oversized field id (set + remove).
            patch(&"f".repeat(MAX_FIELD_ID_BYTES + 1), "v"),
            AppDesignPatch {
                ops: vec![AppDesignPatchOp::Remove {
                    field_id: "f".repeat(MAX_FIELD_ID_BYTES + 1),
                }],
                note: None,
            },
            // Oversized text value.
            patch("big", &"v".repeat(MAX_TEXT_VALUE_BYTES + 1)),
            // Oversized list (item count, then item size).
            AppDesignPatch {
                ops: vec![AppDesignPatchOp::Set {
                    field_id: "screens".into(),
                    value: DesignValue::ScreenList(vec!["s".into(); MAX_LIST_ITEMS + 1]),
                }],
                note: None,
            },
            AppDesignPatch {
                ops: vec![AppDesignPatchOp::Set {
                    field_id: "screens".into(),
                    value: DesignValue::ScreenList(vec!["s".repeat(MAX_LIST_ITEM_BYTES + 1)]),
                }],
                note: None,
            },
            // Oversized note.
            AppDesignPatch {
                ops: vec![set_op("t", "v")],
                note: Some("n".repeat(MAX_NOTE_BYTES + 1)),
            },
        ];
        for (i, bad) in oversized.iter().enumerate() {
            let err = h
                .service
                .update_draft(&record.id, 0, bad)
                .await
                .expect_err("oversized patch must be rejected");
            assert_eq!(err.code(), AppErrorCode::InvalidRequest, "patch #{i}");
            let err = h
                .service
                .store_suggestion(&record.id, bad.clone())
                .await
                .expect_err("oversized suggestion must be rejected");
            assert_eq!(err.code(), AppErrorCode::InvalidRequest, "suggestion #{i}");
        }
        // Nothing was applied, emitted, or persisted.
        let draft = h.service.draft(&record.id).await.unwrap();
        assert_eq!(draft.revision, 0);
        assert!(draft.fields.is_empty());
        assert!(draft.pending_suggestion.is_none());
        drop(h);
        let h2 = harness(dir.path()).await;
        assert_eq!(h2.service.draft(&record.id).await.unwrap().revision, 0);
    }

    /// Data-field ids and HTTPS domains are copied verbatim onto the manifest
    /// by the scaffold, which validates them while the app is already
    /// `generating` — a state whose only exit replays the same input. The
    /// draft gate must therefore reject them here, where the revision and the
    /// workflow state both survive. Both clients mint hyphenated field ids
    /// (`UUID().uuidString.lowercased()` / `field-$index`), so this is the
    /// ordinary "add a data field" path, not an exotic one.
    #[tokio::test]
    async fn draft_gate_rejects_values_the_manifest_contract_forbids() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let record = h
            .service
            .create_app("Contract", "a test app", None)
            .await
            .unwrap();
        let data_fields = |id: &str| AppDesignPatch {
            ops: vec![AppDesignPatchOp::Set {
                field_id: "collection_fields".into(),
                value: DesignValue::DataFieldList(vec![DataFieldSchema {
                    id: id.into(),
                    label: "Title".into(),
                    kind: DataFieldKind::Text,
                    required: false,
                    enum_options: Vec::new(),
                }]),
            }],
            note: None,
        };
        let domains = |domain: &str| AppDesignPatch {
            ops: vec![AppDesignPatchOp::Set {
                field_id: "network_domains".into(),
                value: DesignValue::DomainList(vec![domain.into()]),
            }],
            note: None,
        };
        let rejected: Vec<AppDesignPatch> = vec![
            // The iOS mint: a lowercased UUID (hyphens, digit-initial).
            data_fields("6f0b62b9-5d4a-4d33-9c56-2c9c8a1c2f21"),
            // The Android mint.
            data_fields("field-1"),
            // A pasted URL and a host typed with capitals.
            domains("https://api.example.com"),
            domains("API.Example.com"),
        ];
        for (i, bad) in rejected.iter().enumerate() {
            let err = h
                .service
                .update_draft(&record.id, 0, bad)
                .await
                .expect_err("patch violating the manifest contract must be rejected");
            assert_eq!(err.code(), AppErrorCode::InvalidRequest, "patch #{i}");
            let err = h
                .service
                .store_suggestion(&record.id, bad.clone())
                .await
                .expect_err("suggestion violating the manifest contract must be rejected");
            assert_eq!(err.code(), AppErrorCode::InvalidRequest, "suggestion #{i}");
        }
        // The rejection is revision- and state-preserving, so the next edit
        // resubmits at the same `expected_revision`.
        let draft = h.service.draft(&record.id).await.unwrap();
        assert_eq!(draft.revision, 0);
        assert!(draft.fields.is_empty());
        // Contract-valid values still pass (no over-rejection).
        assert_eq!(
            h.service
                .update_draft(&record.id, 0, &data_fields("recorded_at"))
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            h.service
                .update_draft(&record.id, 1, &domains("api.example.com"))
                .await
                .unwrap(),
            2
        );
    }

    #[tokio::test]
    async fn draft_field_count_accumulation_is_capped() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let record = h
            .service
            .create_app("Fields", "a test app", None)
            .await
            .unwrap();
        // Fill to the cap across several max-size patches.
        assert_eq!(
            MAX_DRAFT_FIELDS % MAX_PATCH_OPS,
            0,
            "test assumes even split"
        );
        let mut revision = 0;
        for chunk in 0..(MAX_DRAFT_FIELDS / MAX_PATCH_OPS) {
            let ops = (0..MAX_PATCH_OPS)
                .map(|i| AppDesignPatchOp::Set {
                    field_id: format!("f{}", chunk * MAX_PATCH_OPS + i),
                    value: DesignValue::Boolean(true),
                })
                .collect();
            revision = h
                .service
                .update_draft(&record.id, revision, &AppDesignPatch { ops, note: None })
                .await
                .unwrap();
        }
        // One more NEW field crosses the cap and is rejected un-persisted.
        let err = h
            .service
            .update_draft(&record.id, revision, &patch("f-one-more", "v"))
            .await
            .unwrap_err();
        assert_eq!(err.code(), AppErrorCode::InvalidRequest);
        let draft = h.service.draft(&record.id).await.unwrap();
        assert_eq!(draft.revision, revision, "rejected patch must not commit");
        assert_eq!(draft.fields.len(), MAX_DRAFT_FIELDS);
        // Overwriting an EXISTING field at the cap is still fine.
        h.service
            .update_draft(&record.id, revision, &patch("f0", "updated"))
            .await
            .unwrap();
        // The cap also applies through apply_suggestion.
        let suggestion = h
            .service
            .store_suggestion(&record.id, patch("f-via-suggestion", "v"))
            .await
            .unwrap();
        let err = h
            .service
            .apply_suggestion(&record.id, &suggestion.suggestion_id, revision + 1)
            .await
            .unwrap_err();
        assert_eq!(err.code(), AppErrorCode::InvalidRequest);
        assert_eq!(
            h.service.draft(&record.id).await.unwrap().fields.len(),
            MAX_DRAFT_FIELDS
        );
    }

    #[tokio::test]
    async fn request_revision_prompt_is_capped_before_state_gating() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let record = h
            .service
            .create_app("Prompt", "a test app", None)
            .await
            .unwrap();
        // Oversized prompt: rejected as invalid_request even though the state
        // gate would also refuse — the cap runs first.
        let err = h
            .service
            .request_revision(&record.id, &"p".repeat(MAX_PROMPT_BYTES + 1))
            .await
            .unwrap_err();
        assert_eq!(err.code(), AppErrorCode::InvalidRequest);
        // A within-cap prompt in the wrong state hits the state gate instead.
        let err = h
            .service
            .request_revision(&record.id, "make it blue")
            .await
            .unwrap_err();
        assert_eq!(err.code(), AppErrorCode::WorkflowStateInvalid);
    }

    #[tokio::test]
    async fn noop_sink_marks_continuations_delivered() {
        let dir = tempfile::tempdir().unwrap();
        let observer = Arc::new(RecordingAppEventObserver::new());
        let service = AppService::load(
            dir.path(),
            Arc::new(FixedClock::new(7)),
            Arc::new(NoopContinuationSink),
            observer as Arc<dyn AppEventObserver>,
        )
        .await
        .unwrap();
        let record = service
            .create_app("N", "a test app", None)
            .await
            .unwrap();
        service.open_designer(&record.id).await.unwrap();
        service.cancel_design(&record.id).await.unwrap();
        let interactions = service.interactions(&record.id).await.unwrap();
        assert!(interactions.undelivered.is_empty());
        assert_eq!(interactions.last_delivered_seq, 1);
    }

    /// Finding 1: a mutation whose serialized document would exceed
    /// [`storage::MAX_DOC_BYTES`] fails typed BEFORE anything reaches disk —
    /// each patch below is within every service input cap, yet the
    /// accumulated draft (JSON escaping expands every control char to six
    /// bytes) would out-grow the load limit and brick the store.
    #[tokio::test]
    async fn over_cap_draft_mutation_fails_typed_and_the_store_still_loads() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let record = h
            .service
            .create_app("Big", "a test app", None)
            .await
            .unwrap();
        let value = "\u{1}".repeat(MAX_TEXT_VALUE_BYTES); // in caps; escapes 6x
        let chunk = |chunk: usize| AppDesignPatch {
            ops: (0..MAX_PATCH_OPS)
                .map(|i| AppDesignPatchOp::Set {
                    field_id: format!("f{}", chunk * MAX_PATCH_OPS + i),
                    value: DesignValue::LongText(value.clone()),
                })
                .collect(),
            note: None,
        };
        // The first in-caps patch lands (~7.7 MiB serialized draft)…
        let revision = h
            .service
            .update_draft(&record.id, 0, &chunk(0))
            .await
            .unwrap();
        // …the second would push the persisted document past 8 MiB.
        let err = h
            .service
            .update_draft(&record.id, revision, &chunk(1))
            .await
            .unwrap_err();
        assert_eq!(err.code(), AppErrorCode::InvalidRequest);
        assert!(err.to_string().contains("durable size limit"), "{err}");
        // Memory rolled back with disk: the failed patch never committed.
        let draft = h.service.draft(&record.id).await.unwrap();
        assert_eq!(draft.revision, revision);
        assert_eq!(draft.fields.len(), MAX_PATCH_OPS);
        // The store on disk still loads cleanly afterwards.
        drop(h);
        let h2 = harness(dir.path()).await;
        let reloaded = h2.service.draft(&record.id).await.unwrap();
        assert_eq!(reloaded.revision, revision);
        assert_eq!(reloaded.fields.len(), MAX_PATCH_OPS);
    }

    /// Finding 3(a): when the per-app batch lands but the index write fails,
    /// the compensating rollback rewrites the ORIGINAL documents so a reload
    /// agrees with the failure the caller saw — the mirror-wins repair must
    /// NOT commit the transition the user was told failed. Injection is a
    /// directory squat on `apps/index.json` (finding 13): it defeats root,
    /// where the old chmod probe returned vacuously.
    #[tokio::test]
    async fn failed_index_write_rolls_back_disk_so_reload_agrees_with_the_failure() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let record = h
            .service
            .create_app("Roll", "a test app", None)
            .await
            .unwrap();
        let gate = h.service.open_designer(&record.id).await.unwrap();
        let _ = h.take_events().await;

        // Make ONLY the index write fail: a directory squatting on
        // index.json blocks the rename for every uid; per-app documents in
        // the subdirectories stay writable.
        let index_path = dir.path().join("apps/index.json");
        let index_before = squat_document(&index_path);
        let per_app_docs = [
            dir.path()
                .join("apps")
                .join(&record.id)
                .join("interactions.json"),
            dir.path()
                .join("apps")
                .join(&record.id)
                .join("workspace/.lingxi/design-spec.json"),
            dir.path()
                .join("apps")
                .join(&record.id)
                .join("workspace/.lingxi/app.json"),
        ];
        let per_app_before: Vec<String> = per_app_docs
            .iter()
            .map(|path| std::fs::read_to_string(path).unwrap())
            .collect();

        let err = h
            .service
            .confirm_design(&record.id, &gate.interaction_id, 0)
            .await
            .unwrap_err();
        // A squatted document path is store tampering, typed storage_corrupt
        // (the write-side twin of the load-side squat contract).
        assert_eq!(err.code(), AppErrorCode::StorageCorrupt, "{err}");
        // Memory rolled back: the gate is still pending, nothing was queued,
        // and no success event leaked out.
        assert_eq!(
            h.service.record(&record.id).await.unwrap().workflow_state,
            AppWorkflowState::AwaitingSpecConfirmation
        );
        let interactions = h.service.interactions(&record.id).await.unwrap();
        assert!(
            interactions.undelivered.is_empty(),
            "no phantom continuation"
        );
        assert_eq!(
            interactions
                .pending
                .as_ref()
                .map(|p| p.interaction_id.as_str()),
            Some(gate.interaction_id.as_str())
        );
        assert!(
            h.take_events().await.is_empty(),
            "a failed mutation emits nothing"
        );
        // DISK rolled back too: the compensating rollback restored every
        // per-app document byte-for-byte.
        for (path, before) in per_app_docs.iter().zip(&per_app_before) {
            assert_eq!(
                &std::fs::read_to_string(path).unwrap(),
                before,
                "{} must be rolled back to its pre-transition bytes",
                path.display()
            );
        }

        unsquat_document(&index_path, &index_before);

        // Disk agrees with the reported failure: the reload does NOT
        // resurrect the confirm, and the surviving gate is re-announced and
        // still satisfiable.
        drop(h);
        let h2 = harness(dir.path()).await;
        assert_eq!(
            h2.service.record(&record.id).await.unwrap().workflow_state,
            AppWorkflowState::AwaitingSpecConfirmation
        );
        assert!(h2
            .service
            .interactions(&record.id)
            .await
            .unwrap()
            .undelivered
            .is_empty());
        let announced = h2.take_events().await;
        assert!(
            announced.iter().any(|event| matches!(
                event,
                AppEvent::DesignerRequested { interaction_id, .. }
                    if *interaction_id == gate.interaction_id
            )),
            "the surviving gate is re-announced at load: {announced:?}"
        );
        h2.service
            .confirm_design(&record.id, &gate.interaction_id, 0)
            .await
            .unwrap();
        assert_eq!(
            h2.service.record(&record.id).await.unwrap().workflow_state,
            AppWorkflowState::Generating
        );
        assert_eq!(h2.sink.accepted().len(), 1);
    }

    /// Finding 1: a MID-BATCH write failure (not just the index seam) now
    /// triggers the compensating rollback — the succeeded prefix's ORIGINAL
    /// documents are rewritten, so disk never keeps a committed prefix of a
    /// transition the caller was told failed (which the next load would have
    /// rolled FORWARD). Injection: directory squat on `interactions.json`
    /// (finding 13's mechanism — works under any uid), which fails
    /// `confirm_design`'s batch after `design-spec.json` already landed.
    #[tokio::test]
    async fn failed_mid_batch_write_rolls_back_the_succeeded_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let record = h
            .service
            .create_app("MidBatch", "a test app", None)
            .await
            .unwrap();
        let gate = h.service.open_designer(&record.id).await.unwrap();
        let _ = h.take_events().await;
        let app_dir = dir.path().join("apps").join(&record.id);
        let draft_path = app_dir.join("workspace/.lingxi/design-spec.json");
        let draft_before = std::fs::read_to_string(&draft_path).unwrap();
        assert!(
            !draft_before.contains("confirmedRevision"),
            "precondition: the confirm has not happened yet"
        );

        // confirm_design changes draft (confirmedRevision), interactions and
        // the record — the canonical batch writes design-spec FIRST, then
        // fails at the squatted interactions.json mid-batch.
        let interactions_path = app_dir.join("interactions.json");
        let interactions_before = squat_document(&interactions_path);
        let err = h
            .service
            .confirm_design(&record.id, &gate.interaction_id, 0)
            .await
            .unwrap_err();
        assert_eq!(err.code(), AppErrorCode::StorageCorrupt, "{err}");

        // The succeeded prefix (design-spec.json) was rolled back to its
        // ORIGINAL bytes — no committed prefix of the failed transition.
        assert_eq!(std::fs::read_to_string(&draft_path).unwrap(), draft_before);
        // Memory agrees: gate still armed, nothing queued, nothing emitted.
        assert_eq!(
            h.service.record(&record.id).await.unwrap().workflow_state,
            AppWorkflowState::AwaitingSpecConfirmation
        );
        let interactions = h.service.interactions(&record.id).await.unwrap();
        assert!(
            interactions.undelivered.is_empty(),
            "no phantom continuation"
        );
        assert_eq!(
            interactions
                .pending
                .as_ref()
                .map(|p| p.interaction_id.as_str()),
            Some(gate.interaction_id.as_str())
        );
        assert!(
            h.take_events().await.is_empty(),
            "a failed mutation emits nothing"
        );

        unsquat_document(&interactions_path, &interactions_before);

        // Reload shows the ORIGINAL state — the gate is re-announced, still
        // armed, and satisfiable; no phantom continuation ever surfaces.
        drop(h);
        let h2 = harness(dir.path()).await;
        assert_eq!(
            h2.service.record(&record.id).await.unwrap().workflow_state,
            AppWorkflowState::AwaitingSpecConfirmation
        );
        let reloaded = h2.service.interactions(&record.id).await.unwrap();
        assert!(reloaded.undelivered.is_empty(), "no phantom continuation");
        assert_eq!(
            reloaded.pending.as_ref().map(|p| p.interaction_id.as_str()),
            Some(gate.interaction_id.as_str()),
            "the gate is still armed after reload"
        );
        assert!(
            h2.service
                .draft(&record.id)
                .await
                .unwrap()
                .confirmed_revision
                .is_none(),
            "the failed confirm left no committed residue"
        );
        let announced = h2.take_events().await;
        assert!(
            announced.iter().any(|event| matches!(
                event,
                AppEvent::DesignerRequested { interaction_id, .. }
                    if *interaction_id == gate.interaction_id
            )),
            "the surviving gate is re-announced at load: {announced:?}"
        );
        h2.service
            .confirm_design(&record.id, &gate.interaction_id, 0)
            .await
            .unwrap();
        assert_eq!(
            h2.sink.accepted().len(),
            1,
            "exactly one delivery after re-confirm"
        );
    }

    /// Finding 3(b): redelivery merges the on-disk queue (union by seq) with
    /// memory, so a continuation that reached disk but was rolled back out of
    /// memory is delivered instead of being clobbered by a memory-derived
    /// persist.
    #[tokio::test]
    async fn redelivery_merges_disk_only_continuations_instead_of_clobbering() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        h.sink.set_fail(true);
        let record = h
            .service
            .create_app("Merge", "a test app", None)
            .await
            .unwrap();
        h.service.open_designer(&record.id).await.unwrap();
        h.service.cancel_design(&record.id).await.unwrap(); // seq 1 queued

        // Simulate the residual window: seq 2 reached DISK while memory
        // rolled back and never learned about it.
        let path = dir
            .path()
            .join("apps")
            .join(&record.id)
            .join("interactions.json");
        let mut on_disk: AppInteractions =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        on_disk.undelivered.push(AppContinuation {
            seq: 2,
            app_id: record.id.clone(),
            kind: AppContinuationKind::DesignConfirmed,
            payload: serde_json::json!({ "revision": 0 }),
            created_at_ms: 9,
        });
        on_disk.next_seq = 3;
        storage::save_interactions(dir.path(), &record.id, &on_disk).unwrap();

        h.sink.set_fail(false);
        assert_eq!(
            h.service.redeliver_undelivered(&record.id).await.unwrap(),
            2,
            "the disk-only seq must be delivered, not erased"
        );
        let seqs: Vec<u64> = h.sink.accepted().iter().map(|(_, c)| c.seq).collect();
        assert_eq!(seqs, vec![1, 2]);
        let drained = h.service.interactions(&record.id).await.unwrap();
        assert!(drained.undelivered.is_empty());
        assert_eq!(drained.last_delivered_seq, 2);
        assert_eq!(drained.next_seq, 3, "the counter is adopted from disk");
        // The merged outcome is what a fresh process sees.
        drop(h);
        let h2 = harness(dir.path()).await;
        let reloaded = h2.service.interactions(&record.id).await.unwrap();
        assert_eq!(reloaded.last_delivered_seq, 2);
        assert_eq!(reloaded.next_seq, 3);
        assert!(reloaded.undelivered.is_empty());
    }

    /// Sink that, on its first delivery, plants an extra continuation
    /// straight onto DISK — modelling a racing commit whose interactions
    /// write landed while the delivery was in flight (and whose memory copy
    /// was rolled back). Wraps a [`RecordingContinuationSink`] for the
    /// assertion surface.
    struct PlantingSink {
        inner: Arc<RecordingContinuationSink>,
        root: std::path::PathBuf,
        plant_for: std::sync::Mutex<Option<String>>,
    }

    #[async_trait::async_trait]
    impl ContinuationSink for PlantingSink {
        async fn deliver(
            &self,
            app_id: &str,
            continuation: &AppContinuation,
        ) -> Result<(), AppError> {
            self.inner.deliver(app_id, continuation).await?;
            let target = self.plant_for.lock().expect("planting sink lock").take();
            if let Some(planted_app) = target {
                let mut disk = storage::load_interactions(&self.root, &planted_app)
                    .expect("planting sink reads a valid store");
                let seq = disk.next_seq;
                disk.undelivered.push(AppContinuation {
                    seq,
                    app_id: planted_app.clone(),
                    kind: AppContinuationKind::RevisionRequested,
                    payload: serde_json::json!({ "prompt": "raced in mid-delivery" }),
                    created_at_ms: 9,
                });
                disk.next_seq += 1;
                storage::save_interactions(&self.root, &planted_app, &disk)
                    .expect("planting sink writes a valid store");
            }
            Ok(())
        }
    }

    /// Redelivery horn A regression (the review's finding 4, verified for
    /// real): a continuation that lands on DISK while a delivery is IN
    /// FLIGHT must survive the post-delivery bookkeeping — the old code
    /// persisted a memory-derived view there, erasing the disk-only seq and
    /// rewinding `nextSeq`; the merge-on-both-sides loop must deliver it in
    /// the same drain instead.
    #[tokio::test]
    async fn in_flight_disk_continuation_survives_post_delivery_bookkeeping() {
        let dir = tempfile::tempdir().unwrap();
        let recording = Arc::new(RecordingContinuationSink::new());
        let sink = Arc::new(PlantingSink {
            inner: Arc::clone(&recording),
            root: dir.path().to_path_buf(),
            plant_for: std::sync::Mutex::new(None),
        });
        let observer = Arc::new(RecordingAppEventObserver::new());
        let service = AppService::load(
            dir.path(),
            Arc::new(FixedClock::new(1_700_000_000_000)),
            Arc::clone(&sink) as Arc<dyn ContinuationSink>,
            observer as Arc<dyn AppEventObserver>,
        )
        .await
        .unwrap();
        recording.set_fail(true);
        let record = service
            .create_app("Race", "a test app", None)
            .await
            .unwrap();
        service.open_designer(&record.id).await.unwrap();
        service.cancel_design(&record.id).await.unwrap(); // seq 1 queued
        recording.set_fail(false);

        // Arm the plant: the NEXT deliver (seq 1) gets seq 2 written to disk
        // mid-flight.
        *sink.plant_for.lock().unwrap() = Some(record.id.clone());
        assert_eq!(
            service.redeliver_undelivered(&record.id).await.unwrap(),
            2,
            "the mid-flight disk continuation must be delivered in the same drain"
        );
        let seqs: Vec<u64> = recording.accepted().iter().map(|(_, c)| c.seq).collect();
        assert_eq!(seqs, vec![1, 2], "both seqs delivered, in order");
        let drained = service.interactions(&record.id).await.unwrap();
        assert!(
            drained.undelivered.is_empty(),
            "nothing clobbered, nothing left"
        );
        assert_eq!(drained.last_delivered_seq, 2);
        assert_eq!(
            drained.next_seq, 3,
            "the counter adopted the mid-flight mint — never rewound"
        );
    }

    /// Redelivery horn B regression (the review's finding 4, verified for
    /// real): when DISK proves memory's armed gate was consumed (pending
    /// cleared + a seq memory never minted), the merge must resolve the
    /// state FORWARD through the shared repair table instead of persisting
    /// an armed-gate + consumption-evidence hybrid — and a second user
    /// confirm must be refused rather than minting a duplicate
    /// `design_confirmed`.
    #[tokio::test]
    async fn disk_proof_of_consumed_gate_resolves_forward_not_double_confirm() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let record = h
            .service
            .create_app("Proof", "a test app", None)
            .await
            .unwrap();
        let gate = h.service.open_designer(&record.id).await.unwrap();

        // Residual window by hand: on DISK the confirm committed (gate
        // cleared + design_confirmed minted) while MEMORY still holds the
        // armed gate.
        let mut on_disk = storage::load_interactions(dir.path(), &record.id).unwrap();
        assert!(
            on_disk.pending.is_some(),
            "precondition: gate armed on disk too"
        );
        on_disk.pending = None;
        on_disk.undelivered.push(AppContinuation {
            seq: 1,
            app_id: record.id.clone(),
            kind: AppContinuationKind::DesignConfirmed,
            payload: serde_json::json!({ "revision": 0 }),
            created_at_ms: 9,
        });
        on_disk.next_seq = 2;
        storage::save_interactions(dir.path(), &record.id, &on_disk).unwrap();

        assert_eq!(
            h.service.redeliver_undelivered(&record.id).await.unwrap(),
            1,
            "the proven confirm is delivered exactly once"
        );
        assert_eq!(
            h.sink
                .accepted()
                .iter()
                .map(|(_, c)| c.kind)
                .collect::<Vec<_>>(),
            vec![AppContinuationKind::DesignConfirmed]
        );
        // The contradiction resolved FORWARD: gate gone, state follows the
        // evidence, and the mirror was persisted with it.
        assert!(h
            .service
            .pending_interaction(&record.id)
            .await
            .unwrap()
            .is_none());
        assert_eq!(
            h.service.record(&record.id).await.unwrap().workflow_state,
            AppWorkflowState::Generating
        );
        let mirror = std::fs::read_to_string(
            dir.path()
                .join("apps")
                .join(&record.id)
                .join("workspace/.lingxi/app.json"),
        )
        .unwrap();
        assert!(mirror.contains("\"generating\""), "{mirror}");
        // No hybrid on disk: the persisted store passes its own invariants
        // and holds no armed gate alongside the evidence.
        let persisted = storage::load_interactions(dir.path(), &record.id).unwrap();
        assert!(persisted.pending.is_none());
        assert!(persisted.undelivered.is_empty());
        assert_eq!(persisted.last_delivered_seq, 1);
        // A second confirm of the stale gate is REFUSED — no duplicate
        // design_confirmed at a fresh seq.
        let err = h
            .service
            .confirm_design(&record.id, &gate.interaction_id, 0)
            .await
            .unwrap_err();
        assert_eq!(err.code(), AppErrorCode::WorkflowStateInvalid);
        let after = h.service.interactions(&record.id).await.unwrap();
        assert_eq!(after.next_seq, 2, "no second continuation was minted");
        assert!(after.undelivered.is_empty());
        // The rolled-forward state is live: the generator's completion is
        // accepted.
        h.service.generation_complete(&record.id).await.unwrap();
    }

    /// Finding 4: `last_error` is the one persisted string that TRUNCATES
    /// instead of rejecting — refusing a runtime failure report would lose
    /// the evidence entirely.
    #[tokio::test]
    async fn oversized_runtime_last_error_is_truncated_not_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let record = h
            .service
            .create_app("Err", "a test app", None)
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

    /// Finding 9: a pending suggestion planted straight into the store (never
    /// gated by `store_suggestion`) is re-validated at apply time; an
    /// over-cap one fails typed AND is consumed so it cannot wedge the draft.
    #[tokio::test]
    async fn disk_planted_over_cap_suggestion_is_rejected_and_consumed() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let record = h
            .service
            .create_app("Sugg", "a test app", None)
            .await
            .unwrap();
        h.service
            .update_draft(&record.id, 0, &patch("title", "T"))
            .await
            .unwrap();
        drop(h);

        // Plant the poisoned suggestion via a direct storage write.
        let mut draft: AppDesignDraft = serde_json::from_str(
            &std::fs::read_to_string(
                dir.path()
                    .join("apps")
                    .join(&record.id)
                    .join("workspace/.lingxi/design-spec.json"),
            )
            .unwrap(),
        )
        .unwrap();
        draft.pending_suggestion = Some(AppDesignSuggestion {
            suggestion_id: "sugg-planted".into(),
            patch: AppDesignPatch {
                ops: vec![AppDesignPatchOp::Set {
                    field_id: "poison".into(),
                    value: DesignValue::LongText("p".repeat(MAX_TEXT_VALUE_BYTES + 1)),
                }],
                note: None,
            },
            based_on_revision: 1,
        });
        storage::save_draft(dir.path(), &record.id, &draft).unwrap();

        let h2 = harness(dir.path()).await;
        let err = h2
            .service
            .apply_suggestion(&record.id, "sugg-planted", 1)
            .await
            .unwrap_err();
        assert_eq!(err.code(), AppErrorCode::InvalidRequest);
        let after = h2.service.draft(&record.id).await.unwrap();
        assert!(after.pending_suggestion.is_none(), "poison is consumed");
        assert_eq!(after.revision, 1, "nothing was applied");
        assert!(!after.fields.contains_key("poison"));
        assert!(
            !h2.observer
                .take()
                .iter()
                .any(|event| matches!(event, AppEvent::DesignDraftChanged { .. })),
            "a rejected apply announces no draft change"
        );
        // The consumption is persisted, not just in memory.
        drop(h2);
        let h3 = harness(dir.path()).await;
        assert!(h3
            .service
            .draft(&record.id)
            .await
            .unwrap()
            .pending_suggestion
            .is_none());
    }

    /// Finding 15: progress reports cap `stage`/`detail` like every sibling
    /// input (rejection loses nothing — the report is not persisted) and
    /// range-check `percent`.
    #[tokio::test]
    async fn generation_progress_rejects_oversized_and_out_of_range_input() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let record = h
            .service
            .create_app("P", "a test app", None)
            .await
            .unwrap();
        let _ = h.take_events().await;
        let base = AppGenerationProgress {
            app_id: record.id.clone(),
            stage: "scaffold".into(),
            percent: Some(100),
            detail: Some("d".repeat(MAX_TEXT_VALUE_BYTES)),
        };
        // At the caps everything is accepted.
        h.service
            .report_generation_progress(base.clone())
            .await
            .unwrap();
        assert_eq!(h.take_events().await.len(), 1);

        let oversized_stage = AppGenerationProgress {
            stage: "s".repeat(MAX_TEXT_VALUE_BYTES + 1),
            ..base.clone()
        };
        let oversized_detail = AppGenerationProgress {
            detail: Some("d".repeat(MAX_TEXT_VALUE_BYTES + 1)),
            ..base.clone()
        };
        let out_of_range_percent = AppGenerationProgress {
            percent: Some(101),
            ..base
        };
        for (label, bad) in [
            ("stage", oversized_stage),
            ("detail", oversized_detail),
            ("percent", out_of_range_percent),
        ] {
            let err = h.service.report_generation_progress(bad).await.unwrap_err();
            assert_eq!(err.code(), AppErrorCode::InvalidRequest, "{label}");
        }
        assert!(
            h.take_events().await.is_empty(),
            "rejected reports emit nothing"
        );
    }

    /// Finding 5: a load re-announces the pending gate of EVERY app through
    /// the observer, so a persisted `interaction_id` whose announcement was
    /// lost to a crash still reaches the client.
    #[tokio::test]
    async fn load_reannounces_every_pending_gate() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let a = h
            .service
            .create_app("GateA", "a test app", None)
            .await
            .unwrap();
        let designer_gate = h.service.open_designer(&a.id).await.unwrap();
        let b = h
            .service
            .create_app("GateB", "a test app", None)
            .await
            .unwrap();
        let gate = h.service.open_designer(&b.id).await.unwrap();
        h.service
            .confirm_design(&b.id, &gate.interaction_id, 0)
            .await
            .unwrap();
        h.service.generation_complete(&b.id).await.unwrap();
        let preview_gate = h.service.validation_passed(&b.id).await.unwrap();
        drop(h);

        let h2 = harness(dir.path()).await;
        assert_eq!(
            h2.take_events().await,
            vec![
                AppEvent::DesignerRequested {
                    app_id: a.id.clone(),
                    interaction_id: designer_gate.interaction_id.clone(),
                    revision: 0,
                },
                AppEvent::PreviewReady {
                    app_id: b.id.clone(),
                    interaction_id: preview_gate.interaction_id.clone(),
                    revision: 0,
                    url: None,
                },
            ],
            "both pending gates are announced at load, in stored order"
        );
    }

    /// The host subscribes its observers AFTER `load` returns, so the load-time
    /// announcement lands in a fanout with no subscribers and is dropped. A
    /// relaunched client is then left without the pending `interaction_id` that
    /// gates `confirm_design`, and the UI — seeing no id — tries
    /// `open_designer`, which is illegal from `awaiting_spec_confirmation`.
    /// `resync_pending_gates` is what re-delivers it to a late subscriber.
    #[tokio::test]
    async fn resync_reannounces_pending_gates_to_a_late_subscriber() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let a = h
            .service
            .create_app("LateSub", "a test app", None)
            .await
            .unwrap();
        let gate = h.service.open_designer(&a.id).await.unwrap();
        drop(h);

        let h2 = harness(dir.path()).await;
        // Drain the load-time announcement: this stands in for the events the
        // real host never sees, because it has not subscribed yet.
        let _ = h2.take_events().await;

        h2.service.resync_pending_gates().await;

        assert_eq!(
            h2.take_events().await,
            vec![AppEvent::DesignerRequested {
                app_id: a.id.clone(),
                interaction_id: gate.interaction_id.clone(),
                revision: 0,
            }],
            "resync must re-deliver the armed gate to an observer that attached after load"
        );
    }

    /// Finding 13: BOTH gate-opening event pairs are state-change-first —
    /// `WorkflowChanged`, then the gate announcement. Order-sensitive on
    /// purpose.
    #[tokio::test]
    async fn gate_opening_event_pairs_are_state_change_first() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let record = h
            .service
            .create_app("Order", "a test app", None)
            .await
            .unwrap();
        let _ = h.take_events().await;

        let gate = h.service.open_designer(&record.id).await.unwrap();
        assert_eq!(
            h.take_events().await,
            vec![
                AppEvent::WorkflowChanged {
                    app_id: record.id.clone(),
                    state: AppWorkflowState::AwaitingSpecConfirmation,
                    detail: None,
                },
                AppEvent::DesignerRequested {
                    app_id: record.id.clone(),
                    interaction_id: gate.interaction_id.clone(),
                    revision: 0,
                },
            ],
            "open_designer must announce the state change BEFORE the gate"
        );

        h.service
            .confirm_design(&record.id, &gate.interaction_id, 0)
            .await
            .unwrap();
        h.service.generation_complete(&record.id).await.unwrap();
        let _ = h.take_events().await;
        let preview = h.service.validation_passed(&record.id).await.unwrap();
        assert_eq!(
            h.take_events().await,
            vec![
                AppEvent::WorkflowChanged {
                    app_id: record.id.clone(),
                    state: AppWorkflowState::AwaitingPreviewConfirmation,
                    detail: None,
                },
                AppEvent::PreviewReady {
                    app_id: record.id.clone(),
                    interaction_id: preview.interaction_id.clone(),
                    revision: 0,
                    url: None,
                },
            ],
            "validation_passed must announce the state change BEFORE the gate"
        );
    }

    /// Finding 8: a crash while the runtime was busy no longer wedges
    /// `delete_app` — the busy state is reconciled at the next load.
    #[tokio::test]
    async fn delete_app_works_after_a_crash_left_the_runtime_busy() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let record = h
            .service
            .create_app("Busy2", "a test app", None)
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

    /// Finding 14: a mutation only rewrites the documents it changed — a
    /// runtime-only update must succeed even when the design-spec document
    /// cannot be written, and must not rewrite the index. Injection is a
    /// directory squat on `design-spec.json` (finding 13): it blocks that
    /// document for EVERY uid, so the test asserts under root too.
    #[tokio::test]
    async fn runtime_only_mutation_skips_untouched_documents() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let record = h
            .service
            .create_app("Lean", "a test app", None)
            .await
            .unwrap();
        let index_path = dir.path().join("apps/index.json");
        let index_before = std::fs::read_to_string(&index_path).unwrap();

        // Block design-spec.json with a squatting directory: atomic writes
        // onto it become impossible (for any uid).
        let draft_path = dir
            .path()
            .join("apps")
            .join(&record.id)
            .join("workspace/.lingxi/design-spec.json");
        let draft_before = squat_document(&draft_path);

        // The runtime-only mutation no longer touches that file: it must
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

        // Control probe: a mutation that DOES touch design-spec.json fails,
        // proving the squat actually blocks that document.
        let err = h
            .service
            .update_draft(&record.id, 0, &patch("t", "v"))
            .await
            .unwrap_err();
        assert_eq!(err.code(), AppErrorCode::StorageCorrupt, "{err}");

        unsquat_document(&draft_path, &draft_before);

        // The runtime change survives a reload (reconciled per finding 8, the
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

    /// Finding 2 (prefix property): the compensating rollback restores
    /// originals in REVERSE canonical order, so dying right after the
    /// mirror-only restore leaves BY CONSTRUCTION the byte-identical state a
    /// forward mid-batch crash leaves (new design-spec + interactions, old
    /// mirror + index) — a state the existing evidence rules repair without
    /// any new arm: the transition rolls forward, the continuation delivers
    /// exactly once, nothing wedges.
    #[tokio::test]
    async fn rollback_death_after_mirror_restore_is_a_forward_crash_shape() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let record = h
            .service
            .create_app("Hybrid", "a test app", None)
            .await
            .unwrap();
        let gate = h.service.open_designer(&record.id).await.unwrap();
        h.service.flush_events().await;
        drop(h);

        // Write the hybrid directly via storage helpers: the confirm's full
        // batch landed, the index write failed, the reverse rollback
        // restored the MIRROR and then died — design-spec + interactions
        // hold the NEW documents, mirror + index the OLD ones. This is
        // exactly `write_order_prefix_through(Interactions)`, i.e. the
        // forward-crash prefix shape.
        let committed = storage::load_all(dir.path()).unwrap();
        let mut confirmed = committed[0].clone();
        confirmed
            .confirm_design(&gate.interaction_id, 0, 1_700_000_000_100)
            .unwrap();
        storage::save_app_files_steps(
            dir.path(),
            &confirmed,
            storage::write_order_prefix_through(storage::AppDocWriteStep::Interactions),
        )
        .unwrap();

        // The load repairs it exactly like a forward mid-batch crash: the
        // transition rolls FORWARD, the queued continuation is truthful and
        // delivers exactly once, and the workflow continues.
        let h2 = harness(dir.path()).await;
        assert_eq!(
            h2.service.record(&record.id).await.unwrap().workflow_state,
            AppWorkflowState::Generating
        );
        assert!(h2
            .service
            .pending_interaction(&record.id)
            .await
            .unwrap()
            .is_none());
        assert_eq!(h2.service.redeliver_all_undelivered().await.unwrap(), 1);
        let accepted = h2.sink.accepted();
        assert_eq!(accepted.len(), 1);
        assert_eq!(accepted[0].1.kind, AppContinuationKind::DesignConfirmed);
        h2.service.generation_complete(&record.id).await.unwrap();
    }

    /// Finding 2 (deeper rollback death): dying AFTER the mirror and
    /// interactions were already restored (only the design spec still new)
    /// leaves the pre-transition state on disk — the load must come up with
    /// the ORIGINAL gate still armed and NO committed transition; the
    /// leftover new design-spec (a `confirmedRevision` residue) is inert and
    /// is overwritten by the eventual real confirm.
    #[tokio::test]
    async fn rollback_death_before_draft_restore_keeps_the_gate_armed() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let record = h
            .service
            .create_app("Hybrid2", "a test app", None)
            .await
            .unwrap();
        let gate = h.service.open_designer(&record.id).await.unwrap();
        h.service.flush_events().await;
        drop(h);

        let committed = storage::load_all(dir.path()).unwrap();
        let mut confirmed = committed[0].clone();
        confirmed
            .confirm_design(&gate.interaction_id, 0, 1_700_000_000_100)
            .unwrap();
        // Only the FIRST canonical step still holds the new document.
        storage::save_app_files_steps(
            dir.path(),
            &confirmed,
            storage::write_order_prefix_through(storage::AppDocWriteStep::DesignSpec),
        )
        .unwrap();

        let h2 = harness(dir.path()).await;
        assert_eq!(
            h2.service.record(&record.id).await.unwrap().workflow_state,
            AppWorkflowState::AwaitingSpecConfirmation,
            "no committed transition"
        );
        let reloaded = h2.service.interactions(&record.id).await.unwrap();
        assert_eq!(
            reloaded.pending.as_ref().map(|p| p.interaction_id.as_str()),
            Some(gate.interaction_id.as_str()),
            "the original gate is still armed"
        );
        assert!(reloaded.undelivered.is_empty(), "no phantom continuation");
        // The armed gate is satisfiable and completes the flow normally.
        h2.service
            .confirm_design(&record.id, &gate.interaction_id, 0)
            .await
            .unwrap();
        assert_eq!(
            h2.service.record(&record.id).await.unwrap().workflow_state,
            AppWorkflowState::Generating
        );
        assert_eq!(h2.sink.accepted().len(), 1);
    }

    /// Finding 3 Horn A: the POST-delivery persist re-reads disk and merges,
    /// so a continuation another writer landed on disk while a delivery was
    /// in flight survives (and is then delivered) instead of being erased by
    /// a memory-derived rewrite.
    #[tokio::test]
    async fn post_delivery_persist_merges_continuations_landed_mid_delivery() {
        use async_trait::async_trait;
        use std::sync::atomic::{AtomicBool, Ordering};

        /// Sink that, on its FIRST delivery, plants an extra continuation
        /// straight onto disk — modelling a concurrent writer landing
        /// between the pre- and post-delivery persists.
        struct PlantingSink {
            inner: RecordingContinuationSink,
            root: PathBuf,
            planted: AtomicBool,
        }
        #[async_trait]
        impl ContinuationSink for PlantingSink {
            async fn deliver(
                &self,
                app_id: &str,
                continuation: &AppContinuation,
            ) -> Result<(), AppError> {
                if !self.planted.swap(true, Ordering::SeqCst) {
                    let mut disk = storage::load_interactions(&self.root, app_id).unwrap();
                    disk.undelivered.push(AppContinuation {
                        seq: disk.next_seq,
                        app_id: app_id.to_string(),
                        kind: AppContinuationKind::DesignCancelled,
                        payload: serde_json::json!({}),
                        created_at_ms: 77,
                    });
                    disk.next_seq += 1;
                    storage::save_interactions(&self.root, app_id, &disk).unwrap();
                }
                self.inner.deliver(app_id, continuation).await
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let sink = Arc::new(PlantingSink {
            inner: RecordingContinuationSink::new(),
            root: dir.path().to_path_buf(),
            planted: AtomicBool::new(false),
        });
        let observer = Arc::new(RecordingAppEventObserver::new());
        let service = AppService::load(
            dir.path(),
            Arc::new(FixedClock::new(1_700_000_000_000)),
            Arc::clone(&sink) as Arc<dyn ContinuationSink>,
            observer as Arc<dyn AppEventObserver>,
        )
        .await
        .unwrap();
        let record = service
            .create_app("Plant", "a test app", None)
            .await
            .unwrap();
        service.open_designer(&record.id).await.unwrap();
        // cancel_design queues seq 1 and drains: delivering seq 1 plants
        // seq 2 on disk mid-flight; the post-delivery merge must keep it and
        // the drain loop must then deliver it too.
        service.cancel_design(&record.id).await.unwrap();

        let seqs: Vec<u64> = sink.inner.accepted().iter().map(|(_, c)| c.seq).collect();
        assert_eq!(
            seqs,
            vec![1, 2],
            "the mid-delivery disk-only seq must survive and deliver"
        );
        let drained = service.interactions(&record.id).await.unwrap();
        assert!(drained.undelivered.is_empty());
        assert_eq!(drained.last_delivered_seq, 2);
        assert_eq!(drained.next_seq, 3, "the planted mint is adopted");
        // A fresh process agrees — nothing was clobbered on disk.
        drop(service);
        let reloaded = storage::load_all(dir.path()).unwrap();
        assert_eq!(reloaded[0].interactions.last_delivered_seq, 2);
        assert!(reloaded[0].interactions.undelivered.is_empty());
    }

    /// Finding 3 Horn B: a merge that would produce an armed-gate +
    /// queued-consumption hybrid (memory's gate armed, disk's atomic write
    /// cleared it and minted the consumption continuation) resolves through
    /// the SAME evidence rules load-time repair uses: the gate is consumed,
    /// the state follows the evidence, and a re-confirm cannot mint a
    /// duplicate confirmation.
    #[tokio::test]
    async fn merged_armed_gate_with_queued_consumption_resolves_per_the_repair_table() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let record = h
            .service
            .create_app("HornB", "a test app", None)
            .await
            .unwrap();
        let gate = h.service.open_designer(&record.id).await.unwrap();
        h.service.flush_events().await;

        // Residual window on disk: the confirm's interactions write landed
        // (gate cleared + design_confirmed minted) but was rolled back out
        // of memory — memory still holds the armed gate.
        let mut disk = storage::load_interactions(dir.path(), &record.id).unwrap();
        disk.pending = None;
        disk.undelivered.push(AppContinuation {
            seq: disk.next_seq,
            app_id: record.id.clone(),
            kind: AppContinuationKind::DesignConfirmed,
            payload: serde_json::json!({ "revision": 0 }),
            created_at_ms: 88,
        });
        disk.next_seq += 1;
        storage::save_interactions(dir.path(), &record.id, &disk).unwrap();

        // The merge resolves the contradiction: evidence wins, exactly like
        // the load-time repair table (design_confirmed → generating).
        assert_eq!(
            h.service.redeliver_undelivered(&record.id).await.unwrap(),
            1
        );
        let accepted = h.sink.accepted();
        assert_eq!(accepted.len(), 1);
        assert_eq!(accepted[0].1.kind, AppContinuationKind::DesignConfirmed);
        assert!(
            h.service
                .pending_interaction(&record.id)
                .await
                .unwrap()
                .is_none(),
            "the provably-consumed gate must not stay armed"
        );
        assert_eq!(
            h.service.record(&record.id).await.unwrap().workflow_state,
            AppWorkflowState::Generating,
            "the state follows the consumption evidence"
        );
        // No double delivery: the stale gate id cannot re-confirm.
        let err = h
            .service
            .confirm_design(&record.id, &gate.interaction_id, 0)
            .await
            .unwrap_err();
        assert_eq!(err.code(), AppErrorCode::WorkflowStateInvalid);
        assert_eq!(
            h.sink.accepted().len(),
            1,
            "exactly one confirmation ever delivered"
        );
        // The resolution was persisted coherently: a fresh load agrees.
        drop(h);
        let h2 = harness(dir.path()).await;
        assert_eq!(
            h2.service.record(&record.id).await.unwrap().workflow_state,
            AppWorkflowState::Generating
        );
        assert!(h2
            .service
            .pending_interaction(&record.id)
            .await
            .unwrap()
            .is_none());
    }

    /// Finding 4: the undelivered queue is byte-capped (4 MiB serialized) —
    /// legal maximum-size prompts can no longer grow `interactions.json`
    /// toward the 8 MiB document bound where every gate op would wedge as
    /// `invalid_request`; the oldest entries drop instead and the store
    /// stays loadable. (Before the byte cap NOTHING would drop here — the
    /// count cap of 256 is never reached.)
    #[tokio::test]
    async fn legal_max_size_prompts_never_wedge_gate_ops() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        h.sink.set_fail(true); // continuations stay queued
        let record = h
            .service
            .create_app("Big", "a test app", None)
            .await
            .unwrap();
        let gate = h.service.open_designer(&record.id).await.unwrap();
        h.service
            .confirm_design(&record.id, &gate.interaction_id, 0)
            .await
            .unwrap();
        h.service.generation_complete(&record.id).await.unwrap();
        h.service.validation_passed(&record.id).await.unwrap();
        // A legal prompt at the cap whose JSON escaping expands 6× — each
        // queued continuation serializes to ~120 KB, so ~35 of them hit the
        // 4 MiB budget (and ~68 would have crossed the 8 MiB doc bound).
        let prompt = "\u{1}".repeat(MAX_PROMPT_BYTES);
        for _ in 0..45 {
            h.service
                .request_revision(&record.id, &prompt)
                .await
                .unwrap();
            h.service.revision_ready(&record.id).await.unwrap();
            h.service.validation_passed(&record.id).await.unwrap();
        }
        let interactions = h.service.interactions(&record.id).await.unwrap();
        assert!(
            interactions.undelivered.first().map(|c| c.seq) > Some(1),
            "the oldest entries must have been dropped (byte cap engaged): first {:?}",
            interactions.undelivered.first().map(|c| c.seq)
        );
        assert!(
            interactions.undelivered.len() < 45,
            "{} entries retained — the byte cap never engaged",
            interactions.undelivered.len()
        );
        // The persisted document stays comfortably inside the load bound.
        let doc_len = std::fs::metadata(
            dir.path()
                .join("apps")
                .join(&record.id)
                .join("interactions.json"),
        )
        .unwrap()
        .len();
        assert!(doc_len <= storage::MAX_DOC_BYTES, "{doc_len} bytes");
        // And the store reloads cleanly with the capped queue intact.
        drop(h);
        let h2 = harness(dir.path()).await;
        let reloaded = h2.service.interactions(&record.id).await.unwrap();
        assert_eq!(reloaded.undelivered.len(), interactions.undelivered.len());
    }

    /// Finding 8: a stale suggestion (based_on_revision behind the current
    /// draft revision) is rejected with a typed `revision_conflict`,
    /// CONSUMED (persisted), announced via `DesignConflict`, and the user's
    /// newer fields are untouched.
    #[tokio::test]
    async fn stale_suggestion_is_rejected_consumed_and_announced() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let record = h
            .service
            .create_app("Stale", "a test app", None)
            .await
            .unwrap();
        // Suggestion computed against revision 0…
        let suggestion = h
            .service
            .store_suggestion(&record.id, patch("accent", "#old"))
            .await
            .unwrap();
        assert_eq!(suggestion.based_on_revision, 0);
        // …then the user edits on to a much newer revision.
        let mut revision = 0;
        for i in 0..3 {
            revision = h
                .service
                .update_draft(&record.id, revision, &patch("title", &format!("v{i}")))
                .await
                .unwrap();
        }
        assert_eq!(revision, 3);
        let _ = h.take_events().await;

        let err = h
            .service
            .apply_suggestion(&record.id, &suggestion.suggestion_id, revision)
            .await
            .unwrap_err();
        assert!(
            matches!(
                err,
                AppError::RevisionConflict {
                    expected: 0,
                    actual: 3
                }
            ),
            "typed conflict must carry based_on vs current: {err}"
        );
        let events = h.take_events().await;
        assert_eq!(
            events,
            vec![AppEvent::DesignConflict {
                app_id: record.id.clone(),
                expected_revision: 0,
                actual_revision: 3,
            }],
            "the rejection is announced, and no draft change is"
        );
        let draft = h.service.draft(&record.id).await.unwrap();
        assert_eq!(draft.revision, 3, "nothing applied");
        assert_eq!(
            draft.fields.get("title"),
            Some(&DesignValue::ShortText("v2".into())),
            "the user's newer edit survives"
        );
        assert!(
            !draft.fields.contains_key("accent"),
            "the stale patch never landed"
        );
        assert!(
            draft.pending_suggestion.is_none(),
            "the stale suggestion is consumed"
        );
        // Consumption persisted: a fresh load agrees, and a replay fails as
        // interaction_invalid (nothing pending).
        drop(h);
        let h2 = harness(dir.path()).await;
        assert!(h2
            .service
            .draft(&record.id)
            .await
            .unwrap()
            .pending_suggestion
            .is_none());
        let err = h2
            .service
            .apply_suggestion(&record.id, &suggestion.suggestion_id, 3)
            .await
            .unwrap_err();
        assert_eq!(err.code(), AppErrorCode::InteractionInvalid);
    }

    /// Finding 9: two service instances over one root coordinate through the
    /// locked, foreign-preserving index transactions — each instance's
    /// creates survive the other's writes, and a delete stays authoritative
    /// without resurrecting or touching the other instance's app.
    #[tokio::test]
    async fn two_service_instances_preserve_each_others_index_entries() {
        let dir = tempfile::tempdir().unwrap();
        let a = harness(dir.path()).await;
        let b = harness(dir.path()).await; // loaded before app1 exists
        let app1 = a
            .service
            .create_app("From A", "a test app", None)
            .await
            .unwrap();
        // B has never seen app1; its index write must preserve it.
        let app2 = b
            .service
            .create_app("From B", "a test app", None)
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
            .create_app("A again", "a test app", None)
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

    /// Finding 11: an observer calling back into an emitting path PANICS
    /// with a clear message (caught here inside the observer) instead of
    /// silently deadlocking the emission queue forever.
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
                Arc::new(RecordingContinuationSink::new()) as Arc<dyn ContinuationSink>,
                Arc::clone(&observer) as Arc<dyn AppEventObserver>,
            )
            .await
            .unwrap(),
        );
        observer.service.set(Arc::clone(&service)).ok().unwrap();

        // The create's emission triggers the observer's reentrant call.
        service
            .create_app("Reenter", "a test app", None)
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

    /// Finding 11: a caller future dropped around a mutation can no longer
    /// lose the mutation's events — whenever the mutation committed, its
    /// FULL event sequence is delivered; when it did not commit, nothing is.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dropped_caller_never_loses_a_committed_mutations_events() {
        let dir = tempfile::tempdir().unwrap();
        let h = harness(dir.path()).await;
        let record = h
            .service
            .create_app("Drop", "a test app", None)
            .await
            .unwrap();
        let _ = h.take_events().await;
        let observer = Arc::clone(&h.observer);
        let service = Arc::new(h.service);
        for i in 0..20u32 {
            let call = {
                let service = Arc::clone(&service);
                let app_id = record.id.clone();
                tokio::spawn(async move { service.open_designer(&app_id).await })
            };
            if i % 2 == 0 {
                tokio::task::yield_now().await; // let some iterations commit
            }
            call.abort();
            let _ = call.await;
            service.flush_events().await;
            let committed = service.record(&record.id).await.unwrap().workflow_state
                == AppWorkflowState::AwaitingSpecConfirmation;
            let events = observer.take();
            if committed {
                assert!(
                    events.iter().any(|event| matches!(
                        event,
                        AppEvent::WorkflowChanged {
                            state: AppWorkflowState::AwaitingSpecConfirmation,
                            ..
                        }
                    )) && events
                        .iter()
                        .any(|event| matches!(event, AppEvent::DesignerRequested { .. })),
                    "iteration {i}: committed mutation lost part of its event \
                     sequence: {events:?}"
                );
                service.cancel_design(&record.id).await.unwrap();
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

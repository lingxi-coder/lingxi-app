//! Desktop-owned durable session coordinator.
//!
//! This module is intentionally additive to the existing composition root.
//! It owns the bounded cost queue and delegates authoritative ordering/replay
//! to [`session::jsonl::DurableJournal`].  The engine root wires one instance
//! per canonical session after it has acquired the process writer lease;
//! scoped cost views retain the same Arc authority rather than reacquiring a
//! lock.

use async_trait::async_trait;
use cost::{
    CostDurabilityGate, CostHydration, CostHydrator, CostMutationId, CostMutationRecord,
    CostMutationSource, CostPersistAck, CostPersistError, CostPersistPermit, CostPersistRequest,
    CostPersistResult, CostPersistence, CostState, CostStateVector,
};
use protocol::SessionId;
use session::jsonl::{DurableJournal, JournalError};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, Mutex as AsyncMutex};

const COST_QUEUE_CAPACITY: usize = 64;

#[derive(Debug, Clone, Default)]
struct CoordinatorProjection {
    latest: Option<(CostState, u64)>,
}

#[derive(Debug, Clone)]
struct CachedCostResult {
    /// Pre-WAL failures retain the submitted record for exact retry matching.
    /// Durable successes keep only an ack and reread the one bounded WAL
    /// record on a duplicate, avoiding one cumulative vector per mutation.
    record: Option<CostMutationRecord>,
    result: CostPersistResult,
}

struct HydratedCostLedger {
    hydration: CostHydration,
    durable_results: std::collections::HashMap<CostMutationId, CachedCostResult>,
}

/// One canonical session's durable ledger owner.
pub struct SessionStateCoordinator {
    state: Arc<CoordinatorState>,
    queue_tx: mpsc::Sender<CostPersistRequest>,
    queue_rx: AsyncMutex<Option<mpsc::Receiver<CostPersistRequest>>>,
}

/// State retained by the writer while it drains accepted requests. It
/// intentionally owns no queue sender: when the last external/session owner
/// drops, the channel closes, the receiver drains, and the writer lease can be
/// released instead of being pinned by a self-owned sender cycle.
struct CoordinatorState {
    session_id: SessionId,
    journal: Arc<DurableJournal>,
    projection: Mutex<CoordinatorProjection>,
    /// Stable mutation outcomes retained after ack receivers are dropped.
    results: Mutex<std::collections::HashMap<CostMutationId, CachedCostResult>>,
    writer_lease: platform_api::live_sessions::SharedSessionWriterLease,
    durability_gate: CostDurabilityGate,
}

impl std::fmt::Debug for SessionStateCoordinator {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SessionStateCoordinator")
            .field("session_id", &self.state.session_id)
            .field("journal_root", &self.state.journal.root())
            .finish_non_exhaustive()
    }
}

impl SessionStateCoordinator {
    /// Open the stable `<lingxi_home>/session-state/<uuid>` directory.
    pub fn open(
        lingxi_home: impl AsRef<Path>,
        session_id: SessionId,
        writer_lease: platform_api::live_sessions::SharedSessionWriterLease,
    ) -> Result<Arc<Self>, CostPersistError> {
        if writer_lease.canonical_session_id() != Some(session_id) {
            return Err(CostPersistError::Rejected(
                "session-state writer claim does not match canonical session".into(),
            ));
        }
        let state_relative = PathBuf::from("session-state").join(session_id.as_uuid().to_string());
        let journal = DurableJournal::open_under(lingxi_home.as_ref(), &state_relative)
            .map_err(map_journal_error)?;
        let (queue_tx, queue_rx) = mpsc::channel(COST_QUEUE_CAPACITY);
        Ok(Arc::new(Self {
            state: Arc::new(CoordinatorState {
                session_id,
                journal: Arc::new(journal),
                projection: Mutex::new(CoordinatorProjection::default()),
                results: Mutex::new(std::collections::HashMap::new()),
                writer_lease,
                durability_gate: CostDurabilityGate::default(),
            }),
            queue_tx,
            queue_rx: AsyncMutex::new(Some(queue_rx)),
        }))
    }

    /// Keep the writer claim alive while returning a scoped clone.
    #[must_use]
    pub fn writer_lease(&self) -> platform_api::live_sessions::SharedSessionWriterLease {
        self.state.writer_lease.clone()
    }

    /// Per-session latch shared by ordinary and Fusion paid prechecks.
    #[must_use]
    pub fn durability_gate(&self) -> CostDurabilityGate {
        self.state.durability_gate.clone()
    }

    /// Shared durable journal handle used by terminal/outbox composition.
    #[must_use]
    pub fn journal(&self) -> Arc<DurableJournal> {
        self.state.journal.clone()
    }

    /// Start the single writer.  Filesystem work is moved to a blocking
    /// worker; requests remain serial even when a prior fsync is slow.
    pub async fn start(self: &Arc<Self>) -> Result<tokio::task::JoinHandle<()>, CostPersistError> {
        let mut receiver = self
            .queue_rx
            .lock()
            .await
            .take()
            .ok_or_else(|| CostPersistError::Rejected("coordinator already started".into()))?;
        let state = self.state.clone();
        let hydration = tokio::task::spawn_blocking({
            let state = state.clone();
            move || state.hydrate_blocking()
        })
        .await
        .map_err(|error| CostPersistError::Storage(error.to_string()))??;
        state
            .projection
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .latest = Some((hydration.state, hydration.journal_revision));
        Ok(tokio::spawn(async move {
            while let Some(request) = receiver.recv().await {
                let worker = state.clone();
                let mutation_id = request.mutation_id.clone();
                let record = CostMutationRecord {
                    cost_revision: request.cost_revision,
                    mutation_id: request.mutation_id.clone(),
                    source: request.source,
                    state: request.state.clone(),
                };
                let result =
                    tokio::task::spawn_blocking(move || worker.persist_request(request)).await;
                if let Err(join_error) = result {
                    let error = CostPersistError::Storage(format!(
                        "session state persistence worker failed: {join_error}"
                    ));
                    state.durability_gate.freeze(error.to_string());
                    state
                        .results
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .entry(mutation_id)
                        .or_insert(CachedCostResult {
                            record: Some(record),
                            result: Err(error),
                        });
                    tracing::error!("session state persistence worker failed: {join_error}");
                }
            }
        }))
    }

    /// Hydrate the complete folded cost state before active-session publish.
    /// Recovery never invokes an executor or provider; it only folds durable
    /// vectors already present in the journal.
    pub fn hydrate_blocking(&self) -> Result<CostHydration, CostPersistError> {
        self.state.hydrate_blocking()
    }

    /// Return the current freeze-safe projection for composition roots that
    /// need to seed a tracker synchronously after boot.
    pub fn projection(&self) -> Option<(CostState, u64)> {
        self.state
            .projection
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .latest
            .clone()
    }

    /// Fold the legacy `lastCost` import decision and opening balance into one
    /// durable mutation.  `None` is meaningful: it records that matching was
    /// evaluated and found no opening balance, preventing a later shadow edit
    /// from importing the same value.
    pub async fn import_legacy_opening_balance(
        &self,
        opening_nano_usd: Option<u64>,
    ) -> Result<CostPersistAck, CostPersistError> {
        let permit = <Self as CostPersistence>::acquire_permit(self, self.state.session_id).await?;
        let ack_rx = {
            let projection = self
                .state
                .projection
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let (current, journal_revision) = projection.latest.as_ref().ok_or_else(|| {
                CostPersistError::Rejected(
                    "legacy import requires a started, hydrated coordinator".into(),
                )
            })?;
            if current.legacy_import_evaluated {
                return Err(CostPersistError::Rejected(
                    "legacy opening balance was already evaluated".into(),
                ));
            }
            let mut staged = current.clone();
            staged.legacy_import_evaluated = true;
            staged.cost_revision = staged
                .cost_revision
                .checked_add(1)
                .ok_or_else(|| CostPersistError::Storage("cost revision overflow".into()))?;
            // A pre-existing V1 WAL is already authoritative, including old
            // V1 prefixes written before the evaluated marker existed. Never
            // merge a later mutable legacy shadow into that ledger.
            let imported_opening = if *journal_revision == 0 {
                if let Some(opening) = opening_nano_usd {
                    staged.total_nano_usd = staged.total_nano_usd.saturating_add(opening);
                    staged.legacy_opening_balance_nano_usd = staged
                        .legacy_opening_balance_nano_usd
                        .saturating_add(opening);
                    true
                } else {
                    false
                }
            } else {
                false
            };
            let mutation_id = CostMutationId::new(format!(
                "{}:v1:{}",
                if imported_opening {
                    "legacy-opening-balance"
                } else {
                    "legacy-import-evaluated"
                },
                self.state.session_id
            ));
            let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
            permit.enqueue(CostPersistRequest {
                session_id: self.state.session_id,
                cost_revision: staged.cost_revision,
                mutation_id,
                state: CostStateVector::from(&staged),
                source: if imported_opening {
                    CostMutationSource::LegacyOpeningBalance
                } else {
                    CostMutationSource::LegacyImportEvaluated
                },
                ack: ack_tx,
            })?;
            ack_rx
        };
        ack_rx.await.map_err(|_| {
            CostPersistError::Storage("legacy opening balance acknowledgment dropped".into())
        })?
    }
}

impl CoordinatorState {
    fn persist_request(&self, request: CostPersistRequest) {
        let mutation_id = request.mutation_id.clone();
        let record = CostMutationRecord {
            cost_revision: request.cost_revision,
            mutation_id: request.mutation_id.clone(),
            source: request.source,
            state: request.state.clone(),
        };
        let cached = self
            .results
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&mutation_id)
            .cloned();
        let outcome = if let Some(cached) = cached {
            self.resolve_cached_result(&cached, &record)
        } else {
            let outcome = self.persist_request_inner(&request, &record);
            let retained_record = outcome.is_err().then_some(record);
            self.results
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(
                    mutation_id.clone(),
                    CachedCostResult {
                        record: retained_record,
                        result: outcome.clone(),
                    },
                );
            outcome
        };
        if let Err(error) = &outcome {
            self.durability_gate.freeze(error.to_string());
            tracing::error!("durable cost mutation {mutation_id:?} failed: {error}");
        }
        // The ack channel is intentionally best-effort.  The journal has
        // already retained the stable mutation id and replay can recover the
        // result even when the original caller was cancelled.
        let _ = request.ack.send(outcome);
    }

    fn resolve_cached_result(
        &self,
        cached: &CachedCostResult,
        submitted: &CostMutationRecord,
    ) -> CostPersistResult {
        if let Some(previous) = cached.record.as_ref() {
            return if previous == submitted {
                cached.result.clone()
            } else {
                Err(mutation_conflict(&submitted.mutation_id))
            };
        }
        let Some(replayed) = self.replayed_result(submitted)? else {
            return Err(CostPersistError::Storage(format!(
                "durable cost mutation disappeared: {}",
                submitted.mutation_id.as_str()
            )));
        };
        if cached.result == Ok(replayed.clone()) {
            Ok(replayed)
        } else {
            Err(CostPersistError::Storage(format!(
                "cached cost acknowledgment disagrees with WAL: {}",
                submitted.mutation_id.as_str()
            )))
        }
    }

    fn replayed_result(
        &self,
        submitted: &CostMutationRecord,
    ) -> Result<Option<CostPersistAck>, CostPersistError> {
        let Some(entry) = self
            .journal
            .find_event_durable(submitted.mutation_id.as_str())
            .map_err(map_journal_error)?
        else {
            return Ok(None);
        };
        let persisted: CostMutationRecord = serde_json::from_value(entry.event)
            .map_err(|error| CostPersistError::Storage(error.to_string()))?;
        if persisted.mutation_id.as_str() != entry.event_id || persisted != *submitted {
            return Err(mutation_conflict(&submitted.mutation_id));
        }
        Ok(Some(CostPersistAck {
            mutation_id: submitted.mutation_id.clone(),
            journal_revision: entry.journal_revision,
            cost_revision: submitted.cost_revision,
        }))
    }

    fn persist_request_inner(
        &self,
        request: &CostPersistRequest,
        record: &CostMutationRecord,
    ) -> CostPersistResult {
        if request.session_id != self.session_id {
            return Err(CostPersistError::Rejected(
                "cost mutation belongs to a different session".into(),
            ));
        }
        let validated_state = match request.state.clone().try_into_state() {
            Ok(state) => state,
            Err(error) => return Err(error),
        };
        if request.state.session_id != request.session_id
            || validated_state.session_id != self.session_id
        {
            return Err(CostPersistError::Rejected(
                "cost request/vector/coordinator session identity mismatch".into(),
            ));
        }
        if request.cost_revision != request.state.cost_revision
            || record.cost_revision != request.cost_revision
        {
            return Err(CostPersistError::Storage(
                "cost request/vector revision mismatch".into(),
            ));
        }
        let previous_revision = self
            .projection
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .latest
            .as_ref()
            .map_or(0, |(previous, _)| previous.cost_revision);
        let Some(expected_revision) = previous_revision.checked_add(1) else {
            return Err(CostPersistError::Storage("cost revision overflow".into()));
        };
        if request.cost_revision != expected_revision {
            if let Some(ack) = self.replayed_result(record)? {
                return Ok(ack);
            }
            return Err(CostPersistError::Storage(
                "cost mutation revision is not the next durable revision".into(),
            ));
        }
        let append = self
            .journal
            .append_once(request.mutation_id.as_str(), record)
            .map_err(map_journal_error);
        match append {
            Ok(append) => {
                {
                    let mut projection = self
                        .projection
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    projection.latest = Some((validated_state.clone(), append.journal_revision));
                }
                // Snapshot failure is derivative.  The WAL append has
                // already crossed the durable acknowledgement boundary.
                if let Err(error) = self
                    .journal
                    .write_snapshot(append.journal_revision, &request.state)
                {
                    tracing::warn!("cost snapshot rebuild deferred: {error}");
                }
                Ok(CostPersistAck {
                    mutation_id: request.mutation_id.clone(),
                    journal_revision: append.journal_revision,
                    cost_revision: request.cost_revision,
                })
            }
            Err(error) => Err(error),
        }
    }

    fn hydrate_blocking(&self) -> Result<CostHydration, CostPersistError> {
        let hydrated = hydrate_from_journal(&self.journal, self.session_id)?;
        let mut results = self
            .results
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for (mutation_id, durable) in hydrated.durable_results {
            if let Some(existing) = results.get(&mutation_id) {
                if existing.record.is_some() || existing.result != durable.result {
                    return Err(CostPersistError::Storage(format!(
                        "cost mutation id conflict while hydrating: {}",
                        mutation_id.as_str()
                    )));
                }
            } else {
                results.insert(mutation_id, durable);
            }
        }
        Ok(hydrated.hydration)
    }
}

fn mutation_conflict(mutation_id: &CostMutationId) -> CostPersistError {
    CostPersistError::Storage(format!(
        "cost mutation id conflict: {}",
        mutation_id.as_str()
    ))
}

fn hydrate_from_journal(
    journal: &DurableJournal,
    session_id: SessionId,
) -> Result<HydratedCostLedger, CostPersistError> {
    let mut latest = CostState {
        session_id,
        ..Default::default()
    };
    let mut last_cost_revision = 0_u64;
    let mut durable_results = std::collections::HashMap::new();
    let mut fold_error = None;
    let replay = journal
        .replay_durable_with(|entry| {
            // Keep validating the physical WAL after a cost semantic error so
            // replay never publishes a prefix merely because its fold failed.
            if fold_error.is_none() {
                if let Err(error) = fold_cost_entry(
                    entry,
                    session_id,
                    &mut last_cost_revision,
                    &mut latest,
                    &mut durable_results,
                ) {
                    fold_error = Some(error);
                }
            }
        })
        .map_err(map_journal_error)?;
    if let Some(error) = fold_error {
        return Err(error);
    }
    if !replay.journal_present {
        return match journal.read_snapshot::<CostStateVector>() {
            Ok(None) => Ok(HydratedCostLedger {
                hydration: CostHydration {
                    state: CostState {
                        session_id,
                        ..Default::default()
                    },
                    journal_revision: 0,
                },
                durable_results: std::collections::HashMap::new(),
            }),
            Ok(Some(_)) | Err(_) => Err(CostPersistError::Storage(
                "cost snapshot exists without its authoritative WAL".into(),
            )),
        };
    }
    // A valid WAL is authoritative. A missing, corrupt, behind, ahead, or
    // prefix-mismatched derivative snapshot is rebuilt without weakening the
    // successful replay result.
    if let Err(error) =
        journal.write_snapshot(replay.last_revision, &CostStateVector::from(&latest))
    {
        tracing::warn!("cost snapshot rebuild deferred: {error}");
    }
    Ok(HydratedCostLedger {
        hydration: CostHydration {
            state: latest,
            journal_revision: replay.last_revision,
        },
        durable_results,
    })
}

fn fold_cost_entry(
    entry: session::jsonl::JournalEntry,
    session_id: SessionId,
    last_cost_revision: &mut u64,
    latest: &mut CostState,
    durable_results: &mut std::collections::HashMap<CostMutationId, CachedCostResult>,
) -> Result<(), CostPersistError> {
    let event: CostMutationRecord = serde_json::from_value(entry.event)
        .map_err(|error| CostPersistError::Storage(error.to_string()))?;
    if event.mutation_id.as_str() != entry.event_id {
        return Err(CostPersistError::Storage(
            "cost journal event id disagrees with mutation id".into(),
        ));
    }
    if event.state.session_id != session_id {
        return Err(CostPersistError::Storage(
            "cost journal contains a foreign session vector".into(),
        ));
    }
    if event.cost_revision != event.state.cost_revision {
        return Err(CostPersistError::Storage(
            "cost journal outer and vector revisions disagree".into(),
        ));
    }
    let expected_revision = last_cost_revision
        .checked_add(1)
        .ok_or_else(|| CostPersistError::Storage("cost revision overflow".into()))?;
    if event.cost_revision != expected_revision {
        return Err(CostPersistError::Storage(
            "cost journal cost revisions are not contiguous".into(),
        ));
    }
    let folded = event
        .state
        .clone()
        .try_into_state()
        .map_err(|error| CostPersistError::Storage(error.to_string()))?;
    let mutation_id = event.mutation_id.clone();
    let ack = CostPersistAck {
        mutation_id: mutation_id.clone(),
        journal_revision: entry.journal_revision,
        cost_revision: event.cost_revision,
    };
    *last_cost_revision = event.cost_revision;
    *latest = folded;
    durable_results.insert(
        mutation_id,
        CachedCostResult {
            record: None,
            result: Ok(ack),
        },
    );
    Ok(())
}

#[async_trait]
impl CostPersistence for SessionStateCoordinator {
    async fn acquire_permit(
        &self,
        session_id: SessionId,
    ) -> Result<CostPersistPermit, CostPersistError> {
        if session_id != self.state.session_id {
            return Err(CostPersistError::Rejected(
                "cost permit belongs to a different session".into(),
            ));
        }
        if let Some(reason) = self.state.durability_gate.frozen_reason() {
            return Err(CostPersistError::Frozen(reason));
        }
        let permit = self
            .queue_tx
            .clone()
            .reserve_owned()
            .await
            .map_err(|_| CostPersistError::Rejected("session state queue is closed".into()))?;
        if let Some(reason) = self.state.durability_gate.frozen_reason() {
            return Err(CostPersistError::Frozen(reason));
        }
        let gate = self.state.durability_gate.clone();
        Ok(CostPersistPermit::new(move |request| {
            if let Some(reason) = gate.frozen_reason() {
                return Err(CostPersistError::Frozen(reason));
            }
            permit.send(request);
            Ok(())
        }))
    }
}

#[async_trait]
impl CostHydrator for SessionStateCoordinator {
    async fn hydrate(&self, session_id: SessionId) -> Result<CostHydration, CostPersistError> {
        if session_id != self.state.session_id {
            return Err(CostPersistError::Rejected(
                "hydration belongs to a different session".into(),
            ));
        }
        let state = self.state.clone();
        tokio::task::spawn_blocking(move || state.hydrate_blocking())
            .await
            .map_err(|error| CostPersistError::Storage(error.to_string()))?
    }
}

fn map_journal_error(error: JournalError) -> CostPersistError {
    CostPersistError::Storage(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use platform_api::live_sessions::{SessionWriterLease, SharedSessionWriterLease};

    struct TestLease(String);

    impl SessionWriterLease for TestLease {
        fn session_id(&self) -> &str {
            &self.0
        }
    }

    fn lease(session_id: SessionId) -> SharedSessionWriterLease {
        Arc::new(TestLease(session_id.to_string()))
    }

    fn coordinator() -> (tempfile::TempDir, Arc<SessionStateCoordinator>, SessionId) {
        let directory = tempfile::tempdir().unwrap();
        let session_id = SessionId::new();
        let coordinator =
            SessionStateCoordinator::open(directory.path(), session_id, lease(session_id)).unwrap();
        (directory, coordinator, session_id)
    }

    fn state(session_id: SessionId, revision: u64, total: u64) -> CostState {
        CostState {
            session_id,
            cost_revision: revision,
            total_nano_usd: total,
            ..Default::default()
        }
    }

    #[test]
    fn constructor_rejects_a_foreign_writer_claim() {
        let directory = tempfile::tempdir().unwrap();
        let session_id = SessionId::new();
        let other = SessionId::new();

        assert!(matches!(
            SessionStateCoordinator::open(directory.path(), session_id, lease(other)),
            Err(CostPersistError::Rejected(message)) if message.contains("claim")
        ));
    }

    #[test]
    fn snapshot_without_authoritative_wal_fails_closed() {
        let (_directory, coordinator, session_id) = coordinator();
        coordinator
            .journal()
            .write_snapshot(1, &CostStateVector::from(&state(session_id, 1, 99)))
            .unwrap();

        assert!(matches!(
            coordinator.hydrate_blocking(),
            Err(CostPersistError::Storage(message)) if message.contains("without its authoritative WAL")
        ));
    }

    #[test]
    fn valid_wal_rebuilds_a_corrupt_snapshot() {
        let (_directory, coordinator, session_id) = coordinator();
        let state = state(session_id, 1, 99);
        let mutation_id = CostMutationId::new("cost-1");
        let record = CostMutationRecord {
            cost_revision: 1,
            mutation_id: mutation_id.clone(),
            source: CostMutationSource::ModelResponse,
            state: CostStateVector::from(&state),
        };
        coordinator
            .journal()
            .append_once(mutation_id.as_str(), &record)
            .unwrap();
        std::fs::write(
            coordinator
                .journal()
                .root()
                .join(session::jsonl::SNAPSHOT_FILE_NAME),
            b"not-json",
        )
        .unwrap();

        let hydration = coordinator.hydrate_blocking().unwrap();

        assert_eq!(hydration.state, state);
        let repaired = coordinator
            .journal()
            .read_snapshot::<CostStateVector>()
            .unwrap()
            .unwrap();
        assert_eq!(repaired.last_journal_revision, 1);
        assert_eq!(repaired.state, CostStateVector::from(&state));
    }

    #[test]
    fn replay_rejects_outer_and_vector_revision_mismatch() {
        let (_directory, coordinator, session_id) = coordinator();
        let mutation_id = CostMutationId::new("cost-1");
        let record = CostMutationRecord {
            cost_revision: 1,
            mutation_id: mutation_id.clone(),
            source: CostMutationSource::ModelResponse,
            state: CostStateVector::from(&state(session_id, 2, 99)),
        };
        coordinator
            .journal()
            .append_once(mutation_id.as_str(), &record)
            .unwrap();

        assert!(matches!(
            coordinator.hydrate_blocking(),
            Err(CostPersistError::Storage(message)) if message.contains("revisions disagree")
        ));
    }

    #[tokio::test]
    async fn duplicate_mutation_id_compares_content_before_reusing_result() {
        let (_directory, coordinator, session_id) = coordinator();
        let mutation_id = CostMutationId::new("cost-1");
        let (first_ack, first_rx) = tokio::sync::oneshot::channel();
        coordinator.state.persist_request(CostPersistRequest {
            session_id,
            cost_revision: 1,
            mutation_id: mutation_id.clone(),
            state: CostStateVector::from(&state(session_id, 1, 10)),
            source: CostMutationSource::ModelResponse,
            ack: first_ack,
        });
        assert!(first_rx.await.unwrap().is_ok());

        let (conflict_ack, conflict_rx) = tokio::sync::oneshot::channel();
        coordinator.state.persist_request(CostPersistRequest {
            session_id,
            cost_revision: 1,
            mutation_id,
            state: CostStateVector::from(&state(session_id, 1, 11)),
            source: CostMutationSource::ModelResponse,
            ack: conflict_ack,
        });

        assert!(matches!(
            conflict_rx.await.unwrap(),
            Err(CostPersistError::Storage(message)) if message.contains("conflict")
        ));
        assert_eq!(coordinator.journal().replay().unwrap().entries.len(), 1);
        assert!(coordinator.durability_gate().frozen_reason().is_some());
    }

    #[tokio::test]
    async fn replayed_mutation_returns_its_original_ack_before_next_revision_validation() {
        let (_directory, coordinator, session_id) = coordinator();
        let mutation_id = CostMutationId::new("replayed-cost-1");
        let persisted = CostMutationRecord {
            cost_revision: 1,
            mutation_id: mutation_id.clone(),
            source: CostMutationSource::ModelResponse,
            state: CostStateVector::from(&state(session_id, 1, 10)),
        };
        coordinator
            .journal()
            .append_once(mutation_id.as_str(), &persisted)
            .unwrap();
        let writer = coordinator.start().await.unwrap();
        assert!(
            coordinator
                .state
                .results
                .lock()
                .unwrap()
                .get(&mutation_id)
                .expect("hydration seeds a stable duplicate result")
                .record
                .is_none(),
            "successful replay cache must not retain a cumulative cost vector"
        );

        let permit = coordinator.acquire_permit(session_id).await.unwrap();
        let (same_ack, same_rx) = tokio::sync::oneshot::channel();
        permit
            .enqueue(CostPersistRequest {
                session_id,
                cost_revision: 1,
                mutation_id: mutation_id.clone(),
                state: persisted.state.clone(),
                source: persisted.source,
                ack: same_ack,
            })
            .unwrap();
        let ack = same_rx.await.unwrap().unwrap();
        assert_eq!(ack.mutation_id, mutation_id);
        assert_eq!(ack.journal_revision, 1);
        assert_eq!(ack.cost_revision, 1);

        // The replay cache compares the complete durable record, not only its
        // id/revision. A changed source or vector is a conflict and freezes.
        let permit = coordinator.acquire_permit(session_id).await.unwrap();
        let (conflict_ack, conflict_rx) = tokio::sync::oneshot::channel();
        permit
            .enqueue(CostPersistRequest {
                session_id,
                cost_revision: 1,
                mutation_id,
                state: CostStateVector::from(&state(session_id, 1, 11)),
                source: CostMutationSource::Administrative,
                ack: conflict_ack,
            })
            .unwrap();
        assert!(matches!(
            conflict_rx.await.unwrap(),
            Err(CostPersistError::Storage(message)) if message.contains("conflict")
        ));
        assert_eq!(coordinator.journal().replay().unwrap().entries.len(), 1);
        assert!(coordinator.durability_gate().frozen_reason().is_some());

        drop(coordinator);
        writer.await.unwrap();
    }

    #[tokio::test]
    async fn final_owner_drop_closes_the_queue_after_draining_accepted_mutations() {
        let directory = tempfile::tempdir().unwrap();
        let session_id = SessionId::new();
        let concrete_lease = Arc::new(TestLease(session_id.to_string()));
        let weak_lease = Arc::downgrade(&concrete_lease);
        let shared_lease: SharedSessionWriterLease = concrete_lease.clone();
        let coordinator =
            SessionStateCoordinator::open(directory.path(), session_id, shared_lease).unwrap();
        drop(concrete_lease);
        let writer = coordinator.start().await.unwrap();
        let permit = coordinator.acquire_permit(session_id).await.unwrap();
        let mutation_id = CostMutationId::new("accepted-before-owner-drop");
        let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
        permit
            .enqueue(CostPersistRequest {
                session_id,
                cost_revision: 1,
                mutation_id: mutation_id.clone(),
                state: CostStateVector::from(&state(session_id, 1, 41)),
                source: CostMutationSource::ModelResponse,
                ack: ack_tx,
            })
            .unwrap();

        // No worker-owned sender remains. Dropping the final external owner
        // closes the channel, but the accepted request is drained first.
        drop(coordinator);
        let ack = ack_rx.await.unwrap().unwrap();
        assert_eq!(ack.mutation_id, mutation_id);
        writer.await.unwrap();
        assert!(
            weak_lease.upgrade().is_none(),
            "writer claim is released after the closed queue is drained"
        );

        let relative = PathBuf::from("session-state").join(session_id.as_uuid().to_string());
        let journal = DurableJournal::open_under(directory.path(), &relative).unwrap();
        assert_eq!(journal.replay().unwrap().entries.len(), 1);
    }

    #[tokio::test]
    async fn first_boot_import_keeps_totals_only_legacy_separate_from_new_aggregates() {
        let (_directory, coordinator, session_id) = coordinator();
        let writer = coordinator.start().await.unwrap();

        coordinator
            .import_legacy_opening_balance(Some(75))
            .await
            .unwrap();
        let hydration = coordinator.hydrate(session_id).await.unwrap();
        assert_eq!(hydration.state.total_nano_usd, 75);
        assert_eq!(hydration.state.legacy_opening_balance_nano_usd, 75);
        assert_eq!(hydration.state.external_nano_usd, 0);
        assert!(hydration.state.legacy_import_evaluated);
        let replay = coordinator.journal().replay().unwrap();
        let event: CostMutationRecord =
            serde_json::from_value(replay.entries.last().unwrap().event.clone()).unwrap();
        assert_eq!(event.source, CostMutationSource::LegacyOpeningBalance);

        drop(coordinator);
        writer.await.unwrap();
    }

    #[tokio::test]
    async fn an_existing_v1_wal_records_evaluation_without_importing_a_legacy_shadow() {
        let (_directory, coordinator, session_id) = coordinator();
        let initial = state(session_id, 1, 30);
        let mutation_id = CostMutationId::new("existing-v1");
        coordinator
            .journal()
            .append_once(
                mutation_id.as_str(),
                &CostMutationRecord {
                    cost_revision: 1,
                    mutation_id: mutation_id.clone(),
                    source: CostMutationSource::ModelResponse,
                    state: CostStateVector::from(&initial),
                },
            )
            .unwrap();
        let writer = coordinator.start().await.unwrap();

        coordinator
            .import_legacy_opening_balance(Some(30))
            .await
            .unwrap();
        let hydration = coordinator.hydrate(session_id).await.unwrap();
        assert_eq!(hydration.state.total_nano_usd, 30);
        assert_eq!(hydration.state.legacy_opening_balance_nano_usd, 0);
        assert_eq!(hydration.state.external_nano_usd, 0);
        assert!(hydration.state.legacy_import_evaluated);
        assert_eq!(hydration.state.cost_revision, 2);
        let replay = coordinator.journal().replay().unwrap();
        let event: CostMutationRecord =
            serde_json::from_value(replay.entries.last().unwrap().event.clone()).unwrap();
        assert_eq!(event.source, CostMutationSource::LegacyImportEvaluated);

        drop(coordinator);
        writer.await.unwrap();
    }

    #[tokio::test]
    async fn no_match_is_durable_and_a_later_legacy_shadow_cannot_be_imported() {
        let (_directory, coordinator, session_id) = coordinator();
        let writer = coordinator.start().await.unwrap();

        coordinator
            .import_legacy_opening_balance(None)
            .await
            .unwrap();
        assert!(matches!(
            coordinator.import_legacy_opening_balance(Some(99)).await,
            Err(CostPersistError::Rejected(message)) if message.contains("already evaluated")
        ));
        let hydration = coordinator.hydrate(session_id).await.unwrap();
        assert_eq!(hydration.state.total_nano_usd, 0);
        assert_eq!(hydration.state.legacy_opening_balance_nano_usd, 0);
        assert!(hydration.state.legacy_import_evaluated);

        drop(coordinator);
        writer.await.unwrap();
    }
}

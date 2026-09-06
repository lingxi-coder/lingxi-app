//! Durable cost-state contracts shared by the cost ledger and app-tier
//! session coordinator.
//!
//! This module deliberately contains no filesystem or engine dependency. The
//! coordinator owns WAL/snapshot IO and implements [`CostPersistence`]; the
//! cost crate owns mutation identity, vector DTO conversion, queue ordering,
//! and the per-session durability/freeze latch.

use crate::tracker::{CostState, ModelUsage};
use crate::ModelRef;
use async_trait::async_trait;
use protocol::SessionId;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::oneshot;

/// Stable identity retained across queue retries and durable acknowledgements.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CostMutationId(String);

impl CostMutationId {
    /// Construct an id supplied by the owner of the mutation.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Borrow the stable wire value.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Why a cost vector changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostMutationSource {
    /// A normal model response.
    ModelResponse,
    /// A Fusion aggregate/legacy settlement retained until PR06.
    FusionAggregate,
    /// The one-time legacy matching decision when no balance is imported.
    LegacyImportEvaluated,
    /// The one-time legacy opening balance import.
    LegacyOpeningBalance,
    /// A reset or line/tool counter mutation.
    Administrative,
}

/// JSON-safe cost state. `per_model_usage` is a vector specifically so
/// structured provider/model identities round-trip without map-key coercion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostStateVector {
    /// Owning session.
    pub session_id: SessionId,
    /// Monotonic cost mutation revision.
    pub cost_revision: u64,
    /// Cumulative total.
    pub total_nano_usd: u64,
    /// Per-model vector.
    pub per_model_usage: Vec<ModelUsage>,
    /// Aggregate timing/counters.
    pub total_api_duration_ms: u64,
    /// Aggregate non-retry timing.
    pub total_api_duration_without_retries_ms: u64,
    /// Aggregate client tool timing.
    pub total_tool_duration_ms: u64,
    /// Models without a pricing row.
    pub unpriced_models: Vec<ModelRef>,
    /// Server-side search count.
    pub total_web_search_requests: u32,
    /// Code-line counters.
    pub total_lines_added: u64,
    /// Code-line counters.
    pub total_lines_removed: u64,
    /// Last usage, when present.
    pub last_usage: Option<crate::Usage>,
    /// Last request cache-read count.
    pub last_cache_read_input_tokens: u64,
    /// Last request cache-creation count.
    pub last_cache_creation_input_tokens: u64,
    /// External aggregate spend.
    #[serde(default)]
    pub external_nano_usd: u64,
    /// Pre-V1 totals-only opening balance, kept distinct from new rollups.
    #[serde(default)]
    pub legacy_opening_balance_nano_usd: u64,
    /// Whether legacy matching was evaluated.
    #[serde(default)]
    pub legacy_import_evaluated: bool,
}

/// Canonical payload persisted for one accepted cost mutation. Keeping this
/// DTO in the cost crate lets the app coordinator validate and replay the same
/// typed identity/revision tuple without defining an engine-local serde shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostMutationRecord {
    /// Cost revision repeated outside the full state vector for validation.
    pub cost_revision: u64,
    /// Stable idempotency identity, equal to the journal envelope event id.
    pub mutation_id: CostMutationId,
    /// Mutation classification retained for audit/replay.
    pub source: CostMutationSource,
    /// Complete post-mutation state.
    pub state: CostStateVector,
}

impl From<&CostState> for CostStateVector {
    fn from(state: &CostState) -> Self {
        let mut per_model_usage = state.per_model_usage.values().cloned().collect::<Vec<_>>();
        per_model_usage.sort_by_key(|entry| {
            format!(
                "{}:{}",
                serde_json::to_string(&entry.model_ref.provider).unwrap_or_default(),
                entry.model_ref.model
            )
        });
        let mut unpriced_models = state.unpriced_models.iter().cloned().collect::<Vec<_>>();
        unpriced_models.sort_by_key(|model| {
            format!(
                "{}:{}",
                serde_json::to_string(&model.provider).unwrap_or_default(),
                model.model
            )
        });
        Self {
            session_id: state.session_id,
            cost_revision: state.cost_revision,
            total_nano_usd: state.total_nano_usd,
            per_model_usage,
            total_api_duration_ms: state.total_api_duration_ms,
            total_api_duration_without_retries_ms: state.total_api_duration_without_retries_ms,
            total_tool_duration_ms: state.total_tool_duration_ms,
            unpriced_models,
            total_web_search_requests: state.total_web_search_requests,
            total_lines_added: state.total_lines_added,
            total_lines_removed: state.total_lines_removed,
            last_usage: state.last_usage.clone(),
            last_cache_read_input_tokens: state.last_cache_read_input_tokens,
            last_cache_creation_input_tokens: state.last_cache_creation_input_tokens,
            external_nano_usd: state.external_nano_usd,
            legacy_opening_balance_nano_usd: state.legacy_opening_balance_nano_usd,
            legacy_import_evaluated: state.legacy_import_evaluated,
        }
    }
}

impl CostStateVector {
    /// Validate and fold the vector back into the in-memory map
    /// representation.  A vector is deliberately not allowed to silently
    /// overwrite duplicate structured keys, and each row must repeat the same
    /// model identity carried by its usage payload.
    pub fn try_into_state(self) -> Result<CostState, CostPersistError> {
        let mut per_model_usage = indexmap::IndexMap::new();
        for entry in self.per_model_usage {
            if per_model_usage
                .insert(entry.model_ref.clone(), entry)
                .is_some()
            {
                return Err(CostPersistError::Storage(
                    "cost vector contains a duplicate model identity".into(),
                ));
            }
        }
        Ok(CostState {
            session_id: self.session_id,
            cost_revision: self.cost_revision,
            total_nano_usd: self.total_nano_usd,
            per_model_usage,
            total_api_duration_ms: self.total_api_duration_ms,
            total_api_duration_without_retries_ms: self.total_api_duration_without_retries_ms,
            total_tool_duration_ms: self.total_tool_duration_ms,
            unpriced_models: self.unpriced_models.into_iter().collect(),
            total_web_search_requests: self.total_web_search_requests,
            total_lines_added: self.total_lines_added,
            total_lines_removed: self.total_lines_removed,
            last_usage: self.last_usage,
            last_cache_read_input_tokens: self.last_cache_read_input_tokens,
            last_cache_creation_input_tokens: self.last_cache_creation_input_tokens,
            external_nano_usd: self.external_nano_usd,
            legacy_opening_balance_nano_usd: self.legacy_opening_balance_nano_usd,
            legacy_import_evaluated: self.legacy_import_evaluated,
        })
    }

    /// Fold a validated vector into a state.  Production hydration should use
    /// [`Self::try_into_state`] and surface a storage failure; this convenience
    /// method is retained for trusted in-memory callers and tests.
    pub fn into_state(self) -> CostState {
        self.try_into_state()
            .expect("invalid CostStateVector supplied to trusted caller")
    }
}

/// Durable acknowledgment returned by an app coordinator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostPersistAck {
    /// Mutation this acknowledgment settles.
    pub mutation_id: CostMutationId,
    /// Journal revision assigned by the coordinator.
    pub journal_revision: u64,
    /// Cost revision acknowledged by the WAL.
    pub cost_revision: u64,
}

/// Persistence failures are retained by the caller and freeze only the
/// affected session; they never become a silent successful charge.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CostPersistError {
    /// Queue rejected the mutation before it entered the ledger.
    #[error("cost persistence queue rejected mutation: {0}")]
    Rejected(String),
    /// Durable state is frozen after a WAL/recovery failure.
    #[error("cost persistence is frozen: {0}")]
    Frozen(String),
    /// The coordinator could not append or acknowledge the request.
    #[error("cost persistence failed: {0}")]
    Storage(String),
}

/// Result sent to the mutation owner. The mutation id remains stable on every
/// retry, including a failed durable acknowledgment.
pub type CostPersistResult = Result<CostPersistAck, CostPersistError>;

/// Request transferred to the app-owned WAL coordinator.
pub struct CostPersistRequest {
    /// Canonical owning session.
    pub session_id: SessionId,
    /// Monotonic cost revision.
    pub cost_revision: u64,
    /// Stable mutation identity.
    pub mutation_id: CostMutationId,
    /// Complete vector snapshot after this mutation.
    pub state: CostStateVector,
    /// Mutation source for replay/audit.
    pub source: CostMutationSource,
    /// One-shot acknowledgment channel. The coordinator must retain the
    /// outcome even if the receiver is dropped, using its mutation table.
    pub ack: oneshot::Sender<CostPersistResult>,
}

/// A bounded queue permit. Capacity is acquired before the caller takes any
/// cost/reservation lock; enqueue itself is synchronous once those locks are
/// held, closing the cancellation gap between mutation and WAL submission.
pub struct CostPersistPermit {
    enqueue: Option<Box<dyn FnOnce(CostPersistRequest) -> Result<(), CostPersistError> + Send>>,
}

impl CostPersistPermit {
    /// Build a permit around a synchronous enqueue closure.
    #[must_use]
    pub fn new<F>(enqueue: F) -> Self
    where
        F: FnOnce(CostPersistRequest) -> Result<(), CostPersistError> + Send + 'static,
    {
        Self {
            enqueue: Some(Box::new(enqueue)),
        }
    }

    /// Enqueue exactly once while the caller still owns its state locks.
    pub fn enqueue(mut self, request: CostPersistRequest) -> Result<(), CostPersistError> {
        self.enqueue
            .take()
            .expect("cost persistence permit can enqueue only once")(request)
    }
}

/// App-tier implementation seam. The cost crate owns the contract; engine
/// desktop supplies the WAL/snapshot coordinator without a dependency cycle.
#[async_trait]
pub trait CostPersistence: Send + Sync {
    /// Reserve bounded queue capacity before state/reservation locks.
    async fn acquire_permit(
        &self,
        session_id: SessionId,
    ) -> Result<CostPersistPermit, CostPersistError>;
}

/// Hydration result installed before a session becomes active.
#[derive(Debug, Clone)]
pub struct CostHydration {
    /// Fully validated state.
    pub state: CostState,
    /// Last authoritative journal revision.
    pub journal_revision: u64,
}

/// App-tier state loader used during boot and hot session switches.
#[async_trait]
pub trait CostHydrator: Send + Sync {
    /// Load and validate the complete durable state for `session_id`.
    async fn hydrate(&self, session_id: SessionId) -> Result<CostHydration, CostPersistError>;
}

/// Shared per-session freeze latch. A WAL/recovery failure freezes paid
/// operations for this session while leaving other sessions healthy.
#[derive(Clone, Default)]
pub struct CostDurabilityGate {
    state: Arc<std::sync::Mutex<Option<String>>>,
}

impl CostDurabilityGate {
    /// Return the current freeze reason, if any.
    pub fn frozen_reason(&self) -> Option<String> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Freeze once; later failures preserve the first actionable reason.
    pub fn freeze(&self, reason: impl Into<String>) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.is_none() {
            *state = Some(reason.into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pricing::{ModelRef, ProviderId};

    #[test]
    fn vector_round_trip_preserves_structured_model_keys() {
        let mut state = CostState::default();
        state.session_id = SessionId::new();
        let model = ModelRef {
            provider: ProviderId::OpenAICompatible {
                name: "gateway".into(),
            },
            model: "model-a".into(),
        };
        state.per_model_usage.insert(
            model.clone(),
            ModelUsage {
                model_ref: model.clone(),
                usage: crate::Usage::default(),
                cache_read_input_tokens: 0,
                cache_creation_input_tokens: 0,
                cost_nano_usd: 7,
            },
        );
        let vector = CostStateVector::from(&state);
        let json = serde_json::to_string(&vector).expect("vector serializes");
        assert!(json.contains("model_ref"));
        assert_eq!(vector.into_state().per_model_usage[&model].cost_nano_usd, 7);
    }

    #[test]
    fn vector_rejects_duplicate_structured_model_keys() {
        let mut state = CostState::default();
        let model = ModelRef {
            provider: ProviderId::Anthropic,
            model: "same".into(),
        };
        state.per_model_usage.insert(
            model.clone(),
            ModelUsage {
                model_ref: model.clone(),
                usage: crate::Usage::default(),
                cache_read_input_tokens: 0,
                cache_creation_input_tokens: 0,
                cost_nano_usd: 1,
            },
        );
        let mut vector = CostStateVector::from(&state);
        let duplicate = vector.per_model_usage[0].clone();
        vector.per_model_usage.push(duplicate);
        assert!(matches!(
            vector.try_into_state(),
            Err(CostPersistError::Storage(message)) if message.contains("duplicate")
        ));
    }

    #[test]
    fn older_v1_vector_without_legacy_attribution_fields_defaults_safely() {
        let session_id = SessionId::new();
        let mut value = serde_json::to_value(CostStateVector::from(&CostState {
            session_id,
            total_nano_usd: 17,
            ..Default::default()
        }))
        .unwrap();
        let object = value.as_object_mut().unwrap();
        object.remove("legacy_opening_balance_nano_usd");
        object.remove("legacy_import_evaluated");
        let restored: CostStateVector = serde_json::from_value(value).unwrap();

        assert_eq!(restored.session_id, session_id);
        assert_eq!(restored.total_nano_usd, 17);
        assert_eq!(restored.legacy_opening_balance_nano_usd, 0);
        assert!(!restored.legacy_import_evaluated);
    }
}

//! Cost tracking — single-writer task ensures consistent persisted state.
//!
//! [`CostTracker`] accumulates per-session API usage and money totals as the
//! engine runs. Every mutation snapshots the new state and forwards it to a
//! single-writer `mpsc` channel; a background task drains that channel to
//! disk. Using one channel as the persistence boundary avoids interleaved
//! writes corrupting the on-disk snapshot.

use crate::{
    calculator::CostCalculator,
    pricing::{PricingCatalog, PricingResolution},
    usage::Usage,
    ModelRef,
};
use indexmap::IndexMap;
use protocol::SessionId;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use telemetry::AnalyticsBus;
use tokio::sync::{mpsc, Mutex, RwLock};

/// Persisted snapshot of one session's cumulative cost and usage.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CostState {
    /// Owning session.
    pub session_id: SessionId,
    /// Cumulative cost across all models, in nano-USD.
    pub total_nano_usd: u64,
    /// Per-model usage and cost breakdown.
    pub per_model_usage: IndexMap<ModelRef, ModelUsage>,
    /// Total wall-clock spent in API calls (including retries), in ms.
    pub total_api_duration_ms: u64,
    /// Total wall-clock spent in API calls excluding retried attempts, in ms.
    pub total_api_duration_without_retries_ms: u64,
    /// Total wall-clock spent in client-side tool execution, in ms.
    pub total_tool_duration_ms: u64,
    /// Models for which no pricing was found; cost recorded as `0` and the
    /// model is added to this set so the host can surface a warning.
    pub unpriced_models: HashSet<ModelRef>,
    /// Cumulative server-side web search request count.
    pub total_web_search_requests: u32,
    /// Cumulative lines added across all edits this session (claude-code
    /// `Pt.totalLinesAdded`).
    pub total_lines_added: u64,
    /// Cumulative lines removed across all edits this session (claude-code
    /// `Pt.totalLinesRemoved`).
    pub total_lines_removed: u64,
    /// Most recent request usage for status-line `current_usage`.
    #[serde(default)]
    pub last_usage: Option<Usage>,
    /// Cache-read tokens associated with [`Self::last_usage`].
    #[serde(default)]
    pub last_cache_read_input_tokens: u64,
    /// Cache-creation tokens associated with [`Self::last_usage`].
    #[serde(default)]
    pub last_cache_creation_input_tokens: u64,
    /// Cumulative externally-priced spend recorded via
    /// [`CostTracker::record_external_cost`] (Fusion panel/analyst/synth
    /// settlement) — a SUBSET already included in [`Self::total_nano_usd`],
    /// tracked separately so a summary can reconcile
    /// `sum(per_model_usage[..].cost_nano_usd) + external_nano_usd ==
    /// total_nano_usd` instead of `total_nano_usd` silently outrunning the
    /// per-model breakdown with no attributable source.
    #[serde(default)]
    pub external_nano_usd: u64,
}

/// Per-model usage and cost slice of a [`CostState`].
///
/// M3-05 adds `cache_read_input_tokens` and `cache_creation_input_tokens`
/// (declaration position locked AFTER `usage` and BEFORE `cost_nano_usd`).
/// These are Anthropic-specific prompt-caching counters surfaced in the
/// `tengu_api_success` payload; they default to `0` for non-Anthropic
/// providers.
///
/// `#[serde(default)]` on the new fields preserves on-disk compatibility:
/// previously persisted [`CostState`] JSON without these fields deserializes
/// with zeros, so an upgrade does not invalidate existing session files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelUsage {
    /// Which model this slice belongs to.
    pub model_ref: ModelRef,
    /// Cumulative usage counters for this model.
    pub usage: Usage,
    /// Cumulative tokens read from the prompt cache (Anthropic-specific).
    #[serde(default)]
    pub cache_read_input_tokens: u64,
    /// Cumulative tokens written into a fresh cache entry (Anthropic-specific).
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    /// Cumulative cost in nano-USD for this model.
    pub cost_nano_usd: u64,
}

/// Shared in-memory session ledger.
///
/// The process owns one active projection, but background work must retain a
/// stable view of the session that originated it. Keeping the state cells in
/// this map means a clear/resume can move the active projection without
/// destroying a previous session's unfinished or completed settlement.
struct SessionLedger {
    active_session: RwLock<SessionId>,
    states: Mutex<HashMap<SessionId, Arc<RwLock<CostState>>>>,
    /// Persisted resume baselines already merged into each session. The
    /// marker is separate from the state cell because a cell can receive
    /// late in-memory settlement before the resume loader supplies its saved
    /// baseline; that baseline must be added exactly once to the delta.
    hydrated_sessions: Mutex<HashMap<SessionId, u64>>,
}

impl SessionLedger {
    fn new(session_id: SessionId, state: CostState) -> Self {
        let mut states = HashMap::new();
        states.insert(session_id, Arc::new(RwLock::new(state)));
        Self {
            active_session: RwLock::new(session_id),
            states: Mutex::new(states),
            hydrated_sessions: Mutex::new(HashMap::new()),
        }
    }

    async fn state_for(&self, session_id: SessionId) -> Arc<RwLock<CostState>> {
        let mut states = self.states.lock().await;
        states
            .entry(session_id)
            .or_insert_with(|| {
                Arc::new(RwLock::new(CostState {
                    session_id,
                    ..Default::default()
                }))
            })
            .clone()
    }

    async fn active_state(&self) -> (SessionId, Arc<RwLock<CostState>>) {
        let session_id = *self.active_session.read().await;
        (session_id, self.state_for(session_id).await)
    }

    async fn switch_active(&self, session_id: SessionId) {
        // Create the destination before publishing it as active. A live turn
        // that starts after the switch always sees a real state cell.
        self.state_for(session_id).await;
        let previous = *self.active_session.read().await;
        if previous != session_id {
            // Leaving a live session establishes its zero persisted baseline:
            // any later resume of that in-process ledger must not add the same
            // on-disk spend a second time. The initial active session remains
            // unmarked until it is either hydrated or left.
            self.hydrated_sessions
                .lock()
                .await
                .entry(previous)
                .or_insert(0);
        }
        *self.active_session.write().await = session_id;
    }
}

/// In-memory accumulator + persistence channel for the active session.
///
/// `CostTracker::scoped` creates a fixed-origin view over the same ledger for
/// delayed work such as Fusion. The ordinary tracker follows the active
/// session projection switched by the orchestrator at clear/resume boundaries.
pub struct CostTracker {
    ledger: Arc<SessionLedger>,
    scope: Option<SessionId>,
    catalog: Arc<PricingCatalog>,
    persist_tx: mpsc::Sender<CostState>,
}

impl CostTracker {
    /// Construct a fresh tracker for `session_id`.
    ///
    /// `persist_tx` is a single-writer channel that drains to disk in a
    /// background task. This prevents concurrent writers from corrupting
    /// the persisted file.
    #[must_use]
    pub fn new(
        session_id: SessionId,
        catalog: Arc<PricingCatalog>,
        persist_tx: mpsc::Sender<CostState>,
    ) -> Self {
        let state = CostState {
            session_id,
            ..Default::default()
        };
        Self {
            ledger: Arc::new(SessionLedger::new(session_id, state)),
            scope: None,
            catalog,
            persist_tx,
        }
    }

    /// Return a view permanently bound to `session_id`.
    ///
    /// Unlike the active tracker, this view never follows a later clear or
    /// resume. Existing and late Fusion settlement therefore remains charged
    /// to the originating session.
    #[must_use]
    pub fn scoped(&self, session_id: SessionId) -> Arc<Self> {
        Arc::new(Self {
            ledger: self.ledger.clone(),
            scope: Some(session_id),
            catalog: self.catalog.clone(),
            persist_tx: self.persist_tx.clone(),
        })
    }

    /// Current active session id. Scoped views return their fixed origin id.
    pub async fn session_id(&self) -> SessionId {
        if let Some(session_id) = self.scope {
            session_id
        } else {
            *self.ledger.active_session.read().await
        }
    }

    async fn selected_state(&self) -> Arc<RwLock<CostState>> {
        if let Some(session_id) = self.scope {
            self.ledger.state_for(session_id).await
        } else {
            self.ledger.active_state().await.1
        }
    }

    /// Resolve the state cell for a budget operation. The caller may acquire
    /// its lock before touching a reservation token so cancellation cannot
    /// consume a hold before the actual charge mutation is ready.
    pub(crate) async fn selected_state_cell(&self) -> Arc<RwLock<CostState>> {
        self.selected_state().await
    }

    /// Switch the active projection while preserving every session cell.
    pub async fn switch_session(&self, session_id: SessionId) {
        if self.scope.is_none() {
            self.ledger.switch_active(session_id).await;
        }
    }

    /// Adopt the orchestrator's construction-time session id without an
    /// async boundary. Builders run before any turn and may attach a tracker
    /// created with a placeholder id (legacy tests/hosts); move that sole
    /// pre-seeded cell while making future active writes use the real id.
    /// No budget reservation may have been issued from the tracker before this
    /// construction step; reservation ownership is intentionally not remapped.
    ///
    /// This is deliberately a construction-only migration, not a general
    /// session switch. Reusing a multi-session tracker or one that has already
    /// published a scoped view would invalidate that view's stable origin, so
    /// those shapes are rejected instead of copying spend into two sessions.
    pub fn adopt_active_session_for_builder(&self, session_id: SessionId) {
        assert!(
            self.scope.is_none(),
            "cost tracker builder adoption requires an active tracker"
        );
        let mut active = self
            .ledger
            .active_session
            .try_write()
            .expect("cost tracker builder adoption must run before concurrent turns");
        let previous = *active;
        if previous == session_id {
            return;
        }
        assert_eq!(
            Arc::strong_count(&self.ledger),
            1,
            "cost tracker builder adoption cannot remap published scoped views"
        );
        let mut states = self
            .ledger
            .states
            .try_lock()
            .expect("cost tracker builder adoption must run before concurrent turns");
        assert_eq!(
            states.len(),
            1,
            "cost tracker builder adoption requires one construction-time session"
        );
        let source = states
            .remove(&previous)
            .expect("active cost session state exists");
        {
            let mut state = source
                .try_write()
                .expect("cost tracker builder adoption source is not in use");
            state.session_id = session_id;
        }
        assert!(states.insert(session_id, source).is_none());
        let mut hydrated = self
            .ledger
            .hydrated_sessions
            .try_lock()
            .expect("cost tracker builder adoption must run before concurrent turns");
        if let Some(baseline) = hydrated.remove(&previous) {
            hydrated.insert(session_id, baseline);
        }
        *active = session_id;
    }

    /// Merge a persisted baseline exactly once for one session. A resumed
    /// session may already have late in-memory settlement, so the baseline is
    /// added to the current cell rather than replacing it. Repeated resume
    /// hydration is a no-op, including after switching away and back.
    pub async fn restore_total_for_session(&self, session_id: SessionId, nano_usd: u64) {
        let state = self.ledger.state_for(session_id).await;
        let mut hydrated = self.ledger.hydrated_sessions.lock().await;
        if hydrated.contains_key(&session_id) {
            return;
        }
        let mut state = state.write().await;
        state.total_nano_usd = state.total_nano_usd.saturating_add(nano_usd);
        hydrated.insert(session_id, nano_usd);
    }

    /// Resolve the exact pricing catalog owned by this tracker, applying the
    /// same non-zero unknown-model fallback used by response accounting.
    /// Callers that surface estimates can therefore report the same price and
    /// provenance that will actually be charged into this session.
    #[must_use]
    pub fn resolve_pricing_with_default(
        &self,
        model_ref: &ModelRef,
    ) -> (crate::ModelPricing, PricingResolution) {
        self.catalog.resolve(model_ref).unwrap_or_else(|_| {
            (
                PricingCatalog::default_unknown_pricing(model_ref),
                PricingResolution::UnpricedModel {
                    requested: model_ref.clone(),
                },
            )
        })
    }

    /// Legacy M1 entry point — delegates to [`Self::record_api_response_v2`]
    /// with zero cache counters, `is_batch_request = false`, and no telemetry
    /// bus. Existing M1/M2 callers compile unchanged.
    ///
    /// `retries` is the number of retried attempts that preceded this final
    /// successful call; when `0`, the call's duration is also folded into
    /// the `without_retries` counter.
    pub async fn record_api_response(
        &self,
        model_ref: ModelRef,
        usage: Usage,
        duration: Duration,
        retries: u32,
    ) {
        let _ = self
            .record_api_response_v2(
                model_ref, usage, duration, retries, 0,     // cache_read_input_tokens
                0,     // cache_creation_input_tokens
                false, // is_batch_request — M3 always false
                None,  // bus — legacy callers don't emit
            )
            .await;
    }

    /// M3-05 entry point: record one successful API response. Returns the
    /// recorded cost in nano-USD for this single call.
    ///
    /// # Spec parity
    ///
    /// - Calls [`CostCalculator::calculate_nano_usd`] for the cost; saturating
    ///   arithmetic per v3 §17.
    /// - **NEVER applies the batches discount in M3** even if
    ///   `is_batch_request = true` (which it isn't in M3 because the
    ///   `/v1/messages/batches` endpoint is M4). M4 will multiply `cost` by
    ///   `(10000 - BATCH_DISCOUNT_BPS) / 10000` for true.
    /// - The per-request success telemetry (`tengu_api_success`) is fired from
    ///   the orchestrator success path, not here; the port-only
    ///   `tengu_cost_recorded` event was dropped under strict parity.
    ///
    /// # Returns
    ///
    /// The cost for **this single call** in nano-USD (not the cumulative
    /// session total). Callers that need the cumulative total can call
    /// [`Self::total_nano_usd`].
    #[allow(clippy::too_many_arguments)]
    pub async fn record_api_response_v2(
        &self,
        model_ref: ModelRef,
        usage: Usage,
        duration: Duration,
        retries: u32,
        cache_read_input_tokens: u64,
        cache_creation_input_tokens: u64,
        is_batch_request: bool,
        bus: Option<&Arc<AnalyticsBus>>,
    ) -> u64 {
        // Resolve pricing. On a catalog miss we do NOT bill zero: mirroring
        // claude-code's getModelCosts (`utils/modelCost.ts:155-163`), the
        // tokens are billed at the DEFAULT_UNKNOWN_MODEL_COST tier ($5/$25,
        // COST_TIER_5_25) instead of returning 0, and the model is flagged
        // unknown. The TS path records this via `setHasUnknownModelCost()`;
        // ours surfaces the model in `state.unpriced_models` (below). That set
        // is the signal a `/cost` summary renderer would read to append
        // " (costs may be inaccurate due to usage of unknown models)"
        // (`cost-tracker.ts:228-233`); no Rust caller renders that string yet,
        // and the renderer lives outside the cost crate, so the warning is
        // surfaced there — here we guarantee the non-zero billing + the flag.
        let (pricing, resolution) = self.resolve_pricing_with_default(&model_ref);

        let cost = CostCalculator::calculate_nano_usd(&usage, &pricing);

        // ----- update in-memory state -----
        let state_cell = self.selected_state().await;
        let (session_id, snap) = {
            let mut state = state_cell.write().await;
            state.total_nano_usd = state.total_nano_usd.saturating_add(cost);
            #[allow(clippy::cast_possible_truncation)]
            let dur_ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
            state.total_api_duration_ms = state.total_api_duration_ms.saturating_add(dur_ms);
            if retries == 0 {
                state.total_api_duration_without_retries_ms = state
                    .total_api_duration_without_retries_ms
                    .saturating_add(dur_ms);
            }
            let entry = state
                .per_model_usage
                .entry(model_ref.clone())
                .or_insert_with(|| ModelUsage {
                    model_ref: model_ref.clone(),
                    usage: Usage::default(),
                    cache_read_input_tokens: 0,
                    cache_creation_input_tokens: 0,
                    cost_nano_usd: 0,
                });
            entry.usage.add(&usage);
            entry.cost_nano_usd = entry.cost_nano_usd.saturating_add(cost);
            entry.cache_read_input_tokens = entry
                .cache_read_input_tokens
                .saturating_add(cache_read_input_tokens);
            entry.cache_creation_input_tokens = entry
                .cache_creation_input_tokens
                .saturating_add(cache_creation_input_tokens);
            if matches!(resolution, PricingResolution::UnpricedModel { .. }) {
                state.unpriced_models.insert(model_ref.clone());
            }
            if let Some(s) = usage.server_tool_use {
                state.total_web_search_requests = state
                    .total_web_search_requests
                    .saturating_add(s.web_search_requests);
            }
            state.last_usage = Some(usage);
            state.last_cache_read_input_tokens = cache_read_input_tokens;
            state.last_cache_creation_input_tokens = cache_creation_input_tokens;
            (state.session_id, state.clone())
        };
        self.persist_snapshot(snap).await;

        // `tengu_cost_recorded` was a PORT-ONLY event (0 hits in claude-code
        // 2.1.195) — dropped under strict parity. The per-request success
        // telemetry is now `tengu_api_success`, fired from the orchestrator
        // success path (where request id / stop reason / provider live), not
        // from here. Cost ACCOUNTING above is untouched.
        let _ = (&bus, is_batch_request, &session_id);

        cost
    }

    /// Cumulative cost across all models in nano-USD.
    pub async fn total_nano_usd(&self) -> u64 {
        self.selected_state().await.read().await.total_nano_usd
    }

    /// Seed the cumulative cost from a restored session (resume). Mirrors
    /// claude-code `setCostStateForRestore` (`bootstrap/state.ts`): a resumed
    /// session must continue from the prior accumulated cost so the footer
    /// shows the running total instead of `$0.0000`, and subsequent turns add
    /// on top. Only the money total is restored here (the port's footer /
    /// status-line cost is derived from it); the per-model token breakdown is
    /// not yet persisted, so it is left empty (documented parity follow-up).
    ///
    /// Does NOT emit on the persist channel — this is a hydrate, not a new
    /// charge, and the on-resume value is already the persisted truth.
    pub async fn restore_total_nano_usd(&self, nano_usd: u64) {
        let session_id = self.session_id().await;
        let state = self.ledger.state_for(session_id).await;
        let mut hydrated = self.ledger.hydrated_sessions.lock().await;
        state.write().await.total_nano_usd = nano_usd;
        hydrated.insert(session_id, nano_usd);
    }

    /// Update the in-memory state with externally-priced spend and return the
    /// snapshot that should be persisted.
    ///
    /// This is the lock-only half of [`Self::record_external_cost`]. Budget
    /// settlement uses it while holding its reservation-book lock so a
    /// concurrent reserve observes either the pre-settlement hold or the
    /// post-settlement realized spend, never both. The persistence send must
    /// happen after that lock is released; a bounded persistence channel must
    /// not hold budget capacity hostage.
    pub(crate) async fn record_external_cost_snapshot(&self, nano_usd: u64) -> Option<CostState> {
        if nano_usd == 0 {
            return None;
        }
        let state_cell = self.selected_state().await;
        let mut state = state_cell.write().await;
        Self::record_external_cost_in_state(&mut state, nano_usd)
    }

    /// Apply one external charge to an already-held state guard. No await is
    /// performed, which lets BudgetEnforcer consume a reservation and charge
    /// the tracker as one cancellation-safe in-memory transition.
    pub(crate) fn record_external_cost_in_state(
        state: &mut CostState,
        nano_usd: u64,
    ) -> Option<CostState> {
        if nano_usd == 0 {
            return None;
        }
        state.total_nano_usd = state.total_nano_usd.saturating_add(nano_usd);
        // Track this addition separately from `per_model_usage` (which
        // this call never touches — there is no single `ModelRef` for a
        // Fusion run's several priced components) so a summary can tell
        // "unattributed but accounted-for" apart from a `by_model`
        // breakdown that has silently fallen behind the total.
        state.external_nano_usd = state.external_nano_usd.saturating_add(nano_usd);
        Some(state.clone())
    }

    /// Persist a snapshot produced by one of the lock-only accounting
    /// operations. The sender is intentionally awaited outside any budget
    /// reservation lock.
    pub(crate) async fn persist_snapshot(&self, snapshot: CostState) {
        let _ = self.persist_tx.send(snapshot).await;
    }

    /// Add externally-priced spend directly onto the cumulative total.
    ///
    /// Used by [`crate::budget::BudgetEnforcer::commit_reservation`] for
    /// Fusion: panel/analyst/synth spend is priced by the fusion crate's own
    /// `FusionPriceBook` from usage the provider adapters never funnel through
    /// [`Self::record_api_response_v2`] (there is no single per-call
    /// `ModelRef`/`Usage` at that seam — a Fusion run prices several models'
    /// worth of usage into one already-computed `actual_nano_usd`). The
    /// addition happens under the SAME write lock `record_api_response_v2`
    /// uses so two concurrent commits cannot lose an update the way a
    /// read-then-[`Self::restore_total_nano_usd`] pair could.
    ///
    /// Emits on the persist channel like a real charge (unlike
    /// `restore_total_nano_usd`, which is a resume hydrate). No-op for `0`.
    ///
    /// KNOWN LIMITATION (tracked, not silently swept): this makes
    /// [`CostState::external_nano_usd`] and [`crate::summary::CostTracker::summary`]
    /// self-reconciling, but does NOT reach the production `/usage`/`/cost`
    /// rendering path — `ConversationModel::snapshot_cost_real`
    /// (`orchestrator/src/conversation/model.rs`) builds `platform_api::CostSnapshot`
    /// from `state.total_nano_usd` and `state.per_model_usage` directly and does
    /// not read this field, so a Fusion-only session still renders a nonzero
    /// total above a `by_model`/token breakdown of zero. Closing that requires
    /// either threading `external_nano_usd` through `CostSnapshot` and
    /// `cost::render::cost_summary_from_snapshot` (touches ~12 `CostSnapshot { .. }`
    /// literal sites plus the byte-pinned render template), or recording Fusion
    /// usage per-model at the spawner/side-query layer instead — a larger,
    /// separate task.
    pub async fn record_external_cost(&self, nano_usd: u64) {
        if let Some(snapshot) = self.record_external_cost_snapshot(nano_usd).await {
            self.persist_snapshot(snapshot).await;
        }
    }

    /// Reset every cumulative counter to zero — the parity twin of claude-code
    /// `resetCostState` (`yJe`), which `clearConversation` invokes so `/clear`
    /// starts a fresh session with a zeroed cost footer/status line instead of
    /// carrying the prior conversation's accumulated total (2.1.211 fix
    /// "Fixed /clear not resetting session cost counter").
    ///
    /// Zeroes the money total, per-model usage breakdown, API/tool durations,
    /// unpriced-model flags, web-search count, and code-change line counters —
    /// the full set `yJe` resets (`totalCostUSD`, `totalAPIDuration`,
    /// `totalAPIDurationWithoutRetries`, `totalToolDuration`, `modelUsage`,
    /// `hasUnknownModelCost`, `totalLinesAdded`, `totalLinesRemoved`).
    ///
    /// The `session_id` is PRESERVED: claude-code regenerates the id in a
    /// separate step (`regenerateSessionId`), and lingxi's orchestrator mints
    /// the fresh `SessionId` on `SessionState` in `clear_session`; the tracker's
    /// copy is informational (snapshots key the id off the live session state).
    ///
    /// In-memory only: like [`Self::restore_total_nano_usd`], this does NOT emit
    /// on the persist channel — a reset is not a new charge, and it mirrors
    /// `yJe`, which mutates the in-process cost singleton only (the prior
    /// session's totals are saved separately, before the reset).
    pub async fn reset(&self) {
        let mut hydrated = self.ledger.hydrated_sessions.lock().await;
        let state_cell = self.selected_state().await;
        let mut state = state_cell.write().await;
        let session_id = state.session_id;
        *state = CostState {
            session_id,
            ..Default::default()
        };
        hydrated.remove(&session_id);
    }

    /// Accumulate one edit's line changes (claude-code `Bhn(added, removed)`:
    /// `Pt.totalLinesAdded += added; Pt.totalLinesRemoved += removed`).
    pub async fn record_code_change(&self, added: u64, removed: u64) {
        let state_cell = self.selected_state().await;
        let mut state = state_cell.write().await;
        state.total_lines_added = state.total_lines_added.saturating_add(added);
        state.total_lines_removed = state.total_lines_removed.saturating_add(removed);
    }

    /// Snapshot the current state. Cloned, safe to inspect off-thread.
    pub async fn snapshot(&self) -> CostState {
        self.selected_state().await.read().await.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pricing::ProviderId;
    use crate::usage::TokenUsage;

    #[tokio::test]
    async fn record_accumulates_cost() {
        let (tx, mut rx) = mpsc::channel(8);
        let tracker = CostTracker::new(
            SessionId::nil(),
            Arc::new(PricingCatalog::builtin_reference()),
            tx,
        );
        let mr = ModelRef {
            provider: ProviderId::Anthropic,
            model: "claude-opus-4-6".into(),
        };
        tracker
            .record_api_response(
                mr.clone(),
                Usage {
                    tokens: TokenUsage {
                        input: 1000,
                        output: 500,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                Duration::from_millis(200),
                0,
            )
            .await;
        let snap = rx.recv().await.unwrap();
        // 1000 * 5000 + 500 * 25000 = 5_000_000 + 12_500_000 = 17_500_000 nano-USD = $0.0175
        assert_eq!(snap.total_nano_usd, 17_500_000);
    }

    #[tokio::test]
    async fn restore_seeds_total_and_subsequent_records_add_on_top() {
        // Resume parity: a restored session continues from the persisted total
        // and new charges accumulate on top of it (not from zero).
        let (tx, mut rx) = mpsc::channel(8);
        let tracker = CostTracker::new(
            SessionId::nil(),
            Arc::new(PricingCatalog::builtin_reference()),
            tx,
        );
        tracker.restore_total_nano_usd(17_500_000).await; // prior session $0.0175
        assert_eq!(tracker.total_nano_usd().await, 17_500_000);

        let mr = ModelRef {
            provider: ProviderId::Anthropic,
            model: "claude-opus-4-6".into(),
        };
        tracker
            .record_api_response(
                mr,
                Usage {
                    tokens: TokenUsage {
                        input: 1000,
                        output: 500,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                Duration::from_millis(200),
                0,
            )
            .await;
        let snap = rx.recv().await.unwrap();
        // restored 17_500_000 + this turn's 17_500_000 = 35_000_000 nano-USD.
        assert_eq!(snap.total_nano_usd, 35_000_000);
        assert_eq!(tracker.total_nano_usd().await, 35_000_000);
    }

    #[tokio::test]
    async fn record_external_cost_adds_onto_existing_total_and_persists() {
        let (tx, mut rx) = mpsc::channel(8);
        let tracker = CostTracker::new(
            SessionId::nil(),
            Arc::new(PricingCatalog::builtin_reference()),
            tx,
        );
        tracker.restore_total_nano_usd(1_000).await;
        tracker.record_external_cost(2_500).await;
        assert_eq!(
            tracker.total_nano_usd().await,
            3_500,
            "external cost adds onto the existing total, never replaces it"
        );
        let snap = rx.recv().await.unwrap();
        assert_eq!(snap.total_nano_usd, 3_500, "the addition is persisted");
    }

    /// The [`crate::summary::CostTracker::summary`] projection (NOT the
    /// production `/cost`/`/usage` path — see the `record_external_cost` doc
    /// comment) must be able to tell "this total includes externally-priced
    /// Fusion spend with no per-model row" apart from "the per-model
    /// breakdown has silently fallen behind the total" — before this fix a
    /// session with ONLY a Fusion run showed a nonzero `total_nano_usd` and
    /// an empty `by_model` map with nothing distinguishing that from a bug.
    #[tokio::test]
    async fn summary_reconciles_external_cost_against_the_empty_by_model_breakdown() {
        let (tx, _rx) = mpsc::channel(8);
        let tracker = CostTracker::new(
            SessionId::nil(),
            Arc::new(PricingCatalog::builtin_reference()),
            tx,
        );
        tracker.record_external_cost(1_800_000_000).await;
        let summary = tracker.summary().await;
        assert!(
            summary.by_model.is_empty(),
            "no per-model call was recorded"
        );
        assert_eq!(
            summary.session.total_nano_usd, 1_800_000_000,
            "the Fusion run's money reached the session total"
        );
        let by_model_total: u64 = summary.by_model.values().map(|m| m.total_nano_usd).sum();
        assert_eq!(
            by_model_total + summary.session.external_nano_usd,
            summary.session.total_nano_usd,
            "by_model's total plus external_nano_usd must reconcile to the \
session total — before this fix external_nano_usd did not exist and this \
gap had no explanation at all"
        );
        assert_eq!(summary.session.external_nano_usd, 1_800_000_000);
    }

    #[tokio::test]
    async fn record_external_cost_zero_is_a_true_noop() {
        let (tx, mut rx) = mpsc::channel(8);
        let tracker = CostTracker::new(
            SessionId::nil(),
            Arc::new(PricingCatalog::builtin_reference()),
            tx,
        );
        tracker.record_external_cost(0).await;
        assert_eq!(tracker.total_nano_usd().await, 0);
        assert!(
            rx.try_recv().is_err(),
            "a zero-cost commit must not emit a persist snapshot"
        );
    }

    #[tokio::test]
    async fn record_v2_tracks_cache_tokens() {
        // Strict-parity note: the tracker no longer emits a telemetry event —
        // `tengu_cost_recorded` was a port-only event (0 hits in claude-code
        // 2.1.195) and was dropped; the per-request success telemetry is now
        // `tengu_api_success`, fired from the orchestrator. This test asserts
        // the cost ACCOUNTING (return value + per-model cache counters).
        let (tx, mut rx) = mpsc::channel(8);
        let tracker = CostTracker::new(
            SessionId::nil(),
            Arc::new(PricingCatalog::builtin_reference()),
            tx,
        );
        let bus = Arc::new(telemetry::AnalyticsBus::new());

        let mr = ModelRef {
            provider: ProviderId::Anthropic,
            model: "claude-opus-4-6".into(),
        };
        let cost = tracker
            .record_api_response_v2(
                mr.clone(),
                Usage {
                    tokens: TokenUsage {
                        input: 1_000,
                        output: 500,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                Duration::from_millis(200),
                0,     // retries
                128,   // cache_read_input_tokens
                64,    // cache_creation_input_tokens
                false, // is_batch_request — ALWAYS false in M3
                Some(&bus),
            )
            .await;

        // 1000 * 5000 + 500 * 25000 = 17_500_000 nano-USD = $0.0175
        assert_eq!(cost, 17_500_000, "returned cost in nano-USD");

        // Snapshot reflects the new cache counters on the per-model entry.
        let snap = rx.recv().await.unwrap();
        assert_eq!(snap.total_nano_usd, 17_500_000);
        let entry = snap.per_model_usage.get(&mr).expect("model entry present");
        assert_eq!(entry.cache_read_input_tokens, 128);
        assert_eq!(entry.cache_creation_input_tokens, 64);
    }

    #[tokio::test]
    async fn record_v2_without_bus_still_updates_state() {
        // No bus → state still updates (the tracker emits no telemetry).
        let (tx, mut rx) = mpsc::channel(8);
        let tracker = CostTracker::new(
            SessionId::nil(),
            Arc::new(PricingCatalog::builtin_reference()),
            tx,
        );
        let mr = ModelRef {
            provider: ProviderId::Anthropic,
            model: "claude-opus-4-6".into(),
        };
        let cost = tracker
            .record_api_response_v2(
                mr,
                Usage {
                    tokens: TokenUsage {
                        input: 100,
                        output: 50,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                Duration::from_millis(10),
                0,
                0,
                0,
                false,
                None, // no bus
            )
            .await;
        assert!(cost > 0);
        let snap = rx.recv().await.unwrap();
        assert!(snap.total_nano_usd > 0);
    }

    #[tokio::test]
    async fn record_v2_unknown_model_bills_default_tier_and_flags() {
        // COST.1 parity: an unknown model is NOT billed at zero. Mirroring
        // claude-code's getModelCosts (`utils/modelCost.ts:155-163`), tokens are
        // billed at the DEFAULT_UNKNOWN_MODEL_COST tier ($5/$25) and the model
        // is flagged in `unpriced_models` so the host can surface the inaccuracy
        // warning. The cost event still fires so dashboards see the call.
        let (tx, mut rx) = mpsc::channel(8);
        let tracker = CostTracker::new(
            SessionId::nil(),
            Arc::new(PricingCatalog::builtin_reference()),
            tx,
        );
        let mr = ModelRef {
            provider: ProviderId::Anthropic,
            model: "claude-nonexistent-model".into(),
        };
        let cost = tracker
            .record_api_response_v2(
                mr.clone(),
                Usage {
                    tokens: TokenUsage {
                        input: 100,
                        output: 50,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                Duration::from_millis(10),
                0,
                0,
                0,
                false,
                None,
            )
            .await;
        // 100 * 5_000 + 50 * 25_000 = 500_000 + 1_250_000 = 1_750_000 nano-USD.
        assert_eq!(
            cost, 1_750_000,
            "unknown model bills at the $5/$25 default tier, not zero"
        );
        let snap = rx.recv().await.unwrap();
        assert!(
            snap.unpriced_models.contains(&mr),
            "unknown model is flagged so the inaccuracy warning can be surfaced"
        );
        assert_eq!(snap.total_nano_usd, 1_750_000);
    }

    #[tokio::test]
    async fn legacy_record_api_response_still_works() {
        // M1/M2 callers (e.g. M1 plan 02 cost-tracking code) still compile
        // and behave identically.
        let (tx, mut rx) = mpsc::channel(8);
        let tracker = CostTracker::new(
            SessionId::nil(),
            Arc::new(PricingCatalog::builtin_reference()),
            tx,
        );
        let mr = ModelRef {
            provider: ProviderId::Anthropic,
            model: "claude-opus-4-6".into(),
        };
        tracker
            .record_api_response(
                mr.clone(),
                Usage {
                    tokens: TokenUsage {
                        input: 1_000,
                        output: 500,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                Duration::from_millis(200),
                0,
            )
            .await;
        let snap = rx.recv().await.unwrap();
        // 17_500_000 nano-USD identical to the existing M1 test.
        assert_eq!(snap.total_nano_usd, 17_500_000);
        // Cache counters default to 0 in the legacy path.
        let entry = snap.per_model_usage.get(&mr).unwrap();
        assert_eq!(entry.cache_read_input_tokens, 0);
        assert_eq!(entry.cache_creation_input_tokens, 0);
    }

    #[tokio::test]
    async fn per_model_usage_preserves_insertion_order() {
        // Byte-parity: claude-code's `cbg` renders "Usage by model:" rows in
        // JS-object insertion (first-seen) order. Our per_model_usage must
        // match — a HashMap would iterate in a per-process-randomized order.
        // Record "zzz-first" before "aaa-second" so neither alphabetical sort
        // nor hash order could coincide with insertion order by accident.
        let (tx, mut rx) = mpsc::channel(8);
        let tracker = CostTracker::new(
            SessionId::nil(),
            Arc::new(PricingCatalog::builtin_reference()),
            tx,
        );
        let first = ModelRef {
            provider: ProviderId::Anthropic,
            model: "zzz-first".into(),
        };
        let second = ModelRef {
            provider: ProviderId::Anthropic,
            model: "aaa-second".into(),
        };
        tracker
            .record_api_response(
                first,
                Usage {
                    tokens: TokenUsage {
                        input: 100,
                        output: 50,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                Duration::from_millis(10),
                0,
            )
            .await;
        let _ = rx.recv().await.unwrap();
        tracker
            .record_api_response(
                second,
                Usage {
                    tokens: TokenUsage {
                        input: 200,
                        output: 75,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                Duration::from_millis(10),
                0,
            )
            .await;
        let _ = rx.recv().await.unwrap();

        let snap = tracker.snapshot().await;
        let order: Vec<String> = snap
            .per_model_usage
            .keys()
            .map(|m| m.model.clone())
            .collect();
        assert_eq!(
            order,
            vec!["zzz-first".to_string(), "aaa-second".to_string()],
            "per_model_usage must iterate in insertion order, not hash order"
        );
    }

    #[tokio::test]
    async fn reset_zeros_every_cumulative_counter_and_keeps_session_id() {
        // parity 2.1.212: `/clear` → resetCostState (yJe) zeroes the whole cost
        // state so a freshly-cleared session no longer shows the prior total.
        let (tx, mut rx) = mpsc::channel(8);
        let tracker = CostTracker::new(
            SessionId::nil(),
            Arc::new(PricingCatalog::builtin_reference()),
            tx,
        );
        // Accrue a known model, an unknown model (flags unpriced_models), a web
        // search, and code-change lines so every reset target is non-zero.
        let priced = ModelRef {
            provider: ProviderId::Anthropic,
            model: "claude-opus-4-6".into(),
        };
        let unknown = ModelRef {
            provider: ProviderId::Anthropic,
            model: "claude-nonexistent-model".into(),
        };
        tracker
            .record_api_response(
                priced,
                Usage {
                    tokens: TokenUsage {
                        input: 1_000,
                        output: 500,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                Duration::from_millis(200),
                0,
            )
            .await;
        let _ = rx.recv().await.unwrap();
        tracker
            .record_api_response(
                unknown.clone(),
                Usage {
                    tokens: TokenUsage {
                        input: 10,
                        output: 5,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                Duration::from_millis(10),
                1, // a retry — folds into total_api_duration_ms only
            )
            .await;
        let _ = rx.recv().await.unwrap();
        tracker.record_code_change(7, 3).await;

        let before = tracker.snapshot().await;
        assert!(before.total_nano_usd > 0);
        assert!(!before.per_model_usage.is_empty());
        assert!(before.unpriced_models.contains(&unknown));
        assert!(before.total_api_duration_ms > 0);

        tracker.reset().await;

        let snap = tracker.snapshot().await;
        assert_eq!(snap.total_nano_usd, 0);
        assert!(snap.per_model_usage.is_empty());
        assert_eq!(snap.total_api_duration_ms, 0);
        assert_eq!(snap.total_api_duration_without_retries_ms, 0);
        assert_eq!(snap.total_tool_duration_ms, 0);
        assert_eq!(snap.total_lines_added, 0);
        assert_eq!(snap.total_lines_removed, 0);
        assert_eq!(snap.total_web_search_requests, 0);
        assert!(snap.unpriced_models.is_empty());
        // session_id survives the reset (claude-code regenerates it separately).
        assert_eq!(snap.session_id, SessionId::nil());
        assert_eq!(tracker.total_nano_usd().await, 0);
    }

    #[tokio::test]
    async fn record_code_change_accumulates() {
        let (tx, _rx) = mpsc::channel(8);
        let tracker = CostTracker::new(
            SessionId::nil(),
            Arc::new(PricingCatalog::builtin_reference()),
            tx,
        );
        tracker.record_code_change(3, 1).await;
        tracker.record_code_change(0, 2).await;
        let snap = tracker.snapshot().await;
        assert_eq!(snap.total_lines_added, 3);
        assert_eq!(snap.total_lines_removed, 3);
    }

    #[tokio::test]
    async fn scoped_session_view_survives_active_switch_and_late_settlement() {
        let (tx, _rx) = mpsc::channel(16);
        let session_a = SessionId::new();
        let session_b = SessionId::new();
        let tracker = Arc::new(CostTracker::new(
            session_a,
            Arc::new(PricingCatalog::builtin_reference()),
            tx,
        ));
        let origin = tracker.scoped(session_a);
        origin.record_external_cost(125).await;

        tracker.switch_session(session_b).await;
        assert_eq!(tracker.total_nano_usd().await, 0);
        origin.record_external_cost(75).await;
        assert_eq!(tracker.total_nano_usd().await, 0);

        tracker.switch_session(session_a).await;
        assert_eq!(tracker.total_nano_usd().await, 200);
        assert_eq!(origin.snapshot().await.session_id, session_a);
    }

    #[tokio::test]
    async fn restore_hydrates_pristine_session_once_without_overwriting_live_spend() {
        let (tx, _rx) = mpsc::channel(16);
        let session = SessionId::new();
        let tracker = CostTracker::new(session, Arc::new(PricingCatalog::builtin_reference()), tx);
        tracker.restore_total_for_session(session, 500).await;
        tracker.restore_total_for_session(session, 900).await;
        assert_eq!(tracker.total_nano_usd().await, 500);

        tracker.record_external_cost(25).await;
        tracker.restore_total_for_session(session, 1_000).await;
        assert_eq!(tracker.total_nano_usd().await, 525);
    }

    #[tokio::test]
    async fn leaving_a_live_session_marks_its_zero_baseline_before_revisit() {
        let (tx, _rx) = mpsc::channel(16);
        let session_a = SessionId::new();
        let session_b = SessionId::new();
        let tracker =
            CostTracker::new(session_a, Arc::new(PricingCatalog::builtin_reference()), tx);
        tracker.record_external_cost(100).await;
        tracker.switch_session(session_b).await;
        tracker.restore_total_for_session(session_a, 100).await;
        tracker.switch_session(session_a).await;
        assert_eq!(tracker.total_nano_usd().await, 100);
    }

    #[tokio::test]
    async fn builder_adoption_moves_state_and_its_hydration_marker_once() {
        let (tx, _rx) = mpsc::channel(16);
        let placeholder = SessionId::new();
        let mounted = SessionId::new();
        let tracker = CostTracker::new(
            placeholder,
            Arc::new(PricingCatalog::builtin_reference()),
            tx,
        );
        tracker.restore_total_for_session(placeholder, 500).await;
        tracker.record_external_cost(25).await;

        tracker.adopt_active_session_for_builder(mounted);
        let adopted = tracker.snapshot().await;
        assert_eq!(adopted.session_id, mounted);
        assert_eq!(adopted.total_nano_usd, 525);

        // The persisted baseline moved with the state. Hydrating the mounted
        // id again must not add the same 500 nano-USD a second time.
        tracker.restore_total_for_session(mounted, 500).await;
        assert_eq!(tracker.total_nano_usd().await, 525);

        // Adoption moves rather than clones: the placeholder no longer owns a
        // duplicate of the mounted session's spend.
        assert_eq!(tracker.scoped(placeholder).total_nano_usd().await, 0);
    }

    #[test]
    #[should_panic(expected = "cannot remap published scoped views")]
    fn builder_adoption_rejects_a_tracker_with_a_published_origin_view() {
        let (tx, _rx) = mpsc::channel(1);
        let placeholder = SessionId::new();
        let tracker = CostTracker::new(
            placeholder,
            Arc::new(PricingCatalog::builtin_reference()),
            tx,
        );
        let _published_origin = tracker.scoped(placeholder);

        tracker.adopt_active_session_for_builder(SessionId::new());
    }

    #[tokio::test]
    #[should_panic(expected = "requires one construction-time session")]
    async fn builder_adoption_rejects_a_multi_session_ledger() {
        let (tx, _rx) = mpsc::channel(1);
        let tracker = CostTracker::new(
            SessionId::new(),
            Arc::new(PricingCatalog::builtin_reference()),
            tx,
        );
        tracker.switch_session(SessionId::new()).await;

        tracker.adopt_active_session_for_builder(SessionId::new());
    }
}

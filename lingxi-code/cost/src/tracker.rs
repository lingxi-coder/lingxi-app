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
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use telemetry::AnalyticsBus;
use tokio::sync::{mpsc, RwLock};

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
/// previously persisted `CostState` JSON without these fields deserializes
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

/// In-memory accumulator + persistence channel for one session's cost state.
pub struct CostTracker {
    state: Arc<RwLock<CostState>>,
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
            state: Arc::new(RwLock::new(state)),
            catalog,
            persist_tx,
        }
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
        let (pricing, resolution) = match self.catalog.resolve(&model_ref) {
            Ok((p, r)) => (p, r),
            Err(_) => (
                PricingCatalog::default_unknown_pricing(&model_ref),
                PricingResolution::UnpricedModel {
                    requested: model_ref.clone(),
                },
            ),
        };

        let cost = CostCalculator::calculate_nano_usd(&usage, &pricing);

        // ----- update in-memory state -----
        let (session_id, snap) = {
            let mut state = self.state.write().await;
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
            (state.session_id, state.clone())
        };
        let _ = self.persist_tx.send(snap).await;

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
        self.state.read().await.total_nano_usd
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
        self.state.write().await.total_nano_usd = nano_usd;
    }

    /// Accumulate one edit's line changes (claude-code `Bhn(added, removed)`:
    /// `Pt.totalLinesAdded += added; Pt.totalLinesRemoved += removed`).
    pub async fn record_code_change(&self, added: u64, removed: u64) {
        let mut state = self.state.write().await;
        state.total_lines_added = state.total_lines_added.saturating_add(added);
        state.total_lines_removed = state.total_lines_removed.saturating_add(removed);
    }

    /// Snapshot the current state. Cloned, safe to inspect off-thread.
    pub async fn snapshot(&self) -> CostState {
        self.state.read().await.clone()
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
}

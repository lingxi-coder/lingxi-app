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
use lingxi_protocol::SessionId;
use lingxi_telemetry::AnalyticsBus;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, RwLock};

/// Persisted snapshot of one session's cumulative cost and usage.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CostState {
    /// Owning session.
    pub session_id: SessionId,
    /// Cumulative cost across all models, in nano-USD.
    pub total_nano_usd: u64,
    /// Per-model usage and cost breakdown.
    pub per_model_usage: HashMap<ModelRef, ModelUsage>,
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
}

/// Per-model usage and cost slice of a [`CostState`].
///
/// M3-05 adds `cache_read_input_tokens` and `cache_creation_input_tokens`
/// (declaration position locked AFTER `usage` and BEFORE `cost_nano_usd`).
/// These are Anthropic-specific prompt-caching counters forwarded to the
/// `tengu_cost_recorded` payload; they default to `0` for non-Anthropic
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

    /// M3-05 entry point: record one successful API response and emit
    /// `tengu_cost_recorded` if a bus is provided. Returns the recorded
    /// cost in nano-USD for this single call.
    ///
    /// # Spec parity
    ///
    /// - Calls [`CostCalculator::calculate_nano_usd`] for the cost; saturating
    ///   arithmetic per v3 §17.
    /// - **NEVER applies the batches discount in M3** even if
    ///   `is_batch_request = true` (which it isn't in M3 because the
    ///   `/v1/messages/batches` endpoint is M4). The `is_batch_request` arg is
    ///   forwarded verbatim to the `tengu_cost_recorded` payload so dashboards
    ///   can group by it later. M4 will multiply `cost` by
    ///   `(10000 - BATCH_DISCOUNT_BPS) / 10000` for true.
    /// - Emits `tengu_cost_recorded` AFTER the in-memory state is updated and
    ///   AFTER the snapshot is forwarded to `persist_tx`, so the event reflects
    ///   the post-record state.
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
        let (pricing, resolution) = match self.catalog.resolve(&model_ref) {
            Ok(p) => (Some(p.0), Some(p.1)),
            // Unknown model → cost is 0, and we surface the model in
            // `state.unpriced_models` so the host can warn. Map `Err` to the
            // `UnpricedModel` resolution variant so the downstream
            // `matches!(resolution, Some(UnpricedModel { .. }))` fires.
            Err(_) => (
                None,
                Some(PricingResolution::UnpricedModel {
                    requested: model_ref.clone(),
                }),
            ),
        };

        let cost = pricing
            .as_ref()
            .map_or(0, |p| CostCalculator::calculate_nano_usd(&usage, p));

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
            if matches!(resolution, Some(PricingResolution::UnpricedModel { .. })) {
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

        // ----- emit tengu_cost_recorded -----
        if let Some(bus) = bus {
            crate::events::emit_cost_recorded(
                bus,
                &model_ref.model,
                usage.tokens.input,
                usage.tokens.output,
                cache_read_input_tokens,
                cache_creation_input_tokens,
                cost,
                &session_id,
                is_batch_request,
            )
            .await;
        }

        cost
    }

    /// Cumulative cost across all models in nano-USD.
    pub async fn total_nano_usd(&self) -> u64 {
        self.state.read().await.total_nano_usd
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
    async fn record_v2_tracks_cache_tokens_and_emits_event() {
        use async_trait::async_trait;
        use lingxi_telemetry::{AnalyticsBus, AnalyticsSink, AnalyticsValue, LogEventMetadata};
        use std::sync::Mutex;

        #[derive(Default)]
        struct CaptureSink {
            events: Mutex<Vec<(String, LogEventMetadata)>>,
        }
        #[async_trait]
        impl AnalyticsSink for CaptureSink {
            async fn log_event(&self, name: &str, metadata: LogEventMetadata) {
                self.events.lock().unwrap().push((name.into(), metadata));
            }
            async fn log_event_async(&self, name: &str, metadata: LogEventMetadata) {
                self.events.lock().unwrap().push((name.into(), metadata));
            }
            fn name(&self) -> &str {
                "capture"
            }
        }

        let (tx, mut rx) = mpsc::channel(8);
        let tracker = CostTracker::new(
            SessionId::nil(),
            Arc::new(PricingCatalog::builtin_reference()),
            tx,
        );
        let bus = Arc::new(AnalyticsBus::new());
        let sink: Arc<CaptureSink> = Arc::new(CaptureSink::default());
        bus.attach_sink(sink.clone() as Arc<dyn AnalyticsSink>)
            .await;

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

        // tengu_cost_recorded fired exactly once with the cache counters.
        let events = sink.events.lock().unwrap();
        assert_eq!(events.len(), 1, "exactly one cost event");
        assert_eq!(events[0].0, "tengu_cost_recorded");
        match &events[0].1["cache_read_input_tokens"] {
            AnalyticsValue::Int(n) => assert_eq!(*n, 128),
            other => panic!("expected Int(128), got {other:?}"),
        }
        match &events[0].1["cache_creation_input_tokens"] {
            AnalyticsValue::Int(n) => assert_eq!(*n, 64),
            other => panic!("expected Int(64), got {other:?}"),
        }
        match &events[0].1["is_batch_request"] {
            AnalyticsValue::Bool(b) => assert!(!*b, "M3 always false"),
            other => panic!("expected Bool(false), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn record_v2_without_bus_silently_skips_emission() {
        // No bus → no event but state still updates.
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
    async fn record_v2_unknown_model_records_zero_cost_and_emits() {
        // Unknown model: cost is 0 but the event still fires (so dashboards
        // see the call). The model is added to `unpriced_models` per existing
        // M1/M2 behavior.
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
        assert_eq!(
            cost, 0,
            "unpriced model yields zero cost (existing M1 behavior)"
        );
        let snap = rx.recv().await.unwrap();
        assert!(snap.unpriced_models.contains(&mr));
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
}

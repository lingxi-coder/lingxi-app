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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelUsage {
    /// Which model this slice belongs to.
    pub model_ref: ModelRef,
    /// Cumulative usage counters for this model.
    pub usage: Usage,
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

    /// Record a completed API response: cost it against the catalog, fold
    /// usage and durations into the running state, and forward the new
    /// snapshot to the persistence channel.
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
        let (pricing, resolution) = match self.catalog.resolve(&model_ref) {
            Ok(p) => (Some(p.0), Some(p.1)),
            Err(_) => (None, None),
        };

        let cost = pricing
            .as_ref()
            .map_or(0, |p| CostCalculator::calculate_nano_usd(&usage, p));

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
                cost_nano_usd: 0,
            });
        entry.usage.add(&usage);
        entry.cost_nano_usd = entry.cost_nano_usd.saturating_add(cost);
        if matches!(resolution, Some(PricingResolution::UnpricedModel { .. })) {
            state.unpriced_models.insert(model_ref);
        }
        if let Some(s) = usage.server_tool_use {
            state.total_web_search_requests = state
                .total_web_search_requests
                .saturating_add(s.web_search_requests);
        }
        let snap = state.clone();
        drop(state);
        let _ = self.persist_tx.send(snap).await;
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
}

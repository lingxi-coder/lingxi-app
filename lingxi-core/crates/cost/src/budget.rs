//! Budget enforcement — pre-call gate + post-call latch.
//!
//! [`BudgetEnforcer`] is consulted before every API call (with an estimated
//! cost) and again after the call returns (with the realized cost). The
//! pre-call gate may emit warnings, ask the host to confirm, or block;
//! the post-call latch sets a one-way "realized exceeded" flag that causes
//! every subsequent pre-call gate to halt regardless of the per-call estimate.

use crate::tracker::CostTracker;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Basis-points threshold at which `tengu_cost_budget_warning` fires.
///
/// 8000 bps = 80% of the configured session limit. Locked by spec §7 line 735
/// and the M3-05 brief.
pub const BUDGET_WARNING_THRESHOLD_BPS: u32 = 8000;

/// Basis-points threshold at which `tengu_cost_budget_exceeded` fires.
///
/// 10000 bps = 100% of the configured session limit. Locked by spec §7 line 736
/// and the M3-05 brief.
pub const BUDGET_EXCEEDED_THRESHOLD_BPS: u32 = 10000;

/// Configuration controlling [`BudgetEnforcer`] behaviour.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetConfig {
    /// Hard limit on total session cost, in nano-USD. `None` disables.
    pub max_session_nano_usd: Option<u64>,
    /// Hard limit on a single turn's cost, in nano-USD. `None` disables.
    pub max_turn_nano_usd: Option<u64>,
    /// Hard limit on a single turn's total tokens. `None` disables.
    pub max_turn_tokens: Option<u64>,
    /// Fractional thresholds (e.g. `[0.5, 0.8, 0.95]`) at which a one-shot
    /// warning event is emitted as the session approaches its budget.
    pub warning_thresholds: Vec<f64>,
    /// What to do when the budget is exceeded (or projected to be).
    pub on_exceed: BudgetExceedPolicy,
}

/// Policy for handling a budget exceedance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BudgetExceedPolicy {
    /// Stop immediately; no further API calls.
    Halt,
    /// Ask the host (and via the host, the user) whether to continue.
    AskUser,
    /// Emit a warning but allow the call to proceed.
    WarnOnly,
}

/// Cost tracker + atomic latch for realized-exceeded budget enforcement.
pub struct BudgetEnforcer {
    config: BudgetConfig,
    cost_tracker: Arc<CostTracker>,
    warnings_fired: RwLock<HashSet<u32>>,
    realized_exceeded: AtomicBool,
}

/// Result of a [`BudgetEnforcer::check_pre_api_call`].
#[derive(Debug, Clone)]
pub enum BudgetCheckResult {
    /// Call is within budget; proceed without surfacing anything.
    Ok,
    /// Crossed a fractional warning threshold (e.g. 50%) for the first time.
    ThresholdWarning {
        /// Threshold percent that fired (e.g. `50`, `80`, `95`).
        pct: u32,
        /// Current cumulative cost in nano-USD.
        current: u64,
        /// Configured maximum session cost in nano-USD.
        limit: u64,
    },
    /// Over-budget but policy is [`BudgetExceedPolicy::WarnOnly`].
    Warn {
        /// Current cumulative cost in nano-USD.
        current: u64,
        /// Configured maximum session cost in nano-USD.
        limit: u64,
    },
    /// Over-budget; policy is [`BudgetExceedPolicy::AskUser`] — host must
    /// prompt the user before continuing.
    AskUser {
        /// Current cumulative cost in nano-USD.
        current: u64,
        /// Configured maximum session cost in nano-USD.
        limit: u64,
    },
    /// Over-budget; policy is [`BudgetExceedPolicy::Halt`] — stop now.
    Halt {
        /// Current cumulative cost in nano-USD.
        current: u64,
        /// Configured maximum session cost in nano-USD.
        limit: u64,
    },
}

impl BudgetEnforcer {
    /// Construct a new enforcer bound to `cost_tracker`.
    #[must_use]
    pub fn new(config: BudgetConfig, cost_tracker: Arc<CostTracker>) -> Self {
        Self {
            config,
            cost_tracker,
            warnings_fired: RwLock::new(HashSet::new()),
            realized_exceeded: AtomicBool::new(false),
        }
    }

    /// Pre-API call gate. After the call returns, call
    /// [`Self::check_post_api_call`] to latch the realized-exceeded flag if
    /// the actual cost overran.
    pub async fn check_pre_api_call(&self, estimated_cost_nano_usd: u64) -> BudgetCheckResult {
        if self.realized_exceeded.load(Ordering::Acquire) {
            let current = self.cost_tracker.total_nano_usd().await;
            let limit = self.config.max_session_nano_usd.unwrap_or(0);
            return BudgetCheckResult::Halt { current, limit };
        }
        let current = self.cost_tracker.total_nano_usd().await;
        let after = current.saturating_add(estimated_cost_nano_usd);
        if let Some(max) = self.config.max_session_nano_usd {
            if after > max {
                return match self.config.on_exceed {
                    BudgetExceedPolicy::Halt => BudgetCheckResult::Halt {
                        current,
                        limit: max,
                    },
                    BudgetExceedPolicy::AskUser => BudgetCheckResult::AskUser {
                        current,
                        limit: max,
                    },
                    BudgetExceedPolicy::WarnOnly => BudgetCheckResult::Warn {
                        current,
                        limit: max,
                    },
                };
            }
            // Threshold warning — cast to f64 only for the ratio comparison.
            #[allow(clippy::cast_precision_loss)]
            let ratio = after as f64 / max as f64;
            for &threshold in &self.config.warning_thresholds {
                if ratio >= threshold {
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let pct = (threshold * 100.0) as u32;
                    let mut fired = self.warnings_fired.write().await;
                    if fired.insert(pct) {
                        return BudgetCheckResult::ThresholdWarning {
                            pct,
                            current,
                            limit: max,
                        };
                    }
                }
            }
        }
        BudgetCheckResult::Ok
    }

    /// Latch the realized-exceeded flag if cumulative cost now exceeds the
    /// configured session limit. Subsequent [`Self::check_pre_api_call`]
    /// returns [`BudgetCheckResult::Halt`].
    pub async fn check_post_api_call(&self, _realized_cost: u64) {
        let total = self.cost_tracker.total_nano_usd().await;
        if let Some(max) = self.config.max_session_nano_usd {
            if total > max {
                self.realized_exceeded.store(true, Ordering::Release);
            }
        }
    }

    /// M3-05 entry point: latch the realized-exceeded flag AND emit budget
    /// alarm events if thresholds were crossed.
    ///
    /// - Fires `tengu_cost_budget_warning` at 80% (basis-points threshold
    ///   `BUDGET_WARNING_THRESHOLD_BPS`); guarded by `warnings_fired` so each
    ///   threshold fires at most once per session.
    /// - Fires `tengu_cost_budget_exceeded` at 100% (basis-points threshold
    ///   `BUDGET_EXCEEDED_THRESHOLD_BPS`); guarded by the existing
    ///   `realized_exceeded` `AtomicBool` so it fires at most once per session.
    /// - Without a bus (`None`), behaves identically to
    ///   [`Self::check_post_api_call`] (latches the flag, emits nothing).
    ///
    /// Existing M1/M2 callers should keep using [`Self::check_post_api_call`];
    /// new M3-05 callers (e.g. api-client integration in Task 7) pass
    /// `Some(&bus)`.
    pub async fn check_post_api_call_with_bus(
        &self,
        _realized_cost: u64,
        bus: Option<&Arc<lingxi_telemetry::AnalyticsBus>>,
    ) {
        let total = self.cost_tracker.total_nano_usd().await;
        let Some(max) = self.config.max_session_nano_usd else {
            // No limit configured — nothing to alarm on.
            return;
        };

        // ----- compute percent in basis points (no f64 in the threshold path) -----
        // Use u128 to avoid intermediate overflow: `total * 10_000` could exceed
        // u64 when total is near u64::MAX. Saturate on the way back down.
        let percent_bps_u128: u128 =
            (u128::from(total)).saturating_mul(10_000) / (u128::from(max).max(1));
        #[allow(clippy::cast_possible_truncation)]
        let percent_bps: u32 = if percent_bps_u128 > u128::from(u32::MAX) {
            u32::MAX
        } else {
            percent_bps_u128 as u32
        };

        // ----- 100% exceeded -----
        if percent_bps >= BUDGET_EXCEEDED_THRESHOLD_BPS {
            // Atomic-latch on realized_exceeded ensures idempotency.
            if !self.realized_exceeded.swap(true, Ordering::AcqRel) {
                if let Some(bus) = bus {
                    emit_budget_exceeded(bus, max, total).await;
                }
            }
        } else if percent_bps >= BUDGET_WARNING_THRESHOLD_BPS {
            // ----- 80% warning (fires once per session) -----
            // Reuse warnings_fired with a synthetic pct value of 80 so the same
            // dedupe set guards both M1 thresholds and the M3-05 BPS warning.
            let mut fired = self.warnings_fired.write().await;
            if fired.insert(80) {
                if let Some(bus) = bus {
                    emit_budget_warning(bus, max, total, percent_bps).await;
                }
            }
        }
    }
}

/// Emit `tengu_cost_budget_warning` with the 3-key spec-locked payload.
///
/// `percent_bps` is computed as `u32` here (basis points, max `10_000` in
/// practice) but M3-06's `BudgetWarningPayload::percent_bps: u64` is the
/// authoritative schema type. The `AnalyticsValue::Int(_ as i64)` cast at the
/// bus boundary is the documented payload-encoding convention shared by all
/// `tengu_*` numeric fields (basis points are always non-negative and well
/// below `i64::MAX`, so the cast is exact and round-trips losslessly back to
/// the `u64` schema field at the `StatsigSink` wire-encode site).
async fn emit_budget_warning(
    bus: &Arc<lingxi_telemetry::AnalyticsBus>,
    limit_nano_usd: u64,
    current_nano_usd: u64,
    percent_bps: u32,
) {
    use lingxi_telemetry::{AnalyticsValue, LogEventMetadata};
    let mut m = LogEventMetadata::new();
    m.insert(
        "limit_usd".into(),
        AnalyticsValue::Int(i64_from_u64_saturating(limit_nano_usd)),
    );
    m.insert(
        "current_usd".into(),
        AnalyticsValue::Int(i64_from_u64_saturating(current_nano_usd)),
    );
    m.insert(
        "percent_bps".into(),
        // i64::from(u32) is infallible; the bus-side i64 stays non-negative
        // and re-decodes to BudgetWarningPayload::percent_bps: u64 cleanly.
        AnalyticsValue::Int(i64::from(percent_bps)),
    );
    bus.log_event("tengu_cost_budget_warning", m).await;
}

/// Emit `tengu_cost_budget_exceeded` with the 2-key spec-locked payload.
async fn emit_budget_exceeded(
    bus: &Arc<lingxi_telemetry::AnalyticsBus>,
    limit_nano_usd: u64,
    current_nano_usd: u64,
) {
    use lingxi_telemetry::{AnalyticsValue, LogEventMetadata};
    let mut m = LogEventMetadata::new();
    m.insert(
        "limit_usd".into(),
        AnalyticsValue::Int(i64_from_u64_saturating(limit_nano_usd)),
    );
    m.insert(
        "current_usd".into(),
        AnalyticsValue::Int(i64_from_u64_saturating(current_nano_usd)),
    );
    bus.log_event("tengu_cost_budget_exceeded", m).await;
}

#[inline]
#[allow(clippy::cast_possible_wrap)]
const fn i64_from_u64_saturating(v: u64) -> i64 {
    if v > i64::MAX as u64 {
        i64::MAX
    } else {
        v as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pricing::{nano_usd_to_dollars_format, CostError, PricingCatalog, ProviderId};
    use crate::usage::{TokenUsage, Usage};
    use crate::ModelRef;
    use async_trait::async_trait;
    use lingxi_protocol::SessionId;
    use lingxi_telemetry::{
        AnalyticsBus, AnalyticsSink, AnalyticsValue, LogEventMetadata,
    };
    use std::sync::Mutex;
    use std::time::Duration;
    use tokio::sync::mpsc;

    fn make_tracker() -> Arc<CostTracker> {
        let (tx, _rx) = mpsc::channel(8);
        Arc::new(CostTracker::new(
            SessionId::nil(),
            Arc::new(PricingCatalog::builtin_reference()),
            tx,
        ))
    }

    #[tokio::test]
    async fn under_budget_returns_ok() {
        let cfg = BudgetConfig {
            max_session_nano_usd: Some(1_000_000_000),
            max_turn_nano_usd: None,
            max_turn_tokens: None,
            warning_thresholds: vec![0.5, 0.8, 0.95],
            on_exceed: BudgetExceedPolicy::Halt,
        };
        let e = BudgetEnforcer::new(cfg, make_tracker());
        assert!(matches!(
            e.check_pre_api_call(10_000_000).await,
            BudgetCheckResult::Ok
        ));
    }

    #[tokio::test]
    async fn over_budget_halts_with_halt_policy() {
        let cfg = BudgetConfig {
            max_session_nano_usd: Some(1_000),
            max_turn_nano_usd: None,
            max_turn_tokens: None,
            warning_thresholds: vec![],
            on_exceed: BudgetExceedPolicy::Halt,
        };
        let e = BudgetEnforcer::new(cfg, make_tracker());
        assert!(matches!(
            e.check_pre_api_call(10_000).await,
            BudgetCheckResult::Halt { .. }
        ));
    }

    // -----------------------------------------------------------------
    // M3-05 Task 4: budget alarm emission at 80% / 100% thresholds.
    // -----------------------------------------------------------------

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

    async fn make_setup(
        limit_nano_usd: u64,
    ) -> (
        Arc<CostTracker>,
        BudgetEnforcer,
        Arc<AnalyticsBus>,
        Arc<CaptureSink>,
    ) {
        let (tx, _rx) = mpsc::channel(8);
        let tracker = Arc::new(CostTracker::new(
            SessionId::nil(),
            Arc::new(PricingCatalog::builtin_reference()),
            tx,
        ));
        let cfg = BudgetConfig {
            max_session_nano_usd: Some(limit_nano_usd),
            max_turn_nano_usd: None,
            max_turn_tokens: None,
            warning_thresholds: vec![], // M3-05 uses BPS thresholds, not the M1 f64 list
            on_exceed: BudgetExceedPolicy::Halt,
        };
        let enforcer = BudgetEnforcer::new(cfg, tracker.clone());
        let bus = Arc::new(AnalyticsBus::new());
        let sink: Arc<CaptureSink> = Arc::new(CaptureSink::default());
        bus.attach_sink(sink.clone() as Arc<dyn AnalyticsSink>)
            .await;
        (tracker, enforcer, bus, sink)
    }

    #[test]
    fn alarm_threshold_constants_match_spec() {
        assert_eq!(BUDGET_WARNING_THRESHOLD_BPS, 8000_u32, "80% in basis points");
        assert_eq!(
            BUDGET_EXCEEDED_THRESHOLD_BPS, 10000_u32,
            "100% in basis points"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn alarm_warning_fires_at_80_percent_with_locked_payload() {
        let (tracker, enforcer, bus, sink) = make_setup(1_000_000_000).await; // $1.00 limit

        // 160_000 input tokens * 5000 nano-USD/tok = 800_000_000 nano-USD = 80%.
        let mr = ModelRef {
            provider: ProviderId::Anthropic,
            model: "claude-opus-4-6".into(),
        };
        tracker
            .record_api_response_v2(
                mr,
                Usage {
                    tokens: TokenUsage {
                        input: 160_000,
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

        enforcer.check_post_api_call_with_bus(0, Some(&bus)).await;

        let events = sink.events.lock().unwrap();
        let warnings: Vec<_> = events
            .iter()
            .filter(|(n, _)| n == "tengu_cost_budget_warning")
            .collect();
        assert_eq!(warnings.len(), 1, "exactly one warning fires at 80%");

        let payload = &warnings[0].1;
        assert_eq!(payload.len(), 3, "warning payload has exactly 3 keys");
        assert!(payload.contains_key("limit_usd"));
        assert!(payload.contains_key("current_usd"));
        assert!(payload.contains_key("percent_bps"));

        match &payload["limit_usd"] {
            AnalyticsValue::Int(n) => assert_eq!(*n, 1_000_000_000),
            other => panic!("limit_usd must be Int, got {other:?}"),
        }
        match &payload["current_usd"] {
            AnalyticsValue::Int(n) => assert_eq!(*n, 800_000_000),
            other => panic!("current_usd must be Int, got {other:?}"),
        }
        match &payload["percent_bps"] {
            AnalyticsValue::Int(n) => assert_eq!(*n, 8000, "exactly 80% in basis points"),
            other => panic!("percent_bps must be Int, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn alarm_warning_does_not_double_emit_within_session() {
        let (tracker, enforcer, bus, sink) = make_setup(1_000_000_000).await;

        // Trip to 80% twice (each post-call check is a separate invocation).
        let mr = ModelRef {
            provider: ProviderId::Anthropic,
            model: "claude-opus-4-6".into(),
        };
        for _ in 0..2 {
            tracker
                .record_api_response_v2(
                    mr.clone(),
                    Usage {
                        tokens: TokenUsage {
                            input: 80_000,
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
            enforcer.check_post_api_call_with_bus(0, Some(&bus)).await;
        }

        let events = sink.events.lock().unwrap();
        let warnings = events
            .iter()
            .filter(|(n, _)| n == "tengu_cost_budget_warning")
            .count();
        assert_eq!(warnings, 1, "warning fires exactly once per session");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn alarm_exceeded_fires_at_100_percent_with_locked_payload() {
        let (tracker, enforcer, bus, sink) = make_setup(1_000_000_000).await;

        let mr = ModelRef {
            provider: ProviderId::Anthropic,
            model: "claude-opus-4-6".into(),
        };
        // 200_000 input tokens * 5000 = 1_000_000_000 nano-USD = 100%.
        tracker
            .record_api_response_v2(
                mr,
                Usage {
                    tokens: TokenUsage {
                        input: 200_000,
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
        enforcer.check_post_api_call_with_bus(0, Some(&bus)).await;

        let events = sink.events.lock().unwrap();
        let exceeded: Vec<_> = events
            .iter()
            .filter(|(n, _)| n == "tengu_cost_budget_exceeded")
            .collect();
        assert_eq!(exceeded.len(), 1, "exactly one exceeded fires at 100%");

        let payload = &exceeded[0].1;
        assert_eq!(payload.len(), 2, "exceeded payload has exactly 2 keys");
        assert!(payload.contains_key("limit_usd"));
        assert!(payload.contains_key("current_usd"));

        match &payload["limit_usd"] {
            AnalyticsValue::Int(n) => assert_eq!(*n, 1_000_000_000),
            other => panic!("limit_usd must be Int, got {other:?}"),
        }
        match &payload["current_usd"] {
            AnalyticsValue::Int(n) => assert_eq!(*n, 1_000_000_000),
            other => panic!("current_usd must be Int, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn alarm_exceeded_idempotent_no_double_emit() {
        let (tracker, enforcer, bus, sink) = make_setup(1_000_000_000).await;

        let mr = ModelRef {
            provider: ProviderId::Anthropic,
            model: "claude-opus-4-6".into(),
        };
        // Two over-budget calls back-to-back. Exceeded must still fire exactly once.
        for _ in 0..2 {
            tracker
                .record_api_response_v2(
                    mr.clone(),
                    Usage {
                        tokens: TokenUsage {
                            input: 200_000,
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
            enforcer.check_post_api_call_with_bus(0, Some(&bus)).await;
        }

        let events = sink.events.lock().unwrap();
        let exceeded = events
            .iter()
            .filter(|(n, _)| n == "tengu_cost_budget_exceeded")
            .count();
        assert_eq!(exceeded, 1, "exceeded fires exactly once thanks to atomic latch");
    }

    #[test]
    fn alarm_budget_exceeded_error_string_byte_for_byte() {
        let e = CostError::BudgetExceeded {
            limit: 100.00,
            current: 150.75,
        };
        assert_eq!(
            e.to_string(),
            "Budget exceeded ($150.75); stopped.",
            "claude-code parity: spec §5 line 498",
        );
    }

    #[test]
    fn alarm_nano_usd_to_dollars_format_matches_spec_examples() {
        assert_eq!(nano_usd_to_dollars_format(1_500_000_000), "$1.50");
        assert_eq!(nano_usd_to_dollars_format(12_345_678_901), "$12.35");
        assert_eq!(nano_usd_to_dollars_format(0), "$0.00");
        assert_eq!(nano_usd_to_dollars_format(999_999_999), "$1.00", "rounding edge");
    }
}

//! End-to-end cost pipeline: 3 simulated API responses trip
//! `tengu_cost_recorded` × 3 + `tengu_cost_budget_warning` + `tengu_cost_budget_exceeded`
//! in the locked order.

use async_trait::async_trait;
use lingxi_cost::{
    BudgetConfig, BudgetEnforcer, BudgetExceedPolicy, CostTracker, ModelRef, PricingCatalog,
    ProviderId, TokenUsage, Usage,
};
use lingxi_protocol::SessionId;
use lingxi_telemetry::{AnalyticsBus, AnalyticsSink, LogEventMetadata};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;

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

#[tokio::test]
async fn three_calls_trigger_warning_then_exceeded() {
    // $1.00 limit. Each call is 100_000 tokens * 5_000 = 500_000_000 nano-USD = $0.50.
    // 3 calls → 1.5 limit; warning fires after call 2 (50% → 100% crosses 80%);
    // exceeded fires after call 2 (since 2 calls = $1.00 exactly = 100%).
    //
    // Adjust: use 80_000 tokens per call ($0.40 each).
    //   After call 1: $0.40 (40%)              — no alarm
    //   After call 2: $0.80 (80%)              — warning fires
    //   After call 3: $1.20 (120%, saturates)  — exceeded fires

    let (tx, _rx) = mpsc::channel(64);
    let tracker = Arc::new(CostTracker::new(
        SessionId::nil(),
        Arc::new(PricingCatalog::builtin_reference()),
        tx,
    ));
    let cfg = BudgetConfig {
        max_session_nano_usd: Some(1_000_000_000), // $1.00
        max_turn_nano_usd: None,
        max_turn_tokens: None,
        warning_thresholds: vec![], // M3-05 BPS path only
        on_exceed: BudgetExceedPolicy::Halt,
    };
    let enforcer = BudgetEnforcer::new(cfg, tracker.clone());

    let bus = Arc::new(AnalyticsBus::new());
    let sink: Arc<CaptureSink> = Arc::new(CaptureSink::default());
    bus.attach_sink(sink.clone() as Arc<dyn AnalyticsSink>).await;

    let mr = ModelRef {
        provider: ProviderId::Anthropic,
        model: "claude-opus-4-6".into(),
    };

    for _ in 0..3 {
        // Record one call.
        let cost = tracker
            .record_api_response_v2(
                mr.clone(),
                Usage {
                    tokens: TokenUsage {
                        input: 80_000,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                Duration::from_millis(50),
                0,
                0,
                0,
                false,
                Some(&bus),
            )
            .await;
        assert_eq!(cost, 400_000_000, "$0.40 per call");
        // Then the post-call budget gate.
        enforcer.check_post_api_call_with_bus(cost, Some(&bus)).await;
    }

    let events = sink.events.lock().unwrap();
    let names: Vec<&str> = events.iter().map(|(n, _)| n.as_str()).collect();

    // 3 cost_recorded + 1 budget_warning + 1 budget_exceeded = 5 events total.
    assert_eq!(events.len(), 5, "expected 5 events; got {names:?}");

    // tengu_cost_recorded fires three times.
    assert_eq!(
        names.iter().filter(|n| **n == "tengu_cost_recorded").count(),
        3,
        "three cost recordings"
    );
    // tengu_cost_budget_warning fires exactly once.
    assert_eq!(
        names
            .iter()
            .filter(|n| **n == "tengu_cost_budget_warning")
            .count(),
        1,
        "one 80% warning"
    );
    // tengu_cost_budget_exceeded fires exactly once.
    assert_eq!(
        names
            .iter()
            .filter(|n| **n == "tengu_cost_budget_exceeded")
            .count(),
        1,
        "one 100% exceeded"
    );

    // Ordering: cost_recorded comes BEFORE the threshold alarm in each tripping call.
    // Call 1: cost_recorded (40%) — no alarm
    // Call 2: cost_recorded (80%), warning (80%)
    // Call 3: cost_recorded (120%), exceeded (>=100%)
    assert_eq!(names[0], "tengu_cost_recorded", "call 1");
    assert_eq!(names[1], "tengu_cost_recorded", "call 2 record");
    assert_eq!(names[2], "tengu_cost_budget_warning", "call 2 warning");
    assert_eq!(names[3], "tengu_cost_recorded", "call 3 record");
    assert_eq!(names[4], "tengu_cost_budget_exceeded", "call 3 exceeded");
}

#[tokio::test]
async fn summary_after_pipeline_reflects_all_three_calls() {
    let (tx, _rx) = mpsc::channel(64);
    let tracker = Arc::new(CostTracker::new(
        SessionId::nil(),
        Arc::new(PricingCatalog::builtin_reference()),
        tx,
    ));
    let mr = ModelRef {
        provider: ProviderId::Anthropic,
        model: "claude-opus-4-6".into(),
    };
    for _ in 0..3 {
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
                Duration::from_millis(50),
                0,
                0,
                0,
                false,
                None,
            )
            .await;
    }
    let summary = tracker.summary().await;
    assert_eq!(summary.session.total_nano_usd, 1_200_000_000, "$1.20 cumulative");
    assert_eq!(summary.by_model.len(), 1);
}

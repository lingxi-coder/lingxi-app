//! Verifies `tengu_cost_recorded` fires with the spec-locked field set
//! (`model`, `input_tokens`, `output_tokens`, `cache_read_input_tokens`,
//! `cache_creation_input_tokens`, `cost_usd`, `session_id`, `is_batch_request`).

use async_trait::async_trait;
use lingxi_cost::emit_cost_recorded;
use lingxi_protocol::SessionId;
use lingxi_telemetry::{AnalyticsBus, AnalyticsSink, AnalyticsValue, LogEventMetadata};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct CaptureSink {
    events: Mutex<Vec<(String, LogEventMetadata)>>,
}

#[async_trait]
impl AnalyticsSink for CaptureSink {
    async fn log_event(&self, name: &str, metadata: LogEventMetadata) {
        self.events
            .lock()
            .expect("capture sink lock poisoned")
            .push((name.into(), metadata));
    }
    async fn log_event_async(&self, name: &str, metadata: LogEventMetadata) {
        self.events
            .lock()
            .expect("capture sink lock poisoned")
            .push((name.into(), metadata));
    }
    fn name(&self) -> &str {
        "capture"
    }
}

#[tokio::test]
async fn emit_cost_recorded_fires_with_locked_payload() {
    let bus = Arc::new(AnalyticsBus::new());
    let sink: Arc<CaptureSink> = Arc::new(CaptureSink::default());
    bus.attach_sink(sink.clone() as Arc<dyn AnalyticsSink>).await;

    let session = SessionId::nil();
    emit_cost_recorded(
        &bus,
        "claude-opus-4-6",
        1_000,  // input
        500,    // output
        128,    // cache_read_input_tokens
        64,     // cache_creation_input_tokens
        17_500_000, // cost in nano-USD (= $0.0175)
        &session,
        false, // is_batch_request — ALWAYS false in M3
    )
    .await;

    let events = sink.events.lock().unwrap();
    assert_eq!(events.len(), 1, "exactly one event must fire");
    let (name, payload) = &events[0];
    assert_eq!(name, "tengu_cost_recorded", "event name must match spec byte-for-byte");

    // All 8 spec-locked keys must be present.
    let expected_keys = [
        "model",
        "input_tokens",
        "output_tokens",
        "cache_read_input_tokens",
        "cache_creation_input_tokens",
        "cost_usd",
        "session_id",
        "is_batch_request",
    ];
    for k in &expected_keys {
        assert!(
            payload.contains_key(*k),
            "payload must contain key {k}; got {:?}",
            payload.keys().collect::<Vec<_>>(),
        );
    }
    assert_eq!(
        payload.len(),
        expected_keys.len(),
        "payload must contain EXACTLY the spec keys; extras: {:?}",
        payload
            .keys()
            .filter(|k| !expected_keys.contains(&k.as_str()))
            .collect::<Vec<_>>(),
    );

    // Value-shape assertions.
    match &payload["model"] {
        AnalyticsValue::String(s) => assert_eq!(s, "claude-opus-4-6"),
        other => panic!("model must be String, got {other:?}"),
    }
    match &payload["input_tokens"] {
        AnalyticsValue::Int(n) => assert_eq!(*n, 1_000),
        other => panic!("input_tokens must be Int, got {other:?}"),
    }
    match &payload["output_tokens"] {
        AnalyticsValue::Int(n) => assert_eq!(*n, 500),
        other => panic!("output_tokens must be Int, got {other:?}"),
    }
    match &payload["cache_read_input_tokens"] {
        AnalyticsValue::Int(n) => assert_eq!(*n, 128),
        other => panic!("cache_read_input_tokens must be Int, got {other:?}"),
    }
    match &payload["cache_creation_input_tokens"] {
        AnalyticsValue::Int(n) => assert_eq!(*n, 64),
        other => panic!("cache_creation_input_tokens must be Int, got {other:?}"),
    }
    match &payload["cost_usd"] {
        AnalyticsValue::Int(n) => assert_eq!(*n, 17_500_000, "cost_usd is nano-USD per v3 §17"),
        other => panic!("cost_usd must be Int, got {other:?}"),
    }
    match &payload["session_id"] {
        AnalyticsValue::String(s) => assert_eq!(s, &session.to_string()),
        other => panic!("session_id must be String, got {other:?}"),
    }
    match &payload["is_batch_request"] {
        AnalyticsValue::Bool(b) => assert!(!*b, "is_batch_request must be false in M3"),
        other => panic!("is_batch_request must be Bool, got {other:?}"),
    }
}

#[tokio::test]
async fn emit_cost_recorded_handles_zero_cost() {
    // Edge case: unpriced model → cost_nano_usd = 0. The event STILL fires.
    let bus = Arc::new(AnalyticsBus::new());
    let sink: Arc<CaptureSink> = Arc::new(CaptureSink::default());
    bus.attach_sink(sink.clone() as Arc<dyn AnalyticsSink>).await;

    emit_cost_recorded(
        &bus,
        "claude-unknown",
        100,
        50,
        0,
        0,
        0,
        &SessionId::nil(),
        false,
    )
    .await;

    let events = sink.events.lock().unwrap();
    assert_eq!(events.len(), 1, "event must fire even at zero cost");
    match &events[0].1["cost_usd"] {
        AnalyticsValue::Int(n) => assert_eq!(*n, 0),
        other => panic!("cost_usd must be Int(0), got {other:?}"),
    }
}

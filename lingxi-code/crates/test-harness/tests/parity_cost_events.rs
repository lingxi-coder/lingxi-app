//! Parity fixture: `tengu_cost_*` event names + payload shapes locked against
//! the M3 spec (`docs/superpowers/specs/2026-05-23-m3-engine-completion-design.md` §7).
//!
//! Drift on this fixture breaks downstream `BigQuery` / Statsig dashboards that
//! join on `model` + `session_id` + `cost_usd` so a parity test is the right
//! place to lock it.

use async_trait::async_trait;
use lingxi_cost::{
    emit_cost_recorded, BATCH_DISCOUNT_BPS, BUDGET_EXCEEDED_THRESHOLD_BPS,
    BUDGET_WARNING_THRESHOLD_BPS,
};
use lingxi_protocol::SessionId;
use lingxi_telemetry::{AnalyticsBus, AnalyticsSink, AnalyticsValue, LogEventMetadata};
use lingxi_test_harness::parity::load_fixture;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Deserialize)]
struct Fixture {
    event_names: EventNames,
    thresholds_bps: Thresholds,
    canonical_cost_recorded_input: CanonicalInput,
    expected_payload_keys_in_declaration_order: Vec<String>,
    expected_payload_values: HashMap<String, serde_json::Value>,
    budget_warning_payload_keys: Vec<String>,
    budget_exceeded_payload_keys: Vec<String>,
}

#[derive(Deserialize)]
struct EventNames {
    cost_recorded: String,
    budget_warning: String,
    budget_exceeded: String,
}

#[derive(Deserialize)]
struct Thresholds {
    warning: u32,
    exceeded: u32,
    batch_discount: u32,
}

#[derive(Deserialize)]
struct CanonicalInput {
    model: String,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_input_tokens: u64,
    cache_creation_input_tokens: u64,
    cost_nano_usd: u64,
    session_id: String,
    is_batch_request: bool,
}

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

#[test]
fn event_names_match_fixture_byte_for_byte() {
    let fx: Fixture = load_fixture("cost_events");
    assert_eq!(
        fx.event_names.cost_recorded, "tengu_cost_recorded",
        "fixture must declare the spec-locked cost_recorded event name"
    );
    assert_eq!(fx.event_names.budget_warning, "tengu_cost_budget_warning");
    assert_eq!(fx.event_names.budget_exceeded, "tengu_cost_budget_exceeded");
}

#[test]
fn thresholds_match_constants_byte_for_byte() {
    let fx: Fixture = load_fixture("cost_events");
    assert_eq!(
        fx.thresholds_bps.warning, BUDGET_WARNING_THRESHOLD_BPS,
        "fixture warning threshold must equal BUDGET_WARNING_THRESHOLD_BPS"
    );
    assert_eq!(
        fx.thresholds_bps.exceeded, BUDGET_EXCEEDED_THRESHOLD_BPS,
        "fixture exceeded threshold must equal BUDGET_EXCEEDED_THRESHOLD_BPS"
    );
    assert_eq!(
        fx.thresholds_bps.batch_discount, BATCH_DISCOUNT_BPS,
        "fixture batch_discount must equal BATCH_DISCOUNT_BPS"
    );
}

#[tokio::test]
async fn cost_recorded_payload_matches_fixture_byte_for_byte() {
    let fx: Fixture = load_fixture("cost_events");

    let bus = Arc::new(AnalyticsBus::new());
    let sink: Arc<CaptureSink> = Arc::new(CaptureSink::default());
    bus.attach_sink(sink.clone() as Arc<dyn AnalyticsSink>)
        .await;

    // SessionId::nil().to_string() = "sess:00000000-0000-0000-0000-000000000000"
    // (the `sess:` prefix is part of the Display impl in lingxi-protocol::ids).
    let session = SessionId::nil();
    assert_eq!(
        session.to_string(),
        fx.canonical_cost_recorded_input.session_id,
        "SessionId::nil().to_string() must match fixture",
    );

    emit_cost_recorded(
        &bus,
        &fx.canonical_cost_recorded_input.model,
        fx.canonical_cost_recorded_input.input_tokens,
        fx.canonical_cost_recorded_input.output_tokens,
        fx.canonical_cost_recorded_input.cache_read_input_tokens,
        fx.canonical_cost_recorded_input.cache_creation_input_tokens,
        fx.canonical_cost_recorded_input.cost_nano_usd,
        &session,
        fx.canonical_cost_recorded_input.is_batch_request,
    )
    .await;

    let events = sink.events.lock().unwrap();
    assert_eq!(events.len(), 1, "exactly one cost event");
    assert_eq!(events[0].0, fx.event_names.cost_recorded);
    let payload = &events[0].1;

    // Every expected key is present and matches its expected value.
    for key in &fx.expected_payload_keys_in_declaration_order {
        let actual = payload
            .get(key)
            .unwrap_or_else(|| panic!("payload missing key {key}"));
        let expected = fx
            .expected_payload_values
            .get(key)
            .unwrap_or_else(|| panic!("fixture missing expected value for key {key}"));
        match (actual, expected) {
            (AnalyticsValue::String(a), serde_json::Value::String(e)) => {
                assert_eq!(a, e, "key {key} string mismatch");
            }
            (AnalyticsValue::Int(a), serde_json::Value::Number(e)) => {
                let e_i64 = e.as_i64().expect("fixture int");
                assert_eq!(*a, e_i64, "key {key} int mismatch");
            }
            (AnalyticsValue::Bool(a), serde_json::Value::Bool(e)) => {
                assert_eq!(*a, *e, "key {key} bool mismatch");
            }
            (a, e) => panic!("key {key}: type mismatch — got {a:?}, fixture {e:?}"),
        }
    }
    // No extra keys.
    assert_eq!(
        payload.len(),
        fx.expected_payload_keys_in_declaration_order.len(),
        "payload has extra keys: {:?}",
        payload
            .keys()
            .filter(|k| !fx.expected_payload_keys_in_declaration_order.contains(k))
            .collect::<Vec<_>>(),
    );
}

#[test]
fn fixture_declares_budget_payload_key_sets() {
    let fx: Fixture = load_fixture("cost_events");
    assert_eq!(
        fx.budget_warning_payload_keys,
        vec!["limit_usd", "current_usd", "percent_bps"],
        "warning payload has exactly 3 keys per spec §7 line 735"
    );
    assert_eq!(
        fx.budget_exceeded_payload_keys,
        vec!["limit_usd", "current_usd"],
        "exceeded payload has exactly 2 keys per spec §7 line 736"
    );
}

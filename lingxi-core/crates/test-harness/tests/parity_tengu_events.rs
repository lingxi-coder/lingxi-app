//! Parity fixture: lock the full 143-name `tengu_*` event-name registry.
//!
//! v3 §32.6 fixture protocol. The fixture is the single source of truth for
//! the M3-06 event-name list. Any drift from `ALL_EVENT_NAMES` indicates an
//! append-only-violation per spec §7 line 787-789.

use lingxi_test_harness::parity::load_fixture;
use serde::Deserialize;

#[derive(Deserialize)]
struct SamplePayload {
    event_name: String,
    statsig_wire: serde_json::Value,
}

#[derive(Deserialize)]
struct Fixture {
    event_names: Vec<String>,
    cost_sample_payloads: Vec<SamplePayload>,
}

#[test]
fn event_names_match_registry_byte_for_byte() {
    let fx: Fixture = load_fixture("tengu_events");
    let registry = lingxi_telemetry::tengu::ALL_EVENT_NAMES;
    assert_eq!(
        fx.event_names.len(),
        registry.len(),
        "fixture has {} names; registry has {}",
        fx.event_names.len(),
        registry.len(),
    );
    for (i, (f, r)) in fx.event_names.iter().zip(registry.iter()).enumerate() {
        assert_eq!(
            f, *r,
            "position {i} differs: fixture {f:?} vs registry {r:?}",
        );
    }
}

#[test]
fn cost_sample_payloads_match_mock_statsig_wire() {
    use lingxi_protocol::Secret;
    use lingxi_telemetry::{AnalyticsValue, LogEventMetadata, MockStatsigSink};
    use std::collections::HashMap;

    let fx: Fixture = load_fixture("tengu_events");
    let sink = MockStatsigSink::new(Secret::new("test-sdk-key".into()));

    for sample in &fx.cost_sample_payloads {
        // Reconstruct the LogEventMetadata from the sample's metadata object.
        let metadata_obj = sample
            .statsig_wire
            .get("metadata")
            .and_then(serde_json::Value::as_object)
            .expect("sample.statsig_wire.metadata is an object");
        let mut md: LogEventMetadata = HashMap::new();
        for (k, v) in metadata_obj {
            let av = match v {
                serde_json::Value::Bool(b) => AnalyticsValue::Bool(*b),
                serde_json::Value::Number(n) if n.is_i64() => {
                    AnalyticsValue::Int(n.as_i64().unwrap())
                }
                serde_json::Value::Number(n) if n.is_f64() => {
                    AnalyticsValue::Float(n.as_f64().unwrap())
                }
                serde_json::Value::String(s) => AnalyticsValue::String(s.clone()),
                serde_json::Value::Null => AnalyticsValue::None,
                other => panic!("unsupported metadata value: {other:?}"),
            };
            md.insert(k.clone(), av);
        }
        let value = sample
            .statsig_wire
            .get("value")
            .and_then(serde_json::Value::as_f64);
        let wire = sink.statsig_wire_payload(&sample.event_name, &md, value);
        assert_eq!(
            wire, sample.statsig_wire,
            "statsig wire JSON for {} drifted from fixture",
            sample.event_name,
        );
    }
}

//! `NoOpSink` + `InMemorySink` + `with_default_sink` behavior.

use lingxi_telemetry::{AnalyticsBus, AnalyticsSink, AnalyticsValue, InMemorySink, NoOpSink};
use std::collections::HashMap;
use std::sync::Arc;

#[tokio::test]
async fn noop_sink_swallows_events_silently() {
    let sink = NoOpSink;
    let mut md = HashMap::new();
    md.insert("k".into(), AnalyticsValue::Int(1));
    sink.log_event("tengu_api_request_started", md).await;
    // No panic = pass; NoOpSink has no observable state.
    assert_eq!(sink.name(), "noop");
}

#[tokio::test]
async fn in_memory_sink_captures_events_in_order() {
    let sink = Arc::new(InMemorySink::new());
    let mut md1 = HashMap::new();
    md1.insert("x".into(), AnalyticsValue::Int(1));
    let mut md2 = HashMap::new();
    md2.insert("y".into(), AnalyticsValue::Int(2));

    sink.log_event("evt_one", md1).await;
    sink.log_event("evt_two", md2).await;

    let events = sink.events().await;
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].name, "evt_one");
    assert_eq!(events[1].name, "evt_two");
}

#[tokio::test]
async fn in_memory_sink_clear_drops_state() {
    let sink = Arc::new(InMemorySink::new());
    sink.log_event("evt", HashMap::new()).await;
    assert_eq!(sink.events().await.len(), 1);
    sink.clear().await;
    assert_eq!(sink.events().await.len(), 0);
}

#[tokio::test]
async fn analytics_bus_with_default_sink_is_noop() {
    let bus = AnalyticsBus::with_default_sink();
    // No buffer drain needed; sink is attached at construction.
    bus.log_event("evt", HashMap::new()).await;
    // The bus has a NoOp sink; we can't observe through it but we can confirm
    // no panic and the bus is in the attached state by attaching another sink
    // (which would fail noisily if the first attach was buggy).
    let in_mem = Arc::new(InMemorySink::new());
    bus.attach_sink(in_mem.clone()).await;
    bus.log_event("after_swap", HashMap::new()).await;
    let events = in_mem.events().await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].name, "after_swap");
}

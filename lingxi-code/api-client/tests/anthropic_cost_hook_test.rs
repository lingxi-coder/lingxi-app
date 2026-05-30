//! Integration test: a successful 200 response triggers
//! `tracker.record_api_response_v2(...)` and emits `tengu_cost_recorded`.

use api_client::anthropic::AnthropicProvider;
use async_trait::async_trait;
use cost::{CostTracker, PricingCatalog};
use protocol::{ContentBlock, ConversationMessage, MessageId, SessionId};
use std::sync::{Arc, Mutex};
use telemetry::{AnalyticsBus, AnalyticsSink, AnalyticsValue, LogEventMetadata};
use tokio::sync::mpsc;

mod mock_server;
use mock_server::{spawn_mock, MockResp};

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

fn msgs() -> Vec<ConversationMessage> {
    vec![ConversationMessage::User {
        id: MessageId::new(),
        content: vec![ContentBlock::Text { text: "hi".into() }],
    }]
}

#[tokio::test]
async fn successful_200_fires_cost_recorded_after_api_succeeded() {
    let server = spawn_mock(vec![MockResp {
        status: 200,
        body: r#"{"id":"msg_01","model":"claude-opus-4-6","content":[{"type":"text","text":"hi"}],"stop_reason":"end_turn","usage":{"input_tokens":1000,"output_tokens":500,"cache_read_input_tokens":128,"cache_creation_input_tokens":64}}"#
            .into(),
        headers: vec![],
    }])
    .await;

    let bus = Arc::new(AnalyticsBus::new());
    let sink: Arc<CaptureSink> = Arc::new(CaptureSink::default());
    bus.attach_sink(sink.clone() as Arc<dyn AnalyticsSink>)
        .await;

    let (tx, _rx) = mpsc::channel(8);
    let tracker = Arc::new(CostTracker::new(
        SessionId::nil(),
        Arc::new(PricingCatalog::builtin_reference()),
        tx,
    ));

    let provider = AnthropicProvider::new("sk-test", Some(server.base_url.clone()))
        .with_bus(bus.clone())
        .with_cost_tracker(tracker.clone());

    let _ = provider
        .messages_create_non_stream("claude-opus-4-6", None, msgs(), &*server.transport())
        .await
        .expect("happy path 200");

    // Ordering: tengu_api_request_succeeded BEFORE tengu_cost_recorded.
    // Scope the MutexGuard so it never crosses the trailing `.await` below.
    {
        let events = sink.events.lock().unwrap();
        let names: Vec<&str> = events.iter().map(|(n, _)| n.as_str()).collect();
        let succeeded_idx = names
            .iter()
            .position(|n| *n == "tengu_api_request_succeeded")
            .expect("tengu_api_request_succeeded must fire");
        let recorded_idx = names
            .iter()
            .position(|n| *n == "tengu_cost_recorded")
            .expect("tengu_cost_recorded must fire");
        assert!(
            succeeded_idx < recorded_idx,
            "tengu_api_request_succeeded must precede tengu_cost_recorded; got {names:?}"
        );

        // The cost event payload reflects the mock-server usage values.
        let (_, payload) = &events[recorded_idx];
        match &payload["input_tokens"] {
            AnalyticsValue::Int(n) => assert_eq!(*n, 1000),
            other => panic!("input_tokens must be Int(1000), got {other:?}"),
        }
        match &payload["output_tokens"] {
            AnalyticsValue::Int(n) => assert_eq!(*n, 500),
            other => panic!("output_tokens must be Int(500), got {other:?}"),
        }
        match &payload["cache_read_input_tokens"] {
            AnalyticsValue::Int(n) => assert_eq!(*n, 128),
            other => panic!("cache_read_input_tokens must be Int(128), got {other:?}"),
        }
        match &payload["cache_creation_input_tokens"] {
            AnalyticsValue::Int(n) => assert_eq!(*n, 64),
            other => panic!("cache_creation_input_tokens must be Int(64), got {other:?}"),
        }
        match &payload["is_batch_request"] {
            AnalyticsValue::Bool(b) => assert!(!*b, "M3 always false"),
            other => panic!("is_batch_request must be Bool(false), got {other:?}"),
        }
    }

    server.shutdown().await;
}

#[tokio::test]
async fn cost_tracker_silently_skipped_without_bus() {
    // Tracker present, bus absent → cost state updates but no event fires.
    let server = spawn_mock(vec![MockResp {
        status: 200,
        body: r#"{"id":"msg_01","model":"claude-opus-4-6","content":[{"type":"text","text":"hi"}],"stop_reason":"end_turn","usage":{"input_tokens":100,"output_tokens":50}}"#
            .into(),
        headers: vec![],
    }])
    .await;

    let (tx, _rx) = mpsc::channel(8);
    let tracker = Arc::new(CostTracker::new(
        SessionId::nil(),
        Arc::new(PricingCatalog::builtin_reference()),
        tx,
    ));

    let provider = AnthropicProvider::new("sk-test", Some(server.base_url.clone()))
        .with_cost_tracker(tracker.clone());

    let _ = provider
        .messages_create_non_stream("claude-opus-4-6", None, msgs(), &*server.transport())
        .await
        .expect("happy path 200");

    // State updated: 100 * 5000 + 50 * 25000 = 1_750_000 nano-USD = $0.00175.
    assert_eq!(tracker.total_nano_usd().await, 1_750_000);
    server.shutdown().await;
}

#[tokio::test]
async fn no_tracker_no_cost_recording_no_event() {
    // Neither tracker nor bus → 200 succeeds, nothing happens cost-side.
    let server = spawn_mock(vec![MockResp {
        status: 200,
        body: r#"{"id":"msg_01","model":"claude-opus-4-6","content":[{"type":"text","text":"hi"}],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}}"#
            .into(),
        headers: vec![],
    }])
    .await;
    let provider = AnthropicProvider::new("sk-test", Some(server.base_url.clone()));
    let r = provider
        .messages_create_non_stream("claude-opus-4-6", None, msgs(), &*server.transport())
        .await;
    assert!(r.is_ok(), "still succeeds without tracker: {r:?}");
    server.shutdown().await;
}

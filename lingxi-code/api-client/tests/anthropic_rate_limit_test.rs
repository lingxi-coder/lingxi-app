//! Integration test: 429 with `Retry-After` sleeps and retries; telemetry events fire.

use api_client::anthropic::AnthropicProvider;
use api_client::ApiError;
use async_trait::async_trait;
use protocol::{ContentBlock, ConversationMessage, MessageId};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use telemetry::{AnalyticsBus, AnalyticsSink, AnalyticsValue, LogEventMetadata};

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
        content: vec![ContentBlock::Text { text: "x".into() }],
    }]
}

#[tokio::test]
async fn four29_with_retry_after_sleeps_and_retries() {
    let server = spawn_mock(vec![
        MockResp {
            status: 429,
            body: "slow down".into(),
            headers: vec![("Retry-After".into(), "1".into())],
        },
        MockResp {
            status: 200,
            body: r#"{"id":"msg_01","model":"claude-opus-4-6","content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}}"#.into(),
            headers: vec![],
        },
    ])
    .await;

    let bus = Arc::new(AnalyticsBus::new());
    let sink: Arc<CaptureSink> = Arc::new(CaptureSink::default());
    bus.attach_sink(sink.clone() as Arc<dyn AnalyticsSink>)
        .await;

    let p = AnthropicProvider::new("sk-test", Some(server.base_url.clone())).with_bus(bus.clone());
    let transport = server.transport();
    let start = Instant::now();
    let r = p
        .messages_create_non_stream("claude-opus-4-6", None, msgs(), transport.as_ref())
        .await;
    let elapsed = start.elapsed();
    assert!(r.is_ok(), "429+200 must succeed: {r:?}");
    assert!(
        elapsed.as_millis() >= 800,
        "must sleep at least ~0.8s before retry (Retry-After: 1, jitter floor 0.8); got {elapsed:?}",
    );

    // Telemetry events: started + rate_limited + succeeded.
    let events = sink.events.lock().unwrap().clone();
    let names: Vec<&str> = events.iter().map(|(n, _)| n.as_str()).collect();
    assert!(
        names.contains(&"tengu_api_request_started"),
        "missing tengu_api_request_started in {names:?}"
    );
    assert!(
        names.contains(&"tengu_api_rate_limited"),
        "missing tengu_api_rate_limited in {names:?}"
    );
    assert!(
        names.contains(&"tengu_api_request_succeeded"),
        "missing tengu_api_request_succeeded in {names:?}"
    );

    server.shutdown().await;
}

#[tokio::test]
async fn four29_without_header_falls_through_to_retry_backoff() {
    let server = spawn_mock(vec![
        MockResp { status: 429, body: "limit".into(), headers: vec![] },
        MockResp { status: 200, body: r#"{"id":"msg_01","model":"claude-opus-4-6","content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}}"#.into(), headers: vec![] },
    ])
    .await;
    let p = AnthropicProvider::new("sk-test", Some(server.base_url.clone()));
    let transport = server.transport();
    let r = p
        .messages_create_non_stream("claude-opus-4-6", None, msgs(), transport.as_ref())
        .await;
    assert!(r.is_ok(), "429+200 must succeed: {r:?}");
    server.shutdown().await;
}

#[tokio::test]
async fn failure_emits_request_failed_event() {
    let server = spawn_mock(vec![
        MockResp {
            status: 503,
            body: "boom".into(),
            headers: vec![],
        },
        MockResp {
            status: 503,
            body: "boom".into(),
            headers: vec![],
        },
        MockResp {
            status: 503,
            body: "boom".into(),
            headers: vec![],
        },
    ])
    .await;
    let bus = Arc::new(AnalyticsBus::new());
    let sink: Arc<CaptureSink> = Arc::new(CaptureSink::default());
    bus.attach_sink(sink.clone() as Arc<dyn AnalyticsSink>)
        .await;
    let p = AnthropicProvider::new("sk-test", Some(server.base_url.clone())).with_bus(bus);

    let transport = server.transport();
    let r = p
        .messages_create_non_stream("claude-opus-4-6", None, msgs(), transport.as_ref())
        .await;
    assert!(matches!(r, Err(ApiError::RetryExhausted { .. })));
    let names: Vec<String> = sink
        .events
        .lock()
        .unwrap()
        .iter()
        .map(|(n, _)| n.clone())
        .collect();
    assert!(
        names.contains(&"tengu_api_request_failed".to_string()),
        "missing tengu_api_request_failed in {names:?}"
    );
    // Locked: payload uses the `error_kind` key with `Verified` newtype.
    let (_, meta) = sink
        .events
        .lock()
        .unwrap()
        .iter()
        .find(|(n, _)| n == "tengu_api_request_failed")
        .cloned()
        .unwrap();
    assert!(meta.contains_key("error_kind"));
    assert!(meta.contains_key("model"));
    assert!(meta.contains_key("request_id"));
    match meta.get("status_code") {
        Some(AnalyticsValue::Int(503) | AnalyticsValue::None) => {}
        other => panic!("status_code must be 503 or None, got {other:?}"),
    }
    server.shutdown().await;
}

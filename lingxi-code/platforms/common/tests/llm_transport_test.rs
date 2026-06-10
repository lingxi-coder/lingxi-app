use std::sync::Mutex;

use async_trait::async_trait;
use platform_common::LlmTransportBridge;
use protocol::{HttpRequest, HttpResponse, SseEvent};
use traits::http::SseStream;
use traits::{HttpError, HttpTransport};

type ScriptedSse = Mutex<Option<Result<Vec<Result<SseEvent, HttpError>>, HttpError>>>;

#[derive(Default)]
struct FakeHttp {
    response: Mutex<Option<Result<HttpResponse, HttpError>>>,
    sse: ScriptedSse,
    seen: Mutex<Option<HttpRequest>>,
}

#[async_trait]
impl HttpTransport for FakeHttp {
    async fn request(&self, req: HttpRequest) -> Result<HttpResponse, HttpError> {
        *self.seen.lock().expect("seen") = Some(req);
        self.response.lock().expect("response").take().expect("scripted response")
    }

    async fn stream_sse(&self, req: HttpRequest) -> Result<SseStream, HttpError> {
        *self.seen.lock().expect("seen") = Some(req);
        let events = self.sse.lock().expect("sse").take().expect("scripted sse")?;
        Ok(Box::pin(futures_util::stream::iter(events)))
    }
}

fn provider_request() -> llm_client::ProviderRequest {
    let mut request = llm_client::ProviderRequest::post_json(
        "https://api.anthropic.com/v1/messages",
        serde_json::json!({"model": "m"}),
    );
    request.headers.insert("x-api-key".to_string(), "k".to_string());
    request
}

#[tokio::test]
async fn execute_maps_request_and_response() {
    let fake = FakeHttp::default();
    *fake.response.lock().unwrap() = Some(Ok(HttpResponse {
        status: 200,
        headers: vec![
            ("Request-Id".to_string(), "req_1".to_string()),
            ("Retry-After".to_string(), "7".to_string()),
        ],
        body: r#"{"id":"msg_1"}"#.to_string(),
    }));
    let bridge = LlmTransportBridge::new(fake);

    let response = llm_client::Transport::execute(&bridge, &provider_request())
        .await
        .expect("response");

    assert_eq!(response.status, 200);
    assert_eq!(response.headers.get("retry-after").map(String::as_str), Some("7"));
    assert_eq!(response.request_id.as_deref(), Some("req_1"));
    assert_eq!(response.body_json["id"], "msg_1");

    let seen = bridge.inner().seen.lock().unwrap().take().expect("request sent");
    assert!(matches!(seen.method, protocol::HttpMethod::Post));
    assert_eq!(seen.url, "https://api.anthropic.com/v1/messages");
    assert!(seen.headers.iter().any(|(k, v)| k == "x-api-key" && v == "k"));
    assert_eq!(seen.body.as_deref(), Some(r#"{"model":"m"}"#));
}

#[tokio::test]
async fn execute_passes_error_statuses_through_as_data() {
    let fake = FakeHttp::default();
    *fake.response.lock().unwrap() = Some(Ok(HttpResponse {
        status: 429,
        headers: vec![],
        body: r#"{"type":"error","error":{"type":"rate_limit_error","message":"slow"}}"#.to_string(),
    }));
    let bridge = LlmTransportBridge::new(fake);

    let response = llm_client::Transport::execute(&bridge, &provider_request())
        .await
        .expect("error status is data, not Err");

    assert_eq!(response.status, 429);
    assert_eq!(response.body_json["error"]["type"], "rate_limit_error");
}

#[tokio::test]
async fn http_status_error_variant_also_becomes_data() {
    let fake = FakeHttp::default();
    *fake.response.lock().unwrap() = Some(Err(HttpError::Status {
        status: 500,
        body: r#"{"type":"error","error":{"type":"api_error","message":"boom"}}"#.to_string(),
    }));
    let bridge = LlmTransportBridge::new(fake);

    let response = llm_client::Transport::execute(&bridge, &provider_request())
        .await
        .expect("status error is data");

    assert_eq!(response.status, 500);
    assert_eq!(response.body_json["error"]["type"], "api_error");
}

#[tokio::test]
async fn connection_errors_map_to_llm_transport_error() {
    let fake = FakeHttp::default();
    *fake.response.lock().unwrap() = Some(Err(HttpError::Connection("dns".to_string())));
    let bridge = LlmTransportBridge::new(fake);

    let error = llm_client::Transport::execute(&bridge, &provider_request())
        .await
        .expect_err("connection error");

    assert!(matches!(error, llm_client::LlmError::Transport { message } if message.contains("dns")));
}

#[allow(clippy::unnecessary_wraps)]
fn sse(data: &str) -> Result<SseEvent, HttpError> {
    Ok(SseEvent {
        event_type: Some("message".to_string()),
        data: data.to_string(),
        id: None,
    })
}

#[tokio::test]
async fn open_stream_maps_sse_events_to_frames() {
    let fake = FakeHttp::default();
    *fake.sse.lock().unwrap() = Some(Ok(vec![
        sse(r#"{"type":"message_stop"}"#),
        sse("[DONE]"),
    ]));
    let bridge = LlmTransportBridge::new(fake);

    let mut streaming = llm_client::Transport::open_stream(&bridge, &provider_request())
        .await
        .expect("stream");

    assert_eq!(streaming.status, 200);
    let first = streaming.frames.next_frame().await.unwrap().expect("frame");
    assert_eq!(first.bytes, br#"{"type":"message_stop"}"#);
    let second = streaming.frames.next_frame().await.unwrap().expect("frame");
    assert_eq!(second.bytes, b"[DONE]");
    assert!(streaming.frames.next_frame().await.unwrap().is_none());
}

#[tokio::test]
async fn open_stream_status_error_yields_status_and_body_frame() {
    let fake = FakeHttp::default();
    *fake.sse.lock().unwrap() = Some(Err(HttpError::Status {
        status: 429,
        body: r#"{"type":"error","error":{"type":"rate_limit_error","message":"slow"}}"#.to_string(),
    }));
    let bridge = LlmTransportBridge::new(fake);

    let mut streaming = llm_client::Transport::open_stream(&bridge, &provider_request())
        .await
        .expect("status error is data");

    assert_eq!(streaming.status, 429);
    let body = streaming.frames.next_frame().await.unwrap().expect("body frame");
    assert!(String::from_utf8(body.bytes).unwrap().contains("rate_limit_error"));
    assert!(streaming.frames.next_frame().await.unwrap().is_none());
}

#[tokio::test]
async fn mid_stream_http_error_maps_to_llm_transport_error() {
    let fake = FakeHttp::default();
    *fake.sse.lock().unwrap() = Some(Ok(vec![
        sse(r#"{"type":"message_start","message":{"id":"m","model":"x","content":[],"usage":{}}}"#),
        Err(HttpError::Connection("reset".to_string())),
    ]));
    let bridge = LlmTransportBridge::new(fake);

    let mut streaming = llm_client::Transport::open_stream(&bridge, &provider_request())
        .await
        .expect("stream");

    assert!(streaming.frames.next_frame().await.unwrap().is_some());
    let error = streaming.frames.next_frame().await.expect_err("mid-stream error");
    assert!(matches!(error, llm_client::LlmError::Transport { message } if message.contains("reset")));
}

#[tokio::test]
async fn bridge_drives_llm_client_event_stream_end_to_end() {
    let fake = FakeHttp::default();
    *fake.sse.lock().unwrap() = Some(Ok(vec![
        sse(r#"{"type":"message_start","message":{"id":"msg_1","model":"claude-sonnet-4-20250514","content":[],"usage":{"input_tokens":1,"output_tokens":0}}}"#),
        sse(r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#),
        sse(r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}"#),
        sse(r#"{"type":"content_block_stop","index":0}"#),
        sse(r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"input_tokens":1,"output_tokens":1}}"#),
        sse(r#"{"type":"message_stop"}"#),
    ]));
    let bridge = LlmTransportBridge::new(fake);

    let client = llm_client::client::DefaultLlmClient::from_config(llm_client::ClientConfig {
        providers: vec![llm_client::ProviderProfile {
            provider_id: llm_client::ProviderId::AnthropicFirstParty,
            profile_name: "anthropic".to_string(),
            base_url: "https://api.anthropic.com".to_string(),
            protocol: llm_client::ProtocolFamily::AnthropicMessages,
            auth: llm_client::AuthStrategy::None,
            credential: llm_client::CredentialConfig::None,
            models: vec![llm_client::ModelProfile {
                display_model: "claude-sonnet-4-20250514".to_string(),
                request_model: "claude-sonnet-4-20250514".to_string(),
                billing_model: "claude-sonnet-4".to_string(),
                aliases: vec![],
                capabilities: llm_client::Capabilities {
                    streaming: true,
                    tools: true,
                    ..Default::default()
                },
            }],
            pricing: llm_client::PricingConfig::default(),
        }],
    })
    .expect("client");

    let mut request = llm_client::LlmRequest::new("claude-sonnet-4-20250514").with_user_text("hello");
    request.stream = true;

    let mut events = client.execute_stream(&request, &bridge).await.expect("stream");
    let mut texts = String::new();
    let mut stop_reason = None;
    while let Some(event) = events.next_event().await.expect("event") {
        match event {
            llm_client::LlmEvent::ContentBlockDelta {
                delta: llm_client::ContentDelta::TextDelta { text },
                ..
            } => texts.push_str(&text),
            llm_client::LlmEvent::MessageDelta { delta, .. } => stop_reason = delta.stop_reason,
            _ => {}
        }
    }
    assert_eq!(texts, "hi");
    assert_eq!(stop_reason.as_deref(), Some("end_turn"));
}

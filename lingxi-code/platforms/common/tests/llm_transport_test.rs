use std::sync::Mutex;

use async_trait::async_trait;
use platform_common::LlmTransportBridge;
use protocol::{HttpRequest, HttpResponse, SseEvent};
use traits::http::{RawByteStream, RawByteStreamWithMeta, SseStream};
use traits::{HttpError, HttpTransport};

type ScriptedSse = Mutex<Option<Result<Vec<Result<SseEvent, HttpError>>, HttpError>>>;
type ScriptedRaw = Mutex<Option<Result<RawByteStreamWithMeta, HttpError>>>;

#[derive(Default)]
struct FakeHttp {
    response: Mutex<Option<Result<HttpResponse, HttpError>>>,
    sse: ScriptedSse,
    raw: ScriptedRaw,
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

    async fn stream_raw_bytes_with_meta(
        &self,
        req: HttpRequest,
    ) -> Result<RawByteStreamWithMeta, HttpError> {
        *self.seen.lock().expect("seen") = Some(req);
        self.raw.lock().expect("raw").take().expect("scripted raw")
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
    // The second payload is an artificial terminator demonstrating verbatim
    // pass-through ([DONE] handling belongs to codecs, not the bridge).
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

    let first = streaming.frames.next_frame().await.unwrap().expect("first frame");
    assert!(String::from_utf8(first.bytes).unwrap().contains("message_start"));
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
            signing: None,
            azure: None,
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

/// When `stream_framing == AwsEventStream` the bridge calls
/// `stream_raw_bytes_with_meta` instead of `stream_sse_with_meta`, and raw
/// byte chunks arrive as `RawStreamFrame`s without SSE splitting.
#[tokio::test]
async fn open_stream_aws_event_stream_routes_to_raw_bytes_path() {
    let chunk1 = b"\x00\x00\x00\x10".to_vec(); // first 4 bytes of a fake frame
    let chunk2 = b"\xFF\xFE\xFD\xFC".to_vec();

    let raw_stream: RawByteStream = Box::pin(futures_util::stream::iter(vec![
        Ok::<Vec<u8>, HttpError>(chunk1.clone()),
        Ok::<Vec<u8>, HttpError>(chunk2.clone()),
    ]));

    let fake = FakeHttp::default();
    *fake.raw.lock().unwrap() = Some(Ok(RawByteStreamWithMeta {
        status: 200,
        headers: vec![("x-amzn-requestid".to_string(), "req-123".to_string())],
        stream: raw_stream,
    }));
    let bridge = LlmTransportBridge::new(fake);

    // Build a request with AwsEventStream framing.
    let mut request = provider_request();
    request.stream_framing = llm_client::StreamFraming::AwsEventStream;

    let mut streaming = llm_client::Transport::open_stream(&bridge, &request)
        .await
        .expect("raw stream");

    assert_eq!(streaming.status, 200);
    assert_eq!(streaming.headers.get("x-amzn-requestid").map(String::as_str), Some("req-123"));

    // Frames are raw byte chunks — NOT SSE-parsed.
    let f1 = streaming.frames.next_frame().await.unwrap().expect("chunk1");
    assert_eq!(f1.bytes, chunk1);
    let f2 = streaming.frames.next_frame().await.unwrap().expect("chunk2");
    assert_eq!(f2.bytes, chunk2);
    assert!(streaming.frames.next_frame().await.unwrap().is_none());
}

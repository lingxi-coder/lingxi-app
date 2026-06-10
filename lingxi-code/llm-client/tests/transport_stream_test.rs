use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;
use std::time::Duration;

use llm_client::client::DefaultLlmClient;
use llm_client::{
    AuthStrategy, BoxFuture, Capabilities, ClientConfig, ContentDelta, CredentialConfig,
    FrameStream, LlmError, LlmEvent, LlmRequest, ModelProfile, PricingConfig, ProtocolFamily,
    ProviderId, ProviderProfile, ProviderRequest, ProviderResponse, RawStreamFrame,
    StreamingResponse, Transport,
};

struct ScriptedFrames {
    items: VecDeque<Result<RawStreamFrame, LlmError>>,
}

impl FrameStream for ScriptedFrames {
    fn next_frame(&mut self) -> BoxFuture<'_, Result<Option<RawStreamFrame>, LlmError>> {
        let next = match self.items.pop_front() {
            Some(Ok(frame)) => Ok(Some(frame)),
            Some(Err(error)) => Err(error),
            None => Ok(None),
        };
        Box::pin(async move { next })
    }
}

struct StreamTransport {
    status: u16,
    headers: BTreeMap<String, String>,
    frames: Mutex<Option<Vec<Result<RawStreamFrame, LlmError>>>>,
}

impl StreamTransport {
    fn scripted(status: u16, frames: Vec<Result<RawStreamFrame, LlmError>>) -> Self {
        Self {
            status,
            headers: BTreeMap::new(),
            frames: Mutex::new(Some(frames)),
        }
    }
}

impl Transport for StreamTransport {
    fn execute<'a>(
        &'a self,
        _request: &'a ProviderRequest,
    ) -> BoxFuture<'a, Result<ProviderResponse, LlmError>> {
        Box::pin(async move {
            Err(LlmError::Transport {
                message: "execute not scripted".to_string(),
            })
        })
    }

    fn open_stream<'a>(
        &'a self,
        _request: &'a ProviderRequest,
    ) -> BoxFuture<'a, Result<StreamingResponse, LlmError>> {
        let frames = self
            .frames
            .lock()
            .expect("frames lock")
            .take()
            .expect("open_stream called once");
        let status = self.status;
        let headers = self.headers.clone();
        Box::pin(async move {
            Ok(StreamingResponse {
                status,
                headers,
                frames: Box::new(ScriptedFrames {
                    items: frames.into(),
                }),
            })
        })
    }
}

// Result-wrapped to match the scripted frame vec's element type.
#[allow(clippy::unnecessary_wraps)]
fn frame(payload: &str) -> Result<RawStreamFrame, LlmError> {
    Ok(RawStreamFrame::new(payload.as_bytes().to_vec()))
}

fn client(protocol: ProtocolFamily, provider_id: ProviderId, base_url: &str) -> DefaultLlmClient {
    DefaultLlmClient::from_config(ClientConfig {
        providers: vec![ProviderProfile {
            provider_id,
            profile_name: "p".to_string(),
            base_url: base_url.to_string(),
            protocol,
            auth: AuthStrategy::None,
            credential: CredentialConfig::None,
            models: vec![ModelProfile {
                display_model: "p-model".to_string(),
                request_model: "p-model".to_string(),
                billing_model: "p-model".to_string(),
                aliases: vec![],
                capabilities: Capabilities {
                    streaming: true,
                    tools: true,
                    ..Default::default()
                },
            }],
            pricing: PricingConfig::default(),
        }],
    })
    .expect("client")
}

fn anthropic_client() -> DefaultLlmClient {
    client(
        ProtocolFamily::AnthropicMessages,
        ProviderId::AnthropicFirstParty,
        "https://api.anthropic.com",
    )
}

fn streaming_request() -> LlmRequest {
    let mut request = LlmRequest::new("p-model").with_user_text("hello");
    request.stream = true;
    request
}

async fn collect_events(
    stream: &mut llm_client::LlmEventStream,
) -> (Vec<LlmEvent>, Option<LlmError>) {
    let mut events = Vec::new();
    loop {
        match stream.next_event().await {
            Ok(Some(event)) => events.push(event),
            Ok(None) => return (events, None),
            Err(error) => return (events, Some(error)),
        }
    }
}

#[tokio::test]
async fn execute_stream_decodes_anthropic_event_sequence() {
    let transport = StreamTransport::scripted(200, vec![
        frame(r#"{"type":"message_start","message":{"id":"msg_1","model":"p-model","content":[],"usage":{"input_tokens":2,"output_tokens":0}}}"#),
        frame(r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#),
        frame(r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}"#),
        frame(r#"{"type":"content_block_stop","index":0}"#),
        frame(r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"input_tokens":2,"output_tokens":1}}"#),
        frame(r#"{"type":"message_stop"}"#),
    ]);
    let client = anthropic_client();

    let mut stream = client
        .execute_stream(&streaming_request(), &transport)
        .await
        .expect("stream");
    let (events, error) = collect_events(&mut stream).await;

    assert!(error.is_none(), "unexpected error: {error:?}");
    assert!(matches!(events[0], LlmEvent::MessageStart { .. }));
    assert!(matches!(
        events[2],
        LlmEvent::ContentBlockDelta { delta: ContentDelta::TextDelta { ref text }, .. } if text == "hi"
    ));
    assert!(matches!(
        events[4],
        LlmEvent::MessageDelta { delta: llm_client::MessageDeltaPayload { stop_reason: Some(ref reason) }, .. }
            if reason == "end_turn"
    ));
    assert!(matches!(events.last(), Some(LlmEvent::MessageStop)));
    assert!(stream.next_event().await.expect("after end").is_none());
}

#[tokio::test]
async fn finish_emits_terminal_events_when_frames_end() {
    let transport = StreamTransport::scripted(200, vec![
        frame(r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"hi"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":2,"candidatesTokenCount":1,"totalTokenCount":3}}"#),
    ]);
    let client = client(
        ProtocolFamily::GeminiGenerateContent,
        ProviderId::Gemini,
        "https://generativelanguage.googleapis.com/v1beta",
    );

    let mut stream = client
        .execute_stream(&streaming_request(), &transport)
        .await
        .expect("stream");
    let (events, error) = collect_events(&mut stream).await;

    assert!(error.is_none(), "unexpected error: {error:?}");
    assert!(matches!(events.first(), Some(LlmEvent::MessageStart { .. })));
    assert!(matches!(
        events.iter().find(|event| matches!(event, LlmEvent::MessageDelta { .. })),
        Some(LlmEvent::MessageDelta { usage: Some(usage), .. }) if usage.billable_tokens.input == 2
    ));
    assert!(matches!(events.last(), Some(LlmEvent::MessageStop)));
}

#[tokio::test]
async fn execute_stream_requires_stream_flag() {
    let transport = StreamTransport::scripted(200, vec![]);
    let client = anthropic_client();

    let error = client
        .execute_stream(&LlmRequest::new("p-model").with_user_text("hello"), &transport)
        .await
        .expect_err("must require stream flag");

    assert!(matches!(error, LlmError::InvalidRequest { message } if message.contains("stream")));
}

#[tokio::test]
async fn error_status_drains_frames_into_taxonomy() {
    let body = r#"{"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}"#;
    let (first, second) = body.as_bytes().split_at(20);
    let mut transport = StreamTransport::scripted(429, vec![
        Ok(RawStreamFrame::new(first.to_vec())),
        Ok(RawStreamFrame::new(second.to_vec())),
    ]);
    transport.headers.insert("retry-after".to_string(), "7".to_string());
    let client = anthropic_client();

    let error = client
        .execute_stream(&streaming_request(), &transport)
        .await
        .expect_err("error status");

    assert!(matches!(
        error,
        LlmError::RateLimited { retry_after: Some(after), .. } if after == Duration::from_secs(7)
    ));
}

#[tokio::test]
async fn mid_stream_failure_after_events_is_stream_interrupted() {
    let transport = StreamTransport::scripted(200, vec![
        frame(r#"{"type":"message_start","message":{"id":"msg_1","model":"p-model","content":[],"usage":{"input_tokens":1,"output_tokens":0}}}"#),
        Err(LlmError::Transport {
            message: "connection reset".to_string(),
        }),
    ]);
    let client = anthropic_client();

    let mut stream = client
        .execute_stream(&streaming_request(), &transport)
        .await
        .expect("stream");

    assert!(matches!(
        stream.next_event().await.expect("first event"),
        Some(LlmEvent::MessageStart { .. })
    ));
    assert!(matches!(
        stream.next_event().await.expect_err("interrupted"),
        LlmError::StreamInterrupted { message } if message.contains("connection reset")
    ));
    assert!(stream.next_event().await.expect("terminal").is_none());
}

#[tokio::test]
async fn failure_before_any_event_stays_transport() {
    let transport = StreamTransport::scripted(200, vec![
        Err(LlmError::Transport {
            message: "connection refused".to_string(),
        }),
    ]);
    let client = anthropic_client();

    let mut stream = client
        .execute_stream(&streaming_request(), &transport)
        .await
        .expect("stream");

    assert!(matches!(
        stream.next_event().await.expect_err("transport failure"),
        LlmError::Transport { message } if message.contains("connection refused")
    ));
    assert!(stream.next_event().await.expect("terminal").is_none());
}

//! Integration test (Batch 5): a 400 `max_tokens` context-overflow is parsed,
//! the request body's `max_tokens` is re-shrunk, and the request is re-issued
//! and succeeds — 1:1 with claude-code `withRetry.ts:384-427`.
//!
//! Uses a body-capturing in-process transport (rather than `mock_server.rs`,
//! which doesn't record request bodies) so we can assert that the **second**
//! attempt carried the reduced `max_tokens`.

use api_client::anthropic::AnthropicProvider;
use api_client::ApiError;
use async_trait::async_trait;
use futures::stream::Stream;
use protocol::{
    ContentBlock, ConversationMessage, HttpRequest, HttpResponse, MessageId, SseEvent,
};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use traits::{HttpError, HttpTransport};

/// A transport that records every request body and replays scripted responses
/// in FIFO order.
struct RecordingTransport {
    responses: Mutex<std::collections::VecDeque<HttpResponse>>,
    bodies: Mutex<Vec<String>>,
}

impl RecordingTransport {
    fn new(responses: Vec<HttpResponse>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
            bodies: Mutex::new(Vec::new()),
        }
    }

    fn recorded_bodies(&self) -> Vec<String> {
        self.bodies.lock().unwrap().clone()
    }
}

#[async_trait]
impl HttpTransport for RecordingTransport {
    async fn request(&self, req: HttpRequest) -> Result<HttpResponse, HttpError> {
        self.bodies
            .lock()
            .unwrap()
            .push(req.body.clone().unwrap_or_default());
        let next = self.responses.lock().unwrap().pop_front();
        next.ok_or_else(|| HttpError::Connection("scripted queue drained".into()))
    }

    async fn stream_sse(&self, _req: HttpRequest) -> Result<traits::http::SseStream, HttpError> {
        let s: Pin<Box<dyn Stream<Item = Result<SseEvent, HttpError>> + Send>> =
            Box::pin(futures::stream::empty());
        Ok(s)
    }
}

fn resp(status: u16, body: &str) -> HttpResponse {
    HttpResponse {
        status,
        headers: Vec::new(),
        body: body.into(),
    }
}

fn make_msgs() -> Vec<ConversationMessage> {
    vec![ConversationMessage::User {
        id: MessageId::new(),
        content: vec![ContentBlock::Text { text: "hi".into() }],
    }]
}

const OVERFLOW_400: &str = r#"{"type":"error","error":{"type":"invalid_request_error","message":"input length and `max_tokens` exceed context limit: 188059 + 20000 > 200000"}}"#;

const SUCCESS_200: &str = r#"{"id":"msg_ok","model":"claude-opus-4-6","content":[{"type":"text","text":"hi"}],"stop_reason":"end_turn","usage":{"input_tokens":2,"output_tokens":1}}"#;

fn max_tokens_of(body: &str) -> u64 {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("max_tokens").and_then(serde_json::Value::as_u64))
        .expect("body must carry max_tokens")
}

/// Overflow 400 on the first attempt → reshrink → 200 on the re-issue.
/// The second request body must carry the reduced `max_tokens`
/// (`max(3000, 200000-188059-1000) = 10941`), and the call must succeed.
#[tokio::test]
async fn overflow_400_reshrinks_max_tokens_then_succeeds() {
    let transport = Arc::new(RecordingTransport::new(vec![
        resp(400, OVERFLOW_400),
        resp(200, SUCCESS_200),
    ]));
    let provider = AnthropicProvider::new("sk-test", Some("http://test.invalid".into()));

    let r = provider
        .messages_create_non_stream_with_opts(
            "claude-opus-4-6",
            None,
            make_msgs(),
            20_000, // initial max_tokens — what triggered the overflow
            Vec::new(),
            None,
            transport.as_ref(),
        )
        .await;

    assert!(r.is_ok(), "overflow reshrink must converge to 200: {r:?}");

    let bodies = transport.recorded_bodies();
    assert_eq!(bodies.len(), 2, "exactly two requests: original + reshrunk");
    assert_eq!(
        max_tokens_of(&bodies[0]),
        20_000,
        "first attempt carries the original max_tokens",
    );
    assert_eq!(
        max_tokens_of(&bodies[1]),
        10_941,
        "second attempt carries the reshrunk max_tokens (200000-188059-1000)",
    );
}

/// The reshrink happens WITHIN one `with_retry` attempt (TS `continue`), so it
/// does NOT consume a normal retry slot: an overflow-then-200 succeeds even
/// when only a single retry budget would otherwise be available.
#[tokio::test]
async fn overflow_does_not_consume_a_normal_retry_slot() {
    // Sequence: overflow 400 (reshrunk in-attempt) → 200. Only two requests.
    let transport = Arc::new(RecordingTransport::new(vec![
        resp(400, OVERFLOW_400),
        resp(200, SUCCESS_200),
    ]));
    let provider = AnthropicProvider::new("sk-test", Some("http://test.invalid".into()));

    let r = provider
        .messages_create_non_stream_with_opts(
            "claude-opus-4-6",
            None,
            make_msgs(),
            20_000,
            Vec::new(),
            None,
            transport.as_ref(),
        )
        .await;
    assert!(r.is_ok(), "{r:?}");
    assert_eq!(transport.recorded_bodies().len(), 2);
}

/// A tiny-context overflow (`available < 3000`) is NOT reshrinkable: the
/// original 400 is surfaced unchanged and only one request is made.
#[tokio::test]
async fn tiny_context_overflow_surfaces_original_400() {
    const TINY_OVERFLOW_400: &str = r#"{"type":"error","error":{"type":"invalid_request_error","message":"input length and `max_tokens` exceed context limit: 199000 + 20000 > 200000"}}"#;
    // available = 200000 - 199000 - 1000 = 0 < 3000 → give up.

    let transport = Arc::new(RecordingTransport::new(vec![resp(400, TINY_OVERFLOW_400)]));
    let provider = AnthropicProvider::new("sk-test", Some("http://test.invalid".into()));

    let r = provider
        .messages_create_non_stream_with_opts(
            "claude-opus-4-6",
            None,
            make_msgs(),
            20_000,
            Vec::new(),
            None,
            transport.as_ref(),
        )
        .await;

    match r {
        Err(ApiError::Server { status: 400, .. }) => {}
        other => panic!("expected the original 400 to surface, got {other:?}"),
    }
    assert_eq!(
        transport.recorded_bodies().len(),
        1,
        "no reshrink → exactly one request",
    );
}

/// A plain 400 (no overflow marker) is terminal and never reshrunk.
#[tokio::test]
async fn plain_400_is_terminal() {
    let transport = Arc::new(RecordingTransport::new(vec![resp(
        400,
        r#"{"type":"error","error":{"type":"invalid_request_error","message":"messages: at least one message is required"}}"#,
    )]));
    let provider = AnthropicProvider::new("sk-test", Some("http://test.invalid".into()));

    let r = provider
        .messages_create_non_stream_with_opts(
            "claude-opus-4-6",
            None,
            make_msgs(),
            20_000,
            Vec::new(),
            None,
            transport.as_ref(),
        )
        .await;

    match r {
        Err(ApiError::Server { status: 400, .. }) => {}
        other => panic!("expected terminal 400, got {other:?}"),
    }
    assert_eq!(transport.recorded_bodies().len(), 1);
}

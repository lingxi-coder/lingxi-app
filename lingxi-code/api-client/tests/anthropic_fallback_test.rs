//! Integration test (Batch 2): the consecutive-529 counter + Opus model
//! fallback path through `messages_create_non_stream_with_fallback` — 1:1 with
//! claude-code `withRetry.ts:326-365`.
//!
//! Uses a scripted in-process transport so we can drive a deterministic run of
//! 529 responses and assert the surfaced `ApiError`.
//!
//! NOTE on env: `resolve_retry_control` reads `FALLBACK_FOR_ALL_PRIMARY_MODELS`
//! / `USER_TYPE` / `IS_SANDBOX`. The first two tests below pass an Opus model
//! with `is_subscriber = false`, which opens the fallback gate via the
//! `!subscriber && is_non_custom_opus` clause **independent of env**, and the
//! `FallbackTriggered` outcome short-circuits before the external/sandbox env
//! checks — so they are robust against ambient env in a parallel test runner.

use api_client::anthropic::AnthropicProvider;
use api_client::ApiError;
use async_trait::async_trait;
use futures::stream::Stream;
use protocol::{ContentBlock, ConversationMessage, HttpRequest, HttpResponse, MessageId, SseEvent};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use traits::{HttpError, HttpTransport};

/// A transport that replays a scripted response on every request, counting the
/// number of requests made.
struct ScriptedTransport {
    response: HttpResponse,
    calls: Mutex<u32>,
}

impl ScriptedTransport {
    fn always(response: HttpResponse) -> Self {
        Self {
            response,
            calls: Mutex::new(0),
        }
    }

    fn call_count(&self) -> u32 {
        *self.calls.lock().unwrap()
    }
}

#[async_trait]
impl HttpTransport for ScriptedTransport {
    async fn request(&self, _req: HttpRequest) -> Result<HttpResponse, HttpError> {
        *self.calls.lock().unwrap() += 1;
        Ok(self.response.clone())
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

/// 3×529 on an Opus primary with a configured fallback model + non-subscriber
/// → the loop surfaces `FallbackTriggered { original, fallback }` so the
/// orchestrator can re-issue against the fallback model.
#[tokio::test]
async fn three_529_with_fallback_surfaces_fallback_triggered() {
    let transport = Arc::new(ScriptedTransport::always(resp(529, "overloaded")));
    let provider = AnthropicProvider::new("sk-test", Some("http://test.invalid".into()));

    let r = provider
        .messages_create_non_stream_with_fallback(
            "claude-opus-4-6",
            None,
            make_msgs(),
            4096,
            Vec::new(),
            None,
            Some("claude-sonnet-4-6".into()),
            false, // is_subscriber stub (Batch 6 refines)
            transport.as_ref(),
        )
        .await;

    match r {
        Err(ApiError::FallbackTriggered {
            original_model,
            fallback_model,
        }) => {
            assert_eq!(original_model, "claude-opus-4-6");
            assert_eq!(fallback_model, "claude-sonnet-4-6");
        }
        other => panic!("expected FallbackTriggered, got {other:?}"),
    }
    // The gate fires on the 3rd consecutive 529.
    assert_eq!(transport.call_count(), 3);
}

/// A non-Opus primary keeps the fallback gate closed (`is_subscriber = false`
/// but `is_non_custom_opus` is false), so even repeated 529s do NOT trigger the
/// fallback; the request exhausts the retry budget and surfaces the byte-locked
/// `Overloaded { repeated: true }` (the final attempt was a 529).
#[tokio::test]
async fn non_opus_model_does_not_trigger_fallback() {
    let transport = Arc::new(ScriptedTransport::always(resp(529, "overloaded")));
    let provider = AnthropicProvider::new("sk-test", Some("http://test.invalid".into()));

    let r = provider
        .messages_create_non_stream_with_fallback(
            "claude-sonnet-4-6", // not a non-custom Opus model
            None,
            make_msgs(),
            4096,
            Vec::new(),
            None,
            Some("claude-haiku-4-5".into()), // fallback present but gate closed
            false,
            transport.as_ref(),
        )
        .await;

    match r {
        Err(ApiError::Overloaded { repeated }) => assert!(repeated),
        other => panic!("expected Overloaded {{ repeated: true }}, got {other:?}"),
    }
    assert_eq!(transport.call_count(), 3, "budget-exhausted, not fallback");
}

/// 2×529 then a 200 recovers: the consecutive counter never reaches the
/// threshold, so neither the fallback nor the repeated-overload terminal fires.
#[tokio::test]
async fn two_529_then_200_recovers() {
    // A queue-backed transport: 529, 529, 200.
    struct QueueTransport {
        responses: Mutex<std::collections::VecDeque<HttpResponse>>,
        calls: Mutex<u32>,
    }
    #[async_trait]
    impl HttpTransport for QueueTransport {
        async fn request(&self, _req: HttpRequest) -> Result<HttpResponse, HttpError> {
            *self.calls.lock().unwrap() += 1;
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| HttpError::Connection("queue drained".into()))
        }
        async fn stream_sse(
            &self,
            _req: HttpRequest,
        ) -> Result<traits::http::SseStream, HttpError> {
            let s: Pin<Box<dyn Stream<Item = Result<SseEvent, HttpError>> + Send>> =
                Box::pin(futures::stream::empty());
            Ok(s)
        }
    }

    const SUCCESS_200: &str = r#"{"id":"msg_ok","model":"claude-opus-4-6","content":[{"type":"text","text":"hi"}],"stop_reason":"end_turn","usage":{"input_tokens":2,"output_tokens":1}}"#;

    let transport = Arc::new(QueueTransport {
        responses: Mutex::new(
            vec![
                resp(529, "overloaded"),
                resp(529, "overloaded"),
                resp(200, SUCCESS_200),
            ]
            .into(),
        ),
        calls: Mutex::new(0),
    });
    let provider = AnthropicProvider::new("sk-test", Some("http://test.invalid".into()));

    let r = provider
        .messages_create_non_stream_with_fallback(
            "claude-opus-4-6",
            None,
            make_msgs(),
            4096,
            Vec::new(),
            None,
            Some("claude-sonnet-4-6".into()),
            false,
            transport.as_ref(),
        )
        .await;

    assert!(r.is_ok(), "2×529 then 200 must recover: {r:?}");
    assert_eq!(*transport.calls.lock().unwrap(), 3);
}

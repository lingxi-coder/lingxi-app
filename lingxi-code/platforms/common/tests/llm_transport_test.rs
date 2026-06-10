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

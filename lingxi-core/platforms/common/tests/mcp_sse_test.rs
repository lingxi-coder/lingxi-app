//! Verifies `connect_sse` wire format against a real HTTP server:
//! - GET request bears `Accept: text/event-stream`.
//! - When auth_token is supplied, GET also bears
//!   `X-Claude-Code-Ide-Authorization: <token>`.
//! - Outbound JSON-RPC requests POST to the same URL with
//!   `Content-Type: application/json`.
//! - SSE event line `data: {json}\n\n` is parsed into a JSON-RPC inbound
//!   message and the matching pending `call` future returns the response.

use axum::{
    extract::State,
    http::HeaderMap,
    response::sse::{Event, Sse},
    routing::{get, post},
    Json, Router,
};
use futures::{stream, StreamExt};
use serde_json::{json, Value};
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

#[derive(Default, Clone)]
struct MockState {
    captured_get_headers: Arc<Mutex<HeaderMap>>,
    captured_post_headers: Arc<Mutex<HeaderMap>>,
    captured_post_body: Arc<Mutex<Vec<Value>>>,
    // The next response (a JSON-RPC reply) the mock will emit over SSE.
    next_sse_event: Arc<Mutex<Option<Value>>>,
}

async fn sse_handler(
    State(state): State<MockState>,
    headers: HeaderMap,
) -> Sse<impl futures::Stream<Item = Result<Event, Infallible>>> {
    *state.captured_get_headers.lock().await = headers;
    let evt = state.next_sse_event.lock().await.clone();
    // Delay emission slightly so the client has a chance to call() and
    // register the pending oneshot for id=1 before the response arrives.
    // (Otherwise the client may receive the response *before* it has
    // registered the pending oneshot, and the response is dropped on the
    // floor — the router would then time out.)
    let initial = stream::once(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        Ok(Event::default().data(evt.unwrap_or_else(|| json!(null)).to_string()))
    });
    let stream = initial.chain(stream::pending());
    Sse::new(stream)
}

async fn post_handler(
    State(state): State<MockState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> &'static str {
    *state.captured_post_headers.lock().await = headers;
    state.captured_post_body.lock().await.push(body);
    "" // No body — response will come back via the SSE channel.
}

async fn spawn_mock(initial_event: Value) -> (String, MockState) {
    let state = MockState::default();
    *state.next_sse_event.lock().await = Some(initial_event);
    let app = Router::new()
        .route("/mcp", get(sse_handler))
        .route("/mcp", post(post_handler))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}/mcp"), state)
}

#[tokio::test]
async fn connect_sse_sends_get_with_accept_event_stream() {
    let pre_baked_reply = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": { "ok": true }
    });
    let (url, state) = spawn_mock(pre_baked_reply).await;

    let conn = lingxi_platform_common::connect_sse(&url, None, &Default::default())
        .await
        .expect("connect_sse should succeed against mock");

    // Send a request; the mock pre-baked the reply so it should match id=1.
    let resp: Value = conn
        .call("test.method", json!({"hello": "world"}))
        .await
        .expect("request should round-trip");
    assert_eq!(resp, json!({"ok": true}));

    let get_headers = state.captured_get_headers.lock().await.clone();
    assert_eq!(
        get_headers.get("accept").and_then(|v| v.to_str().ok()),
        Some("text/event-stream"),
        "GET must carry Accept: text/event-stream"
    );

    let post_headers = state.captured_post_headers.lock().await.clone();
    let ct = post_headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        ct.starts_with("application/json"),
        "POST Content-Type must be application/json, got {ct}"
    );

    let post_bodies = state.captured_post_body.lock().await.clone();
    assert_eq!(post_bodies.len(), 1, "exactly one POST emitted");
    assert_eq!(post_bodies[0]["method"], "test.method");
    assert_eq!(post_bodies[0]["jsonrpc"], "2.0");
    assert_eq!(post_bodies[0]["id"], 1);
}

#[tokio::test]
async fn connect_sse_passes_auth_header_when_token_supplied() {
    let pre_baked_reply = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": null
    });
    let (url, state) = spawn_mock(pre_baked_reply).await;

    let conn = lingxi_platform_common::connect_sse(
        &url,
        Some("abc123def456abc123def456abc12345"),
        &Default::default(),
    )
    .await
    .expect("connect_sse should succeed");

    let _: Result<Value, _> = conn.call("ping", json!({})).await;

    let get_headers = state.captured_get_headers.lock().await.clone();
    let auth = get_headers
        .get("x-claude-code-ide-authorization")
        .and_then(|v| v.to_str().ok());
    assert_eq!(
        auth,
        Some("abc123def456abc123def456abc12345"),
        "GET must carry X-Claude-Code-Ide-Authorization header verbatim"
    );

    let post_headers = state.captured_post_headers.lock().await.clone();
    let auth_post = post_headers
        .get("x-claude-code-ide-authorization")
        .and_then(|v| v.to_str().ok());
    assert_eq!(
        auth_post,
        Some("abc123def456abc123def456abc12345"),
        "POST must also carry the auth header"
    );
}

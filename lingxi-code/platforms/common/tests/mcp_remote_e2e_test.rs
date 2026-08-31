//! Real HTTP and SSE round trips for the shared remote-only transport.

use axum::{
    extract::State,
    response::sse::{Event, Sse},
    routing::{get, post},
    Json, Router,
};
use futures::{stream, Stream, StreamExt};
use platform_common::RemoteMcpTransport;
use serde_json::{json, Value};
use std::convert::Infallible;
use std::time::Duration;
use tokio::sync::broadcast;
use traits::{McpTransport, McpTransportSpec};

fn spec(kind: &str, url: String) -> McpTransportSpec {
    let headers = traits::McpHeaders::new();
    match kind {
        "sse" => McpTransportSpec::Sse {
            url,
            headers,
            headers_helper: None,
            oauth: None,
        },
        _ => McpTransportSpec::Http {
            url,
            headers,
            headers_helper: None,
            oauth: None,
        },
    }
}

fn reply_for(body: &Value) -> Option<Value> {
    let id = body.get("id")?.clone();
    let method = body.get("method").and_then(Value::as_str).unwrap_or("");
    let result = match method {
        "initialize" => json!({
            "protocolVersion": "2025-11-25",
            "capabilities": { "tools": {}, "resources": {}, "prompts": {} },
            "serverInfo": { "name": "common-remote", "version": "test" }
        }),
        "tools/list" => json!({"tools": [{
            "name": "echo", "description": "Echo", "inputSchema": {"type": "object"}
        }]}),
        "tools/call" => json!({
            "content": [{"type": "text", "text": body.pointer("/params/arguments/text").and_then(Value::as_str).unwrap_or("")}],
            "isError": false
        }),
        "resources/list" => {
            json!({"resources": [{"uri": "test://one", "name": "one", "mimeType": "text/plain"}]})
        }
        "resources/read" => json!({"contents": [{"uri": "test://one", "text": "resource"}]}),
        "prompts/list" => {
            json!({"prompts": [{"name": "hello", "description": "Hello", "arguments": []}]})
        }
        _ => json!({}),
    };
    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

async fn http_handler(Json(body): Json<Value>) -> Json<Value> {
    Json(reply_for(&body).unwrap_or_else(|| json!({})))
}

#[derive(Clone)]
struct SseState {
    replies: broadcast::Sender<Value>,
}

async fn sse_get(
    State(state): State<SseState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.replies.subscribe();
    let warmup = stream::once(async {
        tokio::time::sleep(Duration::from_millis(30)).await;
        None::<Value>
    });
    let replies = stream::unfold(rx, |mut rx| async move {
        match rx.recv().await {
            Ok(value) => Some((Some(value), rx)),
            Err(broadcast::error::RecvError::Lagged(_)) => Some((None, rx)),
            Err(broadcast::error::RecvError::Closed) => None,
        }
    });
    Sse::new(warmup.chain(replies).filter_map(|value| async move {
        value.map(|value| Ok(Event::default().data(value.to_string())))
    }))
}

async fn sse_post(State(state): State<SseState>, Json(body): Json<Value>) -> &'static str {
    if let Some(reply) = reply_for(&body) {
        let _ = state.replies.send(reply);
    }
    ""
}

async fn spawn_http() -> String {
    let app = Router::new().route("/mcp", post(http_handler));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}/mcp")
}

async fn spawn_sse() -> String {
    let (replies, _) = broadcast::channel(32);
    let state = SseState { replies };
    let app = Router::new()
        .route("/mcp", get(sse_get))
        .route("/mcp", post(sse_post))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}/mcp")
}

async fn exercise(kind: &str, url: String) {
    let transport = RemoteMcpTransport::new();
    let conn = tokio::time::timeout(Duration::from_secs(5), transport.connect(&spec(kind, url)))
        .await
        .unwrap()
        .unwrap();
    let caps = transport.initialize(&conn).await.unwrap();
    assert!(caps.tools && caps.resources && caps.prompts);
    assert_eq!(
        transport.list_tools(&conn).await.unwrap()[0].tool_name,
        "echo"
    );
    let result = transport
        .call_tool(&conn, "echo", json!({"text": "hello"}))
        .await
        .unwrap();
    assert_eq!(
        result.content.pointer("/0/text").and_then(Value::as_str),
        Some("hello")
    );
    assert_eq!(transport.list_resources(&conn).await.unwrap().len(), 1);
    assert_eq!(
        transport
            .read_resource(&conn, "test://one")
            .await
            .unwrap()
            .content,
        "resource"
    );
    assert_eq!(transport.list_prompts(&conn).await.unwrap().len(), 1);
    transport.disconnect(conn.connection_id).await.unwrap();
    assert!(matches!(
        transport.ping(conn.connection_id).await,
        Err(traits::McpError::Connection(_))
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shared_remote_http_roundtrip() {
    exercise("http", spawn_http().await).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shared_remote_sse_roundtrip() {
    exercise("sse", spawn_sse().await).await;
}

//! MCP SSE transport: GET text/event-stream from `url` for inbound JSON-RPC
//! messages, POST to the same URL for outbound. Matches claude-code's
//! `SSEClientTransport` wire format from `src/services/mcp/client.ts:626-707`.
//!
//! Wire contract (LITERAL):
//! - GET `Accept: text/event-stream`
//! - When `auth_token` is `Some`, both GET and POST carry
//!   `X-Claude-Code-Ide-Authorization: <token>` verbatim (no `Bearer ` prefix).
//! - POST goes to the SAME URL as the GET. `Content-Type: application/json`.
//! - Each SSE event is a single JSON-RPC `Message`: `data: {...}\n\n`.

use std::collections::HashMap;

use eventsource_stream::Eventsource;
use futures::StreamExt;
use lingxi_jsonrpc::messages::Message as JsonRpcMessage;
use lingxi_jsonrpc::{BrokerError, Connection, ConnectionError};
use lingxi_traits::mcp::McpError;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, CONTENT_TYPE, USER_AGENT};
use thiserror::Error;
use tokio::sync::mpsc;

/// Header name used by claude-code IDE plugins for the auth token.
/// LITERAL — must match claude-code byte-for-byte.
/// Source: `claude-code/src/services/mcp/client.ts:713`.
pub const IDE_AUTH_HEADER: &str = "X-Claude-Code-Ide-Authorization";

/// `User-Agent` value emitted by this client.
/// Matches claude-code's `getMCPUserAgent()` shape: `claude-code/<version>`.
#[must_use]
pub fn user_agent() -> String {
    format!("claude-code/{}", env!("CARGO_PKG_VERSION"))
}

/// Errors specific to opening an MCP SSE connection.
#[derive(Debug, Error)]
pub enum SseConnectError {
    /// HTTP request setup or send failed.
    #[error("sse transport error: {0}")]
    Transport(String),
    /// Authorization header value was invalid (non-ASCII, control chars).
    #[error("invalid auth token: {0}")]
    InvalidAuth(String),
}

impl From<SseConnectError> for McpError {
    fn from(value: SseConnectError) -> Self {
        Self::Connection(value.to_string())
    }
}

fn build_headers(
    auth_token: Option<&str>,
    extra_headers: &HashMap<String, String>,
    accept_value: &'static str,
) -> Result<HeaderMap, SseConnectError> {
    let mut h = HeaderMap::new();
    h.insert(ACCEPT, HeaderValue::from_static(accept_value));
    h.insert(
        USER_AGENT,
        HeaderValue::try_from(user_agent())
            .map_err(|e| SseConnectError::Transport(e.to_string()))?,
    );
    if let Some(token) = auth_token {
        let v = HeaderValue::try_from(token)
            .map_err(|e| SseConnectError::InvalidAuth(e.to_string()))?;
        h.insert(
            HeaderName::from_static("x-claude-code-ide-authorization"),
            v,
        );
    }
    for (k, v) in extra_headers {
        let name = HeaderName::try_from(k.as_str())
            .map_err(|e| SseConnectError::Transport(format!("bad header name {k}: {e}")))?;
        let val = HeaderValue::try_from(v.as_str())
            .map_err(|e| SseConnectError::Transport(format!("bad header value: {e}")))?;
        h.insert(name, val);
    }
    Ok(h)
}

/// Open an MCP SSE connection.
///
/// `url` is the URL to GET (for events) AND to POST (for outbound requests).
/// `auth_token`, if `Some`, becomes the `X-Claude-Code-Ide-Authorization`
/// header on BOTH the GET and the POST. `extra_headers` are applied to both
/// directions verbatim.
///
/// # Errors
///
/// - [`SseConnectError::Transport`] if reqwest client construction or the
///   initial GET request fails, or the server returns a non-2xx status on
///   the event-stream GET.
/// - [`SseConnectError::InvalidAuth`] if `auth_token` contains bytes that
///   cannot be expressed in an HTTP header value (non-ASCII, control chars).
#[allow(clippy::implicit_hasher)] // public API: HashMap is the documented config shape
pub async fn connect_sse(
    url: &str,
    auth_token: Option<&str>,
    extra_headers: &HashMap<String, String>,
) -> Result<Connection, SseConnectError> {
    let client = reqwest::Client::builder()
        .build()
        .map_err(|e| SseConnectError::Transport(e.to_string()))?;

    // ---- Open the SSE event-stream GET ----
    let get_headers = build_headers(auth_token, extra_headers, "text/event-stream")?;
    let response = client
        .get(url)
        .headers(get_headers)
        .send()
        .await
        .map_err(|e| SseConnectError::Transport(e.to_string()))?;

    if !response.status().is_success() {
        return Err(SseConnectError::Transport(format!(
            "SSE GET returned {}",
            response.status()
        )));
    }

    let byte_stream = response.bytes_stream();
    let mut event_stream = byte_stream.eventsource();

    // Channels: `inbound_tx` ships parsed JSON-RPC `Message`s from the SSE
    // reader task to the Connection. `outbound_rx` receives outbound
    // `Message`s the Connection wants to POST.
    let (inbound_tx, inbound_rx) = mpsc::unbounded_channel::<JsonRpcMessage>();
    let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel::<JsonRpcMessage>();

    // SSE reader task: parse `data:` JSON frames and forward as Message.
    tokio::spawn(async move {
        while let Some(item) = event_stream.next().await {
            match item {
                Ok(event) => {
                    // claude-code's SDK uses default-event SSE (no `event:` name)
                    // with the `data:` field carrying the JSON-RPC frame. Skip
                    // keep-alive comments / empty data lines.
                    if event.data.is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<JsonRpcMessage>(&event.data) {
                        Ok(msg) => {
                            if inbound_tx.send(msg).is_err() {
                                // Connection dropped; stop reading.
                                break;
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                data = %event.data,
                                "mcp sse: malformed JSON frame, skipping"
                            );
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "mcp sse: stream error, terminating reader");
                    break;
                }
            }
        }
    });

    // POST writer task: drain outbound frames and POST each as one JSON-RPC
    // body. Reuses the same `reqwest::Client` so the underlying connection
    // pool serves both GET and POST.
    let post_url = url.to_string();
    let post_auth = auth_token.map(str::to_string);
    let post_extra = extra_headers.clone();
    let post_client = client.clone();
    tokio::spawn(async move {
        while let Some(frame) = outbound_rx.recv().await {
            // claude-code's SSEClientTransport POST sends application/json;
            // the Accept header is also kept on POST for symmetry with the
            // GET, matching claude-code's SDK which sets both transports'
            // Accept to `text/event-stream`.
            let mut headers =
                match build_headers(post_auth.as_deref(), &post_extra, "text/event-stream") {
                    Ok(h) => h,
                    Err(e) => {
                        tracing::error!(error = %e, "mcp sse: failed to build POST headers");
                        break;
                    }
                };
            headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

            if let Err(e) = post_client
                .post(&post_url)
                .headers(headers)
                .json(&frame)
                .send()
                .await
            {
                tracing::warn!(error = %e, "mcp sse: POST failed");
                // Drop the frame; the router will time out the request.
                // Do not terminate the writer on a single failure — transient
                // network blips should not tear down the whole session.
            }
        }
    });

    // Adapt the inbound mpsc into Stream<Item = Message>.
    let inbound = futures::stream::unfold(inbound_rx, |mut rx| async move {
        rx.recv().await.map(|m| (m, rx))
    });

    // Adapt the outbound mpsc into Sink<Message, Error = ConnectionError>.
    let outbound = futures::sink::unfold(outbound_tx, |tx, msg: JsonRpcMessage| async move {
        tx.send(msg).map_err(|_| {
            ConnectionError::Broker(BrokerError::Join("sse writer task closed".into()))
        })?;
        Ok::<_, ConnectionError>(tx)
    });

    Ok(Connection::from_message_streams(
        Box::pin(inbound),
        Box::pin(outbound),
    ))
}

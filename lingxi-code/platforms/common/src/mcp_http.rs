//! MCP Streamable HTTP transport — POST request, JSON or text/event-stream response.
//! Matches claude-code's `StreamableHTTPClientTransport` from
//! `src/services/mcp/client.ts:784-901` (Accept: application/json, text/event-stream).
//!
//! Wire contract (LITERAL):
//! - POST `Content-Type: application/json` body is a single JSON-RPC frame.
//! - `Accept: application/json, text/event-stream` (the literal claude-code
//!   `MCP_STREAMABLE_HTTP_ACCEPT` const from `client.ts:471`).
//! - `User-Agent: claude-code/<CARGO_PKG_VERSION>` (matches `getMCPUserAgent()`
//!   shape from `utils/http.ts:37-50`).
//! - When `auth_token` is `Some`, POST carries
//!   `X-LingXi-Ide-Authorization: <token>` verbatim (no `Bearer ` prefix).
//! - Response body is EITHER a single `application/json` frame OR a
//!   `text/event-stream` body of zero-or-more frames; content-type selects.


use eventsource_stream::Eventsource;
use futures::StreamExt;
use jsonrpc::messages::Message as JsonRpcMessage;
use jsonrpc::{BrokerError, Connection, ConnectionError};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, CONTENT_TYPE, USER_AGENT};
use thiserror::Error;
use tokio::sync::mpsc;
use traits::mcp::McpError;

/// Literal `Accept` header value for Streamable HTTP. Matches claude-code's
/// `MCP_STREAMABLE_HTTP_ACCEPT` const (`client.ts:471`) byte-for-byte.
const STREAMABLE_HTTP_ACCEPT: &str = "application/json, text/event-stream";

/// `User-Agent` value emitted by this client. Matches claude-code's
/// `getMCPUserAgent()` shape: `claude-code/<version>`.
#[must_use]
pub fn user_agent() -> String {
    format!("claude-code/{}", env!("CARGO_PKG_VERSION"))
}

/// Errors specific to opening an MCP Streamable HTTP connection.
#[derive(Debug, Error)]
pub enum HttpConnectError {
    /// HTTP request setup failed.
    #[error("http transport error: {0}")]
    Transport(String),
    /// Authorization header value was invalid (non-ASCII, control chars).
    #[error("invalid auth token: {0}")]
    InvalidAuth(String),
}

impl From<HttpConnectError> for McpError {
    fn from(value: HttpConnectError) -> Self {
        Self::Connection(value.to_string())
    }
}

fn build_headers<H>(
    auth_token: Option<&str>,
    extra_headers: &H,
) -> Result<HeaderMap, HttpConnectError>
where
    for<'a> &'a H: IntoIterator<Item = (&'a String, &'a String)>,
{
    let mut h = HeaderMap::new();
    h.insert(ACCEPT, HeaderValue::from_static(STREAMABLE_HTTP_ACCEPT));
    h.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    h.insert(
        USER_AGENT,
        HeaderValue::try_from(user_agent())
            .map_err(|e| HttpConnectError::Transport(e.to_string()))?,
    );
    if let Some(token) = auth_token {
        let v = HeaderValue::try_from(token)
            .map_err(|e| HttpConnectError::InvalidAuth(e.to_string()))?;
        h.insert(
            HeaderName::from_static("x-lingxi-ide-authorization"),
            v,
        );
    }
    for (k, v) in extra_headers {
        let name = HeaderName::try_from(k.as_str())
            .map_err(|e| HttpConnectError::Transport(format!("bad header name {k}: {e}")))?;
        let val = HeaderValue::try_from(v.as_str())
            .map_err(|e| HttpConnectError::Transport(format!("bad header value: {e}")))?;
        h.insert(name, val);
    }
    Ok(h)
}

/// Open an MCP Streamable HTTP connection.
///
/// Each outbound JSON-RPC frame is `POSTed` to `url`. The server may reply with
/// either a single `application/json` body OR a `text/event-stream` body of
/// zero-or-more frames — both modes are decoded and routed back through
/// `jsonrpc::Connection`.
///
/// `extra_headers` is generic over the map type so both an unordered `HashMap`
/// and the insertion-ordered [`traits::McpHeaders`] (`IndexMap`) the MCP
/// transport specs now carry are accepted (header order is irrelevant to the
/// emitted HTTP request).
///
/// # Errors
///
/// - [`HttpConnectError::Transport`] if reqwest client construction fails.
/// - [`HttpConnectError::InvalidAuth`] if `auth_token` contains bytes that
///   cannot be expressed in an HTTP header value (non-ASCII, control chars).
pub async fn connect_http<H>(
    url: &str,
    auth_token: Option<&str>,
    extra_headers: &H,
) -> Result<Connection, HttpConnectError>
where
    H: Clone + Send + 'static,
    for<'a> &'a H: IntoIterator<Item = (&'a String, &'a String)>,
{
    let client = reqwest::Client::builder()
        .build()
        .map_err(|e| HttpConnectError::Transport(e.to_string()))?;

    // Validate header construction eagerly so configuration errors surface at
    // connect-time rather than on the first POST.
    let _ = build_headers(auth_token, extra_headers)?;

    // Inbound: frames produced by the POST writer task (from JSON or SSE
    // response bodies). Outbound: frames the Connection wants to POST.
    let (inbound_tx, inbound_rx) = mpsc::unbounded_channel::<JsonRpcMessage>();
    let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel::<JsonRpcMessage>();

    let post_url = url.to_string();
    let post_auth = auth_token.map(str::to_string);
    let post_extra = extra_headers.clone();

    tokio::spawn(async move {
        while let Some(frame) = outbound_rx.recv().await {
            let headers = match build_headers(post_auth.as_deref(), &post_extra) {
                Ok(h) => h,
                Err(e) => {
                    tracing::error!(error = %e, "mcp http: failed to build POST headers");
                    continue;
                }
            };

            let response = match client
                .post(&post_url)
                .headers(headers)
                .json(&frame)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(error = %e, "mcp http: POST failed");
                    continue;
                }
            };

            if !response.status().is_success() {
                tracing::warn!(status = %response.status(), "mcp http: non-success response");
                continue;
            }

            let content_type = response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_lowercase();

            if content_type.starts_with("text/event-stream") {
                // Streaming body: parse with eventsource-stream.
                let mut events = response.bytes_stream().eventsource();
                while let Some(item) = events.next().await {
                    match item {
                        Ok(ev) => {
                            if ev.data.is_empty() {
                                continue;
                            }
                            match serde_json::from_str::<JsonRpcMessage>(&ev.data) {
                                Ok(msg) => {
                                    if inbound_tx.send(msg).is_err() {
                                        return;
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        error = %e,
                                        data = %ev.data,
                                        "mcp http: malformed SSE JSON frame, skipping"
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "mcp http: sse parse error");
                            break;
                        }
                    }
                }
            } else {
                // Single JSON object response.
                match response.json::<JsonRpcMessage>().await {
                    Ok(msg) => {
                        if inbound_tx.send(msg).is_err() {
                            return;
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "mcp http: response not valid JSON-RPC");
                    }
                }
            }
        }
    });

    // Adapt mpsc channels to the Stream/Sink shape `from_message_streams` wants.
    let inbound = futures::stream::unfold(inbound_rx, |mut rx| async move {
        rx.recv().await.map(|m| (m, rx))
    });
    let outbound = futures::sink::unfold(outbound_tx, |tx, msg: JsonRpcMessage| async move {
        tx.send(msg).map_err(|_| {
            ConnectionError::Broker(BrokerError::Join("http writer task closed".into()))
        })?;
        Ok::<_, ConnectionError>(tx)
    });

    Ok(Connection::from_message_streams(
        Box::pin(inbound),
        Box::pin(outbound),
    ))
}

//! MCP Streamable HTTP transport — placeholder. Real implementation lands
//! in Task 5 of plan `docs/superpowers/plans/2026-05-23-m2-02d-mcp-sse-http-bridge.md`.
//!
//! Will provide [`connect_http`] which POSTs JSON-RPC frames to `url` with
//! `Content-Type: application/json` and
//! `Accept: application/json, text/event-stream` (the literal claude-code
//! `MCP_STREAMABLE_HTTP_ACCEPT` header value). Responses can come back as a
//! single JSON object OR as a stream of SSE-framed events.

use lingxi_jsonrpc::Connection;
use lingxi_traits::mcp::McpError;
use std::collections::HashMap;
use thiserror::Error;

/// Errors specific to opening an MCP Streamable HTTP connection.
#[derive(Debug, Error)]
pub enum HttpConnectError {
    /// Generic transport-side failure.
    #[error("http transport error: {0}")]
    Transport(String),
}

impl From<HttpConnectError> for McpError {
    fn from(value: HttpConnectError) -> Self {
        Self::Connection(value.to_string())
    }
}

/// Opens a Streamable HTTP MCP connection. Implemented in Task 5.
#[allow(clippy::implicit_hasher)] // public API: HashMap is the documented config shape
pub async fn connect_http(
    _url: &str,
    _auth_token: Option<&str>,
    _extra_headers: &HashMap<String, String>,
) -> Result<Connection, HttpConnectError> {
    Err(HttpConnectError::Transport(
        "connect_http not yet implemented (Task 5)".into(),
    ))
}

//! MCP SSE transport — placeholder. Real implementation lands in Task 3.
//!
//! Will provide [`connect_sse`] that opens a `text/event-stream` GET against
//! `url`, parses each `data: <json>\n\n` frame into a JSON-RPC `Message`, and
//! POSTs outbound JSON-RPC requests to the SAME URL with
//! `Content-Type: application/json`. Auth flows via the
//! `X-Claude-Code-Ide-Authorization` header verbatim.

use lingxi_jsonrpc::Connection;
use lingxi_traits::mcp::McpError;
use std::collections::HashMap;
use thiserror::Error;

/// Header name used by claude-code IDE plugins for the auth token.
/// LITERAL — must match claude-code byte-for-byte.
pub const IDE_AUTH_HEADER: &str = "X-Claude-Code-Ide-Authorization";

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

/// Opens an SSE event-stream connection. Implemented in Task 3 of plan
/// `docs/superpowers/plans/2026-05-23-m2-02d-mcp-sse-http-bridge.md`.
#[allow(clippy::implicit_hasher)] // public API: HashMap is the documented config shape
pub async fn connect_sse(
    _url: &str,
    _auth_token: Option<&str>,
    _extra_headers: &HashMap<String, String>,
) -> Result<Connection, SseConnectError> {
    Err(SseConnectError::Transport(
        "connect_sse not yet implemented (Task 3)".into(),
    ))
}

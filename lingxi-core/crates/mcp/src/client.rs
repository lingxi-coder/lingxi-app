//! `McpClient` wrapping `lingxi_jsonrpc::Connection`.
//!
//! Full RPC body (initialize, tools/list, tools/call, prompts/list,
//! prompts/get, resources/list, resources/read, ping) lands in M2-02b
//! Tasks 6-12. This file currently exposes only the struct shell + error
//! enum so the public surface in `lib.rs` resolves.

use std::path::PathBuf;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::RwLock;

use lingxi_traits::ServerCapabilitiesDto;

/// Async MCP client built on top of a [`lingxi_jsonrpc::Connection`].
///
/// One instance per server connection; owns its `Connection` and inbound
/// handler registrations.
pub struct McpClient {
    /// Logical server name (used in tool full-names and error messages).
    #[allow(dead_code)] // wired in Tasks 6-12
    server_name: String,
    /// Absolute cwd advertised to the server via `roots/list`.
    #[allow(dead_code)] // wired in Tasks 4-5
    cwd: PathBuf,
    /// Underlying JSON-RPC connection produced by the platform transport.
    #[allow(dead_code)] // wired in Tasks 6-12
    connection: Arc<lingxi_jsonrpc::Connection>,
    /// Server capabilities snapshot from the `initialize` response.
    #[allow(dead_code)] // populated in Task 6
    server_capabilities: RwLock<Option<ServerCapabilitiesDto>>,
    /// Server-provided instructions string from the `initialize` response,
    /// truncated to `MAX_MCP_DESCRIPTION_LENGTH` chars on receipt
    /// (matches claude-code `client.ts:1163-1166`).
    #[allow(dead_code)] // populated in Task 6
    server_instructions: RwLock<Option<String>>,
}

/// Errors emitted by [`McpClient`] operations.
#[derive(Debug, Error)]
pub enum McpClientError {
    /// Tool call exceeded the configured timeout. Message format is wire-
    /// locked: `MCP server "<server>" tool "<tool>" timed out after <secs>s`.
    #[error("MCP server \"{server}\" tool \"{tool}\" timed out after {secs}s")]
    Timeout {
        /// Logical MCP server name from [`McpClient::new`].
        server: String,
        /// Tool name from the failing `tools/call` invocation.
        tool: String,
        /// Configured timeout (seconds).
        secs: u64,
    },
    /// Underlying JSON-RPC transport returned an error response or framing
    /// failure; the inner string is the stringified `JsonRpcError`.
    #[error("JSON-RPC error: {0}")]
    Rpc(String),
    /// Server returned a syntactically valid response that did not match
    /// the expected DTO shape.
    #[error("malformed response: {0}")]
    Deserialize(String),
    /// `initialize` handshake failed.
    #[error("initialize failed: {0}")]
    Initialize(String),
}

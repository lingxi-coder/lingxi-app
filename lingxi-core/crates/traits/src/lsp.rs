//! LSP (Language Server Protocol) transport abstraction.
//!
//! `LspTransport` is the boundary between `lingxi-lsp` (the registry +
//! per-connection state machine) and the platform-specific transport that
//! actually speaks JSON-RPC to language servers. The engine never speaks
//! the wire protocol directly.
//!
//! See spec §25 (LSP) and D17 (Runtime boundary).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

/// Configuration describing one LSP server the engine knows how to start.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspServerConfig {
    /// Logical server name (used as registry key).
    pub name: String,
    /// Executable to spawn (e.g. `rust-analyzer`).
    pub command: String,
    /// CLI arguments for the server.
    pub args: Vec<String>,
    /// Extra environment variables.
    pub env: std::collections::HashMap<String, String>,
    /// Languages this server handles (e.g. `["rust"]`).
    pub trigger_languages: Vec<String>,
    /// Markers used to detect the project root (e.g. `["Cargo.toml"]`).
    pub root_dir_markers: Vec<String>,
    /// Optional server-specific initialization payload.
    pub initialization_options: Option<Value>,
}

/// Handle returned by a successful [`LspTransport::start_server`].
///
/// Reuses [`lingxi_protocol::McpConnectionId`] to keep the connection-id
/// namespace shared between MCP and LSP — both are connection-level
/// routing keys handled by the same registry machinery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspRawConnection {
    /// Stable connection identifier.
    pub connection_id: lingxi_protocol::McpConnectionId,
}

/// Server capability flags returned by `initialize`.
///
/// Mirrors the LSP capability object — booleans indicate whether the
/// server exposes the corresponding feature category.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)] // mirrors the LSP wire spec verbatim
pub struct LspServerCapabilities {
    /// Encoded `textDocumentSyncKind` (`"full"`, `"incremental"`, ...).
    pub text_document_sync: Option<String>,
    /// `textDocument/completion` supported.
    pub completion: bool,
    /// `textDocument/hover` supported.
    pub hover: bool,
    /// `textDocument/definition` supported.
    pub definition: bool,
    /// `textDocument/references` supported.
    pub references: bool,
    /// `textDocument/publishDiagnostics` supported.
    pub diagnostics: bool,
    /// `textDocument/documentSymbol` / `workspace/symbol` supported.
    pub symbols: bool,
    /// `textDocument/formatting` supported.
    pub formatting: bool,
    /// `textDocument/rename` supported.
    pub rename: bool,
    /// `textDocument/codeAction` supported.
    pub code_action: bool,
}

/// Transport boundary between the engine's LSP layer and the wire protocol.
///
/// Implementations live in platform crates. The engine builds an
/// [`LspServerConfig`], calls [`Self::start_server`], performs the
/// `initialize` handshake via [`Self::initialize`], and then issues
/// requests through [`Self::request`] / [`Self::notify`].
#[async_trait]
pub trait LspTransport: Send + Sync {
    /// Spawn the server described by `config` and return a connection
    /// handle.
    ///
    /// # Errors
    /// Returns [`LspError`] when the transport cannot launch the server or
    /// is unsupported on the current platform.
    async fn start_server(&self, config: &LspServerConfig) -> Result<LspRawConnection, LspError>;

    /// Perform the LSP `initialize` handshake against `root_uri` and return
    /// the server's declared capabilities.
    ///
    /// # Errors
    /// Returns [`LspError::Transport`] / [`LspError::ServerError`] on
    /// handshake failure.
    async fn initialize(
        &self,
        conn: &LspRawConnection,
        root_uri: &str,
    ) -> Result<LspServerCapabilities, LspError>;

    /// Issue an LSP request and await the JSON-RPC response.
    ///
    /// # Errors
    /// Returns [`LspError::Transport`] when the channel breaks, or
    /// [`LspError::ServerError`] when the server returns a JSON-RPC error.
    async fn request(
        &self,
        conn: &LspRawConnection,
        method: &str,
        params: Value,
    ) -> Result<Value, LspError>;

    /// Issue an LSP notification (no response expected).
    ///
    /// # Errors
    /// Returns [`LspError::Transport`] when the notification cannot be
    /// delivered.
    async fn notify(
        &self,
        conn: &LspRawConnection,
        method: &str,
        params: Value,
    ) -> Result<(), LspError>;

    /// Shutdown and tear down the server identified by `conn_id`.
    ///
    /// # Errors
    /// Returns [`LspError::Transport`] when the shutdown cannot be
    /// completed gracefully.
    async fn shutdown(&self, conn_id: lingxi_protocol::McpConnectionId) -> Result<(), LspError>;

    /// Whether this transport can carry LSP traffic on the current platform.
    fn is_available(&self) -> bool;
}

/// Failure modes shared by every [`LspTransport`] method.
#[derive(Debug, Clone, Error)]
pub enum LspError {
    /// Transport is not implemented on the current platform.
    #[error("unavailable")]
    Unavailable,
    /// Underlying transport (stdio / TCP) failed.
    #[error("transport error: {0}")]
    Transport(String),
    /// Server returned a JSON-RPC error response.
    #[error("server response: {0}")]
    ServerError(String),
}

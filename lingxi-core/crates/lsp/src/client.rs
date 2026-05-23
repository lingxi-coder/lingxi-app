//! `LspClient` — typed JSON-RPC wrapper around a `lingxi_jsonrpc::Connection`.
//!
//! Mirrors claude-code's `LSPClient` (`claude-code/src/services/lsp/LSPClient.ts`)
//! in shape: typed `initialize`, untyped `request<P, R>` for arbitrary LSP
//! methods, fire-and-forget `notify`, and a `shutdown` sequence that issues
//! the canonical LSP `shutdown` request + `exit` notification.
//!
//! The `Connection` itself is produced by the platform layer (it owns the
//! child process and the stdio pipes); the client does not spawn anything.

use std::sync::Arc;

use lingxi_jsonrpc::{Connection, ConnectionError};
use lingxi_traits::LspError;
use lsp_types::{
    ClientCapabilities, InitializeParams, InitializeResult, ServerCapabilities,
    WorkspaceClientCapabilities,
};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tracing::{debug, warn};

/// Typed client over a JSON-RPC connection to one LSP server.
pub struct LspClient {
    /// Human-readable server name (for tracing).
    name: String,
    /// JSON-RPC plumbing — handles framing, routing, notifications.
    connection: Arc<Connection>,
    /// Server capabilities populated after `initialize` succeeds.
    capabilities: RwLock<Option<ServerCapabilities>>,
}

impl LspClient {
    /// Build a new client around an already-established `Connection`.
    ///
    /// The connection must have been started (its reader/writer tasks
    /// spawned). The client takes shared ownership and uses it for the
    /// lifetime of the LSP server.
    #[must_use]
    pub fn new(name: String, connection: Connection) -> Self {
        Self {
            name,
            connection: Arc::new(connection),
            capabilities: RwLock::new(None),
        }
    }

    /// Construct an `LspClient` from an already-shared `Arc<Connection>`.
    #[must_use]
    pub fn with_shared(name: String, connection: Arc<Connection>) -> Self {
        Self {
            name,
            connection,
            capabilities: RwLock::new(None),
        }
    }

    /// Borrow the underlying JSON-RPC connection (used by `passive_feedback`
    /// to subscribe to notifications).
    #[must_use]
    pub fn connection(&self) -> Arc<Connection> {
        Arc::clone(&self.connection)
    }

    /// LSP server name (registry key).
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Latest cached capabilities; `None` until `initialize` succeeds.
    pub async fn capabilities(&self) -> Option<ServerCapabilities> {
        self.capabilities.read().await.clone()
    }

    /// Perform the LSP `initialize` handshake.
    ///
    /// Sends an `initialize` request followed by the `initialized`
    /// notification per LSP 3.17 spec. Caches the returned
    /// `ServerCapabilities` for later inspection.
    ///
    /// # Errors
    /// Returns [`LspError::Transport`] when the request cannot be delivered,
    /// or [`LspError::ServerError`] when the server returns a JSON-RPC error.
    pub async fn initialize(&self, root_uri: &str) -> Result<ServerCapabilities, LspError> {
        // The LSP spec is permissive about which client capabilities we
        // declare; we send a minimal-but-not-empty shape so servers like
        // `rust-analyzer` and `gopls` don't disable optional features.
        let parsed_root = lsp_types::Url::parse(root_uri)
            .map_err(|e| LspError::Transport(format!("invalid root_uri: {e}")))?;
        // `root_uri` was deprecated in LSP 3.6 in favor of `workspaceFolders`,
        // but rust-analyzer / gopls / pyright still consult it during
        // `initialize`, so we populate both. The allow-block scopes the
        // `deprecated` lint to the deprecated field assignment only.
        let params = {
            #[allow(deprecated)]
            InitializeParams {
                process_id: Some(std::process::id()),
                root_uri: Some(parsed_root),
                capabilities: ClientCapabilities {
                    workspace: Some(WorkspaceClientCapabilities {
                        workspace_folders: Some(true),
                        configuration: Some(true),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                ..Default::default()
            }
        };

        let result: InitializeResult = self.request("initialize", params).await?;

        // Cache capabilities then send the `initialized` notification.
        *self.capabilities.write().await = Some(result.capabilities.clone());
        self.notify("initialized", json!({})).await?;
        debug!(
            target: "lingxi_lsp::client",
            server = %self.name,
            "initialize complete"
        );
        Ok(result.capabilities)
    }

    /// Issue a JSON-RPC request and decode the response.
    ///
    /// `method` is the LSP method name (e.g. `"textDocument/hover"`).
    /// `params` is serialized as the request `params` field; the response
    /// `result` is deserialized into `R`.
    ///
    /// # Errors
    /// Returns [`LspError::Transport`] on transport failure or serialization
    /// errors, and [`LspError::ServerError`] when the server returns a
    /// JSON-RPC error response.
    pub async fn request<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &str,
        params: P,
    ) -> Result<R, LspError> {
        let params_value = serde_json::to_value(params)
            .map_err(|e| LspError::Transport(format!("serialize {method} params: {e}")))?;
        let result_value: Value = self
            .connection
            .call(method, params_value)
            .await
            .map_err(|e| map_connection_error(method, e))?;
        serde_json::from_value(result_value)
            .map_err(|e| LspError::Transport(format!("decode {method} result: {e}")))
    }

    /// Issue a JSON-RPC notification (fire-and-forget).
    ///
    /// Kept `async` for symmetry with [`Self::request`] / [`Self::shutdown`]
    /// — callers can `.await` notify alongside request calls without
    /// branching their code paths, and the signature is forward-compatible
    /// if the underlying router ever needs to await backpressure.
    ///
    /// # Errors
    /// Returns [`LspError::Transport`] when the writer task has terminated
    /// (e.g. the LSP server crashed). Notifications never produce
    /// `ServerError` because no response is expected.
    #[allow(clippy::unused_async)]
    pub async fn notify<P: Serialize>(&self, method: &str, params: P) -> Result<(), LspError> {
        let params_value = serde_json::to_value(params)
            .map_err(|e| LspError::Transport(format!("serialize {method} notify: {e}")))?;
        self.connection
            .notify(method, params_value)
            .map_err(|e| map_connection_error(method, e))
    }

    /// LSP shutdown sequence — `shutdown` request followed by `exit`
    /// notification, per LSP 3.17 `§Lifecycle Messages`.
    ///
    /// Some servers don't respond cleanly to `shutdown`; we treat
    /// shutdown-request failures as warnings (debug-logged) and still send
    /// `exit` so the server process knows to terminate.
    ///
    /// # Errors
    /// Returns [`LspError::Transport`] only if `exit` itself cannot be
    /// delivered (which usually means the connection is already dead).
    pub async fn shutdown(&self) -> Result<(), LspError> {
        match self
            .request::<_, Option<Value>>("shutdown", json!(null))
            .await
        {
            Ok(_) => debug!(
                target: "lingxi_lsp::client",
                server = %self.name,
                "shutdown ack"
            ),
            Err(e) => warn!(
                target: "lingxi_lsp::client",
                server = %self.name,
                error = %e,
                "shutdown errored; continuing to exit"
            ),
        }
        self.notify("exit", json!(null)).await
    }
}

/// Map a `lingxi_jsonrpc::ConnectionError` to the trait-level `LspError`.
///
/// Server-returned JSON-RPC errors (`RouterError::Remote`) become
/// `LspError::ServerError`; every other failure mode (timeout, writer
/// closed, serde, broker) is reported as `LspError::Transport` with the
/// method name prefixed for diagnostics.
fn map_connection_error(method: &str, err: ConnectionError) -> LspError {
    use lingxi_jsonrpc::router::RouterError;
    match err {
        ConnectionError::Router(RouterError::Remote(remote)) => LspError::ServerError(format!(
            "{method} returned code {}: {}",
            remote.code, remote.message
        )),
        other => LspError::Transport(format!("{method}: {other}")),
    }
}

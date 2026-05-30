//! LSP transport — POSIX wiring of `lingxi-lsp::LspClient` over a
//! `tokio::process::Command` child speaking JSON-RPC on stdin/stdout.
//!
//! ## Spawn handshake
//!
//! 1. `tokio::process::Command::spawn()` with `stdin/stdout/stderr` all
//!    `Stdio::piped()` and `kill_on_drop(true)` so the child does not
//!    outlive the transport.
//! 2. **Immediately** call `child.stdin.take()` and `child.stdout.take()`.
//!    If either returns `None`, the spawn race lost (ENOENT, etc.) and the
//!    connection reader task in `lingxi-jsonrpc` would otherwise panic on
//!    a missing stream — we bail with [`LspError::Transport`] first.
//!    (claude-code makes the same guard explicit, see `LSPClient.ts:106`.)
//! 3. Build a `jsonrpc::Connection::new_lsp` over the stdout
//!    reader + stdin writer, then wrap it in [`lsp::LspClient`].
//!
//! The trait's untyped `request/notify` methods delegate to the typed
//! client via `Value` serialization.

use async_trait::async_trait;
use jsonrpc::Connection;
use lsp::LspClient;
use protocol::McpConnectionId;
use serde_json::Value;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tracing::warn;
use traits::{LspError, LspRawConnection, LspServerCapabilities, LspServerConfig, LspTransport};

/// Per-connection bundle: the child process handle + the typed client.
struct ConnectionEntry {
    /// Kept alive so `kill_on_drop(true)` cleans up the child when we drop it.
    #[allow(dead_code)]
    child: Child,
    client: Arc<LspClient>,
}

/// POSIX LSP transport — spawns LSP server processes via `tokio::process`.
#[derive(Default)]
pub struct PosixLspTransport {
    connections: Mutex<HashMap<McpConnectionId, ConnectionEntry>>,
}

impl PosixLspTransport {
    /// Construct a new `PosixLspTransport`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    async fn lookup_client(&self, id: McpConnectionId) -> Option<Arc<LspClient>> {
        self.connections
            .lock()
            .await
            .get(&id)
            .map(|e| Arc::clone(&e.client))
    }
}

#[async_trait]
impl LspTransport for PosixLspTransport {
    async fn start_server(&self, config: &LspServerConfig) -> Result<LspRawConnection, LspError> {
        let mut cmd = Command::new(&config.command);
        cmd.args(&config.args);
        for (k, v) in &config.env {
            cmd.env(k, v);
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = cmd
            .spawn()
            .map_err(|e| LspError::Transport(format!("spawn {}: {}", config.command, e)))?;

        let stdin = child.stdin.take().ok_or_else(|| {
            LspError::Transport(format!(
                "LSP server '{}' stdin not available (spawn race)",
                config.name
            ))
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            LspError::Transport(format!(
                "LSP server '{}' stdout not available (spawn race)",
                config.name
            ))
        })?;

        // Drain stderr into a debug log so the user has diagnostics if the
        // server misbehaves. Don't propagate stderr errors — they're
        // expected to occur when the server exits.
        if let Some(stderr) = child.stderr.take() {
            let name = config.name.clone();
            tokio::spawn(async move {
                use tokio::io::{AsyncBufReadExt, BufReader};
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::debug!(target: "lingxi_lsp::stderr", server = %name, "{}", line);
                }
            });
        }

        let connection = Connection::new_lsp(stdout, stdin);
        let client = Arc::new(LspClient::with_shared(
            config.name.clone(),
            Arc::new(connection),
        ));

        let id = McpConnectionId::new();
        self.connections
            .lock()
            .await
            .insert(id, ConnectionEntry { child, client });
        Ok(LspRawConnection { connection_id: id })
    }

    async fn initialize(
        &self,
        conn: &LspRawConnection,
        root_uri: &str,
    ) -> Result<LspServerCapabilities, LspError> {
        let client = self
            .lookup_client(conn.connection_id)
            .await
            .ok_or_else(|| LspError::Transport("connection not found".into()))?;
        let caps = client.initialize(root_uri).await?;
        Ok(LspServerCapabilities {
            text_document_sync: caps.text_document_sync.map(|_| "Full".to_string()),
            completion: caps.completion_provider.is_some(),
            hover: caps.hover_provider.is_some(),
            definition: caps.definition_provider.is_some(),
            references: caps.references_provider.is_some(),
            diagnostics: true, // assume publishDiagnostics; we subscribe regardless
            symbols: caps.document_symbol_provider.is_some(),
            formatting: caps.document_formatting_provider.is_some(),
            rename: caps.rename_provider.is_some(),
            code_action: caps.code_action_provider.is_some(),
        })
    }

    async fn request(
        &self,
        conn: &LspRawConnection,
        method: &str,
        params: Value,
    ) -> Result<Value, LspError> {
        let client = self
            .lookup_client(conn.connection_id)
            .await
            .ok_or_else(|| LspError::Transport("connection not found".into()))?;
        client.request::<Value, Value>(method, params).await
    }

    async fn notify(
        &self,
        conn: &LspRawConnection,
        method: &str,
        params: Value,
    ) -> Result<(), LspError> {
        let client = self
            .lookup_client(conn.connection_id)
            .await
            .ok_or_else(|| LspError::Transport("connection not found".into()))?;
        client.notify(method, params).await
    }

    async fn shutdown(&self, conn_id: McpConnectionId) -> Result<(), LspError> {
        let removed = self.connections.lock().await.remove(&conn_id);
        if let Some(entry) = removed {
            // Polite shutdown sequence; ignore errors (server may already be dead).
            if let Err(e) = entry.client.shutdown().await {
                warn!(target: "lingxi_lsp::posix", error = %e, "polite shutdown failed; killing child");
            }
            // `kill_on_drop(true)` handles the kill when `entry` falls out of scope.
            drop(entry);
        }
        Ok(())
    }

    fn is_available(&self) -> bool {
        true
    }
}

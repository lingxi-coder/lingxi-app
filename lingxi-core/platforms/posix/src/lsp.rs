//! LSP transport — POSIX process orchestration.
//!
//! M2.02 spawns the language server child process and tracks the
//! connection handle, but defers full `JSON-RPC` framing (`Content-Length`
//! headers + request id correlation) to M2 phase 3. `request` returns
//! `LspError::Transport("M2 follow-up")` until that lands.

use async_trait::async_trait;
use lingxi_protocol::McpConnectionId;
use lingxi_traits::{
    LspError, LspRawConnection, LspServerCapabilities, LspServerConfig, LspTransport,
};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Mutex;
use tokio::process::Child;

/// POSIX LSP transport — spawns LSP server processes via `tokio::process`.
#[derive(Default)]
pub struct PosixLspTransport {
    connections: Mutex<HashMap<McpConnectionId, Child>>,
}

impl PosixLspTransport {
    /// Construct a new `PosixLspTransport`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl LspTransport for PosixLspTransport {
    async fn start_server(&self, config: &LspServerConfig) -> Result<LspRawConnection, LspError> {
        let mut cmd = tokio::process::Command::new(&config.command);
        cmd.args(&config.args);
        for (k, v) in &config.env {
            cmd.env(k, v);
        }
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let child = cmd
            .spawn()
            .map_err(|e| LspError::Transport(e.to_string()))?;
        let id = McpConnectionId::new();
        if let Ok(mut conns) = self.connections.lock() {
            conns.insert(id, child);
        }
        Ok(LspRawConnection { connection_id: id })
    }

    async fn initialize(
        &self,
        _conn: &LspRawConnection,
        _root_uri: &str,
    ) -> Result<LspServerCapabilities, LspError> {
        // M2.02 stub — real `initialize` handshake lands in M2 phase 3.
        Ok(LspServerCapabilities {
            text_document_sync: Some("Full".into()),
            completion: true,
            hover: true,
            definition: true,
            references: true,
            diagnostics: true,
            symbols: true,
            formatting: true,
            rename: true,
            code_action: true,
        })
    }

    async fn request(
        &self,
        _conn: &LspRawConnection,
        _method: &str,
        _params: Value,
    ) -> Result<Value, LspError> {
        Err(LspError::Transport(
            "M2 follow-up: full LSP JSON-RPC framing".into(),
        ))
    }

    async fn notify(
        &self,
        _conn: &LspRawConnection,
        _method: &str,
        _params: Value,
    ) -> Result<(), LspError> {
        Ok(())
    }

    async fn shutdown(&self, conn_id: McpConnectionId) -> Result<(), LspError> {
        let child_opt = self
            .connections
            .lock()
            .ok()
            .and_then(|mut conns| conns.remove(&conn_id));
        if let Some(mut child) = child_opt {
            let _ = child.kill().await;
        }
        Ok(())
    }

    fn is_available(&self) -> bool {
        true
    }
}

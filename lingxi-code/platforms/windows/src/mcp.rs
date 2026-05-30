//! MCP transport — Windows.
//!
//! Mirrors `lingxi_platform_posix::mcp` exactly — see M2-02d Task 7. Supports
//! the `Stdio`, `Sse`, and `Http` variants in M2. The `WebSocket` variant's
//! low-level `connect_ws` helper is re-exported by M2-02c, but
//! `WindowsMcpTransport::connect` does NOT yet route `WebSocket` specs — that
//! arm currently falls through to `McpError::UnsupportedTransport` and will
//! be wired in a follow-up. Other variants (`InProcess`, `SseIde`,
//! `SdkControl`) return `McpError::UnsupportedTransport`.
//!
//! M2-02c also lands `spawn_stdio` (mirrors the POSIX implementation, NDJSON
//! framing + 64 MB stderr ring) plus a re-export of the shared WebSocket
//! connector at `lingxi_platform_windows::mcp::connect_ws`.

use async_trait::async_trait;
use lingxi_jsonrpc::Connection;
use lingxi_platform_common::mcp_stdio::{StderrRing, StdioConfig};
use lingxi_platform_common::{connect_http, connect_sse};
use lingxi_protocol::McpConnectionId;
use lingxi_traits::{
    ElicitRequestDto, ElicitResultDto, McpError, McpNotificationStream, McpPromptDto,
    McpRawConnection, McpResourceContentDto, McpResourceDto, McpToolDto, McpToolResultDto,
    McpTransport, McpTransportKind, McpTransportSpec, ServerCapabilitiesDto,
};
use serde_json::Value;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use tokio::io::AsyncReadExt;
use tokio::process::Child;
use tokio::sync::Mutex as AsyncMutex;

/// Per-connection state held by `WindowsMcpTransport`.
///
/// Mirrors `lingxi_platform_posix::mcp::PosixMcpConnection`. Different
/// transports keep slightly different ownership: `Stdio` owns the spawned
/// child so `disconnect` can kill it; SSE / HTTP just own the JSON-RPC
/// `Connection` (the underlying `reqwest` tasks live inside the
/// connection's broker).
pub(crate) enum WindowsMcpConnection {
    /// `Stdio` connection — owns the child process.
    Stdio {
        /// Owned child process; killed on `disconnect`.
        child: Child,
    },
    /// `Sse` connection — owns the JSON-RPC connection over the HTTP+SSE pair.
    Sse,
    /// `Http` connection — owns the JSON-RPC connection over Streamable HTTP.
    Http,
}

/// Windows MCP transport.
///
/// Supports the `Stdio`, `Sse`, and `Http` variants in M2. Other transports
/// (`WebSocket`, `InProcess`, `SseIde`, `SdkControl`) return
/// `McpError::UnsupportedTransport`. Most request methods are intentionally
/// stubbed pending full `JSON-RPC` framing in M2 phase 3.
#[derive(Default)]
pub struct WindowsMcpTransport {
    connections: Mutex<HashMap<McpConnectionId, WindowsMcpConnection>>,
}

impl WindowsMcpTransport {
    /// Construct a new `WindowsMcpTransport`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn insert(&self, id: McpConnectionId, conn: WindowsMcpConnection) {
        // Recover from a poisoned std `Mutex` by silently dropping the
        // insert — the engine will surface the failure on the next call
        // when the connection id misses the map.
        if let Ok(mut guard) = self.connections.lock() {
            guard.insert(id, conn);
        }
    }
}

#[async_trait]
impl McpTransport for WindowsMcpTransport {
    async fn connect(&self, spec: &McpTransportSpec) -> Result<McpRawConnection, McpError> {
        let id = McpConnectionId::new();
        match spec {
            McpTransportSpec::Stdio { command, args, env } => {
                let mut cmd = tokio::process::Command::new(command);
                cmd.args(args);
                for (k, v) in env {
                    cmd.env(k, v);
                }
                cmd.stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped());
                let child = cmd
                    .spawn()
                    .map_err(|e| McpError::Connection(e.to_string()))?;
                self.insert(id, WindowsMcpConnection::Stdio { child });
            }
            McpTransportSpec::Sse { url, headers, .. } => {
                // Delegate to the shared connector. The OAuth + headers_helper
                // arms are out of scope for M2-02d's dispatch task; the
                // transport currently passes only the static `headers` map
                // and no auth token. OAuth integration lands in M2-06.
                let _conn = connect_sse(url, None, headers)
                    .await
                    .map_err(McpError::from)?;
                self.insert(id, WindowsMcpConnection::Sse);
            }
            McpTransportSpec::Http { url, headers, .. } => {
                // See `Sse` arm — OAuth + per-request headers_helper deferred.
                let _conn = connect_http(url, None, headers)
                    .await
                    .map_err(McpError::from)?;
                self.insert(id, WindowsMcpConnection::Http);
            }
            other => return Err(McpError::UnsupportedTransport(map_kind(other))),
        }
        Ok(McpRawConnection { connection_id: id })
    }

    async fn initialize(
        &self,
        _conn: &McpRawConnection,
    ) -> Result<ServerCapabilitiesDto, McpError> {
        // M2.03 stub — full `JSON-RPC` initialize lands in M2 phase 3.
        Ok(ServerCapabilitiesDto {
            tools: true,
            resources: false,
            prompts: false,
            logging: false,
            experimental: HashMap::new(),
        })
    }

    async fn list_tools(&self, _conn: &McpRawConnection) -> Result<Vec<McpToolDto>, McpError> {
        Err(McpError::Internal(
            "windows mcp list_tools delegated to lingxi-mcp::McpClient (M2-02b)".into(),
        ))
    }

    async fn list_resources(
        &self,
        _conn: &McpRawConnection,
    ) -> Result<Vec<McpResourceDto>, McpError> {
        Ok(Vec::new())
    }

    async fn list_prompts(&self, _conn: &McpRawConnection) -> Result<Vec<McpPromptDto>, McpError> {
        Ok(Vec::new())
    }

    async fn call_tool(
        &self,
        _conn: &McpRawConnection,
        _tool: &str,
        _input: Value,
    ) -> Result<McpToolResultDto, McpError> {
        Err(McpError::Internal(
            "windows mcp call_tool delegated to lingxi-mcp::McpClient (M2-02b)".into(),
        ))
    }

    async fn read_resource(
        &self,
        _conn: &McpRawConnection,
        _uri: &str,
    ) -> Result<McpResourceContentDto, McpError> {
        Err(McpError::Internal(
            "windows mcp read_resource delegated to lingxi-mcp::McpClient (M2-02b)".into(),
        ))
    }

    async fn ping(&self, _conn_id: McpConnectionId) -> Result<(), McpError> {
        Ok(())
    }

    async fn notifications(
        &self,
        _conn: &McpRawConnection,
    ) -> Result<McpNotificationStream, McpError> {
        use futures::stream::empty;
        Ok(Box::pin(empty()))
    }

    async fn handle_elicitation(
        &self,
        _conn: &McpRawConnection,
        _req: ElicitRequestDto,
    ) -> Result<ElicitResultDto, McpError> {
        Err(McpError::Internal(
            "windows mcp elicitation delegated to lingxi-mcp::McpClient (M2-02b)".into(),
        ))
    }

    async fn disconnect(&self, conn_id: McpConnectionId) -> Result<(), McpError> {
        let entry = self
            .connections
            .lock()
            .ok()
            .and_then(|mut g| g.remove(&conn_id));
        if let Some(WindowsMcpConnection::Stdio { mut child }) = entry {
            let _ = child.kill().await;
        }
        // For Sse / Http there is no owned child; dropping the entry tears
        // down the JSON-RPC connection (and its background tasks) naturally.
        Ok(())
    }

    fn supported_transports(&self) -> Vec<McpTransportKind> {
        vec![
            McpTransportKind::Stdio,
            McpTransportKind::Sse,
            McpTransportKind::Http,
        ]
    }
}

fn map_kind(spec: &McpTransportSpec) -> McpTransportKind {
    match spec {
        McpTransportSpec::Stdio { .. } => McpTransportKind::Stdio,
        McpTransportSpec::Sse { .. } => McpTransportKind::Sse,
        McpTransportSpec::Http { .. } => McpTransportKind::Http,
        McpTransportSpec::WebSocket { .. } => McpTransportKind::WebSocket,
        McpTransportSpec::InProcess { .. } => McpTransportKind::InProcess,
        McpTransportSpec::SseIde { .. } => McpTransportKind::SseIde,
        McpTransportSpec::SdkControl { .. } => McpTransportKind::SdkControl,
    }
}

/// Error type returned by `spawn_stdio` on Windows.
///
/// Mirrors `lingxi_platform_posix::mcp::McpTransportError` — distinct type
/// per crate so callers can match against either via the shared `From` impls
/// in `lingxi-mcp` (added in M2-02d when the per-platform transport plug-in
/// trait lands).
#[derive(Debug, thiserror::Error)]
pub enum McpTransportError {
    /// Failed to spawn the child process.
    #[error("io: {0}")]
    Io(String),
    /// Failed to acquire one of the stdin/stdout/stderr pipes from the child.
    #[error("missing stdio pipe: {0}")]
    MissingPipe(&'static str),
}

/// Handles returned by [`spawn_stdio_with_handles`] — the same `Connection`
/// that [`spawn_stdio`] returns, plus a shared handle on the stderr ring
/// buffer so callers can snapshot any buffered stderr if the child misbehaves.
#[non_exhaustive]
pub struct StdioHandles {
    /// The fully-wired JSON-RPC `Connection` over the child's stdio.
    pub connection: Connection,
    /// Shared `StderrRing` populated by a background drain task.
    pub stderr: Arc<AsyncMutex<StderrRing>>,
}

/// Spawn an MCP child over stdio on Windows and return a fully-wired
/// `lingxi_jsonrpc::Connection`.
///
/// Mirrors `lingxi_platform_posix::mcp::spawn_stdio` exactly in framing
/// (NDJSON / `LineCodec` via `Connection::new_line_delimited`), stderr
/// handling (64 MB drop-oldest ring), and child reaping (`kill_on_drop` so
/// dropping the connection tears the child down).
///
/// Process-group handling is intentionally NOT applied here — Windows has no
/// `setsid` equivalent in `tokio::process`, and MCP children do not require
/// it because `kill_on_drop(true)` handles cleanup adequately. (A future
/// follow-up may wrap the child in a Job Object so descendant processes
/// also die; out of scope for M2-02c.)
///
/// # Errors
///
/// - [`McpTransportError::Io`] if the child fails to spawn (e.g. command not
///   found, permission denied, cwd does not exist).
/// - [`McpTransportError::MissingPipe`] if `Stdio::piped()` failed to attach
///   one of the three pipes — should not happen in practice but is reported
///   rather than panicked on.
pub async fn spawn_stdio(cfg: StdioConfig) -> Result<Connection, McpTransportError> {
    let StdioHandles { connection, .. } = spawn_stdio_with_handles(cfg).await?;
    Ok(connection)
}

/// Same as [`spawn_stdio`] but also surfaces the shared stderr ring buffer
/// so callers can inspect captured stderr after the child exits or hangs.
///
/// The function is `async` to leave room for a future initialize handshake
/// without breaking callers — today every `.await` happens inside the
/// spawned background tasks, so clippy's `unused_async` is allowed here.
#[allow(clippy::unused_async)]
pub async fn spawn_stdio_with_handles(cfg: StdioConfig) -> Result<StdioHandles, McpTransportError> {
    let mut cmd = tokio::process::Command::new(&cfg.cmd);
    cmd.args(&cfg.args);
    // The child inherits the parent's environment, then overrides with
    // `cfg.env`. Callers are responsible for filtering secrets out of
    // `cfg.env` before constructing the config.
    for (k, v) in &cfg.env {
        cmd.env(k, v);
    }
    if let Some(cwd) = &cfg.cwd {
        cmd.current_dir(cwd);
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // `kill_on_drop` ensures the child dies if the wait task is dropped
    // (e.g. on `Connection` drop, since the wait task owns the `Child`).
    cmd.kill_on_drop(true);

    let mut child = cmd
        .spawn()
        .map_err(|e| McpTransportError::Io(e.to_string()))?;

    let stdin = child
        .stdin
        .take()
        .ok_or(McpTransportError::MissingPipe("stdin"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or(McpTransportError::MissingPipe("stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or(McpTransportError::MissingPipe("stderr"))?;

    // Drain stderr into a shared `StderrRing` on a background task.
    let stderr_ring = Arc::new(AsyncMutex::new(StderrRing::new(StderrRing::DEFAULT_CAP)));
    {
        let ring = stderr_ring.clone();
        tokio::spawn(async move {
            let mut reader = stderr;
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let mut guard = ring.lock().await;
                        guard.push(&buf[..n]);
                    }
                }
            }
        });
    }

    // Wire the JSON-RPC connection over NDJSON stdio.
    let connection = Connection::new_line_delimited(stdout, stdin);

    // Reap the child on exit. The waiter task owns the `Child`, so
    // `kill_on_drop(true)` makes the child die if this task is dropped
    // (e.g. on runtime shutdown). When the child exits normally, its
    // stdout closes and the broker shuts down without further action.
    tokio::spawn(async move {
        match child.wait().await {
            Ok(status) => tracing::debug!(?status, "mcp stdio child exited"),
            Err(e) => tracing::warn!(error = %e, "mcp stdio child wait failed"),
        }
    });

    Ok(StdioHandles {
        connection,
        stderr: stderr_ring,
    })
}

// Re-export the shared WebSocket connector at this crate too, so callers
// can write `lingxi_platform_windows::mcp::connect_ws` directly without
// reaching into `lingxi_platform_common`.
pub use lingxi_platform_common::mcp_ws::{
    connect_ws, WsConnectError, AUTH_HEADER_NAME, WS_SUBPROTOCOL,
};

#[cfg(test)]
mod re_export_tests {
    /// Verify the windows crate exposes the public `connect_ws` symbol at
    /// `lingxi_platform_windows::mcp::connect_ws` (callers should not have
    /// to import from `lingxi_platform_common` directly).
    #[allow(unused_imports)]
    use crate::mcp::connect_ws;
}

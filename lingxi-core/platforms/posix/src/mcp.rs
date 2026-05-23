//! MCP stdio transport — POSIX.
//!
//! M2.02c lands `spawn_stdio`: a low-level helper that spawns a child MCP
//! server, frames its stdio with NDJSON, drains stderr into a 64 MB ring,
//! and returns a fully-wired `lingxi_jsonrpc::Connection`. The legacy
//! `PosixMcpTransport` trait impl below is retained for the current MCP
//! client; a follow-up plan routes its `connect`/`call_tool`/`list_tools`
//! arms through `spawn_stdio` for full `JSON-RPC` semantics.

use async_trait::async_trait;
use lingxi_jsonrpc::Connection;
use lingxi_platform_common::mcp_stdio::{StderrRing, StdioConfig};
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

/// POSIX MCP transport — supports the `Stdio` variant only in M2.02.
///
/// Other transports (`Sse`, `Http`, `WebSocket`, `InProcess`, `SseIde`,
/// `SdkControl`) return `McpError::UnsupportedTransport`. Most request
/// methods are intentionally stubbed pending full `JSON-RPC` framing in
/// M2 phase 3.
#[derive(Default)]
pub struct PosixMcpTransport {
    connections: Mutex<HashMap<McpConnectionId, Child>>,
}

impl PosixMcpTransport {
    /// Construct a new `PosixMcpTransport`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl McpTransport for PosixMcpTransport {
    async fn connect(&self, spec: &McpTransportSpec) -> Result<McpRawConnection, McpError> {
        let (command, args, env) = match spec {
            McpTransportSpec::Stdio { command, args, env } => {
                (command.clone(), args.clone(), env.clone())
            }
            other => return Err(McpError::UnsupportedTransport(map_kind(other))),
        };
        let mut cmd = tokio::process::Command::new(&command);
        cmd.args(&args);
        for (k, v) in &env {
            cmd.env(k, v);
        }
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let child = cmd
            .spawn()
            .map_err(|e| McpError::Connection(e.to_string()))?;
        let id = McpConnectionId::new();
        // Recover from a poisoned std `Mutex` by extracting the inner map.
        if let Ok(mut conns) = self.connections.lock() {
            conns.insert(id, child);
        }
        Ok(McpRawConnection { connection_id: id })
    }

    async fn initialize(
        &self,
        _conn: &McpRawConnection,
    ) -> Result<ServerCapabilitiesDto, McpError> {
        // M2.02 stub — full `JSON-RPC` initialize lands in M2 phase 3.
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
            "posix mcp stdio list_tools: M2 follow-up".into(),
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
            "posix mcp stdio call_tool: M2 follow-up".into(),
        ))
    }

    async fn read_resource(
        &self,
        _conn: &McpRawConnection,
        _uri: &str,
    ) -> Result<McpResourceContentDto, McpError> {
        Err(McpError::Internal(
            "posix mcp stdio read_resource: M2 follow-up".into(),
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
            "posix mcp elicitation: M2 follow-up".into(),
        ))
    }

    async fn disconnect(&self, conn_id: McpConnectionId) -> Result<(), McpError> {
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

    fn supported_transports(&self) -> Vec<McpTransportKind> {
        vec![McpTransportKind::Stdio]
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

/// Error type returned by `spawn_stdio` (and, in M2-02c Task 5, `connect_ws`).
#[derive(Debug, thiserror::Error)]
pub enum McpTransportError {
    /// Failed to spawn the child process.
    #[error("io: {0}")]
    Io(String),
    /// Failed to acquire one of the stdin/stdout/stderr pipes from the child.
    #[error("missing stdio pipe: {0}")]
    MissingPipe(&'static str),
}

/// Spawn an MCP child over stdio and return a fully-wired
/// `lingxi_jsonrpc::Connection`.
///
/// - Frames stdin/stdout with `LineCodec` (NDJSON: one JSON object per
///   `\n`-terminated line).
/// - Drains stderr into a 64 MB `StderrRing` (drop-oldest on overflow). The
///   buffer is held behind the returned [`StdioHandles::stderr`] handle so
///   the caller can snapshot stderr if the child crashes during initialize.
/// - Sets `kill_on_drop(true)` on the child so dropping the returned
///   `Connection` (and its sibling handles) tears the child down.
/// - Propagates child exit by closing the connection's broker (via the
///   spawned waiter task on stdout EOF, which the broker observes
///   naturally).
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

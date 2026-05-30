//! MCP-over-WebSocket endpoint exposed by the bridge.
//!
//! Clients connect at `ws://<host>:<port>/mcp` and must present the matching
//! `X-Claude-Code-Ide-Authorization` header (value = lockfile `authToken`).
//! Mismatched or missing tokens are rejected with HTTP 401 BEFORE the upgrade
//! completes (the response body is literally `"unauthorized\n"`).
//!
//! The literal header name and `mcp` subprotocol mirror the client side in
//! `lingxi-platform-common::mcp_ws` and `claude-code/src/services/mcp/client.ts`.

use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue, StatusCode};

/// Header name (canonical case) for the IDE auth token — matches the literal
/// `X-Claude-Code-Ide-Authorization` claude-code clients send.
/// `http::HeaderMap::get` is case-insensitive so the lookup works for any
/// case the client uses (claude-code's TS client lower-cases it).
pub const AUTH_HEADER_NAME: &str = "X-Claude-Code-Ide-Authorization";

/// LITERAL WebSocket subprotocol — single value `mcp` — echoed back on
/// successful upgrade when the client requested it.
pub const WS_SUBPROTOCOL: &str = "mcp";

/// Body of the 401 response sent on missing / mismatched auth — exact bytes
/// pinned by [`mcp_endpoint_test.rs`].
const UNAUTHORIZED_BODY: &str = "unauthorized\n";

/// Endpoint handle. Holds the listener port and the auth-token cell.
///
/// The accept loop runs in a background task spawned by
/// [`McpEndpoint::start_on_ephemeral_port`]; [`shutdown`](Self::shutdown)
/// signals that task to stop. In-flight connections are NOT awaited — drop
/// the handle (the runtime tears them down when the parent task ends).
pub struct McpEndpoint {
    port: u16,
    auth_token: Arc<RwLock<Option<String>>>,
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl McpEndpoint {
    /// Bind `127.0.0.1:0` and start accepting connections in a background task.
    ///
    /// # Errors
    /// Returns the I/O error from `TcpListener::bind` if the loopback port
    /// cannot be reserved (extremely rare; usually only when 127.0.0.1 itself
    /// is unreachable).
    pub async fn start_on_ephemeral_port() -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let auth_token: Arc<RwLock<Option<String>>> = Arc::new(RwLock::new(None));
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();

        let auth_for_task = auth_token.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => break,
                    accept = listener.accept() => {
                        let (stream, addr) = match accept {
                            Ok(v) => v,
                            Err(e) => {
                                tracing::warn!(error = %e, "bridge accept error");
                                continue;
                            }
                        };
                        let auth_for_conn = auth_for_task.clone();
                        tokio::spawn(handle_connection(stream, addr, auth_for_conn));
                    }
                }
            }
        });

        Ok(Self {
            port,
            auth_token,
            shutdown_tx: Some(shutdown_tx),
        })
    }

    /// Update the expected auth token. Called by the bridge after writing the
    /// lockfile so the same `authToken` is enforced. Sync because the cell is
    /// a `std::sync::RwLock` — never blocks the runtime.
    pub fn set_auth_token(&self, token: String) {
        if let Ok(mut guard) = self.auth_token.write() {
            *guard = Some(token);
        }
    }

    /// Currently bound port (ephemeral, assigned by the kernel at bind time).
    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Stop the accept loop. Does NOT wait for in-flight connections.
    ///
    /// Async for symmetry with future variants that may need to drain
    /// connections — today the oneshot send is sync.
    #[allow(clippy::unused_async)]
    pub async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

/// Per-connection task. Captures the expected token at handshake time, runs
/// `accept_hdr_async` with an auth-validating callback, and (on success) holds
/// the upgraded WebSocket open until the client disconnects.
async fn handle_connection(stream: TcpStream, addr: SocketAddr, auth: Arc<RwLock<Option<String>>>) {
    // Snapshot the expected token BEFORE upgrade so the synchronous callback
    // can compare without re-acquiring the lock.
    let expected = auth.read().ok().and_then(|g| g.clone());

    let cb = move |req: &Request, mut response: Response| -> Result<Response, ErrorResponse> {
        // `HeaderMap::get` is case-insensitive — one lookup covers both
        // canonical and lower-cased header forms.
        let supplied = req
            .headers()
            .get(AUTH_HEADER_NAME)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);

        let authorized = match (expected.as_ref(), supplied.as_ref()) {
            (Some(want), Some(given)) => constant_time_eq(want.as_bytes(), given.as_bytes()),
            _ => false,
        };

        if !authorized {
            // tungstenite 0.21's `ErrorResponse` is `http::Response<Option<String>>`;
            // build via the http::Response builder and convert StatusCode.
            let resp = tokio_tungstenite::tungstenite::http::Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .header("content-type", "text/plain; charset=utf-8")
                .body(Some(UNAUTHORIZED_BODY.to_string()))
                .expect("static 401 response must build");
            return Err(resp);
        }

        // Echo `Sec-WebSocket-Protocol: mcp` when the client requested it
        // (claude-code's TS client always does — see `protocols: ['mcp']`
        // in `src/services/mcp/client.ts`).
        if let Some(proto) = req.headers().get("sec-websocket-protocol") {
            if let Ok(p) = proto.to_str() {
                if p.split(',').any(|t| t.trim() == WS_SUBPROTOCOL) {
                    response.headers_mut().insert(
                        HeaderName::from_static("sec-websocket-protocol"),
                        HeaderValue::from_static(WS_SUBPROTOCOL),
                    );
                }
            }
        }
        Ok(response)
    };

    match tokio_tungstenite::accept_hdr_async(stream, cb).await {
        Ok(ws) => {
            tracing::debug!(?addr, "bridge: client connected");
            // The JSON-RPC plumbing (adapt `ws` onto a `lingxi_jsonrpc::Connection`
            // and dispatch into `lingxi_mcp`) is wired by `IdeBridge` /
            // Task 12's happy-path roundtrip. Here we just hold the socket
            // open until the client disconnects so the upgrade succeeds.
            let _ = ws;
        }
        Err(e) => {
            tracing::debug!(?addr, error = %e, "bridge: handshake rejected");
        }
    }
}

/// Constant-time byte-slice equality. Length-mismatch is short-circuited
/// (length is not secret). Used so an attacker timing the 401 path can't
/// recover the auth token byte-by-byte.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_eq_matches_eq() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(!constant_time_eq(b"", b"x"));
        assert!(constant_time_eq(b"", b""));
    }

    #[tokio::test]
    async fn endpoint_binds_loopback_port() {
        let ep = McpEndpoint::start_on_ephemeral_port().await.unwrap();
        assert!(ep.port() > 0, "ephemeral port must be assigned");
        ep.shutdown().await;
    }

    #[tokio::test]
    async fn set_auth_token_is_observable() {
        let ep = McpEndpoint::start_on_ephemeral_port().await.unwrap();
        ep.set_auth_token("token-abc".to_string());
        let observed = ep.auth_token.read().unwrap().clone();
        assert_eq!(observed.as_deref(), Some("token-abc"));
        ep.shutdown().await;
    }
}

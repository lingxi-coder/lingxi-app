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

use crate::inbound::{ElicitationCreateHandler, RootsListHandler};

/// Async MCP client built on top of a [`lingxi_jsonrpc::Connection`].
///
/// One instance per server connection; owns its `Connection` and inbound
/// handler registrations.
pub struct McpClient {
    /// Logical server name (used in tool full-names and error messages).
    server_name: String,
    /// Absolute cwd advertised to the server via `roots/list`.
    #[allow(dead_code)] // read indirectly via the registered RootsListHandler
    cwd: PathBuf,
    /// Underlying JSON-RPC connection produced by the platform transport.
    #[allow(dead_code)] // wired in Tasks 8-12 (initialize, tools/list, ...)
    connection: Arc<lingxi_jsonrpc::Connection>,
    /// Server capabilities snapshot from the `initialize` response.
    #[allow(dead_code)] // populated in Task 8
    server_capabilities: RwLock<Option<ServerCapabilitiesDto>>,
    /// Server-provided instructions string from the `initialize` response,
    /// truncated to `MAX_MCP_DESCRIPTION_LENGTH` chars on receipt
    /// (matches claude-code `client.ts:1163-1166`).
    #[allow(dead_code)] // populated in Task 8
    server_instructions: RwLock<Option<String>>,
}

impl McpClient {
    /// Build a new client wrapping a JSON-RPC `Connection`.
    ///
    /// Registers two inbound request handlers required by the
    /// `{roots:{}, elicitation:{}}` capability advertisement:
    ///
    /// * `roots/list` -> [`RootsListHandler`] returning `file://<cwd>`.
    /// * `elicitation/create` -> [`ElicitationCreateHandler`] returning
    ///   `{"action":"cancel"}` (default-deny until the host UI replaces it).
    ///
    /// The platform-side caller is responsible for resolving `cwd` to an
    /// absolute path before passing it here (typically via
    /// `std::env::current_dir()`).
    ///
    /// Async because `lingxi_jsonrpc::Connection::register_handler` is async
    /// (the dispatcher map is behind an async `RwLock`). The plan's pseudo-
    /// signature was synchronous; the real M2-02a API requires `.await`.
    pub async fn new(
        server_name: impl Into<String>,
        cwd: PathBuf,
        connection: Arc<lingxi_jsonrpc::Connection>,
    ) -> Self {
        connection
            .register_handler(
                "roots/list",
                Arc::new(RootsListHandler { cwd: cwd.clone() }),
            )
            .await;
        connection
            .register_handler("elicitation/create", Arc::new(ElicitationCreateHandler))
            .await;

        Self {
            server_name: server_name.into(),
            cwd,
            connection,
            server_capabilities: RwLock::new(None),
            server_instructions: RwLock::new(None),
        }
    }

    /// Server name supplied at construction time. Used as the `<server>`
    /// component in the `mcp__<server>__<tool>` tool full-name format.
    #[must_use]
    pub fn server_name(&self) -> &str {
        &self.server_name
    }
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

#[cfg(test)]
mod constructor_tests {
    use super::*;
    use bytes::Bytes;
    use lingxi_jsonrpc::{Connection, Mode};
    use tokio::sync::mpsc;

    /// Build a `Connection` over a fresh pair of `mpsc<Bytes>` channels and
    /// hand back both peer-side ends so the test can drive the dispatcher
    /// without standing up a full mock server.
    #[allow(clippy::type_complexity)]
    fn paired_connection() -> (Arc<Connection>, mpsc::Sender<Bytes>, mpsc::Receiver<Bytes>) {
        let (peer_to_us_tx, peer_to_us_rx) = mpsc::channel::<Bytes>(8);
        let (us_to_peer_tx, us_to_peer_rx) = mpsc::channel::<Bytes>(8);
        let conn = Arc::new(Connection::new_streams(
            peer_to_us_rx,
            us_to_peer_tx,
            Mode::Lines,
        ));
        (conn, peer_to_us_tx, us_to_peer_rx)
    }

    #[tokio::test]
    async fn constructor_compiles_and_stores_server_name() {
        let (conn, _peer_tx, _peer_rx) = paired_connection();
        let client =
            McpClient::new("filesystem", std::path::PathBuf::from("/tmp/work"), conn).await;
        assert_eq!(client.server_name(), "filesystem");
    }

    #[tokio::test]
    async fn constructor_registers_roots_and_elicitation_handlers() {
        // We cannot peek inside the dispatcher map from outside the crate,
        // but we CAN exercise the registration path end-to-end: send a
        // `roots/list` request over the peer side and observe the response.
        let (conn, peer_tx, mut peer_rx) = paired_connection();
        let _client = McpClient::new(
            "filesystem",
            std::path::PathBuf::from("/Users/example/project"),
            conn,
        )
        .await;

        // Inject a `roots/list` request from the peer side.
        let req = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"roots/list\"}\n";
        peer_tx
            .send(Bytes::from_static(req))
            .await
            .expect("send into broker");

        // Expect a response on the outbound channel.
        let frame = tokio::time::timeout(std::time::Duration::from_secs(2), peer_rx.recv())
            .await
            .expect("response within timeout")
            .expect("frame was sent");
        let text = std::str::from_utf8(&frame).expect("utf-8 frame");
        assert!(
            text.contains(r#""uri":"file:///Users/example/project""#),
            "roots/list handler not registered or response shape wrong: {text}",
        );

        // Inject an `elicitation/create` request as well.
        let req2 = b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"elicitation/create\"}\n";
        peer_tx
            .send(Bytes::from_static(req2))
            .await
            .expect("send second request");
        let frame2 = tokio::time::timeout(std::time::Duration::from_secs(2), peer_rx.recv())
            .await
            .expect("response within timeout")
            .expect("frame was sent");
        let text2 = std::str::from_utf8(&frame2).expect("utf-8 frame");
        assert!(
            text2.contains(r#""action":"cancel""#),
            "elicitation/create handler not registered: {text2}",
        );
    }
}

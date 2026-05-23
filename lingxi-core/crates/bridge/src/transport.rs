//! `IdeBridge` — placeholder stub.
//!
//! M2-01 strips the M1-invented JWT / pairing stack. M2-02 §6.2 will rewrite
//! this stub to: (1) discover the most-recent `~/.claude/ide/<port>.lock`
//! via the M2-02-added `crates/bridge/src/lockfile.rs`; (2) parse the lockfile
//! JSON `{workspaceFolders, pid, ideName, transport, runningInWindows,
//! authToken}`; (3) construct
//! `lingxi_mcp::McpTransportSpec::WebSocket { url: format!("ws://localhost:{port}"),
//! headers: HashMap::from([("X-Claude-Code-Ide-Authorization", authToken)]) }`;
//! (4) hand off to `lingxi_mcp::McpRegistry::connect_with_spec`.
//!
//! The auth header is exactly `X-Claude-Code-Ide-Authorization` — NOT
//! `Authorization: Bearer …`. Locked here so M2-02 can't drift.
//!
//! Until then every method returns [`lingxi_traits::BridgeError::Unsupported`].

use crate::message::BridgeMessagePlaceholder;
use crate::state::BridgeState;
use lingxi_traits::BridgeError;
use tokio::sync::RwLock;

/// Engine-side façade for the local IDE bridge. Stub until M2-02.
pub struct IdeBridge {
    state: RwLock<BridgeState>,
}

impl IdeBridge {
    /// Construct an unconnected `IdeBridge` with default state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: RwLock::new(BridgeState::default()),
        }
    }

    /// Read-only snapshot of the bridge's current observable state.
    pub async fn state(&self) -> BridgeState {
        self.state.read().await.clone()
    }

    /// **Always returns `Unsupported`** until M2-02 wires lockfile + WebSocket.
    /// Kept async for parity with the M2-02 signature.
    #[allow(clippy::unused_async)]
    pub async fn connect(&self) -> Result<(), BridgeError> {
        Err(BridgeError::Unsupported)
    }

    /// **Always returns `Unsupported`**. Removed in M2-02 in favor of MCP JSON-RPC.
    /// Kept async for parity with the M2-02 signature.
    #[allow(clippy::unused_async)]
    pub async fn send_placeholder(
        &self,
        _msg: BridgeMessagePlaceholder,
    ) -> Result<(), BridgeError> {
        Err(BridgeError::Unsupported)
    }

    /// **Always returns `Unsupported`**.
    /// Kept async for parity with the M2-02 signature.
    #[allow(clippy::unused_async)]
    pub async fn disconnect(&self) -> Result<(), BridgeError> {
        Err(BridgeError::Unsupported)
    }
}

impl Default for IdeBridge {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn new_constructs_with_default_state() {
        let s = IdeBridge::new().state().await;
        assert!(!s.connected);
        assert!(s.current_file.is_none());
    }

    #[tokio::test]
    async fn connect_returns_unsupported() {
        let err = IdeBridge::new().connect().await.unwrap_err();
        assert!(matches!(err, BridgeError::Unsupported));
    }

    #[tokio::test]
    async fn send_placeholder_returns_unsupported() {
        let err = IdeBridge::new()
            .send_placeholder(BridgeMessagePlaceholder)
            .await
            .unwrap_err();
        assert!(matches!(err, BridgeError::Unsupported));
    }

    #[tokio::test]
    async fn disconnect_returns_unsupported() {
        let err = IdeBridge::new().disconnect().await.unwrap_err();
        assert!(matches!(err, BridgeError::Unsupported));
    }
}

//! Thin façade over [`BridgeTransport`] holding the current connection.
//!
//! The engine instantiates a single [`IdeBridge`] per session, calls
//! [`IdeBridge::connect`] once, and then dispatches [`crate::message::BridgeMessage`]
//! values through [`IdeBridge::send`]. The platform-supplied
//! [`BridgeTransport`] handles framing, auth, and the underlying socket.

use lingxi_traits::{BridgeConfig, BridgeConnection, BridgeError, BridgeTransport};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Engine-side handle to the IDE bridge.
pub struct IdeBridge {
    transport: Arc<dyn BridgeTransport>,
    connection: RwLock<Option<BridgeConnection>>,
}

impl IdeBridge {
    /// Wrap a transport. Does not open a connection — call [`connect`] first.
    ///
    /// [`connect`]: Self::connect
    #[must_use]
    pub fn new(transport: Arc<dyn BridgeTransport>) -> Self {
        Self {
            transport,
            connection: RwLock::new(None),
        }
    }

    /// Open a connection using `config` and remember the handle for later
    /// [`send`] calls.
    ///
    /// [`send`]: Self::send
    pub async fn connect(&self, config: BridgeConfig) -> Result<(), BridgeError> {
        let conn = self.transport.connect(&config).await?;
        *self.connection.write().await = Some(conn);
        Ok(())
    }

    /// Serialize `msg` and forward it through the transport. Returns
    /// [`BridgeError::Closed`] when no connection is open.
    pub async fn send(&self, msg: crate::message::BridgeMessage) -> Result<(), BridgeError> {
        let conn = self
            .connection
            .read()
            .await
            .clone()
            .ok_or(BridgeError::Closed)?;
        self.transport
            .send(
                &conn,
                serde_json::to_value(&msg).expect("serialize bridge message"),
            )
            .await
    }
}

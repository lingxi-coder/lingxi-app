//! Per-connection LSP state machine.
//!
//! Each registered server progresses through these states in order:
//! `Disconnected` → `Starting` → `Initialized` → `Stopped` (with a side
//! transition to `Failed` if anything goes wrong). The registry holds one
//! [`LspConnectionState`] per server name.
//!
//! See spec §25.2 (`LspConnectionState`).

use lingxi_protocol::McpConnectionId;
use lingxi_traits::{LspServerCapabilities, LspServerConfig};
use std::time::SystemTime;

/// State of one LSP server's connection.
#[derive(Debug, Clone)]
pub enum LspConnectionState {
    /// Server is registered but not yet started.
    Disconnected {
        /// Configuration registered for this server.
        config: LspServerConfig,
    },
    /// Server process has been spawned but `initialize` has not yet
    /// completed.
    Starting {
        /// Configuration registered for this server.
        config: LspServerConfig,
        /// When the start was triggered.
        started_at: SystemTime,
        /// OS-level process id.
        pid: u32,
    },
    /// `initialize` succeeded and the server is ready to handle requests.
    Initialized {
        /// Configuration registered for this server.
        config: LspServerConfig,
        /// Stable connection identifier.
        connection_id: McpConnectionId,
        /// Capabilities reported by the server.
        server_capabilities: LspServerCapabilities,
        /// OS-level process id.
        pid: u32,
    },
    /// Start-up or runtime failed; the registry will not route to this
    /// server until it is restarted.
    Failed {
        /// Configuration registered for this server.
        config: LspServerConfig,
        /// Human-readable failure description.
        error: String,
    },
    /// Server was shut down cleanly.
    Stopped {
        /// Configuration registered for this server.
        config: LspServerConfig,
    },
}

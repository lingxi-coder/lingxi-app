//! `LspRegistry` — owns per-server connection state and plugin-only
//! registration.
//!
//! **Registration is plugin-only.** [`LspRegistry::register_plugin_servers`]
//! is the sole public registration path. The internal
//! [`LspRegistry::register_config`] entry point is `pub(crate)` so
//! user/project settings cannot inject LSP servers — matching claude-code's
//! `getAllLspServers()` which only consults `getPluginLspServers()`
//! (`claude-code/src/services/lsp/config.ts:15-79`).
//!
//! See spec §6.3 (Plan M2-03) — "LSP servers from plugins only".
//!
//! M1.18 ships the registry skeleton; production routing wiring lands in
//! Plan 16 alongside the posix-minimal `LspTransport` implementation.
//!
//! See spec §25.3 (`LspRegistry`).

use crate::connection::LspConnectionState;
use lingxi_protocol::{McpConnectionId, PluginId};
use lingxi_traits::{LspError, LspServerConfig, LspTransport};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Per-host LSP registry.
///
/// Holds the configured servers, their current state, and an in-memory
/// cache that maps individual files to the server name responsible for
/// them (so we don't re-walk the project root on every request).
pub struct LspRegistry {
    /// Server name → live state.
    servers: RwLock<HashMap<String, LspConnectionState>>,
    /// File → server-name routing cache (populated by future
    /// `ensure_server_for_file`).
    #[allow(dead_code)] // Plan 16 wires routing.
    file_route_cache: RwLock<HashMap<PathBuf, String>>,
    /// Underlying transport used to start / talk to servers.
    transport: Arc<dyn LspTransport>,
    /// Plugin id → server names contributed by that plugin (used by
    /// `unregister_plugin`).
    #[allow(dead_code)] // Plan 16 wires plugin teardown.
    plugin_servers: RwLock<HashMap<PluginId, Vec<String>>>,
}

impl LspRegistry {
    /// Build an empty registry backed by `transport`.
    #[must_use]
    pub fn new(transport: Arc<dyn LspTransport>) -> Self {
        Self {
            servers: RwLock::new(HashMap::new()),
            file_route_cache: RwLock::new(HashMap::new()),
            transport,
            plugin_servers: RwLock::new(HashMap::new()),
        }
    }

    /// Register a server configuration in the `Disconnected` state.
    ///
    /// Crate-private: callers outside `lingxi-lsp` must use
    /// [`Self::register_plugin_servers`].
    pub(crate) async fn register_config(&self, config: LspServerConfig) {
        self.servers.write().await.insert(
            config.name.clone(),
            LspConnectionState::Disconnected { config },
        );
    }

    /// Return (or start) the server responsible for `path`.
    ///
    /// M1.18 stub: production wiring lands in Plan 16 (posix-minimal
    /// `LspTransport`). Today it always returns
    /// [`LspError::Unavailable`].
    #[allow(clippy::unused_async)] // Plan 16 wires real async dispatch.
    pub async fn ensure_server_for_file(
        &self,
        _path: &std::path::Path,
    ) -> Result<McpConnectionId, LspError> {
        // Touch the transport so the field is considered used until the
        // production routing wiring lands.
        let _ = self.transport.is_available();
        Err(LspError::Unavailable)
    }

    /// Bulk-register server configurations contributed by `plugin_id`.
    ///
    /// This is the **only** public path for registering LSP servers in
    /// `lingxi-core`. User and project settings cannot register LSP servers
    /// — see module documentation.
    pub async fn register_plugin_servers(
        &self,
        plugin_id: PluginId,
        configs: Vec<LspServerConfig>,
    ) {
        let names: Vec<String> = configs.iter().map(|c| c.name.clone()).collect();
        for c in configs {
            self.register_config(c).await;
        }
        self.plugin_servers.write().await.insert(plugin_id, names);
    }

    /// Drop all servers contributed by `plugin_id` and return their names.
    pub async fn unregister_plugin(&self, plugin_id: &PluginId) -> Vec<String> {
        self.plugin_servers
            .write()
            .await
            .remove(plugin_id)
            .unwrap_or_default()
    }
}

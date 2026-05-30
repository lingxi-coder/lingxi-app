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

use crate::client::LspClient;
use crate::connection::LspConnectionState;
use protocol::{McpConnectionId, PluginId};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use traits::{LspError, LspServerConfig, LspTransport};

/// Per-host LSP registry.
///
/// Holds the configured servers, their current state, and an in-memory
/// cache that maps individual files to the server name responsible for
/// them (so we don't re-walk the project root on every request).
pub struct LspRegistry {
    /// Server name → live state.
    servers: RwLock<HashMap<String, LspConnectionState>>,
    /// Side-channel cache of [`LspClient`] handles per server name.
    ///
    /// Populated by [`Self::register_client`] (M4-07) — production wiring
    /// inserts an `Arc<LspClient>` for each `Initialized` server so the
    /// builtin LSP tool (`LSPTool`) can dispatch over the wire-locked
    /// `tool_operations` surface.
    clients: RwLock<HashMap<String, Arc<LspClient>>>,
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
            clients: RwLock::new(HashMap::new()),
            file_route_cache: RwLock::new(HashMap::new()),
            transport,
            plugin_servers: RwLock::new(HashMap::new()),
        }
    }

    /// Cache an `Arc<LspClient>` for `name` (M4-07).
    pub async fn register_client(&self, name: &str, client: Arc<LspClient>) {
        self.clients.write().await.insert(name.into(), client);
    }

    /// Return the cached `Arc<LspClient>` for `name`, if any (M4-07).
    pub async fn get_client(&self, name: &str) -> Option<Arc<LspClient>> {
        self.clients.read().await.get(name).cloned()
    }

    /// Return the [`LspServerConfig`] for `name`, if any (M4-07).
    pub async fn get_config(&self, name: &str) -> Option<LspServerConfig> {
        let servers = self.servers.read().await;
        match servers.get(name)? {
            LspConnectionState::Disconnected { config }
            | LspConnectionState::Starting { config, .. }
            | LspConnectionState::Initialized { config, .. }
            | LspConnectionState::Failed { config, .. }
            | LspConnectionState::Stopped { config } => Some(config.clone()),
        }
    }

    /// Test-only helper: register `config` (Disconnected state) and cache `client`.
    #[doc(hidden)]
    pub async fn register_test_client(
        &self,
        name: &str,
        config: LspServerConfig,
        client: Arc<LspClient>,
    ) {
        self.servers
            .write()
            .await
            .insert(name.into(), LspConnectionState::Disconnected { config });
        self.clients.write().await.insert(name.into(), client);
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

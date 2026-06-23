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
use crate::diagnostic_registry::LspDiagnosticRegistry;
use crate::passive_feedback::PassiveDiagnosticSubscriber;
use protocol::{McpConnectionId, PluginId};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
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
    /// File → server-name routing cache (populated by [`Self::ensure_server_for_file`]).
    file_route_cache: RwLock<HashMap<PathBuf, String>>,
    /// Underlying transport used to start / talk to servers.
    transport: Arc<dyn LspTransport>,
    /// Plugin id → server names contributed by that plugin (used by
    /// `unregister_plugin`).
    plugin_servers: RwLock<HashMap<PluginId, Vec<String>>>,
    /// Diagnostics sink: when wired, [`Self::ensure_server_for_file`] spawns a
    /// [`PassiveDiagnosticSubscriber`] per started server that drains
    /// `publishDiagnostics` into this registry (which the model surfaces via the
    /// `<new-diagnostics>` reminder).
    diagnostics: Option<LspDiagnosticRegistry>,
    /// Live diagnostic subscribers, kept alive (drop aborts the task) keyed by
    /// server name.
    subscribers: RwLock<HashMap<String, PassiveDiagnosticSubscriber>>,
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
            diagnostics: None,
            subscribers: RwLock::new(HashMap::new()),
        }
    }

    /// Wire a diagnostics sink so each started server drains
    /// `publishDiagnostics` into it (see [`Self::ensure_server_for_file`]).
    #[must_use]
    pub fn with_diagnostics(mut self, diagnostics: LspDiagnosticRegistry) -> Self {
        self.diagnostics = Some(diagnostics);
        self
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
    /// Mirrors claude-code `getOrStartServerForFile`: resolve the configured
    /// server whose `extension_to_language` covers the file's extension; if it
    /// is already `Initialized`, return its connection; otherwise spawn it via
    /// the transport, run the `initialize` handshake against the project root,
    /// bridge the transport's connection into a registry-side [`LspClient`] (so
    /// the `LSPTool` can dispatch over it), record the `Initialized` state, and
    /// — when a diagnostics sink is wired — start a [`PassiveDiagnosticSubscriber`]
    /// that drains `publishDiagnostics` into it.
    ///
    /// # Errors
    /// [`LspError::Unavailable`] when no configured server handles the file;
    /// [`LspError::Transport`] / [`LspError::ServerError`] on spawn/handshake
    /// failure.
    pub async fn ensure_server_for_file(
        &self,
        path: &Path,
    ) -> Result<McpConnectionId, LspError> {
        let ext = file_extension(path);

        // Resolve the responsible server (already-initialized wins; else the
        // first config whose extension map covers this file).
        let chosen = {
            let servers = self.servers.read().await;
            let mut pick: Option<(String, LspServerConfig)> = None;
            for (name, state) in servers.iter() {
                if !config_handles_ext(state.config(), ext.as_deref()) {
                    continue;
                }
                if let LspConnectionState::Initialized { connection_id, .. } = state {
                    return Ok(*connection_id);
                }
                pick = Some((name.clone(), state.config().clone()));
                break;
            }
            pick
        };
        let Some((name, config)) = chosen else {
            return Err(LspError::Unavailable);
        };

        // Spawn + initialize.
        let raw = self.transport.start_server(&config).await?;
        let root_uri = project_root_uri(path, &config.root_dir_markers);
        let caps = self.transport.initialize(&raw, &root_uri).await?;

        // Bridge the transport's live connection into a registry-side client
        // over the SAME shared connection, so the LSP tool can dispatch.
        let connection = self.transport.connection(raw.connection_id).await?;
        let client = Arc::new(LspClient::with_shared(name.clone(), connection.clone()));
        self.clients.write().await.insert(name.clone(), client);

        // Start the passive diagnostics subscriber (kept alive in `subscribers`).
        if let Some(diag) = &self.diagnostics {
            let sub = PassiveDiagnosticSubscriber::spawn(&connection, name.clone(), diag.clone());
            self.subscribers.write().await.insert(name.clone(), sub);
        }

        self.servers.write().await.insert(
            name.clone(),
            LspConnectionState::Initialized {
                config,
                connection_id: raw.connection_id,
                server_capabilities: caps,
                pid: 0,
            },
        );
        self.file_route_cache
            .write()
            .await
            .insert(path.to_path_buf(), name);
        Ok(raw.connection_id)
    }

    /// Ensure the server for `path` is running and return its `(name, client,
    /// config)` — the convenience the `LSPTool` uses to resolve a server from
    /// the file alone (claude-code dispatches by `filePath`, never a model-
    /// supplied server name).
    ///
    /// # Errors
    /// [`LspError::Unavailable`] when no server handles the file (or its
    /// client/config vanished); transport errors propagate from
    /// [`Self::ensure_server_for_file`].
    pub async fn ensure_client_for_file(
        &self,
        path: &Path,
    ) -> Result<(String, Arc<LspClient>, LspServerConfig), LspError> {
        self.ensure_server_for_file(path).await?;
        let name = self
            .file_route_cache
            .read()
            .await
            .get(path)
            .cloned()
            .ok_or(LspError::Unavailable)?;
        let client = self.get_client(&name).await.ok_or(LspError::Unavailable)?;
        let config = self.get_config(&name).await.ok_or(LspError::Unavailable)?;
        Ok((name, client, config))
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
    ///
    /// Removes each contributed server from the live state map (and any cached
    /// client), symmetric with [`Self::register_plugin_servers`], so a disabled
    /// plugin leaves no orphaned LSP server behind.
    pub async fn unregister_plugin(&self, plugin_id: &PluginId) -> Vec<String> {
        let names = self
            .plugin_servers
            .write()
            .await
            .remove(plugin_id)
            .unwrap_or_default();
        if !names.is_empty() {
            let mut servers = self.servers.write().await;
            let mut clients = self.clients.write().await;
            let mut subs = self.subscribers.write().await;
            for n in &names {
                servers.remove(n);
                clients.remove(n);
                subs.remove(n); // drop aborts the subscriber task
            }
        }
        names
    }
}

#[cfg(test)]
mod routing_tests {
    use super::*;
    use async_trait::async_trait;
    use jsonrpc::Connection;
    use serde_json::Value;
    use traits::{LspRawConnection, LspServerCapabilities};

    fn caps() -> LspServerCapabilities {
        LspServerCapabilities {
            text_document_sync: None,
            completion: false,
            hover: true,
            definition: true,
            references: true,
            diagnostics: true,
            symbols: true,
            formatting: false,
            rename: false,
            code_action: false,
        }
    }

    /// Mock transport: returns a fixed connection over an in-memory pipe.
    struct MockTransport {
        conn: Arc<Connection>,
        id: McpConnectionId,
    }
    impl MockTransport {
        fn new() -> Self {
            let (a, _b) = tokio::io::duplex(256);
            let (r, w) = tokio::io::split(a);
            Self {
                conn: Arc::new(Connection::new_line_delimited(r, w)),
                id: McpConnectionId::new(),
            }
        }
    }
    #[async_trait]
    impl LspTransport for MockTransport {
        async fn start_server(
            &self,
            _config: &LspServerConfig,
        ) -> Result<LspRawConnection, LspError> {
            Ok(LspRawConnection { connection_id: self.id })
        }
        async fn initialize(
            &self,
            _conn: &LspRawConnection,
            _root_uri: &str,
        ) -> Result<LspServerCapabilities, LspError> {
            Ok(caps())
        }
        async fn connection(
            &self,
            _conn_id: McpConnectionId,
        ) -> Result<Arc<Connection>, LspError> {
            Ok(self.conn.clone())
        }
        async fn request(
            &self,
            _c: &LspRawConnection,
            _m: &str,
            _p: Value,
        ) -> Result<Value, LspError> {
            Ok(Value::Null)
        }
        async fn notify(
            &self,
            _c: &LspRawConnection,
            _m: &str,
            _p: Value,
        ) -> Result<(), LspError> {
            Ok(())
        }
        async fn shutdown(&self, _id: McpConnectionId) -> Result<(), LspError> {
            Ok(())
        }
        fn is_available(&self) -> bool {
            true
        }
    }

    fn rust_config() -> LspServerConfig {
        LspServerConfig {
            name: "rust-analyzer".into(),
            command: "rust-analyzer".into(),
            args: vec![],
            env: HashMap::new(),
            trigger_languages: vec!["rust".into()],
            root_dir_markers: vec!["Cargo.toml".into()],
            initialization_options: None,
            extension_to_language: HashMap::from([(".rs".to_string(), "rust".to_string())]),
        }
    }

    #[tokio::test]
    async fn ensure_server_routes_starts_caches_and_is_idempotent() {
        let reg = LspRegistry::new(Arc::new(MockTransport::new()));
        reg.register_config(rust_config()).await;

        // A non-matching extension → no server handles it.
        assert!(matches!(
            reg.ensure_server_for_file(Path::new("/p/main.py")).await,
            Err(LspError::Unavailable)
        ));

        // A .rs file routes to rust-analyzer: starts it, caches a client, and
        // records the Initialized state.
        let id = reg
            .ensure_server_for_file(Path::new("/p/src/main.rs"))
            .await
            .expect("starts rust-analyzer");
        assert!(reg.get_client("rust-analyzer").await.is_some());
        assert!(matches!(
            reg.servers.read().await.get("rust-analyzer"),
            Some(LspConnectionState::Initialized { .. })
        ));

        // A second call for another .rs file reuses the running server.
        let id2 = reg
            .ensure_server_for_file(Path::new("/p/src/lib.rs"))
            .await
            .expect("reuses running server");
        assert_eq!(id, id2, "already-initialized server is reused");
    }
}

/// The file's lowercased extension WITH a leading dot (e.g. `.rs`), matching
/// the keys of [`LspServerConfig::extension_to_language`]. `None` for an
/// extensionless file.
fn file_extension(path: &Path) -> Option<String> {
    path.extension()
        .map(|e| format!(".{}", e.to_string_lossy().to_ascii_lowercase()))
}

/// Whether `config` handles a file with extension `ext` (case-folded match
/// against its `extension_to_language` keys).
fn config_handles_ext(config: &LspServerConfig, ext: Option<&str>) -> bool {
    let Some(ext) = ext else {
        return false;
    };
    config
        .extension_to_language
        .keys()
        .any(|k| k.to_ascii_lowercase() == ext)
}

/// The `file://` URI of the project root for `path`: the nearest ancestor
/// directory containing any of `markers` (e.g. `Cargo.toml`), or the file's own
/// parent directory when no marker is found.
fn project_root_uri(path: &Path, markers: &[String]) -> String {
    let start = path.parent().unwrap_or(path);
    let mut dir = Some(start);
    let mut root = start;
    while let Some(d) = dir {
        if markers.iter().any(|m| d.join(m).exists()) {
            root = d;
            break;
        }
        dir = d.parent();
    }
    format!("file://{}", root.to_string_lossy())
}

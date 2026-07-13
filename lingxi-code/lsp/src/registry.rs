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
    /// Extension (lowercased, with leading dot) → server names in
    /// REGISTRATION order.
    ///
    /// Mirrors claude-code's LSP server manager, whose routing table is an
    /// extension → ordered array of server names built in config-registration
    /// order: `getServerForFile` always resolves the FIRST entry, so a
    /// later-registered same-extension server is shadowed and never used
    /// (it gets a registration-time warning instead).
    ext_routes: RwLock<HashMap<String, Vec<String>>>,
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
            ext_routes: RwLock::new(HashMap::new()),
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
        self.record_routes(&config).await;
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
        self.record_routes(&config).await;
        self.servers.write().await.insert(
            config.name.clone(),
            LspConnectionState::Disconnected { config },
        );
    }

    /// Record `config`'s extensions in the registration-order routing table.
    ///
    /// First-registered wins: when another server already handles an
    /// extension, the newcomer is shadowed and — matching claude-code's
    /// registration-time console warning byte-for-byte — we warn
    /// `LSP: extension {ext} already handled by "{first}"; "{name}" will not
    /// be used for {ext} files`.
    async fn record_routes(&self, config: &LspServerConfig) {
        let mut routes = self.ext_routes.write().await;
        for key in config.extension_to_language.keys() {
            let ext = key.to_ascii_lowercase();
            let names = routes.entry(ext.clone()).or_default();
            if names.iter().any(|n| n == &config.name) {
                continue; // re-registration of the same server
            }
            if let Some(first) = names.first() {
                let name = &config.name;
                tracing::warn!(
                    target: "lingxi_lsp::registry",
                    "LSP: extension {ext} already handled by \"{first}\"; \"{name}\" will not be used for {ext} files"
                );
            }
            names.push(config.name.clone());
        }
    }

    /// Return (or start) the server responsible for `path`.
    ///
    /// Mirrors claude-code `getOrStartServerForFile`: resolve the
    /// FIRST-registered server whose `extension_to_language` covers the file's
    /// extension (claude-code's `getServerForFile` always takes the first
    /// entry of its extension → ordered-server-array table; shadowed
    /// same-extension servers are never used); if it is already
    /// `Initialized`, return its connection; otherwise spawn it via
    /// the transport, run the `initialize` handshake against the project root,
    /// bridge the transport's connection into a registry-side [`LspClient`] (so
    /// the `LSPTool` can dispatch over it), record the `Initialized` state, and
    /// — when a diagnostics sink is wired — start a [`PassiveDiagnosticSubscriber`]
    /// that drains `publishDiagnostics` into it.
    ///
    /// # Errors
    /// [`LspError::Unavailable`] when no configured server handles the file;
    /// [`LspError::Transport`] / [`LspError::ServerError`] on spawn/handshake
    /// failure (like claude-code's `ensureServerStarted`, a start failure is
    /// NOT retried against a shadowed same-extension server).
    pub async fn ensure_server_for_file(&self, path: &Path) -> Result<McpConnectionId, LspError> {
        // Resolve the responsible server deterministically: the
        // first-registered server handling this extension.
        let name = {
            let Some(ext) = file_extension(path) else {
                return Err(LspError::Unavailable);
            };
            self.ext_routes
                .read()
                .await
                .get(&ext)
                .and_then(|names| names.first().cloned())
                .ok_or(LspError::Unavailable)?
        };

        // Already initialized → reuse it, recording the route for THIS path
        // so `ensure_client_for_file` resolves every file of the extension,
        // not just the one that first spawned the server.
        let config = {
            let servers = self.servers.read().await;
            let state = servers.get(&name).ok_or(LspError::Unavailable)?;
            if let LspConnectionState::Initialized { connection_id, .. } = state {
                let connection_id = *connection_id;
                drop(servers);
                self.file_route_cache
                    .write()
                    .await
                    .insert(path.to_path_buf(), name);
                return Ok(connection_id);
            }
            state.config().clone()
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
    /// plugin leaves no orphaned LSP server behind. Extension routes and
    /// file-route cache entries pointing at the removed servers are purged so
    /// a stale route can never resolve to a vanished client.
    pub async fn unregister_plugin(&self, plugin_id: &PluginId) -> Vec<String> {
        let names = self
            .plugin_servers
            .write()
            .await
            .remove(plugin_id)
            .unwrap_or_default();
        if !names.is_empty() {
            {
                let mut servers = self.servers.write().await;
                let mut clients = self.clients.write().await;
                let mut subs = self.subscribers.write().await;
                for n in &names {
                    servers.remove(n);
                    clients.remove(n);
                    subs.remove(n); // drop aborts the subscriber task
                }
            }
            {
                let mut routes = self.ext_routes.write().await;
                for list in routes.values_mut() {
                    list.retain(|n| !names.contains(n));
                }
                routes.retain(|_, list| !list.is_empty());
            }
            self.file_route_cache
                .write()
                .await
                .retain(|_, n| !names.contains(n));
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

    /// Mock transport: returns a fixed connection over an in-memory pipe and
    /// records the name of every server config it is asked to start.
    struct MockTransport {
        conn: Arc<Connection>,
        id: McpConnectionId,
        started: std::sync::Mutex<Vec<String>>,
    }
    impl MockTransport {
        fn new() -> Self {
            let (a, _b) = tokio::io::duplex(256);
            let (r, w) = tokio::io::split(a);
            Self {
                conn: Arc::new(Connection::new_line_delimited(r, w)),
                id: McpConnectionId::new(),
                started: std::sync::Mutex::new(Vec::new()),
            }
        }
    }
    #[async_trait]
    impl LspTransport for MockTransport {
        async fn start_server(
            &self,
            config: &LspServerConfig,
        ) -> Result<LspRawConnection, LspError> {
            self.started.lock().unwrap().push(config.name.clone());
            Ok(LspRawConnection {
                connection_id: self.id,
            })
        }
        async fn initialize(
            &self,
            _conn: &LspRawConnection,
            _root_uri: &str,
        ) -> Result<LspServerCapabilities, LspError> {
            Ok(caps())
        }
        async fn connection(&self, _conn_id: McpConnectionId) -> Result<Arc<Connection>, LspError> {
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
        async fn notify(&self, _c: &LspRawConnection, _m: &str, _p: Value) -> Result<(), LspError> {
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
        named_rust_config("rust-analyzer")
    }

    fn named_rust_config(name: &str) -> LspServerConfig {
        LspServerConfig {
            name: name.into(),
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

    /// Regression (2.1.207 P1-09): the already-initialized hit path skipped
    /// the file-route-cache write, so a SECOND file of the same extension
    /// failed `ensure_client_for_file` with `Unavailable` while the server
    /// was healthy (claude-code recomputes routing per call, so every file of
    /// a handled extension resolves).
    #[tokio::test]
    async fn ensure_client_resolves_every_file_of_the_extension() {
        let reg = LspRegistry::new(Arc::new(MockTransport::new()));
        reg.register_config(rust_config()).await;

        let (name_a, _, _) = reg
            .ensure_client_for_file(Path::new("/p/a.rs"))
            .await
            .expect("first file resolves");
        let (name_b, _, _) = reg
            .ensure_client_for_file(Path::new("/p/b.rs"))
            .await
            .expect("second file of the same extension resolves too");
        assert_eq!(name_a, "rust-analyzer");
        assert_eq!(name_a, name_b, "both files route to the same server");
    }

    /// Parity (2.1.207 P1-09): claude-code's routing table is extension →
    /// ordered array in registration order and `getServerForFile` always
    /// takes the first entry — the pick must be deterministic
    /// (first-registered wins) and the shadowed server must never start.
    #[tokio::test]
    async fn first_registered_server_wins_and_shadowed_never_starts() {
        let transport = Arc::new(MockTransport::new());
        let reg = LspRegistry::new(transport.clone());
        // Names chosen so any accidental alphabetical/hash ordering loses.
        reg.register_config(named_rust_config("zzz-first")).await;
        reg.register_config(named_rust_config("aaa-second")).await;

        for file in ["/p/a.rs", "/p/b.rs"] {
            let (name, _, _) = reg
                .ensure_client_for_file(Path::new(file))
                .await
                .expect("routes to the first-registered server");
            assert_eq!(name, "zzz-first", "first-registered server wins");
        }
        assert_eq!(
            *transport.started.lock().unwrap(),
            vec!["zzz-first".to_string()],
            "shadowed same-extension server is never started"
        );
    }

    /// After `unregister_plugin`, routing entries pointing at the removed
    /// servers are purged: no stale route may resolve to a vanished client.
    #[tokio::test]
    async fn unregister_plugin_purges_routes() {
        let reg = LspRegistry::new(Arc::new(MockTransport::new()));
        let plugin = PluginId::new();
        reg.register_plugin_servers(plugin, vec![rust_config()])
            .await;
        reg.ensure_client_for_file(Path::new("/p/a.rs"))
            .await
            .expect("resolves while registered");

        let removed = reg.unregister_plugin(&plugin).await;
        assert_eq!(removed, vec!["rust-analyzer".to_string()]);
        assert!(matches!(
            reg.ensure_client_for_file(Path::new("/p/a.rs")).await,
            Err(LspError::Unavailable)
        ));
        assert!(reg.file_route_cache.read().await.is_empty());
        assert!(reg.ext_routes.read().await.is_empty());
    }
}

/// The file's lowercased extension WITH a leading dot (e.g. `.rs`), matching
/// the keys of [`LspServerConfig::extension_to_language`]. `None` for an
/// extensionless file.
fn file_extension(path: &Path) -> Option<String> {
    path.extension()
        .map(|e| format!(".{}", e.to_string_lossy().to_ascii_lowercase()))
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

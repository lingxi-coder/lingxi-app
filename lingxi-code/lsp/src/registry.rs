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
use crate::open_file_tracker::OpenFileTracker;
use crate::passive_feedback::PassiveDiagnosticSubscriber;
use protocol::{McpConnectionId, PluginId};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::{watch, RwLock};
use traits::{LspError, LspServerConfig, LspTransport};

/// Crash-recovery cap: a server whose start keeps failing is retried until
/// its failure count EXCEEDS this bound, then every further request returns
/// the recorded error without another spawn. Mirrors claude-code's
/// `t.maxRestarts ?? 3`; an explicit `maxRestarts` overrides this default.
const DEFAULT_MAX_RESTARTS: u32 = 3;

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
    /// Manager-wide open-document/LRU/version state. The LSP tool must reuse
    /// this across calls; a per-call tracker would resend `didOpen` forever.
    open_files: OpenFileTracker,
    /// Synchronous tool-availability bit: true once at least one plugin LSP
    /// config is registered.
    has_registered_servers: AtomicBool,
    /// In-flight start claims, keyed by server name (2.1.207 P2-09).
    ///
    /// claude-code's per-server start fn early-returns while the state is
    /// `running` or `starting` — on its single-threaded event loop the
    /// `starting` state alone is the double-start guard. Our spawn is awaited
    /// across task interleavings, so concurrent callers that observe
    /// [`LspConnectionState::Starting`] subscribe to the claim's watch sender
    /// and re-read the state once it drops (see [`StartClaimGuard`]) instead
    /// of spawning a second child.
    ///
    /// `std::sync::Mutex` on purpose: every access is a short synchronous
    /// section (never held across `await`), and it lets [`StartClaimGuard`]
    /// release the claim in `Drop` even when the starting task is cancelled
    /// mid-flight.
    starting: Arc<std::sync::Mutex<HashMap<String, watch::Sender<()>>>>,
}

/// Releases an in-flight start claim on drop.
///
/// Removing (and thereby dropping) the `watch::Sender` wakes every waiter
/// subscribed to it (`Receiver::changed` resolves once the sender is gone).
/// When cancellation happens after spawn, the guard keeps the claim registered
/// until `transport.terminate(...)` completes, so a waiter cannot start a
/// replacement while the abandoned child is still alive.
struct StartClaimGuard {
    starting: Arc<std::sync::Mutex<HashMap<String, watch::Sender<()>>>>,
    name: String,
    transport: Arc<dyn LspTransport>,
    owned_connection: Option<McpConnectionId>,
}

impl StartClaimGuard {
    fn own_connection(&mut self, connection_id: McpConnectionId) {
        self.owned_connection = Some(connection_id);
    }

    fn owned_connection(&self) -> Option<McpConnectionId> {
        self.owned_connection
    }

    fn release_connection(&mut self) {
        self.owned_connection = None;
    }
}

impl Drop for StartClaimGuard {
    fn drop(&mut self) {
        let starting = Arc::clone(&self.starting);
        let name = self.name.clone();
        let Some(connection_id) = self.owned_connection.take() else {
            starting
                .lock()
                .expect("lsp start-claim lock poisoned")
                .remove(&name);
            return;
        };
        let transport = Arc::clone(&self.transport);
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                if let Err(cleanup_error) = transport.terminate(connection_id).await {
                    tracing::warn!(
                        target: "lingxi_lsp::registry",
                        %cleanup_error,
                        "failed to terminate LSP server after cancelled startup"
                    );
                }
                starting
                    .lock()
                    .expect("lsp start-claim lock poisoned")
                    .remove(&name);
            });
        } else {
            starting
                .lock()
                .expect("lsp start-claim lock poisoned")
                .remove(&name);
        }
    }
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
            open_files: OpenFileTracker::new(),
            has_registered_servers: AtomicBool::new(false),
            starting: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }

    /// Wire a diagnostics sink so each started server drains
    /// `publishDiagnostics` into it (see [`Self::ensure_server_for_file`]).
    #[must_use]
    pub fn with_diagnostics(mut self, diagnostics: LspDiagnosticRegistry) -> Self {
        self.diagnostics = Some(diagnostics);
        self
    }

    /// Shared document tracker used by every LSP tool call.
    #[must_use]
    pub fn open_file_tracker(&self) -> OpenFileTracker {
        self.open_files.clone()
    }

    /// Whether the manager has any plugin-provided server configuration.
    #[must_use]
    pub fn has_registered_servers(&self) -> bool {
        self.has_registered_servers.load(Ordering::Acquire)
    }

    /// Whether the LSP tool should be exposed on this host.
    ///
    /// Claude Code keeps the tool inactive both without plugin configuration
    /// and on surfaces that cannot launch plugin language servers.
    #[must_use]
    pub fn is_tool_available(&self) -> bool {
        self.has_registered_servers() && self.transport.is_available()
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
        self.open_files
            .activate_server(name, client.connection())
            .await;
        self.clients.write().await.insert(name.into(), client);
        self.has_registered_servers.store(true, Ordering::Release);
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
        self.has_registered_servers.store(true, Ordering::Release);
    }

    /// Record `config`'s extensions in the registration-order routing table.
    ///
    /// First-registered is primary: when another server already handles an
    /// extension, the newcomer is a fallback and — matching claude-code's
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
    /// Mirrors claude-code `getOrStartServerForFile`: resolve the ordered
    /// servers whose `extension_to_language` covers the file's extension,
    /// trying the first registered server first and falling back to later
    /// same-extension servers when start/initialize fails; if a candidate is
    /// already `Initialized`, return its connection; otherwise spawn it via
    /// the transport, run the `initialize` handshake against the project root,
    /// bridge the transport's connection into a registry-side [`LspClient`] (so
    /// the `LSPTool` can dispatch over it), record the `Initialized` state, and
    /// — when a diagnostics sink is wired — start a [`PassiveDiagnosticSubscriber`]
    /// that drains `publishDiagnostics` into it.
    ///
    /// Start attempts are SERIALIZED per server (2.1.207 P2-09): the caller
    /// that transitions the state to `Starting` under the write lock owns the
    /// spawn; concurrent callers observing `Starting` await that attempt and
    /// re-read the outcome (claude-code's start fn early-returns
    /// `if (state === "running" || state === "starting")`). A start failure
    /// records `Failed` and is retried on a later request until the failure
    /// count exceeds `maxRestarts ?? 3` — from then on the byte-exact
    /// claude-code error `LSP server '{name}' exceeded max crash recovery
    /// attempts (3)` is returned without another spawn.
    ///
    /// # Errors
    /// [`LspError::Unavailable`] when no configured server handles the file;
    /// [`LspError::Transport`] / [`LspError::ServerError`] when every candidate
    /// fails.
    #[allow(clippy::too_many_lines)] // candidate loop + lifecycle helper
    pub async fn ensure_server_for_file(&self, path: &Path) -> Result<McpConnectionId, LspError> {
        let workspace_cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        self.ensure_server_for_file_in_workspace(path, &workspace_cwd)
            .await
    }

    /// Return (or start) the server for `path`, resolving its initialization
    /// workspace against the caller's current session directory.
    ///
    /// Claude Code's LSP manager reads the async-local session cwd at startup;
    /// callers with a live cwd must use this entry point instead of relying on
    /// the host process cwd.
    #[allow(clippy::too_many_lines)] // candidate loop + lifecycle helper
    pub async fn ensure_server_for_file_in_workspace(
        &self,
        path: &Path,
        workspace_cwd: &Path,
    ) -> Result<McpConnectionId, LspError> {
        let names = {
            let Some(ext) = file_extension(path) else {
                return Err(LspError::Unavailable);
            };
            self.ext_routes
                .read()
                .await
                .get(&ext)
                .cloned()
                .ok_or(LspError::Unavailable)?
        };
        let mut last_err = None;
        for name in names {
            match self
                .ensure_named_server_for_file(path, name, workspace_cwd)
                .await
            {
                Ok(connection_id) => return Ok(connection_id),
                Err(err) => {
                    last_err = Some(err);
                }
            }
        }
        Err(last_err.unwrap_or(LspError::Unavailable))
    }

    #[allow(clippy::too_many_lines)] // linear lifecycle: claim → spawn → publish
    async fn ensure_named_server_for_file(
        &self,
        path: &Path,
        name: String,
        workspace_cwd: &Path,
    ) -> Result<McpConnectionId, LspError> {
        enum Claim {
            /// Already initialized → reuse its connection.
            Reuse(McpConnectionId),
            /// We own the start attempt: spawn with `(config, prior_failures)`.
            Start(Box<LspServerConfig>, u32),
            /// Another task owns an in-flight attempt: await it, re-read.
            Wait(watch::Receiver<()>),
            /// Terminal for this request (recorded failure / crash-recovery cap).
            Fail(LspError),
        }

        self.record_dead_connection(&name).await;

        // Whether we already awaited an in-flight attempt: a waiter that then
        // observes `Failed` returns the recorded error instead of claiming an
        // immediate retry (the retry belongs to a LATER request, matching
        // claude-code where the concurrent caller's ensure sees the thrown
        // start error).
        let mut waited = false;
        let (config, prior_failures) = loop {
            let claim = {
                let mut servers = self.servers.write().await;
                let Some(state) = servers.get(&name) else {
                    return Err(LspError::Unavailable);
                };
                match state {
                    LspConnectionState::Initialized { connection_id, .. } => {
                        Claim::Reuse(*connection_id)
                    }
                    LspConnectionState::Starting { .. } => {
                        let rx = self
                            .starting
                            .lock()
                            .expect("lsp start-claim lock poisoned")
                            .get(&name)
                            .map(watch::Sender::subscribe);
                        if let Some(rx) = rx {
                            Claim::Wait(rx)
                        } else {
                            // Stale `Starting` — the previous starting task
                            // was cancelled mid-flight. Reclaim it.
                            let config = state.config().clone();
                            let restarts = match state {
                                LspConnectionState::Starting { restarts, .. } => *restarts,
                                _ => 0,
                            };
                            self.claim_start(&mut servers, &name, config.clone(), restarts);
                            Claim::Start(Box::new(config), restarts)
                        }
                    }
                    LspConnectionState::Disconnected { config }
                    | LspConnectionState::Stopped { config } => {
                        let config = config.clone();
                        self.claim_start(&mut servers, &name, config.clone(), 0);
                        Claim::Start(Box::new(config), 0)
                    }
                    LspConnectionState::Failed {
                        config,
                        error,
                        restarts,
                        max_recovery_reported,
                    } => {
                        if waited || (config.restart_on_crash == Some(false) && *restarts > 0) {
                            Claim::Fail(LspError::ServerError(error.clone()))
                        } else if *restarts > config.max_restarts.unwrap_or(DEFAULT_MAX_RESTARTS) {
                            // claude-code: `if (state === "error" && restartCount
                            // > maxRestarts)` — report ONCE (error log +
                            // tengu_feature_bad lsp_server_start /
                            // lsp_server_max_crash_recovery), record the cap
                            // error as lastError, rethrow it thereafter.
                            if *max_recovery_reported {
                                Claim::Fail(LspError::ServerError(error.clone()))
                            } else {
                                let max_restarts =
                                    config.max_restarts.unwrap_or(DEFAULT_MAX_RESTARTS);
                                let (config, restarts) = (config.clone(), *restarts);
                                let msg = format!(
                                    "LSP server '{name}' exceeded max crash recovery attempts ({max_restarts})"
                                );
                                tracing::error!(target: "lingxi_lsp::registry", "{msg}");
                                servers.insert(
                                    name.clone(),
                                    LspConnectionState::Failed {
                                        config,
                                        error: msg.clone(),
                                        restarts,
                                        max_recovery_reported: true,
                                    },
                                );
                                Claim::Fail(LspError::ServerError(msg))
                            }
                        } else {
                            let (config, restarts) = (config.clone(), *restarts);
                            self.claim_start(&mut servers, &name, config.clone(), restarts);
                            Claim::Start(Box::new(config), restarts)
                        }
                    }
                }
            };
            match claim {
                Claim::Reuse(connection_id) => {
                    // Record the route for THIS path so `ensure_client_for_file`
                    // resolves every file of the extension, not just the one
                    // that first spawned the server.
                    self.file_route_cache
                        .write()
                        .await
                        .insert(path.to_path_buf(), name);
                    return Ok(connection_id);
                }
                Claim::Fail(e) => return Err(e),
                Claim::Wait(mut rx) => {
                    // Resolves when the owning attempt drops its claim
                    // (success, failure, or cancellation) — then re-read.
                    let _ = rx.changed().await;
                    waited = true;
                }
                Claim::Start(config, prior_failures) => break (*config, prior_failures),
            }
        };

        // We own the exclusive start claim: spawn + initialize with no lock
        // held. The guard wakes waiters (and releases the claim) even if this
        // task is cancelled mid-flight, so a stale `Starting` is reclaimable
        // and an already-spawned child is not leaked.
        let mut claim_guard = StartClaimGuard {
            starting: Arc::clone(&self.starting),
            name: name.clone(),
            transport: Arc::clone(&self.transport),
            owned_connection: None,
        };
        tracing::debug!(target: "lingxi_lsp::registry", "Starting LSP server instance: {name}");
        let startup_config = startup_config_for_workspace(&config, workspace_cwd);
        let started = async {
            let raw = self.transport.start_server(&startup_config).await?;
            claim_guard.own_connection(raw.connection_id);
            let root_uri = workspace_root_uri(&startup_config, workspace_cwd);
            let caps = self.transport.initialize(&raw, &root_uri).await?;
            // Bridge the transport's live connection into a registry-side
            // client over the SAME shared connection, so the LSP tool can
            // dispatch.
            let connection = self.transport.connection(raw.connection_id).await?;
            Ok::<_, LspError>((raw, caps, connection))
        }
        .await;

        match started {
            Ok((raw, caps, connection)) => {
                // Publish state + client + subscriber together under the
                // servers write lock; if `unregister_plugin` raced the start
                // and already purged this server, do NOT resurrect it — stop
                // the fresh child instead of leaking it.
                let mut servers = self.servers.write().await;
                if !servers.contains_key(&name) {
                    drop(servers);
                    if let Some(connection_id) = claim_guard.owned_connection() {
                        match self.transport.shutdown(connection_id).await {
                            Ok(()) => claim_guard.release_connection(),
                            Err(e) => tracing::error!(
                                target: "lingxi_lsp::registry",
                                "Failed to stop LSP server '{name}': {e}"
                            ),
                        }
                    }
                    drop(claim_guard);
                    return Err(LspError::Unavailable);
                }
                let client = Arc::new(LspClient::with_shared(name.clone(), connection.clone()));
                self.open_files
                    .activate_server(&name, connection.clone())
                    .await;
                self.clients.write().await.insert(name.clone(), client);
                // Start the passive diagnostics subscriber (kept alive in
                // `subscribers`).
                if config.diagnostics.unwrap_or(true) {
                    if let Some(diag) = &self.diagnostics {
                        let sub = PassiveDiagnosticSubscriber::spawn(
                            &connection,
                            name.clone(),
                            diag.clone(),
                        );
                        self.subscribers.write().await.insert(name.clone(), sub);
                    }
                }
                servers.insert(
                    name.clone(),
                    LspConnectionState::Initialized {
                        config,
                        connection_id: raw.connection_id,
                        server_capabilities: caps,
                        pid: 0,
                        restarts: 0,
                    },
                );
                drop(servers);
                claim_guard.release_connection();
                // Release the claim only AFTER the resolved state is visible,
                // so woken waiters observe `Initialized`, never a stale
                // `Starting`.
                drop(claim_guard);
                tracing::debug!(
                    target: "lingxi_lsp::registry",
                    "LSP server instance started: {name}"
                );
                self.file_route_cache
                    .write()
                    .await
                    .insert(path.to_path_buf(), name);
                Ok(raw.connection_id)
            }
            Err(e) => {
                if let Some(connection_id) = claim_guard.owned_connection() {
                    if let Err(cleanup_error) = self.transport.terminate(connection_id).await {
                        tracing::warn!(
                            target: "lingxi_lsp::registry",
                            server = %name,
                            %cleanup_error,
                            "failed to terminate LSP server after startup error"
                        );
                    } else {
                        claim_guard.release_connection();
                    }
                }
                // claude-code: `Failed to start LSP server '${name}': ...` +
                // tengu_feature_bad(lsp_server_start / lsp_server_start_failed).
                tracing::error!(
                    target: "lingxi_lsp::registry",
                    "Failed to start LSP server '{name}': {e}"
                );
                let mut servers = self.servers.write().await;
                // Skip the write when the server was unregistered mid-start.
                if servers.contains_key(&name) {
                    servers.insert(
                        name.clone(),
                        LspConnectionState::Failed {
                            config,
                            error: e.to_string(),
                            restarts: prior_failures + 1,
                            max_recovery_reported: false,
                        },
                    );
                }
                drop(servers);
                drop(claim_guard);
                Err(e)
            }
        }
    }

    async fn record_dead_connection(&self, name: &str) {
        let observed = {
            let servers = self.servers.read().await;
            match servers.get(name) {
                Some(LspConnectionState::Initialized {
                    config,
                    connection_id,
                    restarts,
                    ..
                }) => Some((config.clone(), *connection_id, *restarts)),
                _ => None,
            }
        };
        let Some((config, connection_id, restarts)) = observed else {
            return;
        };
        if self.transport.is_alive(connection_id).await {
            return;
        }
        let mut servers = self.servers.write().await;
        if !matches!(
            servers.get(name),
            Some(LspConnectionState::Initialized { connection_id: current, .. })
                if *current == connection_id
        ) {
            return;
        }
        let error = format!("LSP server {name} crashed");
        servers.insert(
            name.to_string(),
            LspConnectionState::Failed {
                config,
                error,
                restarts: restarts.saturating_add(1),
                max_recovery_reported: false,
            },
        );
        drop(servers);
        self.clients.write().await.remove(name);
        self.subscribers.write().await.remove(name);
        self.open_files.clear_server(name).await;
    }

    /// Transition `name` to `Starting` and register the in-flight claim's
    /// watch sender, both under the caller's `servers` write lock — the
    /// atomic "I own this start attempt" step of
    /// [`Self::ensure_server_for_file`].
    fn claim_start(
        &self,
        servers: &mut HashMap<String, LspConnectionState>,
        name: &str,
        config: LspServerConfig,
        restarts: u32,
    ) {
        self.starting
            .lock()
            .expect("lsp start-claim lock poisoned")
            .insert(name.to_string(), watch::channel(()).0);
        servers.insert(
            name.to_string(),
            LspConnectionState::Starting {
                config,
                started_at: SystemTime::now(),
                pid: 0,
                restarts,
            },
        );
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
        let workspace_cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        self.ensure_client_for_file_in_workspace(path, &workspace_cwd)
            .await
    }

    /// Resolve the client for `path` using the caller's live session cwd for
    /// any first-time server initialization.
    pub async fn ensure_client_for_file_in_workspace(
        &self,
        path: &Path,
        workspace_cwd: &Path,
    ) -> Result<(String, Arc<LspClient>, LspServerConfig), LspError> {
        self.ensure_server_for_file_in_workspace(path, workspace_cwd)
            .await?;
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

    /// Push a successful file-tool edit into the matching language server.
    ///
    /// The first edit opens the current text; later edits send a full-text
    /// `didChange` followed by `didSave`. Callers intentionally treat errors as
    /// best-effort so code changes never fail merely because an optional LSP
    /// server is missing or unhealthy.
    pub async fn sync_file_after_edit(&self, path: &Path, text: &str) -> Result<(), LspError> {
        let workspace_cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        self.sync_file_after_edit_in_workspace(path, text, &workspace_cwd)
            .await
    }

    /// Push a successful edit while resolving a newly started server against
    /// the caller's live session cwd.
    pub async fn sync_file_after_edit_in_workspace(
        &self,
        path: &Path,
        text: &str,
        workspace_cwd: &Path,
    ) -> Result<(), LspError> {
        if text.len() as u64 > crate::tool_operations::MAX_LSP_FILE_SIZE_BYTES {
            return Ok(());
        }
        let (_, client, config) = self
            .ensure_client_for_file_in_workspace(path, workspace_cwd)
            .await?;
        let uri = lsp_types::Url::from_file_path(path).map_err(|()| {
            LspError::Transport(format!(
                "cannot convert path to file URI: {}",
                path.display()
            ))
        })?;
        crate::tool_operations::sync_document_text(
            &client,
            &self.open_files,
            &config,
            path,
            &uri,
            text,
        )
        .await
        .map_err(|error| LspError::Transport(error.to_string()))
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
    ///
    /// Each removed server that was `Initialized` is best-effort STOPPED via
    /// the transport (2.1.207 P2-09) — claude-code's plugin-refresh path shuts
    /// the old manager instance's running servers down (its `stop` logs
    /// `Failed to stop LSP server '{name}': ...` + `tengu_feature_sad`
    /// `lsp_server_stop` / `lsp_server_stop_failed` and continues), so an
    /// unloaded plugin's child process never outlives its registration.
    pub async fn unregister_plugin(&self, plugin_id: &PluginId) -> Vec<String> {
        let names = self
            .plugin_servers
            .write()
            .await
            .remove(plugin_id)
            .unwrap_or_default();
        if !names.is_empty() {
            // Collect the live connections while removing the entries; a
            // server still `Starting` has no connection yet — its in-flight
            // starter observes the missing entry on completion and stops the
            // fresh child itself (see `ensure_server_for_file`).
            let mut live: Vec<(String, McpConnectionId)> = Vec::new();
            {
                let mut servers = self.servers.write().await;
                let mut clients = self.clients.write().await;
                let mut subs = self.subscribers.write().await;
                for n in &names {
                    if let Some(LspConnectionState::Initialized { connection_id, .. }) =
                        servers.remove(n)
                    {
                        live.push((n.clone(), connection_id));
                    }
                    clients.remove(n);
                    subs.remove(n); // drop aborts the subscriber task
                }
            }
            for name in &names {
                self.open_files.clear_server(name).await;
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
            for (n, connection_id) in live {
                match self.transport.shutdown(connection_id).await {
                    Ok(()) => tracing::debug!(
                        target: "lingxi_lsp::registry",
                        "LSP server instance stopped: {n}"
                    ),
                    Err(e) => tracing::error!(
                        target: "lingxi_lsp::registry",
                        "Failed to stop LSP server '{n}': {e}"
                    ),
                }
            }
            self.has_registered_servers
                .store(!self.servers.read().await.is_empty(), Ordering::Release);
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
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicBool, Ordering};
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
    /// records the name of every server config it is asked to start (plus
    /// every connection it is asked to shut down).
    struct MockTransport {
        conn: Arc<Connection>,
        id: McpConnectionId,
        started: std::sync::Mutex<Vec<String>>,
        started_workspace_folders: std::sync::Mutex<Vec<Option<String>>>,
        initialized_roots: std::sync::Mutex<Vec<String>>,
        shutdowns: std::sync::Mutex<Vec<McpConnectionId>>,
        terminated: std::sync::Mutex<Vec<McpConnectionId>>,
        /// When set, `start_server` parks until a permit is added — lets the
        /// race tests hold an attempt in-flight; entry is signaled via
        /// `entered`.
        start_gate: Option<Arc<tokio::sync::Semaphore>>,
        start_entered: tokio::sync::Notify,
        /// When set, `initialize` parks until a permit is added — lets tests
        /// abort after the child exists but before startup publishes.
        initialize_gate: Option<Arc<tokio::sync::Semaphore>>,
        initialize_entered: tokio::sync::Notify,
        /// When set, `shutdown` parks until a permit is added — lets tests
        /// abort while the registry is trying to stop a child whose
        /// registration disappeared mid-start.
        shutdown_gate: Option<Arc<tokio::sync::Semaphore>>,
        shutdown_entered: tokio::sync::Notify,
        /// When true, `start_server` fails after passing the gate.
        fail_start: bool,
        /// When true, initialization fails after a child/connection id exists.
        fail_initialize: bool,
        fail_names: HashSet<String>,
        alive: AtomicBool,
    }
    impl MockTransport {
        fn new() -> Self {
            Self::build(None, None, None, false, false, HashSet::new())
        }
        fn failing() -> Self {
            Self::build(None, None, None, true, false, HashSet::new())
        }
        fn initialization_failing() -> Self {
            Self::build(None, None, None, false, true, HashSet::new())
        }
        fn failing_names(names: &[&str]) -> Self {
            Self::build(
                None,
                None,
                None,
                false,
                false,
                names.iter().map(|name| (*name).to_string()).collect(),
            )
        }
        fn gated_start(gate: Arc<tokio::sync::Semaphore>, fail_start: bool) -> Self {
            Self::build(Some(gate), None, None, fail_start, false, HashSet::new())
        }
        fn gated_initialize(gate: Arc<tokio::sync::Semaphore>) -> Self {
            Self::build(None, Some(gate), None, false, false, HashSet::new())
        }
        fn gated_initialize_and_shutdown(
            initialize_gate: Arc<tokio::sync::Semaphore>,
            shutdown_gate: Arc<tokio::sync::Semaphore>,
        ) -> Self {
            Self::build(
                None,
                Some(initialize_gate),
                Some(shutdown_gate),
                false,
                false,
                HashSet::new(),
            )
        }
        fn build(
            start_gate: Option<Arc<tokio::sync::Semaphore>>,
            initialize_gate: Option<Arc<tokio::sync::Semaphore>>,
            shutdown_gate: Option<Arc<tokio::sync::Semaphore>>,
            fail_start: bool,
            fail_initialize: bool,
            fail_names: HashSet<String>,
        ) -> Self {
            let (a, _b) = tokio::io::duplex(256);
            let (r, w) = tokio::io::split(a);
            Self {
                conn: Arc::new(Connection::new_line_delimited(r, w)),
                id: McpConnectionId::new(),
                started: std::sync::Mutex::new(Vec::new()),
                started_workspace_folders: std::sync::Mutex::new(Vec::new()),
                initialized_roots: std::sync::Mutex::new(Vec::new()),
                shutdowns: std::sync::Mutex::new(Vec::new()),
                terminated: std::sync::Mutex::new(Vec::new()),
                start_gate,
                start_entered: tokio::sync::Notify::new(),
                initialize_gate,
                initialize_entered: tokio::sync::Notify::new(),
                shutdown_gate,
                shutdown_entered: tokio::sync::Notify::new(),
                fail_start,
                fail_initialize,
                fail_names,
                alive: AtomicBool::new(true),
            }
        }

        fn mark_crashed(&self) {
            self.alive.store(false, Ordering::Release);
        }
    }
    #[async_trait]
    impl LspTransport for MockTransport {
        async fn start_server(
            &self,
            config: &LspServerConfig,
        ) -> Result<LspRawConnection, LspError> {
            self.start_entered.notify_one();
            if let Some(gate) = &self.start_gate {
                gate.acquire().await.expect("gate closed").forget();
            }
            self.started.lock().unwrap().push(config.name.clone());
            self.started_workspace_folders
                .lock()
                .unwrap()
                .push(config.workspace_folder.clone());
            if self.fail_start || self.fail_names.contains(&config.name) {
                return Err(LspError::Transport(format!(
                    "spawn {}: mock failure",
                    config.command
                )));
            }
            self.alive.store(true, Ordering::Release);
            Ok(LspRawConnection {
                connection_id: self.id,
            })
        }
        async fn initialize(
            &self,
            _conn: &LspRawConnection,
            root_uri: &str,
        ) -> Result<LspServerCapabilities, LspError> {
            self.initialize_entered.notify_one();
            if let Some(gate) = &self.initialize_gate {
                gate.acquire().await.expect("gate closed").forget();
            }
            self.initialized_roots
                .lock()
                .unwrap()
                .push(root_uri.to_string());
            if self.fail_initialize {
                Err(LspError::Transport("mock initialize failure".into()))
            } else {
                Ok(caps())
            }
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
        async fn shutdown(&self, id: McpConnectionId) -> Result<(), LspError> {
            self.shutdown_entered.notify_one();
            if let Some(gate) = &self.shutdown_gate {
                gate.acquire().await.expect("gate closed").forget();
            }
            self.shutdowns.lock().unwrap().push(id);
            Ok(())
        }
        async fn terminate(&self, id: McpConnectionId) -> Result<(), LspError> {
            self.terminated.lock().unwrap().push(id);
            self.alive.store(false, Ordering::Release);
            Ok(())
        }
        async fn is_alive(&self, _conn_id: McpConnectionId) -> bool {
            self.alive.load(Ordering::Acquire)
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
            ..Default::default()
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

    #[tokio::test]
    async fn first_start_uses_the_callers_live_workspace_cwd() {
        let transport = Arc::new(MockTransport::new());
        let reg = LspRegistry::new(transport.clone());
        reg.register_config(rust_config()).await;
        let temp = tempfile::tempdir().expect("temp workspace");
        let live_cwd = temp.path().join("after-cd");

        reg.ensure_server_for_file_in_workspace(&live_cwd.join("src/main.rs"), &live_cwd)
            .await
            .expect("server starts from live session cwd");

        let expected = lsp_types::Url::from_file_path(&live_cwd)
            .expect("absolute live cwd")
            .to_string();
        assert_eq!(
            *transport.initialized_roots.lock().unwrap(),
            vec![expected],
            "initialize root must not fall back to the host process cwd"
        );
    }

    #[tokio::test]
    async fn relative_workspace_folder_is_resolved_against_live_cwd() {
        let transport = Arc::new(MockTransport::new());
        let reg = LspRegistry::new(transport.clone());
        let temp = tempfile::tempdir().expect("temp workspace");
        let live_cwd = temp.path().join("after-cd");
        let mut config = rust_config();
        config.workspace_folder = Some("servers/rust".to_string());
        reg.register_config(config).await;

        reg.ensure_server_for_file_in_workspace(&live_cwd.join("src/main.rs"), &live_cwd)
            .await
            .expect("relative workspaceFolder resolves from live cwd");

        let expected = lsp_types::Url::from_file_path(live_cwd.join("servers/rust"))
            .expect("absolute workspace folder")
            .to_string();
        assert_eq!(*transport.initialized_roots.lock().unwrap(), vec![expected]);
    }

    #[tokio::test]
    async fn relative_workspace_folder_is_resolved_before_spawn() {
        let transport = Arc::new(MockTransport::new());
        let reg = LspRegistry::new(transport.clone());
        let temp = tempfile::tempdir().expect("temp workspace");
        let live_cwd = temp.path().join("after-cd");
        let mut config = rust_config();
        config.workspace_folder = Some("servers/rust".to_string());
        reg.register_config(config).await;

        reg.ensure_server_for_file_in_workspace(&live_cwd.join("src/main.rs"), &live_cwd)
            .await
            .expect("relative workspaceFolder resolves before spawn");

        assert_eq!(
            *transport.started_workspace_folders.lock().unwrap(),
            vec![Some(
                live_cwd.join("servers/rust").to_string_lossy().into_owned()
            )],
            "spawn should receive the resolved workspace folder so the child cwd matches Claude"
        );
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

    /// The routing table is extension → ordered array in registration order:
    /// the first server is primary and later same-extension servers are only
    /// fallbacks.
    #[tokio::test]
    async fn first_registered_server_wins_when_healthy() {
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
            "fallback same-extension server is not started while primary is healthy"
        );
    }

    #[tokio::test]
    async fn same_extension_fallback_starts_when_primary_fails() {
        let transport = Arc::new(MockTransport::failing_names(&["zzz-first"]));
        let reg = LspRegistry::new(transport.clone());
        reg.register_config(named_rust_config("zzz-first")).await;
        reg.register_config(named_rust_config("aaa-second")).await;

        let (name, _, _) = reg
            .ensure_client_for_file(Path::new("/p/a.rs"))
            .await
            .expect("fallback server should start");
        assert_eq!(name, "aaa-second");
        assert_eq!(
            *transport.started.lock().unwrap(),
            vec!["zzz-first".to_string(), "aaa-second".to_string()],
            "registry should try the primary and then the fallback"
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

    /// Regression (2.1.207 P2-09): two concurrent FIRST requests for the same
    /// extension must spawn the server exactly once — claude-code's start fn
    /// early-returns while the state is `starting`, so the loser of the race
    /// awaits the in-flight attempt instead of spawning a second child (which
    /// previously also leaked forever in the transport's connection map).
    #[tokio::test]
    async fn concurrent_first_requests_start_the_server_once() {
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let transport = Arc::new(MockTransport::gated_start(gate.clone(), false));
        let reg = Arc::new(LspRegistry::new(transport.clone()));
        reg.register_config(rust_config()).await;

        let a = tokio::spawn({
            let reg = reg.clone();
            async move { reg.ensure_server_for_file(Path::new("/p/a.rs")).await }
        });
        // Task A is parked inside start_server, holding the start claim…
        transport.start_entered.notified().await;
        let b = tokio::spawn({
            let reg = reg.clone();
            async move { reg.ensure_server_for_file(Path::new("/p/b.rs")).await }
        });
        // …let B observe `Starting` and subscribe, then release the spawn.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        gate.add_permits(1);

        let id_a = a.await.unwrap().expect("winner starts the server");
        let id_b = b.await.unwrap().expect("loser reuses the winner's start");
        assert_eq!(id_a, id_b, "both callers share one connection");
        assert_eq!(
            transport.started.lock().unwrap().len(),
            1,
            "start_server ran exactly once"
        );
    }

    /// Regression (2.1.207 P2-09): a failing start records `Failed` (previous
    /// code left the server `Disconnected`, eligible for unbounded respawn).
    /// Later requests retry until the failure count EXCEEDS claude-code's
    /// `maxRestarts ?? 3` (initial attempt + 3 recovery attempts), then the
    /// byte-exact error `LSP server '{name}' exceeded max crash recovery
    /// attempts (3)` is returned with NO further spawn.
    #[tokio::test]
    async fn failed_start_is_retried_then_capped_at_max_crash_recovery() {
        let transport = Arc::new(MockTransport::failing());
        let reg = LspRegistry::new(transport.clone());
        reg.register_config(rust_config()).await;

        for attempt in 1..=(DEFAULT_MAX_RESTARTS + 1) {
            let err = reg
                .ensure_server_for_file(Path::new("/p/a.rs"))
                .await
                .expect_err("mock start always fails");
            assert!(
                matches!(err, LspError::Transport(_)),
                "attempt {attempt} surfaces the spawn error"
            );
            assert!(
                matches!(
                    reg.servers.read().await.get("rust-analyzer"),
                    Some(LspConnectionState::Failed { .. })
                ),
                "attempt {attempt} records the Failed state"
            );
            assert_eq!(
                transport.started.lock().unwrap().len(),
                attempt as usize,
                "attempt {attempt} really spawned"
            );
        }
        for _ in 0..2 {
            let err = reg
                .ensure_server_for_file(Path::new("/p/a.rs"))
                .await
                .expect_err("capped server refuses to start");
            assert_eq!(
                err.to_string(),
                "LSP server 'rust-analyzer' exceeded max crash recovery attempts (3)"
            );
            assert_eq!(
                transport.started.lock().unwrap().len(),
                (DEFAULT_MAX_RESTARTS + 1) as usize,
                "capped server is never spawned again"
            );
        }
    }

    #[tokio::test]
    async fn successful_restart_resets_crash_recovery_budget() {
        let transport = Arc::new(MockTransport::new());
        let reg = LspRegistry::new(transport.clone());
        let mut config = rust_config();
        config.max_restarts = Some(2);
        reg.register_config(config).await;

        reg.ensure_server_for_file(Path::new("/p/a.rs"))
            .await
            .expect("initial start");
        for _ in 0..2 {
            transport.mark_crashed();
            reg.ensure_server_for_file(Path::new("/p/a.rs"))
                .await
                .expect("successful restart should reset crash recovery budget");
        }

        assert_eq!(
            transport.started.lock().unwrap().len(),
            3,
            "initial start plus exactly two successful restarts"
        );
        assert!(
            matches!(
                reg.servers.read().await.get("rust-analyzer"),
                Some(LspConnectionState::Initialized { restarts: 0, .. })
            ),
            "successful startup must reset the restart counter"
        );
    }

    #[tokio::test]
    async fn initialize_failure_terminates_the_spawned_connection() {
        let transport = Arc::new(MockTransport::initialization_failing());
        let reg = LspRegistry::new(transport.clone());
        reg.register_config(rust_config()).await;

        let error = reg
            .ensure_server_for_file(Path::new("/p/a.rs"))
            .await
            .expect_err("mock initialize fails");
        assert!(matches!(
            error,
            LspError::Transport(message) if message == "mock initialize failure"
        ));
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if *transport.terminated.lock().unwrap() == vec![transport.id] {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("post-spawn initialize failure must discard the owned child");
    }

    #[tokio::test]
    async fn cancelled_startup_terminates_spawned_connection_and_resets_restarts_after_recovery() {
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let transport = Arc::new(MockTransport::gated_initialize(gate.clone()));
        let reg = Arc::new(LspRegistry::new(transport.clone()));
        let config = rust_config();
        reg.register_config(config.clone()).await;
        reg.servers.write().await.insert(
            "rust-analyzer".to_string(),
            LspConnectionState::Failed {
                config,
                error: "prior crash".into(),
                restarts: 2,
                max_recovery_reported: false,
            },
        );

        let task = tokio::spawn({
            let reg = reg.clone();
            async move { reg.ensure_server_for_file(Path::new("/p/a.rs")).await }
        });
        transport.initialize_entered.notified().await;
        task.abort();
        let join = task.await.expect_err("startup task should be cancelled");
        assert!(join.is_cancelled(), "task abort must cancel startup");

        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if transport.terminated.lock().unwrap().len() == 1
                    && !reg
                        .starting
                        .lock()
                        .expect("lsp start-claim lock poisoned")
                        .contains_key("rust-analyzer")
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("cancelled startup should terminate and release its claim");

        gate.add_permits(1);
        let id = reg
            .ensure_server_for_file(Path::new("/p/a.rs"))
            .await
            .expect("next request should reclaim the stale startup");
        assert_eq!(id, transport.id);
        assert_eq!(
            transport.started.lock().unwrap().len(),
            2,
            "recovery request should own the next start"
        );
        assert_eq!(
            *transport.terminated.lock().unwrap(),
            vec![transport.id],
            "cancelled startup should discard the orphaned child exactly once"
        );
        assert!(
            matches!(
                reg.servers.read().await.get("rust-analyzer"),
                Some(LspConnectionState::Initialized { restarts: 0, .. })
            ),
            "successful recovery must reset the restart counter"
        );
    }

    #[tokio::test]
    async fn cancelled_unregister_cleanup_keeps_claim_until_drop_terminates_child() {
        let initialize_gate = Arc::new(tokio::sync::Semaphore::new(0));
        let shutdown_gate = Arc::new(tokio::sync::Semaphore::new(0));
        let transport = Arc::new(MockTransport::gated_initialize_and_shutdown(
            initialize_gate.clone(),
            shutdown_gate,
        ));
        let reg = Arc::new(LspRegistry::new(transport.clone()));
        let plugin = PluginId::new();
        reg.register_plugin_servers(plugin, vec![rust_config()])
            .await;

        let task = tokio::spawn({
            let reg = reg.clone();
            async move { reg.ensure_server_for_file(Path::new("/p/a.rs")).await }
        });
        transport.initialize_entered.notified().await;
        assert_eq!(
            reg.unregister_plugin(&plugin).await,
            vec!["rust-analyzer".to_string()]
        );

        initialize_gate.add_permits(1);
        transport.shutdown_entered.notified().await;
        assert!(
            reg.starting
                .lock()
                .expect("lsp start-claim lock poisoned")
                .contains_key("rust-analyzer"),
            "cleanup must retain the start claim while shutdown is still blocked"
        );
        task.abort();
        let join = task.await.expect_err("startup task should be cancelled");
        assert!(
            join.is_cancelled(),
            "task abort must cancel shutdown cleanup"
        );
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if *transport.terminated.lock().unwrap() == vec![transport.id]
                    && !reg
                        .starting
                        .lock()
                        .expect("lsp start-claim lock poisoned")
                        .contains_key("rust-analyzer")
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("drop should terminate the unregistered child before releasing the claim");
        assert!(
            transport.shutdowns.lock().unwrap().is_empty(),
            "cancelled shutdown must not report a completed graceful stop"
        );
    }

    /// Regression (2.1.207 P2-09): a caller that awaited another task's
    /// FAILING start attempt gets the recorded error back — it must not
    /// immediately claim a retry of its own (the retry belongs to a later
    /// request).
    #[tokio::test]
    async fn waiter_on_failed_start_gets_recorded_error_without_respawn() {
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let transport = Arc::new(MockTransport::gated_start(gate.clone(), true));
        let reg = Arc::new(LspRegistry::new(transport.clone()));
        reg.register_config(rust_config()).await;

        let a = tokio::spawn({
            let reg = reg.clone();
            async move { reg.ensure_server_for_file(Path::new("/p/a.rs")).await }
        });
        transport.start_entered.notified().await;
        let b = tokio::spawn({
            let reg = reg.clone();
            async move { reg.ensure_server_for_file(Path::new("/p/b.rs")).await }
        });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        gate.add_permits(1);

        let err_a = a.await.unwrap().expect_err("winner sees the spawn error");
        assert!(matches!(err_a, LspError::Transport(_)));
        let err_b = b
            .await
            .unwrap()
            .expect_err("waiter sees the recorded error");
        assert!(
            matches!(&err_b, LspError::ServerError(msg) if msg.contains("mock failure")),
            "waiter returns the recorded failure, got: {err_b}"
        );
        assert_eq!(
            transport.started.lock().unwrap().len(),
            1,
            "the waiter never spawned a second attempt"
        );
    }

    /// Regression (2.1.207 P2-09): unregistering a plugin STOPS its live
    /// servers via the transport — claude-code's plugin refresh shuts the old
    /// manager instance down, so the child process never outlives the
    /// registration (previously nothing ever called `transport.shutdown`,
    /// leaking the child until app exit).
    #[tokio::test]
    async fn unregister_plugin_shuts_down_live_servers() {
        let transport = Arc::new(MockTransport::new());
        let reg = LspRegistry::new(transport.clone());
        let plugin = PluginId::new();
        reg.register_plugin_servers(plugin, vec![rust_config()])
            .await;
        let id = reg
            .ensure_server_for_file(Path::new("/p/a.rs"))
            .await
            .expect("starts while registered");

        let removed = reg.unregister_plugin(&plugin).await;
        assert_eq!(removed, vec!["rust-analyzer".to_string()]);
        assert_eq!(
            *transport.shutdowns.lock().unwrap(),
            vec![id],
            "the live child is shut down, not leaked"
        );
    }
}

/// The file's lowercased extension WITH a leading dot (e.g. `.rs`), matching
/// the keys of [`LspServerConfig::extension_to_language`]. `None` for an
/// extensionless file.
fn file_extension(path: &Path) -> Option<String> {
    path.extension()
        .map(|e| format!(".{}", e.to_string_lossy().to_ascii_lowercase()))
}

/// The `file://` URI used to initialize a server. An explicit
/// `workspaceFolder` overrides the live session cwd; relative overrides are
/// resolved against that cwd.
fn workspace_root_uri(config: &LspServerConfig, workspace_cwd: &Path) -> String {
    let root = config.workspace_folder.as_deref().map_or_else(
        || workspace_cwd.to_path_buf(),
        |workspace_folder| {
            let path = PathBuf::from(workspace_folder);
            if path.is_absolute() {
                path
            } else {
                workspace_cwd.join(path)
            }
        },
    );
    lsp_types::Url::from_file_path(&root).map_or_else(
        |()| format!("file://{}", root.to_string_lossy()),
        |uri| uri.to_string(),
    )
}

fn startup_config_for_workspace(config: &LspServerConfig, workspace_cwd: &Path) -> LspServerConfig {
    let mut startup = config.clone();
    startup.workspace_folder = config.workspace_folder.as_deref().map(|workspace_folder| {
        let path = PathBuf::from(workspace_folder);
        let resolved = if path.is_absolute() {
            path
        } else {
            workspace_cwd.join(path)
        };
        resolved.to_string_lossy().into_owned()
    });
    startup
}

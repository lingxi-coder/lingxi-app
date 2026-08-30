//! Owns every MCP connection and drives the state machine.
//!
//! Engine code holds an `Arc<McpRegistry>` and uses [`Self::connect`] /
//! [`Self::disconnect`] to manage servers. Startup auto-connect
//! ([`McpRegistry::connect_all`]) and the reconnect/backoff loop
//! ([`McpRegistry::run_reconnect_loop`]) implement the "Plan 13" wiring.

use crate::client::McpClient;
use crate::connection::{McpConnectionState, McpServerConfig};
use crate::hook_dispatch::HookDispatcher;
use crate::normalization::normalize_name_for_mcp;
use crate::oauth::{self, OnAuthorizationUrl};
use crate::raw_conn::RawConnectionProvider;
use indexmap::IndexMap;
use protocol::{AgentId, McpConnectionId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, SystemTime};
use tokio::sync::{broadcast, Mutex, RwLock};
use traits::{
    Clock, HttpTransport, McpError, McpRawConnection, McpTransport, McpTransportSpec,
    SecureStorage, ServerCapabilitiesDto,
};

/// OAuth seam injected into the registry for remote (SSE/HTTP) MCP servers that
/// declare an `oauth` config. When unset, OAuth-configured servers fall back to
/// their static headers (no Bearer attach), and static-token servers are
/// entirely unaffected. Mirrors claude-code's `services/mcp/auth.ts` wiring.
#[derive(Clone)]
pub struct OAuthDeps {
    /// HTTP transport used for `.well-known` discovery, DCR, token exchange,
    /// and refresh against the authorization server.
    pub http: Arc<dyn HttpTransport>,
    /// Wall-clock source used to compute / check token expiry.
    pub clock: Arc<dyn Clock>,
    /// Secure storage backing per-server token persistence (`mcp-oauth`
    /// service, account = `oauth::server_key`).
    pub storage: Arc<dyn SecureStorage>,
    /// Host hook invoked with the authorization URL so the TUI / desktop can
    /// open a browser. Fired once per interactive flow.
    pub on_authorization_url: OnAuthorizationUrl,
    /// Optional Cross-App-Access (XAA / SEP-990) config provider. When wired AND
    /// `LINGXI_ENABLE_XAA` is truthy, an `oauth.xaa==Some(true)` server resolves
    /// its token via the RFC 8693 → RFC 7523 token-exchange chain
    /// ([`crate::xaa::perform_cross_app_access`]) instead of the consent flow.
    ///
    /// This provider supplies the per-server IdP + AS inputs (`id_token`, AS
    /// `client_secret`, IdP token endpoint) that claude-code gathers via
    /// `getXaaIdpSettings`/`acquireIdpIdToken`/`discoverOidc`/`mcpOAuthClientConfig`
    /// (auth.ts:676-744). That IdP-login/secret surface has no config seam in
    /// this codebase yet, so when this is `None` an XAA-flagged server hard-fails
    /// with an actionable residual message rather than silently degrading.
    // Dense OAuth/OIDC vocabulary (IdP, OIDC, AS) reads worse backticked.
    #[allow(clippy::doc_markdown)]
    pub xaa_config: Option<Arc<dyn XaaConfigProvider>>,
}

/// Per-server XAA inputs (the IdP `id_token` + AS/IdP credentials) gathered by
/// the host. Mirrors the bundle claude-code's `performMCPXaaAuth` assembles
/// from user settings + keychain before calling `performCrossAppAccess`
/// (auth.ts:676-744). The IdP browser-login that mints `id_token`
/// (`acquireIdpIdToken`) lives behind this seam.
#[allow(clippy::doc_markdown)]
#[derive(Debug, Clone)]
pub struct XaaInputs {
    /// AS-registered confidential client id (`serverConfig.oauth.clientId`).
    pub client_id: String,
    /// AS-registered confidential client secret (`mcpOAuthClientConfig`).
    pub client_secret: String,
    /// IdP-registered client id (`idp.clientId`).
    pub idp_client_id: String,
    /// Optional IdP client secret (`getIdpClientSecret`).
    pub idp_client_secret: Option<String>,
    /// The user's OIDC `id_token` (cached or freshly minted by IdP login).
    pub idp_id_token: String,
    /// IdP token endpoint (`discoverOidc(...).token_endpoint`).
    pub idp_token_endpoint: String,
}

/// Seam supplying [`XaaInputs`] for an XAA-flagged server. Implemented by the
/// host (desktop/CLI) once the IdP-settings + secret config surface exists.
#[async_trait::async_trait]
pub trait XaaConfigProvider: Send + Sync {
    /// Resolve the XAA inputs for `server_name` / `server_url`, performing the
    /// IdP login (or cache hit) as needed. Returns `Ok(None)` when this server
    /// is not actually XAA-provisioned (caller hard-fails with guidance).
    async fn xaa_inputs(
        &self,
        server_name: &str,
        server_url: &str,
    ) -> Result<Option<XaaInputs>, McpError>;

    /// Drop the cached IdP `id_token` so the next [`Self::xaa_inputs`] call
    /// re-acquires a fresh one (auth.ts `clearIdpIdToken(idp.issuer)`, 1840-1847).
    ///
    /// Called by [`McpRegistry::resolve_xaa_token`] only when the cross-app
    /// token exchange returns a 4xx (the cached `id_token` was rejected); a 5xx
    /// (IdP outage) keeps it. The default is a no-op so providers that don't
    /// cache an `id_token` need no change.
    async fn clear_id_token(&self) -> Result<(), McpError> {
        Ok(())
    }
}

/// Initial reconnect backoff (claude-code `INITIAL_BACKOFF_MS = 1000`).
const INITIAL_BACKOFF: Duration = Duration::from_millis(1000);
/// Ceiling on reconnect backoff (claude-code `MAX_BACKOFF_MS = 30000`).
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// MCP server catalog affected by an inbound `notifications/*/list_changed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpCatalogKind {
    /// The server's `tools/list` result changed.
    Tools,
    /// The server's `prompts/list` result changed.
    Prompts,
    /// The server's `resources/list` result changed.
    Resources,
}

/// One list-changed notification associated with the connection that emitted it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpCatalogChanged {
    /// Raw configured server name.
    pub server_name: String,
    /// Connection generation that emitted the notification.
    pub connection_id: McpConnectionId,
    /// Previous connection partition to remove before applying this change.
    /// Set on disconnect; ordinary list invalidations and fresh connections
    /// leave it `None`.
    pub retired_connection_id: Option<McpConnectionId>,
    /// Catalog to refresh.
    pub kind: McpCatalogKind,
}

#[derive(Clone)]
struct RegisteredClient {
    connection_id: Option<McpConnectionId>,
    client: Arc<McpClient>,
}

/// In-memory registry of every known MCP connection.
pub struct McpRegistry {
    /// Map of server name to current state.
    ///
    /// `pub` so the CLI binary (M6-07 init.rs) can pre-populate
    /// `Disconnected` entries read from `.mcp.json` before the engine
    /// connects, and so engine-side tests can seed states directly.
    pub connections: RwLock<HashMap<String, McpConnectionState>>,
    /// Synchronous mirror of claude-code 2.1.238's `eZf()`
    /// (`bdl(b7e()??[]).length>0`, `cc-238.js @229641619`) — "at least one MCP
    /// client is `type === "pending"`".
    ///
    /// [`Self::connections`] lives behind an async `RwLock`, but the predicate
    /// is needed from `Tool::is_enabled`, which is synchronous. Callers that can
    /// `await` refresh it with [`Self::refresh_pending_servers`] before building
    /// a tool list; [`Self::has_pending_servers`] then reads it without
    /// blocking. Starts `false`, so a host that never refreshes behaves exactly
    /// as it did before this mirror existed.
    pending_servers: std::sync::atomic::AtomicBool,
    /// Serializes connect/disconnect/reconnect for each logical server without
    /// holding the public connection-state lock across transport or OAuth I/O.
    /// Different servers still progress independently.
    lifecycle_locks: StdMutex<HashMap<String, Arc<Mutex<()>>>>,
    /// Single-flight guard for the XAA token resolve chain, keyed by
    /// `oauth::server_key`. Mirrors the oracle's `_refreshInProgress`
    /// promise-sharing guard on `tokens()`/`xaaRefresh()` (@182213696, §26b
    /// delta 2): concurrent resolves for the SAME server share one exchange —
    /// a resolver that has to wait re-reads storage once it acquires the lock
    /// and reuses whatever the winner just persisted instead of re-exchanging.
    xaa_refresh_locks: StdMutex<HashMap<String, Arc<Mutex<()>>>>,
    /// Side-channel cache of [`McpClient`] handles per server name.
    ///
    /// Populated by [`Self::register_client`] / [`Self::register_connected_client`].
    /// Production wiring records the exact connection generation alongside the
    /// client so prompt dispatch can fail closed when a reconnect swaps the
    /// live client between command discovery and `prompts/get`.
    ///
    /// Insertion-ordered ([`IndexMap`]) so [`Self::servers_with_tools`] returns
    /// server names in DISCOVERY order — claude builds `serversWithTools` by
    /// iterating `appState.mcp.tools` in order with no sort
    /// (`AgentTool.tsx:394-405`). A plain `HashMap` would make the required-MCP
    /// gate error text non-deterministic.
    clients: RwLock<IndexMap<String, RegisteredClient>>,
    /// Fan-out for inbound server catalog invalidations. The engine subscribes
    /// once and refreshes the shared tool registry after a successful
    /// `tools/list`; lagged consumers can safely refresh from the latest state.
    catalog_changes: broadcast::Sender<McpCatalogChanged>,
    /// Per-agent connection scoping (subagent isolation).
    #[allow(dead_code)] // populated by `register_for_agent` in Plan 13
    agent_scoped: RwLock<HashMap<AgentId, HashMap<String, McpConnectionId>>>,
    /// Transport boundary supplied by the host platform.
    transport: Arc<dyn McpTransport>,
    /// Optional bridge to the transport's live `jsonrpc::Connection`s.
    ///
    /// When `Some`, [`Self::connect`] builds an [`McpClient`] over the
    /// transport-owned connection and caches it via [`Self::register_client`],
    /// so the 4 builtin MCP tools reach the server at runtime (via
    /// [`Self::get_client`]). When `None` (the `new` path), no client is
    /// registered and `get_client` keeps returning whatever was seeded
    /// manually (e.g. `register_test_client`). Mirrors claude-code's
    /// `ensureConnectedClient` returning a live client (client.ts:1688-1709).
    raw_conn: Option<Arc<dyn RawConnectionProvider>>,
    /// Optional hook-dispatch seam forwarded into each [`McpClient`]'s
    /// `elicitation/create` handler. When `Some`, an incoming elicitation
    /// consults the engine's `Elicitation` hook (claude-code
    /// `runElicitationHooks`); when `None` (the default) the handler keeps its
    /// `{"action":"cancel"}` behavior. Set via [`Self::with_hook_dispatcher`],
    /// matching the `RawConnectionProvider` injection pattern.
    hook_dispatcher: Option<Arc<dyn HookDispatcher>>,
    /// Optional OAuth 2.1 + PKCE seam for remote MCP servers configured with
    /// an `oauth` block. When `None` (the default), [`Self::connect`] never
    /// runs the OAuth flow and OAuth-configured servers connect with only their
    /// static headers; static-token servers are unaffected either way. Wired
    /// via [`Self::with_oauth`].
    oauth: Option<OAuthDeps>,
    /// Session cwd used by dynamic MCP header helpers.
    headers_helper_cwd: std::path::PathBuf,
    /// Plugin roots keyed by scoped MCP server name.
    headers_helper_plugin_roots: RwLock<HashMap<String, std::path::PathBuf>>,
    /// LIVE additional working directories (settings `additionalDirectories`
    /// union CLI `--add-dir`, plus any runtime `/add-dir`) advertised alongside
    /// cwd on each server's `roots/list`.
    ///
    /// A SHARED [`crate::SharedRoots`] cell — the SAME `Arc` is forwarded into
    /// every [`McpClient`] built by [`Self::connect`] (via
    /// [`McpClient::with_roots`]), so a directory pushed via [`Self::add_root`]
    /// at runtime is seen by ALL connected servers' `roots/list` handlers
    /// without a reconnect, matching claude-code 2.1.207 `r1d()`
    /// (`[cwd, ...additionalWorkingDirectories]`). Empty by default (cwd-only
    /// roots, unchanged). Set via [`Self::with_additional_roots`].
    additional_roots: crate::SharedRoots,
    /// Interval used by the background health-check task.
    #[allow(dead_code)] // consumed by the health-check loop in Plan 13
    pub health_check_interval: Duration,
    /// Maximum consecutive reconnect attempts before declaring `Failed`.
    ///
    /// Consumed by [`Self::run_reconnect_loop`]; matches claude-code's
    /// `MAX_RECONNECT_ATTEMPTS = 5`.
    pub max_retry_count: u32,
}

/// Message for a remote server with no usable URL (oracle
/// `"No URL configured for this server"`).
pub const UNCONFIGURED_MESSAGE: &str = "No URL configured for this server";

/// Is this a remote (url-bearing) spec whose URL is blank?
///
/// Oracle `zar(e)`'s fallback arm: `!e.configError && "url" in e &&
/// e.url.trim() === ""`. Stdio servers have no url and are never unconfigured
/// by this test.
#[must_use]
pub fn is_unconfigured_remote(spec: &McpTransportSpec) -> bool {
    match spec {
        McpTransportSpec::Sse { url, .. }
        | McpTransportSpec::Http { url, .. }
        | McpTransportSpec::WebSocket { url, .. } => url.trim().is_empty(),
        _ => false,
    }
}

impl McpRegistry {
    /// Build a registry bound to a platform transport.
    ///
    /// No `RawConnectionProvider` is wired, so [`Self::connect`] does NOT build
    /// a live [`McpClient`] — use [`Self::with_raw_conn`] for that.
    #[must_use]
    pub fn new(transport: Arc<dyn McpTransport>) -> Self {
        let (catalog_changes, _unused_rx) = broadcast::channel(64);
        Self {
            connections: RwLock::new(HashMap::new()),
            pending_servers: std::sync::atomic::AtomicBool::new(false),
            lifecycle_locks: StdMutex::new(HashMap::new()),
            xaa_refresh_locks: StdMutex::new(HashMap::new()),
            clients: RwLock::new(IndexMap::new()),
            catalog_changes,
            agent_scoped: RwLock::new(HashMap::new()),
            transport,
            raw_conn: None,
            hook_dispatcher: None,
            oauth: None,
            headers_helper_cwd: std::env::current_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from(".")),
            headers_helper_plugin_roots: RwLock::new(HashMap::new()),
            additional_roots: crate::new_shared_roots(Vec::new()),
            health_check_interval: Duration::from_secs(30),
            max_retry_count: 5,
        }
    }

    fn lifecycle_lock(&self, name: &str) -> Arc<Mutex<()>> {
        let mut locks = self
            .lifecycle_locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Arc::clone(
            locks
                .entry(name.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(()))),
        )
    }

    /// Per-server-key single-flight lock for the XAA resolve chain (§26b
    /// delta 2). See [`Self::xaa_refresh_locks`].
    fn xaa_refresh_lock(&self, key: &str) -> Arc<Mutex<()>> {
        let mut locks = self
            .xaa_refresh_locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Arc::clone(
            locks
                .entry(key.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(()))),
        )
    }

    /// Subscribe to inbound MCP catalog invalidations.
    #[must_use]
    pub fn subscribe_catalog_changes(&self) -> broadcast::Receiver<McpCatalogChanged> {
        self.catalog_changes.subscribe()
    }

    /// Return one refresh request for every catalog currently advertised by
    /// every connected server. Consumers use this to recover deterministically
    /// after a lagged broadcast receiver instead of leaving a missed
    /// `list_changed` notification stale until the server happens to emit
    /// another one.
    pub async fn catalog_refresh_snapshot(&self) -> Vec<McpCatalogChanged> {
        let conns = self.connections.read().await;
        let mut changes = Vec::new();
        for (server_name, state) in conns.iter() {
            let McpConnectionState::Connected {
                connection_id,
                capabilities,
                ..
            } = state
            else {
                continue;
            };
            for (supported, kind) in [
                (capabilities.tools, McpCatalogKind::Tools),
                (capabilities.prompts, McpCatalogKind::Prompts),
                (capabilities.resources, McpCatalogKind::Resources),
            ] {
                if supported {
                    changes.push(McpCatalogChanged {
                        server_name: server_name.clone(),
                        connection_id: *connection_id,
                        retired_connection_id: None,
                        kind,
                    });
                }
            }
        }
        changes
    }

    /// Re-query the catalog named by `change` and atomically replace only that
    /// slice of the connected-state snapshot. A stale connection generation is
    /// ignored. The old catalog remains intact on RPC/decode failure.
    pub async fn refresh_catalog(
        &self,
        change: &McpCatalogChanged,
    ) -> Result<Option<McpConnectionId>, McpError> {
        let current_id = {
            let conns = self.connections.read().await;
            match conns.get(&change.server_name) {
                Some(McpConnectionState::Connected { connection_id, .. })
                    if *connection_id == change.connection_id =>
                {
                    *connection_id
                }
                _ => return Ok(None),
            }
        };
        let Some(client) = self.get_client(&change.server_name).await else {
            return Err(McpError::Internal(format!(
                "MCP server \"{}\" has no live client for catalog refresh",
                change.server_name
            )));
        };

        enum Refreshed {
            Tools(Vec<traits::McpToolDto>),
            Prompts(Vec<traits::McpPromptDto>),
            Resources(Vec<traits::McpResourceDto>),
        }
        let refreshed = match change.kind {
            McpCatalogKind::Tools => client
                .list_tools()
                .await
                .map(Refreshed::Tools)
                .map_err(|e| McpError::Internal(e.to_string()))?,
            McpCatalogKind::Prompts => client
                .list_prompts()
                .await
                .map(Refreshed::Prompts)
                .map_err(|e| McpError::Internal(e.to_string()))?,
            McpCatalogKind::Resources => client
                .list_resources()
                .await
                .map(Refreshed::Resources)
                .map_err(|e| McpError::Internal(e.to_string()))?,
        };

        let mut conns = self.connections.write().await;
        let Some(McpConnectionState::Connected {
            connection_id,
            tools,
            prompts,
            resources,
            ..
        }) = conns.get_mut(&change.server_name)
        else {
            return Ok(None);
        };
        if *connection_id != current_id {
            return Ok(None);
        }
        match refreshed {
            Refreshed::Tools(next) => *tools = next,
            Refreshed::Prompts(next) => *prompts = next,
            Refreshed::Resources(next) => *resources = next,
        }
        Ok(Some(current_id))
    }

    fn spawn_catalog_change_listener(
        &self,
        server_name: String,
        connection_id: McpConnectionId,
        connection: Arc<jsonrpc::Connection>,
    ) {
        let mut notifications = connection.notifications();
        let changes = self.catalog_changes.clone();
        tokio::spawn(async move {
            loop {
                let notification = match notifications.recv().await {
                    Ok(notification) => notification,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                };
                let kind = match notification.method.as_str() {
                    "notifications/tools/list_changed" => McpCatalogKind::Tools,
                    "notifications/prompts/list_changed" => McpCatalogKind::Prompts,
                    "notifications/resources/list_changed" => McpCatalogKind::Resources,
                    _ => continue,
                };
                let _ = changes.send(McpCatalogChanged {
                    server_name: server_name.clone(),
                    connection_id,
                    retired_connection_id: None,
                    kind,
                });
            }
        });
    }

    /// Build a registry bound to a platform transport AND a bridge to its live
    /// `jsonrpc::Connection`s (typically the same platform object implementing
    /// both [`McpTransport`] and [`RawConnectionProvider`]).
    ///
    /// With `raw_conn` wired, [`Self::connect`] builds an [`McpClient`] per
    /// connected server and caches it so [`Self::get_client`] returns a working
    /// client to the builtin MCP tools.
    #[must_use]
    pub fn with_raw_conn(
        transport: Arc<dyn McpTransport>,
        raw_conn: Arc<dyn RawConnectionProvider>,
    ) -> Self {
        Self {
            raw_conn: Some(raw_conn),
            ..Self::new(transport)
        }
    }

    /// Inject the optional [`HookDispatcher`] forwarded into every
    /// [`McpClient`] built by [`Self::connect`]. Builder-style so it composes
    /// with [`Self::new`] / [`Self::with_raw_conn`]:
    ///
    /// ```ignore
    /// let reg = McpRegistry::with_raw_conn(transport, raw_conn)
    ///     .with_hook_dispatcher(Some(orchestrator_dispatcher));
    /// ```
    ///
    /// `None` leaves the default behavior (handler returns
    /// `{"action":"cancel"}`); `Some(_)` enables the `Elicitation` hook path.
    #[must_use]
    pub fn with_hook_dispatcher(mut self, dispatcher: Option<Arc<dyn HookDispatcher>>) -> Self {
        self.hook_dispatcher = dispatcher;
        self
    }

    /// Inject the OAuth 2.1 + PKCE seam ([`OAuthDeps`]) for remote MCP servers.
    /// Builder-style so it composes with [`Self::new`] / [`Self::with_raw_conn`]:
    ///
    /// ```ignore
    /// let reg = McpRegistry::with_raw_conn(transport, raw_conn).with_oauth(deps);
    /// ```
    ///
    /// With this wired, [`Self::connect`] resolves a Bearer token for any
    /// `Sse{oauth:Some}` / `Http{oauth:Some}` server (load → refresh-on-expiry →
    /// interactive flow), injects `Authorization: Bearer <token>` into the
    /// spec's headers, and retries once after a 401. Static-token servers
    /// (`oauth: None`) take the unchanged path.
    #[must_use]
    pub fn with_oauth(mut self, deps: OAuthDeps) -> Self {
        self.oauth = Some(deps);
        self
    }

    /// Use the session cwd for every dynamic headers helper.
    #[must_use]
    pub fn with_headers_helper_cwd(mut self, cwd: std::path::PathBuf) -> Self {
        self.headers_helper_cwd = cwd;
        self
    }

    /// Associate a plugin MCP server with the plugin root used by its helper.
    pub async fn set_headers_helper_plugin_root(
        &self,
        server: impl Into<String>,
        root: std::path::PathBuf,
    ) {
        self.headers_helper_plugin_roots
            .write()
            .await
            .insert(server.into(), root);
    }

    /// Remove a plugin helper context when its owning plugin unloads.
    pub async fn remove_headers_helper_plugin_root(&self, server: &str) {
        self.headers_helper_plugin_roots
            .write()
            .await
            .remove(server);
    }

    /// Inject the session's LIVE additional working directories (settings
    /// `additionalDirectories` union CLI `--add-dir`) advertised alongside cwd
    /// on each connected server's `roots/list`. Builder-style so it composes
    /// with [`Self::new`] / [`Self::with_raw_conn`]:
    ///
    /// ```ignore
    /// let roots = mcp::new_shared_roots(vec![PathBuf::from("/tmp/extra")]);
    /// let reg = McpRegistry::with_raw_conn(transport, raw_conn)
    ///     .with_additional_roots(roots);
    /// ```
    ///
    /// Takes the SHARED [`crate::SharedRoots`] cell (not a snapshot) so the
    /// composition root can retain the SAME `Arc` and later push into it via
    /// [`Self::add_root`] to drive a runtime `/add-dir`. An empty cell (the
    /// default) leaves `roots/list` cwd-only (unchanged). Matches claude-code
    /// 2.1.207 `r1d()` = `[cwd, ...additionalWorkingDirectories]`.
    #[must_use]
    pub fn with_additional_roots(mut self, roots: crate::SharedRoots) -> Self {
        self.additional_roots = roots;
        self
    }

    /// Push `dir` into the LIVE additional-roots set (the shared `roots/list`
    /// source) with a jzn-style change-compare: returns `true` when the dir was
    /// newly added, `false` when it was already present (a strict no-op). The
    /// caller only fires [`Self::notify_roots_list_changed_all`] on a `true`
    /// result — matching claude-code, which recomputes the sorted additional-dir
    /// list (`jzn`) and notifies MCP roots ONLY on a real change. Parity 2.1.207
    /// P1-08 runtime `/add-dir`.
    pub fn add_root(&self, dir: std::path::PathBuf) -> bool {
        let mut guard = self
            .additional_roots
            .write()
            .expect("additional_roots lock poisoned");
        if guard.iter().any(|d| d == &dir) {
            return false;
        }
        guard.push(dir);
        true
    }

    /// Snapshot of the LIVE additional-roots set (test / observability).
    #[must_use]
    pub fn additional_roots_snapshot(&self) -> Vec<std::path::PathBuf> {
        self.additional_roots
            .read()
            .expect("additional_roots lock poisoned")
            .clone()
    }

    /// Send `notifications/roots/list_changed` to EVERY connected MCP client,
    /// telling each server the client's working-dir set changed so it should
    /// re-query `roots/list`. A 1:1 port of claude-code's
    /// `notifyMcpRootsListChanged` → `UMy()` fan-out, which calls
    /// `sendRootsListChanged()` on every connected client. Best-effort per
    /// client (a per-client send failure is logged + swallowed inside
    /// [`McpClient::send_roots_list_changed`]). Returns the number of clients
    /// notified. Parity 2.1.207 P1-08.
    pub async fn notify_roots_list_changed_all(&self) -> usize {
        let clients: Vec<Arc<McpClient>> = self
            .clients
            .read()
            .await
            .values()
            .map(|entry| Arc::clone(&entry.client))
            .collect();
        for client in &clients {
            client.send_roots_list_changed();
        }
        clients.len()
    }

    /// Whether the OAuth seam ([`OAuthDeps`]) has been injected via
    /// [`Self::with_oauth`]. `false` leaves OAuth-configured remote servers on
    /// their static-header fallback; `true` enables the interactive
    /// load → refresh → consent flow. Used by the desktop composition-root test
    /// to assert OAuth is production-reachable.
    #[must_use]
    pub fn has_oauth(&self) -> bool {
        self.oauth.is_some()
    }

    /// Whether a Cross-App-Access ([`XaaConfigProvider`]) provider is wired into
    /// the injected [`OAuthDeps`]. `false` (the default, and the state when no
    /// `xaaIdp` settings tier is present) leaves an `oauth.xaa` server on its
    /// actionable hard-fail; `true` means the host can supply the IdP `id_token`
    /// + AS credentials and the XAA token-exchange chain can run. Used by the
    /// desktop composition-root test to assert the XAA seam is reachable when
    /// configured.
    #[must_use]
    pub fn has_xaa(&self) -> bool {
        self.oauth.as_ref().is_some_and(|d| d.xaa_config.is_some())
    }

    /// Cache an `Arc<McpClient>` for `name` (M4-07).
    ///
    /// The platform host calls this after building the client (typically
    /// alongside `connect`). Builtin tools then call [`Self::get_client`]
    /// to dispatch over the wire-locked client surface.
    pub async fn register_client(&self, name: &str, client: Arc<McpClient>) {
        self.clients.write().await.insert(
            name.into(),
            RegisteredClient {
                connection_id: None,
                client,
            },
        );
    }

    async fn register_connected_client(
        &self,
        name: &str,
        connection_id: McpConnectionId,
        client: Arc<McpClient>,
    ) {
        self.clients.write().await.insert(
            name.into(),
            RegisteredClient {
                connection_id: Some(connection_id),
                client,
            },
        );
    }

    /// Return the cached `Arc<McpClient>` for `name`, if any (M4-07).
    ///
    /// Matches by NORMALIZED key: a model-supplied `<server>` token is the
    /// normalized form (`mcp__<normalize(server)>__<tool>`), while clients are
    /// stored under the RAW `config.name` (so `/mcp` shows the raw display
    /// name). This mirrors claude-code's `normalizeNameForMCP(client.name) ===
    /// serverName` lookup (normalization.rs:22-24, client.ts).
    pub async fn get_client(&self, name: &str) -> Option<Arc<McpClient>> {
        self.clients
            .read()
            .await
            .iter()
            .find(|(k, _)| normalize_name_for_mcp(k) == name)
            .map(|(_, entry)| Arc::clone(&entry.client))
    }

    /// Whether `name` can currently accept tool calls.
    ///
    /// Most transports dispatch through a live [`McpClient`]. Same-process
    /// providers are deliberately different: their [`McpTransport`] already
    /// is the invocation boundary, so requiring an otherwise-unused JSON-RPC
    /// connection would turn a successfully discovered `InProcess` server
    /// into an uncallable catalog. Only a connected `InProcess` server may use
    /// this direct path; stdio and network transports remain fail-closed on a
    /// missing client.
    pub async fn has_callable_server(&self, name: &str) -> bool {
        self.get_client(name).await.is_some()
            || self.direct_inprocess_connection(name).await.is_some()
    }

    async fn direct_inprocess_connection(&self, name: &str) -> Option<(McpConnectionId, String)> {
        let connections = self.connections.read().await;
        connections.iter().find_map(|(raw_name, state)| {
            if normalize_name_for_mcp(raw_name) != name {
                return None;
            }
            match state {
                McpConnectionState::Connected {
                    config:
                        McpServerConfig {
                            spec: McpTransportSpec::InProcess { registry_key },
                            ..
                        },
                    connection_id,
                    ..
                } => Some((*connection_id, registry_key.clone())),
                _ => None,
            }
        })
    }

    /// Return the [`McpServerConfig`] for `name`, if any (M4-07).
    ///
    /// Reads the current state-map; returns the config from any variant
    /// that carries one. Matches by NORMALIZED key (see [`Self::get_client`])
    /// so a model-supplied `<server>` token resolves a raw stored key.
    pub async fn get_config(&self, name: &str) -> Option<McpServerConfig> {
        let conns = self.connections.read().await;
        let state = conns
            .iter()
            .find(|(k, _)| normalize_name_for_mcp(k) == name)
            .map(|(_, v)| v)?;
        match state {
            McpConnectionState::Disconnected { config, .. }
            | McpConnectionState::Connecting { config, .. }
            | McpConnectionState::AwaitingOAuth { config, .. }
            | McpConnectionState::Connected { config, .. }
            | McpConnectionState::HealthChecking { config, .. }
            | McpConnectionState::Reconnecting { config, .. }
            | McpConnectionState::Failed { config, .. }
            | McpConnectionState::Stopped { config } => Some(config.clone()),
        }
    }

    /// Call one MCP tool and retry a single authentication failure after a
    /// full reconnect. Reconnect re-runs `headersHelper` and OAuth resolution;
    /// a second 401/403 is returned unchanged and never loops.
    pub async fn call_tool_with_auth_retry(
        &self,
        server: &str,
        full_name: &str,
        input: serde_json::Value,
        tool_use_id: Option<&str>,
        on_progress: Option<crate::client::McpProgressCallback>,
    ) -> Result<traits::McpToolResultDto, crate::client::McpClientError> {
        let Some(client) = self.get_client(server).await else {
            let Some((connection_id, _registry_key)) =
                self.direct_inprocess_connection(server).await
            else {
                return Err(crate::client::McpClientError::Rpc(format!(
                    "MCP server \"{server}\" has no live client"
                )));
            };

            // `full_name` is the model-facing `mcp__<server>__<tool>` name.
            // The caller has already resolved the final segment back to the
            // server's raw tool name, so stripping the fixed prefix is safe and
            // avoids teaching an in-process provider about FQN normalization.
            let prefix = format!("mcp__{server}__");
            let tool_name = full_name.strip_prefix(&prefix).ok_or_else(|| {
                crate::client::McpClientError::Rpc(format!(
                    "invalid MCP tool name {full_name:?} for server {server:?}"
                ))
            })?;
            return self
                .transport
                .call_tool(&McpRawConnection { connection_id }, tool_name, input)
                .await
                .map_err(|error| crate::client::McpClientError::Rpc(error.to_string()));
        };
        let first = client
            .call_tool_with_progress(full_name, input.clone(), tool_use_id, on_progress.clone())
            .await;
        let Err(error) = first else {
            return first;
        };
        if !error.is_auth_response() {
            return Err(error);
        }

        let raw_name = {
            let connections = self.connections.read().await;
            connections
                .keys()
                .find(|name| normalize_name_for_mcp(name) == server)
                .cloned()
        }
        .ok_or_else(|| crate::client::McpClientError::Rpc(error.to_string()))?;
        self.reconnect(&raw_name)
            .await
            .map_err(|retry_error| crate::client::McpClientError::Rpc(retry_error.to_string()))?;
        let refreshed = self.get_client(server).await.ok_or_else(|| {
            crate::client::McpClientError::Rpc(format!(
                "MCP server \"{server}\" did not publish a client after authentication refresh"
            ))
        })?;
        refreshed
            .call_tool_with_progress(full_name, input, tool_use_id, on_progress)
            .await
    }

    /// Test-only helper that registers a config (`Disconnected` state) and
    /// caches a pre-built `Arc<McpClient>` for `name`. Used by M4-07 tools
    /// unit tests to exercise the dispatch paths without spinning up a
    /// real transport.
    #[doc(hidden)]
    pub async fn register_test_client(
        &self,
        name: &str,
        config: McpServerConfig,
        client: Arc<McpClient>,
    ) {
        self.connections.write().await.insert(
            name.into(),
            McpConnectionState::Disconnected {
                config,
                last_error: None,
            },
        );
        self.clients.write().await.insert(
            name.into(),
            RegisteredClient {
                connection_id: None,
                client,
            },
        );
    }

    /// Connect, run `initialize`, and discover the server's catalog.
    ///
    /// Re-uses the existing connection if `config.name` is already in the
    /// `Connected` state.
    pub async fn connect(&self, config: McpServerConfig) -> Result<McpConnectionId, McpError> {
        // `zar()` — refuse an unconfigured remote server BEFORE opening a
        // socket or spawning anything. Oracle @231408727:
        //   if (zar(t)) return {type:"failed", errorCode:"UNCONFIGURED", ...}
        //
        // This guard is what makes it safe for `json_config` to keep
        // blank-url entries: they now reach the listing, and `mcp list`
        // health-probes approved servers, so without it a typo'd config would
        // become a live connect attempt against an empty URL.
        if is_unconfigured_remote(&config.spec) {
            return Err(McpError::Connection(UNCONFIGURED_MESSAGE.to_string()));
        }
        let lifecycle = self.lifecycle_lock(&config.name);
        let _guard = lifecycle.lock().await;
        self.connect_locked(config).await
    }

    async fn connect_locked(&self, config: McpServerConfig) -> Result<McpConnectionId, McpError> {
        let result = self.connect_locked_inner(config.clone()).await;
        if let Err(error) = &result {
            // A failed public connect must never strand the registry in
            // `Connecting`. Reconnect scheduling only considers disconnected
            // states, and `/mcp` should expose the actual last failure.
            self.connections.write().await.insert(
                config.name.clone(),
                McpConnectionState::Disconnected {
                    config,
                    last_error: Some(error.to_string()),
                },
            );
        }
        result
    }

    async fn connect_locked_inner(
        &self,
        config: McpServerConfig,
    ) -> Result<McpConnectionId, McpError> {
        if let Some(McpConnectionState::Connected { connection_id, .. }) =
            self.connections.read().await.get(&config.name)
        {
            return Ok(*connection_id);
        }

        // claude `Nxe`'s two pre-dial gates, in order — neither dials.
        //
        // 1. `zar`: nothing to dial (a blank `url` and no `configError`). The
        //    oracle logs `mcp_connect_skipped` reason "unconfigured" and fails
        //    with `errorCode:"UNCONFIGURED"`; the text is
        //    `configError ?? "No URL configured for this server"`, and the port
        //    reaches this arm only with `configError` absent.
        // 2. A `configError` (e.g. a url that expanded to an empty string) →
        //    `errorCode:"INVALID_CONFIG"`, error text = the configError.
        //
        // The frozen `McpError` carries no error code, so both surface as a
        // `Connection` error holding the oracle's text; callers that need the
        // distinction (the `mcp list`/`mcp get` status) re-derive it from the
        // config via [`McpServerConfig::is_unconfigured`].
        if config.is_unconfigured() {
            return Err(McpError::Connection(
                crate::connection::UNCONFIGURED_ERROR.to_string(),
            ));
        }
        if let Some(err) = &config.config_error {
            emit_server_config_invalid(&config, telemetry::tengu::mcp::ConfigInvalidSource::Loader);
            return Err(McpError::Connection(err.clone()));
        }
        // 3. §18 — the oracle's CONNECT-TIME `new URL(t.url)` re-check, run in
        //    the same block as gate 2 (`Ve` @182283839 / `Ae` @182488092:
        //    `let C=t.configError; if(!C&&"url" in t) try{new URL(t.url)}
        //    catch{C="'url' is not a valid URL. ..."}; if(C) return ...`).
        //    `configError` wins (gate 2 already returned), so this only fires
        //    for a url the LOADER never flagged: present, non-blank, and
        //    unparseable (a bare hostname with no scheme, say). Same
        //    `errorCode:"INVALID_CONFIG"` as gate 2; also does not dial.
        if let Some(err) = config.connect_time_url_error() {
            emit_server_config_invalid(
                &config,
                telemetry::tengu::mcp::ConfigInvalidSource::Connect,
            );
            return Err(McpError::Connection(err.to_string()));
        }

        self.connections.write().await.insert(
            config.name.clone(),
            McpConnectionState::Connecting {
                config: config.clone(),
                started_at: SystemTime::now(),
            },
        );

        // MCPLIFE.4: bound the connect + initialize handshake by MCP_TIMEOUT
        // (default 30s), mirroring claude-code's getConnectionTimeoutMs() race
        // (`services/mcp/client.ts:456-458` + the `Promise.race` at 1020-1080).
        // The transport connect (SSE GET / process spawn) is otherwise unbounded;
        // map an elapsed deadline to a `Connection` error (the frozen `McpError`
        // has no connect-timeout variant — `Timeout` is tool-call-specific).
        let connect_timeout = mcp_connection_timeout();

        // OAuth (SSE/HTTP with an `oauth` block + wired `OAuthDeps`): resolve a
        // Bearer token and inject it into the spec headers before connecting.
        // Returns `(augmented_spec, server_key)` so a 401 can drive a refresh +
        // retry. Static-token servers (and any server when `oauth` is unwired)
        // resolve to the spec unchanged with no server key.
        //
        // §19: a static `headers.Authorization` (`has_user_auth_header`) or a
        // headers-helper-minted one (`helper_minted_authorization`) is
        // AUTHORITATIVE over OAuth — oracle `hasUserAuthHeader` /
        // `helperMintsAuthHeader`, checked (in that order) BEFORE any OAuth
        // provider is constructed, so its Bearer can never be injected at
        // all, let alone overwrite the value. `classify_auth_failure` reports
        // a subsequent auth-type failure with the oracle's exact
        // `AUTH_HEADER_REJECTED` / `HEADERS_HELPER_AUTH_REJECTED` copy instead
        // of the raw transport error; it is a no-op pass-through whenever
        // neither flag applies, so it is safe to wrap every exit below.
        let has_user_auth_header = crate::negotiation::spec_has_authorization(&config.spec);
        let helper_enabled = crate::headers_helper::has_headers_helper(&config.spec);
        let mut resolved_config = config.clone();
        let plugin_root = self
            .headers_helper_plugin_roots
            .read()
            .await
            .get(&config.name)
            .cloned();
        let (helper_spec, mut helper_minted_authorization) =
            crate::headers_helper::resolve_headers_helper_in(
                &config,
                &self.headers_helper_cwd,
                plugin_root.as_deref(),
            )
            .await?;
        resolved_config.spec = helper_spec;
        let (connect_spec, oauth_key) = if has_user_auth_header || helper_minted_authorization {
            (resolved_config.spec.clone(), None)
        } else {
            self.resolve_oauth_spec(&resolved_config).await?
        };

        // §17: resolve the MCP protocol-era negotiation mode for this
        // connect attempt (oracle `co(Wr(t.type,...), t, cn(t))`), logging
        // its two possible `warn`-level messages exactly as the oracle does.
        // The resolved mode is not yet consumed to change the connect
        // timeout or the `initialize` wire frame — that requires the
        // `server/discover` era-probe sub-protocol and its pinned-legacy
        // reconnect ladder, deferred (see `protocol_negotiation` module
        // docs). Every label reachable in this port resolves to `Legacy`
        // without an explicit `MCP_PROTOCOL_NEGOTIATION=auto` AND a wired
        // feature-flag fetcher (neither exists by default), so today's
        // connect flow is already byte-exact with the legacy path.
        let negotiation_mode = crate::protocol_negotiation::resolve_for_spec(
            &connect_spec,
            connect_timeout.as_millis() as u64,
        );
        tracing::debug!(
            server = %config.name,
            mode = ?negotiation_mode,
            "MCP protocol-era negotiation resolved"
        );

        let attempt =
            |spec: McpTransportSpec| self.connect_attempt(spec, connect_timeout, &config.name);

        let (conn, caps) = match attempt(connect_spec.clone()).await {
            Ok(pair) => pair,
            // 403 `insufficient_scope` for an OAuth server → step-up: the AS
            // requires an elevated scope (RFC 6750). Re-run the interactive flow
            // requesting that scope (RFC 6749 §6 forbids scope elevation via
            // refresh, so we MUST do a fresh PKCE flow), re-inject the Bearer,
            // and retry ONCE. Mirrors auth.ts `wrapFetchWithStepUpDetection`
            // (1354-1374) + `markStepUpPending`/`cachedStepUpScope` persistence.
            // Checked BEFORE the 401 branch so a 403 never falls into refresh.
            //
            // Unreachable when `has_user_auth_header || helper_minted_authorization`:
            // `oauth_key` is `None` in that case (OAuth was never constructed
            // above), so `classify_auth_failure` in this arm is always a
            // pass-through.
            Err(e) if oauth_key.is_some() => {
                // §24c: a live `WWW-Authenticate` challenge on THIS failure may
                // name where the server's RFC 9728 Protected Resource Metadata
                // actually lives (`resource_metadata`); when present it is
                // threaded into the re-auth's discovery instead of the
                // well-known guess (oracle `Be`/`Hxt`). `None` for any error
                // shape that doesn't carry a structured `WWW-Authenticate`
                // (the string-flattened fallback path).
                let resource_metadata_url = error_resource_metadata_url(&e);
                if let Some(scope) = error_is_403_insufficient_scope(&e) {
                    let stepped = self
                        .step_up_oauth_spec(
                            &resolved_config,
                            &scope,
                            resource_metadata_url.as_deref(),
                        )
                        .await?;
                    attempt(stepped).await.map_err(|e| {
                        crate::negotiation::classify_auth_failure(
                            e,
                            has_user_auth_header,
                            helper_minted_authorization,
                        )
                    })?
                } else if error_is_401(&e) {
                    // 401 → the access token is stale: force a refresh (or a
                    // fresh interactive flow), re-inject the Bearer, retry ONCE.
                    // Faithful-core 401 detection: the transport flattens errors
                    // to strings (structured status is a noted residual).
                    let refreshed = self
                        .reauth_oauth_spec(&resolved_config, resource_metadata_url.as_deref())
                        .await?;
                    attempt(refreshed).await.map_err(|e| {
                        crate::negotiation::classify_auth_failure(
                            e,
                            has_user_auth_header,
                            helper_minted_authorization,
                        )
                    })?
                } else {
                    return Err(crate::negotiation::classify_auth_failure(
                        e,
                        has_user_auth_header,
                        helper_minted_authorization,
                    ));
                }
            }
            Err(e) if helper_enabled && error_is_auth_response(&e) => {
                // A helper may emit short-lived credentials. Re-run it for one
                // auth failure and reconnect once; never loop indefinitely.
                // Re-derive `helper_minted_authorization` from THIS rerun (it
                // can legitimately change run to run) so a still-failing retry
                // classifies against what the helper minted this time.
                let (refreshed, minted) = crate::headers_helper::resolve_headers_helper_in(
                    &config,
                    &self.headers_helper_cwd,
                    plugin_root.as_deref(),
                )
                .await?;
                helper_minted_authorization = minted;
                attempt(refreshed).await.map_err(|e| {
                    crate::negotiation::classify_auth_failure(
                        e,
                        has_user_auth_header,
                        helper_minted_authorization,
                    )
                })?
            }
            Err(e) => {
                return Err(crate::negotiation::classify_auth_failure(
                    e,
                    has_user_auth_header,
                    helper_minted_authorization,
                ))
            }
        };
        // §20b — `tengu_mcp_tools_listed`'s `listDurationMs:Date.now()-o`.
        // Oracle `yt` (@182326900) opens the timer immediately BEFORE the
        // `tools/list` round-trip and `yn` reads it immediately after, with
        // no other RPC inside the window: `let d=Date.now(), …,
        // h=await …"tools/list"…, _=yn(e,h,d,"live",r)`. Timing the whole
        // catalog block instead (tools + resources + prompts, as an earlier
        // revision did) turned this into a multiple-x overstatement of the
        // operation the field is named for — a server answering `tools/list`
        // in 40 ms but `resources/list`/`prompts/list` in 300 ms each
        // reported ~640 ms. Stop the clock where the oracle stops it.
        let mut tools_list_elapsed = std::time::Duration::ZERO;
        let catalog = async {
            let tools = if caps.tools {
                let started = std::time::Instant::now();
                let listed = self.transport.list_tools(&conn).await?;
                tools_list_elapsed = started.elapsed();
                listed
            } else {
                Vec::new()
            };
            let resources = if caps.resources {
                self.transport.list_resources(&conn).await?
            } else {
                Vec::new()
            };
            // §26a — `resources/templates/list` is gated on the SAME
            // `resources` capability as `resources/list` / `resources/read`
            // (oracle: `case"resources/list":case"resources/templates/list":
            // case"resources/read":if(!this._capabilities.resources)throw…`,
            // @167690139), not a separate capability bit.
            //
            // The fetch is NON-FATAL. Templates are optional in the MCP spec:
            // a server may declare `capabilities.resources` on the strength of
            // `resources/list` alone and answer `-32601 Method not found` here
            // (the posix transport flattens that to `McpError::Internal` via
            // `map_call_err`). Oracle `Qe` (2.1.251 Mach-O @182528544) wraps
            // the whole fetch in a catch that returns `[]` on EVERY error —
            // `catch(t){ mr().resourceTemplateLists.delete(ur(e.name,e.config));
            // Z(e.name,`Failed to fetch resource templates: ${l(t)}`); let r=[];
            // if(!(t instanceof Er&&t.code===Ir.MethodNotFound))
            // qt().discoveryFetchErrors.set(r,we(t)); return r }` — so it can
            // never fail a connection. Propagating it with `?` instead
            // disconnected the live transport below and dropped ALL of the
            // server's tools/resources/prompts.
            let resource_templates = if caps.resources {
                match self.transport.list_resource_templates(&conn).await {
                    Ok(templates) => templates,
                    Err(error) => {
                        tracing::warn!(
                            server = %config.name,
                            %error,
                            "Failed to fetch resource templates"
                        );
                        Vec::new()
                    }
                }
            } else {
                Vec::new()
            };
            let prompts = if caps.prompts {
                self.transport.list_prompts(&conn).await?
            } else {
                Vec::new()
            };
            Ok::<_, McpError>((tools, resources, resource_templates, prompts))
        }
        .await;
        let (mut tools, resources, resource_templates, prompts) = match catalog {
            Ok(catalog) => catalog,
            Err(error) => {
                // `initialize` succeeded, so the transport is live. A catalog
                // failure must retire it before the state falls back to
                // Disconnected; otherwise stdio children/sockets leak.
                let _ = self.transport.disconnect(conn.connection_id).await;
                return Err(error);
            }
        };

        // Rewrite the empty `<server>` token the transport emits (it has no
        // logical server name, only an `McpConnectionId`). This is the missing
        // "rewrite site" the posix `list_tools` comment defers to: stamp the
        // RAW `config.name` into `server_name` and build the normalized FQN
        // `mcp__<normalize(server)>__<normalize(tool)>`. Mirrors claude-code's
        // `buildMcpToolName(client.name, tool.name)` (client.ts:1768 →
        // `mcpStringUtils.ts:51` `getMcpPrefix(server) + normalizeNameForMCP(tool)`),
        // which normalizes BOTH segments with the 1:1 `normalizeNameForMCP`
        // (normalization.rs). The RAW `tool_name` stays on the dto for dispatch
        // (the server expects the unnormalized wire name, recovered via
        // [`McpRegistry::resolve_wire_tool_name`] — claude-code carries it as
        // `mcpInfo.toolName`, client.ts:1774). For valid-identifier tool names
        // the normalized form equals the raw one, so the FQN is byte-unchanged
        // except for names with characters outside `[a-zA-Z0-9_-]`.
        let normalized_server = normalize_name_for_mcp(&config.name);
        // §20a — normalize or drop each tool's `inputSchema` before it reaches
        // the model (oracle `Wrt`/`qrt`, see [`crate::tool_schema`]). This is
        // the CONNECT path: `McpClient::list_tools` runs the same decision but
        // is reached only by `refresh_catalog`, so without this the transform
        // never applied to the tool list `build_registered_mcp_tools` actually
        // hands the model. The per-server gate reads the SAME resolved URL the
        // client is given below via `with_server_url`.
        let gate_url = {
            let u = spec_url(&config.spec);
            (!u.is_empty()).then(|| u.to_string())
        };
        let server_display = config.name.clone();
        // §20b — `yn`'s seven per-server tool-schema-classification counters
        // (see `telemetry::tengu::mcp::DegradedReason`'s doc for the full
        // `x`/`W`/`ue`/`_e`/`xe`/`F`/`X` trace). Tallied across the WHOLE
        // tool list, then one `tengu_mcp_degraded` fires per nonzero bucket
        // AFTER the loop — the oracle does not fire one event per tool.
        let mut degraded_counts: std::collections::HashMap<
            telemetry::tengu::mcp::DegradedReason,
            u32,
        > = std::collections::HashMap::new();
        // Oracle `yn`'s FIRST statement (@182316780), 20 lines above the
        // seven counters below: `if(u.length===0&&r==="live")
        // s("tengu_mcp_degraded",{reason:w("connected_zero_tools"),…})`.
        // `u` is the RAW `tools/list` response, so this is measured BEFORE
        // the §20a filter runs — a server whose every tool the filter dropped
        // reports its drop reason, not zero-tools. `r==="live"` holds because
        // this is the connect path (a fresh dial); the cached-row adoption
        // path the oracle also feeds `yn` from does not exist in this port.
        // Gated on `caps.tools` for the same reason `tools_listed` is: with
        // no tools capability the oracle never reaches `yn` at all, so an
        // empty list there is not a degraded signal.
        if connected_zero_tools_fires(caps.tools, tools.len()) {
            degraded_counts.insert(telemetry::tengu::mcp::DegradedReason::ConnectedZeroTools, 1);
        }
        tools.retain_mut(|dto| {
            let decision = crate::tool_schema::decide_tool_schema(
                gate_url.as_deref(),
                &dto.input_schema,
            );
            // The oracle's `x++` and its validity counters are INDEPENDENT
            // (the `x++` arm falls through into the validity check), so a
            // normalized-then-invalid tool increments TWO buckets and fires
            // TWO events. See `tool_schema::ToolSchemaDecision::normalized`.
            if decision.normalized {
                *degraded_counts
                    .entry(telemetry::tengu::mcp::DegradedReason::ToolSchemaNormalized)
                    .or_insert(0) += 1;
            }
            if let Some(reason) = decision.classification {
                *degraded_counts.entry(reason).or_insert(0) += 1;
            }
            if let Some(reason) = decision.drop_reason {
                tracing::warn!(
                    server = %server_display,
                    tool = %dto.tool_name,
                    "Skipping tool \"{}\": {reason}. Other tools from this server remain available.",
                    dto.tool_name
                );
                return false;
            }
            if let Some(warning) = &decision.warning {
                tracing::debug!(
                    server = %server_display,
                    tool = %dto.tool_name,
                    "Tool \"{}\" {warning}",
                    dto.tool_name
                );
            }
            if let Some(note) = decision.description_note {
                // oracle: `E.description ? `${note}\n\n${description}` : note`.
                dto.description = if dto.description.is_empty() {
                    note
                } else {
                    format!("{note}\n\n{}", dto.description)
                };
            }
            dto.input_schema = decision.schema;
            dto.server_name.clone_from(&server_display);
            dto.full_name = format!(
                "mcp__{}__{}",
                normalized_server,
                normalize_name_for_mcp(&dto.tool_name)
            );
            true
        });

        // `tengu_mcp_tools_listed` — once per successful `tools/list`, not
        // when the server had no `tools` capability at all (oracle only
        // reaches this call site from inside the tools-listing branch).
        if caps.tools {
            telemetry::emit_mcp_tools_listed(&tools_listed_payload(
                config.spec.kind(),
                tools_list_elapsed,
                &tools,
                &server_display,
            ));
        }

        // Fire one `tengu_mcp_degraded` per nonzero classification bucket.
        // oracle: `c(e.config.type??"stdio")` — the RAW config `type`
        // string, matching `McpTransportSpec::kind()` (NOT the `Wr`-mapped
        // `ide`/`sdk-control` labels `protocol_negotiation.rs` uses
        // elsewhere) — see `server_key`'s doc for why `kind()` itself must
        // never change shape; this only READS it. The payload-BUILDING step
        // is a pure, non-async, non-tracing function so the aggregation
        // logic is unit-testable without a tracing-capture race against the
        // other `#[tokio::test]`s sharing this binary (a real `tracing`
        // pitfall: `subscriber::set_default` is thread-local, but callsite
        // `Interest` caching is process-global, so a concurrently-running
        // test's subscriber can race the cache and silently starve this
        // one's events under `cargo test`'s default parallelism).
        for payload in
            degraded_payloads_for_server(&degraded_counts, config.spec.kind(), &server_display)
        {
            telemetry::emit_mcp_degraded(&payload);
        }

        let connection_id = conn.connection_id;
        let server_name = config.name.clone();
        // Capture the per-server config options before `config` is moved into
        // the `Connected` state below — threaded into the `McpClient` further
        // down (parity 2.1.207 P2-01).
        let config_timeout_ms = config.timeout_ms;
        let config_always_load = config.always_load;
        // Transport kind feeds the `GLd` idle-timeout default (stdio 30 min /
        // remote 5 min / in-process none) on the built `McpClient`.
        let config_transport_kind = config.spec.transport_kind();
        // Resolved endpoint URL (`None` for stdio/url-less transports) — feeds
        // the §20a per-server gate on the `McpClient` refresh path.
        let config_server_url = gate_url.clone();
        self.connections.write().await.insert(
            server_name.clone(),
            McpConnectionState::Connected {
                config,
                connection_id,
                capabilities: caps,
                tools,
                resources,
                resource_templates,
                prompts,
                connected_at: SystemTime::now(),
            },
        );

        // Bridge the transport's live `jsonrpc::Connection` into an `McpClient`
        // so the 4 builtin MCP tools dispatch over the wire-locked client
        // surface. The `initialize` handshake was ALREADY performed by
        // `transport.initialize` above, so we do NOT re-run `McpClient::initialize`
        // (that would double-handshake) — the `McpClient`'s own
        // `server_capabilities` cache stays unpopulated, and tools rely on the
        // transport-discovered caps stored in the `Connected` state. Mirrors
        // claude-code's `ensureConnectedClient` returning a live client
        // (client.ts:1688-1709). Registered under the RAW `config.name` so
        // `/mcp` display stays raw; `get_client` matches by normalized key.
        if let Some(raw_conn) = &self.raw_conn {
            if let Some(connection) = raw_conn.connection_for(connection_id) {
                self.spawn_catalog_change_listener(
                    server_name.clone(),
                    connection_id,
                    connection.clone(),
                );
                let cwd = std::env::current_dir().unwrap_or_default();
                // Forward the optional hook dispatcher so this server's
                // `elicitation/create` handler can fire the `Elicitation` hook.
                // `None` => default `{"action":"cancel"}` (unchanged). Also
                // advertise the session's additional working dirs (settings
                // `additionalDirectories` + `--add-dir`) on `roots/list`, so a
                // connected server sees the full working-dir set (parity 2.1.207
                // r1d() = [cwd, ...additionalWorkingDirectories]).
                let client = Arc::new(
                    McpClient::with_roots(
                        server_name.clone(),
                        cwd,
                        self.additional_roots.clone(),
                        connection,
                        self.hook_dispatcher.clone(),
                    )
                    .await
                    // Carry the resolved per-server config `timeout` (folded
                    // with `request_timeout_ms`) into the BHs per-call resolver,
                    // and the server-level `alwaysLoad` flag into each listed
                    // tool's `always_load` bit (parity 2.1.207 P2-01).
                    .with_config_options(config_timeout_ms, config_always_load)
                    // Transport kind → `GLd` idle-timeout default (parity 2.1.207
                    // P2-01 remainder).
                    .with_transport_kind(config_transport_kind)
                    // §20a — the connected server's URL feeds the per-server
                    // schema-normalization gate on `refresh_catalog`'s
                    // `list_tools`, exactly as `gate_url` above feeds the
                    // connect path. Without it the gate can only ever match a
                    // bare `"*"` entry.
                    .with_server_url(config_server_url),
                );
                self.register_connected_client(&server_name, connection_id, client)
                    .await;
            }
        }

        // Drive the engine's already-Arc-wrapped ToolRegistry for both startup
        // and reconnects. The startup event is an idempotent replacement; a
        // reconnect needs it because its new connection id did not exist in the
        // boot-time MCP partition.
        let _ = self.catalog_changes.send(McpCatalogChanged {
            server_name,
            connection_id,
            retired_connection_id: None,
            kind: McpCatalogKind::Tools,
        });

        Ok(connection_id)
    }

    /// Connect and initialize under one deadline. If initialization fails or
    /// times out after a transport was opened, retire that transport before
    /// returning so callers never leak a live stdio child/socket.
    async fn connect_attempt(
        &self,
        spec: McpTransportSpec,
        timeout: Duration,
        server_name: &str,
    ) -> Result<(McpRawConnection, ServerCapabilitiesDto), McpError> {
        let deadline = tokio::time::Instant::now() + timeout;
        let timeout_error = || {
            McpError::Connection(format!(
                "MCP server \"{server_name}\" connection timed out after {}ms",
                timeout.as_millis()
            ))
        };
        let conn = tokio::time::timeout_at(deadline, self.transport.connect(&spec))
            .await
            .map_err(|_| timeout_error())??;
        match tokio::time::timeout_at(deadline, self.transport.initialize(&conn)).await {
            Ok(Ok(capabilities)) => Ok((conn, capabilities)),
            Ok(Err(error)) => {
                let _ = self.transport.disconnect(conn.connection_id).await;
                Err(error)
            }
            Err(_) => {
                let _ = self.transport.disconnect(conn.connection_id).await;
                Err(timeout_error())
            }
        }
    }

    /// Resolve the spec to connect with, attaching a Bearer token for OAuth
    /// servers. Returns `(spec, Some(server_key))` for an OAuth-configured
    /// SSE/HTTP server (token loaded → refreshed-on-expiry → freshly minted via
    /// the interactive flow), or `(config.spec.clone(), None)` for static-token
    /// servers and any server when the OAuth seam is unwired.
    ///
    /// Mirrors claude-code's per-connect token resolution (`auth.ts` `tokens()`
    /// + `useManageMCPConnections` attaching the Authorization header).
    async fn resolve_oauth_spec(
        &self,
        config: &McpServerConfig,
    ) -> Result<(McpTransportSpec, Option<String>), McpError> {
        let Some(deps) = &self.oauth else {
            return Ok((config.spec.clone(), None));
        };
        let Some(oauth_cfg) = spec_oauth(&config.spec) else {
            return Ok((config.spec.clone(), None));
        };
        let key = oauth::server_key(&config.name, &config.spec);

        // XAA (cross-app-access, SEP-990): when `oauth.xaa` is set, XAA is the
        // ONLY auth path — never fall through to the consent flow (auth.ts:857-
        // 900). Gated by `LINGXI_ENABLE_XAA` (mirror of CLAUDE_CODE_ENABLE_XAA);
        // a flagged server with the env unset hard-fails with actionable copy.
        if oauth_cfg.xaa == Some(true) {
            let token = self.resolve_xaa_token(config, &key, deps).await?;
            return Ok((
                inject_bearer(&config.spec, token.access_token.expose_secret()),
                Some(key),
            ));
        }

        // 1. Stored token, unexpired → use it.
        // 2. Stored token, expired with a refresh token → refresh, persist.
        // 3. No usable token → run the interactive flow, persist.
        let token = match oauth::load_tokens(&deps.storage, &key).await? {
            // Unexpired stored token → use it directly.
            Some(stored) if deps.clock.now() < stored.expires_at() => stored.into_tokens(),
            // Expired stored token.
            Some(stored) => {
                match stored.refresh_token.clone() {
                    Some(refresh) => {
                        // Proactive (pre-connect) refresh: no live challenge
                        // exists yet, so discovery uses the well-known guess.
                        let meta = oauth::discover_auth_server_metadata(
                            &deps.http,
                            spec_url(&config.spec),
                            oauth_cfg.auth_server_metadata_url.as_deref(),
                            None,
                        )
                        .await?;
                        // Prefer the client_id the stored tokens were minted
                        // with (DCR-issued OR configured) so silent refresh
                        // re-sends it; fall back to the configured id, then ""
                        // (auth.ts clientInformation(), 1482-1506).
                        let client_id = stored
                            .client_id
                            .clone()
                            .or_else(|| oauth_cfg.client_id.clone())
                            .unwrap_or_default();
                        let refreshed = oauth::refresh_tokens(
                            &deps.http,
                            &deps.clock,
                            &meta,
                            &client_id,
                            None, // public client — no confidential secret to send
                            &refresh,
                        )
                        .await?;
                        oauth::save_tokens(&deps.storage, &deps.clock, &key, &refreshed).await?;
                        refreshed
                    }
                    None => {
                        self.run_interactive_oauth(config, oauth_cfg, &key, deps, None, None)
                            .await?
                    }
                }
            }
            None => {
                self.run_interactive_oauth(config, oauth_cfg, &key, deps, None, None)
                    .await?
            }
        };

        Ok((
            inject_bearer(&config.spec, token.access_token.expose_secret()),
            Some(key),
        ))
    }

    /// Re-authenticate after a 401: refresh if a refresh token is stored, else
    /// run a fresh interactive flow, then return the spec with the new Bearer.
    /// `resource_metadata_url` (§24c) is the `resource_metadata` param parsed
    /// from the triggering 401's `WWW-Authenticate` challenge, when it carried
    /// a structured one — threaded into discovery in place of the well-known
    /// guess.
    async fn reauth_oauth_spec(
        &self,
        config: &McpServerConfig,
        resource_metadata_url: Option<&str>,
    ) -> Result<McpTransportSpec, McpError> {
        let deps = self
            .oauth
            .as_ref()
            .ok_or_else(|| McpError::OAuth("oauth seam not wired".into()))?;
        let oauth_cfg = spec_oauth(&config.spec)
            .ok_or_else(|| McpError::OAuth("server has no oauth config".into()))?;
        let key = oauth::server_key(&config.name, &config.spec);

        // §26b delta 3: an XAA-flagged server's 401 must stay on the XAA
        // path — never fall through to the refresh-or-interactive-consent
        // logic below, matching this module's own guarantee (see
        // `resolve_oauth_spec`) that XAA is the ONLY auth path. The server
        // just rejected whatever was cached, so `force_fresh` skips reusing
        // it: a stored refresh token drives the ordinary refresh grant; its
        // absence (or rejection) drives a fresh silent IdP+AS exchange.
        // Interactive consent is never reachable from this arm.
        if oauth_cfg.xaa == Some(true) {
            let token = self
                .resolve_xaa_token_inner(config, &key, deps, true, resource_metadata_url)
                .await?;
            return Ok(inject_bearer(
                &config.spec,
                token.access_token.expose_secret(),
            ));
        }

        let stored = oauth::load_tokens(&deps.storage, &key).await?;
        let token = match stored.as_ref().and_then(|t| t.refresh_token.clone()) {
            Some(refresh) => {
                let meta = oauth::discover_auth_server_metadata(
                    &deps.http,
                    spec_url(&config.spec),
                    oauth_cfg.auth_server_metadata_url.as_deref(),
                    resource_metadata_url,
                )
                .await?;
                // Prefer the persisted (DCR-issued or configured) client_id so
                // refresh re-sends it (auth.ts clientInformation(), 1482-1506).
                let client_id = stored
                    .as_ref()
                    .and_then(|t| t.client_id.clone())
                    .or_else(|| oauth_cfg.client_id.clone())
                    .unwrap_or_default();
                match oauth::refresh_tokens(
                    &deps.http,
                    &deps.clock,
                    &meta,
                    &client_id,
                    None, // public client — no confidential secret to send
                    &refresh,
                )
                .await
                {
                    Ok(t) => {
                        oauth::save_tokens(&deps.storage, &deps.clock, &key, &t).await?;
                        t
                    }
                    // Refresh token rejected → fall back to a fresh flow.
                    Err(oauth::OAuthError::RefreshRejected(_)) => {
                        self.run_interactive_oauth(
                            config,
                            oauth_cfg,
                            &key,
                            deps,
                            None,
                            resource_metadata_url,
                        )
                        .await?
                    }
                    Err(e) => return Err(e.into()),
                }
            }
            None => {
                self.run_interactive_oauth(
                    config,
                    oauth_cfg,
                    &key,
                    deps,
                    None,
                    resource_metadata_url,
                )
                .await?
            }
        };

        Ok(inject_bearer(
            &config.spec,
            token.access_token.expose_secret(),
        ))
    }

    /// Step-up re-auth after a 403 `insufficient_scope`: persist the required
    /// `scope` onto the stored entry (auth.ts `markStepUpPending`/`stepUpScope`,
    /// 1896), then run a fresh interactive flow requesting that elevated scope
    /// and return the spec with the new Bearer. A refresh CANNOT elevate scope
    /// (RFC 6749 §6), so this always drives the PKCE flow. `resource_metadata_url`
    /// (§24c) is the `resource_metadata` param parsed from the SAME 403
    /// challenge that carried the elevated `scope`, when present (oracle
    /// `_stepUpAuthorize`: `if(e.resourceMetadataUrl)this._resourceMetadataUrl=
    /// e.resourceMetadataUrl`).
    async fn step_up_oauth_spec(
        &self,
        config: &McpServerConfig,
        scope: &str,
        resource_metadata_url: Option<&str>,
    ) -> Result<McpTransportSpec, McpError> {
        let deps = self
            .oauth
            .as_ref()
            .ok_or_else(|| McpError::OAuth("oauth seam not wired".into()))?;
        let oauth_cfg = spec_oauth(&config.spec)
            .ok_or_else(|| McpError::OAuth("server has no oauth config".into()))?;
        let key = oauth::server_key(&config.name, &config.spec);

        // §26b delta 3, second arm: an XAA-flagged server's 403 must stay on
        // the XAA path exactly as its 401 does (`reauth_oauth_spec` above).
        // Oracle `pEr` — the ONE entry point to the consent flow — opens with
        // `if(t.oauth?.xaa){ ...await gt(...); return }` (@182200767), so no
        // step-up, cached `stepUpScope`, or elevated-scope request can ever
        // reach `redirectToAuthorization` for an XAA server. Without this
        // guard the 403 branch WINS the race (it is evaluated before the 401
        // branch in `connect_locked_inner`) and binds a loopback listener +
        // opens a browser on an enterprise deployment that has no interactive
        // consent surface at all. The scope persist below is skipped for the
        // same reason: the oracle only writes `stepUpScope` from inside
        // `redirectToAuthorization`, which XAA never reaches.
        if oauth_cfg.xaa == Some(true) {
            let token = self
                .resolve_xaa_token_inner(config, &key, deps, true, resource_metadata_url)
                .await?;
            return Ok(inject_bearer(
                &config.spec,
                token.access_token.expose_secret(),
            ));
        }

        // Persist the elevated scope so it survives even if the interactive flow
        // is interrupted and resumed later (auth.ts caches it on the stored
        // entry). Best-effort: a storage failure must not block the step-up.
        if let Ok(Some(mut stored)) = oauth::load_tokens(&deps.storage, &key).await {
            stored.step_up_scope = Some(scope.to_string());
            let _ = oauth::store_tokens(&deps.storage, &deps.clock, &key, &stored).await;
        }

        let token = self
            .run_interactive_oauth(
                config,
                oauth_cfg,
                &key,
                deps,
                Some(scope),
                resource_metadata_url,
            )
            .await?;
        Ok(inject_bearer(
            &config.spec,
            token.access_token.expose_secret(),
        ))
    }

    /// Resolve an access token for an XAA-flagged server (auth.ts
    /// `performMCPXaaAuth`, 664-845). Thin wrapper over
    /// [`Self::resolve_xaa_token_inner`] for the per-connect (non-401) resolve
    /// path, which may reuse a cached token.
    async fn resolve_xaa_token(
        &self,
        config: &McpServerConfig,
        key: &str,
        deps: &OAuthDeps,
    ) -> Result<oauth::Tokens, McpError> {
        self.resolve_xaa_token_inner(config, key, deps, false, None)
            .await
    }

    /// Core of the XAA resolve chain (oracle `tokens()`, @182213696), shared by
    /// the per-connect resolve ([`Self::resolve_xaa_token`], `force_fresh:
    /// false` — may reuse a cached token) and the post-401 re-auth
    /// ([`Self::reauth_oauth_spec`]'s xaa arm, `force_fresh: true` — the
    /// server just rejected whatever is cached, so the cache-hit branches
    /// below are skipped and a refresh/exchange is always attempted).
    /// `resource_metadata_url` (§24c) is the live 401 challenge's
    /// `resource_metadata` param, when any (`None` from the per-connect
    /// wrapper, which has no live challenge to read).
    ///
    /// Gating (auth.ts:871-876): `LINGXI_ENABLE_XAA` must be truthy or this
    /// hard-fails with actionable copy. Single-flight (§26b delta 2, oracle
    /// `_refreshInProgress`): callers for the SAME `key` serialize on
    /// [`Self::xaa_refresh_lock`], so at most one refresh/exchange runs at a
    /// time; a resolver that has to wait re-reads storage once it acquires the
    /// lock and reuses whatever the winner just persisted instead of
    /// re-exchanging.
    ///
    /// §26b delta 4: once a refresh token is on file, it ALWAYS takes the
    /// ordinary refresh route (`oauth::refresh_tokens`, whose confidential-
    /// client auth method is chosen from the AS's advertised
    /// `token_endpoint_auth_methods_supported`) — the full IdP+AS exchange
    /// ([`crate::xaa::perform_cross_app_access`]) is reserved for "no refresh
    /// token stored" (never had one, or the AS just rejected it, which falls
    /// through rather than opening an interactive flow — XAA is never
    /// interactive). §26b delta 1: absent a refresh token, the exchange is
    /// silent-triggered only when the access token is missing or expires
    /// within 300s (oracle `!n?.refreshToken && (!n?.accessToken ||
    /// (n.expiresAt-Date.now())/1000<=300)`); otherwise the cached access
    /// token is reused as-is. That same 300s window ALSO bounds reuse on the
    /// refresh-token arm (oracle `tokens()`'s `r<=300&&n.refreshToken`
    /// proactive refresh, which runs for every server after the XAA block).
    /// `force_fresh` skips both cache-hit checks.
    async fn resolve_xaa_token_inner(
        &self,
        config: &McpServerConfig,
        key: &str,
        deps: &OAuthDeps,
        force_fresh: bool,
        resource_metadata_url: Option<&str>,
    ) -> Result<oauth::Tokens, McpError> {
        // Gate on the enable flag (mirror of CLAUDE_CODE_ENABLE_XAA).
        if !traits::env::is_env_truthy(std::env::var("LINGXI_ENABLE_XAA").ok().as_deref()) {
            return Err(McpError::OAuth(format!(
                "XAA is not enabled (set LINGXI_ENABLE_XAA=1). Remove 'xaa' from \
                 server '{}' to use the standard consent flow.",
                config.name
            )));
        }

        // Single-flight: serialize concurrent resolves for this server key.
        let lock = self.xaa_refresh_lock(key);
        let _single_flight = lock.lock().await;

        let stored = oauth::load_tokens(&deps.storage, key).await?;

        match stored {
            Some(s) => {
                if let Some(refresh) = s.refresh_token.clone() {
                    // Delta 4: a refresh token on file takes the ordinary
                    // refresh route — never the full exchange below.
                    //
                    // Reuse is bounded by the SAME 300s proactive window the
                    // no-refresh-token arm below uses: oracle `tokens()`
                    // (@182213696) runs `if(r!=null&&r<=300&&n.refreshToken
                    // &&!d){...refreshAuthorization(n.refreshToken)...}`
                    // AFTER the XAA block, so a stored refresh token does not
                    // exempt a token from proactive refresh — it is what
                    // makes proactive refresh possible. Reusing until hard
                    // expiry instead hands the transport a token seconds from
                    // death, buying an avoidable 401 + reauth round-trip (or
                    // a connect failure if it lapses mid-handshake).
                    let expiring_soon = s
                        .expires_at()
                        .duration_since(deps.clock.now())
                        .map(|remaining| remaining <= Duration::from_secs(300))
                        .unwrap_or(true);
                    if !force_fresh && !expiring_soon {
                        return Ok(s.into_tokens());
                    }
                    let meta = oauth::discover_auth_server_metadata(
                        &deps.http,
                        spec_url(&config.spec),
                        None,
                        resource_metadata_url,
                    )
                    .await?;
                    let client_id = s.client_id.clone().unwrap_or_default();
                    let client_secret = s.client_secret.clone();
                    match oauth::refresh_tokens(
                        &deps.http,
                        &deps.clock,
                        &meta,
                        &client_id,
                        client_secret.as_deref(),
                        &refresh,
                    )
                    .await
                    {
                        Ok(refreshed) => {
                            let stored_new = oauth::StoredTokens {
                                access_token: refreshed.access_token.expose_secret().clone(),
                                refresh_token: refreshed
                                    .refresh_token
                                    .as_ref()
                                    .map(|t| t.expose_secret().clone()),
                                expires_at_unix: refreshed
                                    .expires_at
                                    .duration_since(SystemTime::UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_secs(),
                                client_id: Some(client_id),
                                client_secret,
                                step_up_scope: None,
                            };
                            oauth::store_tokens(&deps.storage, &deps.clock, key, &stored_new)
                                .await
                                .map_err(McpError::from)?;
                            return Ok(stored_new.into_tokens());
                        }
                        // The refresh token itself is dead — fall through to
                        // the silent IdP+AS exchange below rather than a
                        // fresh interactive flow (XAA is never interactive).
                        Err(oauth::OAuthError::RefreshRejected(_)) => {}
                        Err(e) => return Err(e.into()),
                    }
                } else if !force_fresh {
                    // Delta 1: no refresh token — reuse the cached access
                    // token outright unless it is missing or expiring within
                    // 300s (`Err` from `duration_since` means already past).
                    let expiring_soon = s
                        .expires_at()
                        .duration_since(deps.clock.now())
                        .map(|remaining| remaining <= Duration::from_secs(300))
                        .unwrap_or(true);
                    if !expiring_soon {
                        return Ok(s.into_tokens());
                    }
                    tracing::debug!(
                        server = %config.name,
                        "XAA: access_token expiring, attempting silent exchange"
                    );
                }
            }
            None => {
                tracing::debug!(
                    server = %config.name,
                    "XAA: no access_token yet, attempting silent exchange"
                );
            }
        }

        // The IdP-login + AS-secret surface is supplied by the host provider.
        // Absent it, XAA cannot proceed — a clearly-noted residual seam.
        let provider = deps.xaa_config.as_ref().ok_or_else(|| {
            McpError::OAuth(format!(
                "XAA: no IdP connection configured for server '{}'. The XAA \
                 IdP-login/secret config surface (getXaaIdpSettings / \
                 acquireIdpIdToken / mcpOAuthClientConfig) is not yet wired \
                 (residual).",
                config.name
            ))
        })?;
        let inputs = provider
            .xaa_inputs(&config.name, spec_url(&config.spec))
            .await?
            .ok_or_else(|| {
                McpError::OAuth(format!(
                    "XAA: server '{}' is not XAA-provisioned (no IdP/AS inputs).",
                    config.name
                ))
            })?;

        let result = match crate::xaa::perform_cross_app_access(
            &deps.http,
            spec_url(&config.spec),
            &crate::xaa::XaaConfig {
                client_id: &inputs.client_id,
                client_secret: &inputs.client_secret,
                idp_client_id: &inputs.idp_client_id,
                idp_client_secret: inputs.idp_client_secret.as_deref(),
                idp_id_token: &inputs.idp_id_token,
                idp_token_endpoint: &inputs.idp_token_endpoint,
            },
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::debug!(server = %config.name, "XAA silent exchange failed: {e}");
                // 4xx token-exchange ⇒ the cached id_token was rejected; drop it
                // so the next resolve re-acquires (auth.ts:1840-1847
                // `clearIdpIdToken(idp.issuer)`). 5xx (IdP outage) keeps it.
                // Best-effort: a clear failure must not mask the exchange error.
                if e.should_clear_id_token() {
                    let _ = provider.clear_id_token().await;
                }
                return Err(e.into());
            }
        };

        // Persist: carry the AS confidential client_id/secret so refresh +
        // RFC-7009 revocation can authenticate the confidential client
        // (auth.ts:807-825 token-save). `expires_in` (when present) sets expiry.
        let expires_at = match result.tokens.expires_in {
            Some(secs) => deps.clock.now() + Duration::from_secs(secs),
            // No expiry advertised → treat as already-stale so the next connect
            // re-runs the exchange (XAA tokens are cheap to re-mint, silent).
            None => deps.clock.now(),
        };
        let stored = oauth::StoredTokens {
            access_token: result.tokens.access_token.clone(),
            refresh_token: result.tokens.refresh_token.clone(),
            expires_at_unix: expires_at
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            client_id: Some(inputs.client_id.clone()),
            client_secret: Some(inputs.client_secret.clone()),
            step_up_scope: None,
        };
        oauth::store_tokens(&deps.storage, &deps.clock, key, &stored)
            .await
            .map_err(McpError::from)?;

        Ok(stored.into_tokens())
    }

    /// Drive the full interactive OAuth flow and persist the resulting tokens.
    ///
    /// `scope_override` carries an elevated scope cached from a prior 403
    /// `insufficient_scope` step-up (auth.ts `cachedStepUpScope`); when set the
    /// authorize URL requests it instead of the advertised scope. On a
    /// successful grant the persisted `step_up_scope` is cleared (auth.ts:1705).
    /// `resource_metadata_url` (§24c) carries a `resource_metadata` challenge
    /// param from the live 401/403 that triggered this flow (`None` for the
    /// proactive, no-challenge callers — a fresh server with no stored token,
    /// or a stale-token silent refresh that hasn't hit the wire yet).
    async fn run_interactive_oauth(
        &self,
        config: &McpServerConfig,
        oauth_cfg: &traits::McpOAuthConfigDto,
        key: &str,
        deps: &OAuthDeps,
        scope_override: Option<&str>,
        resource_metadata_url: Option<&str>,
    ) -> Result<oauth::Tokens, McpError> {
        // Effective elevated scope: an explicit override (the 403 step-up path)
        // wins; otherwise honor any `step_up_scope` cached on the stored entry
        // from a previous 403 `insufficient_scope` (auth.ts:906-909
        // `cachedStepUpScope`). The stored blob is about to be overwritten by the
        // fresh grant, so we read it before driving the flow.
        let pinned_scope = oauth_cfg
            .scopes
            .as_deref()
            .map(str::trim)
            .filter(|scope| !scope.is_empty())
            .map(str::to_string);
        let cached_scope = match (pinned_scope, scope_override) {
            (Some(scope), _) => Some(scope),
            (None, Some(scope)) => Some(scope.to_string()),
            (None, None) => oauth::load_tokens(&deps.storage, key)
                .await
                .ok()
                .flatten()
                .and_then(|t| t.step_up_scope),
        };

        // Surface `AwaitingOAuth` while the user completes the browser flow.
        let callback_port = oauth_cfg.callback_port.unwrap_or(0);
        self.connections.write().await.insert(
            config.name.clone(),
            McpConnectionState::AwaitingOAuth {
                config: config.clone(),
                callback_port,
            },
        );
        let tokens = oauth::perform_oauth_flow_for_reauth(
            &deps.http,
            &deps.clock,
            oauth_cfg,
            &config.name,
            spec_url(&config.spec),
            &deps.on_authorization_url,
            cached_scope.as_deref(),
            resource_metadata_url,
        )
        .await?;
        // A fresh grant clears any pending step-up scope (auth.ts:1705): the new
        // tokens carry the elevated scope, so the cache must not linger.
        oauth::save_tokens(&deps.storage, &deps.clock, key, &tokens).await?;
        Ok(tokens)
    }

    /// Connect every server in `configs` at startup, mirroring claude-code's
    /// `loadAndConnectMcpConfigs` (services/mcp/useManageMCPConnections.ts).
    ///
    /// For each config:
    /// - **Disabled** servers (`config.disabled`) are seeded as
    ///   `Disconnected { last_error: None }` and skipped — `/mcp` still lists
    ///   them but they never auto-connect. This subsumes the manual
    ///   pre-population block in `apps/cli/src/init.rs`.
    /// - Otherwise [`Self::connect`] is invoked. On error the state is
    ///   overwritten with `Disconnected { last_error: Some(..) }` so the
    ///   reconnect loop ([`Self::run_reconnect_loop`]) can pick it up — note
    ///   [`Self::connect`] itself leaves the state stuck in `Connecting` on a
    ///   transport error, which this wrapper compensates for.
    ///
    /// One server's failure never aborts the batch; per-server failures are
    /// logged via `tracing::warn!`. Returns the per-server outcome in the same
    /// order as `configs`.
    pub async fn connect_all(
        &self,
        configs: Vec<McpServerConfig>,
    ) -> Vec<(String, Result<McpConnectionId, McpError>)> {
        use futures_util::future::join_all;
        let futs = configs.into_iter().map(|config| async move {
            let name = config.name.clone();
            if config.disabled {
                self.connections.write().await.insert(
                    name.clone(),
                    McpConnectionState::Disconnected {
                        config,
                        last_error: None,
                    },
                );
                tracing::debug!(server = %name, "skipping disabled MCP server");
                return None;
            }

            let result = self.connect(config.clone()).await;
            if let Err(ref e) = result {
                tracing::warn!(server = %name, error = %e, "MCP auto-connect failed");
                // `connect` leaves the state in `Connecting` on a transport
                // error; reset it to a loop-eligible `Disconnected`.
                self.connections.write().await.insert(
                    name.clone(),
                    McpConnectionState::Disconnected {
                        config,
                        last_error: Some(e.to_string()),
                    },
                );
            }
            Some((name, result))
        });
        join_all(futs).await.into_iter().flatten().collect()
    }

    /// Background reconnect/backoff loop (claude-code `reconnectWithBackoff`).
    ///
    /// Spawn with `tokio::spawn(registry.clone().run_reconnect_loop())`. The
    /// loop periodically scans `connections` for servers eligible to retry —
    /// `Disconnected { last_error: Some(_) }` (a failed connect) or
    /// `Reconnecting { .. }` (a retry already in flight) — and drives each
    /// through [`Self::reconnect_one`].
    ///
    /// Per claude-code, stdio servers are NOT auto-reconnected (a dead local
    /// process won't recover on its own); only remote transports are enrolled.
    pub async fn run_reconnect_loop(self: Arc<Self>) {
        loop {
            let candidates: Vec<McpServerConfig> = {
                let conns = self.connections.read().await;
                conns
                    .values()
                    .filter_map(|state| match state {
                        McpConnectionState::Disconnected {
                            config,
                            last_error: Some(_),
                        }
                        | McpConnectionState::Reconnecting { config, .. } => Some(config.clone()),
                        _ => None,
                    })
                    .filter(|config| !config.disabled && config.spec.kind() != "stdio")
                    .collect()
            };

            for config in candidates {
                Arc::clone(&self).reconnect_one(config).await;
            }

            tokio::time::sleep(self.health_check_interval).await;
        }
    }

    /// Drive a single server through the reconnect/backoff schedule.
    ///
    /// Attempts `connect` up to `self.max_retry_count` times. The FIRST attempt
    /// fires immediately (no leading sleep); after a failed NON-final attempt
    /// the task sleeps `min(INITIAL_BACKOFF * 2^(attempt-1), MAX_BACKOFF)` and
    /// the final attempt has NO trailing sleep — matching claude-code
    /// `useManageMCPConnections.ts:372-461`. For the default 5 attempts the
    /// sleeps are 1s, 2s, 4s, 8s, so attempts fire at t = 0, 1, 3, 7, 15s (the
    /// 16s/30s-cap value is never used). While waiting, the server rests in
    /// `Reconnecting { retry_count, next_retry_at }`. On success the server is
    /// left `Connected` (set by [`Self::connect`]); after the final attempt
    /// fails it transitions to `Failed { error, attempts }`.
    ///
    /// Aborts without marking `Failed` if the server is concurrently `Stopped`
    /// or its config is flipped to `disabled` (claude-code disabled-mid-wait
    /// guard).
    async fn reconnect_one(self: Arc<Self>, config: McpServerConfig) {
        let name = config.name.clone();
        let max = self.max_retry_count.max(1);

        for attempt in 1..=max {
            // Everything except the inter-attempt backoff runs UNDER the
            // per-server lifecycle lock, so the guard-read + `Reconnecting`
            // insert + connect (+ the terminal `Failed` insert) form ONE
            // critical section that cannot interleave with a locked
            // set_disabled/disconnect/connect. Previously the guard-read and the
            // `Reconnecting` insert ran unlocked, and only the inner connect took
            // the lock — a TOCTOU that could overwrite a `Connected{connection_id}`
            // won by a concurrent connect, stranding that id (unreachable for
            // teardown = a leaked remote connection). `true` ⇒ back off + retry.
            let retry = {
                let lifecycle = self.lifecycle_lock(&name);
                let _guard = lifecycle.lock().await;

                match self.connections.read().await.get(&name) {
                    Some(McpConnectionState::Stopped { .. }) | None => {
                        tracing::debug!(server = %name, "reconnect aborted: server stopped");
                        return;
                    }
                    // A concurrent connect/reconnect already brought the server
                    // up — leave its live connection alone; overwriting it with
                    // `Reconnecting` would leak that connection id.
                    Some(McpConnectionState::Connected { .. }) => {
                        tracing::debug!(server = %name, "reconnect aborted: already connected");
                        return;
                    }
                    Some(state) if state_is_disabled(state) => {
                        tracing::debug!(server = %name, "reconnect aborted: server disabled");
                        return;
                    }
                    _ => {}
                }

                // `next_retry_at` is the wall-clock time the NEXT attempt would
                // fire if this one fails; the final attempt has no successor, so
                // it points at the present.
                let next_retry_at = match post_attempt_backoff(attempt, max) {
                    Some(backoff) => SystemTime::now() + backoff,
                    None => SystemTime::now(),
                };
                self.connections.write().await.insert(
                    name.clone(),
                    McpConnectionState::Reconnecting {
                        config: config.clone(),
                        retry_count: attempt,
                        next_retry_at,
                    },
                );

                // claude-code runs attempt 1 IMMEDIATELY — there is NO leading
                // sleep before the first connect (`useManageMCPConnections.ts:372`).
                // `connect_locked` (NOT `connect`) — the lifecycle lock is already
                // held; `connect` would re-acquire it and deadlock.
                match self.connect_locked(config.clone()).await {
                    Ok(_) => {
                        tracing::info!(server = %name, attempt, "MCP reconnect succeeded");
                        return;
                    }
                    Err(e) if attempt == max => {
                        tracing::warn!(
                            server = %name,
                            attempts = attempt,
                            error = %e,
                            "MCP reconnect exhausted; marking Failed"
                        );
                        self.connections.write().await.insert(
                            name.clone(),
                            McpConnectionState::Failed {
                                config: config.clone(),
                                error: e.to_string(),
                                attempts: attempt,
                            },
                        );
                        return;
                    }
                    Err(e) => {
                        tracing::debug!(
                            server = %name,
                            attempt,
                            error = %e,
                            "MCP reconnect attempt failed; backing off"
                        );
                        true
                    }
                }
            };

            // Back off ONLY after a failed NON-final attempt (lock released); the
            // final attempt has no trailing sleep (claude-code schedules the
            // *next* retry, never one after the last).
            if retry {
                if let Some(backoff) = post_attempt_backoff(attempt, max) {
                    tokio::time::sleep(backoff).await;
                }
            }
        }
    }

    /// Drop the named connection and transition it to `Stopped`.
    pub async fn disconnect(&self, name: &str) -> Result<(), McpError> {
        let lifecycle = self.lifecycle_lock(name);
        let _guard = lifecycle.lock().await;
        self.disconnect_locked(name).await
    }

    async fn disconnect_locked(&self, name: &str) -> Result<(), McpError> {
        let Some((connection_id, config)) = ({
            let conns = self.connections.read().await;
            match conns.get(name) {
                Some(McpConnectionState::Connected {
                    connection_id,
                    config,
                    ..
                }) => Some((*connection_id, config.clone())),
                _ => None,
            }
        }) else {
            return Ok(());
        };

        // Do not remove the state before the transport confirms teardown. If
        // teardown fails, callers keep the still-live state and cached client.
        // The per-server lifecycle lock prevents a concurrent connect from
        // racing this await, while snapshots for every server remain unblocked.
        self.transport.disconnect(connection_id).await?;

        let transitioned = {
            let mut conns = self.connections.write().await;
            let same_generation = matches!(
                conns.get(name),
                Some(McpConnectionState::Connected {
                    connection_id: current,
                    ..
                }) if *current == connection_id
            );
            if same_generation {
                conns.insert(
                    name.to_string(),
                    McpConnectionState::Stopped {
                        config: config.clone(),
                    },
                );
            }
            same_generation
        };
        if !transitioned {
            return Ok(());
        }

        // Drop any cached `McpClient` so `get_client(name)` stops returning a
        // handle to the now-dead connection. `shift_remove` preserves discovery
        // order for the remaining entries.
        self.clients.write().await.shift_remove(name);
        // Remove this connection's dynamic tool partition immediately. A later
        // reconnect emits a fresh Tools event with its new connection id.
        let _ = self.catalog_changes.send(McpCatalogChanged {
            server_name: name.to_string(),
            connection_id,
            retired_connection_id: Some(connection_id),
            kind: McpCatalogKind::Tools,
        });

        // Token revocation (RFC 7009) is best-effort and intentionally runs
        // after local state/catalog retirement, so slow network I/O cannot make
        // a dead server continue to appear live.
        if let (Some(deps), Some(oauth_cfg)) = (self.oauth.as_ref(), spec_oauth(&config.spec)) {
            let key = oauth::server_key(&config.name, &config.spec);
            oauth::revoke_server_tokens(
                &deps.storage,
                &deps.http,
                &key,
                spec_url(&config.spec),
                oauth_cfg,
            )
            .await;
        }
        Ok(())
    }

    /// Remove a configured server and its cached client from the registry.
    ///
    /// This is intentionally separate from `disconnect`: a disconnected
    /// server remains visible in `/mcp`, while a configuration reload must
    /// remove entries deleted from the on-disk config as well.
    pub async fn remove(&self, name: &str) -> Result<(), McpError> {
        self.disconnect(name).await?;
        self.connections.write().await.remove(name);
        self.clients.write().await.shift_remove(name);
        Ok(())
    }

    /// Toggle one registered server immediately and retain the updated config
    /// for subsequent reconnects/startups. Disabling retires the live transport
    /// and dynamic tool partition without revoking OAuth credentials; enabling
    /// performs a fresh connect in the current session.
    ///
    /// Returns `Ok(None)` when the config was already in the requested state (a
    /// no-op — claude's `p` filter excludes it). Otherwise `Ok(Some(state))`
    /// carries the server's post-toggle action state (claude's fulfilled
    /// `u(name).type`): after disable it is [`traits::McpActionState::Disabled`];
    /// after enable it is the live post-connect state read back from the
    /// registry (`Connected` / `Failed` / `NeedsAuth` / …). Crucially, a failed
    /// enable **connect** is NOT surfaced as `Err` — the server flips on but
    /// stays disconnected, mirroring claude's `u(name)` fulfilling with
    /// `{type:"failed"}`; only a failed **disable teardown** stays an `Err`
    /// (claude's rejected promise → the server "couldn't be changed").
    pub async fn set_disabled(
        &self,
        name: &str,
        disabled: bool,
    ) -> Result<Option<traits::McpActionState>, McpError> {
        let lifecycle = self.lifecycle_lock(name);
        let _guard = lifecycle.lock().await;

        let (mut config, live_connection) = {
            let conns = self.connections.read().await;
            let Some(state) = conns.get(name) else {
                return Err(McpError::Internal(format!(
                    "no MCP server named \"{name}\""
                )));
            };
            if state.config().disabled == disabled {
                return Ok(None);
            }
            let live_connection = match state {
                McpConnectionState::Connected { connection_id, .. }
                | McpConnectionState::HealthChecking { connection_id, .. } => Some(*connection_id),
                _ => None,
            };
            (state.config().clone(), live_connection)
        };

        if disabled {
            // Keep the live state/client until the transport confirms teardown.
            // This makes a failed disable visible and safely retryable.
            if let Some(connection_id) = live_connection {
                self.transport.disconnect(connection_id).await?;
            }
            config.disabled = true;
            self.connections.write().await.insert(
                name.to_string(),
                McpConnectionState::Disconnected {
                    config,
                    last_error: None,
                },
            );
            self.clients.write().await.shift_remove(name);
            if let Some(connection_id) = live_connection {
                let _ = self.catalog_changes.send(McpCatalogChanged {
                    server_name: name.to_string(),
                    connection_id,
                    retired_connection_id: Some(connection_id),
                    kind: McpCatalogKind::Tools,
                });
            }
            return Ok(Some(traits::McpActionState::Disabled));
        }

        config.disabled = false;
        self.connections.write().await.insert(
            name.to_string(),
            McpConnectionState::Disconnected {
                config: config.clone(),
                last_error: None,
            },
        );
        // A failed enable connect is a SETTLED "failed" outcome, not an error:
        // `connect_locked` already records `Disconnected { last_error }` on
        // failure, so the server flips on but reads back as `Failed`
        // ("not connected"). This mirrors claude's `u(name)` fulfilling with
        // `{type:"failed"}` rather than rejecting, so the /mcp handler can emit
        // "Enabled …, but it isn't connected yet." instead of a hard error.
        let _ = self.connect_locked(config).await;
        let resulting = {
            let conns = self.connections.read().await;
            conns
                .get(name)
                .map_or(traits::McpActionState::Failed, project_action_state)
        };
        Ok(Some(resulting))
    }

    /// Reconnect a single known server by name (`/mcp reconnect <server>`):
    /// tear down the live connection and re-establish it from the config the
    /// registry retains in every connection state. `Err(McpError::Internal)`
    /// when no server by that name is registered (callers pre-check via
    /// [`Self::server_names`] for the user-facing "no server named" message).
    pub async fn reconnect(&self, name: &str) -> Result<(), McpError> {
        let lifecycle = self.lifecycle_lock(name);
        let _guard = lifecycle.lock().await;
        // Every connection state carries its originating config; pull it out so
        // we can re-`connect` after tearing the live connection down.
        let config = {
            let conns = self.connections.read().await;
            let Some(state) = conns.get(name) else {
                return Err(McpError::Internal(format!(
                    "no MCP server named \"{name}\""
                )));
            };
            state.config().clone()
        };
        // Best-effort teardown (a never-connected server is a no-op), then a
        // fresh connect. `connect` early-returns the existing id if already
        // connected, so the disconnect must land first.
        self.disconnect_locked(name).await?;
        self.connect_locked(config).await.map(|_| ())
    }

    /// The names of every registered server (any connection state), sorted —
    /// the set `/mcp reconnect all` iterates.
    pub async fn server_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.connections.read().await.keys().cloned().collect();
        names.sort();
        names
    }

    /// Project every known connection into the trait-facing
    /// [`traits::McpServerInfo`] shape. Used by
    /// `OrchestratorHandle::list_mcp_servers` (M6-07) so `/mcp` can list
    /// the registry without exposing the internal state-machine enum.
    ///
    /// Returned list is sorted by `name` for stable display order.
    pub async fn snapshot(&self) -> Vec<traits::McpServerInfo> {
        let conns = self.connections.read().await;
        let mut out: Vec<traits::McpServerInfo> = conns
            .values()
            .map(|s| traits::McpServerInfo {
                name: s.name().to_string(),
                status: project_status(s),
                transport: s.transport_kind().to_string(),
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// Fine-grained `(name, McpActionState)` pairs for the `/mcp` action
    /// handler (`reconnect|enable|disable`). Unlike [`Self::snapshot`], this
    /// preserves the full state vocabulary (pending / disabled / needs-auth /
    /// failed) the handler needs to pick claude-code's byte-exact state-aware
    /// message. Sorted by name for stable display.
    pub async fn action_states(&self) -> Vec<(String, traits::McpActionState)> {
        let conns = self.connections.read().await;
        let mut out: Vec<(String, traits::McpActionState)> = conns
            .values()
            .map(|s| (s.name().to_string(), project_action_state(s)))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// Failed servers with their sanitized error text, for the `ToolSearch`
    /// empty-result diagnostics note — a port of claude-code's `wZr(u())`
    /// (`failed_mcp_servers`). Every server whose action state projects to
    /// [`traits::McpActionState::Failed`] ("not connected") is included,
    /// carrying its recorded error where one exists, sanitized through
    /// [`sanitize_diagnostic`] (claude's `xLt`). Both the name and the error are
    /// sanitized (claude sanitizes both); the error is the untrusted, model-
    /// visible part. Sorted by name.
    ///
    /// Unlike claude, the port carries no per-server `errorCode`, so it cannot
    /// distinguish the `UNCONFIGURED` state claude's `kee` excludes — every
    /// projected-`Failed` server is surfaced.
    pub async fn failed_action_servers(&self) -> Vec<(String, Option<String>)> {
        let conns = self.connections.read().await;
        let mut out: Vec<(String, Option<String>)> = conns
            .values()
            .filter(|s| project_action_state(s) == traits::McpActionState::Failed)
            .map(|s| {
                let error = match s {
                    McpConnectionState::Failed { error, .. } => Some(error.clone()),
                    McpConnectionState::Disconnected { last_error, .. } => last_error.clone(),
                    _ => None,
                };
                (
                    sanitize_diagnostic(s.name()),
                    error.map(|e| sanitize_diagnostic(&e)),
                )
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// Names of servers currently in a PENDING (still-connecting) state —
    /// `Connecting` / `AwaitingOAuth` / `Reconnecting` (claude-code's MCP client
    /// `type === "pending"`). These may yet expose tools, so the `AgentTool`
    /// required-MCP gate waits on them before failing. NOTE: the public
    /// [`traits::McpStatus`] UI projection collapses these into `Disconnected`;
    /// this reads the INTERNAL state map so a connecting server is
    /// distinguishable from a failed/absent one (the gap that blocked the
    /// 30s poll-wait).
    pub async fn servers_pending(&self) -> Vec<String> {
        self.connections
            .read()
            .await
            .values()
            .filter(|s| {
                matches!(
                    s,
                    McpConnectionState::Connecting { .. }
                        | McpConnectionState::AwaitingOAuth { .. }
                        | McpConnectionState::Reconnecting { .. }
                )
            })
            .map(|s| s.name().to_string())
            .collect()
    }

    /// Recompute and store the [`Self::has_pending_servers`] mirror, returning
    /// the fresh value.
    ///
    /// This is claude-code's `eZf()` (`bdl(b7e()??[]).length>0`,
    /// `cc-238.js @229641619`), where `bdl` filters MCP clients on
    /// `type === "pending"`. [`project_action_state`] reproduces that
    /// discriminant, so `needs-auth` (a SEPARATE client type upstream) does NOT
    /// count as pending here — unlike [`Self::servers_pending`], which
    /// deliberately folds `AwaitingOAuth` in for the `AgentTool` required-MCP
    /// poll-wait.
    ///
    /// Call it from an async seam right before assembling a tool list; the
    /// `WaitForMcpServers` tool's synchronous `is_enabled` then reads the
    /// mirror.
    pub async fn refresh_pending_servers(&self) -> bool {
        let pending = self
            .connections
            .read()
            .await
            .values()
            .any(|s| project_action_state(s) == traits::McpActionState::Pending);
        self.pending_servers
            .store(pending, std::sync::atomic::Ordering::Relaxed);
        pending
    }

    /// Synchronous read of the pending-server mirror last written by
    /// [`Self::refresh_pending_servers`].
    #[must_use]
    pub fn has_pending_servers(&self) -> bool {
        self.pending_servers
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Names of servers in a terminal FAILED state (`Failed` — exhausted retries;
    /// claude-code's MCP client `type === "failed"`). The required-MCP gate's
    /// poll-wait stops early when a required server fails rather than waiting out
    /// the full deadline.
    pub async fn servers_failed(&self) -> Vec<String> {
        self.connections
            .read()
            .await
            .values()
            .filter(|s| matches!(s, McpConnectionState::Failed { .. }))
            .map(|s| s.name().to_string())
            .collect()
    }

    /// The set of MCP server names that currently expose at least one tool —
    /// i.e. servers that are connected AND authenticated (an unauthenticated
    /// server has no tools). Port of claude-code's `serversWithTools` derivation
    /// in `AgentTool.call` (`AgentTool.tsx:394-405`): claude scans
    /// `appState.mcp.tools` for `mcp__<server>__<tool>` and collects the distinct
    /// `<server>` part. Here we ask each registered [`McpClient`] for its tools
    /// and extract the server segment from each tool's `full_name`
    /// (`mcp__<server>__<tool>`), so a server with zero tools (e.g. still
    /// awaiting OAuth) is correctly absent.
    ///
    /// Used by `AgentTool`'s pre-spawn `required_mcp_servers` gate
    /// (`AgentTool.tsx:367-409`). Returned list is in DISCOVERY (insertion)
    /// order and deduplicated — claude builds `serversWithTools` by iterating
    /// `appState.mcp.tools` in order and pushing first-seen server names, with
    /// NO sort, so the required-MCP error lists servers in that same order.
    pub async fn servers_with_tools(&self) -> Vec<String> {
        let clients: Vec<(String, Arc<McpClient>)> = self
            .clients
            .read()
            .await
            .iter()
            .map(|(name, entry)| (name.clone(), Arc::clone(&entry.client)))
            .collect();
        let cached: HashMap<String, Vec<traits::McpToolDto>> = self
            .connections
            .read()
            .await
            .iter()
            .filter_map(|(name, state)| match state {
                McpConnectionState::Connected { tools, .. } => Some((name.clone(), tools.clone())),
                _ => None,
            })
            .collect();
        let mut out: Vec<String> = Vec::new();
        for (name, client) in clients {
            let tools = if let Some(tools) = cached.get(&name) {
                tools.clone()
            } else {
                match client.list_tools().await {
                    Ok(tools) => tools,
                    Err(_) => continue,
                }
            };
            for tool in tools {
                // `full_name` is `mcp__<server>__<tool>` (rewrite site in
                // `connect`); the server segment is index 1.
                let parts: Vec<&str> = tool.full_name.split("__").collect();
                if let Some(server) = parts.get(1) {
                    if !server.is_empty() && !out.iter().any(|s| s == server) {
                        out.push((*server).to_string());
                    }
                }
            }
        }
        out
    }

    /// Every prompt advertised by a CONNECTED server, as
    /// `(server_name, connection_id, prompt)`.
    ///
    /// Claude-code merges these into the slash-command list (`getAllCommands`
    /// folding in `mcp.commands`), which is what makes an MCP prompt reachable
    /// as `/<server>:<prompt>` and findable by the `Skill` tool. The registry
    /// has always FETCHED them (`prompts/list` during `connect`, refreshed on
    /// `notifications/prompts/list_changed`); nothing read them back.
    ///
    /// Only `Connected` servers contribute: a reconnecting or failed server's
    /// last-known prompts would advertise commands that cannot be fetched.
    /// Ordered by server name so the merged command list is deterministic.
    pub async fn connected_prompts(
        &self,
    ) -> Vec<(String, protocol::McpConnectionId, traits::McpPromptDto)> {
        let conns = self.connections.read().await;
        let mut servers: Vec<&String> = conns.keys().collect();
        servers.sort();
        let mut out = Vec::new();
        for name in servers {
            if let Some(crate::connection::McpConnectionState::Connected {
                connection_id,
                prompts,
                ..
            }) = conns.get(name)
            {
                for prompt in prompts {
                    out.push((name.clone(), *connection_id, prompt.clone()));
                }
            }
        }
        out
    }

    /// Render one prompt through the exact live connection that advertised it.
    ///
    /// Commands retain a [`McpConnectionId`] rather than only the logical server
    /// name so reconnecting a server cannot accidentally dispatch a stale menu
    /// entry through a newer connection generation.
    pub async fn get_prompt(
        &self,
        connection_id: protocol::McpConnectionId,
        prompt_name: &str,
        arguments: serde_json::Value,
    ) -> Result<serde_json::Value, McpError> {
        let server_name = {
            let connections = self.connections.read().await;
            connections
                .iter()
                .find_map(|(name, state)| match state {
                    crate::connection::McpConnectionState::Connected {
                        connection_id: current,
                        ..
                    } if *current == connection_id => Some(name.clone()),
                    _ => None,
                })
                .ok_or_else(|| {
                    McpError::Internal(format!(
                        "MCP prompt connection {connection_id} is no longer active"
                    ))
                })?
        };

        let client = {
            let clients = self.clients.read().await;
            let entry = clients.get(&server_name).ok_or_else(|| {
                McpError::Internal(format!(
                    "MCP prompt server {server_name} has no live client"
                ))
            })?;
            if entry.connection_id != Some(connection_id) {
                return Err(McpError::Internal(format!(
                    "MCP prompt connection {connection_id} is no longer active"
                )));
            }
            Arc::clone(&entry.client)
        };

        client
            .get_prompt(prompt_name, arguments)
            .await
            .map_err(|error| McpError::Internal(error.to_string()))
    }

    /// Recover the RAW wire tool name for a model-facing MCP tool `full_name`.
    ///
    /// The model-facing `full_name` (`mcp__<normalize(server)>__<normalize(tool)>`,
    /// the rewrite site in [`Self::connect`]) carries the NORMALIZED tool
    /// segment, 1:1 with claude-code's `buildMcpToolName`. The MCP server,
    /// however, expects the UNNORMALIZED wire name in its `tools/call` request.
    /// claude-code keeps it as `mcpInfo.toolName` (`client.ts:1774`); here it
    /// lives on the cached [`traits::McpToolDto::tool_name`], so the dispatch
    /// path recovers it by matching the dto whose `full_name` equals the
    /// model-supplied name.
    ///
    /// `normalized_server` is the FQN's server segment (already normalized).
    /// Returns `None` when no connected server matches it or `full_name` is
    /// unknown — the caller then falls back to the parsed (normalized) segment,
    /// a no-op for valid-identifier names where raw == normalized.
    pub async fn resolve_wire_tool_name(
        &self,
        normalized_server: &str,
        full_name: &str,
    ) -> Option<String> {
        let conns = self.connections.read().await;
        for state in conns.values() {
            if let McpConnectionState::Connected { config, tools, .. } = state {
                if normalize_name_for_mcp(&config.name) == normalized_server {
                    return tools
                        .iter()
                        .find(|dto| dto.full_name == full_name)
                        .map(|dto| dto.tool_name.clone());
                }
            }
        }
        None
    }
}

/// MCPLIFE.4: the connect+initialize handshake deadline, mirroring claude-code's
/// `getConnectionTimeoutMs()` (`services/mcp/client.ts:456-458`):
/// `parseInt(process.env.MCP_TIMEOUT || '', 10) || 30000` — a positive integer
/// number of milliseconds, defaulting to 30s when unset / non-numeric / zero.
pub(crate) fn mcp_connection_timeout() -> Duration {
    let ms = std::env::var("MCP_TIMEOUT")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(30_000);
    Duration::from_millis(ms)
}

/// Exponential backoff for reconnect `attempt` (1-based), capped at
/// [`MAX_BACKOFF`]. Mirrors claude-code's
/// `min(INITIAL_BACKOFF_MS * 2^(attempt-1), MAX_BACKOFF_MS)`.
fn backoff_for(attempt: u32) -> Duration {
    let factor = 1u64
        .checked_shl(attempt.saturating_sub(1))
        .unwrap_or(u64::MAX);
    let base = u64::try_from(INITIAL_BACKOFF.as_millis()).unwrap_or(u64::MAX);
    let millis = base.saturating_mul(factor);
    Duration::from_millis(millis).min(MAX_BACKOFF)
}

/// Backoff to sleep AFTER reconnect `attempt` (1-based) within a run of `max`
/// attempts, or `None` when no sleep should occur.
///
/// Encodes claude-code's schedule (`useManageMCPConnections.ts:446-460`): a
/// backoff is taken only after a failed NON-final attempt; the final attempt
/// (`attempt == max`) has no trailing sleep, and — because attempt 1 fires
/// immediately — there is never a leading sleep. For the default `max == 5`
/// the sleeps are 1s, 2s, 4s, 8s, so attempts fire at t = 0, 1, 3, 7, 15s; the
/// 16s / 30s-cap value is therefore never used.
fn post_attempt_backoff(attempt: u32, max: u32) -> Option<Duration> {
    if attempt >= max {
        None
    } else {
        Some(backoff_for(attempt))
    }
}

#[cfg(test)]
mod connection_timeout_tests {
    //! MCPLIFE.4 — connect deadline parity with claude-code
    //! `getConnectionTimeoutMs()` (`parseInt(MCP_TIMEOUT) || 30000`).
    use super::mcp_connection_timeout;
    use std::time::Duration;

    // The only test in this crate that mutates MCP_TIMEOUT; the connect-path
    // tests don't assert the deadline, so the shared-env mutation is harmless.
    #[test]
    fn timeout_defaults_to_30s_and_honors_positive_env() {
        std::env::remove_var("MCP_TIMEOUT");
        assert_eq!(mcp_connection_timeout(), Duration::from_secs(30));
        std::env::set_var("MCP_TIMEOUT", "5000");
        assert_eq!(mcp_connection_timeout(), Duration::from_millis(5000));
        // parseInt(..) || 30000 — zero / non-numeric / empty fall back to 30s.
        std::env::set_var("MCP_TIMEOUT", "0");
        assert_eq!(mcp_connection_timeout(), Duration::from_secs(30));
        std::env::set_var("MCP_TIMEOUT", "not-a-number");
        assert_eq!(mcp_connection_timeout(), Duration::from_secs(30));
        std::env::remove_var("MCP_TIMEOUT");
    }
}

#[cfg(test)]
mod backoff_schedule_tests {
    //! MCP reconnect backoff schedule parity with claude-code
    //! `useManageMCPConnections.ts:372-461` (`MAX_RECONNECT_ATTEMPTS = 5`,
    //! `INITIAL_BACKOFF_MS = 1000`, `MAX_BACKOFF_MS = 30000`).
    use super::{backoff_for, post_attempt_backoff, INITIAL_BACKOFF, MAX_BACKOFF};
    use std::time::Duration;

    /// The ordered list of inter-attempt sleeps actually taken during a
    /// reconnect run of `max` attempts — derived from the same
    /// [`post_attempt_backoff`] decision the production loop uses.
    fn sleep_schedule(max: u32) -> Vec<Duration> {
        (1..=max)
            .filter_map(|attempt| post_attempt_backoff(attempt, max))
            .collect()
    }

    #[test]
    fn constants_match_claude_code() {
        assert_eq!(INITIAL_BACKOFF, Duration::from_millis(1000));
        assert_eq!(MAX_BACKOFF, Duration::from_secs(30));
    }

    #[test]
    fn no_leading_sleep_and_no_trailing_sleep_on_final_attempt() {
        let max = 5;
        // The final attempt never sleeps afterwards (no trailing sleep).
        assert_eq!(post_attempt_backoff(max, max), None);
        // Every non-final attempt sleeps. The first inter-attempt sleep happens
        // AFTER attempt 1 — the loop calls `connect` before any sleep, so
        // attempt 1 has no leading sleep.
        for attempt in 1..max {
            assert!(
                post_attempt_backoff(attempt, max).is_some(),
                "attempt {attempt} of {max} should be followed by a backoff",
            );
        }
        // Exactly `max - 1` sleeps occur across the whole run.
        assert_eq!(sleep_schedule(max).len(), (max - 1) as usize);
    }

    #[test]
    fn schedule_is_1_2_4_8_seconds_and_16s_is_never_slept() {
        // claude-code sleeps 1s, 2s, 4s, 8s between the 5 attempts. The 16s
        // value (and the 30s cap) is NEVER used — it would only ever be a
        // trailing sleep after the final attempt, which does not exist.
        assert_eq!(
            sleep_schedule(5),
            vec![
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(4),
                Duration::from_secs(8),
            ],
        );
        assert!(
            !sleep_schedule(5).contains(&Duration::from_secs(16)),
            "the 16s backoff (attempt 5) must never be slept",
        );
    }

    #[test]
    fn attempt_fire_times_are_0_1_3_7_15_seconds() {
        // Cumulative offsets at which each of the 5 attempts fires. Attempt 1
        // at t = 0 proves there is no leading sleep.
        let mut fire_times = vec![Duration::ZERO];
        let mut acc = Duration::ZERO;
        for s in sleep_schedule(5) {
            acc += s;
            fire_times.push(acc);
        }
        assert_eq!(
            fire_times,
            vec![
                Duration::from_secs(0),
                Duration::from_secs(1),
                Duration::from_secs(3),
                Duration::from_secs(7),
                Duration::from_secs(15),
            ],
        );
    }

    #[test]
    fn backoff_for_is_exponential_and_capped_at_max() {
        assert_eq!(backoff_for(1), Duration::from_secs(1));
        assert_eq!(backoff_for(2), Duration::from_secs(2));
        assert_eq!(backoff_for(3), Duration::from_secs(4));
        assert_eq!(backoff_for(4), Duration::from_secs(8));
        assert_eq!(backoff_for(5), Duration::from_secs(16));
        // 1000 * 2^5 = 32s exceeds the 30s ceiling → clamped.
        assert_eq!(backoff_for(6), MAX_BACKOFF);
        // Large attempts saturate at the cap, never overflow.
        assert_eq!(backoff_for(100), MAX_BACKOFF);
    }
}

/// Borrow the `oauth` config block of an SSE/HTTP spec, if present. Other
/// transports (stdio, websocket, …) never carry OAuth → `None`.
fn spec_oauth(spec: &McpTransportSpec) -> Option<&traits::McpOAuthConfigDto> {
    match spec {
        McpTransportSpec::Sse { oauth, .. } | McpTransportSpec::Http { oauth, .. } => {
            oauth.as_ref()
        }
        _ => None,
    }
}

/// Endpoint URL of an SSE/HTTP spec (used as the OAuth `server_url` for
/// discovery and the `getServerKey` hash). Empty for non-remote specs.
fn spec_url(spec: &McpTransportSpec) -> &str {
    match spec {
        McpTransportSpec::Sse { url, .. } | McpTransportSpec::Http { url, .. } => url,
        _ => "",
    }
}

/// Clone `spec` with `Authorization: Bearer <token>` set in its headers map.
/// Only SSE/HTTP specs carry headers; other variants are returned unchanged.
fn inject_bearer(spec: &McpTransportSpec, access_token: &str) -> McpTransportSpec {
    let bearer = format!("Bearer {access_token}");
    match spec.clone() {
        McpTransportSpec::Sse {
            url,
            mut headers,
            headers_helper,
            oauth,
        } => {
            headers.insert("Authorization".into(), bearer);
            McpTransportSpec::Sse {
                url,
                headers,
                headers_helper,
                oauth,
            }
        }
        McpTransportSpec::Http {
            url,
            mut headers,
            headers_helper,
            oauth,
        } => {
            headers.insert("Authorization".into(), bearer);
            McpTransportSpec::Http {
                url,
                headers,
                headers_helper,
                oauth,
            }
        }
        other => other,
    }
}

/// Faithful-core 401 detection. The error reaching here is always the result
/// of `connect_attempt` (transport `connect` + `initialize`): SSE's pre-flight
/// GET returns `McpError::HttpResponse` directly on a non-2xx status
/// (`platforms/common/src/mcp_sse.rs`), and Streamable HTTP's `initialize`
/// unwraps the same shape from a synthetic JSON-RPC error's structured
/// `data: {httpStatus, wwwAuthenticate}` (`handshake_error`,
/// `platforms/posix/src/mcp.rs` — mirrored on any platform that wires a real
/// HTTP transport). The substring match on `"401"` remains as a fallback for
/// any OTHER path that still flattens to a string (e.g. a raw connection
/// failure whose message happens to mention a status code) — it is not
/// expected to be the primary match for a real 401 any more.
fn error_is_401(e: &McpError) -> bool {
    matches!(e, McpError::HttpResponse { status: 401, .. })
        || matches!(
            e,
            McpError::Connection(m) | McpError::Handshake(m) if m.contains("401")
        )
}

pub(crate) fn error_is_auth_response(error: &McpError) -> bool {
    error_is_401(error)
        || matches!(error, McpError::HttpResponse { status: 403, .. })
        || matches!(
            error,
            McpError::Connection(message) | McpError::Handshake(message)
                if message.contains("403")
        )
}

/// Faithful-core 403 `insufficient_scope` step-up detection (auth.ts
/// `wrapFetchWithStepUpDetection`, 1354-1374). The primary path is now
/// structural: `connect_attempt`'s error carries a genuine `www_authenticate`
/// header value in `McpError::HttpResponse` (see `error_is_401`'s note — SSE's
/// pre-flight GET, and Streamable HTTP's `initialize` via `handshake_error`),
/// so `www_authenticate.contains("insufficient_scope")` and
/// `extract_scope_from_www_auth` run against the real header text. The
/// `Connection`/`Handshake` string arms remain as a substring-matched
/// fallback (`"403"` + `"insufficient_scope"`, extracting a `scope="…"`/
/// `scope=…` token per RFC 6750 §3 — the same shape as the SDK's
/// `extractFieldFromWwwAuth`) for any error shape that still flattens to a
/// string. Returns the elevated scope when present.
///
/// A tool-call-time 403 (post-connect, i.e. `McpClient::call_tool_with_progress`
/// rather than `connect_attempt`) is a SEPARATE path: `mcp/src/client.rs`'s
/// `mcp_client_error_from_rpc` already reconstructs a structured
/// `McpClientError::HttpResponse` from the same
/// `MCP_HTTP_STATUS=…;WWW_AUTHENTICATE=…` marker, consumed by
/// `call_tool_with_auth_retry`'s `is_auth_response()` check — this function
/// is never called on that path, so it is out of scope here.
fn error_is_403_insufficient_scope(e: &McpError) -> Option<String> {
    if let McpError::HttpResponse {
        status: 403,
        www_authenticate: Some(value),
    } = e
    {
        return value
            .contains("insufficient_scope")
            .then(|| extract_scope_from_www_auth(value))
            .flatten();
    }
    let (McpError::Connection(msg) | McpError::Handshake(msg)) = e else {
        return None;
    };
    if !(msg.contains("403") && msg.contains("insufficient_scope")) {
        return None;
    }
    extract_scope_from_www_auth(msg)
}

/// Extract a `resource_metadata` challenge param (RFC 9728) from a connect
/// failure's `WWW-Authenticate` header, for both the 401-reauth and 403
/// step-up call sites (§24c: oracle `Be`/`H0e`, threaded into `discover_
/// auth_server_metadata` so a server that publishes its Protected Resource
/// Metadata somewhere other than the well-known guess still resolves).
/// Structural only: `McpError::HttpResponse` (the shape `connect_attempt`'s
/// error now carries — see `error_is_401`'s note) is the sole source; the
/// string-flattened `Connection`/`Handshake` fallback shapes elsewhere in this
/// file don't carry a real header to reparse, so they yield `None` here and
/// discovery falls back to its well-known guess, exactly as before this
/// finding.
fn error_resource_metadata_url(e: &McpError) -> Option<String> {
    let McpError::HttpResponse {
        www_authenticate: Some(value),
        ..
    } = e
    else {
        return None;
    };
    oauth::parse_www_authenticate_challenge(value).resource_metadata_url
}

/// Hand-rolled equivalent of `wwwAuth.match(/scope=(?:"([^"]+)"|([^\s,]+))/)`
/// (auth.ts:1365). Finds the first `scope=` and returns its value, honoring an
/// optional double-quoted form; an unquoted value runs to the first whitespace
/// or comma. Avoids a `regex` dependency.
fn extract_scope_from_www_auth(s: &str) -> Option<String> {
    let idx = s.find("scope=")?;
    let rest = &s[idx + "scope=".len()..];
    if let Some(after_quote) = rest.strip_prefix('"') {
        // Quoted: up to the next `"`.
        let end = after_quote.find('"')?;
        let scope = &after_quote[..end];
        return (!scope.is_empty()).then(|| scope.to_string());
    }
    // Unquoted: up to the first whitespace or comma.
    let end = rest
        .find(|c: char| c.is_whitespace() || c == ',')
        .unwrap_or(rest.len());
    let scope = &rest[..end];
    (!scope.is_empty()).then(|| scope.to_string())
}

/// Test-only re-exports of internal OAuth-error classifiers, so integration
/// tests (`tests/oauth_flow_test.rs`) can unit-check the step-up detection
/// without making the helpers part of the public API.
#[doc(hidden)]
pub mod test_support {
    use traits::McpError;

    /// See [`super::error_is_403_insufficient_scope`].
    #[must_use]
    pub fn error_is_403_insufficient_scope(e: &McpError) -> Option<String> {
        super::error_is_403_insufficient_scope(e)
    }
}

/// Sanitize a model-visible diagnostic string (an MCP server name or failure
/// message) — a 1:1 port of claude-code's `xLt`, used to build the `ToolSearch`
/// empty-result failed-server note. Steps mirror claude exactly:
///
/// 1. NFKC-normalize.
/// 2. Replace control (`\p{Cc}`) / format (`\p{Cf}`) characters and
///    U+2028/U+2029 with a space (claude's `qU`).
/// 3. Replace angle brackets, `"`, `;` and a set of fancy quote / bracket
///    characters with a space.
/// 4. Collapse runs of whitespace to a single space and trim.
/// 5. Truncate to 200 characters, appending `…` (U+2026) when it was longer.
///
/// Steps 2–4 all map their targets to a space and then collapse, so a single
/// pass that treats every stripped-or-whitespace character as a collapsing
/// space produces the same result.
fn sanitize_diagnostic(input: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    /// claude's `RJu`.
    const LIMIT: usize = 200;

    // The explicit character class claude replaces with a space (step 3).
    fn is_stripped(c: char) -> bool {
        matches!(
            c,
            '<' | '>'
                | '"'
                | ';'
                | '\u{2018}'
                | '\u{2019}'
                | '\u{201A}'
                | '\u{201C}'
                | '\u{201D}'
                | '\u{201E}'
                | '\u{00AB}'
                | '\u{00BB}'
                | '\u{2039}'
                | '\u{203A}'
                | '\u{2329}'
                | '\u{232A}'
                | '\u{27E8}'
                | '\u{27E9}'
                | '\u{27EA}'
                | '\u{27EB}'
                | '\u{3008}'
                | '\u{3009}'
                | '\u{300A}'
                | '\u{300B}'
        )
    }

    let normalized: String = input.nfkc().collect();
    let mut collapsed = String::with_capacity(normalized.len());
    let mut pending_space = false;
    for c in normalized.chars() {
        // Step 2 (`qU`): control / format chars + U+2028/U+2029, step 3's
        // explicit class, and any other whitespace (step 4's `\s+`) all become
        // collapsing spaces.
        let is_control_or_format =
            c.is_control() || matches!(c, '\u{2028}' | '\u{2029}') || is_format_char(c);
        if is_control_or_format || is_stripped(c) || c.is_whitespace() {
            pending_space = true;
            continue;
        }
        if pending_space && !collapsed.is_empty() {
            collapsed.push(' ');
        }
        pending_space = false;
        collapsed.push(c);
    }
    // `collapsed` already has no leading/interior double spaces or trailing
    // space (pending_space is dropped at end), i.e. it is already trimmed.
    if collapsed.chars().count() > LIMIT {
        let head: String = collapsed.chars().take(LIMIT).collect();
        format!("{head}\u{2026}")
    } else {
        collapsed
    }
}

/// Whether `c` is a Unicode format character (general category `Cf`) — the
/// portion of claude's `qU` (`\p{Cf}`) not covered by [`char::is_control`]
/// (which is `Cc`). Enumerates the stable `Cf` code-point blocks.
fn is_format_char(c: char) -> bool {
    let cp = c as u32;
    matches!(
        cp,
        0x00AD              // SOFT HYPHEN
        | 0x0600..=0x0605   // Arabic number signs
        | 0x061C            // Arabic Letter Mark
        | 0x06DD            // Arabic End of Ayah
        | 0x070F            // Syriac Abbreviation Mark
        | 0x0890..=0x0891   // Arabic pound / piastre marks
        | 0x08E2            // Arabic disputed end of ayah
        | 0x180E            // Mongolian vowel separator
        | 0x200B..=0x200F   // zero-width + LTR/RTL marks
        | 0x202A..=0x202E   // directional formatting
        | 0x2060..=0x2064   // word joiner + invisible operators
        | 0x2066..=0x206F   // directionality + deprecated
        | 0xFEFF            // ZERO WIDTH NO-BREAK SPACE / BOM
        | 0xFFF9..=0xFFFB   // interlinear annotation
        | 0x110BD           // Kaithi number sign
        | 0x110CD           // Kaithi number sign above
        | 0x13430..=0x1343F // Egyptian Hieroglyph format controls
        | 0x1BCA0..=0x1BCA3 // Shorthand format controls
        | 0x1D173..=0x1D17A // Musical symbol begin/end
        | 0xE0001           // Language tag
        | 0xE0020..=0xE007F // Tags block
    )
}

/// §20b — `tengu_mcp_server_config_invalid`: a server's config failed the
/// loader-time or connect-time URL/shape re-validation. Oracle call site:
/// `s("tengu_mcp_server_config_invalid",{transportType:c(t.type??"stdio"),
/// field:w("url"),source:w(t.configError?"loader":"connect")})` — `field` is
/// always the literal `"url"`, the sole re-validation target either gate
/// checks (see [`McpServerConfig::config_error`] /
/// [`McpServerConfig::connect_time_url_error`]'s docs for the two gates this
/// fires from).
/// §20b — build `tengu_mcp_tools_listed`'s payload. Pure: takes the already
/// resolved/filtered tool list and elapsed duration rather than reaching
/// into `self`/`conn`, so the field-mapping (`tool_count`/`always_load_count`
/// off the FINAL post-§20a-filter list, not the raw transport response) is
/// unit-testable without standing up a mock transport.
///
/// `discovery_source` is unconditionally `"live"` here — this is the
/// CONNECT path (a fresh dial), never the cached-row-adoption path the
/// oracle's `discoverySource` also covers (§18's deferred
/// `cached-row adopt subscriber threw` item; this port has no cached-row
/// adoption at all yet).
/// Oracle `yn`'s FIRST statement (@182316780):
/// `if(u.length===0&&r==="live")s("tengu_mcp_degraded",{reason:w("connected_zero_tools"),…})`.
///
/// Pure and separate from the `connect` body so the three conditions are
/// unit-testable without standing up a transport:
///
/// * `u.length === 0` — `u` is the RAW `tools/list` response, so emptiness is
///   measured BEFORE the §20a schema filter runs. A server whose every tool
///   that filter dropped reports its drop reason, NOT zero-tools.
/// * `r === "live"` — always true on this path (a fresh dial); the oracle's
///   cached-row adoption path, which also feeds `yn`, does not exist here.
/// * `caps.tools` — with no tools capability the oracle never reaches `yn`,
///   so an empty list there is not a degraded signal (same gate as
///   `tengu_mcp_tools_listed`).
fn connected_zero_tools_fires(caps_tools: bool, raw_tool_count: usize) -> bool {
    caps_tools && raw_tool_count == 0
}

fn tools_listed_payload(
    transport_kind: &str,
    elapsed: std::time::Duration,
    tools: &[traits::McpToolDto],
    server_name: &str,
) -> telemetry::tengu::mcp::ToolsListedPayload {
    use telemetry::pii::Verified;
    telemetry::tengu::mcp::ToolsListedPayload {
        transport_type: Verified::assert_safe(transport_kind.to_string()),
        list_duration_ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
        tool_count: u32::try_from(tools.len()).unwrap_or(u32::MAX),
        always_load_count: u32::try_from(
            tools.iter().filter(|t| t.always_load == Some(true)).count(),
        )
        .unwrap_or(u32::MAX),
        discovery_source: Verified::assert_safe("live".to_string()),
        // Oracle: `mcpServerName:EA(ln(e.name),HT(e.name,e.config))` — `EA`
        // returns `undefined` (the spread DROPS the key) unless the
        // first-party gate holds. Emitting the raw name unconditionally, as
        // an earlier revision did, made a user's private server name an
        // analytics dimension on every connect. See
        // `telemetry::tengu::mcp::server_name_gate`.
        mcp_server_name: telemetry::tengu::mcp::server_name_gate(transport_kind)
            .then(|| Verified::assert_safe(server_name.to_string())),
    }
}

fn emit_server_config_invalid(
    config: &McpServerConfig,
    source: telemetry::tengu::mcp::ConfigInvalidSource,
) {
    telemetry::emit_mcp_server_config_invalid(&server_config_invalid_payload(config, source));
}

/// Pure payload-building half of [`emit_server_config_invalid`] — split out
/// so the loader-vs-connect classification is unit-testable directly,
/// without a tracing-capture race (see `degraded_payloads_for_server`'s doc
/// for why that race is real in this shared test binary).
fn server_config_invalid_payload(
    config: &McpServerConfig,
    source: telemetry::tengu::mcp::ConfigInvalidSource,
) -> telemetry::tengu::mcp::ServerConfigInvalidPayload {
    use telemetry::pii::Verified;
    telemetry::tengu::mcp::ServerConfigInvalidPayload {
        transport_type: Verified::assert_safe(config.spec.kind().to_string()),
        field: Verified::assert_safe("url".to_string()),
        source,
    }
}

/// §20b — build the `tengu_mcp_degraded` payload for every NONZERO bucket in
/// one server's tallied tool-schema classification counts. Pure and
/// deterministic (no telemetry emission, no tracing) so the aggregation
/// logic — which count-field a reason maps to, and that every nonzero
/// bucket becomes exactly one payload — is unit-testable directly, without
/// racing a concurrently-running test's `tracing` subscriber over the
/// process-global callsite `Interest` cache (see the call site's doc for
/// why that race is real, not hypothetical).
fn degraded_payloads_for_server(
    counts: &std::collections::HashMap<telemetry::tengu::mcp::DegradedReason, u32>,
    transport_kind: &str,
    server_name: &str,
) -> Vec<telemetry::tengu::mcp::DegradedPayload> {
    use telemetry::pii::Verified;
    use telemetry::tengu::mcp::{DegradedPayload, DegradedReason};

    if counts.is_empty() {
        return Vec::new();
    }
    let transport_type = Verified::assert_safe(transport_kind.to_string());
    // Same gate as `tools_listed_payload` — the oracle spreads the SAME `P`
    // into every per-server `tengu_mcp_degraded`.
    let mcp_server_name = telemetry::tengu::mcp::server_name_gate(transport_kind)
        .then(|| Verified::assert_safe(server_name.to_string()));
    let mut out = Vec::with_capacity(counts.len());
    for (reason, count) in counts {
        let (normalized_count, skipped_count, kept_count) = match reason {
            // Oracle's `connected_zero_tools` payload is
            // `{reason,transportType,mcpServerName,..._}` — no count field.
            DegradedReason::ConnectedZeroTools => (None, None, None),
            DegradedReason::ToolSchemaNormalized => (Some(*count), None, None),
            DegradedReason::ToolSchemaNormalizeGated
            | DegradedReason::ToolSchemaUnsupported
            | DegradedReason::ToolSchemaInvalid
            | DegradedReason::ToolPropertyKeyInvalid => (None, Some(*count), None),
            DegradedReason::ToolSchemaInvalidGated
            | DegradedReason::ToolPropertyKeyInvalidGated => (None, None, Some(*count)),
            // `SchemaValidatorUnavailable` is process-global (fired from
            // `tool_schema::meta_validator`, never tallied into this
            // per-server map) and the enum is `#[non_exhaustive]` — a future
            // oracle-confirmed sibling with no known count-field mapping
            // falls here too, skipped rather than guessed at.
            _ => continue,
        };
        out.push(DegradedPayload {
            reason: *reason,
            transport_type: Some(transport_type.clone()),
            normalized_count,
            skipped_count,
            kept_count,
            mcp_server_name: mcp_server_name.clone(),
        });
    }
    out
}

/// Whether a state's config is flagged `disabled` (mid-reconnect guard).
fn state_is_disabled(state: &McpConnectionState) -> bool {
    match state {
        McpConnectionState::Disconnected { config, .. }
        | McpConnectionState::Connecting { config, .. }
        | McpConnectionState::AwaitingOAuth { config, .. }
        | McpConnectionState::Connected { config, .. }
        | McpConnectionState::HealthChecking { config, .. }
        | McpConnectionState::Reconnecting { config, .. }
        | McpConnectionState::Failed { config, .. }
        | McpConnectionState::Stopped { config } => config.disabled,
    }
}

/// Project a [`McpConnectionState`] onto the fine-grained
/// [`traits::McpActionState`] used by the `/mcp reconnect|enable|disable`
/// action handler — a faithful mirror of claude-code's client `type`
/// discriminant. The `config.disabled` gate takes precedence (a disabled
/// server reports `"disabled"` regardless of its last live state), then:
/// `Connected`/`HealthChecking` → connected, `Connecting`/`Reconnecting` →
/// pending, `AwaitingOAuth` → needs-auth, everything else (`Failed`,
/// `Disconnected`, `Stopped`) → failed ("not connected").
fn project_action_state(state: &McpConnectionState) -> traits::McpActionState {
    use traits::McpActionState;
    if state_is_disabled(state) {
        return McpActionState::Disabled;
    }
    match state {
        McpConnectionState::Connected { .. } | McpConnectionState::HealthChecking { .. } => {
            McpActionState::Connected
        }
        McpConnectionState::Connecting { .. } | McpConnectionState::Reconnecting { .. } => {
            McpActionState::Pending
        }
        McpConnectionState::AwaitingOAuth { .. } => McpActionState::NeedsAuth,
        McpConnectionState::Failed { .. }
        | McpConnectionState::Disconnected { .. }
        | McpConnectionState::Stopped { .. } => McpActionState::Failed,
    }
}

/// Project a [`McpConnectionState`] variant onto the trait-facing
/// [`traits::McpStatus`] (M6-07).
fn project_status(state: &McpConnectionState) -> traits::McpStatus {
    use traits::McpStatus;
    match state {
        McpConnectionState::Connected { .. } => McpStatus::Connected,
        McpConnectionState::Disconnected {
            last_error: Some(e),
            ..
        } => McpStatus::Error(e.clone()),
        McpConnectionState::Disconnected { .. }
        | McpConnectionState::Connecting { .. }
        | McpConnectionState::AwaitingOAuth { .. }
        | McpConnectionState::HealthChecking { .. }
        | McpConnectionState::Reconnecting { .. }
        | McpConnectionState::Stopped { .. } => McpStatus::Disconnected,
        McpConnectionState::Failed { error, .. } => McpStatus::Error(error.clone()),
    }
}

#[cfg(test)]
mod tests {
    //! Bridge (Batch 1) + FQN-rewrite / normalize-match (Batch 2) tests.
    //!
    //! A broader mock-transport lifecycle test lives in
    //! `test-harness/tests/mcp_lifecycle.rs`.
    use super::*;
    use crate::connection::{ConfigScope, McpServerConfig};
    use async_trait::async_trait;
    use bytes::Bytes;
    use jsonrpc::{Connection, Mode};
    use protocol::McpConnectionId as ConnId;
    use serde_json::Value;
    use std::future::Future;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex as TestMutex;
    use std::task::{Context, Poll, Wake, Waker};
    use tokio::sync::{mpsc, Notify};
    use traits::{
        ElicitRequestDto, ElicitResultDto, McpError, McpNotificationStream, McpPromptDto,
        McpRawConnection, McpResourceContentDto, McpResourceDto, McpToolDto, McpToolResultDto,
        McpTransport, McpTransportKind, McpTransportSpec, ServerCapabilitiesDto,
    };

    /// Build a `Connection` over a fresh pair of `mpsc<Bytes>` channels (the
    /// `paired_connection` pattern from `client.rs:536`); the peer ends are
    /// dropped — these tests only assert client *presence*, not round-trips.
    fn paired_connection() -> Arc<Connection> {
        let (_peer_to_us_tx, peer_to_us_rx) = mpsc::channel::<Bytes>(8);
        let (us_to_peer_tx, _us_to_peer_rx) = mpsc::channel::<Bytes>(8);
        Arc::new(Connection::new_streams(
            peer_to_us_rx,
            us_to_peer_tx,
            Mode::Lines,
        ))
    }

    /// Paired connection retaining the mock-server ends for notification and
    /// request/response catalog-refresh tests.
    #[allow(clippy::type_complexity)]
    fn drivable_connection() -> (Arc<Connection>, mpsc::Sender<Bytes>, mpsc::Receiver<Bytes>) {
        let (peer_to_us_tx, peer_to_us_rx) = mpsc::channel::<Bytes>(8);
        let (us_to_peer_tx, us_to_peer_rx) = mpsc::channel::<Bytes>(8);
        (
            Arc::new(Connection::new_streams(
                peer_to_us_rx,
                us_to_peer_tx,
                Mode::Lines,
            )),
            peer_to_us_tx,
            us_to_peer_rx,
        )
    }

    /// Functional mock transport that ALSO bridges a paired in-memory
    /// `jsonrpc::Connection` through [`RawConnectionProvider`].
    ///
    /// On `connect` it mints a fresh id, stashes a paired connection under it,
    /// and returns canned tools (with the empty `<server>` token the real
    /// transport emits, so the registry's rewrite is exercised).
    struct BridgeMock {
        tools: Vec<McpToolDto>,
        // §26a — canned `resources/templates/list` rows and the capability
        // presence bit that gates whether `connect` fetches them at all
        // (`initialize` reports `resources: true` only when this is set).
        resource_templates: Vec<traits::McpResourceTemplateDto>,
        resources_capability: AtomicBool,
        list_resource_templates_fails: AtomicBool,
        conns: TestMutex<HashMap<ConnId, Arc<Connection>>>,
        list_tools_fails: AtomicBool,
        disconnect_fails: AtomicBool,
        block_disconnect: AtomicBool,
        disconnect_started: Notify,
        disconnect_release: Notify,
    }

    impl BridgeMock {
        fn new(tool_names: &[&str]) -> Self {
            let tools = tool_names
                .iter()
                .map(|t| McpToolDto {
                    // Emit the empty `<server>` token, exactly like the posix
                    // transport's `list_tools` (mcp.rs:397) does.
                    full_name: format!("mcp____{t}"),
                    server_name: String::new(),
                    tool_name: (*t).to_string(),
                    description: format!("{t} tool"),
                    input_schema: serde_json::json!({"type": "object"}),
                    search_hint: None,
                    always_load: None,
                    requires_user_interaction: false,
                })
                .collect();
            Self {
                tools,
                resource_templates: Vec::new(),
                resources_capability: AtomicBool::new(false),
                list_resource_templates_fails: AtomicBool::new(false),
                conns: TestMutex::new(HashMap::new()),
                list_tools_fails: AtomicBool::new(false),
                disconnect_fails: AtomicBool::new(false),
                block_disconnect: AtomicBool::new(false),
                disconnect_started: Notify::new(),
                disconnect_release: Notify::new(),
            }
        }

        /// A mock whose server advertises the `resources` capability and
        /// answers `resources/templates/list` with `templates` (§26a).
        fn with_resource_templates(templates: Vec<traits::McpResourceTemplateDto>) -> Self {
            let mock = Self::new(&[]);
            mock.resources_capability.store(true, Ordering::SeqCst);
            Self {
                resource_templates: templates,
                ..mock
            }
        }

        /// Same as [`Self::new`], but each tool carries a caller-supplied
        /// `inputSchema` so the §20a connect-path decision can be driven.
        fn with_tool_schemas(tools: &[(&str, serde_json::Value)]) -> Self {
            let mut mock = Self::new(&[]);
            mock.tools = tools
                .iter()
                .map(|(name, schema)| McpToolDto {
                    full_name: format!("mcp____{name}"),
                    server_name: String::new(),
                    tool_name: (*name).to_string(),
                    description: format!("{name} tool"),
                    input_schema: schema.clone(),
                    search_hint: None,
                    always_load: None,
                    requires_user_interaction: false,
                })
                .collect();
            mock
        }
    }

    #[async_trait]
    impl McpTransport for BridgeMock {
        async fn connect(&self, _s: &McpTransportSpec) -> Result<McpRawConnection, McpError> {
            let id = ConnId::new();
            self.conns.lock().unwrap().insert(id, paired_connection());
            Ok(McpRawConnection { connection_id: id })
        }
        async fn initialize(
            &self,
            _c: &McpRawConnection,
        ) -> Result<ServerCapabilitiesDto, McpError> {
            Ok(ServerCapabilitiesDto {
                tools: true,
                resources: self.resources_capability.load(Ordering::SeqCst),
                prompts: false,
                logging: false,
                experimental: HashMap::new(),
            })
        }
        async fn list_resource_templates(
            &self,
            _c: &McpRawConnection,
        ) -> Result<Vec<traits::McpResourceTemplateDto>, McpError> {
            if self.list_resource_templates_fails.load(Ordering::SeqCst) {
                // What a server with no `resources/templates/list` handler
                // really replies: JSON-RPC -32601, which the posix transport
                // flattens through `map_call_err` into `McpError::Internal`.
                return Err(McpError::Internal(
                    "MCP error -32601: Method not found".into(),
                ));
            }
            Ok(self.resource_templates.clone())
        }
        async fn list_tools(&self, _c: &McpRawConnection) -> Result<Vec<McpToolDto>, McpError> {
            if self.list_tools_fails.load(Ordering::SeqCst) {
                return Err(McpError::Internal("list tools failed".into()));
            }
            Ok(self.tools.clone())
        }
        async fn list_resources(
            &self,
            _c: &McpRawConnection,
        ) -> Result<Vec<McpResourceDto>, McpError> {
            Ok(Vec::new())
        }
        async fn list_prompts(&self, _c: &McpRawConnection) -> Result<Vec<McpPromptDto>, McpError> {
            Ok(Vec::new())
        }
        async fn call_tool(
            &self,
            _c: &McpRawConnection,
            _t: &str,
            _i: Value,
        ) -> Result<McpToolResultDto, McpError> {
            Ok(McpToolResultDto {
                content: Value::Null,
                is_error: false,
                ..Default::default()
            })
        }
        async fn read_resource(
            &self,
            _c: &McpRawConnection,
            _u: &str,
        ) -> Result<McpResourceContentDto, McpError> {
            Err(McpError::Internal("not implemented".into()))
        }
        async fn ping(&self, _id: ConnId) -> Result<(), McpError> {
            Ok(())
        }
        async fn notifications(
            &self,
            _c: &McpRawConnection,
        ) -> Result<McpNotificationStream, McpError> {
            // Never exercised by these tests (the registry's connect path does
            // not subscribe to notifications).
            unreachable!("notifications not used by bridge/FQN tests")
        }
        async fn handle_elicitation(
            &self,
            _c: &McpRawConnection,
            _r: ElicitRequestDto,
        ) -> Result<ElicitResultDto, McpError> {
            Err(McpError::Internal("not implemented".into()))
        }
        async fn disconnect(&self, id: ConnId) -> Result<(), McpError> {
            self.disconnect_started.notify_one();
            if self.block_disconnect.load(Ordering::SeqCst) {
                self.disconnect_release.notified().await;
            }
            if self.disconnect_fails.load(Ordering::SeqCst) {
                return Err(McpError::Internal("disconnect failed".into()));
            }
            self.conns.lock().unwrap().remove(&id);
            Ok(())
        }
        fn supported_transports(&self) -> Vec<McpTransportKind> {
            vec![McpTransportKind::Stdio]
        }
    }

    impl RawConnectionProvider for BridgeMock {
        fn connection_for(&self, id: ConnId) -> Option<Arc<Connection>> {
            self.conns.lock().unwrap().get(&id).cloned()
        }
    }

    /// Same-process transport used to prove that an `InProcess` catalog is
    /// callable without manufacturing a JSON-RPC client solely for dispatch.
    struct DirectInProcessMock {
        connection_id: ConnId,
        calls: TestMutex<Vec<(String, Value)>>,
    }

    impl DirectInProcessMock {
        fn new() -> Self {
            Self {
                connection_id: ConnId::new(),
                calls: TestMutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl McpTransport for DirectInProcessMock {
        async fn connect(&self, spec: &McpTransportSpec) -> Result<McpRawConnection, McpError> {
            assert!(
                matches!(spec, McpTransportSpec::InProcess { registry_key } if registry_key == "local_apps")
            );
            Ok(McpRawConnection {
                connection_id: self.connection_id,
            })
        }

        async fn initialize(
            &self,
            _conn: &McpRawConnection,
        ) -> Result<ServerCapabilitiesDto, McpError> {
            Ok(ServerCapabilitiesDto {
                tools: true,
                resources: false,
                prompts: false,
                logging: false,
                experimental: HashMap::new(),
            })
        }

        async fn list_tools(&self, _conn: &McpRawConnection) -> Result<Vec<McpToolDto>, McpError> {
            Ok(vec![McpToolDto {
                server_name: String::new(),
                tool_name: "list".into(),
                description: "list local apps".into(),
                input_schema: serde_json::json!({"type":"object"}),
                full_name: String::new(),
                search_hint: None,
                always_load: Some(true),
                requires_user_interaction: false,
            }])
        }

        async fn list_resources(
            &self,
            _conn: &McpRawConnection,
        ) -> Result<Vec<McpResourceDto>, McpError> {
            Ok(Vec::new())
        }

        async fn list_prompts(
            &self,
            _conn: &McpRawConnection,
        ) -> Result<Vec<McpPromptDto>, McpError> {
            Ok(Vec::new())
        }

        async fn call_tool(
            &self,
            _conn: &McpRawConnection,
            tool: &str,
            input: Value,
        ) -> Result<McpToolResultDto, McpError> {
            self.calls
                .lock()
                .unwrap()
                .push((tool.into(), input.clone()));
            Ok(McpToolResultDto {
                content: serde_json::json!([{"type":"text","text":"ok"}]),
                structured_content: Some(serde_json::json!({"tool": tool, "input": input})),
                is_error: false,
                ..Default::default()
            })
        }

        async fn read_resource(
            &self,
            _conn: &McpRawConnection,
            _uri: &str,
        ) -> Result<McpResourceContentDto, McpError> {
            Err(McpError::Internal("resources disabled".into()))
        }

        async fn ping(&self, _connection_id: ConnId) -> Result<(), McpError> {
            Ok(())
        }

        async fn notifications(
            &self,
            _conn: &McpRawConnection,
        ) -> Result<McpNotificationStream, McpError> {
            unreachable!("direct registry connections do not subscribe to JSON-RPC notifications")
        }

        async fn handle_elicitation(
            &self,
            _conn: &McpRawConnection,
            _request: ElicitRequestDto,
        ) -> Result<ElicitResultDto, McpError> {
            Err(McpError::Internal("elicitation disabled".into()))
        }

        async fn disconnect(&self, _connection_id: ConnId) -> Result<(), McpError> {
            Ok(())
        }

        fn supported_transports(&self) -> Vec<McpTransportKind> {
            vec![McpTransportKind::InProcess]
        }
    }

    struct NoopWake;

    impl Wake for NoopWake {
        fn wake(self: Arc<Self>) {}
    }

    fn cfg(name: &str) -> McpServerConfig {
        McpServerConfig {
            name: name.into(),
            spec: McpTransportSpec::Stdio {
                command: "echo".into(),
                args: vec![],
                env: HashMap::new(),
            },
            scope: ConfigScope::Project,
            disabled: false,
            timeout_ms: None,
            always_load: false,
            config_error: None,
        }
    }

    /// [`cfg`] with a remote `http` spec, so the §20a per-server gate has a
    /// hostname to resolve.
    fn http_cfg(name: &str, url: &str) -> McpServerConfig {
        McpServerConfig {
            spec: McpTransportSpec::Http {
                url: url.into(),
                headers: Default::default(),
                headers_helper: None,
                oauth: None,
            },
            ..cfg(name)
        }
    }

    // ---- Batch 1: client bridge --------------------------------------------

    #[tokio::test]
    async fn connect_registers_client_when_raw_conn_present() {
        let mock = Arc::new(BridgeMock::new(&["read"]));
        let registry = McpRegistry::with_raw_conn(
            mock.clone() as Arc<dyn McpTransport>,
            mock as Arc<dyn RawConnectionProvider>,
        );
        registry.connect(cfg("mock")).await.unwrap();
        assert!(
            registry.get_client("mock").await.is_some(),
            "a live McpClient must be registered when raw_conn is present"
        );
    }

    #[tokio::test]
    async fn inprocess_server_dispatches_directly_without_jsonrpc_client() {
        let transport = Arc::new(DirectInProcessMock::new());
        let registry = McpRegistry::new(transport.clone());
        registry
            .connect(McpServerConfig {
                name: "local_apps".into(),
                spec: McpTransportSpec::InProcess {
                    registry_key: "local_apps".into(),
                },
                scope: ConfigScope::Managed,
                disabled: false,
                timeout_ms: None,
                always_load: true,
                config_error: None,
            })
            .await
            .unwrap();

        assert!(registry.get_client("local_apps").await.is_none());
        assert!(registry.has_callable_server("local_apps").await);
        let result = registry
            .call_tool_with_auth_retry(
                "local_apps",
                "mcp__local_apps__list",
                serde_json::json!({"limit": 5}),
                None,
                None,
            )
            .await
            .unwrap();

        assert_eq!(
            result.structured_content,
            Some(serde_json::json!({"tool":"list","input":{"limit":5}}))
        );
        assert_eq!(
            *transport.calls.lock().unwrap(),
            vec![("list".into(), serde_json::json!({"limit": 5}))]
        );
    }

    #[tokio::test]
    async fn catalog_failure_disconnects_transport_and_records_retryable_state() {
        let mock = Arc::new(BridgeMock::new(&["read"]));
        mock.list_tools_fails.store(true, Ordering::SeqCst);
        let registry = McpRegistry::with_raw_conn(
            mock.clone() as Arc<dyn McpTransport>,
            mock.clone() as Arc<dyn RawConnectionProvider>,
        );

        let error = registry.connect(cfg("mock")).await.unwrap_err();
        assert!(error.to_string().contains("list tools failed"));
        assert!(mock.conns.lock().unwrap().is_empty());
        let states = registry.connections.read().await;
        assert!(matches!(
            states.get("mock"),
            Some(McpConnectionState::Disconnected {
                last_error: Some(message),
                ..
            }) if message.contains("list tools failed")
        ));
    }

    #[tokio::test]
    async fn connect_and_disconnect_publish_tool_partition_lifecycle() {
        let mock = Arc::new(BridgeMock::new(&["read"]));
        let registry = McpRegistry::with_raw_conn(
            mock.clone() as Arc<dyn McpTransport>,
            mock as Arc<dyn RawConnectionProvider>,
        );
        let mut changes = registry.subscribe_catalog_changes();

        let connection_id = registry.connect(cfg("mock")).await.unwrap();
        let connected = tokio::time::timeout(Duration::from_secs(2), changes.recv())
            .await
            .expect("connect catalog event within timeout")
            .expect("catalog sender remains live");
        assert_eq!(
            connected,
            McpCatalogChanged {
                server_name: "mock".into(),
                connection_id,
                retired_connection_id: None,
                kind: McpCatalogKind::Tools,
            }
        );

        registry.disconnect("mock").await.unwrap();
        let disconnected = tokio::time::timeout(Duration::from_secs(2), changes.recv())
            .await
            .expect("disconnect catalog event within timeout")
            .expect("catalog sender remains live");
        assert_eq!(
            disconnected,
            McpCatalogChanged {
                server_name: "mock".into(),
                connection_id,
                retired_connection_id: Some(connection_id),
                kind: McpCatalogKind::Tools,
            }
        );
    }

    #[tokio::test]
    async fn failed_disconnect_preserves_live_state_client_and_catalog() {
        let mock = Arc::new(BridgeMock::new(&["read"]));
        let registry = McpRegistry::with_raw_conn(
            mock.clone() as Arc<dyn McpTransport>,
            mock.clone() as Arc<dyn RawConnectionProvider>,
        );
        let mut changes = registry.subscribe_catalog_changes();

        let connection_id = registry.connect(cfg("mock")).await.unwrap();
        changes.recv().await.unwrap();
        mock.disconnect_fails.store(true, Ordering::SeqCst);

        let error = registry.disconnect("mock").await.unwrap_err();
        assert!(error.to_string().contains("disconnect failed"));
        assert!(registry.get_client("mock").await.is_some());
        let conns = registry.connections.read().await;
        assert!(matches!(
            conns.get("mock"),
            Some(McpConnectionState::Connected {
                connection_id: current,
                ..
            }) if *current == connection_id
        ));
        drop(conns);
        assert!(
            tokio::time::timeout(Duration::from_millis(50), changes.recv())
                .await
                .is_err(),
            "a failed teardown must not retire the live tool partition"
        );
    }

    #[tokio::test]
    async fn live_disable_retires_connection_without_revoking_reconnect_config() {
        let mock = Arc::new(BridgeMock::new(&["read"]));
        let registry = McpRegistry::with_raw_conn(
            mock.clone() as Arc<dyn McpTransport>,
            mock as Arc<dyn RawConnectionProvider>,
        );
        let mut changes = registry.subscribe_catalog_changes();
        let connection_id = registry.connect(cfg("mock")).await.unwrap();
        changes.recv().await.unwrap();

        assert_eq!(
            registry.set_disabled("mock", true).await.unwrap(),
            Some(traits::McpActionState::Disabled)
        );
        assert!(registry.get_client("mock").await.is_none());
        assert_eq!(
            registry.action_states().await,
            vec![("mock".to_string(), traits::McpActionState::Disabled)]
        );
        let retired = changes.recv().await.unwrap();
        assert_eq!(retired.retired_connection_id, Some(connection_id));

        assert_eq!(
            registry.set_disabled("mock", false).await.unwrap(),
            Some(traits::McpActionState::Connected)
        );
        assert!(registry.get_client("mock").await.is_some());
        assert_eq!(
            registry.action_states().await,
            vec![("mock".to_string(), traits::McpActionState::Connected)]
        );
    }

    #[tokio::test]
    async fn failed_live_disable_preserves_connected_generation() {
        let mock = Arc::new(BridgeMock::new(&["read"]));
        let registry = McpRegistry::with_raw_conn(
            mock.clone() as Arc<dyn McpTransport>,
            mock.clone() as Arc<dyn RawConnectionProvider>,
        );
        let connection_id = registry.connect(cfg("mock")).await.unwrap();
        mock.disconnect_fails.store(true, Ordering::SeqCst);

        assert!(registry.set_disabled("mock", true).await.is_err());
        assert!(registry.get_client("mock").await.is_some());
        let conns = registry.connections.read().await;
        assert!(matches!(
            conns.get("mock"),
            Some(McpConnectionState::Connected {
                connection_id: current,
                config,
                ..
            }) if *current == connection_id && !config.disabled
        ));
    }

    #[tokio::test]
    async fn enabling_a_server_that_fails_to_connect_settles_as_failed_not_error() {
        let mock = Arc::new(BridgeMock::new(&["read"]));
        let registry = McpRegistry::with_raw_conn(
            mock.clone() as Arc<dyn McpTransport>,
            mock.clone() as Arc<dyn RawConnectionProvider>,
        );
        registry.connect(cfg("mock")).await.unwrap();
        // Disabling settles as `Disabled`.
        assert_eq!(
            registry.set_disabled("mock", true).await.unwrap(),
            Some(traits::McpActionState::Disabled)
        );
        // The next connect will fail (the catalog fetch errors out).
        mock.list_tools_fails.store(true, Ordering::SeqCst);
        // Re-enabling must NOT return `Err` — claude's `u(name)` fulfills with
        // `{type:"failed"}` rather than rejecting. The registry flips the server
        // on but reads it back as `Failed` ("not connected"), so the /mcp handler
        // can render "Enabled …, but it isn't connected yet." instead of erroring.
        assert_eq!(
            registry.set_disabled("mock", false).await.unwrap(),
            Some(traits::McpActionState::Failed)
        );
        assert_eq!(
            registry.action_states().await,
            vec![("mock".to_string(), traits::McpActionState::Failed)]
        );
    }

    #[tokio::test]
    async fn failed_action_servers_reports_sanitized_errors_sorted() {
        let mock = Arc::new(BridgeMock::new(&[]));
        let registry = McpRegistry::with_raw_conn(
            mock.clone() as Arc<dyn McpTransport>,
            mock as Arc<dyn RawConnectionProvider>,
        );
        {
            let mut conns = registry.connections.write().await;
            conns.insert(
                "zeta".into(),
                McpConnectionState::Failed {
                    config: cfg("zeta"),
                    // The quotes must be stripped by the `xLt` sanitizer.
                    error: "he said \"boom\"".into(),
                    attempts: 3,
                },
            );
            conns.insert(
                "alpha".into(),
                McpConnectionState::Failed {
                    config: cfg("alpha"),
                    error: "Blocked by enterprise managed policy".into(),
                    attempts: 1,
                },
            );
        }
        // Sorted by name; each error run through `sanitize_diagnostic`.
        assert_eq!(
            registry.failed_action_servers().await,
            vec![
                (
                    "alpha".to_string(),
                    Some("Blocked by enterprise managed policy".to_string())
                ),
                ("zeta".to_string(), Some("he said boom".to_string())),
            ]
        );
    }

    #[test]
    fn sanitize_diagnostic_strips_quotes_controls_and_caps_at_200() {
        // Angle brackets, `"`, `;` and the fancy-quote set collapse to spaces.
        assert_eq!(sanitize_diagnostic("hello \"world\""), "hello world");
        assert_eq!(sanitize_diagnostic("angle <b> ; semi"), "angle b semi");
        // Control chars (`\p{Cc}`) collapse to a single space.
        assert_eq!(sanitize_diagnostic("a\u{0000}\u{0007}b"), "a b");
        // Runs of whitespace collapse and the result is trimmed.
        assert_eq!(sanitize_diagnostic("  trim   me  "), "trim me");
        // Capped at 200 chars + `…` (U+2026).
        let capped = sanitize_diagnostic(&"x".repeat(250));
        assert_eq!(capped.chars().count(), 201);
        assert!(capped.ends_with('\u{2026}'));
    }

    #[tokio::test]
    async fn slow_disconnect_does_not_block_registry_snapshots() {
        let mock = Arc::new(BridgeMock::new(&["read"]));
        let registry = Arc::new(McpRegistry::with_raw_conn(
            mock.clone() as Arc<dyn McpTransport>,
            mock.clone() as Arc<dyn RawConnectionProvider>,
        ));
        registry.connect(cfg("mock")).await.unwrap();
        mock.block_disconnect.store(true, Ordering::SeqCst);

        let disconnect = {
            let registry = Arc::clone(&registry);
            tokio::spawn(async move { registry.disconnect("mock").await })
        };
        mock.disconnect_started.notified().await;

        let snapshot = tokio::time::timeout(Duration::from_millis(50), registry.snapshot())
            .await
            .expect("transport teardown must not hold the connection-state lock");
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].status, traits::McpStatus::Connected);

        mock.disconnect_release.notify_one();
        disconnect.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn reconnect_one_does_not_clobber_an_already_connected_server() {
        let mock = Arc::new(BridgeMock::new(&["read"]));
        let registry = Arc::new(McpRegistry::with_raw_conn(
            mock.clone() as Arc<dyn McpTransport>,
            mock.clone() as Arc<dyn RawConnectionProvider>,
        ));
        let id1 = registry.connect(cfg("mock")).await.unwrap();

        // A stale reconnect candidate fires for a server that is now Connected
        // (a concurrent connect won the race). Under the fix it aborts on the
        // Connected guard instead of overwriting the live connection with
        // `Reconnecting` — which would strand `id1`, unreachable for teardown.
        Arc::clone(&registry).reconnect_one(cfg("mock")).await;

        let conns = registry.connections.read().await;
        match conns.get("mock") {
            Some(McpConnectionState::Connected { connection_id, .. }) => {
                assert_eq!(
                    *connection_id, id1,
                    "the live connection id must be preserved"
                );
            }
            other => panic!("expected the connection to stay Connected, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn reconnect_one_reconnects_a_disconnected_server() {
        // Regression: the reconnect path still works for a genuinely
        // reconnect-worthy (non-Connected) state.
        let mock = Arc::new(BridgeMock::new(&["read"]));
        let registry = Arc::new(McpRegistry::with_raw_conn(
            mock.clone() as Arc<dyn McpTransport>,
            mock.clone() as Arc<dyn RawConnectionProvider>,
        ));
        registry.connections.write().await.insert(
            "mock".into(),
            McpConnectionState::Disconnected {
                config: cfg("mock"),
                last_error: Some("boom".into()),
            },
        );

        Arc::clone(&registry).reconnect_one(cfg("mock")).await;

        let conns = registry.connections.read().await;
        match conns.get("mock") {
            Some(McpConnectionState::Connected { .. }) => {}
            other => panic!("expected reconnect to reach Connected, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn inbound_tools_list_changed_is_forwarded_with_connection_generation() {
        let mock = Arc::new(BridgeMock::new(&[]));
        let registry = McpRegistry::new(mock as Arc<dyn McpTransport>);
        let (connection, peer_tx, _peer_rx) = drivable_connection();
        let connection_id = ConnId::new();
        let mut changes = registry.subscribe_catalog_changes();
        registry.spawn_catalog_change_listener("srv".into(), connection_id, connection);

        let mut frame = serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/tools/list_changed",
            "params": {}
        }))
        .unwrap();
        frame.push(b'\n');
        peer_tx.send(Bytes::from(frame)).await.unwrap();

        let change = tokio::time::timeout(Duration::from_secs(2), changes.recv())
            .await
            .expect("catalog notification within timeout")
            .expect("catalog sender remains live");
        assert_eq!(
            change,
            McpCatalogChanged {
                server_name: "srv".into(),
                connection_id,
                retired_connection_id: None,
                kind: McpCatalogKind::Tools,
            }
        );
    }

    #[tokio::test]
    async fn refresh_tools_catalog_replaces_connected_snapshot_after_success() {
        let mock = Arc::new(BridgeMock::new(&[]));
        let registry = Arc::new(McpRegistry::new(mock as Arc<dyn McpTransport>));
        let (connection, peer_tx, mut peer_rx) = drivable_connection();
        let client = Arc::new(
            McpClient::new("srv", std::path::PathBuf::from("/tmp/work"), connection).await,
        );
        let connection_id = ConnId::new();
        registry.clients.write().await.insert(
            "srv".into(),
            RegisteredClient {
                connection_id: Some(connection_id),
                client,
            },
        );
        registry.connections.write().await.insert(
            "srv".into(),
            McpConnectionState::Connected {
                config: cfg("srv"),
                connection_id,
                capabilities: ServerCapabilitiesDto {
                    tools: true,
                    resources: false,
                    prompts: false,
                    logging: false,
                    experimental: HashMap::new(),
                },
                tools: BridgeMock::new(&["old"]).tools,
                resources: Vec::new(),
                resource_templates: Vec::new(),
                prompts: Vec::new(),
                connected_at: SystemTime::now(),
            },
        );

        let change = McpCatalogChanged {
            server_name: "srv".into(),
            connection_id,
            retired_connection_id: None,
            kind: McpCatalogKind::Tools,
        };
        let refresh_registry = registry.clone();
        let refresh = tokio::spawn(async move { refresh_registry.refresh_catalog(&change).await });
        let request_frame = tokio::time::timeout(Duration::from_secs(2), peer_rx.recv())
            .await
            .expect("tools/list request within timeout")
            .expect("tools/list frame");
        let request: Value = serde_json::from_slice(&request_frame).unwrap();
        assert_eq!(request["method"], "tools/list");
        let mut response = serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": request["id"].clone(),
            "result": {
                "tools": [{
                    "name": "new tool",
                    "description": "fresh",
                    "inputSchema": {"type": "object"}
                }]
            }
        }))
        .unwrap();
        response.push(b'\n');
        peer_tx.send(Bytes::from(response)).await.unwrap();

        assert_eq!(
            refresh.await.unwrap().unwrap(),
            Some(connection_id),
            "current connection generation must be refreshed"
        );
        let conns = registry.connections.read().await;
        let McpConnectionState::Connected { tools, .. } = conns.get("srv").unwrap() else {
            panic!("server must remain connected")
        };
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].tool_name, "new tool");
        assert_eq!(tools[0].full_name, "mcp__srv__new_tool");
    }

    #[tokio::test]
    async fn get_prompt_rejects_generation_swapped_client_after_validation() {
        let registry = Arc::new(McpRegistry::new(Arc::new(BridgeMock::new(&[]))));
        let (conn_a, mut peer_a) = observable_connection();
        let (conn_b, mut peer_b) = observable_connection();
        let client_a =
            Arc::new(McpClient::new("srv", std::path::PathBuf::from("/tmp/work"), conn_a).await);
        let client_b =
            Arc::new(McpClient::new("srv", std::path::PathBuf::from("/tmp/work"), conn_b).await);
        let old_id = ConnId::new();
        let new_id = ConnId::new();

        registry.connections.write().await.insert(
            "srv".into(),
            McpConnectionState::Connected {
                config: cfg("srv"),
                connection_id: old_id,
                capabilities: ServerCapabilitiesDto {
                    tools: false,
                    resources: false,
                    prompts: true,
                    logging: false,
                    experimental: HashMap::new(),
                },
                tools: Vec::new(),
                resources: Vec::new(),
                resource_templates: Vec::new(),
                prompts: vec![McpPromptDto {
                    name: "draft".into(),
                    description: None,
                    arguments: Vec::new(),
                }],
                connected_at: SystemTime::now(),
            },
        );
        let mut clients = registry.clients.write().await;
        clients.insert(
            "srv".into(),
            RegisteredClient {
                connection_id: Some(old_id),
                client: client_a,
            },
        );

        let mut get_prompt = std::pin::pin!(registry.get_prompt(
            old_id,
            "draft",
            serde_json::json!({ "topic": "release" })
        ));
        let waker = Waker::from(Arc::new(NoopWake));
        let mut cx = Context::from_waker(&waker);
        assert!(matches!(get_prompt.as_mut().poll(&mut cx), Poll::Pending));

        registry.connections.write().await.insert(
            "srv".into(),
            McpConnectionState::Connected {
                config: cfg("srv"),
                connection_id: new_id,
                capabilities: ServerCapabilitiesDto {
                    tools: false,
                    resources: false,
                    prompts: true,
                    logging: false,
                    experimental: HashMap::new(),
                },
                tools: Vec::new(),
                resources: Vec::new(),
                resource_templates: Vec::new(),
                prompts: vec![McpPromptDto {
                    name: "draft".into(),
                    description: None,
                    arguments: Vec::new(),
                }],
                connected_at: SystemTime::now(),
            },
        );
        clients.insert(
            "srv".into(),
            RegisteredClient {
                connection_id: Some(new_id),
                client: client_b,
            },
        );
        drop(clients);

        let error = get_prompt.await.unwrap_err();
        assert!(error.to_string().contains(&format!(
            "MCP prompt connection {old_id} is no longer active"
        )));
        assert!(
            peer_a.try_recv().is_err(),
            "the original generation must not receive prompts/get after replacement",
        );
        assert!(
            peer_b.try_recv().is_err(),
            "the new generation must not receive a stale prompts/get request",
        );
    }

    #[tokio::test]
    async fn connect_skips_client_when_raw_conn_none() {
        let mock = Arc::new(BridgeMock::new(&["read"]));
        // `new` wires NO RawConnectionProvider.
        let registry = McpRegistry::new(mock as Arc<dyn McpTransport>);
        registry.connect(cfg("mock")).await.unwrap();
        assert!(
            registry.get_client("mock").await.is_none(),
            "no client must be registered without a raw_conn bridge"
        );
    }

    #[tokio::test]
    async fn disconnect_drops_registered_client() {
        let mock = Arc::new(BridgeMock::new(&["read"]));
        let registry = McpRegistry::with_raw_conn(
            mock.clone() as Arc<dyn McpTransport>,
            mock as Arc<dyn RawConnectionProvider>,
        );
        registry.connect(cfg("mock")).await.unwrap();
        assert!(registry.get_client("mock").await.is_some());
        registry.disconnect("mock").await.unwrap();
        assert!(
            registry.get_client("mock").await.is_none(),
            "disconnect must drop the cached client"
        );
    }

    // ---- Batch 2: FQN rewrite + normalize-match ----------------------------

    #[tokio::test]
    async fn connect_rewrites_fqn_with_normalized_server() {
        let mock = Arc::new(BridgeMock::new(&["read"]));
        let registry = McpRegistry::with_raw_conn(
            mock.clone() as Arc<dyn McpTransport>,
            mock as Arc<dyn RawConnectionProvider>,
        );
        // Raw name `my.server` normalizes to `my_server` in the FQN, while the
        // stored key (and `server_name`) stay raw for `/mcp` display.
        registry.connect(cfg("my.server")).await.unwrap();

        let conns = registry.connections.read().await;
        let McpConnectionState::Connected { tools, .. } = conns.get("my.server").unwrap() else {
            panic!("expected Connected state");
        };
        assert_eq!(tools[0].full_name, "mcp__my_server__read");
        assert_eq!(tools[0].server_name, "my.server");
        drop(conns);

        // A model-supplied normalized `<server>` token resolves the raw key.
        assert!(registry.get_client("my_server").await.is_some());
        assert!(registry.get_config("my_server").await.is_some());
    }

    /// §26a — a server whose `initialize` declares the `resources` capability
    /// has its `resources/templates/list` fetched at connect time and stashed
    /// on the `Connected` state, exactly like `resources`/`tools`/`prompts`.
    /// Oracle gates `resources/templates/list` on the SAME capability as
    /// `resources/list` (@167690139), not a separate template bit — this is
    /// the connect-time "catalog fetch" the audit found entirely absent
    /// (registry.rs's fetch was `list_tools`/`list_resources`/`list_prompts`
    /// only).
    #[tokio::test]
    async fn connect_fetches_resource_templates_when_resources_capability_is_present() {
        let mock = Arc::new(BridgeMock::with_resource_templates(vec![
            traits::McpResourceTemplateDto {
                uri_template: "file:///{path}".into(),
                name: "file-template".into(),
                description: Some("A file on disk".into()),
                mime_type: Some("text/plain".into()),
            },
        ]));
        let registry = McpRegistry::with_raw_conn(
            mock.clone() as Arc<dyn McpTransport>,
            mock as Arc<dyn RawConnectionProvider>,
        );
        registry.connect(cfg("srv")).await.unwrap();

        let conns = registry.connections.read().await;
        let McpConnectionState::Connected {
            resource_templates, ..
        } = conns.get("srv").unwrap()
        else {
            panic!("expected Connected state");
        };
        assert_eq!(
            resource_templates.len(),
            1,
            "connect must fetch resources/templates/list when resources capability is present"
        );
        assert_eq!(resource_templates[0].uri_template, "file:///{path}");
        assert_eq!(resource_templates[0].name, "file-template");
        assert_eq!(
            resource_templates[0].description.as_deref(),
            Some("A file on disk")
        );
        assert_eq!(
            resource_templates[0].mime_type.as_deref(),
            Some("text/plain")
        );
    }

    /// The capability gate: without the `resources` capability the fetch must
    /// be SKIPPED entirely (mirrors the existing `resources`/`prompts` gates
    /// just above it), not merely returning empty because the mock had none.
    #[tokio::test]
    async fn connect_skips_resource_templates_fetch_without_resources_capability() {
        // `BridgeMock::new` reports `resources: false` from `initialize`, so
        // even a mock stocked with templates must yield none on connect.
        let mut mock = BridgeMock::new(&[]);
        mock.resource_templates = vec![traits::McpResourceTemplateDto {
            uri_template: "file:///{path}".into(),
            name: "unreachable".into(),
            description: None,
            mime_type: None,
        }];
        let mock = Arc::new(mock);
        let registry = McpRegistry::with_raw_conn(
            mock.clone() as Arc<dyn McpTransport>,
            mock as Arc<dyn RawConnectionProvider>,
        );
        registry.connect(cfg("srv")).await.unwrap();

        let conns = registry.connections.read().await;
        let McpConnectionState::Connected {
            resource_templates, ..
        } = conns.get("srv").unwrap()
        else {
            panic!("expected Connected state");
        };
        assert!(
            resource_templates.is_empty(),
            "resources capability absent -> templates fetch must be skipped, got {resource_templates:?}"
        );
    }

    /// §26a — a `resources/templates/list` FAILURE must never fail the
    /// connection. Templates are optional in the MCP spec: a server can
    /// legally declare `capabilities.resources` (because it registered
    /// `resources/list`) and answer `-32601 Method not found` for
    /// `resources/templates/list`. Oracle `Qe` (2.1.251 Mach-O @182528544)
    /// wraps the whole fetch in `try{...}catch(t){ ...; let r=[]; if(!(t
    /// instanceof Er&&t.code===Ir.MethodNotFound)) qt().discoveryFetchErrors
    /// .set(r,we(t)); return r }` — EVERY error path returns an empty array,
    /// and MethodNotFound is explicitly benign. Joining the fetch to the
    /// catalog block with `?` instead disconnected the live transport
    /// (registry.rs's `Err` arm) and returned `Err` from `connect`, so a
    /// server that connected fine before the fetch existed lost ALL of its
    /// tools, resources and prompts.
    #[tokio::test]
    async fn connect_survives_a_resource_templates_fetch_that_fails() {
        let mock = Arc::new(BridgeMock::new(&["alpha"]));
        mock.resources_capability.store(true, Ordering::SeqCst);
        mock.list_resource_templates_fails
            .store(true, Ordering::SeqCst);
        let registry = McpRegistry::with_raw_conn(
            mock.clone() as Arc<dyn McpTransport>,
            mock as Arc<dyn RawConnectionProvider>,
        );
        let connected = registry.connect(cfg("srv")).await;
        assert!(
            connected.is_ok(),
            "a -32601 on resources/templates/list must NOT fail the connection, got {:?}",
            connected.err()
        );

        let conns = registry.connections.read().await;
        let McpConnectionState::Connected {
            tools,
            resource_templates,
            ..
        } = conns.get("srv").unwrap()
        else {
            panic!("expected Connected state, got {:?}", conns.get("srv"));
        };
        assert_eq!(
            tools.len(),
            1,
            "the server's tools must survive a failed template fetch"
        );
        assert!(
            resource_templates.is_empty(),
            "a failed template fetch yields an EMPTY list (oracle `Qe`'s catch), got {resource_templates:?}"
        );
    }

    /// §20a runs on the CONNECT path, not just on `McpClient::list_tools`.
    ///
    /// `McpRegistry::connect` fills `McpConnectionState::Connected { tools }`
    /// from `self.transport.list_tools(...)` — the posix transport, which
    /// never touches `tool_schema`. That is the list
    /// `build_registered_mcp_tools` hands the model on every desktop session;
    /// `McpClient::list_tools` (where the decision already lived) is reached
    /// only by `refresh_catalog`. Without the decision here a root-`anyOf`
    /// schema the oracle drops is forwarded to the model verbatim, and the
    /// two `mock_mcp.rs` integration tests that cover `McpClient::list_tools`
    /// stay green throughout.
    // Same rationale as `connect_resolves_the_schema_gate_from_the_servers_own_hostname`
    // below: the §20a flags are PROCESS-global, so the guard must span the
    // `connect` await — holding it across the await IS the point of the lock.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn connect_applies_the_tool_schema_decision_to_the_model_facing_list() {
        // This test asserts the DEFAULT (both gates off) behaviour by reading
        // the PROCESS-GLOBAL §20a flags, so it must hold the same lock every
        // other §20a test in this binary holds — see
        // `tool_schema::flag_test_lock`. Without it a concurrently-running
        // `tool_schema` test that sets `tengu_mcp_normalize_root_combinators`
        // makes `combo_tool` survive here and this assertion fails at random.
        let _g = crate::tool_schema::flag_test_lock();
        let mock = Arc::new(BridgeMock::with_tool_schemas(&[
            (
                "plain_tool",
                serde_json::json!({"type": "object", "properties": {"a": {"type": "string"}}}),
            ),
            (
                "combo_tool",
                serde_json::json!({"anyOf": [
                    {"type": "object", "properties": {"a": {"type": "string"}}},
                    {"type": "object", "properties": {"b": {"type": "string"}}}
                ]}),
            ),
        ]));
        let registry = McpRegistry::with_raw_conn(
            mock.clone() as Arc<dyn McpTransport>,
            mock as Arc<dyn RawConnectionProvider>,
        );
        registry.connect(cfg("combos")).await.unwrap();

        let conns = registry.connections.read().await;
        let McpConnectionState::Connected { tools, .. } = conns.get("combos").unwrap() else {
            panic!("expected Connected state");
        };
        let names: Vec<&str> = tools.iter().map(|t| t.tool_name.as_str()).collect();
        assert_eq!(
            names,
            vec!["plain_tool"],
            "the root-anyOf tool must be dropped from the CONNECTED tool list \
             (the normalize gate is off by default), leaving the plain tool: {tools:?}"
        );
        assert_eq!(tools[0].full_name, "mcp__combos__plain_tool");
    }

    /// §20b — connecting a server with two droppable tools must still leave
    /// only the healthy tool in the model-facing list (the `retain_mut`
    /// aggregation change must not perturb the KEEP/DROP decision itself).
    /// The aggregated `tengu_mcp_degraded` payload-building itself is unit
    /// tested directly on `degraded_payloads_for_server` below — NOT via a
    /// tracing capture here, deliberately: `tracing::subscriber::set_default`
    /// is thread-local, but callsite `Interest` caching is process-global, so
    /// a concurrently-running test's subscriber can race the cache and
    /// silently starve this one's captured events under `cargo test`'s
    /// default parallelism (confirmed empirically: green alone under
    /// `--test-threads=1`, flaky in the full suite).
    #[tokio::test]
    async fn connect_still_drops_both_anyof_tools_with_aggregation_wired() {
        let mock = Arc::new(BridgeMock::with_tool_schemas(&[
            (
                "plain_tool",
                serde_json::json!({"type": "object", "properties": {"a": {"type": "string"}}}),
            ),
            (
                "combo_one",
                serde_json::json!({"anyOf": [
                    {"type": "object", "properties": {"a": {"type": "string"}}}
                ]}),
            ),
            (
                "combo_two",
                serde_json::json!({"anyOf": [
                    {"type": "object", "properties": {"b": {"type": "string"}}}
                ]}),
            ),
        ]));
        let registry = McpRegistry::with_raw_conn(
            mock.clone() as Arc<dyn McpTransport>,
            mock as Arc<dyn RawConnectionProvider>,
        );
        registry.connect(cfg("degraded_combos")).await.unwrap();

        let conns = registry.connections.read().await;
        let McpConnectionState::Connected { tools, .. } = conns.get("degraded_combos").unwrap()
        else {
            panic!("expected Connected state");
        };
        let names: Vec<&str> = tools.iter().map(|t| t.tool_name.as_str()).collect();
        assert_eq!(names, vec!["plain_tool"]);
    }

    /// §20b — `degraded_payloads_for_server` (the pure aggregation step) maps
    /// each nonzero classification bucket to exactly one payload, with the
    /// right count field populated. Reverting the match arms (e.g. routing
    /// `ToolSchemaUnsupported` to `normalized_count`, or emitting one payload
    /// per tool instead of per bucket) is caught here with no tracing
    /// dependency at all.
    #[test]
    fn degraded_payloads_for_server_maps_each_bucket_to_its_own_count_field() {
        use std::collections::HashMap;
        use telemetry::tengu::mcp::DegradedReason;
        use telemetry::Verified;

        let mut counts = HashMap::new();
        counts.insert(DegradedReason::ToolSchemaNormalized, 3);
        counts.insert(DegradedReason::ToolSchemaNormalizeGated, 2);
        counts.insert(DegradedReason::ToolSchemaUnsupported, 1);
        counts.insert(DegradedReason::ToolSchemaInvalid, 4);
        counts.insert(DegradedReason::ToolPropertyKeyInvalid, 5);
        counts.insert(DegradedReason::ToolSchemaInvalidGated, 6);
        counts.insert(DegradedReason::ToolPropertyKeyInvalidGated, 7);

        let mut payloads = degraded_payloads_for_server(&counts, "http", "srv");
        payloads.sort_by_key(|p| p.reason.wire_str());

        let by_reason: std::collections::HashMap<&'static str, _> = payloads
            .iter()
            .map(|p| {
                (
                    p.reason.wire_str(),
                    (p.normalized_count, p.skipped_count, p.kept_count),
                )
            })
            .collect();
        assert_eq!(
            payloads.len(),
            7,
            "one payload per nonzero bucket: {payloads:?}"
        );
        assert_eq!(by_reason["tool_schema_normalized"], (Some(3), None, None));
        assert_eq!(
            by_reason["tool_schema_normalize_gated"],
            (None, Some(2), None)
        );
        assert_eq!(by_reason["tool_schema_unsupported"], (None, Some(1), None));
        assert_eq!(by_reason["tool_schema_invalid"], (None, Some(4), None));
        assert_eq!(
            by_reason["tool_property_key_invalid"],
            (None, Some(5), None)
        );
        assert_eq!(
            by_reason["tool_schema_invalid_gated"],
            (None, None, Some(6))
        );
        assert_eq!(
            by_reason["tool_property_key_invalid_gated"],
            (None, None, Some(7))
        );
        for p in &payloads {
            assert_eq!(
                p.transport_type.as_ref().map(Verified::as_str),
                Some("http")
            );
            assert!(
                p.mcp_server_name.is_none(),
                "`http` is user-configurable; the oracle's HT gate drops the name"
            );
        }
    }

    /// ROUND-1 REGRESSION. `connected_zero_tools` is the FIRST statement of
    /// the oracle's `yn` — 20 lines above the seven tool-schema counters the
    /// module doc transcribed verbatim while calling that set complete. The
    /// port emitted nothing for it, so the most common silent-MCP-failure
    /// signal (an OAuth-pending, resources-only, or fully-filtered server)
    /// was invisible.
    ///
    /// NOTE ON COVERAGE: this pins the predicate, not the call site. The
    /// aggregated event itself cannot be asserted from a `connect` test here
    /// — see `connect_still_drops_both_anyof_tools_with_aggregation_wired`
    /// for why this file deliberately does no tracing capture. What the call
    /// site must preserve, and what review must check, is that the argument
    /// is the RAW `tools.len()` read BEFORE `retain_mut` filters the list.
    #[test]
    fn connected_zero_tools_fires_only_on_an_empty_raw_list_with_the_tools_capability() {
        assert!(
            connected_zero_tools_fires(true, 0),
            "server advertised tools/list and returned an empty array"
        );
        assert!(
            !connected_zero_tools_fires(true, 2),
            "a NON-empty raw list never fires it, however many tools the \u{a7}20a filter \
             later drops \u{2014} those report their own drop reason instead"
        );
        assert!(
            !connected_zero_tools_fires(false, 0),
            "with no tools capability the oracle never reaches `yn`, so an empty list is \
             not a degraded signal"
        );
        assert!(!connected_zero_tools_fires(false, 3));
    }

    /// The reason maps to a payload with NO count field — the oracle emits
    /// `{reason,transportType,mcpServerName,..._}` for it.
    #[test]
    fn connected_zero_tools_bucket_becomes_a_countless_payload() {
        let counts = std::collections::HashMap::from([(
            telemetry::tengu::mcp::DegradedReason::ConnectedZeroTools,
            1,
        )]);
        let payloads = degraded_payloads_for_server(&counts, "stdio", "srv");
        assert_eq!(payloads.len(), 1, "one payload for the one nonzero bucket");
        let p = &payloads[0];
        assert_eq!(p.reason.wire_str(), "connected_zero_tools");
        assert!(p.normalized_count.is_none());
        assert!(p.skipped_count.is_none());
        assert!(p.kept_count.is_none());
        assert_eq!(
            p.transport_type
                .as_ref()
                .map(telemetry::pii::Verified::as_str),
            Some("stdio")
        );
    }

    #[test]
    fn degraded_payloads_for_server_is_empty_when_no_bucket_is_nonzero() {
        assert!(
            degraded_payloads_for_server(&std::collections::HashMap::new(), "stdio", "srv")
                .is_empty()
        );
    }

    /// §20b — `server_config_invalid_payload` carries the RAW config `type`
    /// string (`McpTransportSpec::kind()`, not a `protocol_negotiation.rs`
    /// `Wr`-mapped label), the fixed literal `"url"` field, and passes the
    /// caller's loader-vs-connect classification straight through. Both
    /// `connect()` gates (`config.config_error` / `connect_time_url_error()`)
    /// funnel through this one function, so a test here covers both call
    /// sites' payload shape without needing to race a tracing capture
    /// against `connect()`'s own dial path.
    #[test]
    fn server_config_invalid_payload_carries_the_raw_transport_kind_and_fixed_field() {
        use telemetry::tengu::mcp::ConfigInvalidSource;

        let cfg = http_cfg("broken", "${MISSING:-}");

        let loader = server_config_invalid_payload(&cfg, ConfigInvalidSource::Loader);
        assert_eq!(loader.transport_type.as_str(), "http");
        assert_eq!(loader.field.as_str(), "url");
        assert_eq!(loader.source.wire_str(), "loader");

        let connect = server_config_invalid_payload(&cfg, ConfigInvalidSource::Connect);
        assert_eq!(connect.source.wire_str(), "connect");
    }

    /// §20b — `tools_listed_payload` counts off the FINAL (post-§20a-filter)
    /// list, not a raw pre-filter count, and `always_load_count` only tallies
    /// `Some(true)` (a `None`/`Some(false)` tool must NOT count). Reverting
    /// either the length source or the filter predicate is caught here.
    #[test]
    fn tools_listed_payload_counts_off_the_final_list() {
        let tools = vec![
            McpToolDto {
                tool_name: "a".into(),
                full_name: "mcp__srv__a".into(),
                server_name: "srv".into(),
                description: String::new(),
                input_schema: serde_json::json!({}),
                search_hint: None,
                always_load: Some(true),
                requires_user_interaction: false,
            },
            McpToolDto {
                tool_name: "b".into(),
                full_name: "mcp__srv__b".into(),
                server_name: "srv".into(),
                description: String::new(),
                input_schema: serde_json::json!({}),
                search_hint: None,
                always_load: Some(false),
                requires_user_interaction: false,
            },
            McpToolDto {
                tool_name: "c".into(),
                full_name: "mcp__srv__c".into(),
                server_name: "srv".into(),
                description: String::new(),
                input_schema: serde_json::json!({}),
                search_hint: None,
                always_load: None,
                requires_user_interaction: false,
            },
        ];
        let payload =
            tools_listed_payload("http", std::time::Duration::from_millis(42), &tools, "srv");
        assert_eq!(payload.transport_type.as_str(), "http");
        assert_eq!(payload.list_duration_ms, 42);
        assert_eq!(payload.tool_count, 3);
        assert_eq!(
            payload.always_load_count, 1,
            "only the Some(true) tool counts"
        );
        assert_eq!(payload.discovery_source.as_str(), "live");
        // Gated: `http` is a user-configurable transport, so the oracle's
        // `HT` gate is false and `EA` drops the key entirely.
        assert!(
            payload.mcp_server_name.is_none(),
            "a user-configured server's raw name must never reach telemetry"
        );
    }

    /// §20a's per-server gate resolves from the connected server's URL
    /// hostname (oracle `Ot(e,t)`: `new URL(t.url).hostname`). `connect` must
    /// therefore hand the gate the URL off `config.spec` — otherwise the gate
    /// sees `None` for every server and only a bare `"*"` entry could ever
    /// enable either transform.
    ///
    /// Written under the ORACLE's literal flag key rather than the module's
    /// private constant, so a misspelling there cannot make this green.
    // The §20a flags are PROCESS-global, so the guard must span both `connect`
    // calls — that is the whole point of the lock, and the `mcp` lib test
    // binary runs `tool_schema`'s gate tests in the same process.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn connect_resolves_the_schema_gate_from_the_servers_own_hostname() {
        let _g = crate::tool_schema::flag_test_lock();
        let flag = crate::tool_schema::ORACLE_NORMALIZE_FLAG_FOR_TEST;
        telemetry::test_set_flag_list(flag, vec!["mcp.example.com".to_string()]);

        let listed = |cfg: McpServerConfig| async move {
            let mock = Arc::new(BridgeMock::with_tool_schemas(&[(
                "combo_tool",
                serde_json::json!({"anyOf": [
                    {"type": "object", "properties": {"a": {"type": "string"}}}
                ]}),
            )]));
            let registry = McpRegistry::with_raw_conn(
                mock.clone() as Arc<dyn McpTransport>,
                mock as Arc<dyn RawConnectionProvider>,
            );
            let name = cfg.name.clone();
            registry.connect(cfg).await.unwrap();
            let conns = registry.connections.read().await;
            let McpConnectionState::Connected { tools, .. } = conns.get(&name).unwrap() else {
                panic!("expected Connected state");
            };
            tools.clone()
        };

        // The LISTED hostname: normalization applies, the tool survives with a
        // rewritten object schema and the "Input constraint:" note.
        let listed_tools = listed(http_cfg("listed", "https://mcp.example.com/v1")).await;
        assert_eq!(
            listed_tools.len(),
            1,
            "a server whose hostname is on the flag list must have its schema NORMALIZED, not dropped: {listed_tools:?}"
        );
        assert_eq!(
            listed_tools[0].input_schema["type"],
            serde_json::json!("object")
        );
        assert!(listed_tools[0].description.starts_with("Input constraint:"));

        // An UNLISTED hostname takes the gate-off branch and is dropped.
        let other_tools = listed(http_cfg("other", "https://mcp.other.org/v1")).await;
        assert!(
            other_tools.is_empty(),
            "a server off the flag list must still be dropped: {other_tools:?}"
        );

        telemetry::test_clear_flag_list(flag);
    }

    #[tokio::test]
    async fn connect_preserves_double_underscore_tool_name() {
        let mock = Arc::new(BridgeMock::new(&["read__file"]));
        let registry = McpRegistry::with_raw_conn(
            mock.clone() as Arc<dyn McpTransport>,
            mock as Arc<dyn RawConnectionProvider>,
        );
        registry.connect(cfg("fs")).await.unwrap();

        let conns = registry.connections.read().await;
        let McpConnectionState::Connected { tools, .. } = conns.get("fs").unwrap() else {
            panic!("expected Connected state");
        };
        // The `__` inside the tool name survives verbatim (normalize is a no-op
        // for names already matching `[a-zA-Z0-9_-]` — `_` is a valid char).
        assert_eq!(tools[0].full_name, "mcp__fs__read__file");
    }

    #[tokio::test]
    async fn connect_normalizes_special_char_tool_segment_and_resolves_raw_wire_name() {
        // claude-code's `buildMcpToolName` normalizes BOTH the server AND the
        // tool segment (`client.ts:1768` → `mcpStringUtils.ts:51`). A tool whose
        // wire name contains a character outside `[a-zA-Z0-9_-]` (here the `.` in
        // `weather.now`) gets a NORMALIZED model-facing FQN, while the RAW wire
        // name is kept on the dto for dispatch (claude-code's `mcpInfo.toolName`).
        let mock = Arc::new(BridgeMock::new(&["weather.now"]));
        let registry = McpRegistry::with_raw_conn(
            mock.clone() as Arc<dyn McpTransport>,
            mock as Arc<dyn RawConnectionProvider>,
        );
        registry.connect(cfg("forecast")).await.unwrap();

        let conns = registry.connections.read().await;
        let McpConnectionState::Connected { tools, .. } = conns.get("forecast").unwrap() else {
            panic!("expected Connected state");
        };
        // Model-facing FQN: the `.` normalizes to `_`.
        assert_eq!(tools[0].full_name, "mcp__forecast__weather_now");
        // The dto keeps the RAW wire name for dispatch.
        assert_eq!(tools[0].tool_name, "weather.now");
        drop(conns);

        // resolve_wire_tool_name maps the normalized model-facing FQN back to
        // the RAW wire name the server expects on `tools/call`.
        assert_eq!(
            registry
                .resolve_wire_tool_name("forecast", "mcp__forecast__weather_now")
                .await
                .as_deref(),
            Some("weather.now"),
        );
        // Unknown FQN or server ⇒ None (the caller falls back to the parsed
        // segment, a no-op for valid-identifier names).
        assert_eq!(
            registry
                .resolve_wire_tool_name("forecast", "mcp__forecast__missing")
                .await,
            None,
        );
        assert_eq!(
            registry
                .resolve_wire_tool_name("nope", "mcp__forecast__weather_now")
                .await,
            None,
        );
    }

    #[tokio::test]
    async fn connect_normalizes_claudeai_prefixed_server() {
        let mock = Arc::new(BridgeMock::new(&["search"]));
        let registry = McpRegistry::with_raw_conn(
            mock.clone() as Arc<dyn McpTransport>,
            mock as Arc<dyn RawConnectionProvider>,
        );
        // "claude.ai Linear" → normalize → "claude_ai_Linear".
        registry.connect(cfg("claude.ai Linear")).await.unwrap();

        let conns = registry.connections.read().await;
        let McpConnectionState::Connected { tools, .. } = conns.get("claude.ai Linear").unwrap()
        else {
            panic!("expected Connected state");
        };
        assert_eq!(tools[0].full_name, "mcp__claude_ai_Linear__search");
        drop(conns);
        assert!(registry.get_client("claude_ai_Linear").await.is_some());
    }

    // ---- P1-08: runtime `/add-dir` live roots + notification fan-out -------

    /// Build a paired `Connection` whose PEER ends stay observable (unlike the
    /// module `paired_connection`, which drops them), so a test can read the
    /// frames the client emits.
    fn observable_connection() -> (Arc<Connection>, mpsc::Receiver<Bytes>) {
        let (_peer_to_us_tx, peer_to_us_rx) = mpsc::channel::<Bytes>(8);
        let (us_to_peer_tx, us_to_peer_rx) = mpsc::channel::<Bytes>(8);
        let conn = Arc::new(Connection::new_streams(
            peer_to_us_rx,
            us_to_peer_tx,
            Mode::Lines,
        ));
        (conn, us_to_peer_rx)
    }

    #[test]
    fn add_root_reports_change_only_on_a_real_add() {
        // jzn-style change-compare: a NEW dir returns true and lands in the
        // shared set; re-adding it returns false (a no-op, so the caller sends
        // NO roots/list_changed notification).
        let registry = McpRegistry::new(Arc::new(BridgeMock::new(&[])));
        assert!(
            registry.add_root(std::path::PathBuf::from("/extra")),
            "first add of a dir must report a change"
        );
        assert!(
            !registry.add_root(std::path::PathBuf::from("/extra")),
            "re-adding an already-present dir must report NO change"
        );
        assert_eq!(
            registry.additional_roots_snapshot(),
            vec![std::path::PathBuf::from("/extra")],
            "the dir is stored exactly once",
        );
    }

    #[tokio::test]
    async fn notify_roots_list_changed_all_fans_out_one_per_client() {
        // The fan-out sends exactly one `notifications/roots/list_changed` to
        // EVERY connected client (claude-code notifyMcpRootsListChanged → per-
        // client sendRootsListChanged).
        let registry = McpRegistry::new(Arc::new(BridgeMock::new(&[])));

        let (conn_a, mut peer_a) = observable_connection();
        let (conn_b, mut peer_b) = observable_connection();
        let client_a = Arc::new(McpClient::new("a", std::path::PathBuf::from("/a"), conn_a).await);
        let client_b = Arc::new(McpClient::new("b", std::path::PathBuf::from("/b"), conn_b).await);
        registry.register_test_client("a", cfg("a"), client_a).await;
        registry.register_test_client("b", cfg("b"), client_b).await;

        let notified = registry.notify_roots_list_changed_all().await;
        assert_eq!(notified, 2, "both connected clients must be notified");

        for peer in [&mut peer_a, &mut peer_b] {
            let frame = tokio::time::timeout(std::time::Duration::from_secs(2), peer.recv())
                .await
                .expect("notification within timeout")
                .expect("a frame was emitted");
            let text = std::str::from_utf8(&frame).expect("utf-8 frame");
            assert!(
                text.contains(r#""method":"notifications/roots/list_changed""#),
                "each client must receive the roots/list_changed notification: {text}",
            );
            assert!(
                !text.contains(r#""id""#),
                "a notification carries no id: {text}"
            );
            // Exactly ONE frame per client — no second notification queued.
            assert!(
                peer.try_recv().is_err(),
                "a client must receive exactly one notification",
            );
        }
    }

    #[tokio::test]
    async fn notify_roots_list_changed_all_on_empty_registry_notifies_none() {
        let registry = McpRegistry::new(Arc::new(BridgeMock::new(&[])));
        assert_eq!(registry.notify_roots_list_changed_all().await, 0);
    }

    // -----------------------------------------------------------------
    // §26b delta 2: XAA single-flight. `resolve_xaa_token`/
    // `resolve_xaa_token_inner` are crate-private, so this lives here
    // (rather than in the integration suite, `mcp/tests/oauth_flow_test.rs`)
    // where it can call them directly — the public `connect()` entry point
    // already serializes per server name via `lifecycle_lock`, which would
    // mask whether the XAA-specific guard does anything at all.
    // -----------------------------------------------------------------

    struct FixedClock(std::time::SystemTime);
    impl traits::Clock for FixedClock {
        fn now(&self) -> std::time::SystemTime {
            self.0
        }
    }

    #[derive(Default)]
    struct XaaMemStorage {
        map: TestMutex<HashMap<(String, String), protocol::SecureStorageData>>,
    }
    #[async_trait]
    impl traits::SecureStorage for XaaMemStorage {
        async fn store(
            &self,
            service: &str,
            account: &str,
            data: protocol::SecureStorageData,
        ) -> Result<(), traits::SecureStorageError> {
            self.map
                .lock()
                .unwrap()
                .insert((service.into(), account.into()), data);
            Ok(())
        }
        async fn retrieve(
            &self,
            service: &str,
            account: &str,
        ) -> Result<Option<protocol::SecureStorageData>, traits::SecureStorageError> {
            Ok(self
                .map
                .lock()
                .unwrap()
                .get(&(service.into(), account.into()))
                .cloned())
        }
        async fn delete(
            &self,
            service: &str,
            account: &str,
        ) -> Result<(), traits::SecureStorageError> {
            self.map
                .lock()
                .unwrap()
                .remove(&(service.into(), account.into()));
            Ok(())
        }
        async fn list(&self, service: &str) -> Result<Vec<String>, traits::SecureStorageError> {
            Ok(self
                .map
                .lock()
                .unwrap()
                .keys()
                .filter(|(s, _)| s == service)
                .map(|(_, a)| a.clone())
                .collect())
        }
        fn is_encrypted(&self) -> bool {
            false
        }
        fn backend(&self) -> traits::SecureStorageBackend {
            traits::SecureStorageBackend::PlainText
        }
    }

    /// XAA config provider handing back fixed IdP+AS inputs.
    struct FixedXaaProvider;
    #[async_trait]
    impl XaaConfigProvider for FixedXaaProvider {
        async fn xaa_inputs(
            &self,
            _server_name: &str,
            _server_url: &str,
        ) -> Result<Option<XaaInputs>, McpError> {
            Ok(Some(XaaInputs {
                client_id: "as-client".into(),
                client_secret: "as-secret".into(),
                idp_client_id: "idp-client".into(),
                idp_client_secret: None,
                idp_id_token: "the-id-token".into(),
                idp_token_endpoint: "https://idp.example.com/token".into(),
            }))
        }
    }

    /// HTTP mock answering the XAA discovery/exchange legs; the AS
    /// jwt-bearer POST (the "mint the access token" step) counts its hits
    /// and sleeps briefly before completing, giving a concurrent second
    /// resolve every opportunity to race ahead if the single-flight guard
    /// is missing.
    struct GatedXaaHttp {
        exchange_calls: std::sync::atomic::AtomicUsize,
    }
    impl GatedXaaHttp {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                exchange_calls: std::sync::atomic::AtomicUsize::new(0),
            })
        }
    }
    #[async_trait]
    impl traits::HttpTransport for GatedXaaHttp {
        async fn request(
            &self,
            req: protocol::HttpRequest,
        ) -> Result<protocol::HttpResponse, traits::HttpError> {
            let url = req.url.clone();
            if url.contains("oauth-protected-resource") {
                return Ok(protocol::HttpResponse {
                    status: 200,
                    headers: vec![],
                    body: r#"{"resource":"https://mcp.example.com/v1","authorization_servers":["https://as.example.com"]}"#.into(),
                    body_bytes: Vec::new(),
                });
            }
            if url.contains("oauth-authorization-server") {
                return Ok(protocol::HttpResponse {
                    status: 200,
                    headers: vec![],
                    body: r#"{"issuer":"https://as.example.com","token_endpoint":"https://as.example.com/token","grant_types_supported":["urn:ietf:params:oauth:grant-type:jwt-bearer"]}"#.into(),
                    body_bytes: Vec::new(),
                });
            }
            if url.contains("idp.example.com/token") {
                return Ok(protocol::HttpResponse {
                    status: 200,
                    headers: vec![],
                    body: r#"{"access_token":"id-jag","issued_token_type":"urn:ietf:params:oauth:token-type:id-jag"}"#.into(),
                    body_bytes: Vec::new(),
                });
            }
            if url == "https://as.example.com/token" {
                self.exchange_calls
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(50)).await;
                return Ok(protocol::HttpResponse {
                    status: 200,
                    headers: vec![],
                    body:
                        r#"{"access_token":"xaa-access","token_type":"Bearer","expires_in":3600}"#
                            .into(),
                    body_bytes: Vec::new(),
                });
            }
            Ok(protocol::HttpResponse {
                status: 404,
                headers: vec![],
                body: String::new(),
                body_bytes: Vec::new(),
            })
        }
        async fn stream_sse(
            &self,
            _req: protocol::HttpRequest,
        ) -> Result<traits::http::SseStream, traits::HttpError> {
            Err(traits::HttpError::InvalidRequest("unused".into()))
        }
    }

    fn xaa_unit_test_config(name: &str) -> McpServerConfig {
        McpServerConfig {
            name: name.into(),
            spec: McpTransportSpec::Http {
                url: "https://mcp.example.com/v1".into(),
                headers: traits::McpHeaders::new(),
                headers_helper: None,
                oauth: Some(traits::McpOAuthConfigDto {
                    client_id: Some("as-client".into()),
                    callback_port: None,
                    auth_server_metadata_url: None,
                    scopes: None,
                    xaa: Some(true),
                }),
            },
            scope: ConfigScope::Project,
            disabled: false,
            timeout_ms: None,
            always_load: false,
            config_error: None,
        }
    }

    /// §26b delta 2: two concurrent `resolve_xaa_token` calls for the SAME
    /// server key must share one exchange. Without the `xaa_refresh_lock`
    /// guard, task B (spawned once task A's exchange is confirmed in
    /// flight, and given a 50ms window while A "sleeps" mid-request) would
    /// independently run its own full IdP+AS chain and land on the same AS
    /// jwt-bearer endpoint too, driving `exchange_calls` to 2.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn xaa_concurrent_resolves_share_one_exchange() {
        std::env::set_var("LINGXI_ENABLE_XAA", "1");

        let http = GatedXaaHttp::new();
        let registry = Arc::new(McpRegistry::new(Arc::new(BridgeMock::new(&[]))).with_oauth(
            OAuthDeps {
                http: http.clone() as Arc<dyn traits::HttpTransport>,
                clock: Arc::new(FixedClock(
                    std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_000),
                )),
                storage: Arc::new(XaaMemStorage::default()) as Arc<dyn traits::SecureStorage>,
                on_authorization_url: Arc::new(|_url: &str| {}),
                xaa_config: Some(Arc::new(FixedXaaProvider)),
            },
        ));

        let config = xaa_unit_test_config("xaa-concurrent");
        let key = oauth::server_key(&config.name, &config.spec);

        let (r1, c1, k1) = (registry.clone(), config.clone(), key.clone());
        let task_a = tokio::spawn(async move {
            let deps = r1.oauth.as_ref().unwrap().clone();
            r1.resolve_xaa_token(&c1, &k1, &deps).await
        });
        let (r2, c2, k2) = (registry.clone(), config.clone(), key.clone());
        let task_b = tokio::spawn(async move {
            let deps = r2.oauth.as_ref().unwrap().clone();
            r2.resolve_xaa_token(&c2, &k2, &deps).await
        });

        let (res_a, res_b) = tokio::join!(task_a, task_b);
        std::env::remove_var("LINGXI_ENABLE_XAA");

        let tok_a = res_a.unwrap().expect("task A resolves");
        let tok_b = res_b.unwrap().expect("task B resolves");
        assert_eq!(tok_a.access_token.expose_secret(), "xaa-access");
        assert_eq!(tok_b.access_token.expose_secret(), "xaa-access");
        assert_eq!(
            http.exchange_calls
                .load(std::sync::atomic::Ordering::SeqCst),
            1,
            "single-flight: exactly one AS jwt-bearer exchange for two concurrent resolves"
        );
    }
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use crate::connection::{ConfigScope, McpServerConfig};
    use async_trait::async_trait;
    use protocol::McpConnectionId as ConnId;
    use serde_json::Value;
    use std::sync::Arc;
    use traits::{
        ElicitRequestDto, ElicitResultDto, McpError, McpNotificationStream, McpPromptDto,
        McpRawConnection, McpResourceContentDto, McpResourceDto, McpServerInfo, McpStatus,
        McpToolDto, McpToolResultDto, McpTransport, McpTransportKind, McpTransportSpec,
        ServerCapabilitiesDto,
    };

    /// Minimal in-crate stub transport — only `connections` matters for
    /// `snapshot`, so every method panics if called.
    struct StubTransport;

    #[async_trait]
    impl McpTransport for StubTransport {
        async fn connect(&self, _s: &McpTransportSpec) -> Result<McpRawConnection, McpError> {
            unreachable!()
        }
        async fn initialize(
            &self,
            _c: &McpRawConnection,
        ) -> Result<ServerCapabilitiesDto, McpError> {
            unreachable!()
        }
        async fn list_tools(&self, _c: &McpRawConnection) -> Result<Vec<McpToolDto>, McpError> {
            unreachable!()
        }
        async fn list_resources(
            &self,
            _c: &McpRawConnection,
        ) -> Result<Vec<McpResourceDto>, McpError> {
            unreachable!()
        }
        async fn list_prompts(&self, _c: &McpRawConnection) -> Result<Vec<McpPromptDto>, McpError> {
            unreachable!()
        }
        async fn call_tool(
            &self,
            _c: &McpRawConnection,
            _t: &str,
            _i: Value,
        ) -> Result<McpToolResultDto, McpError> {
            unreachable!()
        }
        async fn read_resource(
            &self,
            _c: &McpRawConnection,
            _u: &str,
        ) -> Result<McpResourceContentDto, McpError> {
            unreachable!()
        }
        async fn ping(&self, _id: ConnId) -> Result<(), McpError> {
            unreachable!()
        }
        async fn notifications(
            &self,
            _c: &McpRawConnection,
        ) -> Result<McpNotificationStream, McpError> {
            unreachable!()
        }
        async fn handle_elicitation(
            &self,
            _c: &McpRawConnection,
            _r: ElicitRequestDto,
        ) -> Result<ElicitResultDto, McpError> {
            unreachable!()
        }
        async fn disconnect(&self, _id: ConnId) -> Result<(), McpError> {
            unreachable!()
        }
        fn supported_transports(&self) -> Vec<McpTransportKind> {
            vec![McpTransportKind::Stdio]
        }
    }

    fn stdio_cfg(name: &str) -> McpServerConfig {
        McpServerConfig {
            name: name.into(),
            spec: McpTransportSpec::Stdio {
                command: "echo".into(),
                args: vec![],
                env: std::collections::HashMap::new(),
            },
            scope: ConfigScope::Project,
            disabled: false,
            timeout_ms: None,
            always_load: false,
            config_error: None,
        }
    }

    #[tokio::test]
    async fn snapshot_empty_registry() {
        let r = McpRegistry::new(Arc::new(StubTransport));
        assert_eq!(r.snapshot().await, Vec::<McpServerInfo>::new());
    }

    #[tokio::test]
    async fn unconfigured_and_invalid_config_short_circuit_before_dialing() {
        // `StubTransport::connect` is `unreachable!()`, so reaching the dial
        // panics — both of `Nxe`'s pre-dial gates have to fire here, and they
        // carry DIFFERENT oracle error codes (UNCONFIGURED vs INVALID_CONFIG)
        // that `mcp list`/`mcp get` render differently.
        let r = McpRegistry::new(Arc::new(StubTransport));

        let mut blank = stdio_cfg("blank");
        blank.spec = McpTransportSpec::Http {
            url: "   ".into(),
            headers: traits::McpHeaders::default(),
            headers_helper: None,
            oauth: None,
        };
        assert!(
            blank.is_unconfigured(),
            "blank url + no configError = `zar`"
        );
        assert_eq!(
            r.connect(blank).await.unwrap_err().to_string(),
            "connection failed: No URL configured for this server"
        );

        let mut broken = stdio_cfg("broken");
        broken.spec = McpTransportSpec::Http {
            url: "${MISSING:-}".into(),
            headers: traits::McpHeaders::default(),
            headers_helper: None,
            oauth: None,
        };
        broken.config_error =
            Some("'url' \"${MISSING:-}\" expanded to an empty string.".to_string());
        assert!(
            !broken.is_unconfigured(),
            "`url_invalid` is INVALID_CONFIG, never UNCONFIGURED"
        );
        assert_eq!(
            r.connect(broken).await.unwrap_err().to_string(),
            "connection failed: 'url' \"${MISSING:-}\" expanded to an empty string."
        );

        // §18 — the oracle's CONNECT-TIME `new URL(t.url)` re-check
        // (`Ve`/`Ae` @182283839 / @182488092), which fires on a url that is
        // present and non-blank but does not parse. Nothing at load time
        // records a `config_error` for it, so before this gate existed the
        // registry dialed a garbage url and surfaced a raw transport error.
        // `StubTransport::connect` is `unreachable!()`, so reaching the dial
        // panics the test.
        let mut malformed = stdio_cfg("malformed");
        malformed.spec = McpTransportSpec::Http {
            url: "api.example.com/mcp".into(),
            headers: traits::McpHeaders::default(),
            headers_helper: None,
            oauth: None,
        };
        assert!(
            !malformed.is_unconfigured(),
            "a non-blank url is never UNCONFIGURED"
        );
        assert_eq!(
            r.connect(malformed).await.unwrap_err().to_string(),
            "connection failed: 'url' is not a valid URL. Update the server's config and reconnect."
        );
    }

    #[tokio::test]
    async fn servers_with_tools_empty_when_no_clients() {
        // No registered clients (e.g. a server still connecting / awaiting OAuth
        // exposes no tools) → empty. Used by AgentTool's required-MCP gate.
        let r = McpRegistry::new(Arc::new(StubTransport));
        assert!(r.servers_with_tools().await.is_empty());
    }

    #[tokio::test]
    async fn servers_pending_and_failed_classify_internal_states() {
        // The required-MCP poll-wait needs to distinguish a still-connecting
        // server from a failed/absent one — the public `McpStatus` projection
        // collapses these, so `servers_pending`/`servers_failed` read the
        // internal state map. `pending` = Connecting | AwaitingOAuth |
        // Reconnecting; `failed` = Failed. Connected/Disconnected/Stopped are
        // neither.
        let r = McpRegistry::new(Arc::new(StubTransport));
        {
            let mut c = r.connections.write().await;
            c.insert(
                "connecting".into(),
                McpConnectionState::Connecting {
                    config: stdio_cfg("connecting"),
                    started_at: SystemTime::now(),
                },
            );
            c.insert(
                "awaiting".into(),
                McpConnectionState::AwaitingOAuth {
                    config: stdio_cfg("awaiting"),
                    callback_port: 7777,
                },
            );
            c.insert(
                "reconnecting".into(),
                McpConnectionState::Reconnecting {
                    config: stdio_cfg("reconnecting"),
                    retry_count: 2,
                    next_retry_at: SystemTime::now(),
                },
            );
            c.insert(
                "boom".into(),
                McpConnectionState::Failed {
                    config: stdio_cfg("boom"),
                    error: "nope".into(),
                    attempts: 5,
                },
            );
            c.insert(
                "idle".into(),
                McpConnectionState::Disconnected {
                    config: stdio_cfg("idle"),
                    last_error: None,
                },
            );
        }
        let mut pending = r.servers_pending().await;
        pending.sort();
        assert_eq!(pending, vec!["awaiting", "connecting", "reconnecting"]);
        assert_eq!(r.servers_failed().await, vec!["boom".to_string()]);
    }

    #[tokio::test]
    async fn snapshot_disconnected_server_no_error() {
        let r = McpRegistry::new(Arc::new(StubTransport));
        r.connections.write().await.insert(
            "memory".into(),
            McpConnectionState::Disconnected {
                config: stdio_cfg("memory"),
                last_error: None,
            },
        );
        let snap = r.snapshot().await;
        assert_eq!(
            snap,
            vec![McpServerInfo {
                name: "memory".into(),
                status: McpStatus::Disconnected,
                transport: "stdio".into(),
            }]
        );
    }

    #[tokio::test]
    async fn snapshot_disconnected_with_error_becomes_error_status() {
        let r = McpRegistry::new(Arc::new(StubTransport));
        r.connections.write().await.insert(
            "memory".into(),
            McpConnectionState::Disconnected {
                config: stdio_cfg("memory"),
                last_error: Some("boom".into()),
            },
        );
        let snap = r.snapshot().await;
        assert_eq!(snap[0].status, McpStatus::Error("boom".into()));
    }

    #[tokio::test]
    async fn snapshot_sorts_by_name() {
        let r = McpRegistry::new(Arc::new(StubTransport));
        {
            let mut c = r.connections.write().await;
            c.insert(
                "memory".into(),
                McpConnectionState::Disconnected {
                    config: stdio_cfg("memory"),
                    last_error: None,
                },
            );
            c.insert(
                "filesystem".into(),
                McpConnectionState::Disconnected {
                    config: stdio_cfg("filesystem"),
                    last_error: None,
                },
            );
        }
        let snap = r.snapshot().await;
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[0].name, "filesystem");
        assert_eq!(snap[1].name, "memory");
    }
}

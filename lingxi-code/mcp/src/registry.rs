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
use protocol::{AgentId, McpConnectionId};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::RwLock;
use traits::{Clock, HttpTransport, McpError, McpTransport, McpTransportSpec, SecureStorage};

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
}

/// Initial reconnect backoff (claude-code `INITIAL_BACKOFF_MS = 1000`).
const INITIAL_BACKOFF: Duration = Duration::from_millis(1000);
/// Ceiling on reconnect backoff (claude-code `MAX_BACKOFF_MS = 30000`).
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// In-memory registry of every known MCP connection.
pub struct McpRegistry {
    /// Map of server name to current state.
    ///
    /// `pub` so the CLI binary (M6-07 init.rs) can pre-populate
    /// `Disconnected` entries read from `.mcp.json` before the engine
    /// connects, and so engine-side tests can seed states directly.
    pub connections: RwLock<HashMap<String, McpConnectionState>>,
    /// Side-channel cache of [`McpClient`] handles per server name.
    ///
    /// Populated by [`Self::register_client`] (M4-07) — production wiring
    /// inserts an `Arc<McpClient>` for each `Connected` server so the
    /// builtin MCP tools (`MCPTool`, `ListMcpResourcesTool`,
    /// `ReadMcpResourceTool`) can dispatch through the wire-locked
    /// client surface (in particular, `McpClientError::Timeout`).
    clients: RwLock<HashMap<String, Arc<McpClient>>>,
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
    /// Interval used by the background health-check task.
    #[allow(dead_code)] // consumed by the health-check loop in Plan 13
    pub health_check_interval: Duration,
    /// Maximum consecutive reconnect attempts before declaring `Failed`.
    ///
    /// Consumed by [`Self::run_reconnect_loop`]; matches claude-code's
    /// `MAX_RECONNECT_ATTEMPTS = 5`.
    pub max_retry_count: u32,
}

impl McpRegistry {
    /// Build a registry bound to a platform transport.
    ///
    /// No `RawConnectionProvider` is wired, so [`Self::connect`] does NOT build
    /// a live [`McpClient`] — use [`Self::with_raw_conn`] for that.
    #[must_use]
    pub fn new(transport: Arc<dyn McpTransport>) -> Self {
        Self {
            connections: RwLock::new(HashMap::new()),
            clients: RwLock::new(HashMap::new()),
            agent_scoped: RwLock::new(HashMap::new()),
            transport,
            raw_conn: None,
            hook_dispatcher: None,
            oauth: None,
            health_check_interval: Duration::from_secs(30),
            max_retry_count: 5,
        }
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

    /// Cache an `Arc<McpClient>` for `name` (M4-07).
    ///
    /// The platform host calls this after building the client (typically
    /// alongside `connect`). Builtin tools then call [`Self::get_client`]
    /// to dispatch over the wire-locked client surface.
    pub async fn register_client(&self, name: &str, client: Arc<McpClient>) {
        self.clients.write().await.insert(name.into(), client);
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
            .map(|(_, v)| Arc::clone(v))
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
        self.clients.write().await.insert(name.into(), client);
    }

    /// Connect, run `initialize`, and discover the server's catalog.
    ///
    /// Re-uses the existing connection if `config.name` is already in the
    /// `Connected` state.
    pub async fn connect(&self, config: McpServerConfig) -> Result<McpConnectionId, McpError> {
        if let Some(McpConnectionState::Connected { connection_id, .. }) =
            self.connections.read().await.get(&config.name)
        {
            return Ok(*connection_id);
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
        let (connect_spec, oauth_key) = self.resolve_oauth_spec(&config).await?;

        let attempt = |spec: McpTransportSpec| {
            let transport = Arc::clone(&self.transport);
            async move {
                let conn = transport.connect(&spec).await?;
                let caps = transport.initialize(&conn).await?;
                Ok::<_, McpError>((conn, caps))
            }
        };

        let (conn, caps) = match tokio::time::timeout(connect_timeout, attempt(connect_spec.clone()))
            .await
            .map_err(|_elapsed| {
                McpError::Connection(format!(
                    "MCP connection timed out after {}s",
                    connect_timeout.as_secs()
                ))
            })? {
            Ok(pair) => pair,
            // 401 on a connect/initialize for an OAuth server → the access token
            // is stale: force a refresh (or a fresh interactive flow), re-inject
            // the Bearer, and retry ONCE. Faithful-core 401 detection: the
            // transport flattens errors to strings, so we substring-match "401"
            // (structured status is a noted residual).
            Err(e) if oauth_key.is_some() && error_is_401(&e) => {
                let refreshed = self.reauth_oauth_spec(&config).await?;
                tokio::time::timeout(connect_timeout, attempt(refreshed))
                    .await
                    .map_err(|_elapsed| {
                        McpError::Connection(format!(
                            "MCP connection timed out after {}s",
                            connect_timeout.as_secs()
                        ))
                    })??
            }
            Err(e) => return Err(e),
        };
        let mut tools = self.transport.list_tools(&conn).await?;
        let resources = self.transport.list_resources(&conn).await?;
        let prompts = self.transport.list_prompts(&conn).await?;

        // Rewrite the empty `<server>` token the transport emits (it has no
        // logical server name, only an `McpConnectionId`). This is the missing
        // "rewrite site" the posix `list_tools` comment defers to: stamp the
        // RAW `config.name` into `server_name` and build the normalized FQN
        // `mcp__<normalize(server)>__<tool>`. Mirrors claude-code's
        // `buildMcpToolName(client.name, tool.name)` (client.ts:1768) with the
        // 1:1 `normalizeNameForMCP` (normalization.rs). A `tool_name` already
        // containing `__` (e.g. `read__file`) survives verbatim.
        let normalized_server = normalize_name_for_mcp(&config.name);
        for dto in &mut tools {
            dto.server_name.clone_from(&config.name);
            dto.full_name = format!("mcp__{}__{}", normalized_server, dto.tool_name);
        }

        let connection_id = conn.connection_id;
        let server_name = config.name.clone();
        self.connections.write().await.insert(
            server_name.clone(),
            McpConnectionState::Connected {
                config,
                connection_id,
                capabilities: caps,
                tools,
                resources,
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
                let cwd = std::env::current_dir().unwrap_or_default();
                // Forward the optional hook dispatcher so this server's
                // `elicitation/create` handler can fire the `Elicitation` hook.
                // `None` => default `{"action":"cancel"}` (unchanged).
                let client = Arc::new(
                    McpClient::with_hook_dispatcher(
                        server_name.clone(),
                        cwd,
                        connection,
                        self.hook_dispatcher.clone(),
                    )
                    .await,
                );
                self.register_client(&server_name, client).await;
            }
        }

        Ok(connection_id)
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
                        let meta = oauth::discover_auth_server_metadata(
                            &deps.http,
                            spec_url(&config.spec),
                            oauth_cfg.auth_server_metadata_url.as_deref(),
                        )
                        .await?;
                        let client_id =
                            oauth_cfg.client_id.clone().unwrap_or_default();
                        let refreshed = oauth::refresh_tokens(
                            &deps.http,
                            &deps.clock,
                            &meta,
                            &client_id,
                            &refresh,
                        )
                        .await?;
                        oauth::save_tokens(&deps.storage, &deps.clock, &key, &refreshed).await?;
                        refreshed
                    }
                    None => self.run_interactive_oauth(config, oauth_cfg, &key, deps).await?,
                }
            }
            None => self.run_interactive_oauth(config, oauth_cfg, &key, deps).await?,
        };

        Ok((
            inject_bearer(&config.spec, token.access_token.expose_secret()),
            Some(key),
        ))
    }

    /// Re-authenticate after a 401: refresh if a refresh token is stored, else
    /// run a fresh interactive flow, then return the spec with the new Bearer.
    async fn reauth_oauth_spec(
        &self,
        config: &McpServerConfig,
    ) -> Result<McpTransportSpec, McpError> {
        let deps = self
            .oauth
            .as_ref()
            .ok_or_else(|| McpError::OAuth("oauth seam not wired".into()))?;
        let oauth_cfg = spec_oauth(&config.spec)
            .ok_or_else(|| McpError::OAuth("server has no oauth config".into()))?;
        let key = oauth::server_key(&config.name, &config.spec);

        let token = match oauth::load_tokens(&deps.storage, &key)
            .await?
            .and_then(|t| t.refresh_token)
        {
            Some(refresh) => {
                let meta = oauth::discover_auth_server_metadata(
                    &deps.http,
                    spec_url(&config.spec),
                    oauth_cfg.auth_server_metadata_url.as_deref(),
                )
                .await?;
                let client_id = oauth_cfg.client_id.clone().unwrap_or_default();
                match oauth::refresh_tokens(&deps.http, &deps.clock, &meta, &client_id, &refresh)
                    .await
                {
                    Ok(t) => {
                        oauth::save_tokens(&deps.storage, &deps.clock, &key, &t).await?;
                        t
                    }
                    // Refresh token rejected → fall back to a fresh flow.
                    Err(oauth::OAuthError::RefreshRejected(_)) => {
                        self.run_interactive_oauth(config, oauth_cfg, &key, deps).await?
                    }
                    Err(e) => return Err(e.into()),
                }
            }
            None => self.run_interactive_oauth(config, oauth_cfg, &key, deps).await?,
        };

        Ok(inject_bearer(&config.spec, token.access_token.expose_secret()))
    }

    /// Drive the full interactive OAuth flow and persist the resulting tokens.
    async fn run_interactive_oauth(
        &self,
        config: &McpServerConfig,
        oauth_cfg: &traits::McpOAuthConfigDto,
        key: &str,
        deps: &OAuthDeps,
    ) -> Result<oauth::Tokens, McpError> {
        // Surface `AwaitingOAuth` while the user completes the browser flow.
        let callback_port = oauth_cfg.callback_port.unwrap_or(0);
        self.connections.write().await.insert(
            config.name.clone(),
            McpConnectionState::AwaitingOAuth {
                config: config.clone(),
                callback_port,
            },
        );
        let tokens = oauth::perform_oauth_flow(
            &deps.http,
            &deps.clock,
            oauth_cfg,
            spec_url(&config.spec),
            &deps.on_authorization_url,
        )
        .await?;
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
        let mut out = Vec::with_capacity(configs.len());
        for config in configs {
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
                continue;
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
            out.push((name, result));
        }
        out
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
            // Disabled/stopped guard: re-read state before each attempt.
            match self.connections.read().await.get(&name) {
                Some(McpConnectionState::Stopped { .. }) | None => {
                    tracing::debug!(server = %name, "reconnect aborted: server stopped");
                    return;
                }
                Some(state) if state_is_disabled(state) => {
                    tracing::debug!(server = %name, "reconnect aborted: server disabled");
                    return;
                }
                _ => {}
            }

            // `next_retry_at` is the wall-clock time the NEXT attempt would fire
            // if this one fails; the final attempt has no successor, so it
            // points at the present.
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
            match self.connect(config.clone()).await {
                Ok(_) => {
                    tracing::info!(server = %name, attempt, "MCP reconnect succeeded");
                    return;
                }
                Err(e) => {
                    if attempt == max {
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
                    tracing::debug!(
                        server = %name,
                        attempt,
                        error = %e,
                        "MCP reconnect attempt failed; backing off"
                    );
                    // Back off ONLY after a failed NON-final attempt; the final
                    // attempt has no trailing sleep (claude-code schedules the
                    // *next* retry, never one after the last).
                    if let Some(backoff) = post_attempt_backoff(attempt, max) {
                        tokio::time::sleep(backoff).await;
                    }
                }
            }
        }
    }

    /// Drop the named connection and transition it to `Stopped`.
    pub async fn disconnect(&self, name: &str) -> Result<(), McpError> {
        let mut conns = self.connections.write().await;
        if let Some(McpConnectionState::Connected {
            connection_id,
            config,
            ..
        }) = conns.remove(name)
        {
            self.transport.disconnect(connection_id).await?;
            // Drop any cached `McpClient` so `get_client(name)` stops returning
            // a handle to the now-dead connection (registered under the raw
            // `config.name` in `connect`).
            self.clients.write().await.remove(name);
            conns.insert(name.into(), McpConnectionState::Stopped { config });
        }
        Ok(())
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
}

/// MCPLIFE.4: the connect+initialize handshake deadline, mirroring claude-code's
/// `getConnectionTimeoutMs()` (`services/mcp/client.ts:456-458`):
/// `parseInt(process.env.MCP_TIMEOUT || '', 10) || 30000` — a positive integer
/// number of milliseconds, defaulting to 30s when unset / non-numeric / zero.
fn mcp_connection_timeout() -> Duration {
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
    let factor = 1u64.checked_shl(attempt.saturating_sub(1)).unwrap_or(u64::MAX);
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
        McpTransportSpec::Sse { oauth, .. } | McpTransportSpec::Http { oauth, .. } => oauth.as_ref(),
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
            oauth,
        } => {
            headers.insert("Authorization".into(), bearer);
            McpTransportSpec::Http {
                url,
                headers,
                oauth,
            }
        }
        other => other,
    }
}

/// Faithful-core 401 detection: the transport flattens HTTP failures to a
/// `Connection` / `Handshake` error string, so we substring-match `"401"`.
/// Structured-status detection is a noted residual.
fn error_is_401(e: &McpError) -> bool {
    matches!(
        e,
        McpError::Connection(m) | McpError::Handshake(m) if m.contains("401")
    )
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
    use std::sync::Mutex as StdMutex;
    use tokio::sync::mpsc;
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

    /// Functional mock transport that ALSO bridges a paired in-memory
    /// `jsonrpc::Connection` through [`RawConnectionProvider`].
    ///
    /// On `connect` it mints a fresh id, stashes a paired connection under it,
    /// and returns canned tools (with the empty `<server>` token the real
    /// transport emits, so the registry's rewrite is exercised).
    struct BridgeMock {
        tools: Vec<McpToolDto>,
        conns: StdMutex<HashMap<ConnId, Arc<Connection>>>,
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
                })
                .collect();
            Self {
                tools,
                conns: StdMutex::new(HashMap::new()),
            }
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
                resources: false,
                prompts: false,
                logging: false,
                experimental: HashMap::new(),
            })
        }
        async fn list_tools(&self, _c: &McpRawConnection) -> Result<Vec<McpToolDto>, McpError> {
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
        // The `__` inside the tool name survives verbatim.
        assert_eq!(tools[0].full_name, "mcp__fs__read__file");
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
        let McpConnectionState::Connected { tools, .. } =
            conns.get("claude.ai Linear").unwrap()
        else {
            panic!("expected Connected state");
        };
        assert_eq!(tools[0].full_name, "mcp__claude_ai_Linear__search");
        drop(conns);
        assert!(registry.get_client("claude_ai_Linear").await.is_some());
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
        }
    }

    #[tokio::test]
    async fn snapshot_empty_registry() {
        let r = McpRegistry::new(Arc::new(StubTransport));
        assert_eq!(r.snapshot().await, Vec::<McpServerInfo>::new());
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

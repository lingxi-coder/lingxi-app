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
use crate::raw_conn::RawConnectionProvider;
use protocol::{AgentId, McpConnectionId};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::RwLock;
use traits::{McpError, McpTransport};

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

        let conn = self.transport.connect(&config.spec).await?;
        let caps = self.transport.initialize(&conn).await?;
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
    /// Attempts `connect` up to `self.max_retry_count` times. Between attempts
    /// the server rests in `Reconnecting { retry_count, next_retry_at }` and
    /// the task sleeps for `min(INITIAL_BACKOFF * 2^(attempt-1), MAX_BACKOFF)`
    /// — i.e. 1s, 2s, 4s, 8s, 16s for attempts 1-4 (capped at 30s). On success
    /// the server is left `Connected` (set by [`Self::connect`]); after the
    /// final attempt fails it transitions to `Failed { error, attempts }`.
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

            let backoff = backoff_for(attempt);
            let next_retry_at = SystemTime::now() + backoff;
            self.connections.write().await.insert(
                name.clone(),
                McpConnectionState::Reconnecting {
                    config: config.clone(),
                    retry_count: attempt,
                    next_retry_at,
                },
            );

            tokio::time::sleep(backoff).await;

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

/// Exponential backoff for reconnect `attempt` (1-based), capped at
/// [`MAX_BACKOFF`]. Mirrors claude-code's
/// `min(INITIAL_BACKOFF_MS * 2^(attempt-1), MAX_BACKOFF_MS)`.
fn backoff_for(attempt: u32) -> Duration {
    let factor = 1u64.checked_shl(attempt.saturating_sub(1)).unwrap_or(u64::MAX);
    let base = u64::try_from(INITIAL_BACKOFF.as_millis()).unwrap_or(u64::MAX);
    let millis = base.saturating_mul(factor);
    Duration::from_millis(millis).min(MAX_BACKOFF)
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

//! Per-agent MCP connection scoping for a subagent's OWN inline
//! `mcpServers` frontmatter (spec §24b).
//!
//! Round 6 filed [`AgentScopedConnections`] as "dead code, possible
//! connection leak"; round 9 corrected that — the gap is LARGER, and no leak
//! is possible, because nothing ever connected a per-subagent scope in the
//! first place. This file was the unused teardown half of a feature whose
//! connect/inject halves were never built (`agent::tool_resolver` always
//! passed a literal `&[]` for a subagent's own MCP tools). It is now used by
//! [`agent::tool_resolver::SubagentMcpConnector`]'s production implementation
//! at the composition root, wired through
//! `agent::handle::PoolSubagentSpawner::with_mcp_subagent_connector`.
//!
//! ## Why connections are name-mangled, not a second registry
//!
//! [`crate::registry::McpRegistry`] keys its live connection table by the
//! server's logical NAME alone (`connect(config)` reuses an existing
//! `Connected` entry for `config.name`; `disconnect(name)` tears down exactly
//! that entry) — there is no per-caller/per-scope namespace. Connecting a
//! subagent's inline server under its bare frontmatter name into the SAME
//! shared registry the main session uses would let two different scopes that
//! happen to declare the same server name collide: a second `connect` would
//! silently reuse (or, worse, replace) the first's entry, and a `disconnect`
//! issued when one scope's owning agent exits would tear down a connection a
//! sibling scope is still using.
//!
//! [`AgentScopedConnections`] avoids that by connecting under a per-agent
//! MANGLED name ([`AgentScopedConnections::scoped_key`]) that embeds the
//! owning [`AgentId`], so every agent's inline servers occupy their own
//! exclusive slot in the shared registry's name-keyed map — collision-free by
//! construction — while still reusing the live, fully-configured registry
//! (OAuth, roots, the elicitation hook dispatcher, the reconnect loop) rather
//! than standing up a second bare instance that would silently lack all of
//! that.

use crate::registry::McpRegistry;
use crate::McpServerConfig;
use protocol::{AgentId, McpConnectionId};
use std::collections::HashMap;

/// Tracks which connections each agent is responsible for.
#[derive(Default)]
pub struct AgentScopedConnections {
    /// `agent -> (original server name -> connection id)`. The registry's own
    /// key for that connection is [`AgentScopedConnections::scoped_key`]`(agent,
    /// name)`, not the bare `name` stored here — the bare name is what
    /// [`Self::cleanup`] re-mangles to find it again.
    inner: HashMap<AgentId, HashMap<String, McpConnectionId>>,
}

impl AgentScopedConnections {
    /// Build an empty scope table.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The per-agent-mangled registry key for `server` (see the module docs).
    ///
    /// Uses the bare UUID (`agent.as_uuid()`), not [`AgentId`]'s `Display`
    /// (`"agent:<uuid>"`): the registry's own name lookups
    /// ([`McpRegistry::get_config`] and the tool-FQN builder both compare
    /// against [`protocol::normalize_name_for_mcp`]-normalized names, which
    /// replaces the `:` with `_`) — a bare UUID's hyphenated hex is already
    /// entirely within the `[a-zA-Z0-9_-]` charset that normalization leaves
    /// untouched, so the mangled key round-trips through every MCP name path
    /// unchanged instead of silently diverging from what was stored.
    fn scoped_key(agent: AgentId, server: &str) -> String {
        format!("__agent_scope__{}__{server}", agent.as_uuid())
    }

    /// Record that `agent` owns the connection to `server` (registered under
    /// [`Self::scoped_key`] in `registry`, not the bare `server` name).
    pub fn register(&mut self, agent: AgentId, server: String, conn: McpConnectionId) {
        self.inner.entry(agent).or_default().insert(server, conn);
    }

    /// Connect `agent`'s scoped inline `mcpServers` — already converted by
    /// [`agent::agent_mcp_specs_to_scoped_configs`] into `.mcp.json`-shaped
    /// [`McpServerConfig`]s — into the SHARED `registry`, each under
    /// [`Self::scoped_key`], and record every success so [`Self::cleanup`]
    /// can tear it down.
    ///
    /// Best-effort per server, mirroring [`McpRegistry::connect_all`]'s
    /// partial-success semantics: a `disabled` config is skipped with no
    /// dial attempt (matching `connect_all`'s own skip, so a subagent server
    /// disabled via the `.mcp.json`-shaped body respects that flag), and a
    /// failed connect is logged and simply contributes no tools rather than
    /// failing the whole spawn — a broken inline server must not block the
    /// agent it is attached to from running at all.
    pub async fn connect_all(
        &mut self,
        registry: &McpRegistry,
        agent: AgentId,
        configs: Vec<McpServerConfig>,
    ) -> Vec<McpConnectionId> {
        let mut ids = Vec::new();
        for mut cfg in configs {
            let server = cfg.name.clone();
            if cfg.disabled {
                tracing::debug!(agent = %agent, server = %server, "skipping disabled subagent-scoped MCP server");
                continue;
            }
            cfg.name = Self::scoped_key(agent, &server);
            match registry.connect(cfg).await {
                Ok(id) => {
                    self.register(agent, server, id);
                    ids.push(id);
                }
                Err(error) => {
                    tracing::warn!(
                        agent = %agent,
                        server = %server,
                        %error,
                        "subagent inline MCP server failed to connect"
                    );
                }
            }
        }
        ids
    }

    /// Disconnect every connection `agent` owns in `registry` (by
    /// [`Self::scoped_key`]) and drop the bookkeeping. Idempotent: a no-op
    /// when `agent` connected nothing, including when it never declared
    /// inline `mcpServers` at all.
    pub async fn cleanup(&mut self, registry: &McpRegistry, agent: &AgentId) -> Vec<McpConnectionId> {
        let Some(owned) = self.inner.remove(agent) else {
            return Vec::new();
        };
        let mut ids = Vec::with_capacity(owned.len());
        for (server, id) in owned {
            let key = Self::scoped_key(*agent, &server);
            if let Err(error) = registry.disconnect(&key).await {
                tracing::warn!(
                    agent = %agent,
                    server = %server,
                    %error,
                    "failed to disconnect subagent-scoped MCP server on exit"
                );
            }
            ids.push(id);
        }
        ids
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoped_key_embeds_the_agent_id_so_two_agents_cannot_collide() {
        let a = AgentId::new();
        let b = AgentId::new();
        let ka = AgentScopedConnections::scoped_key(a, "docs");
        let kb = AgentScopedConnections::scoped_key(b, "docs");
        assert_ne!(
            ka, kb,
            "same server name, different agents, must mangle to different registry keys"
        );
        assert!(ka.contains("docs"));
        assert!(ka.contains(&a.as_uuid().to_string()));
    }

    #[test]
    fn register_then_manual_removal_tracks_and_clears_bookkeeping() {
        // Exercises the plain HashMap bookkeeping half in isolation (the
        // connect/disconnect-through-a-real-registry half is covered by the
        // end-to-end tests below).
        let mut scope = AgentScopedConnections::new();
        let agent = AgentId::new();
        let conn = McpConnectionId::new();
        scope.register(agent, "docs".to_string(), conn);
        assert_eq!(scope.inner.get(&agent).map(HashMap::len), Some(1));
        // A second agent's cleanup must not see the first agent's entry.
        let other = AgentId::new();
        assert!(scope.inner.get(&other).is_none());
        let owned = scope.inner.remove(&agent);
        assert_eq!(
            owned.map(|m| m.into_values().collect::<Vec<_>>()),
            Some(vec![conn])
        );
        assert!(scope.inner.get(&agent).is_none());
    }

    // ── end-to-end: connect_all / cleanup against a real McpRegistry ──

    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex as StdMutex};
    use traits::{McpRawConnection, McpTransport, McpTransportKind, McpTransportSpec, ServerCapabilitiesDto};

    /// Minimal transport: `connect` mints a fresh id and records it (so
    /// `disconnect` can be asserted against), `initialize` declares NO
    /// capabilities (so the registry never calls the list_* methods — see
    /// `connect_locked_inner`'s `if caps.tools { list_tools } else { vec![] }`
    /// gate), and every catalog/notification method is unreachable.
    #[derive(Default)]
    struct StubTransport {
        connected: StdMutex<Vec<protocol::McpConnectionId>>,
        disconnected: StdMutex<Vec<protocol::McpConnectionId>>,
        connect_calls: AtomicUsize,
    }

    #[async_trait]
    impl McpTransport for StubTransport {
        async fn connect(
            &self,
            _spec: &McpTransportSpec,
        ) -> Result<McpRawConnection, traits::McpError> {
            self.connect_calls.fetch_add(1, Ordering::SeqCst);
            let id = McpConnectionId::new();
            self.connected.lock().unwrap().push(id);
            Ok(McpRawConnection { connection_id: id })
        }
        async fn initialize(
            &self,
            _conn: &McpRawConnection,
        ) -> Result<ServerCapabilitiesDto, traits::McpError> {
            Ok(ServerCapabilitiesDto {
                tools: false,
                resources: false,
                prompts: false,
                logging: false,
                experimental: HashMap::new(),
            })
        }
        async fn list_tools(
            &self,
            _conn: &McpRawConnection,
        ) -> Result<Vec<traits::McpToolDto>, traits::McpError> {
            unreachable!("capabilities.tools = false ⇒ never called")
        }
        async fn list_resources(
            &self,
            _conn: &McpRawConnection,
        ) -> Result<Vec<traits::McpResourceDto>, traits::McpError> {
            unreachable!("capabilities.resources = false ⇒ never called")
        }
        async fn list_prompts(
            &self,
            _conn: &McpRawConnection,
        ) -> Result<Vec<traits::McpPromptDto>, traits::McpError> {
            unreachable!("capabilities.prompts = false ⇒ never called")
        }
        async fn call_tool(
            &self,
            _conn: &McpRawConnection,
            _tool: &str,
            _input: serde_json::Value,
        ) -> Result<traits::McpToolResultDto, traits::McpError> {
            unreachable!("not exercised by connect/disconnect")
        }
        async fn read_resource(
            &self,
            _conn: &McpRawConnection,
            _uri: &str,
        ) -> Result<traits::McpResourceContentDto, traits::McpError> {
            unreachable!("not exercised by connect/disconnect")
        }
        async fn ping(&self, _id: McpConnectionId) -> Result<(), traits::McpError> {
            unreachable!("not exercised by connect/disconnect")
        }
        async fn notifications(
            &self,
            _conn: &McpRawConnection,
        ) -> Result<traits::McpNotificationStream, traits::McpError> {
            unreachable!("not exercised by connect/disconnect")
        }
        async fn handle_elicitation(
            &self,
            _conn: &McpRawConnection,
            _req: traits::ElicitRequestDto,
        ) -> Result<traits::ElicitResultDto, traits::McpError> {
            unreachable!("not exercised by connect/disconnect")
        }
        async fn disconnect(&self, id: McpConnectionId) -> Result<(), traits::McpError> {
            self.disconnected.lock().unwrap().push(id);
            Ok(())
        }
        fn supported_transports(&self) -> Vec<McpTransportKind> {
            vec![McpTransportKind::Stdio]
        }
    }

    fn stdio_config(name: &str) -> McpServerConfig {
        McpServerConfig {
            name: name.to_string(),
            spec: McpTransportSpec::Stdio {
                command: "npx".to_string(),
                args: vec!["-y".to_string(), "docs-mcp".to_string()],
                env: HashMap::new(),
            },
            scope: crate::ConfigScope::Agent,
            disabled: false,
            timeout_ms: None,
            always_load: false,
            config_error: None,
        }
    }

    #[tokio::test]
    async fn connect_all_reaches_the_real_registry_and_cleanup_disconnects_it() {
        let transport = Arc::new(StubTransport::default());
        let registry = McpRegistry::new(transport.clone() as Arc<dyn McpTransport>);
        let mut scope = AgentScopedConnections::new();
        let agent = AgentId::new();

        let ids = scope
            .connect_all(&registry, agent, vec![stdio_config("docs")])
            .await;
        assert_eq!(ids.len(), 1, "one config, one successful connection");
        assert_eq!(
            transport.connect_calls.load(Ordering::SeqCst),
            1,
            "connect_all must actually dial the transport, not just record bookkeeping"
        );
        // Connected under the MANGLED name, not the bare "docs" — proves the
        // collision-avoidance design, not just the doc comment's claim.
        let mangled = AgentScopedConnections::scoped_key(agent, "docs");
        assert!(
            registry.get_config(&mangled).await.is_some(),
            "must be registered under the per-agent-mangled key"
        );
        assert!(
            registry.get_config("docs").await.is_none(),
            "must NOT be registered under the bare server name (that would be a collision hazard)"
        );

        scope.cleanup(&registry, &agent).await;
        assert_eq!(
            transport.disconnected.lock().unwrap().len(),
            1,
            "cleanup must actually disconnect the transport connection, not merely drop bookkeeping"
        );
        // `McpRegistry::disconnect` transitions the entry to `Stopped` (kept
        // around so a `/mcp`-style listing can still show it) rather than
        // removing it outright — `snapshot()` collapses that to
        // `McpStatus::Disconnected`.
        let entry = registry
            .snapshot()
            .await
            .into_iter()
            .find(|info| info.name == mangled)
            .expect("the mangled entry must still be present, now Stopped/Disconnected");
        assert_eq!(entry.status, traits::McpStatus::Disconnected);
    }

    #[tokio::test]
    async fn two_agents_with_the_same_server_name_do_not_collide() {
        let transport = Arc::new(StubTransport::default());
        let registry = McpRegistry::new(transport.clone() as Arc<dyn McpTransport>);
        let mut scope = AgentScopedConnections::new();
        let a = AgentId::new();
        let b = AgentId::new();

        scope
            .connect_all(&registry, a, vec![stdio_config("docs")])
            .await;
        scope
            .connect_all(&registry, b, vec![stdio_config("docs")])
            .await;
        assert_eq!(
            transport.connect_calls.load(Ordering::SeqCst),
            2,
            "two different agents' same-named server must be TWO independent connections"
        );

        // Tearing down `a` must not touch `b`'s identically-named connection.
        scope.cleanup(&registry, &a).await;
        assert_eq!(transport.disconnected.lock().unwrap().len(), 1);
        assert!(
            registry
                .get_config(&AgentScopedConnections::scoped_key(b, "docs"))
                .await
                .is_some(),
            "agent b's connection must survive agent a's cleanup"
        );

        scope.cleanup(&registry, &b).await;
        assert_eq!(transport.disconnected.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn disabled_config_is_skipped_with_no_dial_attempt() {
        let transport = Arc::new(StubTransport::default());
        let registry = McpRegistry::new(transport.clone() as Arc<dyn McpTransport>);
        let mut scope = AgentScopedConnections::new();
        let mut cfg = stdio_config("docs");
        cfg.disabled = true;

        let ids = scope.connect_all(&registry, AgentId::new(), vec![cfg]).await;
        assert!(ids.is_empty());
        assert_eq!(
            transport.connect_calls.load(Ordering::SeqCst),
            0,
            "a disabled inline server must never be dialed"
        );
    }

    #[tokio::test]
    async fn cleanup_on_an_agent_that_connected_nothing_is_a_no_op() {
        let transport = Arc::new(StubTransport::default());
        let registry = McpRegistry::new(transport.clone() as Arc<dyn McpTransport>);
        let mut scope = AgentScopedConnections::new();
        // Never called connect_all for this agent at all.
        let ids = scope.cleanup(&registry, &AgentId::new()).await;
        assert!(ids.is_empty());
        assert!(transport.disconnected.lock().unwrap().is_empty());
    }
}

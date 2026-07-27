//! Per-connection configuration, scope, and state machine.
//!
//! One MCP server is modelled as a config plus a state. The registry
//! (`registry.rs`) drives transitions between the variants below using
//! the platform-supplied [`traits::McpTransport`].

use protocol::McpConnectionId;
use serde::{Deserialize, Serialize};
use std::time::SystemTime;
use traits::{McpPromptDto, McpResourceDto, McpToolDto, McpTransportSpec, ServerCapabilitiesDto};

/// Static configuration for one MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Logical server name (used as a map key and in approval prompts).
    pub name: String,
    /// Transport configuration consumed by the platform.
    pub spec: McpTransportSpec,
    /// Origin of the config, used by the approval policy.
    pub scope: ConfigScope,
    /// When true the registry must not auto-connect at startup.
    #[allow(dead_code)] // honoured by the connect loop in Plan 13
    pub disabled: bool,
    /// Per-server `tools/call` timeout (ms): the config `timeout` field, with
    /// the sse/http `request_timeout_ms` alias folded in at parse time (RAn:
    /// `timeout ??= min(request_timeout_ms, 300_000)`). `None` = no per-server
    /// override → the shared BHs resolver ([`crate::client::mcp_tool_timeout_for`])
    /// falls back to the `MCP_TOOL_TIMEOUT` env var / 100_000_000 default. Kept
    /// OFF [`McpTransportSpec`] on purpose so it never perturbs the
    /// `getServerKey`/`oauth::server_key` config hash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// `alwaysLoad`: force every tool from this server into the prompt, never
    /// deferred behind tool search ("Equivalent to setting defer_loading:false
    /// on the API"). OR'd into each tool's `always_load` bit at list time.
    #[serde(default, skip_serializing_if = "is_false")]
    pub always_load: bool,
}

/// `skip_serializing_if` predicate: omit a `bool` field from the serialized
/// form when it holds its `false` default (keeps the on-wire shape unchanged
/// for the common case).
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(b: &bool) -> bool {
    !*b
}

/// Origin of an [`McpServerConfig`]; drives the approval policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[allow(missing_docs)]
pub enum ConfigScope {
    Local,
    User,
    Project,
    Dynamic,
    Enterprise,
    ClaudeAi,
    Managed,
    /// Agent frontmatter `mcpServers` (claude scope `"agent"`, stamped by
    /// `agentMcpSpecsToScopedConfigs`). Session-scoped like [`Self::Dynamic`],
    /// but NEVER project-approval-gated (claude's approval prompt covers
    /// `.mcp.json` project servers only) and subject to the enterprise
    /// allow/deny policy at merge time (claude `Z__` contains `"agent"`).
    Agent,
}

/// State machine for one MCP connection.
///
/// All transitions go through [`crate::registry::McpRegistry`]; consumers
/// only read the current state.
#[derive(Debug, Clone)]
pub enum McpConnectionState {
    /// Not yet connected, optionally carrying the most recent error.
    Disconnected {
        /// Config the registry will use on the next connect attempt.
        config: McpServerConfig,
        /// Last error, if any.
        last_error: Option<String>,
    },
    /// Connect call is in-flight.
    Connecting {
        /// Config being connected.
        config: McpServerConfig,
        /// When the attempt started.
        started_at: SystemTime,
    },
    /// Connect handshake is waiting on an OAuth callback.
    AwaitingOAuth {
        /// Config waiting for OAuth completion.
        config: McpServerConfig,
        /// Loopback port we are listening on.
        callback_port: u16,
    },
    /// Active connection with discovered capabilities and tools.
    Connected {
        /// Config for the active connection.
        config: McpServerConfig,
        /// Transport-issued connection identifier.
        connection_id: McpConnectionId,
        /// Server capabilities returned by `initialize`.
        capabilities: ServerCapabilitiesDto,
        /// Tools advertised by the server.
        tools: Vec<McpToolDto>,
        /// Resources advertised by the server.
        resources: Vec<McpResourceDto>,
        /// Prompts advertised by the server.
        prompts: Vec<McpPromptDto>,
        /// When the connection became `Connected`.
        connected_at: SystemTime,
    },
    /// A liveness ping is in-flight.
    HealthChecking {
        /// Connection being pinged.
        connection_id: McpConnectionId,
        /// Config of the connection being pinged.
        config: McpServerConfig,
    },
    /// Backoff before the next reconnect attempt.
    Reconnecting {
        /// Config to retry.
        config: McpServerConfig,
        /// Consecutive failure count.
        retry_count: u32,
        /// Earliest time the next attempt may run.
        next_retry_at: SystemTime,
    },
    /// Permanently failed after exhausting retries.
    Failed {
        /// Config that failed.
        config: McpServerConfig,
        /// Most recent error message.
        error: String,
        /// Total attempts made.
        attempts: u32,
    },
    /// Explicitly stopped by the user.
    Stopped {
        /// Config of the stopped connection.
        config: McpServerConfig,
    },
}

impl McpConnectionState {
    /// Logical server name carried by every variant.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Disconnected { config, .. }
            | Self::Connecting { config, .. }
            | Self::AwaitingOAuth { config, .. }
            | Self::Connected { config, .. }
            | Self::HealthChecking { config, .. }
            | Self::Reconnecting { config, .. }
            | Self::Failed { config, .. }
            | Self::Stopped { config } => &config.name,
        }
    }

    /// The originating [`McpServerConfig`], carried by every variant. Used by
    /// [`crate::registry::McpRegistry::reconnect`] to re-establish a connection
    /// after tearing the live one down.
    #[must_use]
    pub fn config(&self) -> &McpServerConfig {
        match self {
            Self::Disconnected { config, .. }
            | Self::Connecting { config, .. }
            | Self::AwaitingOAuth { config, .. }
            | Self::Connected { config, .. }
            | Self::HealthChecking { config, .. }
            | Self::Reconnecting { config, .. }
            | Self::Failed { config, .. }
            | Self::Stopped { config } => config,
        }
    }

    /// Transport-kind label of the connection's config — `"stdio"`,
    /// `"sse"`, `"http"`, etc. Used by [`crate::registry::McpRegistry::snapshot`]
    /// (M6-07) to populate `McpServerInfo::transport`.
    #[must_use]
    pub fn transport_kind(&self) -> &'static str {
        let cfg = match self {
            Self::Disconnected { config, .. }
            | Self::Connecting { config, .. }
            | Self::AwaitingOAuth { config, .. }
            | Self::Connected { config, .. }
            | Self::HealthChecking { config, .. }
            | Self::Reconnecting { config, .. }
            | Self::Failed { config, .. }
            | Self::Stopped { config } => config,
        };
        cfg.spec.kind()
    }
}

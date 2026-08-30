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
    /// Config-level error that makes the server unconnectable (claude
    /// `configError`, reason `url_invalid`): set at parse time when a remote
    /// entry's `url` expanded to an empty string. The server is KEPT in the
    /// inventory, but the connect path short-circuits to a failure WITHOUT
    /// dialing.
    ///
    /// This is claude's `INVALID_CONFIG`, not `UNCONFIGURED`: `klr`
    /// (@231828222) tags the case `configErrorReason:"url_invalid"`, so `zar`
    /// (@231408681) is false on both disjuncts and `Nxe` (@232117552) falls
    /// through the unconfigured gate into the next one →
    /// `errorCode:"INVALID_CONFIG"`. `Qee` (@231862992) is therefore false and
    /// `yEp` renders `✘ Failed to connect` with this text as the issue —
    /// `- Not configured` is reserved for [`McpServerConfig::is_unconfigured`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_error: Option<String>,
}

/// claude's error text for a server with nothing to dial (`Nxe`'s
/// `t.configError ?? "No URL configured for this server"`).
pub const UNCONFIGURED_ERROR: &str = "No URL configured for this server";

/// claude's byte-exact `configError` text for a non-blank but syntactically
/// malformed `url` (`Ve`/`Ae` @182283xxx: `try{new URL(t.url)}catch{C="'url'
/// is not a valid URL. Update the server's config and reconnect."}`). Unlike
/// [`UNCONFIGURED_ERROR`] (a blank url) this fires on a url that IS present
/// but does not parse — e.g. a bare hostname with no scheme. See
/// [`McpServerConfig::connect_time_url_error`].
pub const INVALID_URL_ERROR: &str = "'url' is not a valid URL. Update the server's config and reconnect.";

impl McpServerConfig {
    /// claude `zar` (@231408681) — is this server *unconfigured* (nothing to
    /// dial) rather than *misconfigured*? True when no [`Self::config_error`]
    /// was recorded and the transport carries a blank `url`. `Nxe` then skips
    /// the connect with `errorCode:"UNCONFIGURED"`, the ONLY case `mcp list` /
    /// `mcp get` render as `- Not configured` (`Qee` → `yEp`).
    ///
    /// `zar`'s other disjunct (`configErrorReason === "url_empty"`) is produced
    /// only by the plugin-MCP normalizer (@231445287), a path this port does
    /// not model, so the reason discriminator is not carried on the config.
    #[must_use]
    pub fn is_unconfigured(&self) -> bool {
        if self.config_error.is_some() {
            return false;
        }
        // `"url" in e` — the transports whose config object declares a `url`.
        let url = match &self.spec {
            McpTransportSpec::Sse { url, .. }
            | McpTransportSpec::Http { url, .. }
            | McpTransportSpec::WebSocket { url, .. }
            | McpTransportSpec::SseIde { url, .. } => url,
            McpTransportSpec::Stdio { .. }
            | McpTransportSpec::InProcess { .. }
            | McpTransportSpec::SdkControl { .. } => return false,
        };
        url.trim().is_empty()
    }

    /// §18 — claude's CONNECT-TIME url re-validation, run in `Ve`/`Ae`
    /// (2.1.251 @182283917 / @182488170) immediately after the
    /// [`Self::is_unconfigured`] gate and BEFORE dialing:
    /// `let C=t.configError; if(!C&&"url"in t) try{new URL(t.url)}
    /// catch{C="'url' is not a valid URL. ..."}`. It fires even when no
    /// loader-time [`Self::config_error`] was ever recorded — the loader
    /// (`mcp/src/json_config.rs`) only stamps `config_error` for a url that
    /// EXPANDED to empty, so a syntactically invalid but non-empty url (a
    /// bare hostname with no scheme, say) reaches this port's connect path
    /// completely unflagged today.
    ///
    /// Returns `None` when a [`Self::config_error`] is already recorded
    /// (that one wins — oracle telemetry tags it `source:"loader"` instead of
    /// this method's implicit `source:"connect"`, `tengu_mcp_server_config_invalid`,
    /// deferred by name), when the transport carries no `url` field, or when
    /// the url is blank ([`Self::is_unconfigured`] owns that case:
    /// `errorCode:"UNCONFIGURED"`, not `"INVALID_CONFIG"`) or parses.
    ///
    /// The oracle's `errorCode` for a `Some` return is `"INVALID_CONFIG"` —
    /// the same code [`Self::config_error`] already produces at its one
    /// existing call site (`mcp/src/registry.rs::connect_locked_inner`), so
    /// wiring this in is a one-line `.or_else` alongside that check, not a
    /// new branch. NOT YET WIRED there (registry.rs is out of this task's
    /// file ownership) — see `McpConnectErrorCode` doc below.
    #[must_use]
    pub fn connect_time_url_error(&self) -> Option<&'static str> {
        if self.config_error.is_some() {
            return None;
        }
        let url = match &self.spec {
            McpTransportSpec::Sse { url, .. }
            | McpTransportSpec::Http { url, .. }
            | McpTransportSpec::WebSocket { url, .. }
            | McpTransportSpec::SseIde { url, .. } => url,
            McpTransportSpec::Stdio { .. }
            | McpTransportSpec::InProcess { .. }
            | McpTransportSpec::SdkControl { .. } => return None,
        };
        if url.trim().is_empty() {
            return None; // `is_unconfigured`'s case, not this one.
        }
        if url::Url::parse(url).is_err() {
            Some(INVALID_URL_ERROR)
        } else {
            None
        }
    }
}

/// Oracle `errorCode` vocabulary for a failed MCP connect attempt — the
/// discriminator field on claude's `{type:"failed", ...}` (`Nxe`/`Ve`/`Ae`).
/// The frozen [`traits::McpError`] carries no code field (byte-exact display
/// text only — see the doc on [`McpServerConfig::config_error`]), so a
/// caller needing the oracle's discriminator re-derives it from the config
/// plus the failure text, exactly as [`McpServerConfig::is_unconfigured`]
/// already does for `UNCONFIGURED` alone. [`Self::classify`] generalizes
/// that to the three codes buildable from information already on this port's
/// connect path.
///
/// Deliberately NOT covered here:
/// - `AUTH_HEADER_REJECTED` / `HEADERS_HELPER_AUTH_REJECTED` (§19, LANDED in
///   `crate::negotiation::classify_auth_failure`) — that function returns a
///   byte-exact MESSAGE, not a discriminated code, and lives outside this
///   task's file ownership (`mcp/src/registry.rs`/`negotiation.rs` wiring).
/// - `FIRST_PARTY_AUTH_REJECTED` — a deliberate non-goal per
///   `crate::negotiation`'s module doc (no first-party-auth/claudeai-proxy
///   bearer concept exists in this port to trigger it).
/// - Every code in the discovery-cache / server-identity-epoch family
///   (`lazy_dial_failed`, the `cached-row ... subscriber threw:` log lines,
///   `IDENTITY_CHANGED`/`mcp_reconnect_identity_changed`) and the `roots/list`
///   staging-root log line — these belong to an `identityBaseline` /
///   `identityEpoch` / cached-row-subscriber subsystem this port has no
///   analogue of at all (confirmed absent — see §24e in
///   `docs/mcp-plugin-byte-alignment-2.1.251-2026-08-28.md`), not a gap in
///   this error-code model. Building them means building that subsystem
///   first; out of scope for this task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpConnectErrorCode {
    /// Oracle `errorCode:"UNCONFIGURED"` — [`McpServerConfig::is_unconfigured`].
    Unconfigured,
    /// Oracle `errorCode:"INVALID_CONFIG"` — a loader-time
    /// [`McpServerConfig::config_error`] OR this port's connect-time
    /// [`McpServerConfig::connect_time_url_error`] re-check.
    InvalidConfig,
    /// Oracle `errorCode:"CONNECT_TIMEOUT"` (2.1.251 @182283392 `xo`/@182487645
    /// `No`): `Object.assign(new R(msg,"MCP connection timeout"),
    /// {code:"CONNECT_TIMEOUT"})`, gated `tengu_mcp_connect_timeout_retry`
    /// (default-on; DEFERRED — no feature-flag plumbing exists in this port,
    /// so the tag applies unconditionally, matching the flag's shipped
    /// default). NOTE the correction: `"MCP connection timeout"` is the
    /// `TelemetrySafeError`'s SECOND constructor arg — `telemetryMessage`, a
    /// generic label used only for telemetry hashing — NOT the user-visible
    /// text. The displayed message stays the oracle's first arg, the exact
    /// detailed string this port's `registry.rs::connect_attempt` already
    /// emits (`MCP server "{name}" connection timed out after {ms}ms`); no
    /// message text changes, only the missing discriminator.
    ConnectTimeout,
    /// Any other failure (a real transport/handshake error, an auth-type
    /// rejection classified by message text only, etc.) — the oracle's
    /// remaining codes are out of this model's scope (see the type doc).
    Other,
}

impl McpConnectErrorCode {
    /// Re-derive the oracle's `errorCode` for one connect failure.
    /// `config` is the server's static config; `message` is the text the
    /// connect attempt actually failed with (`McpError`'s `Display`, or a
    /// stored `McpConnectionState::Failed::error`).
    ///
    /// Ordering mirrors the oracle's `Nxe`: unconfigured is checked FIRST
    /// (never dials), then invalid config (never dials either), and only
    /// once both pass could a real dial have happened — so `ConnectTimeout`
    /// is only reachable once neither pre-dial gate applies.
    #[must_use]
    pub fn classify(config: &McpServerConfig, message: &str) -> Self {
        if config.is_unconfigured() {
            return Self::Unconfigured;
        }
        if config.config_error.is_some() || config.connect_time_url_error().is_some() {
            return Self::InvalidConfig;
        }
        if is_connect_timeout_message(message) {
            return Self::ConnectTimeout;
        }
        Self::Other
    }
}

/// Matches the exact text `registry.rs::connect_attempt`'s `timeout_error`
/// closure emits (`MCP server "{name}" connection timed out after {ms}ms`) —
/// the only producer of this text on the connect path today.
fn is_connect_timeout_message(message: &str) -> bool {
    message.contains("connection timed out after")
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

#[cfg(test)]
mod tests {
    use super::*;
    use traits::McpHeaders;

    fn cfg(spec: McpTransportSpec, config_error: Option<&str>) -> McpServerConfig {
        McpServerConfig {
            name: "srv".to_string(),
            spec,
            scope: ConfigScope::User,
            disabled: false,
            timeout_ms: None,
            always_load: false,
            config_error: config_error.map(str::to_string),
        }
    }

    fn http_spec(url: &str) -> McpTransportSpec {
        McpTransportSpec::Http {
            url: url.to_string(),
            headers: McpHeaders::default(),
            headers_helper: None,
            oauth: None,
        }
    }

    fn stdio_spec() -> McpTransportSpec {
        McpTransportSpec::Stdio {
            command: "cmd".to_string(),
            args: vec![],
            env: std::collections::HashMap::default(),
        }
    }

    // ── `connect_time_url_error` ──

    #[test]
    fn malformed_nonempty_url_is_flagged_invalid() {
        // A non-empty url with no scheme is exactly the oracle's `try{new
        // URL(t.url)}catch{...}` failure case — `is_unconfigured` (a
        // trim().is_empty() check) does NOT catch it, so without this method
        // it would reach the dial unflagged.
        let c = cfg(http_spec("not-a-url"), None);
        assert!(!c.is_unconfigured(), "non-empty url is not UNCONFIGURED");
        assert_eq!(c.connect_time_url_error(), Some(INVALID_URL_ERROR));
    }

    #[test]
    fn wellformed_url_is_not_flagged() {
        let c = cfg(http_spec("https://mcp.example/api"), None);
        assert_eq!(c.connect_time_url_error(), None);
    }

    #[test]
    fn blank_url_is_unconfigured_not_invalid() {
        // `is_unconfigured` owns the blank-url case (`errorCode:"UNCONFIGURED"`);
        // this method must defer to it, not double-report as INVALID_CONFIG.
        let c = cfg(http_spec(""), None);
        assert!(c.is_unconfigured());
        assert_eq!(c.connect_time_url_error(), None);
    }

    #[test]
    fn existing_loader_config_error_wins_over_the_connect_time_check() {
        // A loader-stamped `config_error` (source:"loader") must not be
        // overridden or duplicated by this connect-time (source:"connect")
        // re-check, even when the url also happens to be unparseable.
        let c = cfg(http_spec("not-a-url"), Some("expanded to an empty string"));
        assert_eq!(c.connect_time_url_error(), None);
    }

    #[test]
    fn transports_without_a_url_field_are_never_flagged() {
        let c = cfg(stdio_spec(), None);
        assert_eq!(c.connect_time_url_error(), None);
    }

    // ── `McpConnectErrorCode::classify` ──

    #[test]
    fn classify_unconfigured_beats_everything_else() {
        let c = cfg(http_spec(""), None);
        assert_eq!(
            McpConnectErrorCode::classify(&c, "irrelevant message"),
            McpConnectErrorCode::Unconfigured
        );
    }

    #[test]
    fn classify_invalid_config_from_loader_error() {
        let c = cfg(http_spec("https://ok.example"), Some("bad config"));
        assert_eq!(
            McpConnectErrorCode::classify(&c, "irrelevant message"),
            McpConnectErrorCode::InvalidConfig
        );
    }

    #[test]
    fn classify_invalid_config_from_connect_time_url_check() {
        let c = cfg(http_spec("not-a-url"), None);
        assert_eq!(
            McpConnectErrorCode::classify(&c, "irrelevant message"),
            McpConnectErrorCode::InvalidConfig
        );
    }

    #[test]
    fn classify_connect_timeout_from_message_text() {
        let c = cfg(http_spec("https://ok.example"), None);
        let msg = r#"MCP server "srv" connection timed out after 30000ms"#;
        assert_eq!(
            McpConnectErrorCode::classify(&c, msg),
            McpConnectErrorCode::ConnectTimeout
        );
    }

    #[test]
    fn classify_other_for_an_ordinary_transport_failure() {
        let c = cfg(http_spec("https://ok.example"), None);
        assert_eq!(
            McpConnectErrorCode::classify(&c, "connection refused"),
            McpConnectErrorCode::Other
        );
    }
}

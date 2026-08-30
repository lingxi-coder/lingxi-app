//! `tengu_mcp_*` **analytics-event** schemas (2.1.251 byte-alignment §18/§20b).
//!
//! This module is deliberately narrow: a full-binary grep for `tengu_mcp_`
//! turns up 100+ hits, but the great majority are **Statsig feature-flag**
//! names read via `I(name, default)` / `Ot(name, e)` (e.g.
//! `tengu_mcp_protocol_negotiation_http`, `tengu_mcp_normalize_root_combinators`,
//! `tengu_mcp_connect_timeout_retry`), not analytics events with a payload.
//! Those are already ported as plain `&str` constants passed to
//! [`telemetry::flag_bool`] at their call sites (`mcp/src/protocol_negotiation.rs`,
//! `mcp/src/tool_schema.rs`) — mirroring the existing
//! `tengu_surface_failed_mcp_servers` pattern — and do NOT belong in this
//! count-locked event registry.
//!
//! The two names below are confirmed, by direct oracle disassembly, to be
//! real `s("tengu_mcp_…", {…})`-style analytics-bus emissions (the same `s`
//! call site used by every other `tengu_*` event in the binary):
//!
//! - `tengu_mcp_server_config_invalid` — `mcp/src/connection.rs`'s
//!   `Transport::connect_time_url_error` / `config_error` paths already
//!   compute the oracle's `INVALID_CONFIG` classification and explicitly
//!   defer this event "by name" (see that file's doc comment). Oracle call
//!   site (`registry.rs`-equivalent connect path):
//!   `s("tengu_mcp_server_config_invalid",{transportType:c(t.type??"stdio"),
//!   field:w("url"),source:w(t.configError?"loader":"connect")})`.
//! - `tengu_mcp_tools_listed` — emitted once per successful `tools/list`.
//!   Oracle call site: `s("tengu_mcp_tools_listed",{transportType:c(e.config.type
//!   ??"stdio"),listDurationMs:Date.now()-o,toolCount:L.length,
//!   alwaysLoadCount:Q(L,(E)=>E.alwaysLoad===!0),discoverySource:c(r),..._,
//!   mcpServerName:EA(ln(e.name),HT(e.name,e.config))})`.
//!
//!   The byte-alignment doc (§20b) additionally lists `normalizedCount` /
//!   `keptCount` as fields of this event. **That does not hold at the
//!   oracle**: those two keys sit only *adjacent* in the binary's string
//!   pool (next to the `tool_schema_*` classification labels), never inside
//!   this event's actual object-literal call site. The `..._` spread above is
//!   a distinct, not-yet-traced object (most likely the schema-normalization
//!   counts) — omitted here rather than guessed at, so
//!   [`ToolsListedPayload`] models only the six confirmed keys. Whoever wires
//!   the emit site must trace `_`'s producer and extend the payload before
//!   adding those fields; do not add them from the doc's guess alone.
//!
//! Sibling copy/error-code work named alongside these in §18/§19/§20b
//! (`CONNECT_TIMEOUT`/`AUTH_HEADER_REJECTED`/… `errorCode`s, the
//! `mcp_list_tools_*` / `mcp_connect_*` OTel log-gate names, and the plain
//! user-facing copy strings) is NOT telemetry-event substrate and is left for
//! the behaviour/copy tasks that own those call sites.

use crate::pii::Verified;
use serde::{Deserialize, Serialize};

/// `tengu_mcp_server_config_invalid` — a server's config failed the
/// connect-time (or loader-time) URL/shape re-validation.
pub const SERVER_CONFIG_INVALID: &str = "tengu_mcp_server_config_invalid";
/// `tengu_mcp_tools_listed` — a `tools/list` round-trip completed and the
/// tool set was bound.
pub const TOOLS_LISTED: &str = "tengu_mcp_tools_listed";

/// Registry block — order is locked (append-only). Consumed by
/// [`crate::tengu::ALL_EVENT_NAMES`].
pub const NAMES: &[&str] = &[SERVER_CONFIG_INVALID, TOOLS_LISTED];

/// Where the invalid-config classification was raised. Oracle:
/// `source:w(t.configError?"loader":"connect")`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ConfigInvalidSource {
    /// Stamped at config-load time (`mcp/src/json_config.rs`) — a url that
    /// expanded to empty.
    Loader,
    /// Raised at connect time — a syntactically invalid, non-empty url.
    Connect,
}

/// Payload for [`SERVER_CONFIG_INVALID`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfigInvalidPayload {
    /// Transport kind (`stdio`/`sse`/`http`/…), defaulted like the oracle's
    /// `c(t.type??"stdio")`.
    pub transport_type: Verified,
    /// The offending config field. Oracle only ever passes the literal
    /// `"url"` here (the sole connect-time re-validation target).
    pub field: Verified,
    /// Loader vs. connect-time classification.
    pub source: ConfigInvalidSource,
}

/// Payload for [`TOOLS_LISTED`]. See the module doc for the two fields the
/// byte-alignment audit's guess did NOT hold for (`normalizedCount` /
/// `keptCount`) — intentionally absent here.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolsListedPayload {
    /// Transport kind, same default rule as [`ServerConfigInvalidPayload`].
    pub transport_type: Verified,
    /// Wall-clock duration of the `tools/list` round-trip in milliseconds.
    pub list_duration_ms: u64,
    /// Number of tools returned.
    pub tool_count: u32,
    /// Number of returned tools flagged `alwaysLoad: true`.
    pub always_load_count: u32,
    /// Discovery source (`live`, cached-row adoption, …).
    pub discovery_source: Verified,
    /// The (possibly disambiguated) server display name.
    pub mcp_server_name: Verified,
}

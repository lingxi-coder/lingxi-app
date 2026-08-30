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
//!   this event's actual object-literal call site, so [`ToolsListedPayload`]
//!   models only the six confirmed keys.
//!
//! ## `normalizedCount` / `keptCount` traced — they belong to a THIRD event
//!
//! The two keys this module's earlier revision left as an open question
//! (see above) are real, but on `tengu_mcp_degraded`
//! ([`DEGRADED`]), not `tengu_mcp_tools_listed`. Oracle `yn` (the tool-list
//! post-processing pass, `cc_all.txt` @182316780-182319300) tallies SEVEN
//! disjoint counters while walking one server's tool list — `x`
//! (normalized), `W` (would-normalize but the gate is off), `ue` (an
//! unsupported, non-array root combinator), `_e`/`xe` (meta-schema /
//! property-key invalid, drop-gate ON), `F`/`X` (the same two, drop-gate
//! OFF) — then fires ONE `tengu_mcp_degraded` per NONZERO counter, after the
//! whole list is processed (not one event per tool):
//! ```text
//! if(x>0)s("tengu_mcp_degraded",{reason:w("tool_schema_normalized"),
//!   transportType:c(e.config.type??"stdio"),normalizedCount:x,
//!   mcpServerName:P,..._});
//! if(W>0)s("tengu_mcp_degraded",{reason:w("tool_schema_normalize_gated"),
//!   transportType:c(e.config.type??"stdio"),skippedCount:W,
//!   mcpServerName:P,..._});
//! // ...unsupported/invalid/property_key_invalid follow the same shape,
//! // skippedCount when drop-gate ON, keptCount when drop-gate OFF (the
//! // "_gated" reason variants).
//! ```
//! A SEPARATE, process-global call site (`qr()`'s `M===null` arm,
//! @182172950) fires `tengu_mcp_degraded` with reason
//! `schema_validator_unavailable` and no count/transport/server fields at
//! all, when the bundled meta-schema validator fails to compile.
//!
//! The `..._` spread on every per-server variant is the SAME
//! not-yet-traced identity object [`ToolsListedPayload`]'s doc flags —
//! omitted here for the same reason (no guessed field beats an honest
//! smaller payload).
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
/// `tengu_mcp_degraded` — one of the §20a tool-schema classifications (or
/// the process-global validator-unavailable fallback) fired at least once.
pub const DEGRADED: &str = "tengu_mcp_degraded";

/// Registry block — order is locked (append-only). Consumed by
/// [`crate::tengu::ALL_EVENT_NAMES`].
pub const NAMES: &[&str] = &[SERVER_CONFIG_INVALID, TOOLS_LISTED, DEGRADED];

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

/// `tengu_mcp_degraded`'s `reason` — the eight oracle-confirmed values (see
/// the module doc's `yn` trace). `#[non_exhaustive]` matches this module's
/// established convention ([`ConfigInvalidSource`]) for an oracle enum that
/// could gain siblings in a later release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum DegradedReason {
    /// A root-combinator schema was flattened (the normalize gate was ON).
    /// Carries [`DegradedPayload::normalized_count`].
    ToolSchemaNormalized,
    /// A root-combinator schema WOULD have been flattened but the normalize
    /// gate was OFF, so the tool was dropped instead. Carries
    /// [`DegradedPayload::skipped_count`].
    ToolSchemaNormalizeGated,
    /// A root-combinator value couldn't be represented as a flattened
    /// schema at all (unconditionally dropped, no gate involved). Carries
    /// [`DegradedPayload::skipped_count`].
    ToolSchemaUnsupported,
    /// The schema failed the meta-schema validity check and the drop gate
    /// was ON (tool dropped). Carries [`DegradedPayload::skipped_count`].
    ToolSchemaInvalid,
    /// A top-level property key failed the naming regex and the drop gate
    /// was ON (tool dropped). Carries [`DegradedPayload::skipped_count`].
    ToolPropertyKeyInvalid,
    /// The schema failed the meta-schema validity check but the drop gate
    /// was OFF (tool kept, with a warning). Carries
    /// [`DegradedPayload::kept_count`].
    ToolSchemaInvalidGated,
    /// A top-level property key failed the naming regex but the drop gate
    /// was OFF (tool kept, with a warning). Carries
    /// [`DegradedPayload::kept_count`].
    ToolPropertyKeyInvalidGated,
    /// The bundled JSON-Schema 2020-12 meta-validator failed to compile —
    /// process-global, fires at most once, carries none of the per-server
    /// fields (no `transport_type`/count/`mcp_server_name`).
    SchemaValidatorUnavailable,
}

impl DegradedReason {
    /// The wire (`snake_case`) string this variant serializes to — used by
    /// [`crate::emit_mcp_degraded`], which needs the bare string for a
    /// `tracing` field rather than a JSON round-trip.
    #[must_use]
    pub fn wire_str(self) -> &'static str {
        match self {
            Self::ToolSchemaNormalized => "tool_schema_normalized",
            Self::ToolSchemaNormalizeGated => "tool_schema_normalize_gated",
            Self::ToolSchemaUnsupported => "tool_schema_unsupported",
            Self::ToolSchemaInvalid => "tool_schema_invalid",
            Self::ToolPropertyKeyInvalid => "tool_property_key_invalid",
            Self::ToolSchemaInvalidGated => "tool_schema_invalid_gated",
            Self::ToolPropertyKeyInvalidGated => "tool_property_key_invalid_gated",
            Self::SchemaValidatorUnavailable => "schema_validator_unavailable",
        }
    }
}

/// Payload for [`DEGRADED`]. Exactly one of `normalized_count` /
/// `skipped_count` / `kept_count` is populated, matching whichever
/// [`DegradedReason`] fired — see each variant's doc. The
/// [`DegradedReason::SchemaValidatorUnavailable`] case populates none of
/// them, nor `transport_type` / `mcp_server_name` (it is not scoped to a
/// server or a tool list).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DegradedPayload {
    /// Which classification fired.
    pub reason: DegradedReason,
    /// Transport kind, same default rule as [`ServerConfigInvalidPayload`].
    /// Absent only for [`DegradedReason::SchemaValidatorUnavailable`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transport_type: Option<Verified>,
    /// Count of tools normalized on this server this pass.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub normalized_count: Option<u32>,
    /// Count of tools dropped on this server this pass.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped_count: Option<u32>,
    /// Count of tools kept-with-warning on this server this pass.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kept_count: Option<u32>,
    /// The (possibly disambiguated) server display name. Absent only for
    /// [`DegradedReason::SchemaValidatorUnavailable`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp_server_name: Option<Verified>,
}

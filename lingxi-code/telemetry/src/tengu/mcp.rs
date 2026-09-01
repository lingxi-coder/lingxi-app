//! `tengu_mcp_*` **analytics-event** schemas (2.1.251 byte-alignment §18/§20b,
//! §11 discovery-cache).
//!
//! ## Scope: 4 of the oracle's 53 `tengu_mcp_*` analytics events
//!
//! An earlier revision of this doc claimed the two originally-ported names
//! were "the ONLY two confirmed real `tengu_mcp_*` ANALYTICS events at the
//! oracle; everything else with that prefix is a Statsig feature-flag name".
//! **That claim was false and has been retracted.** Scanning the 2.1.251
//! image for the analytics-bus call shape
//! `(?<![A-Za-z0-9_$])s\("tengu_mcp_[a-z0-9_]+"` returns **53 distinct event
//! names**, among them `tengu_mcp_server_connection_succeeded` /
//! `_failed`, `tengu_mcp_list_changed`, `tengu_mcp_listen_reopen`,
//! `tengu_mcp_sdk_generation`, `tengu_mcp_oauth_flow_start`/`_success`/
//! `_failure`/`_error`, `tengu_mcp_registry_fetch`,
//! `tengu_mcp_elicitation_shown`/`_response`,
//! and `tengu_mcp_first_party_auto_auth`. Several are independently
//! corroborated by the oracle's own event allowlist array (@156122853).
//!
//! It IS separately true that a full-binary grep for the bare prefix
//! `tengu_mcp_` turns up 100+ hits of which many are **Statsig
//! feature-flag** names read via `I(name, default)` / `Ot(name, e)` (e.g.
//! `tengu_mcp_protocol_negotiation_http`,
//! `tengu_mcp_normalize_root_combinators`,
//! `tengu_mcp_connect_timeout_retry`). Those are ported as plain `&str`
//! constants passed to [`telemetry::flag_bool`] at their call sites and do
//! NOT belong in this count-locked event registry. But "not every
//! `tengu_mcp_*` string is an event" does not license "only two strings are
//! events", and conflating the two produced both the retracted claim below
//! and an under-modelled [`DegradedReason`].
//!
//! **This module deliberately ports 4 of the 53.** The remaining 49 are an
//! OPEN parity gap (§20b-remainder), not a closed one. Do not read
//! `NAMES.len() == 4`, [`crate::tengu::ALL_EVENT_NAMES`], or the
//! `tengu_events.json` parity fixture as evidence that MCP analytics is
//! complete.
//!
//! ## The four ported events
//!
//! - `tengu_mcp_server_config_invalid` — `mcp/src/connection.rs`'s
//!   `Transport::connect_time_url_error` / `config_error` paths already
//!   compute the oracle's `INVALID_CONFIG` classification. Oracle call
//!   site: `s("tengu_mcp_server_config_invalid",{transportType:c(t.type
//!   ??"stdio"),field:w("url"),source:w(t.configError?"loader":"connect")})`.
//! - `tengu_mcp_tools_listed` — emitted once per successful `tools/list`.
//!   Oracle call site: `s("tengu_mcp_tools_listed",{transportType:c(e.config.type
//!   ??"stdio"),listDurationMs:Date.now()-o,toolCount:L.length,
//!   alwaysLoadCount:Q(L,(E)=>E.alwaysLoad===!0),discoverySource:c(r),..._,
//!   mcpServerName:EA(ln(e.name),HT(e.name,e.config))})`.
//! - `tengu_mcp_degraded` — see [`DegradedReason`].
//! - `tengu_mcp_discovery_source` — §11 discovery-cache observability. See
//!   [`DiscoverySourcePayload`] for the two oracle call sites (a fresh/stale
//!   HIT, and a MISS gated by `Ko`) and `mcp::discovery_cache`'s module doc
//!   for what this port actually wires (today: the MISS side only).
//!
//! ## `listDurationMs` measures `tools/list` ALONE
//!
//! Oracle `yt` (@182326900): `let d=Date.now(), …, h=await …"tools/list"…,
//! _=yn(e,h,d,"live",r)`, and inside `yn`: `listDurationMs:Date.now()-o`
//! with `o` bound to that `d`. The timer opens immediately before the
//! `tools/list` round-trip and closes immediately after it — no
//! `resources/list` or `prompts/list` call sits inside the window.
//!
//! ## `mcpServerName` is GATED, and absent for ordinary servers
//!
//! The oracle does NOT attach the raw server name unconditionally. It
//! computes `P = EA(ln(e.name), HT(e.name, e.config))` where
//! `EA(n,e){return e?Vo(n):void 0}` (@153570482) — returning `undefined`,
//! which the object spread DROPS, unless the predicate holds. The predicate
//! `HT(e,t)` (@156027690) is:
//!
//! ```text
//! function HT(e,t){
//!   if(t===void 0){ if(Z3t.has(e))return!0; return $M(void 0,void 0) }
//!   if(dM(e,t))return!0;                                   // first-party stdio
//!   if("url"in t&&X7e(t.url)&&ln(e)===uy)return!0;         // first-party url
//!   return $M(t.type,vAe(t))
//! }
//! function dM(e,t){return C6(t)&&(OH(e)||F6(e))}           // stdio AND name in {Ed,e0}
//! function $M(e,t){
//!   if(process.env.CLAUDE_CODE_ENTRYPOINT==="local-agent")return!0;
//!   if(e==="claudeai-proxy")return!0;
//!   if(t&&tM(t))return!0;                                   // official registry url
//!   if(t&&rA(t))return!0;                                   // anthropic /v1/design/ url
//!   return!1
//! }
//! ```
//!
//! Every arm is a FIRST-PARTY test: a built-in Anthropic stdio server
//! (`OH`/`F6` compare `ln(name)` against two fixed constants), a
//! `claudeai-proxy` transport, an official-registry URL, or an
//! Anthropic-issued design URL. For an ordinary user-configured server —
//! `acme-internal-payroll` in someone's `.mcp.json` — `HT` is **false** and
//! the oracle emits `tengu_mcp_tools_listed` / `tengu_mcp_degraded` with no
//! `mcpServerName` key at all.
//!
//! Modelling the field as required-and-always-populated therefore did not
//! just diverge from the oracle, it turned a private server name into an
//! analytics dimension on every connect. The field is now
//! `Option<Verified>`; see [`server_name_gate`] for what this port can and
//! cannot evaluate.
//!
//! ## Known-missing: the `..._` identity spread
//!
//! Every per-server emission above also spreads `_ = Xe(e.config, e.name)`
//! (@182265525): `function Xe(e,t){let o=Lg(e),r=t?{mcpServerKeyHash:eP(t)}:{};
//! if(o)return{mcpServerBaseUrl:o,...r};return r}` — i.e. an optional
//! `mcpServerBaseUrl` plus `mcpServerKeyHash`, the hashed identifier the
//! oracle carries in place of the raw name. Neither is modelled here:
//! `eP`'s digest is not traced, and inventing a hash would fabricate wire
//! data rather than port it. Recorded as an open gap, not a settled shape.

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
/// `tengu_mcp_discovery_source` — §11 discovery-cache observability. See
/// [`DiscoverySourcePayload`].
pub const DISCOVERY_SOURCE: &str = "tengu_mcp_discovery_source";

/// Registry block — order is locked (append-only). Consumed by
/// [`crate::tengu::ALL_EVENT_NAMES`].
pub const NAMES: &[&str] = &[
    SERVER_CONFIG_INVALID,
    TOOLS_LISTED,
    DEGRADED,
    DISCOVERY_SOURCE,
];

/// Oracle `HT(name, config)` — decides whether `mcpServerName` is attached
/// to `tengu_mcp_tools_listed` / `tengu_mcp_degraded` at all (see the module
/// doc for the full disassembly). Every arm of `HT` tests for a FIRST-PARTY
/// Anthropic server; for an ordinary user-configured server it is false and
/// the oracle omits the key entirely.
///
/// # This port evaluates to `false` for everything it can currently build
///
/// The gate's four true-arms are, one by one, unreachable here:
///
/// - `dM` (stdio AND `ln(name)` equal to one of two fixed built-in server
///   names, `OH`@155370892 / `F6`@155377771) — this port has neither
///   built-in server, so no configured name can match.
/// - `"url" in t && X7e(t.url) && ln(e)===uy` — same two fixed names.
/// - `$M`'s `type === "claudeai-proxy"` — [`platform_api::McpTransportSpec`] has
///   no `claudeai-proxy` variant (`kind()` yields only `stdio`/`sse`/`http`/
///   `websocket`/`inprocess`/`sse-ide`/`sdk-control`), so no live spec can
///   produce that string.
/// - `$M`'s `tM(url)` / `rA(url)` official-registry and Anthropic
///   `/v1/design/` URL allowlists — not ported.
///
/// Returning a constant `false` is therefore the byte-correct answer for
/// every server this port can reach, and it is safe by construction: the
/// only possible divergence is under-reporting the name of a first-party
/// server that cannot exist here, never leaking a user's private one. Wiring
/// a real predicate is the job of whichever change first ports one of the
/// four arms above; this function is the single place to do it.
#[must_use]
pub fn server_name_gate(transport_kind: &str) -> bool {
    // `$M`'s `e === "claudeai-proxy"` arm, spelled out so that adding the
    // variant to `McpTransportSpec` lights this up without a second search.
    // The remaining three arms need substrate this port does not have.
    transport_kind == "claudeai-proxy"
}

/// Where the invalid-config classification was raised. Oracle:
/// `source:w(t.configError?"loader":"connect")`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ConfigInvalidSource {
    /// Stamped at config-load time (`mcp/src/json_config.rs`) — a url that
    /// expanded to empty.
    Loader,
    /// Raised at connect time — a syntactically invalid, non-empty url.
    Connect,
}

impl ConfigInvalidSource {
    /// The wire (snake_case) string this variant serializes to — used by
    /// [`crate::emit_mcp_server_config_invalid`], which needs the bare
    /// string for a `tracing` field rather than a `Debug`/JSON round-trip.
    #[must_use]
    pub fn wire_str(self) -> &'static str {
        match self {
            Self::Loader => "loader",
            Self::Connect => "connect",
        }
    }
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
    /// The (possibly disambiguated) server display name — present ONLY when
    /// the oracle's first-party gate holds ([`server_name_gate`]). Absent
    /// for every ordinary user-configured server; see the module doc.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp_server_name: Option<Verified>,
}

/// Payload for [`DISCOVERY_SOURCE`] (§11).
///
/// Two oracle call sites feed this event, both recovered from 2.1.251 in the
/// same chunk as `cot`/`me` (`mcp::discovery_cache`'s module doc has the full
/// disassembly):
///
/// * A fresh/stale HIT (@182535700 region): `s("tengu_mcp_discovery_source",
///   {source:w(U.kind==="fresh"?"cache_fresh":"cache_stale"),
///   transportType:c(E.type??"stdio"),entryAgeMs:ie,...Oe(E,C)})` —
///   unconditional once a `Fresh`/`Stale` decision is reached.
/// * A MISS on the live-dial path (@182538500 region): `if(Ho()&&Ko(U))
///   s("tengu_mcp_discovery_source",{source:w(Jo(U)),
///   transportType:c(E.type??"stdio"),...Oe(E,C)})` — gated by `Ko` (see
///   the `mcp` crate's `discovery_cache::miss_emits_discovery_source_telemetry`,
///   which ports `Ko` byte-exact) and carries NO `entryAgeMs` (there is no
///   entry).
///
/// Both call sites also spread `...Oe(E,C)` — the SAME `mcpServerBaseUrl`/
/// `mcpServerKeyHash` identity spread already documented as known-missing on
/// [`ToolsListedPayload`]'s module doc (`Xe`/`Oe` are chunk-local aliases for
/// one helper); not modelled here for the same reason.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoverySourcePayload {
    /// Transport kind, same default rule as [`ServerConfigInvalidPayload`].
    pub transport_type: Verified,
    /// `cache_fresh` / `cache_stale` (HIT) or one of `mcp`'s
    /// `discovery_cache::miss_telemetry_value` strings (MISS): `live`,
    /// `miss_disabled`, `miss_expired`, `miss_corrupt`, `miss_strike`,
    /// `miss_no_fingerprint`.
    pub source: Verified,
    /// Entry age in milliseconds — present ONLY on a `Fresh`/`Stale` HIT
    /// (oracle `entryAgeMs:ie`). Always absent on a MISS.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry_age_ms: Option<u64>,
}

/// `tengu_mcp_degraded`'s `reason`.
///
/// An earlier revision called the eight tool-schema values below "the eight
/// oracle-confirmed values … the complete `yn` set". **That was wrong.**
/// Scanning 2.1.251 for `tengu_mcp_degraded",{reason:w("…")}` yields TEN
/// literal reasons, and two more arrive through the `reason` parameter of
/// `_n` (@182328873), called with `w("resources_list_failed")` and
/// `w("prompts_list_failed")`:
///
/// | reason | oracle site | ported |
/// |---|---|---|
/// | `connected_zero_tools` | `yn` @182316780, its FIRST statement | ✅ |
/// | `tool_schema_normalized` … `tool_property_key_invalid_gated` (7) | `yn` tail | ✅ |
/// | `schema_validator_unavailable` | `qr()` @182172950 | ✅ |
/// | `tools_list_failed` | @182328200 | ❌ open |
/// | `resources_list_failed` / `prompts_list_failed` | `_n` @182328873 | ❌ open |
///
/// The missed `connected_zero_tools` sat 20 lines ABOVE the seven counters
/// the module doc transcribed verbatim, inside the very function that doc
/// claimed to have traced — which is exactly how a wrong completeness claim
/// hides a gap. The three remaining reasons are list-RPC failure paths this
/// port does not yet classify; they are an open gap, not a closed set.
///
/// `#[non_exhaustive]` matches this module's established convention
/// ([`ConfigInvalidSource`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum DegradedReason {
    /// The server connected and `tools/list` succeeded but returned ZERO
    /// tools, on a LIVE dial (`r === "live"`; the cached-row adoption paths
    /// do not fire it). Oracle `yn`'s first statement:
    /// `if(u.length===0&&r==="live")s("tengu_mcp_degraded",
    /// {reason:w("connected_zero_tools"),transportType:c(e.config.type
    /// ??"stdio"),mcpServerName:P,..._})`.
    ///
    /// Counted off the RAW `tools/list` response, BEFORE the §20a schema
    /// filter runs — a server whose every tool was dropped by that filter
    /// is NOT `connected_zero_tools`, it is one of the drop reasons below.
    /// Carries none of the count fields.
    ConnectedZeroTools,
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
            Self::ConnectedZeroTools => "connected_zero_tools",
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

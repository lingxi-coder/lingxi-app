//! §11 — the MCP **discovery cache**: a persisted, per-server record of the
//! last successful `initialize`/`tools/list`/`prompts/list`/`resources/list`
//! round so a reconnect can skip re-discovery when nothing has changed.
//!
//! This is a **greenfield port** — the oracle's on-disk shape (an encrypted-
//! at-rest-adjacent JSON blob keyed by an OAuth-grant-token-bound fingerprint,
//! written through a pluggable KV storage backend with symlink/oversize
//! refusal) is not wire-visible to any other client, so nothing here needs to
//! byte-match it. What *is* ported, because it IS externally observable
//! (config keys a user can write, log lines, telemetry bucket names), is:
//!
//! * the two new config keys, `discoveryCache` (sse/http only) and `role`
//!   (see [`crate::json_config::discovery_cache_flag`] /
//!   [`crate::json_config::role_flag`] — recognized but NOT yet threaded onto
//!   [`crate::connection::McpServerConfig`]/[`traits::McpTransportSpec`]; see
//!   the module-end note for why);
//! * the miss-reason vocabulary and the fresh/stale/miss decision oracle
//!   `cot` implements (2.1.251 Mach-O @176266197), recovered byte-exact from
//!   the binary:
//!   ```text
//!   async function cot(e,t,r=Date.now()){
//!     let o=me(t);
//!     if(o!==void 0&&!Ie(o))return{kind:"miss",reason:o==="transport"?"transport":"disabled"};
//!     if(o!==void 0){ /* purge this server's cache-key family */ return{kind:"miss",reason:"disabled"} }
//!     let s; try{s=await we(e,t)}catch{return{kind:"miss",reason:"no-fingerprint"}}
//!     if(s===void 0)return{kind:"miss",reason:"no-fingerprint"};
//!     let d; try{d=await Q(s,_())}catch(S){ if(refused-entry) return corrupt; return{kind:"miss",reason:"absent"} }
//!     if(d===void 0)return{kind:"miss",reason:"absent"};
//!     let w=schema.safeParse(...); if(!w.success) return corrupt;
//!     let k=w.data;
//!     if(k.cacheKey!==A(e,t)) return corrupt; // keyed for another server
//!     if(k.consecutiveRefreshFailures>=pe()) return{kind:"miss",reason:"strike-threshold"};
//!     let R=ye(); if(oe(k)-r>R) return{kind:"miss",reason:"expired"};
//!     let u=max(0,r-k.savedAt); if(u>=R) return{kind:"miss",reason:"expired"};
//!     if(u<Oe() && !(k.capabilities.tools&&k.tools.length===0)) return{kind:"fresh",entry:k,ageMs:u};
//!     return{kind:"stale",entry:k,ageMs:u}
//!   }
//!   ```
//!   `me(t)` (@176260900) is the eligibility gate, in this exact order:
//!   the env/kill-switch feature gate (`Sln`, @161529583) FIRST, then
//!   `identity-changed`, then a `type!=="http"&&type!=="sse"` transport gate
//!   (`"transport"`), then `cli-owned`/`env-placeholder`/`ambient-credential`
//!   guards, then a `W`-table walk that yields `"opt-out"` (config
//!   `discoveryCache:false`) or `"headers-helper"` (a configured
//!   `headersHelper`). `Ie(o)` (@176260899: `W.some(t=>t.reason===o)`) is
//!   true ONLY for `"opt-out"`/`"headers-helper"` — exactly the two reasons
//!   that ALSO purge the on-disk entry family before reporting the miss;
//!   every other disable reason (including the env/kill-switch gate) reports
//!   the miss WITHOUT a purge. [`CacheGateReason::purges_existing_entry`]
//!   mirrors that split.
//!
//! Recovered numeric defaults (2.1.251 @176259300, `Pe/Me/Ae/xe`):
//! TTL 900s (`MCP_DISCOVERY_CACHE_TTL_S`), max-stale 14 400s
//! (`MCP_DISCOVERY_CACHE_MAX_STALE_S`), a 604 800s (7-day) hard ceiling on
//! max-stale, and a strike threshold of 1 (`MCP_DISCOVERY_CACHE_STRIKES`) —
//! i.e. by default a SINGLE recorded refresh failure already misses the
//! cache. `MCP_DISCOVERY_CACHE` itself defaults OFF (`Sln`'s `"not-enabled"`
//! arm) — the feature is opt-IN.
//!
//! ## What is DEFERRED (named, not built — see the batch report)
//!
//! * **The real fingerprint.** Oracle `we`/`Be` (@176263300-ish) bind the
//!   cache key to a SHA-256 of `{sdkVersion, grantToken}` where `grantToken`
//!   is derived from the server's OAuth **refresh token** — so a token
//!   rotation invalidates the cache. [`DiscoveryCacheStore::cache_key`] hashes
//!   only `{name, transport kind, url, headers}`; wiring in the OAuth grant
//!   token is real work (`mcp::oauth`) left for the wave that also does the
//!   live registry integration. [`MissReason::NoFingerprint`] is therefore
//!   never constructed today — kept for vocabulary completeness.
//! * **`identity-changed` / `cli-owned` / `env-placeholder` /
//!   `ambient-credential`.** Oracle guards between the feature gate and the
//!   transport gate (`identity-changed`) and between the transport gate and
//!   the opt-out/headers-helper table (the other three) — see `me`'s source
//!   above. All four collapse to [`MissReason::Disabled`] like every other
//!   non-transport reason, so [`cache_gate`] omitting them changes no
//!   OBSERVABLE decision except in the narrow case where one of them alone
//!   would have disabled a server that is otherwise feature-enabled and
//!   sse/http — i.e. today's [`cache_gate`] is a strict SUBSET of disable
//!   reasons, never a superset (never permits caching the oracle would
//!   refuse).
//! * **`skills-capable` / `channel-capable` / `live-connection`.** Round-3
//!   of the batch audit misfiled these as a separate "MCP skills" feature;
//!   round-4 (§24e) corrected that — they are discovery-cache MISS REASONS
//!   `rs`/`Vo` (@182313623/@182512831) apply AFTER a fresh/stale hit, when
//!   the entry's capabilities match a coordinator-mode "skills" or "channel"
//!   predicate this port has no concept of at all (no coordinator/multi-agent
//!   surface exists here — see `mcp::protocol_negotiation`'s module docs,
//!   which deferred the SAME two reasons for the identical reason). Building
//!   a guess at `Gmt`/`o_e`'s predicate would BE the separate feature the
//!   correction warns against — not built. `live-connection` similarly ties
//!   to an in-flight-connection check at the plugin-discovery call site this
//!   port doesn't have; also not built.
//! * **`discoveryCache`/`role` config threading.** `McpServerConfig` still has
//!   no `discovery_cache_opt_out` field for the same reason as before: `apps/
//!   cli/src/commands/mcp.rs` and ~7 sibling files (see git blame on this
//!   paragraph) construct `McpServerConfig`/every `McpTransportSpec` variant
//!   as exhaustive struct literals with no `..`, so adding a field to either
//!   breaks their compile and remains out of scope here. [`cache_gate`]/
//!   [`decide`] therefore keep accepting `discovery_cache_opt_out` as an
//!   explicit `Option<bool>` parameter; every production call site in
//!   `mcp::registry` passes `None` (nothing can ever produce
//!   [`CacheGateReason::OptOut`] in practice today — only direct unit tests
//!   of [`cache_gate`] exercise that arm). [`CacheGateReason::HeadersHelper`]
//!   IS reachable in production (`McpTransportSpec::Http`/`Sse` already carry
//!   `headers_helper`).
//!
//! ## What §11 Stage 1 wires in (this revision)
//!
//! `mcp::registry::McpRegistry` gained an optional
//! `Arc<DiscoveryCacheStore>` (set via `with_discovery_cache_store`; `None`
//! by default, so every existing caller is unaffected). When set:
//!
//! * **Write.** After a successful LIVE discovery round, `connect` persists
//!   the full catalog (tools/resources/resource_templates/prompts +
//!   capabilities) for a cache-ELIGIBLE server ([`cache_gate`] returning
//!   `None`) via [`DiscoveryCacheEntry::new`]/[`DiscoveryCacheStore::store`],
//!   resetting `consecutive_refresh_failures` to 0 (oracle `Wo`/`Mt`,
//!   restricted to the identity/in-flight-swap-free subset this port can
//!   evaluate — see the deferred note above). A gate reason that
//!   [`CacheGateReason::purges_existing_entry`] flags (`HeadersHelper`, and
//!   `OptOut` if it ever becomes reachable) instead purges any existing
//!   on-disk entry, best-effort.
//! * **Strikes.** A connect attempt that ultimately FAILS for a
//!   cache-eligible server with an EXISTING on-disk entry increments that
//!   entry's `consecutive_refresh_failures` (best-effort; a server with no
//!   entry yet records nothing — there is nothing to strike). This is an
//!   approximation of the oracle's background-revalidation strike counter
//!   (`_6e`, only reachable from a `Stale`-hit's revalidation path): this
//!   port has no background revalidation yet (Stage 3, deferred), so the
//!   nearest available signal is treating every connect as an implicit
//!   refresh attempt.
//! * **Telemetry.** [`crate::registry`] calls [`decide`] before every dial
//!   purely for observability and reports [`MissReason`]s that oracle `Ko`
//!   (2.1.251, same chunk as `cot`) surfaces (`absent`/`expired`/`corrupt`/
//!   `strike-threshold`/`no-fingerprint`) as `tengu_mcp_discovery_source`
//!   with `source` = [`miss_telemetry_value`] — see
//!   [`miss_emits_discovery_source_telemetry`] for the exact set and the
//!   recovered `Ko`/`Jo` source. A `Fresh`/`Stale` decision emits NOTHING:
//!   the oracle's hit-branch event describes actually SERVING the cached
//!   catalog without dialing, which this port does not do yet (see Stage 2
//!   below), so emitting `cache_fresh`/`cache_stale` here would misreport an
//!   action that never happened.
//!
//! ## What is still NOT built (Stage 2 / Stage 3)
//!
//! * **Serving from cache (Stage 2).** A `Fresh`/`Stale` decision does not
//!   skip the dial — every connect still goes live and re-discovers the
//!   catalog over the wire, then overwrites the entry it just read. Building
//!   this needs a `type:"cached"` connection state in `mcp::connection` plus
//!   a lazy first-tool-use dial, which is real transport-lifecycle surgery;
//!   see the batch report for what specifically blocks it if it was not
//!   completed this wave.
//! * **Background revalidation on a `Stale` hit (Stage 3).** The oracle
//!   kicks off an async re-dial after serving a stale entry immediately;
//!   without Stage 2 there is no "serve stale, refresh behind it" moment to
//!   hang this off of.

use traits::{
    McpPromptDto, McpResourceDto, McpResourceTemplateDto, McpToolDto, McpTransportSpec,
    ServerCapabilitiesDto,
};

// ── config-independent constants (oracle `Pe`/`Me`/`Ae`/`xe`) ──────────────

/// Schema version tag written into every persisted entry (`v` in the oracle
/// schema `B`, literal `1` there — this port's own numbering, bumped to `2`
/// when the entry grew a full catalog payload; see [`DiscoveryCacheEntry`]).
/// A version this store doesn't recognize is treated as
/// [`EntryLookup::Corrupt`] — never trusted, never an error. A `v1` entry
/// (this module's original metadata-only shape) still PARSES cleanly —
/// every field schema v2 added is `#[serde(default)]`, on purpose, so a
/// future additive change never needs another version bump — but is
/// rejected by the explicit `entry.version != CACHE_SCHEMA_VERSION` check in
/// [`DiscoveryCacheStore::load`], never trusted with a silently-defaulted
/// (empty) catalog.
pub const CACHE_SCHEMA_VERSION: u32 = 2;

/// The feature opt-in env var (oracle `MCP_DISCOVERY_CACHE`, read through a
/// boolean-coerced env schema — `a.MCP_DISCOVERY_CACHE===true`/`===false`).
/// Reused here via [`traits::env::is_env_truthy`]/[`traits::env::is_env_defined_falsy`],
/// this port's established idiom for a coerced-boolean env var.
pub const ENV_ENABLED: &str = "MCP_DISCOVERY_CACHE";

/// Shared serial guard for tests that mutate `ENV_ENABLED`. Env vars are
/// process-global, so the registry's eligibility tests must take the same
/// lock this module's own tests use or they race.
#[cfg(test)]
pub(crate) fn tests_env_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}
/// Fresh-window override, in SECONDS (oracle `MCP_DISCOVERY_CACHE_TTL_S`).
pub const ENV_TTL_SECONDS: &str = "MCP_DISCOVERY_CACHE_TTL_S";
/// Max-stale override, in SECONDS (oracle `MCP_DISCOVERY_CACHE_MAX_STALE_S`).
pub const ENV_MAX_STALE_SECONDS: &str = "MCP_DISCOVERY_CACHE_MAX_STALE_S";
/// Strike-threshold override (oracle `MCP_DISCOVERY_CACHE_STRIKES`).
pub const ENV_STRIKES: &str = "MCP_DISCOVERY_CACHE_STRIKES";

/// Oracle `Pe` — default fresh-window seconds (15 minutes).
const DEFAULT_TTL_SECONDS: u64 = 900;
/// Oracle `Me` — default max-stale seconds (4 hours).
const DEFAULT_MAX_STALE_SECONDS: u64 = 14_400;
/// Oracle `Ae` — hard ceiling on max-stale regardless of the env override (7 days).
const MAX_STALE_CEILING_SECONDS: u64 = 604_800;
/// Oracle `xe` — default strike threshold (a SINGLE recorded failure misses).
const DEFAULT_STRIKES: u32 = 1;

/// Oracle `Sln()`'s enabled check, restricted to the literal env var this
/// port has (no kill-switch/SDK-embedder concept exists here — see module
/// docs). Unset or unrecognized ⇒ `false` (the oracle's default-off
/// `"not-enabled"` arm); an explicit falsy value (`"false"`/`"0"`/…) is ALSO
/// `false` (oracle's `"env-disabled"`) — the two collapse to the same
/// disable reason at [`cache_gate`], matching `cot`'s own collapse.
#[must_use]
pub fn feature_enabled() -> bool {
    traits::env::is_env_truthy(std::env::var(ENV_ENABLED).ok().as_deref())
}

/// Plain, non-negative-integer env parse shared by the three numeric knobs
/// below — mirrors this crate's existing `MCP_TIMEOUT` convention
/// (`registry::mcp_connection_timeout`): trim, parse as `u64`, `0` or
/// non-numeric falls back to `default`.
fn positive_seconds_from_env(var: &str, default: u64) -> u64 {
    std::env::var(var)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(default)
}

/// Oracle `ye()` — max-stale window in milliseconds, clamped to the 7-day
/// ceiling regardless of how large an override is set.
#[must_use]
pub fn max_stale_ms() -> u64 {
    positive_seconds_from_env(ENV_MAX_STALE_SECONDS, DEFAULT_MAX_STALE_SECONDS)
        .min(MAX_STALE_CEILING_SECONDS)
        .saturating_mul(1000)
}

/// Oracle `Oe()` — fresh window in milliseconds: the TTL override, further
/// capped by [`max_stale_ms`] (a fresh window can never outlive the max-stale
/// window).
#[must_use]
pub fn ttl_ms() -> u64 {
    positive_seconds_from_env(ENV_TTL_SECONDS, DEFAULT_TTL_SECONDS)
        .saturating_mul(1000)
        .min(max_stale_ms())
}

/// Oracle `pe()` — the `consecutiveRefreshFailures` strike threshold.
#[must_use]
pub fn strike_threshold() -> u32 {
    let raw = std::env::var(ENV_STRIKES).ok();
    let Some(raw) = raw else {
        return DEFAULT_STRIKES;
    };
    raw.trim()
        .parse::<u32>()
        .ok()
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT_STRIKES)
}

// ── eligibility gate (oracle `me`/`Ie`) ─────────────────────────────────────

/// Why [`cache_gate`] refused to consult the cache at all — oracle `me`'s
/// disable reasons, restricted to the subset this port can evaluate today
/// (see the module-level DEFERRED note for the rest).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheGateReason {
    /// `type!=="http"&&type!=="sse"` — only these two transports are ever
    /// cache-eligible.
    Transport,
    /// [`feature_enabled`] is `false` (unset, or an explicit falsy value).
    FeatureDisabled,
    /// The config declared `discoveryCache:false` (oracle `W`'s `"opt-out"`
    /// entry).
    OptOut,
    /// The config declared a `headersHelper` (oracle `W`'s `"headers-helper"`
    /// entry) — an executable-derived header can't be safely assumed stable
    /// across a cached round.
    HeadersHelper,
}

impl CacheGateReason {
    /// Oracle `o==="transport"?"transport":"disabled"` — every reason BUT
    /// `Transport` collapses to the single generic [`MissReason::Disabled`].
    #[must_use]
    pub fn miss_reason(self) -> MissReason {
        match self {
            Self::Transport => MissReason::Transport,
            Self::FeatureDisabled | Self::OptOut | Self::HeadersHelper => MissReason::Disabled,
        }
    }

    /// Oracle `Ie(o)` — does this reason ALSO purge any existing on-disk
    /// entry for the server before reporting the miss? True only for the two
    /// reasons a user can flip at runtime (`opt-out`, `headers-helper`); a
    /// stale entry left behind by either would otherwise resurface the
    /// moment the config reverts.
    #[must_use]
    pub fn purges_existing_entry(self) -> bool {
        matches!(self, Self::OptOut | Self::HeadersHelper)
    }
}

/// Oracle `me(t)`, restricted to the reasons this port can evaluate (see
/// module docs). Checked in the SAME order as the oracle — the feature gate
/// fires before the transport check, so a disabled feature on a non-sse/http
/// spec reports `FeatureDisabled`, not `Transport`, exactly like `cot`.
/// Returns `None` when the server is cache-eligible.
#[must_use]
pub fn cache_gate(
    spec: &McpTransportSpec,
    discovery_cache_opt_out: Option<bool>,
    feature_enabled: bool,
) -> Option<CacheGateReason> {
    if !feature_enabled {
        return Some(CacheGateReason::FeatureDisabled);
    }
    let headers_helper = match spec {
        McpTransportSpec::Sse { headers_helper, .. }
        | McpTransportSpec::Http { headers_helper, .. } => headers_helper,
        _ => return Some(CacheGateReason::Transport),
    };
    if discovery_cache_opt_out == Some(false) {
        return Some(CacheGateReason::OptOut);
    }
    if headers_helper.is_some() {
        return Some(CacheGateReason::HeadersHelper);
    }
    None
}

// ── miss-reason vocabulary (oracle `rs`/`Vo`/`as`) ──────────────────────────

/// The full oracle miss-reason vocabulary. Variants marked DEFERRED are
/// never constructed by [`decide`] today — kept so the type is complete and
/// a future wave can wire them in without a breaking rename.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissReason {
    /// Non-cache-eligible transport.
    Transport,
    /// Any disable reason but `Transport` — see [`CacheGateReason::miss_reason`].
    Disabled,
    /// No on-disk entry for this server.
    Absent,
    /// The on-disk entry failed to parse, had the wrong schema version, was
    /// a symlink/non-regular/oversize file, or was keyed for a different
    /// server (a cache-key mismatch) — oracle collapses all of these into
    /// its own `"corrupt"` path (`h()`/`p("mcp_discovery_cache","corrupt_entry")`).
    Corrupt,
    /// The entry aged past the max-stale window (or carries an
    /// implausible future timestamp — a clock-skew guard).
    Expired,
    /// `consecutiveRefreshFailures` reached [`strike_threshold`].
    Strike,
    /// DEFERRED — ties to OAuth-grant-token fingerprinting; never
    /// constructed today (see module docs).
    NoFingerprint,
    /// DEFERRED — an in-flight live connection already exists for this
    /// server; never constructed today.
    LiveConnection,
    /// DEFERRED — the cached entry declares a "skills" capability the
    /// coordinator-mode predicate this port lacks would reject; never
    /// constructed today (§24e).
    SkillsCapable,
    /// DEFERRED — the cached entry declares a "channel" capability; never
    /// constructed today (§24e).
    ChannelCapable,
}

/// Oracle `as(e)` — the telemetry bucket name for a decision. Curiously,
/// `Transport`/`Absent`/`LiveConnection`/`SkillsCapable`/`ChannelCapable` all
/// map to the literal `"live"` (the oracle only tracks cache HEALTH —
/// disabled/expired/corrupt/strike/no-fingerprint — as `miss_*`; the rest
/// are "we did a live fetch, nothing to measure"), verified byte-exact at
/// 2.1.251 Mach-O @182315844.
#[must_use]
pub fn miss_telemetry_value(reason: MissReason) -> &'static str {
    match reason {
        MissReason::Disabled => "miss_disabled",
        MissReason::Expired => "miss_expired",
        MissReason::Corrupt => "miss_corrupt",
        MissReason::Strike => "miss_strike",
        MissReason::NoFingerprint => "miss_no_fingerprint",
        MissReason::Transport
        | MissReason::Absent
        | MissReason::LiveConnection
        | MissReason::SkillsCapable
        | MissReason::ChannelCapable => "live",
    }
}

/// Oracle `Ko(e)` (2.1.251, the SAME chunk as `cot`/`Jo`/`Vo` — resolved from
/// the region around @182512938, NOT the unrelated `Ko`/`Jo` functions that
/// share these minified names elsewhere in the binary; see this module's
/// header note on chunk-local names). Recovered source:
/// ```text
/// function Ko(e){switch(e.reason){
///   case"absent":case"expired":case"corrupt":
///   case"strike-threshold":case"no-fingerprint":return!0;
///   case"disabled":case"transport":case"live-connection":
///   case"skills-capable":case"channel-capable":return!1;
///   default:return e.reason}}
/// ```
/// This gates whether `tengu_mcp_discovery_source` fires at ALL on a MISS
/// decision — `true` for the five reasons meaning "a disk read was actually
/// attempted and came up unusable", `false` for the three gate-level reasons
/// meaning the cache was never consulted (`disabled`/`transport`/
/// `live-connection`) plus the two capability-miss reasons this port never
/// constructs. The fresh/stale HIT branch emits unconditionally — this gate
/// applies only to [`Decision::Miss`] (see the module doc's "What §11 Stage
/// 1 wires in" section for why a hit emits nothing in THIS port today).
#[must_use]
pub fn miss_emits_discovery_source_telemetry(reason: MissReason) -> bool {
    matches!(
        reason,
        MissReason::Absent
            | MissReason::Expired
            | MissReason::Corrupt
            | MissReason::Strike
            | MissReason::NoFingerprint
    )
}

/// Wall-clock "now", ms since the Unix epoch — the real-clock reading
/// production callers hand to [`DiscoveryCachePolicy::from_env`]/
/// [`DiscoveryCacheEntry::new`]. Tests use their own literal values instead
/// (both take `now_ms`/`saved_at_ms` as plain parameters precisely so they
/// don't need this).
#[must_use]
pub fn now_ms() -> u64 {
    u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
}

// ── the persisted entry + decision (oracle `B`/`cot`) ───────────────────────

/// `serverInfo` from the server's `initialize` response (oracle schema `B`'s
/// `serverInfo` sub-object — spread onto the served "cached" client only
/// when present: `...v.serverInfo && {serverInfo:{name:...,version:...}}`).
///
/// This port's [`traits::McpTransport::initialize`] returns only
/// [`ServerCapabilitiesDto`] — the wire `serverInfo` block is discarded
/// before it reaches `mcp::registry`, so nothing populates this field today.
/// Kept as a real (rather than omitted) field so schema v2 is
/// forward-compatible with whichever future change threads `serverInfo`
/// through the transport trait.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DiscoveryCacheServerInfo {
    /// Server name as reported at `initialize`.
    pub name: String,
    /// Server version as reported at `initialize`.
    pub version: String,
}

/// One persisted discovery-cache entry.
///
/// Schema v2: carries the FULL catalog a cache hit would need to serve a
/// server without dialing (tools/resources/resource_templates/prompts +
/// capabilities), not just the metadata v1 needed to make the fresh/stale/
/// miss decision. Still narrower than the oracle's `B` schema
/// (`negotiatedEra` is not modelled — no protocol-era negotiation reaches
/// this deep in the port; see `mcp::protocol_negotiation`'s module docs).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DiscoveryCacheEntry {
    /// Schema version — see [`CACHE_SCHEMA_VERSION`].
    #[serde(rename = "v")]
    pub version: u32,
    /// Expected value: [`DiscoveryCacheStore::cache_key`] for the server this
    /// entry belongs to. A mismatch (entry read from the right file path but
    /// keyed for a different server/config) is treated as [`MissReason::Corrupt`].
    pub cache_key: String,
    /// When this entry was saved, ms since the Unix epoch.
    pub saved_at_ms: u64,
    /// A separate, possibly-later save timestamp for a partial (tools-only)
    /// refresh — oracle `toolsSavedAt`. `None` when never separately updated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools_saved_at_ms: Option<u64>,
    /// Consecutive refresh failures recorded against this entry.
    #[serde(default)]
    pub consecutive_refresh_failures: u32,
    /// Server capability flags returned by `initialize`. [`decide`]'s
    /// degenerate check reads `capabilities.tools` directly (oracle
    /// `k.capabilities.tools`).
    #[serde(default)]
    pub capabilities: ServerCapabilitiesDto,
    /// `serverInfo`, when this port has one to store — see
    /// [`DiscoveryCacheServerInfo`]'s doc (always `None` today).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_info: Option<DiscoveryCacheServerInfo>,
    /// The cached `tools/list` result. [`decide`]'s degenerate check reads
    /// `tools.is_empty()` directly (oracle `k.tools.length===0`).
    #[serde(default)]
    pub tools: Vec<McpToolDto>,
    /// The cached `resources/list` result.
    #[serde(default)]
    pub resources: Vec<McpResourceDto>,
    /// The cached `resources/templates/list` result.
    #[serde(default)]
    pub resource_templates: Vec<McpResourceTemplateDto>,
    /// The cached `prompts/list` result.
    #[serde(default)]
    pub prompts: Vec<McpPromptDto>,
}

impl DiscoveryCacheEntry {
    /// Build a fresh entry for `cache_key`, saved "now" (`saved_at_ms`),
    /// with `consecutive_refresh_failures` reset to 0 and no `server_info`
    /// (see [`Self::with_server_info`] to attach one when a future change
    /// makes that possible).
    #[must_use]
    pub fn new(
        cache_key: String,
        saved_at_ms: u64,
        capabilities: ServerCapabilitiesDto,
        tools: Vec<McpToolDto>,
        resources: Vec<McpResourceDto>,
        resource_templates: Vec<McpResourceTemplateDto>,
        prompts: Vec<McpPromptDto>,
    ) -> Self {
        Self {
            version: CACHE_SCHEMA_VERSION,
            cache_key,
            saved_at_ms,
            tools_saved_at_ms: None,
            consecutive_refresh_failures: 0,
            capabilities,
            server_info: None,
            tools,
            resources,
            resource_templates,
            prompts,
        }
    }

    /// Attach a `serverInfo` — chainable builder, kept separate from
    /// [`Self::new`] so the common (server-info-less) construction doesn't
    /// have to thread an extra `None` through every call site.
    #[must_use]
    pub fn with_server_info(mut self, server_info: DiscoveryCacheServerInfo) -> Self {
        self.server_info = Some(server_info);
        self
    }
}

/// The outcome of a store read, already folded through the fail-safe
/// corruption checks ([`DiscoveryCacheStore::load`]) — the input [`decide`]
/// consumes.
#[derive(Debug, Clone, PartialEq)]
pub enum EntryLookup {
    /// No on-disk entry.
    Absent,
    /// An on-disk entry existed but was unusable (see [`MissReason::Corrupt`]'s doc).
    Corrupt,
    /// A structurally valid, correctly-keyed entry.
    Found(DiscoveryCacheEntry),
}

/// The three-way outcome oracle `cot` returns (`{kind:"fresh"|"stale"|"miss",...}`).
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    /// Cache hit, young enough to skip re-discovery outright.
    Fresh {
        /// The hit entry.
        entry: DiscoveryCacheEntry,
        /// Age of the entry at decision time, ms.
        age_ms: u64,
    },
    /// Cache hit, but old enough that a caller should refresh in the
    /// background while still serving the stale data immediately
    /// (stale-while-revalidate) — this port does not implement the
    /// revalidate half; a stale hit is a signal, not a mandate.
    Stale {
        /// The hit entry.
        entry: DiscoveryCacheEntry,
        /// Age of the entry at decision time, ms.
        age_ms: u64,
    },
    /// No usable cache; the caller must do a live discovery round.
    Miss {
        /// Why.
        reason: MissReason,
    },
}

/// The numeric knobs [`decide`] needs, bundled into one value so the
/// function itself stays under clippy's `too_many_arguments` threshold.
/// Construct directly from the env-backed defaults with [`Self::from_env`],
/// or with explicit values (tests, or a future caller with its own clock).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiscoveryCachePolicy {
    /// "Now", ms since the Unix epoch — threaded explicitly rather than read
    /// internally so [`decide`] stays a pure function of its inputs.
    pub now_ms: u64,
    /// Oracle `Oe()` — see [`ttl_ms`].
    pub ttl_ms: u64,
    /// Oracle `ye()` — see [`max_stale_ms`].
    pub max_stale_ms: u64,
    /// Oracle `pe()` — see [`strike_threshold`].
    pub strike_threshold: u32,
}

impl DiscoveryCachePolicy {
    /// Build from the env-backed defaults/overrides ([`ttl_ms`],
    /// [`max_stale_ms`], [`strike_threshold`]) plus a caller-supplied clock
    /// reading.
    #[must_use]
    pub fn from_env(now_ms: u64) -> Self {
        Self {
            now_ms,
            ttl_ms: ttl_ms(),
            max_stale_ms: max_stale_ms(),
            strike_threshold: strike_threshold(),
        }
    }
}

/// Oracle `cot` (2.1.251 Mach-O @176266197) — the pure decision, given
/// everything a store read + the config already resolved. See module docs
/// for the full recovered source and the DEFERRED reasons this omits.
#[must_use]
pub fn decide(
    spec: &McpTransportSpec,
    discovery_cache_opt_out: Option<bool>,
    feature_enabled: bool,
    lookup: EntryLookup,
    policy: DiscoveryCachePolicy,
) -> Decision {
    if let Some(reason) = cache_gate(spec, discovery_cache_opt_out, feature_enabled) {
        return Decision::Miss {
            reason: reason.miss_reason(),
        };
    }
    let entry = match lookup {
        EntryLookup::Absent => {
            return Decision::Miss {
                reason: MissReason::Absent,
            }
        }
        EntryLookup::Corrupt => {
            return Decision::Miss {
                reason: MissReason::Corrupt,
            }
        }
        EntryLookup::Found(entry) => entry,
    };
    if entry.consecutive_refresh_failures >= policy.strike_threshold {
        return Decision::Miss {
            reason: MissReason::Strike,
        };
    }
    // Oracle `oe(k)=max(k.savedAt,k.toolsSavedAt??0)`; `oe(k)-r>R` is a
    // clock-skew guard against a saved timestamp implausibly ahead of "now".
    let anchor = entry.saved_at_ms.max(entry.tools_saved_at_ms.unwrap_or(0));
    if anchor.saturating_sub(policy.now_ms) > policy.max_stale_ms {
        return Decision::Miss {
            reason: MissReason::Expired,
        };
    }
    let age_ms = policy.now_ms.saturating_sub(entry.saved_at_ms);
    if age_ms >= policy.max_stale_ms {
        return Decision::Miss {
            reason: MissReason::Expired,
        };
    }
    // `k.capabilities.tools&&k.tools.length===0` — a server that claims the
    // tools capability but returned none is treated as degenerate and never
    // served fresh, even within the TTL window (ties to the oracle's
    // `connected_zero_tools` degraded-connection telemetry). Schema v2 stores
    // the real fields, so this reads them directly — byte-exact with the
    // oracle expression, not a flattened approximation.
    let degenerate = entry.capabilities.tools && entry.tools.is_empty();
    if age_ms < policy.ttl_ms && !degenerate {
        Decision::Fresh { entry, age_ms }
    } else {
        Decision::Stale { entry, age_ms }
    }
}

// ── persisted store ──────────────────────────────────────────────────────

/// Oversize guard. LingXi-original: the oracle's is 8 MiB, tuned for a much
/// richer payload (full tool/resource/prompt bodies) this format doesn't
/// carry; 1 MiB is generous for the slim [`DiscoveryCacheEntry`] shape above
/// and is not meant to byte-match anything.
const MAX_ENTRY_BYTES: u64 = 1 << 20;

/// A directory of one-file-per-server discovery-cache entries. The root is
/// caller-supplied (see the module-level DEFERRED note on wiring the real
/// production directory) so this type stays trivially testable against a
/// tempdir.
#[derive(Debug, Clone)]
pub struct DiscoveryCacheStore {
    root: std::path::PathBuf,
}

impl DiscoveryCacheStore {
    /// Open a store rooted at `root`. Does not touch the filesystem — the
    /// directory is created lazily on first [`Self::store`].
    #[must_use]
    pub fn new(root: impl Into<std::path::PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Oracle `A(e,t)=ur(e,ge(t))` restricted to what this port has: a
    /// SHA-256 over `{name, transport kind, url, headers}` (NOT the oracle's
    /// OAuth-grant-token-bound fingerprint — see the module-level DEFERRED
    /// note). Non-remote specs (no `url`/`headers`) still get a stable key
    /// from `{name, kind}` alone, so [`Self::load`]/[`Self::store`] never
    /// panic on them even though [`cache_gate`] already refuses anything but
    /// sse/http.
    #[must_use]
    pub fn cache_key(name: &str, spec: &McpTransportSpec) -> String {
        use sha2::{Digest, Sha256};
        let (kind, url, headers_json) = match spec {
            McpTransportSpec::Sse { url, headers, .. }
            | McpTransportSpec::Http { url, headers, .. } => (
                spec.kind(),
                url.as_str(),
                serde_json::to_string(headers).unwrap_or_default(),
            ),
            other => (other.kind(), "", String::new()),
        };
        let material = format!("{name}\0{kind}\0{url}\0{headers_json}");
        let mut hasher = Sha256::new();
        hasher.update(material.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    fn entry_path(&self, key: &str) -> std::path::PathBuf {
        self.root.join(format!("{key}.json"))
    }

    /// Fail-safe read. A missing file is [`EntryLookup::Absent`]; a symlink,
    /// a non-regular file, an oversize file, invalid JSON, a schema-version
    /// mismatch, or a stored `cache_key` that doesn't match `expected_key`
    /// are ALL [`EntryLookup::Corrupt`] — this store never surfaces a raw
    /// I/O error and never trusts a garbled/mismatched entry, matching the
    /// oracle's fail-safe posture (`miss_corrupt` exists for exactly this).
    #[must_use]
    pub fn load(&self, expected_key: &str) -> EntryLookup {
        let path = self.entry_path(expected_key);
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return EntryLookup::Absent,
            Err(_) => return EntryLookup::Corrupt,
        };
        if meta.file_type().is_symlink() || !meta.is_file() {
            return EntryLookup::Corrupt;
        }
        if meta.len() > MAX_ENTRY_BYTES {
            return EntryLookup::Corrupt;
        }
        let Ok(raw) = std::fs::read_to_string(&path) else {
            return EntryLookup::Corrupt;
        };
        let Ok(entry) = serde_json::from_str::<DiscoveryCacheEntry>(&raw) else {
            return EntryLookup::Corrupt;
        };
        if entry.version != CACHE_SCHEMA_VERSION || entry.cache_key != expected_key {
            return EntryLookup::Corrupt;
        }
        EntryLookup::Found(entry)
    }

    /// Atomic write: serialize to a sibling temp file (named with the
    /// current PID so two concurrent writers for the SAME key never collide
    /// on the temp path), then rename into place. A crash mid-write leaves
    /// only an orphaned `.tmp-*` file behind — the real path is untouched
    /// until the rename commits.
    ///
    /// # Errors
    /// Any I/O failure creating the directory, writing the temp file, or
    /// renaming it into place.
    pub fn store(&self, entry: &DiscoveryCacheEntry) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.root)?;
        let path = self.entry_path(&entry.cache_key);
        let tmp_path = self
            .root
            .join(format!("{}.tmp-{}", entry.cache_key, std::process::id()));
        let bytes = serde_json::to_vec(entry)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(&tmp_path, bytes)?;
        std::fs::rename(&tmp_path, &path)?;
        Ok(())
    }

    /// Delete the entry for `key`. A missing file is not an error (mirrors
    /// the oracle's best-effort purge on `opt-out`/`headers-helper` —
    /// [`CacheGateReason::purges_existing_entry`]).
    ///
    /// # Errors
    /// Any I/O failure other than the file already being absent.
    pub fn purge(&self, key: &str) -> std::io::Result<()> {
        match std::fs::remove_file(self.entry_path(key)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Env-mutating tests share process-global state; cargo runs a crate's
    /// unit tests on multiple threads by default. Every test that touches
    /// `ENV_ENABLED`/`ENV_TTL_SECONDS`/`ENV_MAX_STALE_SECONDS`/`ENV_STRIKES`
    /// holds this lock for its whole body (same pattern as
    /// `protocol_negotiation`'s `flag_test_lock`).
    fn env_test_lock() -> &'static std::sync::Mutex<()> {
        static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
    }

    fn clear_env() {
        for var in [
            ENV_ENABLED,
            ENV_TTL_SECONDS,
            ENV_MAX_STALE_SECONDS,
            ENV_STRIKES,
        ] {
            std::env::remove_var(var);
        }
    }

    fn http_spec(url: &str, headers_helper: Option<&str>) -> McpTransportSpec {
        McpTransportSpec::Http {
            url: url.to_string(),
            headers: traits::McpHeaders::new(),
            headers_helper: headers_helper.map(str::to_string),
            oauth: None,
        }
    }

    fn stdio_spec() -> McpTransportSpec {
        McpTransportSpec::Stdio {
            command: "srv".to_string(),
            args: vec![],
            env: std::collections::HashMap::new(),
        }
    }

    // ── cache_gate ──────────────────────────────────────────────────────

    #[test]
    fn cache_gate_non_remote_transport_is_transport_reason() {
        assert_eq!(
            cache_gate(&stdio_spec(), None, true),
            Some(CacheGateReason::Transport)
        );
    }

    /// Oracle order: the feature gate is checked BEFORE the transport check
    /// (`me`'s `Sln()` call precedes its `type!=="http"&&type!=="sse"`
    /// check) — so a disabled feature on a non-sse/http spec reports
    /// `FeatureDisabled`, never `Transport`.
    #[test]
    fn cache_gate_feature_disabled_outranks_transport_mismatch() {
        assert_eq!(
            cache_gate(&stdio_spec(), None, false),
            Some(CacheGateReason::FeatureDisabled)
        );
    }

    #[test]
    fn cache_gate_opt_out_reason() {
        assert_eq!(
            cache_gate(&http_spec("https://x.example", None), Some(false), true),
            Some(CacheGateReason::OptOut)
        );
        // `Some(true)` and `None` are NOT opt-out.
        assert_eq!(
            cache_gate(&http_spec("https://x.example", None), Some(true), true),
            None
        );
    }

    #[test]
    fn cache_gate_headers_helper_reason() {
        assert_eq!(
            cache_gate(
                &http_spec("https://x.example", Some("./helper")),
                None,
                true
            ),
            Some(CacheGateReason::HeadersHelper)
        );
    }

    #[test]
    fn cache_gate_eligible_when_no_disable_reason_applies() {
        assert_eq!(
            cache_gate(&http_spec("https://x.example", None), None, true),
            None
        );
    }

    #[test]
    fn opt_out_and_headers_helper_purge_but_transport_and_feature_disabled_do_not() {
        assert!(CacheGateReason::OptOut.purges_existing_entry());
        assert!(CacheGateReason::HeadersHelper.purges_existing_entry());
        assert!(!CacheGateReason::Transport.purges_existing_entry());
        assert!(!CacheGateReason::FeatureDisabled.purges_existing_entry());
    }

    #[test]
    fn every_disable_reason_but_transport_collapses_to_disabled() {
        assert_eq!(
            CacheGateReason::Transport.miss_reason(),
            MissReason::Transport
        );
        assert_eq!(
            CacheGateReason::FeatureDisabled.miss_reason(),
            MissReason::Disabled
        );
        assert_eq!(CacheGateReason::OptOut.miss_reason(), MissReason::Disabled);
        assert_eq!(
            CacheGateReason::HeadersHelper.miss_reason(),
            MissReason::Disabled
        );
    }

    // ── miss_telemetry_value (oracle `as`) ────────────────────────────────

    #[test]
    fn telemetry_bucket_names_match_the_oracle_as_function() {
        assert_eq!(miss_telemetry_value(MissReason::Disabled), "miss_disabled");
        assert_eq!(miss_telemetry_value(MissReason::Expired), "miss_expired");
        assert_eq!(miss_telemetry_value(MissReason::Corrupt), "miss_corrupt");
        assert_eq!(miss_telemetry_value(MissReason::Strike), "miss_strike");
        assert_eq!(
            miss_telemetry_value(MissReason::NoFingerprint),
            "miss_no_fingerprint"
        );
        for live in [
            MissReason::Transport,
            MissReason::Absent,
            MissReason::LiveConnection,
            MissReason::SkillsCapable,
            MissReason::ChannelCapable,
        ] {
            assert_eq!(miss_telemetry_value(live), "live");
        }
    }

    /// Oracle `Ko`'s exact true/false split, transcribed above
    /// [`miss_emits_discovery_source_telemetry`]'s doc. Note `Absent` is
    /// `Ko`-true (the event DOES fire, with `source:"live"` via
    /// [`miss_telemetry_value`]) even though it maps to the same `"live"`
    /// string `Transport` does, which is `Ko`-false — the two are easy to
    /// conflate since `miss_telemetry_value` alone can't tell them apart;
    /// this gate is a SEPARATE decision on the reason, not on the string.
    #[test]
    fn miss_emits_discovery_source_telemetry_matches_the_oracle_ko_split() {
        for should_emit in [
            MissReason::Absent,
            MissReason::Expired,
            MissReason::Corrupt,
            MissReason::Strike,
            MissReason::NoFingerprint,
        ] {
            assert!(
                miss_emits_discovery_source_telemetry(should_emit),
                "{should_emit:?} must emit"
            );
        }
        for should_not_emit in [
            MissReason::Disabled,
            MissReason::Transport,
            MissReason::LiveConnection,
            MissReason::SkillsCapable,
            MissReason::ChannelCapable,
        ] {
            assert!(
                !miss_emits_discovery_source_telemetry(should_not_emit),
                "{should_not_emit:?} must NOT emit"
            );
        }
    }

    // ── decide ────────────────────────────────────────────────────────────

    /// `ServerCapabilitiesDto` with only `tools` set, the shape every
    /// existing `decide` test needs.
    fn caps_tools(tools: bool) -> ServerCapabilitiesDto {
        ServerCapabilitiesDto {
            tools,
            resources: false,
            prompts: false,
            logging: false,
            experimental: std::collections::HashMap::new(),
        }
    }

    fn sample_tools(n: usize) -> Vec<McpToolDto> {
        (0..n)
            .map(|i| McpToolDto {
                server_name: "srv".into(),
                tool_name: format!("t{i}"),
                description: String::new(),
                input_schema: serde_json::json!({}),
                full_name: format!("mcp__srv__t{i}"),
                search_hint: None,
                always_load: None,
                requires_user_interaction: false,
            })
            .collect()
    }

    /// An entry with `capabilities.tools=true` and 3 tools — non-degenerate,
    /// matching every pre-schema-v2 test's implicit assumption.
    fn entry_at(saved_at_ms: u64) -> DiscoveryCacheEntry {
        DiscoveryCacheEntry::new(
            "k".into(),
            saved_at_ms,
            caps_tools(true),
            sample_tools(3),
            vec![],
            vec![],
            vec![],
        )
    }

    fn policy(
        now_ms: u64,
        ttl_ms: u64,
        max_stale_ms: u64,
        strike_threshold: u32,
    ) -> DiscoveryCachePolicy {
        DiscoveryCachePolicy {
            now_ms,
            ttl_ms,
            max_stale_ms,
            strike_threshold,
        }
    }

    #[test]
    fn decide_gate_reason_wins_before_any_entry_is_consulted() {
        let spec = http_spec("https://x.example", None);
        let d = decide(
            &spec,
            Some(false), // opt-out
            true,
            EntryLookup::Found(entry_at(1_000)),
            policy(1_000, 900_000, 14_400_000, 1),
        );
        assert_eq!(
            d,
            Decision::Miss {
                reason: MissReason::Disabled
            }
        );
    }

    #[test]
    fn decide_absent() {
        let spec = http_spec("https://x.example", None);
        let d = decide(
            &spec,
            None,
            true,
            EntryLookup::Absent,
            policy(1_000, 900_000, 14_400_000, 1),
        );
        assert_eq!(
            d,
            Decision::Miss {
                reason: MissReason::Absent
            }
        );
    }

    #[test]
    fn decide_corrupt() {
        let spec = http_spec("https://x.example", None);
        let d = decide(
            &spec,
            None,
            true,
            EntryLookup::Corrupt,
            policy(1_000, 900_000, 14_400_000, 1),
        );
        assert_eq!(
            d,
            Decision::Miss {
                reason: MissReason::Corrupt
            }
        );
    }

    #[test]
    fn decide_strike_threshold_reached() {
        let spec = http_spec("https://x.example", None);
        let mut entry = entry_at(1_000);
        entry.consecutive_refresh_failures = 1;
        let d = decide(
            &spec,
            None,
            true,
            EntryLookup::Found(entry),
            policy(1_000, 900_000, 14_400_000, 1), // threshold
        );
        assert_eq!(
            d,
            Decision::Miss {
                reason: MissReason::Strike
            }
        );
    }

    #[test]
    fn decide_below_strike_threshold_is_not_a_strike_miss() {
        let spec = http_spec("https://x.example", None);
        let mut entry = entry_at(1_000);
        entry.consecutive_refresh_failures = 1;
        let d = decide(
            &spec,
            None,
            true,
            EntryLookup::Found(entry),
            policy(1_000, 900_000, 14_400_000, 2), // threshold not yet reached
        );
        assert!(matches!(d, Decision::Fresh { .. }));
    }

    #[test]
    fn decide_expired_by_age() {
        let spec = http_spec("https://x.example", None);
        let entry = entry_at(0);
        let d = decide(
            &spec,
            None,
            true,
            EntryLookup::Found(entry),
            policy(15_000, 5_000, 10_000, 1), // age 15s >= 10s max-stale
        );
        assert_eq!(
            d,
            Decision::Miss {
                reason: MissReason::Expired
            }
        );
    }

    #[test]
    fn decide_expired_by_future_clock_skew() {
        let spec = http_spec("https://x.example", None);
        // saved_at is 20s AHEAD of "now" — implausible, treated as expired.
        let entry = entry_at(20_000);
        let d = decide(
            &spec,
            None,
            true,
            EntryLookup::Found(entry),
            policy(1_000, 5_000, 10_000, 1),
        );
        assert_eq!(
            d,
            Decision::Miss {
                reason: MissReason::Expired
            }
        );
    }

    #[test]
    fn decide_fresh_within_ttl() {
        let spec = http_spec("https://x.example", None);
        let entry = entry_at(1_000);
        let d = decide(
            &spec,
            None,
            true,
            EntryLookup::Found(entry.clone()),
            policy(2_000, 5_000, 10_000, 1),
        );
        assert_eq!(
            d,
            Decision::Fresh {
                entry,
                age_ms: 1_000
            }
        );
    }

    #[test]
    fn decide_stale_when_past_ttl_but_within_max_stale() {
        let spec = http_spec("https://x.example", None);
        let entry = entry_at(0);
        let d = decide(
            &spec,
            None,
            true,
            EntryLookup::Found(entry.clone()),
            policy(6_000, 5_000, 10_000, 1),
        );
        assert_eq!(
            d,
            Decision::Stale {
                entry,
                age_ms: 6_000
            }
        );
    }

    #[test]
    fn decide_degenerate_zero_tools_forces_stale_even_within_ttl() {
        let spec = http_spec("https://x.example", None);
        let entry = DiscoveryCacheEntry::new(
            "k".into(),
            1_000,
            caps_tools(true),
            vec![],
            vec![],
            vec![],
            vec![],
        );
        let d = decide(
            &spec,
            None,
            true,
            EntryLookup::Found(entry.clone()),
            policy(1_500, 5_000, 10_000, 1),
        );
        assert_eq!(d, Decision::Stale { entry, age_ms: 500 });
    }

    #[test]
    fn decide_zero_tools_without_the_tools_capability_is_not_degenerate() {
        let spec = http_spec("https://x.example", None);
        // capabilities.tools=false, tools=[]: the server never CLAIMED
        // tools, so an empty list is expected, not degenerate.
        let entry = DiscoveryCacheEntry::new(
            "k".into(),
            1_000,
            caps_tools(false),
            vec![],
            vec![],
            vec![],
            vec![],
        );
        let d = decide(
            &spec,
            None,
            true,
            EntryLookup::Found(entry.clone()),
            policy(1_500, 5_000, 10_000, 1),
        );
        assert_eq!(d, Decision::Fresh { entry, age_ms: 500 });
    }

    // ── env parsing ───────────────────────────────────────────────────────

    #[test]
    fn feature_enabled_matrix() {
        let _guard = env_test_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        clear_env();
        assert!(!feature_enabled(), "unset defaults off");
        std::env::set_var(ENV_ENABLED, "true");
        assert!(feature_enabled());
        std::env::set_var(ENV_ENABLED, "false");
        assert!(!feature_enabled());
        std::env::set_var(ENV_ENABLED, "nonsense");
        assert!(!feature_enabled(), "unrecognized value defaults off");
        clear_env();
    }

    #[test]
    fn ttl_and_max_stale_defaults() {
        let _guard = env_test_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        clear_env();
        assert_eq!(max_stale_ms(), 14_400_000);
        assert_eq!(ttl_ms(), 900_000);
        assert_eq!(strike_threshold(), 1);
        clear_env();
    }

    #[test]
    fn max_stale_env_override_is_clamped_to_the_seven_day_ceiling() {
        let _guard = env_test_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        clear_env();
        std::env::set_var(ENV_MAX_STALE_SECONDS, "99999999");
        assert_eq!(max_stale_ms(), 604_800_000);
        clear_env();
    }

    #[test]
    fn ttl_env_override_is_capped_by_max_stale() {
        let _guard = env_test_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        clear_env();
        std::env::set_var(ENV_TTL_SECONDS, "999999");
        std::env::set_var(ENV_MAX_STALE_SECONDS, "100");
        assert_eq!(max_stale_ms(), 100_000);
        assert_eq!(ttl_ms(), 100_000, "ttl can never exceed max-stale");
        clear_env();
    }

    #[test]
    fn strikes_env_override_and_non_positive_fallback() {
        let _guard = env_test_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        clear_env();
        std::env::set_var(ENV_STRIKES, "3");
        assert_eq!(strike_threshold(), 3);
        std::env::set_var(ENV_STRIKES, "0");
        assert_eq!(strike_threshold(), 1, "non-positive falls back to default");
        clear_env();
    }

    // ── store ─────────────────────────────────────────────────────────────

    #[test]
    fn store_roundtrips_an_entry() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = DiscoveryCacheStore::new(dir.path());
        let entry = DiscoveryCacheEntry::new(
            "abc123".into(),
            42,
            caps_tools(true),
            sample_tools(5),
            vec![traits::McpResourceDto {
                uri: "file:///a".into(),
                name: "a".into(),
                mime_type: None,
            }],
            vec![traits::McpResourceTemplateDto {
                uri_template: "file:///{path}".into(),
                name: "tmpl".into(),
                description: None,
                mime_type: None,
            }],
            vec![traits::McpPromptDto {
                name: "p".into(),
                description: None,
                arguments: vec![],
            }],
        )
        .with_server_info(DiscoveryCacheServerInfo {
            name: "srv".into(),
            version: "1.0".into(),
        });
        store.store(&entry).expect("store");
        assert_eq!(store.load("abc123"), EntryLookup::Found(entry));
    }

    #[test]
    fn store_load_missing_file_is_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = DiscoveryCacheStore::new(dir.path());
        assert_eq!(store.load("nope"), EntryLookup::Absent);
    }

    #[test]
    fn store_load_corrupt_json_is_corrupt_not_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("k.json"), b"not json").expect("write");
        let store = DiscoveryCacheStore::new(dir.path());
        assert_eq!(store.load("k"), EntryLookup::Corrupt);
    }

    #[test]
    fn store_load_oversize_is_corrupt() {
        let dir = tempfile::tempdir().expect("tempdir");
        let huge = vec![b'a'; (MAX_ENTRY_BYTES + 1) as usize];
        std::fs::write(dir.path().join("k.json"), huge).expect("write");
        let store = DiscoveryCacheStore::new(dir.path());
        assert_eq!(store.load("k"), EntryLookup::Corrupt);
    }

    #[test]
    fn store_load_wrong_schema_version_is_corrupt() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = DiscoveryCacheStore::new(dir.path());
        // Otherwise-complete v2 shape, but a version this store doesn't
        // recognize — must be Corrupt via the explicit version check, not
        // the parse-failure path (unlike the v1-shape test below).
        let bad = serde_json::json!({
            "v": 999,
            "cache_key": "k",
            "saved_at_ms": 1,
            "consecutive_refresh_failures": 0,
            "capabilities": {"tools": false, "resources": false, "prompts": false, "logging": false, "experimental": {}},
            "tools": [],
            "resources": [],
            "resource_templates": [],
            "prompts": [],
        });
        std::fs::write(dir.path().join("k.json"), bad.to_string()).expect("write");
        assert_eq!(store.load("k"), EntryLookup::Corrupt);
    }

    #[test]
    fn store_load_old_v1_shape_is_corrupt() {
        // The pre-schema-v2 on-disk shape: correct `v`, but missing every
        // field schema v2 added. Must be Corrupt — proving the version bump
        // actually invalidates old entries rather than silently defaulting
        // their new fields to something `decide` might trust.
        let dir = tempfile::tempdir().expect("tempdir");
        let store = DiscoveryCacheStore::new(dir.path());
        let old = serde_json::json!({
            "v": 1,
            "cache_key": "k",
            "saved_at_ms": 1,
            "consecutive_refresh_failures": 0,
            "capabilities_tools": true,
            "tool_count": 3,
        });
        std::fs::write(dir.path().join("k.json"), old.to_string()).expect("write");
        assert_eq!(store.load("k"), EntryLookup::Corrupt);
    }

    #[test]
    fn store_load_cache_key_mismatch_is_corrupt() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = DiscoveryCacheStore::new(dir.path());
        // Stored under path "k.json" but its OWN cacheKey field says "other" —
        // simulates the oracle's "entry keyed for another server" case.
        let entry = DiscoveryCacheEntry::new(
            "other".into(),
            1,
            caps_tools(false),
            vec![],
            vec![],
            vec![],
            vec![],
        );
        std::fs::write(
            dir.path().join("k.json"),
            serde_json::to_string(&entry).expect("serialize"),
        )
        .expect("write");
        assert_eq!(store.load("k"), EntryLookup::Corrupt);
    }

    #[cfg(unix)]
    #[test]
    fn store_load_symlink_is_corrupt() {
        let dir = tempfile::tempdir().expect("tempdir");
        let real = dir.path().join("real.json");
        let entry = DiscoveryCacheEntry::new(
            "k".into(),
            1,
            caps_tools(false),
            vec![],
            vec![],
            vec![],
            vec![],
        );
        std::fs::write(&real, serde_json::to_string(&entry).expect("serialize")).expect("write");
        std::os::unix::fs::symlink(&real, dir.path().join("k.json")).expect("symlink");
        let store = DiscoveryCacheStore::new(dir.path());
        assert_eq!(store.load("k"), EntryLookup::Corrupt);
    }

    #[test]
    fn purge_of_a_missing_entry_is_not_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = DiscoveryCacheStore::new(dir.path());
        store.purge("nope").expect("purge of absent entry is Ok");
    }

    #[test]
    fn purge_removes_a_stored_entry() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = DiscoveryCacheStore::new(dir.path());
        let entry = DiscoveryCacheEntry::new(
            "abc".into(),
            1,
            caps_tools(false),
            vec![],
            vec![],
            vec![],
            vec![],
        );
        store.store(&entry).expect("store");
        store.purge("abc").expect("purge");
        assert_eq!(store.load("abc"), EntryLookup::Absent);
    }

    #[test]
    fn cache_key_is_stable_and_distinguishes_url() {
        let a = DiscoveryCacheStore::cache_key("srv", &http_spec("https://a.example", None));
        let b = DiscoveryCacheStore::cache_key("srv", &http_spec("https://b.example", None));
        let a_again = DiscoveryCacheStore::cache_key("srv", &http_spec("https://a.example", None));
        assert_eq!(a, a_again);
        assert_ne!(a, b);
    }
}

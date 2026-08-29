//! `.mcp.json` parser (M6-07).
//!
//! Reads the claude-code-compatible shape:
//! ```json
//! {
//!   "mcpServers": {
//!     "memory":     { "command": "mcp-memory",     "args": [], "env": {} },
//!     "filesystem": { "command": "mcp-filesystem", "args": ["/tmp"], "env": {} }
//!   }
//! }
//! ```
//! Each entry is projected into a [`crate::McpServerConfig`] with
//! [`crate::ConfigScope`] supplied by the caller. URL-based ("http"/"sse")
//! entries are accepted via the `url` field.

use crate::connection::{ConfigScope, McpServerConfig};
use crate::env_expansion::expand_env_vars_in_string;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use traits::{McpHeaders, McpOAuthConfigDto, McpTransportSpec};

/// Expand `${VAR}` / `${VAR:-default}` references in one string against the
/// process environment, appending any missing-variable names to `missing`.
/// 1:1 with the inner `expandString` of claude-code `expandEnvVars`
/// (`services/mcp/config.ts:562-566`).
fn expand_field(value: &str, missing: &mut Vec<String>) -> String {
    let r = expand_env_vars_in_string(value);
    missing.extend(r.missing_vars);
    r.expanded
}

/// Expand the VALUES of a `HashMap` (keys untouched), mirroring the TS
/// `mapValues(map, expandString)` used for `env` and `headers`
/// (`services/mcp/config.ts:579,595`).
fn expand_map_values(
    map: HashMap<String, String>,
    missing: &mut Vec<String>,
) -> HashMap<String, String> {
    map.into_iter()
        .map(|(k, v)| {
            let v = expand_field(&v, missing);
            (k, v)
        })
        .collect()
}

/// Order-preserving variant of [`expand_map_values`] for `headers`. The header
/// key order must survive parse → spec so the `getServerKey` config hash
/// byte-matches claude-code (see [`traits::McpHeaders`]).
fn expand_header_values(map: McpHeaders, missing: &mut Vec<String>) -> McpHeaders {
    map.into_iter()
        .map(|(k, v)| {
            let v = expand_field(&v, missing);
            (k, v)
        })
        .collect()
}

/// Errors raised while parsing a `.mcp.json` file.
#[derive(Debug, thiserror::Error)]
pub enum McpJsonError {
    /// Invalid JSON.
    #[error("invalid .mcp.json: {0}")]
    Json(#[from] serde_json::Error),
    /// An entry had neither `command` nor `url` set.
    #[error("server '{0}' missing both command and url")]
    UnknownTransport(String),
}

#[derive(Debug, Deserialize)]
struct McpJsonEntry {
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: HashMap<String, String>,
    #[serde(default)]
    url: Option<String>,
    /// `sdk`-only: the host control-channel identifier. Oracle `MAn`
    /// (2.1.251 Mach-O @154585319) is
    /// `f({type:N("sdk"),name:i(),timeout:o().optional(),alwaysLoad:q().optional()})`
    /// — `name` is a REQUIRED `i()`, in pointed contrast to its `.optional()`
    /// siblings, so `{"type":"sdk"}` fails `safeParse` and the entry is
    /// skipped. It is a plain `i()` with no `.min(1)`, so an EMPTY string is
    /// schema-valid. No other member of the union declares `name`, so zod
    /// strips it everywhere else (mirrored here: only the sdk branch reads
    /// it). The declared value is what the oracle carries on the config and
    /// compares in `tUt` (@160913341:
    /// `Object.values(e).every(t=>t.type==="sdk"&&t.name==="claude-vscode")`)
    /// — it is NOT forced to equal the entry's map key.
    #[serde(default)]
    name: Option<String>,
    /// "http" | "sse" — only honoured when `url` is set.
    #[serde(default, rename = "type")]
    transport_type: Option<String>,
    #[serde(default)]
    headers: McpHeaders,
    #[serde(default, rename = "headersHelper", alias = "headers_helper")]
    headers_helper: Option<String>,
    #[serde(default)]
    oauth: Option<McpOAuthConfigDto>,
    /// LingXi-original extension: a per-entry on/off switch checked directly
    /// on the raw JSON server object. Confirmed ABSENT from every member of
    /// the oracle's on-disk union (2.1.251 Mach-O @154583724 — none of
    /// `fYe`/`OAn`/`l`/`d`/`sGt`/`LAn`/`MAn`/`NAn` declare a `disabled` key),
    /// so a real claude-code `.mcp.json` sharing this field would have it
    /// silently stripped by zod (schemas here are not `.strict()`) and the
    /// server would load enabled regardless. Kept anyway: real Claude Code's
    /// only per-server disable knobs live OUTSIDE `.mcp.json` — the
    /// `disabledMcpServers` / `disabledMcpjsonServers` project-settings lists
    /// (see [`crate::server_gate`]) — so this field adds a strictly
    /// *additional* way to suppress a server from within its own config
    /// entry; it never makes the port accept something the oracle would
    /// reject. Do not remove without a replacement — this is load-bearing
    /// for [`crate::registry`]'s disabled-server handling.
    #[serde(default)]
    disabled: bool,
    /// Per-server `tools/call` timeout in ms. claude-code zod schema
    /// (all transports): `timeout: RKe().optional()` where `RKe =
    /// E.number().int().positive()`. A present-but-invalid value (non-integer,
    /// non-positive, wrong type) fails the entry's `safeParse` → the entry is
    /// skipped (mirrored here: a non-`u64` value fails `serde` decode →
    /// [`build_servers_from_map`] logs + skips the entry).
    #[serde(default, deserialize_with = "de_positive_timeout")]
    timeout: Option<u64>,
    /// sse/http-only alias for `timeout`. claude-code schema:
    /// `request_timeout_ms: nil()` where `nil =
    /// E.number().int().positive().optional().catch(void 0)` — the `.catch`
    /// means an invalid value is coerced to `undefined` (NOT an entry failure).
    /// Held as opaque JSON so a bad value never fails the whole entry; coerced
    /// to a positive integer by [`as_positive_int_ms`] and folded into
    /// `timeout` via the `RAn` transform below (sse/http only).
    #[serde(default, rename = "request_timeout_ms")]
    request_timeout_ms: Option<serde_json::Value>,
    /// `alwaysLoad`: force all of this server's tools into the prompt (never
    /// deferred behind tool search). claude-code schema (all transports):
    /// `alwaysLoad: E.boolean().optional()`.
    #[serde(default, rename = "alwaysLoad")]
    always_load: Option<bool>,
    /// `claudeai-proxy`-only: the claude.ai-issued connector identifier.
    /// Oracle `NAn` (2.1.251 Mach-O @154585377) is
    /// `f({type:N("claudeai-proxy"),url:i(),id:i(),displayName:i().optional(),
    /// iconUrl:i().optional(),timeout:o().optional(),alwaysLoad:q().optional(),
    /// toolPermissions:...,stateless:...,cachedInitResponse:...,...})` — `id`
    /// is a REQUIRED `i()` (no `.optional()`, in pointed contrast to
    /// `displayName`/`iconUrl` and the rest of the schema's tail), so a
    /// `claudeai-proxy` entry with no `id` fails `safeParse` and the whole
    /// entry is skipped — the same shape as `sdk`'s required `name`. `i()`
    /// has no `.min(1)`, so an EMPTY string is schema-valid; only presence is
    /// checked. No other union member declares `id`, so zod strips it
    /// everywhere else (mirrored here: only the claudeai-proxy branch reads
    /// it). The claude.ai connector surface is out of scope, so the value
    /// itself is validated for presence and otherwise unused.
    #[serde(default)]
    id: Option<String>,
}

/// Coerce a JSON value to a positive-integer millisecond count, mirroring the
/// claude-code `request_timeout_ms` zod schema `E.number().int().positive()
/// .optional().catch(void 0)`: a positive integer is kept; anything else
/// (missing, non-number, non-integer, `<= 0`, or overflowing `u64`) becomes
/// `None` (the `.catch(void 0)` leniency).
fn as_positive_int_ms(v: Option<&serde_json::Value>) -> Option<u64> {
    // `as_u64` already rejects negatives, fractional numbers, and non-numbers.
    let n = v?.as_u64()?;
    (n > 0).then_some(n)
}

/// Deserialize the per-server `timeout` field, mirroring the claude-code zod
/// schema `timeout: E.number().int().positive().optional()` (NO `.catch`): an
/// absent field is `None`; a present positive integer is `Some(n)`; a present
/// non-positive `0` — a valid `u64` that `.positive()` rejects — fails the
/// deserialize so [`build_servers_from_map`] logs + SKIPS the whole entry
/// (matching CC's `safeParse` failure). Negatives, fractional, and non-numeric
/// values are already rejected by the `u64` decode itself.
fn de_positive_timeout<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match Option::<u64>::deserialize(deserializer)? {
        Some(0) => Err(serde::de::Error::custom(
            "timeout must be a positive integer",
        )),
        other => Ok(other),
    }
}

/// Validate the `oauth` child, mirroring the oracle's `a()` zod schema
/// (2.1.251 Mach-O @154583724):
/// ```text
/// a = z.object({
///   clientId: z.string().optional(),
///   callbackPort: z.number().int().positive().optional(),
///   authServerMetadataUrl: z.string().url()
///     .startsWith("https://", {message:"authServerMetadataUrl must use https://"})
///     .optional(),
///   scopes: z.string().min(1).optional(),
///   xaa: z.boolean().optional(),
/// })
/// ```
/// Unlike `role`/`request_timeout_ms` (which use `.catch(void 0)` to
/// silently discard a bad value), `oauth: a().optional()` has NO `.catch`:
/// the child schema only short-circuits when the KEY is absent, so a
/// PRESENT-but-invalid `oauth` object fails the containing entry's whole
/// `safeParse`, exactly like a malformed `command`/`url`/`headers`. Returns
/// `None` when the entry must be rejected outright; `Some(oauth)` otherwise
/// (including `Some(None)` when no `oauth` block was given at all).
///
/// `callbackPort` is only checked against `Some(0)` here — a value the oracle
/// schema also rejects. Ports above `u16::MAX` are schema-valid at the oracle
/// (`z.number().int().positive()` has no upper bound) but already fail to
/// deserialize into this port's `McpOAuthConfigDto::callback_port: u16`
/// (`traits/src/mcp.rs`, owned by another batch) before this function ever
/// runs — a stricter, not more permissive, divergence, and out of scope for
/// this fix (changing the DTO's field type is a wire-shape change).
fn validate_oauth_child(oauth: Option<McpOAuthConfigDto>) -> Option<Option<McpOAuthConfigDto>> {
    let Some(cfg) = oauth else {
        return Some(None);
    };
    if cfg.callback_port == Some(0) {
        return None;
    }
    if let Some(url) = &cfg.auth_server_metadata_url {
        if !url.starts_with("https://") || url::Url::parse(url).is_err() {
            return None;
        }
    }
    if let Some(scopes) = &cfg.scopes {
        if scopes.is_empty() {
            return None;
        }
    }
    Some(Some(cfg))
}

/// Parse a `.mcp.json` payload (raw file contents) into a list of configs.
///
/// `scope` propagates onto every returned config so the approval policy can
/// distinguish project from user origins.
///
/// Returns `Ok(vec![])` when the input has no usable server map.
///
/// Mirrors claude-code `loadMcpServersFromFile`
/// (`utils/plugins/mcpPluginIntegration.ts:240-259`): the server map is
/// `parsed.mcpServers || parsed` — a top-level object WITHOUT an `mcpServers`
/// wrapper is treated as a bare map of `{name: serverConfig}`. Each entry is
/// validated independently; an invalid entry (neither `command` nor `url`, or
/// a shape that fails to deserialize) is logged and SKIPPED — the valid
/// siblings are kept and the file is never dropped.
pub fn parse_mcp_json_string(
    raw: &str,
    scope: ConfigScope,
) -> Result<Vec<McpServerConfig>, McpJsonError> {
    // Parse to a generic value first so we can apply the `parsed.mcpServers ||
    // parsed` precedence before committing to the entry shape. Genuinely
    // invalid JSON still fails here (the `Json` error path), matching the JS
    // `try { jsonParse(content) }` outer guard.
    let parsed: serde_json::Value = serde_json::from_str(raw)?;

    // `parsed.mcpServers || parsed`: prefer an `mcpServers` object when present,
    // otherwise treat the whole top-level object as the server map. A
    // non-object top-level (array/string/number/bool/null) yields no servers,
    // matching JS where `Object.entries` over a non-record produces nothing
    // usable.
    let server_map = match parsed.get("mcpServers") {
        Some(v) => v,
        None => &parsed,
    };
    let serde_json::Value::Object(entries) = server_map else {
        return Ok(Vec::new());
    };
    Ok(build_servers_from_map(entries, scope, false))
}

/// Parse a PLUGIN `.mcp.json` payload (or a plugin manifest's inline
/// `mcpServers` record). Identical to [`parse_mcp_json_string`] except that
/// the two internal-only IDE transports ([`IDE_ONLY_TYPES`]) are recognized.
///
/// The oracle's plugin loader `vve` validates each entry against the FULL
/// 8-arm union — `let B=KY().safeParse(U); if(B.success) M[F]=B.data; else
/// n(`Invalid MCP server config for ${F} in ${d}: ...`)` — rather than the
/// 7-key config table `ZGn` that `xqe` uses for `.mcp.json` / settings /
/// `--mcp-config`. Same file name, different schema layer.
///
/// # Errors
/// Returns [`McpJsonError::Json`] when `raw` is not valid JSON.
pub fn parse_plugin_mcp_json_string(
    raw: &str,
    scope: ConfigScope,
) -> Result<Vec<McpServerConfig>, McpJsonError> {
    let parsed: serde_json::Value = serde_json::from_str(raw)?;
    let server_map = match parsed.get("mcpServers") {
        Some(v) => v,
        None => &parsed,
    };
    let serde_json::Value::Object(entries) = server_map else {
        return Ok(Vec::new());
    };
    Ok(build_servers_from_map(entries, scope, true))
}

/// Parse MCP servers from a GLOBAL CONFIG file (`~/.lingxi.json`): reads ONLY the
/// top-level `mcpServers` object. claude-code reads `config.mcpServers` directly
/// (`wt().mcpServers`) with NO `|| parsed` bare-map fallback — the global config
/// holds dozens of unrelated keys (`numStartups`, `projects`, `oauthAccount`, …)
/// that must never be mistaken for server entries. An absent / non-object
/// `mcpServers` yields no servers.
///
/// # Errors
/// Returns [`McpJsonError::Json`] when `raw` is not valid JSON.
pub fn parse_global_config_mcp_servers(
    raw: &str,
    scope: ConfigScope,
) -> Result<Vec<McpServerConfig>, McpJsonError> {
    let parsed: serde_json::Value = serde_json::from_str(raw)?;
    let Some(serde_json::Value::Object(entries)) = parsed.get("mcpServers") else {
        return Ok(Vec::new());
    };
    Ok(build_servers_from_map(entries, scope, false))
}

/// Build validated `McpServerConfig`s from a `{ name: entry }` server map.
/// Shared by [`parse_mcp_json_string`] (bare-map fallback) and
/// [`parse_global_config_mcp_servers`] (mcpServers-only). Invalid entries are
/// logged + skipped (keeping valid siblings); the result is sorted by name.
fn build_servers_from_map(
    entries: &serde_json::Map<String, serde_json::Value>,
    scope: ConfigScope,
    ide_transports_allowed: bool,
) -> Vec<McpServerConfig> {
    let mut out = Vec::new();
    for (name, raw_entry) in entries {
        if let Some(cfg) = build_entry(name, raw_entry, scope, ide_transports_allowed) {
            out.push(cfg);
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// The REMOTE transport `type` values the on-disk CONFIG layer recognizes.
///
/// Oracle `ZGn` (2.1.251 Mach-O @160905638) is the lookup table `xqe` uses for
/// every config source — `.mcp.json` (project/enterprise, via `Iqe`), the
/// `user`/`local` settings `configObject`s, and the `--mcp-config` inline
/// payload:
/// ```text
/// var ZGn={stdio:fYe,sse:OAn,http:sGt,"streamable-http":sGt,ws:LAn,sdk:MAn,"claudeai-proxy":NAn};
/// ```
/// `sse-ide` and `ws-ide` are ABSENT from it — see [`IDE_ONLY_TYPES`].
const CONFIG_REMOTE_TYPES: &[&str] = &["sse", "http", "streamable-http", "ws", "claudeai-proxy"];

/// The two internal-only IDE transports.
///
/// They ARE members of the 8-arm union `KY` (@154586195:
/// `KY=m(()=>dt([fYe(),OAn(),l(),d(),sGt(),LAn(),MAn(),NAn()]))`, where `l` is
/// `sse-ide` and `d` is `ws-ide`), which is what the PLUGIN `.mcp.json` loader
/// `vve` validates against (`let B=KY().safeParse(U)`, @160865300 region) and
/// what agent frontmatter declares (`Dnt=m(()=>dt([i(),De(i(),KY())]))`
/// @160484780) — but they are NOT keys of `ZGn`, so at the config layer
/// `xqe` emits `Skipped — unknown MCP server type "sse-ide" for server "…"`
/// and never loads the entry. Two different schema layers wear the same
/// on-disk file name; the port keeps them apart via `ide_transports_allowed`.
///
/// The agent layer accepts them at the SCHEMA level and then drops them per
/// entry in `esn` (@160485539: `Skipping internal-only MCP transport '…'`),
/// which the port does in `agent::mcp_servers` before this module is reached.
const IDE_ONLY_TYPES: &[&str] = &["sse-ide", "ws-ide"];

/// Does `entry` satisfy the REQUIRED fields of the union member its `type`
/// selects? The single source of truth shared by
/// [`server_entry_shape_is_valid`] (the quiet JSON-agent validator) and
/// [`build_server_from_json_entry`] (the loader), so the two can never again
/// disagree about which shapes are acceptable.
///
/// `ide_transports_allowed` selects the schema LAYER: `false` = the config
/// lookup table [`CONFIG_REMOTE_TYPES`] (oracle `ZGn`, 7 keys); `true` = the
/// full union `KY` (8 arms), which additionally admits [`IDE_ONLY_TYPES`].
fn entry_satisfies_schema(entry: &McpJsonEntry, ide_transports_allowed: bool) -> bool {
    match entry.transport_type.as_deref() {
        // `fYe`: `type: N("stdio").optional()`, `command: i().min(1,"Command
        // cannot be empty")`. An absent `type` is a valid stdio entry.
        None | Some("stdio") => entry.command.as_deref().is_some_and(|c| !c.is_empty()),
        // `MAn`: `name: i()` REQUIRED (no `.min(1)`, so `""` is valid); no
        // `command`, no `url`.
        Some("sdk") => entry.name.is_some(),
        // `NAn` @154585377: `url:i(),id:i()` — BOTH required, `id` with no
        // `.optional()` sibling in common with the rest of the union.
        Some("claudeai-proxy") => entry.url.is_some() && entry.id.is_some(),
        // Every remaining remote member declares a required `url: i()`.
        Some(t) if CONFIG_REMOTE_TYPES.contains(&t) => entry.url.is_some(),
        Some(t) if ide_transports_allowed && IDE_ONLY_TYPES.contains(&t) => entry.url.is_some(),
        // A `type` outside the layer's union rejects the whole entry.
        _ => false,
    }
}

/// Quiet shape check for one raw `.mcp.json`-style entry value: does it
/// deserialize as a server entry AND satisfy the required fields of the union
/// member its `type` selects ([`entry_satisfies_schema`])?
///
/// Mirrors the zod `safeParse` success predicate WITHOUT logging — used by the
/// strict JSON-agent `mcpServers` validator (`z.array(AgentMcpServerSpecSchema)`,
/// where any invalid record value drops the whole agent) so validation at parse
/// time does not double-log the entry the conversion would log again later.
///
/// The agent layer validates against `KY` (`Dnt=m(()=>dt([i(),De(i(),KY())]))`,
/// @160484780), the 8-arm union, so [`IDE_ONLY_TYPES`] are accepted HERE and
/// dropped per entry later by `agent::mcp_servers` (oracle `esn`) — an
/// `sse-ide` entry must not take the whole agent down with it.
#[must_use]
pub fn server_entry_shape_is_valid(raw_entry: &serde_json::Value) -> bool {
    McpJsonEntry::deserialize(raw_entry)
        .map(|e| entry_satisfies_schema(&e, true))
        .unwrap_or(false)
}

/// Build ONE validated [`McpServerConfig`] from a raw `{ name: entry }` map
/// entry (the loop body of [`build_servers_from_map`], extracted). `None` when
/// the entry is invalid — logged + skipped (TS `safeParse` failure →
/// `logForDebugging` + `continue`), keeping valid siblings.
///
/// Public because agent frontmatter `mcpServers` bodies share EXACTLY this
/// config shape: claude `agentMcpSpecsToScopedConfigs` (obs) spreads the parsed
/// config and stamps `scope:"agent"` — the agent crate's conversion calls this
/// with [`ConfigScope::Agent`].
#[must_use]
pub fn build_server_from_json_entry(
    name: &str,
    raw_entry: &serde_json::Value,
    scope: ConfigScope,
) -> Option<McpServerConfig> {
    build_entry(name, raw_entry, scope, false)
}

/// [`build_server_from_json_entry`] with the schema layer selected explicitly.
/// `ide_transports_allowed` = `false` is the config layer (oracle `ZGn`);
/// `true` is the plugin `.mcp.json` layer (oracle `KY`, see
/// [`parse_plugin_mcp_json_string`]).
fn build_entry(
    name: &str,
    raw_entry: &serde_json::Value,
    scope: ConfigScope,
    ide_transports_allowed: bool,
) -> Option<McpServerConfig> {
    {
        // Per-entry validation: a shape that fails to deserialize is logged and
        // skipped (TS `safeParse` failure → `logForDebugging` + `continue`),
        // keeping the valid siblings rather than dropping the whole file.
        let entry: McpJsonEntry = match McpJsonEntry::deserialize(raw_entry) {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(
                    server = %name,
                    error = %e,
                    "mcp.json: invalid server config; skipping entry"
                );
                return None;
            }
        };
        let name = name.to_string();
        // Expand `${VAR}` / `${VAR:-default}` references in the transport
        // fields, mirroring claude-code `expandEnvVars(config)`
        // (`services/mcp/config.ts:556-615`): stdio expands `command`/`args`/
        // `env` VALUES; remote expands `url`/`headers` VALUES. Missing-variable
        // names (deduped, as TS `[...new Set(missingVars)]`) are logged but do
        // NOT fail the parse — the literal `${VAR}` is left in place (TS surfaces
        // a non-fatal config error; a `warn!` is the closest non-breaking
        // analogue for this parser).
        let mut missing: Vec<String> = Vec::new();
        // claude `configError` (`configErrorReason:"url_invalid"`): set when a
        // remote `url` expands to empty; carried on the config, never fatal.
        let mut config_error: Option<String> = None;
        // Discriminate by `type` FIRST. The oracle's config loader `xqe`
        // (@160909118) does exactly this: `let W = typeof B.type==="string" ?
        // B.type : "stdio"` picks the key, `ZGn[W]` picks the schema, and the
        // ONE schema it picked is `safeParse`d — no other arm is tried. (`KY`
        // itself is `dt([...])` and `dt` is `z.union`, NOT
        // `z.discriminatedUnion` — @154569475
        // `function dt(e,t){return new yn({type:"union",options:e,...})}`,
        // `yn = ZodUnion`; the discriminated-union factory is the sibling
        // `ps`. Under a plain union every arm is tried and the entry survives
        // if ANY matches, which for these schemas selects the same arm the
        // `type` literal names and rejects the same inputs — but do not reason
        // from "the discriminator picks exactly one arm" when editing this.)
        // An absent `type` is `fYe`, i.e. stdio, and ONLY stdio: it is never
        // guessed from whichever other fields happen to be present. A `type`
        // value outside the layer's union fails the whole entry. This fixes
        // §12's shape-driven bugs: a bare `{"url":...}` (no `type`, no
        // `command`) used to fall through to Http; `{"type":"http",
        // "command":"x"}` used to be parsed as Stdio because the old code
        // checked `command` before `type`; `{"type":"bogus",...}` and the
        // non-oracle alias `"websocket"` used to silently default to
        // Http/WebSocket instead of being rejected.
        let ty = entry.transport_type.as_deref();
        let is_stdio = matches!(ty, None | Some("stdio"));
        let spec = if ty == Some("sdk") {
            // Oracle `MAn` @154585319 (v2.1.251 Mach-O, minified `mcp-sdk.js`
            // chunk): `f({type:N("sdk"),name:i(),timeout:o().optional(),
            // alwaysLoad:q().optional()})` — no `command`, no `url`, and
            // `name` is REQUIRED (its siblings all carry `.optional()`), so
            // `{"type":"sdk"}` fails `safeParse` and the entry is skipped.
            //
            // The control-channel id is that DECLARED `name`, not the map
            // key: the oracle keeps it on the config and reads it back in
            // `tUt` (@160913341,
            // `every(t=>t.type==="sdk"&&t.name==="claude-vscode")`), which
            // gates the enterprise/remote sdk carve-out. The map key stays
            // the registry key (`A[U]=Oe`). A stray `url` on an sdk entry is
            // schema-unknown and silently stripped by zod, so it is ignored.
            let Some(control_channel_id) = entry.name.clone() else {
                tracing::warn!(
                    server = %name,
                    "mcp.json: sdk server is missing the required \"name\"; skipping entry"
                );
                return None;
            };
            McpTransportSpec::SdkControl { control_channel_id }
        } else if is_stdio {
            let Some(cmd) = entry.command else {
                // No `type` (or explicit `type:"stdio"`) and no `command`:
                // TS `safeParse` rejects it → log + skip, keeping valid
                // siblings. Covers the old bug where a bare `{"url":...}`
                // (no `type`) was accepted as Http — it is really an
                // implicit stdio attempt with a missing `command`.
                tracing::warn!(
                    server = %name,
                    "mcp.json: entry missing both command and url; skipping"
                );
                return None;
            };
            // Oracle `fYe`: `command: i().min(1,"Command cannot be empty")`
            // — an explicit empty string fails validation (a whitespace-only
            // string still satisfies `.min(1)`; this is a length check, not
            // a trim check).
            if cmd.is_empty() {
                tracing::warn!(
                    server = %name,
                    "mcp.json: stdio server has an empty \"command\"; skipping entry (oracle: \"Command cannot be empty\")"
                );
                return None;
            }
            McpTransportSpec::Stdio {
                command: expand_field(&cmd, &mut missing),
                args: entry
                    .args
                    .into_iter()
                    .map(|a| expand_field(&a, &mut missing))
                    .collect(),
                env: expand_map_values(entry.env, &mut missing),
            }
        } else {
            // Every remaining recognized `type` is a URL transport. WHICH
            // ones are recognized depends on the schema layer: the config
            // layer sees only `ZGn`'s keys ([`CONFIG_REMOTE_TYPES`]), the
            // plugin `.mcp.json` layer additionally sees the two
            // [`IDE_ONLY_TYPES`] arms of `KY`.
            let recognized = ty.is_some_and(|t| {
                CONFIG_REMOTE_TYPES.contains(&t)
                    || (ide_transports_allowed && IDE_ONLY_TYPES.contains(&t))
            });
            if !recognized {
                // A `type` string this layer's union has no arm for — e.g.
                // `"bogus"`, the non-oracle alias `"websocket"`, or (at the
                // config layer) `"sse-ide"`/`"ws-ide"` — fails the whole
                // entry; it must NOT silently default to Http/WebSocket.
                // Matches `xqe`'s `Skipped — unknown MCP server type "<t>"
                // for server "<name>"` and the port's own
                // `config_diagnostics::KNOWN_MCP_TYPES`, which has always
                // listed exactly `ZGn`'s 7 keys.
                tracing::warn!(
                    server = %name,
                    r#type = ?ty,
                    "mcp.json: unknown MCP server type; skipping entry"
                );
                return None;
            }
            let Some(raw_url) = entry.url else {
                // A recognized remote type with no `url` at all (e.g.
                // `{"type":"http","command":"x"}`, no `url`): the remote
                // schemas require `url` (a required `i()`, not `.optional()`),
                // so this must be rejected — NOT reinterpreted as stdio just
                // because `command` happens to be present.
                tracing::warn!(
                    server = %name,
                    "mcp.json: entry missing both command and url; skipping"
                );
                return None;
            };
            let url = expand_field(&raw_url, &mut missing);
            // `ey_`'s `urlExpandedToEmpty` = `url.trim() !== "" && expanded
            // .trim() === ""`. A url that was ALREADY blank is not that case:
            // the remote schemas (`cLi` @226761199, `J5n` @226762069) declare
            // `url: E.string()` with NO `.min(1)`, so a blank url is
            // schema-VALID and the entry is kept with no `configError` — which
            // is precisely the `zar` shape `mcp list`/`get` render as
            // `- Not configured` (see `McpServerConfig::is_unconfigured`).
            let url = if !raw_url.trim().is_empty() && url.trim().is_empty() {
                // 2.1.220 `klr`/`ey_`: a NON-blank url that expanded to an
                // empty string KEEPS the server, tagged with the byte-exact
                // `configError` (reason `url_invalid` ⇒ INVALID_CONFIG, NOT
                // unconfigured); the connect path never dials it.
                // The spec keeps the UNEXPANDED url so list/get display shows
                // the `${VAR}` reference (the oracle's display view `bEp` maps
                // back to the `expandVars:false` config for the same effect).
                config_error = Some(format!(
                    "'url' {} expanded to an empty string. Set the referenced \
                     environment variable, or update the server's config and \
                     reconnect.",
                    serde_json::to_string(&raw_url).unwrap_or_else(|_| format!("\"{raw_url}\""))
                ));
                raw_url
            } else {
                url
            };
            let headers = expand_header_values(entry.headers, &mut missing);
            let headers_helper = entry
                .headers_helper
                .map(|helper| expand_field(&helper, &mut missing));
            match ty {
                Some("sse") => {
                    let Some(oauth) = validate_oauth_child(entry.oauth) else {
                        tracing::warn!(server = %name, "mcp.json: invalid oauth config; skipping entry");
                        return None;
                    };
                    McpTransportSpec::Sse {
                        url,
                        headers,
                        headers_helper,
                        oauth,
                    }
                }
                // `claudeai-proxy`: binary-confirmed at offsets 74175408 and
                // 81811504. Used for claude.ai hosted MCP servers; the proxy
                // URL + OAuth handling are resolved by the platform layer.
                // Parsed as Http so the transport chain receives the URL —
                // the platform recognises the `claudeai-proxy` discriminator
                // via the `type` tag when it serializes the spec. Out of
                // scope (claude.ai connector surface): the oracle's `NAn`
                // schema has no `oauth` field at all, left unvalidated here.
                Some("claudeai-proxy") => {
                    // `NAn` @154585377 declares `id: i()` with NO
                    // `.optional()` — in pointed contrast to `displayName`/
                    // `iconUrl` and the rest of the schema's tail — so an
                    // entry with no `id` fails `safeParse` and must be
                    // skipped, the same way a nameless `sdk` entry is
                    // skipped above. The port has no use for the value
                    // itself (out of scope); only presence is checked.
                    if entry.id.is_none() {
                        tracing::warn!(
                            server = %name,
                            "mcp.json: claudeai-proxy server is missing the required \"id\"; skipping entry"
                        );
                        return None;
                    }
                    McpTransportSpec::Http {
                        url,
                        headers,
                        headers_helper,
                        oauth: entry.oauth,
                    }
                }
                // Reachable ONLY on the plugin `.mcp.json` layer
                // (`ide_transports_allowed`), which validates against `KY`.
                // §10 (unmodelled `ideName`/`authToken`, and the unused
                // `McpTransportSpec::SseIde` variant) is a DIFFERENT,
                // unassigned finding — these keep dialling as Http exactly as
                // before rather than being newly rejected on that layer.
                Some("sse-ide" | "ws-ide") => McpTransportSpec::Http {
                    url,
                    headers,
                    headers_helper,
                    oauth: entry.oauth,
                },
                Some("ws") => McpTransportSpec::WebSocket {
                    url,
                    headers,
                    headers_helper,
                },
                // Only "http" | "streamable-http" remain reachable here.
                _ => {
                    let Some(oauth) = validate_oauth_child(entry.oauth) else {
                        tracing::warn!(server = %name, "mcp.json: invalid oauth config; skipping entry");
                        return None;
                    };
                    McpTransportSpec::Http {
                        url,
                        headers,
                        headers_helper,
                        oauth,
                    }
                }
            }
        };
        if !missing.is_empty() {
            // Dedup preserving first-seen order (TS `[...new Set(missingVars)]`).
            let mut seen = std::collections::HashSet::new();
            let deduped: Vec<&String> = missing.iter().filter(|v| seen.insert(*v)).collect();
            tracing::warn!(
                server = %name,
                missing = ?deduped,
                "mcp.json: unresolved ${{VAR}} references left literal"
            );
        }
        // Resolve the per-server `tools/call` timeout. claude-code applies the
        // `RAn` transform to the sse/http schemas ONLY:
        //   RAn({request_timeout_ms:e, ...t}) =>
        //     {...t, ...(t.timeout===void 0 && e!==void 0 && {timeout: min(e, 300_000)})}
        // i.e. when `timeout` is unset and `request_timeout_ms` is set, fold the
        // alias in capped at 300_000ms (LTm). The stdio / sdk-control / ws /
        // ide schemas carry no `request_timeout_ms` field at all (oracle
        // `LAn`, the `ws` schema, has no such key — zod would strip it even
        // if present), so the alias is honoured for the sse/http-family
        // transports only.
        //
        // `McpTransportSpec::Http` also carries `claudeai-proxy` (and,
        // separately, the plugin-layer-only sse-ide/ws-ide), whose schema
        // `NAn` (@154585377) declares neither `request_timeout_ms` nor
        // `.transform(iGt)` — the oracle strips the alias for it entirely.
        // Gate the fold on `ty` (not just the `Http` variant) so a
        // `claudeai-proxy` entry never gets it, matching real `http`/
        // `streamable-http`/`sse` byte for byte. The sse-ide/ws-ide half of
        // the residual is a separate, unassigned finding (§10) and is left
        // exactly as before.
        let timeout_ms = match &spec {
            McpTransportSpec::Sse { .. } | McpTransportSpec::Http { .. }
                if ty != Some("claudeai-proxy") =>
            {
                entry.timeout.or_else(|| {
                    as_positive_int_ms(entry.request_timeout_ms.as_ref()).map(|e| e.min(300_000))
                })
            }
            _ => entry.timeout,
        };
        let always_load = entry.always_load.unwrap_or(false);
        Some(McpServerConfig {
            name,
            spec,
            scope,
            disabled: entry.disabled,
            timeout_ms,
            always_load,
            config_error,
        })
    }
}

/// Load + merge MCP configs from the project `.mcp.json` (cwd) and the user
/// GLOBAL CONFIG file (`~/.lingxi.json`). Project entries take precedence on
/// name collision.
///
/// The project file is parsed with the `mcpServers || parsed` bare-map fallback
/// (`.mcp.json` may be a bare server map); the global config is parsed
/// `mcpServers`-only ([`parse_global_config_mcp_servers`]) so its many unrelated
/// top-level keys are never mistaken for servers. Missing files yield empty
/// lists. Parse errors are logged via `tracing::warn!` and the corresponding
/// file is skipped — a malformed config must not break startup.
#[must_use]
pub fn load_mcp_json_with_precedence(
    project_path: &Path,
    global_path: &Path,
) -> Vec<McpServerConfig> {
    let mut by_name: HashMap<String, McpServerConfig> = HashMap::new();

    // User-global first (lower precedence). The global path is the `~/.lingxi.json`
    // global config, so read ONLY its `mcpServers` key (no bare-map fallback).
    if let Ok(raw) = std::fs::read_to_string(global_path) {
        match parse_global_config_mcp_servers(&raw, ConfigScope::User) {
            Ok(cfgs) => {
                for c in cfgs {
                    by_name.insert(c.name.clone(), c);
                }
            }
            Err(e) => tracing::warn!(
                error = %e,
                path = %global_path.display(),
                "skipping malformed user mcp.json"
            ),
        }
    }

    // Project overrides on name collision. Byte-faithful `Iqe` shape/size
    // guard (`crate::config_diagnostics::read_mcp_config_file`) — a missing
    // file is routine and stays silent (matching oracle); a shape/size
    // rejection or other read error is already logged inside that call.
    match crate::config_diagnostics::read_mcp_config_file(project_path, ConfigScope::Project) {
        Ok(raw) => match parse_mcp_json_string(&raw, ConfigScope::Project) {
            Ok(cfgs) => {
                for c in cfgs {
                    by_name.insert(c.name.clone(), c);
                }
            }
            Err(e) => tracing::warn!(
                error = %e,
                path = %project_path.display(),
                "skipping malformed project .mcp.json"
            ),
        },
        Err(_) => {}
    }

    let mut out: Vec<McpServerConfig> = by_name.into_values().collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Parse LOCAL-scope MCP servers from a GLOBAL CONFIG file (`~/.lingxi.json`):
/// reads ONLY `projects.<project_key>.mcpServers` (claude-code local scope =
/// `wt().projects[Pt()].mcpServers`). NO bare-map fallback. An absent project
/// entry or `mcpServers` key yields no servers.
///
/// `project_key` is the canonical project key
/// ([`migrations::global_config::project_path_for_config`]) — the same key
/// claude-code stores per-project config under.
///
/// # Errors
/// Returns [`McpJsonError::Json`] when `raw` is not valid JSON.
pub fn parse_local_config_mcp_servers(
    raw: &str,
    project_key: &str,
    scope: ConfigScope,
) -> Result<Vec<McpServerConfig>, McpJsonError> {
    let parsed: serde_json::Value = serde_json::from_str(raw)?;
    let Some(serde_json::Value::Object(entries)) = parsed
        .get("projects")
        .and_then(|p| p.get(project_key))
        .and_then(|proj| proj.get("mcpServers"))
    else {
        return Ok(Vec::new());
    };
    Ok(build_servers_from_map(entries, scope, false))
}

/// Load + merge MCP servers across all THREE claude-code config scopes, with
/// precedence LOCAL > PROJECT > USER (claude-code's `["local","project","user"]`
/// priority order — a server defined in a higher scope overrides a same-named
/// one in a lower scope):
/// - USER    = `<global_config>` top-level `mcpServers`
/// - LOCAL   = `<global_config>` `projects.<cwd_key>.mcpServers`
/// - PROJECT = `<cwd>/.mcp.json` (the bare-map `mcpServers || parsed` fallback applies)
///
/// `global_config_path` is `~/.lingxi.json`; `cwd_key` is
/// `migrations::global_config::project_path_for_config(cwd)`. Missing files yield
/// empty lists; parse errors are logged and skipped — a malformed config must
/// not break startup.
#[must_use]
pub fn load_mcp_servers(
    project_mcp_path: &Path,
    global_config_path: &Path,
    cwd: &Path,
) -> Vec<McpServerConfig> {
    let mut by_name: HashMap<String, McpServerConfig> = HashMap::new();
    let global_raw = std::fs::read_to_string(global_config_path).ok();

    // USER (lowest precedence): global config top-level `mcpServers`.
    if let Some(raw) = &global_raw {
        match parse_global_config_mcp_servers(raw, ConfigScope::User) {
            Ok(cfgs) => {
                for c in cfgs {
                    by_name.insert(c.name.clone(), c);
                }
            }
            Err(e) => tracing::warn!(
                error = %e,
                path = %global_config_path.display(),
                "skipping malformed global config (user-scope mcpServers)"
            ),
        }
    }

    // PROJECT (middle): `<cwd>/.mcp.json` (bare-map fallback allowed). Byte-
    // faithful `Iqe` shape/size guard — see the sibling call in
    // [`load_mcp_json_with_precedence`] for the rationale.
    match crate::config_diagnostics::read_mcp_config_file(project_mcp_path, ConfigScope::Project) {
        Ok(raw) => match parse_mcp_json_string(&raw, ConfigScope::Project) {
            Ok(cfgs) => {
                // Approval is evaluated before precedence. A pending/rejected
                // project entry remains visible when it is the only candidate,
                // but it may not shadow an already-loaded user server.
                let policy = crate::server_gate::McpPolicyContext::load(global_config_path, cwd);
                for mut c in cfgs {
                    let decision = policy.decide(&c);
                    let project_blocked = matches!(
                        decision,
                        crate::server_gate::McpServerDecision::Block(
                            crate::server_gate::McpServerBlockReason::ProjectPendingApproval
                                | crate::server_gate::McpServerBlockReason::ProjectRejected
                        )
                    );
                    if project_blocked {
                        c.disabled = true;
                        by_name.entry(c.name.clone()).or_insert(c);
                    } else {
                        by_name.insert(c.name.clone(), c);
                    }
                }
            }
            Err(e) => tracing::warn!(
                error = %e,
                path = %project_mcp_path.display(),
                "skipping malformed project .mcp.json"
            ),
        },
        Err(_) => {}
    }

    // LOCAL (highest): global config `projects.<cwd_key>.mcpServers`.
    if let Some(raw) = &global_raw {
        let key = migrations::global_config::project_path_for_config(cwd);
        match parse_local_config_mcp_servers(raw, &key, ConfigScope::Local) {
            Ok(cfgs) => {
                for c in cfgs {
                    by_name.insert(c.name.clone(), c);
                }
            }
            Err(e) => tracing::warn!(
                error = %e,
                path = %global_config_path.display(),
                "skipping malformed global config (local-scope mcpServers)"
            ),
        }
    }

    let mut out: Vec<McpServerConfig> = by_name.into_values().collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn parse_empty_json_yields_no_servers() {
        let cfgs = parse_mcp_json_string("{}", ConfigScope::Project).unwrap();
        assert!(cfgs.is_empty());
    }

    #[test]
    fn parse_two_stdio_servers() {
        let raw = r#"{
          "mcpServers": {
            "memory":     { "command": "mcp-memory",     "args": [],         "env": {} },
            "filesystem": { "command": "mcp-filesystem", "args": ["/tmp"],   "env": {} }
          }
        }"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::Project).unwrap();
        assert_eq!(cfgs.len(), 2);
        // Sorted by name.
        assert_eq!(cfgs[0].name, "filesystem");
        assert_eq!(cfgs[1].name, "memory");
        match &cfgs[0].spec {
            McpTransportSpec::Stdio { command, args, .. } => {
                assert_eq!(command, "mcp-filesystem");
                assert_eq!(args, &vec!["/tmp".to_string()]);
            }
            other => panic!("expected Stdio, got {other:?}"),
        }
    }

    #[test]
    fn parse_http_url_transport() {
        // §12: an http entry MUST name its type explicitly — the oracle's
        // remote schemas are only reached by naming the `type` key `xqe`
        // looks up in `ZGn`, never inferred from `url` alone (see
        // [`bare_url_without_type_is_rejected`] for the case this replaces).
        let raw = r#"{
          "mcpServers": {
            "remote": { "type": "http", "url": "https://example.test/mcp" }
          }
        }"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::User).unwrap();
        assert_eq!(cfgs.len(), 1);
        match &cfgs[0].spec {
            McpTransportSpec::Http { url, .. } => {
                assert_eq!(url, "https://example.test/mcp");
            }
            other => panic!("expected Http, got {other:?}"),
        }
    }

    /// §12 fix: before this change, `{"url":...}` with NO `type` (and no
    /// `command`) was silently accepted as Http. At the config layer `xqe`
    /// computes `let W = typeof B.type==="string" ? B.type : "stdio"`, so an
    /// absent `type` selects `ZGn["stdio"] = fYe`, which then fails
    /// `command: i().min(1)` because there is no `command` — the whole entry
    /// is skipped, not routed to Http. (The oracle even has a dedicated
    /// message for this exact shape: `Skipped — MCP server "<n>" has a "url"
    /// but no "type"; add "type": "http" (or "sse" / "ws") to this entry`,
    /// reproduced in `config_diagnostics`.) NOTE the union `KY` is built with
    /// `dt` = `z.union`, NOT `z.discriminatedUnion` (@154569475:
    /// `function dt(e,t){return new yn({type:"union",...})}`, `yn=ZodUnion`);
    /// under a plain union every arm is tried and all of them reject this
    /// shape, so the outcome is the same — but the mechanism is not a
    /// discriminator.
    #[test]
    fn bare_url_without_type_is_rejected() {
        let raw = r#"{"mcpServers":{"remote":{"url":"https://example.test/mcp"}}}"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::User).unwrap();
        assert!(
            cfgs.is_empty(),
            "a url-only entry with no type is an implicit (and invalid) stdio attempt, not Http"
        );
    }

    /// §12 fix: `{"type":"bogus","url":...}` must fail the whole entry — the
    /// oracle's config table `ZGn` has no key for an unrecognized `type`
    /// string (`Object.hasOwn(ZGn,W)` is false → `Skipped — unknown MCP
    /// server type`), so it does NOT fall back to Http just because a `url`
    /// happens to be present.
    #[test]
    fn unknown_type_with_url_is_rejected_not_defaulted_to_http() {
        let raw = r#"{"mcpServers":{"remote":{"type":"bogus","url":"https://example.test/mcp"}}}"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::User).unwrap();
        assert!(cfgs.is_empty(), "an unrecognized type must reject the entry");
    }

    /// §12 fix: `{"type":"http","command":"x"}` (no `url`) used to be parsed
    /// as Stdio because the old loader checked `command` before `type`. The
    /// oracle's `http` schema requires `url` (a required field, not a
    /// `command`), so this entry must be rejected entirely.
    #[test]
    fn http_type_with_command_but_no_url_is_rejected() {
        let raw = r#"{"mcpServers":{"remote":{"type":"http","command":"x"}}}"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::User).unwrap();
        assert!(
            cfgs.is_empty(),
            "an http-typed entry must never be reinterpreted as stdio"
        );
    }

    /// §12 fix: oracle `fYe`: `command: i().min(1,"Command cannot be empty")`
    /// — an explicit empty string must fail the entry, not pass through as a
    /// (useless) empty-command stdio server.
    #[test]
    fn empty_stdio_command_is_rejected() {
        let raw = r#"{"mcpServers":{"s":{"command":""}}}"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::Project).unwrap();
        assert!(cfgs.is_empty(), "an empty stdio command must reject the entry");
    }

    /// Oracle `MAn`: `{type:"sdk",name,timeout,alwaysLoad}` — NO `url` and NO
    /// `command`, so the entry must not fall through to the "missing both
    /// command and url" branch. The control-channel id is the entry's own
    /// DECLARED `name`, never a url — sdk entries have none.
    #[test]
    fn sdk_entry_with_no_url_is_accepted_and_keyed_by_name() {
        let raw = r#"{
          "mcpServers": {
            "claude-vscode": { "type": "sdk", "name": "claude-vscode", "timeout": 5000, "alwaysLoad": true }
          }
        }"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::User).unwrap();
        assert_eq!(cfgs.len(), 1, "sdk entry without url/command must be kept");
        assert_eq!(cfgs[0].name, "claude-vscode");
        match &cfgs[0].spec {
            McpTransportSpec::SdkControl { control_channel_id } => {
                assert_eq!(control_channel_id, "claude-vscode");
            }
            other => panic!("expected SdkControl, got {other:?}"),
        }
        // timeout/alwaysLoad are still honoured (all-transports fields).
        assert_eq!(cfgs[0].timeout_ms, Some(5000));
        assert!(cfgs[0].always_load);
    }

    /// Oracle `MAn` @154585319:
    /// `f({type:N("sdk"),name:i(),timeout:o().optional(),alwaysLoad:q().optional()})`
    /// — `name` is the ONE field with no `.optional()`, so `{"type":"sdk"}`
    /// fails `ZGn["sdk"]().safeParse` and `xqe` emits `Skipped — invalid MCP
    /// server config for "x": name: expected string, received undefined`.
    /// The port used to accept it and register a phantom `SdkControl` server
    /// with no SDK instance behind it.
    #[test]
    fn sdk_entry_without_name_is_rejected() {
        let raw = r#"{"mcpServers":{"x":{"type":"sdk","timeout":5000}}}"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::User).unwrap();
        assert!(
            cfgs.is_empty(),
            "`MAn.name` is required; a nameless sdk entry must be skipped"
        );
        // `i()` has no `.min(1)`, so an EMPTY name is schema-valid and kept.
        let raw = r#"{"mcpServers":{"x":{"type":"sdk","name":""}}}"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::User).unwrap();
        assert_eq!(cfgs.len(), 1, "an empty `name` still satisfies `i()`");
    }

    /// The oracle keeps the DECLARED `name` on the config — it is what `tUt`
    /// (@160913341, `every(t=>t.type==="sdk"&&t.name==="claude-vscode")`)
    /// compares for the enterprise/remote sdk carve-out — while the map key
    /// stays the registry key (`A[U]=Oe`). The port used to discard the
    /// declared name and substitute the map key.
    #[test]
    fn sdk_entry_uses_the_declared_name_not_the_map_key() {
        let raw = r#"{"mcpServers":{"vscode":{"type":"sdk","name":"claude-vscode"}}}"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::User).unwrap();
        assert_eq!(cfgs.len(), 1);
        assert_eq!(cfgs[0].name, "vscode", "registry key is the map key");
        match &cfgs[0].spec {
            McpTransportSpec::SdkControl { control_channel_id } => {
                assert_eq!(
                    control_channel_id, "claude-vscode",
                    "the control channel is the DECLARED name, not the map key"
                );
            }
            other => panic!("expected SdkControl, got {other:?}"),
        }
    }

    /// A stray `url` on an sdk entry is schema-unknown (oracle `MAn` has no
    /// `url` field) and must be ignored, not used as the control-channel id.
    #[test]
    fn sdk_entry_ignores_a_stray_url_field() {
        let raw = r#"{"mcpServers":{"srv":{"type":"sdk","name":"srv","url":"should-be-ignored"}}}"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::User).unwrap();
        assert_eq!(cfgs.len(), 1);
        match &cfgs[0].spec {
            McpTransportSpec::SdkControl { control_channel_id } => {
                assert_eq!(control_channel_id, "srv", "must key off name, not url");
            }
            other => panic!("expected SdkControl, got {other:?}"),
        }
    }

    // ── §12 follow-up: the CONFIG layer's type table is `ZGn` (7 keys), not
    //    the 8-arm union `KY`. `xqe` looks the entry's `type` up in
    //    `ZGn={stdio:fYe,sse:OAn,http:sGt,"streamable-http":sGt,ws:LAn,
    //    sdk:MAn,"claudeai-proxy":NAn}` (@160905638) and, finding no key,
    //    emits `Skipped — unknown MCP server type "sse-ide" for server "ide"`.
    //    The PLUGIN `.mcp.json` loader `vve` validates against `KY` instead
    //    (`let B=KY().safeParse(U)`), which DOES have `sse-ide`/`ws-ide`
    //    arms. Same file name, two schema layers. ──

    #[test]
    fn ide_only_types_are_rejected_at_the_config_layer() {
        for ty in ["sse-ide", "ws-ide"] {
            let raw = format!(
                r#"{{"mcpServers":{{"ide":{{"type":"{ty}","url":"http://127.0.0.1:9999/sse","ideName":"VS Code"}}}}}}"#
            );
            let cfgs = parse_mcp_json_string(&raw, ConfigScope::Project).unwrap();
            assert!(
                cfgs.is_empty(),
                "{ty} is absent from ZGn; a .mcp.json entry naming it must be skipped, not dialled as HTTP"
            );
            // Every config entry point shares the table.
            let global = format!(r#"{{"mcpServers":{{"ide":{{"type":"{ty}","url":"http://x/sse"}}}}}}"#);
            assert!(parse_global_config_mcp_servers(&global, ConfigScope::User)
                .unwrap()
                .is_empty());
        }
    }

    /// The port's own `config_diagnostics::KNOWN_MCP_TYPES` already listed
    /// exactly `ZGn`'s 7 keys, so before this fix the loader CONNECTED a
    /// server that `/doctor` and startup reported as skipped. Pin the two
    /// against each other so they cannot drift apart again.
    #[test]
    fn loader_and_diagnostics_agree_on_the_config_type_table() {
        for ty in [
            "stdio",
            "sse",
            "http",
            "streamable-http",
            "ws",
            "sdk",
            "claudeai-proxy",
            "sse-ide",
            "ws-ide",
            "websocket",
            "bogus",
        ] {
            let entry = serde_json::json!({
                "type": ty, "url": "https://x.test/mcp", "command": "c", "name": "n",
                "id": "conn-1"
            });
            let loader_kept =
                build_server_from_json_entry("srv", &entry, ConfigScope::Project).is_some();
            let diagnostics_kept = crate::config_diagnostics::collect_mcp_config_warnings(
                &serde_json::json!({ "mcpServers": { "srv": entry } }),
                ConfigScope::Project,
                None,
            )
            .is_empty();
            assert_eq!(
                loader_kept, diagnostics_kept,
                "type {ty:?}: loader kept={loader_kept} but diagnostics silent={diagnostics_kept}"
            );
        }
    }

    /// The plugin layer is `KY`, so the two IDE arms survive there (they are
    /// still dialled as Http — §10, unmodelled `ideName`/`authToken`, is a
    /// separate finding). This is what keeps the config-layer tightening from
    /// silently amputating plugin-declared IDE servers.
    #[test]
    fn plugin_layer_still_accepts_the_ide_only_types() {
        for ty in ["sse-ide", "ws-ide"] {
            let raw = format!(
                r#"{{"mcpServers":{{"ide":{{"type":"{ty}","url":"http://127.0.0.1:9999/sse","ideName":"VS Code"}}}}}}"#
            );
            let cfgs = parse_plugin_mcp_json_string(&raw, ConfigScope::Dynamic).unwrap();
            assert_eq!(cfgs.len(), 1, "{ty} is an arm of KY, which the plugin loader uses");
        }
        // The plugin layer is not a free-for-all: a type outside KY still fails.
        let raw = r#"{"mcpServers":{"x":{"type":"bogus","url":"http://x"}}}"#;
        assert!(parse_plugin_mcp_json_string(raw, ConfigScope::Dynamic)
            .unwrap()
            .is_empty());
    }

    /// [`server_entry_shape_is_valid`] documents itself as mirroring the zod
    /// `safeParse` success predicate, and the JSON-agent validator
    /// (`agent::catalog::validate_mcp_servers_schema`) drops a whole agent on
    /// a `false`. It was left on the old `command || url || type=="sdk"` rule
    /// while the loader was tightened four ways, so an agent could pass
    /// validation and then silently lose the very server the validator
    /// blessed. Both now route through `entry_satisfies_schema`; this pins
    /// them together over every shape the loader tests exercise.
    #[test]
    fn shape_validator_agrees_with_the_loader() {
        let cases = [
            // (entry, expected-accepted)
            (serde_json::json!({"command": "docs-server"}), true),
            (serde_json::json!({"type": "stdio", "command": "c"}), true),
            (serde_json::json!({"command": ""}), false),
            (serde_json::json!({"url": "https://x.test/mcp"}), false),
            (
                serde_json::json!({"type": "bogus", "url": "https://x.test/mcp"}),
                false,
            ),
            (
                serde_json::json!({"type": "websocket", "url": "wss://x.test"}),
                false,
            ),
            (serde_json::json!({"type": "http", "command": "x"}), false),
            (
                serde_json::json!({"type": "http", "url": "https://x.test/mcp"}),
                true,
            ),
            (serde_json::json!({"type": "ws", "url": "wss://x.test"}), true),
            (serde_json::json!({"type": "sdk"}), false),
            (serde_json::json!({"type": "sdk", "name": "n"}), true),
            (
                serde_json::json!({"type": "claudeai-proxy", "url": "https://x.test/mcp"}),
                false,
            ),
            (
                serde_json::json!({
                    "type": "claudeai-proxy", "url": "https://x.test/mcp", "id": "conn-1"
                }),
                true,
            ),
            (serde_json::json!({"timeout": 0, "command": "c"}), false),
        ];
        // Collect ALL mismatches rather than aborting on the first, so a
        // regression names every shape it broke.
        let mut bad: Vec<String> = Vec::new();
        for (entry, expected) in cases {
            let validator = server_entry_shape_is_valid(&entry);
            let loader = build_server_from_json_entry("srv", &entry, ConfigScope::Agent).is_some();
            if validator != expected || loader != expected {
                bad.push(format!(
                    "{entry}: expected {expected}, validator={validator}, loader={loader}"
                ));
            }
        }
        assert!(bad.is_empty(), "validator/loader disagreed on: {bad:#?}");
        // The one deliberate difference: the agent schema is `KY`, so the two
        // internal-only IDE transports pass validation (the agent-level drop
        // happens later, in `agent::mcp_servers`, mirroring oracle `esn`) even
        // though the CONFIG-layer loader rejects them.
        let ide = serde_json::json!({"type": "sse-ide", "url": "http://x/sse", "ideName": "VS Code"});
        assert!(server_entry_shape_is_valid(&ide));
        assert!(build_server_from_json_entry("ide", &ide, ConfigScope::Agent).is_none());
    }

    #[test]
    fn parses_remote_headers_helper_and_pinned_oauth_scopes() {
        let raw = r#"{
          "mcpServers": {
            "remote": {
              "type": "http",
              "url": "https://example.test/mcp",
              "headers": {"Authorization": "static"},
              "headersHelper": "printf '{}';",
              "oauth": {
                "clientId": "client-id",
                "callbackPort": 8123,
                "authServerMetadataUrl": "https://auth.example/.well-known/oauth-authorization-server",
                "scopes": "read write",
                "xaa": false
              }
            }
          }
        }"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::User).unwrap();
        let McpTransportSpec::Http {
            headers_helper,
            oauth: Some(oauth),
            ..
        } = &cfgs[0].spec
        else {
            panic!("expected HTTP OAuth config")
        };
        assert_eq!(headers_helper.as_deref(), Some("printf '{}';"));
        assert_eq!(oauth.client_id.as_deref(), Some("client-id"));
        assert_eq!(oauth.callback_port, Some(8123));
        assert_eq!(oauth.scopes.as_deref(), Some("read write"));
    }

    // ── §12 OAuth-child validation (oracle `a()`, 2.1.251 Mach-O @154583724):
    //    `callbackPort: v().int().positive()`, `authServerMetadataUrl: i()
    //    .url().startsWith("https://")`, `scopes: i().min(1)` — all
    //    `.optional()` but NONE `.catch()`, so a PRESENT-but-invalid value
    //    fails the WHOLE entry, exactly like a malformed `command`/`url`. ──

    #[test]
    fn oauth_callback_port_zero_rejects_the_whole_entry() {
        let raw = r#"{"mcpServers":{"remote":{"type":"http","url":"https://x.test","oauth":{"callbackPort":0}}}}"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::User).unwrap();
        assert!(
            cfgs.is_empty(),
            "callbackPort:0 fails `.int().positive()`; the server must be skipped, not connected with port 0"
        );
    }

    #[test]
    fn oauth_non_https_auth_server_metadata_url_rejects_the_whole_entry() {
        let raw = r#"{"mcpServers":{"remote":{"type":"http","url":"https://x.test","oauth":{"authServerMetadataUrl":"http://auth.example/.well-known/oauth-authorization-server"}}}}"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::User).unwrap();
        assert!(
            cfgs.is_empty(),
            "a non-https authServerMetadataUrl must fail `.startsWith(\"https://\")`"
        );

        // An arbitrary non-URL string must also reject (the `.url()` half).
        let raw = r#"{"mcpServers":{"remote":{"type":"http","url":"https://x.test","oauth":{"authServerMetadataUrl":"not-a-url"}}}}"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::User).unwrap();
        assert!(cfgs.is_empty(), "a non-URL authServerMetadataUrl must fail `.url()`");
    }

    #[test]
    fn oauth_empty_scopes_rejects_the_whole_entry() {
        let raw = r#"{"mcpServers":{"remote":{"type":"http","url":"https://x.test","oauth":{"scopes":""}}}}"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::User).unwrap();
        assert!(cfgs.is_empty(), "an empty scopes string fails `.min(1)`");
    }

    #[test]
    fn oauth_valid_https_metadata_url_is_kept() {
        let raw = r#"{"mcpServers":{"remote":{"type":"http","url":"https://x.test","oauth":{"authServerMetadataUrl":"https://auth.example/.well-known/oauth-authorization-server","callbackPort":8123,"scopes":"read"}}}}"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::User).unwrap();
        assert_eq!(cfgs.len(), 1, "a fully valid oauth child must keep the server");
    }

    #[test]
    fn websocket_accepts_ordered_headers_and_helper() {
        // §12: only the oracle's literal `"ws"` names the ws schema
        // (`LAn`, 2.1.251 Mach-O @154585377) — the non-oracle alias
        // `"websocket"` is exercised separately in
        // [`non_oracle_websocket_alias_is_rejected`].
        let raw = r#"{
          "mcpServers": {
            "remote": {
              "type": "ws",
              "url": "wss://example.test/mcp",
              "headers": {"X-First": "1", "X-Second": "2"},
              "headersHelper": "helper"
            }
          }
        }"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::User).unwrap();
        let McpTransportSpec::WebSocket {
            headers,
            headers_helper,
            ..
        } = &cfgs[0].spec
        else {
            panic!("expected WebSocket config")
        };
        assert_eq!(
            headers.keys().map(String::as_str).collect::<Vec<_>>(),
            ["X-First", "X-Second"]
        );
        assert_eq!(headers_helper.as_deref(), Some("helper"));
    }

    /// §12 fix: `"websocket"` is not one of the oracle's union
    /// literals (only `"ws"` is) — it must reject, not silently alias to
    /// `McpTransportSpec::WebSocket`.
    #[test]
    fn non_oracle_websocket_alias_is_rejected() {
        let raw = r#"{"mcpServers":{"remote":{"type":"websocket","url":"wss://example.test/mcp"}}}"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::User).unwrap();
        assert!(cfgs.is_empty(), "\"websocket\" is not an oracle-valid type");
    }

    /// §12 fix: oracle `LAn` (the `ws` schema) has no `request_timeout_ms`
    /// field at all — unlike `sse`/`http`, the alias must never be folded
    /// into `timeout` for a `ws` transport.
    #[test]
    fn ws_ignores_request_timeout_ms() {
        let raw = r#"{"mcpServers":{"r":{"type":"ws","url":"wss://x.test","request_timeout_ms":45000}}}"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::User).unwrap();
        assert_eq!(cfgs.len(), 1);
        assert_eq!(
            cfgs[0].timeout_ms, None,
            "ws must not fold request_timeout_ms (oracle schema strips it)"
        );
    }

    /// Oracle `NAn` @154585377 declares `timeout:o().optional()` but NO
    /// `request_timeout_ms` field and no `.transform(iGt)` — unlike
    /// `sse`/`http`'s `sGt`/`OAn` (which DO fold the alias via `RAn`),
    /// `claudeai-proxy` must never receive it, even though it is parsed onto
    /// the same `McpTransportSpec::Http` Rust variant those two use.
    #[test]
    fn claudeai_proxy_ignores_request_timeout_ms() {
        let raw = r#"{"mcpServers":{"r":{"type":"claudeai-proxy","url":"https://x.test","id":"conn-1","request_timeout_ms":45000}}}"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::User).unwrap();
        assert_eq!(cfgs.len(), 1);
        assert_eq!(
            cfgs[0].timeout_ms, None,
            "claudeai-proxy must not fold request_timeout_ms (oracle schema NAn has no such field)"
        );
        // An explicit `timeout` is still honoured — only the alias is stripped.
        let raw = r#"{"mcpServers":{"r":{"type":"claudeai-proxy","url":"https://x.test","id":"conn-1","timeout":9000,"request_timeout_ms":45000}}}"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::User).unwrap();
        assert_eq!(cfgs[0].timeout_ms, Some(9000));
    }

    /// Oracle `NAn` @154585377 declares `id: i()` with NO `.optional()` — in
    /// pointed contrast to `displayName`/`iconUrl` and the rest of the
    /// schema's tail — so a `claudeai-proxy` entry with no `id` fails
    /// `safeParse` and must be skipped, the same way a nameless `sdk` entry
    /// is skipped (see `sdk_entry_without_name_is_rejected`).
    #[test]
    fn claudeai_proxy_entry_without_id_is_rejected() {
        let raw = r#"{"mcpServers":{"x":{"type":"claudeai-proxy","url":"https://x.test"}}}"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::User).unwrap();
        assert!(
            cfgs.is_empty(),
            "`NAn.id` is required; an idless claudeai-proxy entry must be skipped"
        );
        // `i()` has no `.min(1)`, so an EMPTY id still satisfies the schema.
        let raw = r#"{"mcpServers":{"x":{"type":"claudeai-proxy","url":"https://x.test","id":""}}}"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::User).unwrap();
        assert_eq!(cfgs.len(), 1, "an empty `id` still satisfies `i()`");
    }

    #[test]
    fn missing_command_and_url_is_skipped_not_error() {
        // claude-code `loadMcpServersFromFile` logs+skips an invalid entry
        // (safeParse failure → continue) instead of dropping the file. A lone
        // bad entry therefore yields an empty list, NOT an Err.
        let raw = r#"{"mcpServers":{"bogus":{}}}"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::Project).unwrap();
        assert!(cfgs.is_empty(), "bad entry skipped, no Err");
    }

    /// An empty remote url is KEPT, not dropped.
    ///
    /// Verified by running both binaries over the same `.mcp.json`: the oracle
    /// LISTS a `{"type":"http","url":""}` server (`a[u]=T` retains every entry)
    /// and refuses it at CONNECT as UNCONFIGURED (`zar`). Dropping it here made
    /// the server vanish from `mcp list` and `mcp get` with no explanation, so
    /// a typo or an unset env var looked like the server had never been
    /// configured at all.
    #[test]
    fn blank_remote_url_is_kept_as_unconfigured() {
        // `cLi`/`J5n` declare `url: E.string()` with NO `.min(1)` (contrast
        // stdio's `command: E.string().min(1)`), so a blank url is
        // schema-VALID: `klr` keeps the entry with NO configError, which is
        // exactly `zar`'s shape ⇒ `- Not configured`.
        for raw in [
            r#"{"mcpServers":{"remote":{"type":"http","url":"   "}}}"#,
            r#"{"mcpServers":{"remote":{"type":"http","url":""}}}"#,
        ] {
            let cfgs = parse_mcp_json_string(raw, ConfigScope::Project).unwrap();
            assert_eq!(cfgs.len(), 1, "blank remote url must keep the server");
            assert_eq!(cfgs[0].config_error, None, "blank url is not a configError");
            assert!(
                cfgs[0].is_unconfigured(),
                "blank url with no configError is claude `zar`"
            );
        }
    }

    #[test]
    fn url_expanded_to_empty_is_kept_with_config_error() {
        // 2.1.220 `klr`/`ey_` (`urlExpandedToEmpty`): a NON-empty url that
        // expands to an empty string KEEPS the server, tagged with the
        // byte-exact configError. Reason `url_invalid` ⇒ `zar` is false and
        // `Nxe` classifies it INVALID_CONFIG (`✘ Failed to connect` + the
        // configError as the issue), NOT `- Not configured`.
        //
        // NB an UNSET `${VAR}` with no default stays LITERAL (bY returns the
        // match) so it does NOT empty the url; emptiness needs a var that IS
        // set to "" or — deterministic for a test — an empty `:-` default.
        let raw = r#"{"mcpServers":{"r":{"type":"http","url":"${LINGXI_MCP_TEST_UNSET_M5:-}"}}}"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::Project).unwrap();
        assert_eq!(cfgs.len(), 1, "expanded-to-empty url must keep the server");
        assert_eq!(
            cfgs[0].config_error.as_deref(),
            Some(
                "'url' \"${LINGXI_MCP_TEST_UNSET_M5:-}\" expanded to an empty string. \
                 Set the referenced environment variable, or update the server's \
                 config and reconnect."
            )
        );
        // The spec keeps the UNEXPANDED url so list/get display the `${VAR}`
        // reference (the oracle's display view maps back to the
        // `expandVars:false` config for the same effect).
        match &cfgs[0].spec {
            McpTransportSpec::Http { url, .. } => {
                assert_eq!(url, "${LINGXI_MCP_TEST_UNSET_M5:-}");
            }
            other => panic!("expected Http, got {other:?}"),
        }
        assert!(
            !cfgs[0].is_unconfigured(),
            "`configErrorReason:\"url_invalid\"` is INVALID_CONFIG, not UNCONFIGURED"
        );
        // A url that expands to something non-empty carries NO configError.
        let ok = parse_mcp_json_string(
            r#"{"mcpServers":{"r":{"type":"http","url":"${B:-https://x.test}"}}}"#,
            ConfigScope::Project,
        )
        .unwrap();
        assert_eq!(ok[0].config_error, None);
        assert!(!ok[0].is_unconfigured());
    }

    #[test]
    fn bare_map_without_mcpservers_wrapper_parses() {
        // claude-code: `const mcpServers = parsed.mcpServers || parsed` — a
        // top-level map of {name: serverConfig} with NO `mcpServers` wrapper is
        // accepted as the server map directly.
        let raw = r#"{"x":{"command":"foo"}}"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::Project).unwrap();
        assert_eq!(cfgs.len(), 1);
        assert_eq!(cfgs[0].name, "x");
        match &cfgs[0].spec {
            McpTransportSpec::Stdio { command, .. } => assert_eq!(command, "foo"),
            other => panic!("expected Stdio, got {other:?}"),
        }
    }

    #[test]
    fn invalid_entry_is_skipped_valid_kept() {
        // One invalid entry (no command/url) and one valid: the valid one
        // survives, the file is NOT dropped (per-entry skip, no Err).
        let raw = r#"{"mcpServers":{"bad":{},"good":{"command":"g"}}}"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::Project).unwrap();
        assert_eq!(cfgs.len(), 1);
        assert_eq!(cfgs[0].name, "good");
        match &cfgs[0].spec {
            McpTransportSpec::Stdio { command, .. } => assert_eq!(command, "g"),
            other => panic!("expected Stdio, got {other:?}"),
        }
    }

    #[test]
    fn mcpservers_wrapper_takes_precedence_over_sibling_keys() {
        // `parsed.mcpServers || parsed`: when the wrapper exists, ONLY its
        // contents are used — sibling top-level keys are ignored, not merged.
        let raw = r#"{
          "mcpServers": { "wrapped": { "command": "w" } },
          "sibling": { "command": "s" }
        }"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::Project).unwrap();
        assert_eq!(cfgs.len(), 1);
        assert_eq!(cfgs[0].name, "wrapped");
    }

    #[test]
    fn project_overrides_user_on_name_collision() {
        let dir = TempDir::new().unwrap();
        let project = dir.path().join(".mcp.json");
        let global = dir.path().join("user-mcp.json");
        fs::write(&global, r#"{"mcpServers":{"x":{"command":"global-x"}}}"#).unwrap();
        fs::write(&project, r#"{"mcpServers":{"x":{"command":"project-x"}}}"#).unwrap();

        let cfgs = load_mcp_json_with_precedence(&project, &global);
        assert_eq!(cfgs.len(), 1);
        assert_eq!(cfgs[0].scope, ConfigScope::Project);
        match &cfgs[0].spec {
            McpTransportSpec::Stdio { command, .. } => assert_eq!(command, "project-x"),
            other => panic!("got {other:?}"),
        }
    }

    // ── Batch 5b: ${VAR} env-expansion wired into the parse site ──────────

    #[test]
    fn stdio_fields_expand_default_values() {
        // No env mutation needed: `${VAR:-default}` resolves to the default.
        let raw = r#"{
          "mcpServers": {
            "s": {
              "command": "${BIN:-mcp-memory}",
              "args": ["--port", "${PORT:-8080}"],
              "env": { "TOKEN": "${TOK:-abc}" }
            }
          }
        }"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::Project).unwrap();
        assert_eq!(cfgs.len(), 1);
        match &cfgs[0].spec {
            McpTransportSpec::Stdio { command, args, env } => {
                assert_eq!(command, "mcp-memory");
                assert_eq!(args, &vec!["--port".to_string(), "8080".to_string()]);
                assert_eq!(env.get("TOKEN").map(String::as_str), Some("abc"));
            }
            other => panic!("expected Stdio, got {other:?}"),
        }
    }

    #[test]
    fn url_and_headers_expand_default_values() {
        let raw = r#"{
          "mcpServers": {
            "r": {
              "type": "http",
              "url": "${BASE:-https://example.test}/mcp",
              "headers": { "Authorization": "Bearer ${TOKEN:-xyz}" }
            }
          }
        }"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::User).unwrap();
        match &cfgs[0].spec {
            McpTransportSpec::Http { url, headers, .. } => {
                assert_eq!(url, "https://example.test/mcp");
                assert_eq!(
                    headers.get("Authorization").map(String::as_str),
                    Some("Bearer xyz")
                );
            }
            other => panic!("expected Http, got {other:?}"),
        }
    }

    #[test]
    fn headers_preserve_config_insertion_order_for_server_key() {
        // Two headers declared Z-then-A (NON-alphabetical). The parsed spec
        // must keep that order so `oauth::server_key` byte-matches claude-code's
        // insertion-order `JSON.stringify` (see oauth::tests). A sorted map
        // would reorder to A,Z and diverge.
        let raw = r#"{
          "mcpServers": {
            "ordered": {
              "url": "https://mcp.example.com/v1",
              "type": "http",
              "headers": { "Z-Header": "z", "A-Header": "a" }
            }
          }
        }"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::Project).unwrap();
        match &cfgs[0].spec {
            McpTransportSpec::Http { headers, .. } => {
                let order: Vec<&str> = headers.keys().map(String::as_str).collect();
                assert_eq!(order, vec!["Z-Header", "A-Header"], "insertion order kept");
            }
            other => panic!("expected Http, got {other:?}"),
        }
        // End-to-end: the server key matches the pinned insertion-order
        // reference hash (Node-computed in oauth::tests), NOT the sorted one.
        let key = crate::oauth::server_key("ordered", &cfgs[0].spec);
        assert_eq!(key, "ordered|b555b45e666ffa13");
    }

    #[test]
    fn set_env_var_is_substituted_into_command() {
        // A uniquely-named var (set for this process) is substituted.
        std::env::set_var("LINGXI_MCP_TEST_BIN_5B", "/opt/mcp/bin");
        let raw = r#"{"mcpServers":{"s":{"command":"${LINGXI_MCP_TEST_BIN_5B}"}}}"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::Project).unwrap();
        match &cfgs[0].spec {
            McpTransportSpec::Stdio { command, .. } => assert_eq!(command, "/opt/mcp/bin"),
            other => panic!("expected Stdio, got {other:?}"),
        }
        std::env::remove_var("LINGXI_MCP_TEST_BIN_5B");
    }

    #[test]
    fn missing_var_is_left_literal_and_does_not_fail_parse() {
        // An unset `${MISSING}` with no default is left verbatim; parsing still
        // succeeds (TS surfaces a non-fatal error, never aborts the config).
        let raw = r#"{"mcpServers":{"s":{"command":"${LINGXI_MCP_TEST_UNSET_5B}","args":["ok"]}}}"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::Project).unwrap();
        match &cfgs[0].spec {
            McpTransportSpec::Stdio { command, args, .. } => {
                assert_eq!(command, "${LINGXI_MCP_TEST_UNSET_5B}");
                assert_eq!(args, &vec!["ok".to_string()]);
            }
            other => panic!("expected Stdio, got {other:?}"),
        }
    }

    #[test]
    fn malformed_user_file_is_skipped_silently() {
        let dir = TempDir::new().unwrap();
        let project = dir.path().join(".mcp.json");
        let global = dir.path().join("user-mcp.json");
        fs::write(&global, "{ not json").unwrap();
        fs::write(&project, r#"{"mcpServers":{"y":{"command":"y"}}}"#).unwrap();
        let cfgs = load_mcp_json_with_precedence(&project, &global);
        assert_eq!(cfgs.len(), 1);
        assert_eq!(cfgs[0].name, "y");
    }

    // ── Global config (`~/.lingxi.json`) user-scope MCP, mcpServers-only ──────

    #[test]
    fn global_config_reads_only_mcp_servers_key_not_siblings() {
        // A real `~/.lingxi.json` has many unrelated top-level keys. The global
        // reader must extract ONLY `mcpServers` and never treat e.g. `projects`
        // or `numStartups` as server entries (claude-code `wt().mcpServers`).
        let raw = r#"{
          "numStartups": 7,
          "oauthAccount": { "emailAddress": "x@y.z" },
          "projects": { "/some/proj": { "allowedTools": ["Bash"] } },
          "mcpServers": { "mem": { "command": "mcp-mem" } }
        }"#;
        let cfgs = parse_global_config_mcp_servers(raw, ConfigScope::User).unwrap();
        assert_eq!(cfgs.len(), 1);
        assert_eq!(cfgs[0].name, "mem");
        assert_eq!(cfgs[0].scope, ConfigScope::User);
    }

    #[test]
    fn global_config_without_mcp_servers_yields_none_no_bare_map_fallback() {
        // No `mcpServers` key: must yield NO servers. The bare-map `|| parsed`
        // fallback (valid for `.mcp.json`) must NOT apply to the global config —
        // otherwise `numStartups` / `projects` would be mis-parsed as servers.
        let raw = r#"{ "numStartups": 7, "projects": { "/p": {} } }"#;
        let cfgs = parse_global_config_mcp_servers(raw, ConfigScope::User).unwrap();
        assert!(cfgs.is_empty());
    }

    #[test]
    fn precedence_loader_reads_global_config_mcp_servers_key() {
        // End-to-end: the precedence loader pointed at a global-config file reads
        // its `mcpServers`, ignoring sibling keys.
        let dir = TempDir::new().unwrap();
        let project = dir.path().join(".mcp.json"); // absent
        let global = dir.path().join(".lingxi.json");
        fs::write(
            &global,
            r#"{"numStartups":3,"mcpServers":{"g":{"command":"g"}}}"#,
        )
        .unwrap();
        let cfgs = load_mcp_json_with_precedence(&project, &global);
        assert_eq!(cfgs.len(), 1);
        assert_eq!(cfgs[0].name, "g");
        assert_eq!(cfgs[0].scope, ConfigScope::User);
    }

    // ── Local-scope MCP (`~/.lingxi.json` projects[<key>].mcpServers) ─────────

    #[test]
    fn local_scope_reads_projects_keyed_mcp_servers() {
        let raw = r#"{"projects":{"/some/proj":{"mcpServers":{"loc":{"command":"loc-cmd"}}}}}"#;
        let cfgs = parse_local_config_mcp_servers(raw, "/some/proj", ConfigScope::Local).unwrap();
        assert_eq!(cfgs.len(), 1);
        assert_eq!(cfgs[0].name, "loc");
        assert_eq!(cfgs[0].scope, ConfigScope::Local);
    }

    #[test]
    fn local_scope_absent_project_key_yields_none() {
        // Project entry exists for a DIFFERENT dir → no local servers for ours.
        let raw = r#"{"projects":{"/other":{"mcpServers":{"x":{"command":"x"}}}}}"#;
        let cfgs = parse_local_config_mcp_servers(raw, "/some/proj", ConfigScope::Local).unwrap();
        assert!(cfgs.is_empty());
    }

    #[test]
    fn three_scope_precedence_local_over_project_over_user() {
        // Same server name "s" in ALL three scopes → LOCAL wins (claude-code
        // priority order ["local","project","user"]).
        let dir = TempDir::new().unwrap();
        let cwd = dir.path();
        let project = cwd.join(".mcp.json");
        let global = cwd.join(".lingxi.json");
        // Key BOTH the fixture and the loader via the same canonical resolver, so
        // they match regardless of temp-dir symlink canonicalization.
        let key = migrations::global_config::project_path_for_config(cwd);
        let mut projects = serde_json::Map::new();
        projects.insert(
            key,
            serde_json::json!({ "mcpServers": { "s": { "command": "local-s" } } }),
        );
        let global_json = serde_json::json!({
            "mcpServers": { "s": { "command": "user-s" } },
            "projects": serde_json::Value::Object(projects),
        });
        fs::write(&global, serde_json::to_string(&global_json).unwrap()).unwrap();
        fs::write(&project, r#"{"mcpServers":{"s":{"command":"project-s"}}}"#).unwrap();

        let cfgs = load_mcp_servers(&project, &global, cwd);
        assert_eq!(cfgs.len(), 1);
        assert_eq!(cfgs[0].scope, ConfigScope::Local);
        match &cfgs[0].spec {
            McpTransportSpec::Stdio { command, .. } => assert_eq!(command, "local-s"),
            other => panic!("expected Stdio, got {other:?}"),
        }
    }

    #[test]
    fn three_scope_distinct_names_all_present_with_correct_scopes() {
        let dir = TempDir::new().unwrap();
        let cwd = dir.path();
        let project = cwd.join(".mcp.json");
        let global = cwd.join(".lingxi.json");
        let key = migrations::global_config::project_path_for_config(cwd);
        let mut projects = serde_json::Map::new();
        projects.insert(
            key,
            serde_json::json!({ "mcpServers": { "loc": { "command": "loc" } } }),
        );
        let global_json = serde_json::json!({
            "numStartups": 9,
            "mcpServers": { "usr": { "command": "usr" } },
            "projects": serde_json::Value::Object(projects),
        });
        fs::write(&global, serde_json::to_string(&global_json).unwrap()).unwrap();
        fs::write(&project, r#"{"mcpServers":{"prj":{"command":"prj"}}}"#).unwrap();

        let cfgs = load_mcp_servers(&project, &global, cwd);
        assert_eq!(cfgs.len(), 3);
        let by_name: std::collections::HashMap<&str, ConfigScope> =
            cfgs.iter().map(|c| (c.name.as_str(), c.scope)).collect();
        assert_eq!(by_name["loc"], ConfigScope::Local);
        assert_eq!(by_name["prj"], ConfigScope::Project);
        assert_eq!(by_name["usr"], ConfigScope::User);
    }

    #[test]
    fn pending_project_server_does_not_shadow_user_server() {
        let dir = TempDir::new().unwrap();
        let cwd = dir.path();
        let project = cwd.join(".mcp.json");
        let global = cwd.join(".lingxi.json");
        fs::write(
            &global,
            r#"{"mcpServers":{"docs":{"command":"user-docs"}}}"#,
        )
        .unwrap();
        fs::write(
            &project,
            r#"{"mcpServers":{"docs":{"command":"project-docs"}}}"#,
        )
        .unwrap();

        let cfgs = load_mcp_servers(&project, &global, cwd);
        assert_eq!(cfgs.len(), 1);
        assert_eq!(cfgs[0].scope, ConfigScope::User);
        assert!(matches!(
            &cfgs[0].spec,
            McpTransportSpec::Stdio { command, .. } if command == "user-docs"
        ));
    }

    #[test]
    fn approved_project_server_can_shadow_user_server() {
        let dir = TempDir::new().unwrap();
        let cwd = dir.path();
        let project = cwd.join(".mcp.json");
        let global = cwd.join(".lingxi.json");
        let key = migrations::global_config::project_path_for_config(cwd);
        let global_json = serde_json::json!({
            "mcpServers": { "docs": { "command": "user-docs" } },
            "projects": {
                key: { "enabledMcpjsonServers": ["docs"] }
            }
        });
        fs::write(&global, serde_json::to_vec(&global_json).unwrap()).unwrap();
        fs::write(
            &project,
            r#"{"mcpServers":{"docs":{"command":"project-docs"}}}"#,
        )
        .unwrap();

        let cfgs = load_mcp_servers(&project, &global, cwd);
        assert_eq!(cfgs.len(), 1);
        assert_eq!(cfgs[0].scope, ConfigScope::Project);
        assert!(matches!(
            &cfgs[0].spec,
            McpTransportSpec::Stdio { command, .. } if command == "project-docs"
        ));
    }

    // ── Per-server `timeout` / `request_timeout_ms` / `alwaysLoad` (parity
    //    2.1.207 P2-01): zod schemas `timeout: RKe().optional()` (all
    //    transports), `request_timeout_ms: nil()` (sse/http only, folded by
    //    `RAn`), `alwaysLoad: E.boolean().optional()`. ─────────────────────

    #[test]
    fn stdio_timeout_is_parsed() {
        let raw = r#"{"mcpServers":{"s":{"command":"c","timeout":5000}}}"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::Project).unwrap();
        assert_eq!(cfgs[0].timeout_ms, Some(5000));
        assert!(!cfgs[0].always_load);
    }

    #[test]
    fn http_request_timeout_ms_folds_into_timeout_capped_at_300_000() {
        // RAn: `timeout` unset + `request_timeout_ms` set → timeout =
        // min(request_timeout_ms, 300_000). A huge alias is capped.
        let raw = r#"{"mcpServers":{"r":{"type":"http","url":"https://x.test","request_timeout_ms":999999999}}}"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::User).unwrap();
        assert_eq!(cfgs[0].timeout_ms, Some(300_000), "capped at LTm=300_000");

        // Under the cap it is folded verbatim.
        let raw = r#"{"mcpServers":{"r":{"type":"http","url":"https://x.test","request_timeout_ms":45000}}}"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::User).unwrap();
        assert_eq!(cfgs[0].timeout_ms, Some(45000));
    }

    #[test]
    fn http_timeout_wins_over_request_timeout_ms() {
        // RAn only folds when `timeout` is UNSET; an explicit `timeout` wins and
        // is NOT capped at 300_000.
        let raw = r#"{"mcpServers":{"r":{"type":"http","url":"https://x.test","timeout":600000,"request_timeout_ms":10}}}"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::User).unwrap();
        assert_eq!(cfgs[0].timeout_ms, Some(600000));
    }

    #[test]
    fn stdio_ignores_request_timeout_ms() {
        // The stdio zod schema carries no `request_timeout_ms` field (zod strips
        // it); the alias must NOT be folded for a stdio transport.
        let raw = r#"{"mcpServers":{"s":{"command":"c","request_timeout_ms":45000}}}"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::Project).unwrap();
        assert_eq!(cfgs[0].timeout_ms, None);
    }

    #[test]
    fn always_load_is_parsed() {
        let raw = r#"{"mcpServers":{"s":{"command":"c","alwaysLoad":true}}}"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::Project).unwrap();
        assert!(cfgs[0].always_load);
    }

    #[test]
    fn invalid_request_timeout_ms_is_caught_not_fatal() {
        // `nil()` has `.catch(void 0)`: a bad value becomes undefined rather than
        // failing the entry. A string alias must NOT skip the server; it is just
        // ignored (no fold).
        let raw = r#"{"mcpServers":{"r":{"type":"http","url":"https://x.test","request_timeout_ms":"nope"}}}"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::User).unwrap();
        assert_eq!(
            cfgs.len(),
            1,
            "invalid request_timeout_ms must not drop the server"
        );
        assert_eq!(cfgs[0].timeout_ms, None);

        // Non-positive alias is also coerced away (.positive()).
        let raw = r#"{"mcpServers":{"r":{"type":"http","url":"https://x.test","request_timeout_ms":0}}}"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::User).unwrap();
        assert_eq!(cfgs[0].timeout_ms, None);
    }

    #[test]
    fn invalid_timeout_type_skips_entry() {
        // `timeout: RKe()` has NO `.catch`, so a non-integer value fails the
        // entry's safeParse → the whole server is skipped (valid siblings kept).
        let raw =
            r#"{"mcpServers":{"bad":{"command":"c","timeout":"soon"},"good":{"command":"g"}}}"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::Project).unwrap();
        assert_eq!(cfgs.len(), 1);
        assert_eq!(cfgs[0].name, "good");
    }

    #[test]
    fn zero_timeout_skips_entry() {
        // `timeout: RKe()` = `number().int().positive()` with NO `.catch`, so a
        // present non-positive `0` fails the entry's safeParse → the whole
        // server is skipped (valid siblings kept). `0` is a valid `u64`, so
        // serde alone accepts it; the positive-integer guard is what drops the
        // entry, matching CC's `.positive()` (review RV10).
        let raw = r#"{"mcpServers":{"bad":{"command":"c","timeout":0},"good":{"command":"g"}}}"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::Project).unwrap();
        assert_eq!(cfgs.len(), 1);
        assert_eq!(cfgs[0].name, "good");

        // Same for a remote entry with a non-positive `timeout` (the direct
        // `timeout` field, distinct from the `.catch`-lenient
        // `request_timeout_ms` alias). `type` must be explicit here — without
        // it this would already be rejected as a type-less (implicit-stdio)
        // entry for an unrelated reason, which would pin nothing about
        // `timeout:0` specifically.
        let raw = r#"{"mcpServers":{"r":{"type":"http","url":"https://x.test","timeout":0}}}"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::User).unwrap();
        assert!(cfgs.is_empty(), "timeout:0 must drop the remote server");
    }

    #[test]
    fn new_fields_do_not_change_server_key_hash() {
        // The per-server timeout/alwaysLoad live on McpServerConfig, NOT on the
        // McpTransportSpec, so `oauth::server_key` (which hashes only the spec)
        // is identical with or without them.
        let bare = r#"{"mcpServers":{"ordered":{"url":"https://mcp.example.com/v1","type":"http","headers":{"Z-Header":"z","A-Header":"a"}}}}"#;
        let with = r#"{"mcpServers":{"ordered":{"url":"https://mcp.example.com/v1","type":"http","headers":{"Z-Header":"z","A-Header":"a"},"timeout":12345,"alwaysLoad":true}}}"#;
        let a = parse_mcp_json_string(bare, ConfigScope::Project).unwrap();
        let b = parse_mcp_json_string(with, ConfigScope::Project).unwrap();
        assert_eq!(
            crate::oauth::server_key("ordered", &a[0].spec),
            crate::oauth::server_key("ordered", &b[0].spec),
            "timeout/alwaysLoad must not perturb the server-key config hash",
        );
        // ...and the fields were actually captured on the config.
        assert_eq!(b[0].timeout_ms, Some(12345));
        assert!(b[0].always_load);
    }

    // ── §23b: the `Iqe` shape/size guard wired into the loaders ───────────

    /// A valid-JSON `.mcp.json` (one "big" server) padded with trailing
    /// whitespace past `MCP_CONFIG_MAX_BYTES`. `serde_json` accepts trailing
    /// whitespace, so this is ONLY rejected by the shape/size guard — if that
    /// guard were removed, both loaders below would happily read and parse
    /// it, and "big" would appear in the result.
    fn oversized_but_well_formed_mcp_json() -> Vec<u8> {
        let mut body = br#"{"mcpServers":{"big":{"command":"c"}}}"#.to_vec();
        let target = crate::config_diagnostics::MCP_CONFIG_MAX_BYTES as usize + 1;
        body.resize(target, b' ');
        body
    }

    #[test]
    fn precedence_loader_skips_an_oversized_project_mcp_json() {
        let dir = TempDir::new().unwrap();
        let project = dir.path().join(".mcp.json");
        let global = dir.path().join("user-mcp.json"); // absent
        fs::write(&project, oversized_but_well_formed_mcp_json()).unwrap();

        let cfgs = load_mcp_json_with_precedence(&project, &global);
        assert!(
            cfgs.is_empty(),
            "an over-cap .mcp.json must be rejected by the shape/size guard, not parsed"
        );
    }

    #[test]
    fn load_mcp_servers_skips_an_oversized_project_mcp_json() {
        let dir = TempDir::new().unwrap();
        let project = dir.path().join(".mcp.json");
        let global = dir.path().join(".lingxi.json"); // absent
        fs::write(&project, oversized_but_well_formed_mcp_json()).unwrap();

        let cfgs = load_mcp_servers(&project, &global, dir.path());
        assert!(
            cfgs.is_empty(),
            "an over-cap .mcp.json must be rejected by the shape/size guard, not parsed"
        );
    }
}

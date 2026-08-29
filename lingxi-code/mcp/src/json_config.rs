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
    Ok(build_servers_from_map(entries, scope))
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
    Ok(build_servers_from_map(entries, scope))
}

/// Build validated `McpServerConfig`s from a `{ name: entry }` server map.
/// Shared by [`parse_mcp_json_string`] (bare-map fallback) and
/// [`parse_global_config_mcp_servers`] (mcpServers-only). Invalid entries are
/// logged + skipped (keeping valid siblings); the result is sorted by name.
fn build_servers_from_map(
    entries: &serde_json::Map<String, serde_json::Value>,
    scope: ConfigScope,
) -> Vec<McpServerConfig> {
    let mut out = Vec::new();
    for (name, raw_entry) in entries {
        if let Some(cfg) = build_server_from_json_entry(name, raw_entry, scope) {
            out.push(cfg);
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Quiet shape check for one raw `.mcp.json`-style entry value: does it
/// deserialize as a server entry AND carry a transport (`command` or `url`)?
/// Mirrors the zod `safeParse` success predicate WITHOUT logging — used by the
/// strict JSON-agent `mcpServers` validator (`z.array(AgentMcpServerSpecSchema)`,
/// where any invalid record value drops the whole agent) so validation at parse
/// time does not double-log the entry the conversion would log again later.
#[must_use]
pub fn server_entry_shape_is_valid(raw_entry: &serde_json::Value) -> bool {
    McpJsonEntry::deserialize(raw_entry)
        .map(|e| {
            // `type:"sdk"` (oracle `MAn`) needs neither `command` nor `url` —
            // see [`build_server_from_json_entry`].
            e.command.is_some() || e.url.is_some() || e.transport_type.as_deref() == Some("sdk")
        })
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
        // Discriminate by `type` FIRST, mirroring the oracle's
        // `z.discriminatedUnion("type", [stdio, sse, sse-ide, ws-ide, http,
        // ws, sdk, claudeai-proxy])` (`KY`, 2.1.251 Mach-O @154583724). An
        // absent `type` is a VALID "stdio" discriminator — zod's
        // `getDiscriminator` unwraps `ZodOptional` so `fYe`'s
        // `type: N("stdio").optional()` registers `[undefined, "stdio"]` —
        // so a `type`-less entry is validated as stdio and ONLY as stdio,
        // never guessed from whichever other fields happen to be present.
        // A `type` value matching none of the union's literals fails the
        // whole entry. This fixes §12's shape-driven bugs: a bare
        // `{"url":...}` (no `type`, no `command`) used to fall through to
        // Http; `{"type":"http","command":"x"}` used to be parsed as Stdio
        // because the old code checked `command` before `type`;
        // `{"type":"bogus",...}` and the non-oracle alias `"websocket"` used
        // to silently default to Http/WebSocket instead of being rejected.
        let ty = entry.transport_type.as_deref();
        let is_stdio = matches!(ty, None | Some("stdio"));
        let spec = if ty == Some("sdk") {
            // Oracle `MAn` @154585319 (v2.1.251 Mach-O, minified `mcp-sdk.js`
            // chunk): `f({type:N("sdk"),name:i(),timeout:o().optional(),
            // alwaysLoad:q().optional()})` — no `command`, no `url`.
            //
            // The schema's own `name` is always the entry's map key — every
            // construction site in the oracle stamps it that way
            // (`d[S]={type:"sdk",name:S,...}` @174336969,
            // `P[Fe]={type:"sdk",name:Fe,...}` @176568709) — so the
            // control-channel id is this server's NAME, never a URL (a stray
            // `url` field on an sdk entry is schema-unknown and would be
            // silently stripped by zod, so it is likewise ignored here).
            McpTransportSpec::SdkControl {
                control_channel_id: name.clone(),
            }
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
            // Every remaining oracle-recognized `type` is a URL transport.
            // `sse-ide`/`ws-ide` are deliberately included here even though
            // the oracle requires `ideName` on both (§10 — a DIFFERENT,
            // unassigned finding): they are preserved dialling as Http
            // exactly as before, rather than newly rejected by this change.
            let recognized = matches!(
                ty,
                Some("sse" | "http" | "streamable-http" | "ws" | "claudeai-proxy" | "sse-ide" | "ws-ide")
            );
            if !recognized {
                // A `type` string outside the oracle's 8-member union (e.g.
                // `"bogus"`, or the non-oracle alias `"websocket"`) fails the
                // whole entry — it must NOT silently default to Http/WebSocket.
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
                Some("claudeai-proxy") => McpTransportSpec::Http {
                    url,
                    headers,
                    headers_helper,
                    oauth: entry.oauth,
                },
                // §10 (unmodelled ideName/authToken) — preserved as before.
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
        // transports only. `McpTransportSpec::Http` here may also carry a
        // claudeai-proxy or sse-ide/ws-ide entry (out of scope, §10/claude.ai
        // surface); those already produced `Http` before this change, so
        // folding the alias for them too is pre-existing behaviour, not a
        // new divergence introduced by this fix.
        let timeout_ms = match &spec {
            McpTransportSpec::Sse { .. } | McpTransportSpec::Http { .. } => {
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

    // Project overrides on name collision.
    if let Ok(raw) = std::fs::read_to_string(project_path) {
        match parse_mcp_json_string(&raw, ConfigScope::Project) {
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
        }
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
    Ok(build_servers_from_map(entries, scope))
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

    // PROJECT (middle): `<cwd>/.mcp.json` (bare-map fallback allowed).
    if let Ok(raw) = std::fs::read_to_string(project_mcp_path) {
        match parse_mcp_json_string(&raw, ConfigScope::Project) {
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
        }
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
        // remote schemas are only reached via the `type` discriminator, never
        // inferred from `url` alone (see [`bare_url_without_type_is_rejected`]
        // for the case this replaces).
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
    /// `command`) was silently accepted as Http. The oracle's discriminated
    /// union treats an absent `type` as an IMPLICIT `stdio` attempt (zod's
    /// `getDiscriminator` on `fYe`'s `type: N("stdio").optional()` registers
    /// `[undefined, "stdio"]`), which then fails `command.min(1)` because
    /// there is no `command` — the whole entry is skipped, not routed to Http.
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
    /// oracle's discriminated union has no arm for an unrecognized `type`
    /// string, so it does NOT fall back to Http just because a `url` happens
    /// to be present.
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
    /// `command`. Before this fix the entry fell through to the
    /// "missing both command and url" branch and was silently dropped, even
    /// though `KNOWN_MCP_TYPES` (config_diagnostics.rs) already blessed `sdk`
    /// as a recognized type. The control-channel id must come from the
    /// server's own NAME (the map key), never a url — sdk entries have none.
    #[test]
    fn sdk_entry_with_no_url_is_accepted_and_keyed_by_name() {
        let raw = r#"{
          "mcpServers": {
            "claude-vscode": { "type": "sdk", "timeout": 5000, "alwaysLoad": true }
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

    /// A stray `url` on an sdk entry is schema-unknown (oracle `MAn` has no
    /// `url` field) and must be ignored, not used as the control-channel id.
    #[test]
    fn sdk_entry_ignores_a_stray_url_field() {
        let raw = r#"{"mcpServers":{"srv":{"type":"sdk","url":"should-be-ignored"}}}"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::User).unwrap();
        assert_eq!(cfgs.len(), 1);
        match &cfgs[0].spec {
            McpTransportSpec::SdkControl { control_channel_id } => {
                assert_eq!(control_channel_id, "srv", "must key off name, not url");
            }
            other => panic!("expected SdkControl, got {other:?}"),
        }
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
        // §12: only the oracle's literal `"ws"` is a valid ws discriminator
        // (`LAn`, 2.1.251 Mach-O @154583724) — the non-oracle alias
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

    /// §12 fix: `"websocket"` is not one of the oracle's 8 discriminated-union
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
}

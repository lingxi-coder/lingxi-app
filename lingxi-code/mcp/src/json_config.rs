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
use traits::{McpHeaders, McpTransportSpec};

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
fn expand_map_values(map: HashMap<String, String>, missing: &mut Vec<String>) -> HashMap<String, String> {
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
    #[serde(default)]
    disabled: bool,
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

    let mut out = Vec::new();
    for (name, raw_entry) in entries {
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
                continue;
            }
        };
        let name = name.clone();
        // Expand `${VAR}` / `${VAR:-default}` references in the transport
        // fields, mirroring claude-code `expandEnvVars(config)`
        // (`services/mcp/config.ts:556-615`): stdio expands `command`/`args`/
        // `env` VALUES; remote expands `url`/`headers` VALUES. Missing-variable
        // names (deduped, as TS `[...new Set(missingVars)]`) are logged but do
        // NOT fail the parse — the literal `${VAR}` is left in place (TS surfaces
        // a non-fatal config error; a `warn!` is the closest non-breaking
        // analogue for this parser).
        let mut missing: Vec<String> = Vec::new();
        let spec = if let Some(cmd) = entry.command {
            McpTransportSpec::Stdio {
                command: expand_field(&cmd, &mut missing),
                args: entry
                    .args
                    .into_iter()
                    .map(|a| expand_field(&a, &mut missing))
                    .collect(),
                env: expand_map_values(entry.env, &mut missing),
            }
        } else if let Some(url) = entry.url {
            let url = expand_field(&url, &mut missing);
            let headers = expand_header_values(entry.headers, &mut missing);
            match entry.transport_type.as_deref() {
                Some("sse") => McpTransportSpec::Sse {
                    url,
                    headers,
                    headers_helper: None,
                    oauth: None,
                },
                _ => McpTransportSpec::Http {
                    url,
                    headers,
                    oauth: None,
                },
            }
        } else {
            // Entry has neither `command` nor `url`: invalid transport. TS
            // `safeParse` rejects it → log + skip, keeping valid siblings.
            tracing::warn!(
                server = %name,
                "mcp.json: entry missing both command and url; skipping"
            );
            continue;
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
        out.push(McpServerConfig {
            name,
            spec,
            scope,
            disabled: entry.disabled,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Load + merge `.mcp.json` configs from the project path (cwd) and the
/// user-global path. Project entries take precedence on name collision.
///
/// Missing files yield empty lists. Parse errors are logged via
/// `tracing::warn!` and the corresponding file is skipped — a malformed
/// `.mcp.json` must not break startup.
#[must_use]
pub fn load_mcp_json_with_precedence(
    project_path: &Path,
    global_path: &Path,
) -> Vec<McpServerConfig> {
    let mut by_name: HashMap<String, McpServerConfig> = HashMap::new();

    // User-global first (lower precedence).
    if let Ok(raw) = std::fs::read_to_string(global_path) {
        match parse_mcp_json_string(&raw, ConfigScope::User) {
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
        let raw = r#"{
          "mcpServers": {
            "remote": { "url": "https://example.test/mcp" }
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

    #[test]
    fn missing_command_and_url_is_skipped_not_error() {
        // claude-code `loadMcpServersFromFile` logs+skips an invalid entry
        // (safeParse failure → continue) instead of dropping the file. A lone
        // bad entry therefore yields an empty list, NOT an Err.
        let raw = r#"{"mcpServers":{"bogus":{}}}"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::Project).unwrap();
        assert!(cfgs.is_empty(), "bad entry skipped, no Err");
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
        let raw =
            r#"{"mcpServers":{"s":{"command":"${LINGXI_MCP_TEST_UNSET_5B}","args":["ok"]}}}"#;
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
}

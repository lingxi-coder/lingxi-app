//! MCP config-load diagnostics — a byte-faithful port of claude-code 2.1.206's
//! `F7t` per-entry warnings (2.1.220's `klr`), including the
//! leading/trailing-whitespace scan (`ty_`) added in 2.1.219.
//!
//! `F7t` validates one config source's `mcpServers` and, for every entry it
//! drops, records a user-facing warning. The port's loader
//! ([`crate::json_config`]) already drops invalid entries (via `tracing::warn!`);
//! this module is an ADDITIVE pass that reproduces `F7t`'s exact warning
//! strings so they can be surfaced at startup and in `/doctor`, without
//! touching the loader.
//!
//! One string is not byte-reproducible: the `<issues>` detail inside
//! `Skipped — invalid MCP server config for "<name>": <issues>` comes from
//! Zod's validator; the port emits the exact message *shell* with a best-effort
//! reason. Every other `F7t` warning is byte-exact.

use crate::connection::ConfigScope;
use crate::normalization::is_reserved_mcp_server_name;
use serde_json::Value;

/// claude `mcpErrorMetadata.severity`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpConfigSeverity {
    /// A whole-source shape error (`F7t`'s top-level `!i.success`).
    Fatal,
    /// A per-entry skip (`F7t`'s `l(...)` warnings).
    Warning,
}

/// One MCP config diagnostic — claude `F7t`'s error/warning records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpConfigWarning {
    /// Originating config file, when known (claude `...o&&{file:o}`).
    pub file: Option<String>,
    /// Dotted path (`mcpServers.<name>` for per-entry, empty for shape errors).
    pub path: String,
    /// User-facing message (byte-exact vs `F7t`, except the invalid-config
    /// `<issues>` detail).
    pub message: String,
    /// Optional remediation hint (claude `...d&&{suggestion:d}`).
    pub suggestion: Option<String>,
    /// Config scope this source was loaded at.
    pub scope: ConfigScope,
    /// The offending server name, for per-entry warnings.
    pub server_name: Option<String>,
    /// Fatal (shape) vs warning (per-entry skip).
    pub severity: McpConfigSeverity,
}

impl McpConfigWarning {
    /// Render as a single stderr line, mirroring how claude logs these
    /// (`message` then a ` (<suggestion>)` tail when present).
    #[must_use]
    pub fn to_stderr_line(&self) -> String {
        match &self.suggestion {
            Some(s) => format!("{} ({s})", self.message),
            None => self.message.clone(),
        }
    }
}

/// The MCP server `type` values claude recognizes (`Alu`'s keys). An entry with
/// any other `type` string triggers the unknown-type warning.
const KNOWN_MCP_TYPES: &[&str] = &[
    "stdio",
    "sse",
    "http",
    "streamable-http",
    "ws",
    "sdk",
    "claudeai-proxy",
];

/// claude's byte-exact suggestion for the unknown-type warning.
const VALID_TYPES_SUGGESTION: &str =
    "Valid types are: stdio, sse, http (or streamable-http), ws, sdk";

/// Collect the `F7t` diagnostics for ONE config source (`configObject` = the
/// parsed file / config object holding `mcpServers`). `file` is the source path
/// for the `file:` field (and the `servers`-typo suggestion).
#[must_use]
pub fn collect_mcp_config_warnings(
    config: &Value,
    scope: ConfigScope,
    file: Option<&str>,
) -> Vec<McpConfigWarning> {
    // Top-level shape: `mcpServers` must be an object. The one byte-reproducible
    // shape error is the `"servers"` typo (claude's special-cased message); other
    // shape failures surface a Zod message we do not reproduce, so we stay quiet
    // there (the port also tolerates a bare map on some sources).
    let Some(Value::Object(servers)) = config.get("mcpServers") else {
        if config.is_object()
            && config.get("servers").is_some()
            && config.get("mcpServers").is_none()
        {
            return vec![McpConfigWarning {
                file: file.map(str::to_string),
                path: String::new(),
                message: "Missing \"mcpServers\" \u{2014} found \"servers\" instead. Claude Code reads MCP servers from the \"mcpServers\" key.".to_string(),
                suggestion: Some(format!(
                    "Rename the top-level \"servers\" key to \"mcpServers\" in {}",
                    file.unwrap_or("your MCP config")
                )),
                scope,
                server_name: None,
                severity: McpConfigSeverity::Fatal,
            }];
        }
        return Vec::new();
    };

    let mut out = Vec::new();
    let mut warn = |name: &str, message: String, suggestion: Option<String>| {
        out.push(McpConfigWarning {
            file: file.map(str::to_string),
            path: format!("mcpServers.{name}"),
            message,
            suggestion,
            scope,
            server_name: Some(name.to_string()),
            severity: McpConfigSeverity::Warning,
        });
    };

    for (name, entry) in servers {
        // `type` = the string `type`, else "stdio" (claude `typeof u.type==="string"?u.type:"stdio"`).
        let ty = entry.get("type").and_then(Value::as_str).unwrap_or("stdio");
        if !KNOWN_MCP_TYPES.contains(&ty) {
            warn(
                name,
                format!("Skipped \u{2014} unknown MCP server type \"{ty}\" for server \"{name}\""),
                Some(VALID_TYPES_SUGGESTION.to_string()),
            );
            continue;
        }
        if !entry_valid_for_type(entry, ty) {
            // The specific "url but no type" case (claude's dedicated branch).
            if entry.is_object()
                && entry.get("type").is_none()
                && entry.get("url").is_some()
                && entry.get("command").is_none()
            {
                warn(
                    name,
                    format!("Skipped \u{2014} MCP server \"{name}\" has a \"url\" but no \"type\"; add \"type\": \"http\" (or \"sse\" / \"ws\") to this entry"),
                    None,
                );
                continue;
            }
            warn(
                name,
                format!(
                    "Skipped \u{2014} invalid MCP server config for \"{name}\": {}",
                    invalid_reason(entry, ty)
                ),
                None,
            );
            continue;
        }
        // Reserved name (unless SDK) — claude `TEt(c) && m.type!=="sdk"`.
        if is_reserved_mcp_server_name(name) && ty != "sdk" {
            warn(
                name,
                format!("\"{name}\" is a reserved MCP server name and was not loaded"),
                Some(format!(
                    "Rename this server in your MCP config \u{2014} \"{name}\" is reserved for internal use"
                )),
            );
            continue;
        }
        // Leading/trailing whitespace (claude `ty_`, new in 2.1.219): the entry
        // is still LOADED — the warning flags values that are "used exactly as
        // written". Runs before the missing-env expansion pass, matching
        // `klr`'s `let y=ty_(g)` placement. Both warnings can fire for one
        // entry.
        let ws_fields = collect_whitespace_fields(entry, ty);
        if !ws_fields.is_empty() {
            warn(
                name,
                format!(
                    "Leading or trailing whitespace in: {}",
                    ws_fields.join(", ")
                ),
                Some(format!(
                    "Remove the whitespace from these values in the \"{name}\" entry \u{2014} they are used exactly as written"
                )),
            );
        }
        // Missing env vars (claude `Osg` — only stdio/sse/http/ws expand).
        let missing = collect_missing_env_vars(entry, ty);
        if !missing.is_empty() {
            let joined = missing.join(", ");
            warn(
                name,
                format!("Missing environment variables: {joined}"),
                Some(format!("Set the following environment variables: {joined}")),
            );
        }
    }
    out
}

/// Loader validity per type (aligned with [`crate::json_config`]): stdio needs
/// a `command`, every remote type needs a `url`.
fn entry_valid_for_type(entry: &Value, ty: &str) -> bool {
    let has_command = entry
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(|s| !s.trim().is_empty());
    let has_url = entry
        .get("url")
        .and_then(Value::as_str)
        .is_some_and(|s| !s.trim().is_empty());
    match ty {
        "stdio" => has_command,
        _ => has_url,
    }
}

/// Best-effort `<issues>` reason for the invalid-config warning (the one
/// non-byte-reproducible detail — Zod's validator output).
fn invalid_reason(entry: &Value, ty: &str) -> String {
    let missing_command = entry
        .get("command")
        .and_then(Value::as_str)
        .map_or(true, |s| s.trim().is_empty());
    let missing_url = entry
        .get("url")
        .and_then(Value::as_str)
        .map_or(true, |s| s.trim().is_empty());
    if ty == "stdio" && missing_command {
        "command: Required".to_string()
    } else if missing_url {
        "url: Required".to_string()
    } else {
        "invalid entry".to_string()
    }
}

/// claude `ty_` (2.1.219) — config fields whose value carries leading or
/// trailing whitespace (`value !== value.trim()`). Field labels are byte-exact:
/// `command`, `url`, `args[<i>]`, `env.<key>` / `headers.<key>` for values, and
/// `env name <json>` / `header name <json>` (the key JSON-quoted via `Ie` =
/// `JSON.stringify`) when the KEY itself has whitespace — the value of such a
/// key is still checked under its raw `env.<key>` label.
///
/// `ty_` runs on the schema-VALIDATED entry, whose zod parse strips fields the
/// transport type does not declare; the port mirrors that by gating on the
/// type family (stdio → command/args/env, everything else → url/headers).
fn collect_whitespace_fields(entry: &Value, ty: &str) -> Vec<String> {
    let mut fields: Vec<String> = Vec::new();
    fn check(label: String, value: &str, fields: &mut Vec<String>) {
        if value != value.trim() {
            fields.push(label);
        }
    }
    // `Ie` = JSON.stringify (string serialization cannot fail).
    fn json_quote(s: &str) -> String {
        serde_json::to_string(s).unwrap_or_else(|_| format!("\"{s}\""))
    }
    match ty {
        "stdio" => {
            if let Some(c) = entry.get("command").and_then(Value::as_str) {
                check("command".to_string(), c, &mut fields);
            }
            if let Some(args) = entry.get("args").and_then(Value::as_array) {
                for (i, a) in args.iter().enumerate() {
                    if let Some(s) = a.as_str() {
                        check(format!("args[{i}]"), s, &mut fields);
                    }
                }
            }
            if let Some(env) = entry.get("env").and_then(Value::as_object) {
                for (k, v) in env {
                    if k != k.trim() {
                        fields.push(format!("env name {}", json_quote(k)));
                    }
                    if let Some(s) = v.as_str() {
                        check(format!("env.{k}"), s, &mut fields);
                    }
                }
            }
        }
        _ => {
            if let Some(u) = entry.get("url").and_then(Value::as_str) {
                check("url".to_string(), u, &mut fields);
            }
            if let Some(h) = entry.get("headers").and_then(Value::as_object) {
                for (k, v) in h {
                    if k != k.trim() {
                        fields.push(format!("header name {}", json_quote(k)));
                    }
                    if let Some(s) = v.as_str() {
                        check(format!("headers.{k}"), s, &mut fields);
                    }
                }
            }
        }
    }
    fields
}

/// claude `Osg` — the env-var references left unresolved after expanding the
/// fields Osg expands (stdio: command/args/env values; sse/http/ws: url/headers
/// values; other types expand nothing). Deduped, first-seen order (`Fo`).
fn collect_missing_env_vars(entry: &Value, ty: &str) -> Vec<String> {
    let mut all: Vec<String> = Vec::new();
    let push = |s: &str, all: &mut Vec<String>| {
        all.extend(crate::env_expansion::expand_env_vars_in_string(s).missing_vars);
    };
    match ty {
        "stdio" => {
            if let Some(c) = entry.get("command").and_then(Value::as_str) {
                push(c, &mut all);
            }
            if let Some(args) = entry.get("args").and_then(Value::as_array) {
                for a in args {
                    if let Some(s) = a.as_str() {
                        push(s, &mut all);
                    }
                }
            }
            if let Some(env) = entry.get("env").and_then(Value::as_object) {
                for v in env.values() {
                    if let Some(s) = v.as_str() {
                        push(s, &mut all);
                    }
                }
            }
        }
        "sse" | "http" | "ws" => {
            if let Some(u) = entry.get("url").and_then(Value::as_str) {
                push(u, &mut all);
            }
            if let Some(h) = entry.get("headers").and_then(Value::as_object) {
                for v in h.values() {
                    if let Some(s) = v.as_str() {
                        push(s, &mut all);
                    }
                }
            }
        }
        // sdk / claudeai-proxy / streamable-http / ide → Osg expands nothing.
        _ => {}
    }
    // `Fo` — dedup preserving first-seen order.
    let mut seen = std::collections::HashSet::new();
    all.into_iter().filter(|v| seen.insert(v.clone())).collect()
}

/// Collect `F7t` diagnostics across every file-based MCP config source the
/// loader reads — the project `.mcp.json` (at `cwd`), and the user + local
/// `mcpServers` in the global config file — so startup and `/doctor` can report
/// them. Silent (empty) when configs are clean, so a healthy setup prints
/// nothing.
#[must_use]
pub fn collect_all_mcp_config_warnings(
    cwd: &std::path::Path,
    global_config_path: Option<&std::path::Path>,
) -> Vec<McpConfigWarning> {
    collect_all_mcp_config_warnings_at(&cwd.join(".mcp.json"), cwd, global_config_path)
}

/// [`collect_all_mcp_config_warnings`] with an EXPLICIT project `.mcp.json`
/// path, for callers whose loader discovers the project file by walking up
/// from `cwd` (the oracle resolves the project config at the workspace root,
/// not literally `<cwd>/.mcp.json`). `cwd` still keys the local scope.
#[must_use]
pub fn collect_all_mcp_config_warnings_at(
    project_mcp_path: &std::path::Path,
    cwd: &std::path::Path,
    global_config_path: Option<&std::path::Path>,
) -> Vec<McpConfigWarning> {
    let mut out = Vec::new();
    let read_json = |p: &std::path::Path| -> Option<Value> {
        serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()
    };

    // Project scope: the discovered `.mcp.json`.
    let project = project_mcp_path;
    if let Some(v) = read_json(project) {
        out.extend(collect_mcp_config_warnings(
            &v,
            ConfigScope::Project,
            Some(&project.to_string_lossy()),
        ));
    }

    // User + Local scope: the global config file's top-level `mcpServers`
    // (user) and `projects.<cwd-key>.mcpServers` (local).
    if let Some(gp) = global_config_path {
        if let Some(v) = read_json(gp) {
            let file = gp.to_string_lossy();
            out.extend(collect_mcp_config_warnings(
                &v,
                ConfigScope::User,
                Some(&file),
            ));
            let key = migrations::global_config::project_path_for_config(cwd);
            if let Some(proj) = v.get("projects").and_then(|p| p.get(&key)) {
                out.extend(collect_mcp_config_warnings(
                    proj,
                    ConfigScope::Local,
                    Some(&file),
                ));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn only(config: &Value) -> Vec<McpConfigWarning> {
        collect_mcp_config_warnings(config, ConfigScope::Project, Some("/p/.mcp.json"))
    }

    #[test]
    fn unknown_type_warning_is_byte_exact() {
        let c = json!({"mcpServers":{"srv":{"type":"grpc","url":"x"}}});
        let w = only(&c);
        assert_eq!(w.len(), 1);
        assert_eq!(
            w[0].message,
            "Skipped \u{2014} unknown MCP server type \"grpc\" for server \"srv\""
        );
        assert_eq!(w[0].suggestion.as_deref(), Some(VALID_TYPES_SUGGESTION));
        assert_eq!(w[0].path, "mcpServers.srv");
        assert_eq!(w[0].server_name.as_deref(), Some("srv"));
    }

    #[test]
    fn url_without_type_warning_is_byte_exact() {
        let c = json!({"mcpServers":{"remote":{"url":"https://x"}}});
        let w = only(&c);
        assert_eq!(
            w[0].message,
            "Skipped \u{2014} MCP server \"remote\" has a \"url\" but no \"type\"; add \"type\": \"http\" (or \"sse\" / \"ws\") to this entry"
        );
        assert!(w[0].suggestion.is_none());
    }

    #[test]
    fn invalid_config_shell_is_byte_exact() {
        // stdio (explicit) with no command → invalid.
        let c = json!({"mcpServers":{"bad":{"type":"stdio"}}});
        let w = only(&c);
        assert!(w[0]
            .message
            .starts_with("Skipped \u{2014} invalid MCP server config for \"bad\": "));
    }

    #[test]
    fn empty_remote_url_is_reported_as_required() {
        let c = json!({"mcpServers":{"bad":{"type":"http","url":"   "}}});
        let w = only(&c);
        assert_eq!(w.len(), 1);
        assert_eq!(
            w[0].message,
            "Skipped \u{2014} invalid MCP server config for \"bad\": url: Required"
        );
    }

    #[test]
    fn reserved_name_warning_is_byte_exact() {
        let c = json!({"mcpServers":{"workspace":{"type":"stdio","command":"c"}}});
        let w = only(&c);
        assert_eq!(
            w[0].message,
            "\"workspace\" is a reserved MCP server name and was not loaded"
        );
        assert_eq!(
            w[0].suggestion.as_deref(),
            Some("Rename this server in your MCP config \u{2014} \"workspace\" is reserved for internal use")
        );
        // An SDK server with a reserved name is NOT flagged (claude `m.type!=="sdk"`).
        let c2 = json!({"mcpServers":{"workspace":{"type":"sdk","url":"chan"}}});
        assert!(only(&c2).is_empty());
    }

    #[test]
    fn missing_env_warning_is_byte_exact_and_deduped() {
        let c = json!({"mcpServers":{"srv":{"type":"stdio","command":"${TOK}","args":["${TOK}","${OTHER}"]}}});
        let w = only(&c);
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].message, "Missing environment variables: TOK, OTHER");
        assert_eq!(
            w[0].suggestion.as_deref(),
            Some("Set the following environment variables: TOK, OTHER")
        );
    }

    #[test]
    fn whitespace_warning_is_byte_exact_stdio() {
        // `ty_` (2.1.219): field labels `command`, `args[<i>]`,
        // `env name <json>` for a whitespace-carrying KEY (JSON-quoted via
        // `Ie` = JSON.stringify), `env.<key>` (raw key) for its value.
        let c = json!({"mcpServers":{"srv":{
            "type":"stdio",
            "command":"cmd ",
            "args":[" a","b"],
            "env":{" K ":"v ","OK":"clean"}
        }}});
        let w = only(&c);
        assert_eq!(w.len(), 1);
        assert_eq!(
            w[0].message,
            "Leading or trailing whitespace in: command, args[0], env name \" K \", env. K "
        );
        assert_eq!(
            w[0].suggestion.as_deref(),
            Some(
                "Remove the whitespace from these values in the \"srv\" entry \u{2014} they are used exactly as written"
            )
        );
        assert_eq!(w[0].severity, McpConfigSeverity::Warning);
    }

    #[test]
    fn whitespace_warning_is_byte_exact_remote() {
        let c = json!({"mcpServers":{"web":{
            "type":"http",
            "url":"https://x.test ",
            "headers":{"X-A ":"v","X-B":" v"}
        }}});
        let w = only(&c);
        assert_eq!(w.len(), 1);
        assert_eq!(
            w[0].message,
            "Leading or trailing whitespace in: url, header name \"X-A \", headers.X-B"
        );
    }

    #[test]
    fn whitespace_and_missing_env_both_fire_for_one_entry() {
        // `klr` runs `ty_` BEFORE the expansion pass; both warnings can fire.
        let c = json!({"mcpServers":{"srv":{
            "type":"stdio",
            "command":"${LINGXI_DIAG_UNSET_M5} "
        }}});
        let w = only(&c);
        assert_eq!(w.len(), 2);
        assert_eq!(w[0].message, "Leading or trailing whitespace in: command");
        assert_eq!(
            w[1].message,
            "Missing environment variables: LINGXI_DIAG_UNSET_M5"
        );
    }

    #[test]
    fn skipped_entries_get_no_whitespace_warning() {
        // A reserved-name (or otherwise skipped) entry never reaches `ty_` —
        // `klr` `continue`s before the whitespace scan.
        let c = json!({"mcpServers":{"workspace":{"type":"stdio","command":"c "}}});
        let w = only(&c);
        assert_eq!(w.len(), 1);
        assert!(w[0].message.contains("reserved MCP server name"));
    }

    #[test]
    fn servers_typo_shape_error_is_byte_exact() {
        let c = json!({"servers":{"a":{"type":"stdio","command":"c"}}});
        let w = collect_mcp_config_warnings(&c, ConfigScope::User, Some("/u/config.json"));
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].severity, McpConfigSeverity::Fatal);
        assert_eq!(
            w[0].message,
            "Missing \"mcpServers\" \u{2014} found \"servers\" instead. Claude Code reads MCP servers from the \"mcpServers\" key."
        );
        assert_eq!(
            w[0].suggestion.as_deref(),
            Some("Rename the top-level \"servers\" key to \"mcpServers\" in /u/config.json")
        );
    }

    #[test]
    fn valid_entries_and_absent_mcpservers_produce_no_warnings() {
        assert!(
            only(&json!({"mcpServers":{"ok":{"type":"stdio","command":"c","args":["a"]}}}))
                .is_empty()
        );
        assert!(only(&json!({"mcpServers":{"web":{"type":"http","url":"https://x"}}})).is_empty());
        // No mcpServers and no "servers" typo → nothing to diagnose.
        assert!(only(&json!({"other":1})).is_empty());
    }

    #[test]
    fn stderr_line_appends_suggestion() {
        let c = json!({"mcpServers":{"srv":{"type":"grpc","url":"x"}}});
        let line = only(&c)[0].to_stderr_line();
        assert_eq!(
            line,
            "Skipped \u{2014} unknown MCP server type \"grpc\" for server \"srv\" (Valid types are: stdio, sse, http (or streamable-http), ws, sdk)"
        );
    }
}

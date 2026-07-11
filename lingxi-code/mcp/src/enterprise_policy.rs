//! Enterprise MCP policy — a byte-faithful port of claude-code 2.1.206's
//! managed-MCP gates.
//!
//! When an organization ships a managed MCP configuration, `claude mcp add`
//! runs three checks (in `TPe`/`addMcpServer`, after the reserved-name check
//! and before any scope write):
//!
//! 1. **`D1()`** — is enterprise MCP configuration *active*? If a managed
//!    `managed-mcp.json` exists and parses, the enterprise config has exclusive
//!    control and every add is refused.
//! 2. **`bPe(name, config)`** — is the server *explicitly denied* by policy?
//! 3. **`gPe(name, config)`** — is the server *allowed* by policy?
//!
//! This module ports the substrate. Stage 1 (this file) implements the managed
//! path resolution and `D1()`; the `bPe`/`gPe` allow/deny matchers land in a
//! follow-up so each step stays non-divergent (`D1` is checked first, so when
//! it fires the matchers never run).

use std::path::{Path, PathBuf};

/// Env override relocating the managed (policy) settings root — the same
/// variable the engine's `managed_settings_dir` honors, so a test (or an
/// unusual deployment) can point both at one directory. Unset in production.
pub const MANAGED_DIR_ENV: &str = "LINGXI_MANAGED_DIR";

/// The OS-specific managed settings root — claude-code's `XM()`
/// (`getManagedFilePath`), LingXi-branded (see [`branding`]). Honors
/// [`MANAGED_DIR_ENV`] first (test/relocation), else the hardcoded OS path.
#[must_use]
pub fn managed_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os(MANAGED_DIR_ENV).filter(|v| !v.is_empty()) {
        return PathBuf::from(dir);
    }
    if cfg!(target_os = "macos") {
        PathBuf::from(branding::MANAGED_DIR_MACOS)
    } else if cfg!(target_os = "windows") {
        PathBuf::from(branding::MANAGED_DIR_WINDOWS)
    } else {
        PathBuf::from(branding::MANAGED_DIR_UNIX)
    }
}

/// claude-code `qFr()` — the managed MCP config path,
/// `<managed_dir>/managed-mcp.json`.
#[must_use]
pub fn managed_mcp_config_path() -> PathBuf {
    managed_dir().join("managed-mcp.json")
}

/// claude-code `D1()` — is enterprise MCP configuration active?
///
/// `D1` memoizes `$7t({filePath: qFr(), scope:"enterprise"}).config !== null`:
/// the managed MCP config file is read and parsed, and the config is non-null
/// exactly when it is a regular file within the size limit holding a valid JSON
/// object. An absent, empty, oversized, or malformed file → not active.
#[must_use]
pub fn enterprise_mcp_active() -> bool {
    enterprise_mcp_active_at(&managed_mcp_config_path())
}

/// claude-code's byte-exact `mcp add` rejection when [`enterprise_mcp_active`].
pub const ENTERPRISE_EXCLUSIVE_CONTROL_MESSAGE: &str =
    "Cannot add MCP server: enterprise MCP configuration is active and has exclusive control over MCP servers";

/// The [`enterprise_mcp_active`] core, parameterized on the config path so it is
/// testable without touching the process-global managed dir / env.
#[must_use]
pub fn enterprise_mcp_active_at(path: &Path) -> bool {
    // claude's `$7t` gates on "regular file within size limit"; here a failed
    // read (ENOENT / not-a-file / unreadable) is the same not-active signal.
    let Ok(raw) = std::fs::read_to_string(path) else {
        return false;
    };
    if raw.trim().is_empty() {
        return false;
    }
    // `config !== null` ⟺ the file parsed to a JSON object. Malformed JSON or a
    // non-object top level yields a null config (not active).
    matches!(
        serde_json::from_str::<serde_json::Value>(&raw),
        Ok(serde_json::Value::Object(_))
    )
}

// ────────────────────────────────────────────────────────────────────────────
// Stage 2 — the `bPe` (deny) / `gPe` (allow) policy matchers.
//
// claude reads the allow/deny lists from the *effective merged* settings
// (`$n()`); the CLI `mcp add` has no effective-settings reader, so we read them
// from the managed policy tiers (`managed-settings.json` + `managed-settings.d`)
// — the canonical, enterprise home for `deniedMcpServers`/`allowedMcpServers`.
// (A deny/allow list placed in a *personal* user-tier settings file is not yet
// consulted here; that awaits a shared effective-settings reader.)
// ────────────────────────────────────────────────────────────────────────────

use serde::Deserialize;
use serde_json::Value;

/// One allow/deny matcher entry (claude's `allowedMcpServers` /
/// `deniedMcpServers` items). Any subset of the three keys may be present; the
/// shape predicates `a3t`/`ZCn`/`ewn` test which is set.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerMatcher {
    /// `a3t`: match by server name.
    #[serde(default)]
    pub server_name: Option<String>,
    /// `ZCn`: match by the exact `[command, ...args]` array (env-expanded).
    #[serde(default)]
    pub server_command: Option<Vec<String>>,
    /// `ewn`: match by URL (via the `UFr` glob).
    #[serde(default)]
    pub server_url: Option<String>,
}

/// The enterprise MCP allow/deny policy (`deniedMcpServers` / `allowedMcpServers`).
#[derive(Debug, Clone, Default)]
pub struct McpPolicy {
    /// claude `deniedMcpServers` (absent ⇒ nothing denied).
    pub denied: Option<Vec<McpServerMatcher>>,
    /// claude `allowedMcpServers` (absent ⇒ everything allowed).
    pub allowed: Option<Vec<McpServerMatcher>>,
}

/// claude's byte-exact `mcp add` rejection when a server is denied (`bPe`).
#[must_use]
pub fn denied_message(name: &str) -> String {
    format!("Cannot add MCP server \"{name}\": server is explicitly blocked by enterprise policy")
}

/// claude's byte-exact `mcp add` rejection when a server is not allowed (`gPe`).
#[must_use]
pub fn not_allowed_message(name: &str) -> String {
    format!("Cannot add MCP server \"{name}\": not allowed by enterprise policy")
}

/// Read the enterprise MCP policy from the managed settings tiers, folded in
/// ascending priority (`managed-settings.json` base, then `managed-settings.d/`
/// `*.json` sorted, later wins) — the same tier order as the engine's
/// `managed_settings_raw_tiers`.
#[must_use]
pub fn read_managed_mcp_policy() -> McpPolicy {
    read_managed_mcp_policy_in(&managed_dir())
}

/// [`read_managed_mcp_policy`] rooted at an explicit dir (testable).
#[must_use]
pub fn read_managed_mcp_policy_in(dir: &Path) -> McpPolicy {
    let mut merged = serde_json::Map::new();
    let mut fold = |raw: &str| {
        if let Ok(Value::Object(m)) = serde_json::from_str::<Value>(raw) {
            for (k, v) in m {
                merged.insert(k, v);
            }
        }
    };
    if let Ok(raw) = std::fs::read_to_string(dir.join("managed-settings.json")) {
        fold(&raw);
    }
    let drop_in = dir.join("managed-settings.d");
    if let Ok(rd) = std::fs::read_dir(&drop_in) {
        let mut names: Vec<std::ffi::OsString> = rd
            .flatten()
            .map(|e| e.file_name())
            .filter(|n| {
                let s = n.to_string_lossy();
                s.ends_with(".json") && !s.starts_with('.')
            })
            .collect();
        names.sort();
        for name in names {
            if let Ok(raw) = std::fs::read_to_string(drop_in.join(&name)) {
                fold(&raw);
            }
        }
    }
    let parse = |key: &str| -> Option<Vec<McpServerMatcher>> {
        merged
            .get(key)
            .and_then(|v| serde_json::from_value::<Vec<McpServerMatcher>>(v.clone()).ok())
    };
    McpPolicy {
        denied: parse("deniedMcpServers"),
        allowed: parse("allowedMcpServers"),
    }
}

/// claude `Get`/`Ove` — expand `${VAR}` / `${VAR:-default}` in a string.
fn get(s: &str) -> String {
    crate::env_expansion::expand_env_vars_in_string(s).expanded
}

/// claude `oVn` — the `[command, ...args]` array for a stdio config, or `None`
/// when the config is a non-stdio type (or has no command to match).
fn config_command(config: &Value) -> Option<Vec<String>> {
    if let Some(ty) = config.get("type").and_then(Value::as_str) {
        if ty != "stdio" {
            return None;
        }
    }
    let command = config.get("command").and_then(Value::as_str)?;
    let mut out = vec![command.to_string()];
    if let Some(args) = config.get("args").and_then(Value::as_array) {
        for a in args {
            if let Some(s) = a.as_str() {
                out.push(s.to_string());
            }
        }
    }
    Some(out)
}

/// claude `iVn` — the config's `url`, or `None`.
fn config_url(config: &Value) -> Option<&str> {
    config.get("url").and_then(Value::as_str)
}

/// claude `Ilu` — element-wise array equality.
fn arrays_eq(a: &[String], b: &[String]) -> bool {
    a == b
}

/// claude `bPe(name, config)` — is the server *explicitly denied* by policy?
#[must_use]
pub fn is_denied(name: &str, config: &Value, policy: &McpPolicy) -> bool {
    let Some(denied) = policy.denied.as_ref() else {
        return false;
    };
    for m in denied {
        if m.server_name.as_deref() == Some(name) {
            return true;
        }
    }
    if let Some(cmd) = config_command(config) {
        let i: Vec<String> = cmd.iter().map(|s| get(s)).collect();
        for m in denied {
            if let Some(sc) = m.server_command.as_ref() {
                let sc_e: Vec<String> = sc.iter().map(|s| get(s)).collect();
                if arrays_eq(&sc_e, &i) {
                    return true;
                }
            }
        }
    }
    if let Some(url) = config_url(config) {
        let iu = get(url);
        for m in denied {
            if let Some(su) = m.server_url.as_ref() {
                if url_matches(&iu, &get(su)) {
                    return true;
                }
            }
        }
    }
    false
}

/// claude `gPe(name, config)` — is the server *allowed* by policy?
#[must_use]
pub fn is_allowed(name: &str, config: &Value, policy: &McpPolicy) -> bool {
    if is_denied(name, config, policy) {
        return false;
    }
    let Some(allowed) = policy.allowed.as_ref() else {
        return true; // no allowlist ⇒ everything allowed
    };
    if allowed.is_empty() {
        return false; // empty allowlist ⇒ nothing allowed
    }
    let has_cmd = allowed.iter().any(|m| m.server_command.is_some());
    let has_url = allowed.iter().any(|m| m.server_url.is_some());
    let name_match =
        || allowed.iter().any(|m| m.server_name.as_deref() == Some(name));

    if let Some(cmd) = config_command(config) {
        if has_cmd {
            let a: Vec<String> = cmd.iter().map(|s| get(s)).collect();
            for m in allowed {
                if let Some(sc) = m.server_command.as_ref() {
                    let sc_e: Vec<String> = sc.iter().map(|s| get(s)).collect();
                    if arrays_eq(&sc_e, &a) {
                        return true;
                    }
                }
            }
            false
        } else {
            name_match()
        }
    } else if let Some(url) = config_url(config) {
        if has_url {
            let a = get(url);
            for m in allowed {
                if let Some(su) = m.server_url.as_ref() {
                    if url_matches(&a, &get(su)) {
                        return true;
                    }
                }
            }
            false
        } else {
            name_match()
        }
    } else {
        name_match()
    }
}

/// A fixed wildcard sentinel (claude's `L7t`, which is random per-process only
/// to avoid colliding with real URL content; a fixed lowercase-alnum string is
/// equally collision-free and deterministic, and parses as a URL scheme so the
/// protocol-wildcard branch works).
const WILDCARD_SENTINEL: &str = "zzwildcardsentinelzz";

/// Escape the regex metacharacters claude's `UFr` escapes — the JS class
/// `/[.+?^${}()|[\]\\]/g` — deliberately leaving `*` for the wildcard step.
fn escape_re(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(
            c,
            '.' | '+' | '?' | '^' | '$' | '{' | '}' | '(' | ')' | '|' | '[' | ']' | '\\'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Replace `:<sentinel>` with `:0` when it stands where a port would (followed
/// by `/`, `?`, `#`, or end-of-string) — the look-ahead-free equivalent of
/// claude's `n.replace(/:${L7t}(?=[/?#]|$)/, ":0")`.
fn replace_wildcard_port(n: &str) -> String {
    let needle = format!(":{WILDCARD_SENTINEL}");
    let mut out = String::with_capacity(n.len());
    let mut rest = n;
    while let Some(pos) = rest.find(&needle) {
        let after = &rest[pos + needle.len()..];
        out.push_str(&rest[..pos]);
        if after.is_empty() || after.starts_with(['/', '?', '#']) {
            out.push_str(":0");
        } else {
            out.push_str(&needle);
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

/// JS `url.protocol` (scheme + `:`).
fn js_protocol(u: &url::Url) -> String {
    format!("{}:", u.scheme())
}

/// JS `url.host` (hostname, plus `:port` only when the port is non-default).
fn js_host(u: &url::Url) -> String {
    match u.port() {
        Some(p) => format!("{}:{}", u.host_str().unwrap_or(""), p),
        None => u.host_str().unwrap_or("").to_string(),
    }
}

/// JS `url.port` ("" when default/absent, else the port string).
fn js_port(u: &url::Url) -> String {
    u.port().map(|p| p.to_string()).unwrap_or_default()
}

/// JS `url.search` (`?query` or "").
fn js_search(u: &url::Url) -> String {
    u.query().map(|q| format!("?{q}")).unwrap_or_default()
}

/// claude `UFr(url, pattern)` — glob-match a URL against a policy pattern.
/// A faithful port of the 2.1.206 matcher, mapping JS `URL` accessors onto the
/// `url` crate (protocol keeps its `:`, host folds in a non-default port, the
/// `L7t` sentinel stands in for `*` so `*://`, `host:*`, `*.host`, and path
/// globs all parse).
#[must_use]
pub fn url_matches(url_str: &str, pattern: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    let Ok(r) = url::Url::parse(url_str) else {
        return false;
    };
    let n = pattern.replace('*', WILDCARD_SENTINEL);
    let mut o = false; // "port is wildcard" flag
    let mut parsed = url::Url::parse(&n).ok();
    if parsed.is_none() {
        // A wildcard *port* (`host:*`) makes the sentinel an invalid port; retry
        // with `:0` (claude's `:${L7t}(?=[/?#]|$)` → `:0`). The `regex` crate has
        // no look-ahead, so the "followed by /?# or end" guard is done manually.
        let u = replace_wildcard_port(&n);
        if u != n {
            if let Ok(p) = url::Url::parse(&u) {
                parsed = Some(p);
                o = true;
            }
        }
    }
    let Some(i) = parsed else {
        // Pattern isn't a full URL → treat as a plain glob over the whole
        // reconstructed URL string. `*` → `[^/]*`; the ORIGINAL pattern's other
        // metachars are escaped.
        let reconstructed = format!(
            "{}//{}{}{}",
            js_protocol(&r),
            js_host(&r),
            r.path(),
            js_search(&r)
        );
        let d = escape_re(pattern).replace('*', "[^/]*");
        return regex::Regex::new(&format!("^{d}$"))
            .map(|re| re.is_match(&reconstructed))
            .unwrap_or(false);
    };

    // Protocol must match (or be the sentinel wildcard scheme).
    if js_protocol(&i) != format!("{WILDCARD_SENTINEL}:") && js_protocol(&i) != js_protocol(&r) {
        return false;
    }
    // Hostname (trailing dot stripped, lowercased) must match, `*` → `[^/]*`.
    let host = r
        .host_str()
        .unwrap_or("")
        .trim_end_matches('.')
        .to_lowercase();
    let pat_host = i
        .host_str()
        .unwrap_or("")
        .trim_end_matches('.')
        .to_lowercase()
        .replace(WILDCARD_SENTINEL, "*");
    let host_re = escape_re(&pat_host).replace('*', "[^/]*");
    if !regex::Regex::new(&format!("^{host_re}$"))
        .map(|re| re.is_match(&host))
        .unwrap_or(false)
    {
        return false;
    }
    // A wildcard in the hostname with an empty pattern port ⇒ port wildcard too.
    if js_port(&i).is_empty() && i.host_str().unwrap_or("").contains(WILDCARD_SENTINEL) {
        o = true;
    }
    if !o && js_port(&i) != js_port(&r) {
        return false;
    }
    // No meaningful path/search in the pattern (and it didn't end in `/`) ⇒ match.
    let ipath = i.path();
    if (ipath == "/" || ipath.is_empty()) && js_search(&i).is_empty() && !n.ends_with('/') {
        return true;
    }
    // Otherwise the path+search must match, `*` → `.*`.
    let pat_path = format!("{}{}", i.path(), js_search(&i)).replace(WILDCARD_SENTINEL, "*");
    let path_re = escape_re(&pat_path).replace('*', ".*");
    regex::Regex::new(&format!("^{path_re}$"))
        .map(|re| re.is_match(&format!("{}{}", r.path(), js_search(&r))))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &tempfile::TempDir, name: &str, body: &str) -> PathBuf {
        let p = dir.path().join(name);
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn absent_file_is_not_active() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!enterprise_mcp_active_at(&dir.path().join("managed-mcp.json")));
    }

    #[test]
    fn valid_json_object_is_active() {
        let dir = tempfile::tempdir().unwrap();
        // Any valid JSON object → config non-null → active (matches `$7t`,
        // which does not require an `mcpServers` key at this gate).
        let p = write(&dir, "managed-mcp.json", r#"{"mcpServers":{"corp":{"type":"stdio","command":"c"}}}"#);
        assert!(enterprise_mcp_active_at(&p));
        let p2 = write(&dir, "empty-obj.json", "{}");
        assert!(enterprise_mcp_active_at(&p2));
    }

    #[test]
    fn empty_or_malformed_or_non_object_is_not_active() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!enterprise_mcp_active_at(&write(&dir, "empty.json", "")));
        assert!(!enterprise_mcp_active_at(&write(&dir, "ws.json", "   \n ")));
        assert!(!enterprise_mcp_active_at(&write(&dir, "bad.json", "{not json")));
        // A valid JSON value that is not an object → null config.
        assert!(!enterprise_mcp_active_at(&write(&dir, "arr.json", "[1,2,3]")));
        assert!(!enterprise_mcp_active_at(&write(&dir, "str.json", "\"hi\"")));
    }

    #[test]
    fn message_is_byte_exact() {
        assert_eq!(
            ENTERPRISE_EXCLUSIVE_CONTROL_MESSAGE,
            "Cannot add MCP server: enterprise MCP configuration is active and has exclusive control over MCP servers"
        );
    }

    #[test]
    fn managed_mcp_path_is_managed_dir_plus_filename() {
        // Isolate from any real managed dir via the env override.
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: single-threaded test; restored immediately after.
        let prev = std::env::var_os(MANAGED_DIR_ENV);
        std::env::set_var(MANAGED_DIR_ENV, dir.path());
        assert_eq!(managed_mcp_config_path(), dir.path().join("managed-mcp.json"));
        match prev {
            Some(v) => std::env::set_var(MANAGED_DIR_ENV, v),
            None => std::env::remove_var(MANAGED_DIR_ENV),
        }
    }

    // ── bPe / gPe matcher tests ──

    fn matcher(json: &str) -> Vec<McpServerMatcher> {
        serde_json::from_str(json).unwrap()
    }
    fn stdio(cmd: &str, args: &[&str]) -> Value {
        serde_json::json!({"type":"stdio","command":cmd,"args":args})
    }
    fn http(u: &str) -> Value {
        serde_json::json!({"type":"http","url":u})
    }

    #[test]
    fn deny_by_name_command_url() {
        // name
        let p = McpPolicy {
            denied: Some(matcher(r#"[{"serverName":"corp"}]"#)),
            allowed: None,
        };
        assert!(is_denied("corp", &stdio("c", &[]), &p));
        assert!(!is_denied("other", &stdio("c", &[]), &p));
        // command (exact [command, ...args])
        let p = McpPolicy {
            denied: Some(matcher(r#"[{"serverCommand":["npx","-y","evil"]}]"#)),
            allowed: None,
        };
        assert!(is_denied("x", &stdio("npx", &["-y", "evil"]), &p));
        assert!(!is_denied("x", &stdio("npx", &["-y", "good"]), &p));
        // url (UFr)
        let p = McpPolicy {
            denied: Some(matcher(r#"[{"serverUrl":"https://evil.example.com"}]"#)),
            allowed: None,
        };
        assert!(is_denied("x", &http("https://evil.example.com/mcp"), &p));
        assert!(!is_denied("x", &http("https://ok.example.com/mcp"), &p));
    }

    #[test]
    fn allow_semantics_absent_empty_and_fallback() {
        let cfg = stdio("c", &[]);
        // absent allowlist ⇒ everything allowed
        assert!(is_allowed("x", &cfg, &McpPolicy::default()));
        // empty allowlist ⇒ nothing allowed
        assert!(!is_allowed(
            "x",
            &cfg,
            &McpPolicy { denied: None, allowed: Some(vec![]) }
        ));
        // allowlist has only name matchers (no command) ⇒ name fallback
        let p = McpPolicy {
            denied: None,
            allowed: Some(matcher(r#"[{"serverName":"corp"}]"#)),
        };
        assert!(is_allowed("corp", &cfg, &p));
        assert!(!is_allowed("nope", &cfg, &p));
        // deny overrides allow
        let p = McpPolicy {
            denied: Some(matcher(r#"[{"serverName":"corp"}]"#)),
            allowed: Some(matcher(r#"[{"serverName":"corp"}]"#)),
        };
        assert!(!is_allowed("corp", &cfg, &p));
    }

    #[test]
    fn allow_by_command_when_command_matchers_present() {
        // With a command matcher present, a stdio config matches by command, not name.
        let p = McpPolicy {
            denied: None,
            allowed: Some(matcher(r#"[{"serverCommand":["npx","corp-mcp"]}]"#)),
        };
        assert!(is_allowed("anything", &stdio("npx", &["corp-mcp"]), &p));
        assert!(!is_allowed("anything", &stdio("npx", &["other"]), &p));
    }

    #[test]
    fn ufr_star_and_exact_and_scheme() {
        assert!(url_matches("https://a.com/x", "*"));
        assert!(url_matches("https://a.com/mcp", "https://a.com"));
        assert!(url_matches("https://a.com:8443/mcp", "https://a.com:8443"));
        // wrong scheme / host / port
        assert!(!url_matches("http://a.com/mcp", "https://a.com"));
        assert!(!url_matches("https://b.com/mcp", "https://a.com"));
        assert!(!url_matches("https://a.com:9000/mcp", "https://a.com:8443"));
        // invalid config url
        assert!(!url_matches("not a url", "https://a.com"));
    }

    #[test]
    fn ufr_wildcards_host_port_path_protocol() {
        // host wildcard
        assert!(url_matches("https://api.example.com/mcp", "https://*.example.com"));
        assert!(!url_matches("https://example.com/mcp", "https://*.example.com"));
        // protocol wildcard
        assert!(url_matches("http://a.com/x", "*://a.com"));
        assert!(url_matches("https://a.com/x", "*://a.com"));
        // port wildcard
        assert!(url_matches("https://a.com:1234/x", "https://a.com:*"));
        assert!(url_matches("https://a.com:9999/x", "https://a.com:*"));
        // path wildcard
        assert!(url_matches("https://a.com/team/mcp", "https://a.com/team/*"));
        assert!(!url_matches("https://a.com/other/mcp", "https://a.com/team/*"));
    }

    #[test]
    fn ufr_case_insensitive_host_and_trailing_dot() {
        assert!(url_matches("https://API.Example.COM/x", "https://api.example.com"));
        assert!(url_matches("https://a.com./x", "https://a.com"));
    }

    #[test]
    fn policy_messages_are_byte_exact() {
        assert_eq!(
            denied_message("corp"),
            "Cannot add MCP server \"corp\": server is explicitly blocked by enterprise policy"
        );
        assert_eq!(
            not_allowed_message("corp"),
            "Cannot add MCP server \"corp\": not allowed by enterprise policy"
        );
    }

    #[test]
    fn read_managed_policy_folds_base_then_dropins() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("managed-settings.json"),
            r#"{"deniedMcpServers":[{"serverName":"base-deny"}]}"#,
        )
        .unwrap();
        let d = dir.path().join("managed-settings.d");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("10-org.json"),
            r#"{"allowedMcpServers":[{"serverName":"corp"}]}"#,
        )
        .unwrap();
        std::fs::write(d.join("README.md"), "ignored").unwrap();
        let p = read_managed_mcp_policy_in(dir.path());
        assert!(is_denied("base-deny", &stdio("c", &[]), &p));
        assert!(is_allowed("corp", &stdio("c", &[]), &p));
        assert!(!is_allowed("stranger", &stdio("c", &[]), &p));
    }

    #[test]
    fn no_managed_policy_is_inert() {
        // Empty dir ⇒ no denied/allowed ⇒ nothing denied, everything allowed.
        let dir = tempfile::tempdir().unwrap();
        let p = read_managed_mcp_policy_in(dir.path());
        assert!(p.denied.is_none() && p.allowed.is_none());
        assert!(!is_denied("x", &stdio("c", &[]), &p));
        assert!(is_allowed("x", &stdio("c", &[]), &p));
    }
}

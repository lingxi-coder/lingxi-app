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
// Stage 2 — the `bPe`/`edt` (deny) / `gPe`/`ZFe` (allow) policy matchers.
//
// claude reads the allow/deny lists from the *effective merged* settings
// (`$n()`); the CLI `mcp add` has no effective-settings reader, so we read them
// from the managed policy tiers (`managed-settings.json` + `managed-settings.d`)
// — the canonical, enterprise home for `deniedMcpServers`/`allowedMcpServers`.
// (A deny/allow list placed in a *personal* user-tier settings file is not yet
// consulted here; that awaits a shared effective-settings reader.)
//
// Since 2.1.219 the POLICY side of every predicate expands against a dedicated
// policy expansion env (frozen startup snapshot + managed-source env — see the
// `PolicyExpansionEnv` section below), never the ambient process env, and
// allowlist URL entries whose expansion is UNSAFE fail closed (`dWu`).
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

/// Walk the managed settings tiers in ascending priority
/// (`managed-settings.json` base, then `managed-settings.d/` `*.json` sorted,
/// later wins), calling `f` with each tier's top-level JSON object. Shared by
/// the policy reader and the policy-expansion env fold.
fn for_each_managed_settings_tier(dir: &Path, f: &mut dyn FnMut(&serde_json::Map<String, Value>)) {
    let mut fold = |raw: &str| {
        if let Ok(Value::Object(m)) = serde_json::from_str::<Value>(raw) {
            f(&m);
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
}

/// Which policy list a matcher entry came from — the two lists validate
/// `serverName` differently (claude's `gqn` allowed-entry vs `_qn`
/// denied-entry zod schemas).
#[derive(Clone, Copy, PartialEq, Eq)]
enum MatcherListKind {
    Allowed,
    Denied,
}

/// The zod validation an allow/deny entry must pass (claude `gqn`/`_qn`),
/// byte-exact messages. Field checks run before the exactly-one refine, like
/// zod. `None` ⇒ valid.
fn matcher_validation_error(m: &McpServerMatcher, kind: MatcherListKind) -> Option<&'static str> {
    if let Some(name) = m.server_name.as_deref() {
        match kind {
            MatcherListKind::Allowed => {
                // gqn: `/^[a-zA-Z0-9_-]+$/`.
                if name.is_empty()
                    || !name
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
                {
                    return Some(
                        "Server name can only contain letters, numbers, hyphens, and underscores",
                    );
                }
            }
            MatcherListKind::Denied => {
                if name.is_empty() {
                    return Some("Server name must be non-empty");
                }
                if name.trim().is_empty() {
                    return Some("Server name must not be whitespace-only");
                }
                if name != name.trim() {
                    return Some(
                        "Server name has leading or trailing whitespace and will never match (names are compared verbatim)",
                    );
                }
            }
        }
    }
    if let Some(cmd) = m.server_command.as_ref() {
        if cmd.is_empty() {
            return Some("Server command must have at least one element (the command)");
        }
    }
    let present = [
        m.server_name.is_some(),
        m.server_command.is_some(),
        m.server_url.is_some(),
    ];
    if present.iter().filter(|p| **p).count() != 1 {
        return Some("Entry must have exactly one of \"serverName\", \"serverCommand\", or \"serverUrl\"");
    }
    None
}

/// [`read_managed_mcp_policy`] rooted at an explicit dir (testable). Entries
/// failing the zod-mirrored validation ([`matcher_validation_error`]) are
/// dropped with a warning — they never participate in matching, exactly as an
/// entry the oracle's typed settings never contained.
#[must_use]
pub fn read_managed_mcp_policy_in(dir: &Path) -> McpPolicy {
    let mut merged = serde_json::Map::new();
    for_each_managed_settings_tier(dir, &mut |m| {
        for (k, v) in m {
            merged.insert(k.clone(), v.clone());
        }
    });
    let parse = |key: &str, kind: MatcherListKind| -> Option<Vec<McpServerMatcher>> {
        let list = merged
            .get(key)
            .and_then(|v| serde_json::from_value::<Vec<McpServerMatcher>>(v.clone()).ok())?;
        Some(
            list.into_iter()
                .filter(|m| match matcher_validation_error(m, kind) {
                    None => true,
                    Some(msg) => {
                        tracing::warn!("{key}: {msg}");
                        false
                    }
                })
                .collect(),
        )
    };
    McpPolicy {
        denied: parse("deniedMcpServers", MatcherListKind::Denied),
        allowed: parse("allowedMcpServers", MatcherListKind::Allowed),
    }
}

/// claude `byo` — expand `${VAR}` / `${VAR:-default}` against the LIVE
/// process environment. This is the CANDIDATE-config side only; POLICY
/// strings expand against [`PolicyExpansionEnv`] (see below), never the live
/// env.
fn get(s: &str) -> String {
    crate::env_expansion::expand_env_vars_in_string(s).expanded
}

// ────────────────────────────────────────────────────────────────────────────
// Policy expansion environment (claude `NQr`/`cWu`/`U__`/`uWu`, 2.1.219+).
//
// Policy predicates (`serverCommand` items, `serverUrl` patterns) do NOT
// expand against the ambient process env: the oracle expands them with the
// frozen STARTUP env snapshot overlaid by the managed settings sources' own
// `env` blocks (`cWu`), so a settings-file `env` entry from a lower-trust
// scope can never satisfy — or dodge — an enterprise predicate. The DENY side
// additionally gets a fallback env assembled from the global config,
// userSettings, flagSettings, and policySettings `env` blocks (`U__`), letting
// denylist entries keep matching values a user configured anywhere. (The
// `--settings` flagSettings tier is not plumbed into this crate; its slot in
// the fallback fold is documented here and intentionally empty.)
// ────────────────────────────────────────────────────────────────────────────

use indexmap::IndexMap;

/// One settings source's `env` block → string (key, value) pairs — claude
/// `ELt`: only string values participate and `NO_COLOR`/`FORCE_COLOR` are
/// stripped. (The oracle additionally filters host-orchestration control keys
/// (`aqu`…`cqu`) guarding hosted deployments this port does not model.)
fn settings_env_pairs(env: &Value) -> Vec<(String, String)> {
    let Value::Object(m) = env else {
        return Vec::new();
    };
    m.iter()
        .filter(|(k, _)| k.as_str() != "NO_COLOR" && k.as_str() != "FORCE_COLOR")
        .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
        .collect()
}

/// claude `cWu`'s settings-source env fold (the `for t of VQ()` loop) over the
/// managed tiers. `VQ()` folds first-seen-wins over DESCENDING priority; our
/// tier walk is ascending (drop-ins override base), so later-wins here is the
/// same fold viewed from the other end.
fn managed_sources_env_in(dir: &Path) -> IndexMap<String, String> {
    let mut out = IndexMap::new();
    for_each_managed_settings_tier(dir, &mut |m| {
        if let Some(env) = m.get("env") {
            for (k, v) in settings_env_pairs(env) {
                out.insert(k, v);
            }
        }
    });
    out
}

/// The global config's `env` block (claude `Rt().env` — `~/.lingxi.json`), a
/// deny-side fallback tier.
fn global_config_env() -> IndexMap<String, String> {
    let Some(path) = migrations::global_config::global_config_path() else {
        return IndexMap::new();
    };
    let Ok(map) = migrations::global_config::read_map(&path) else {
        return IndexMap::new();
    };
    map.get("env")
        .map(|v| settings_env_pairs(v).into_iter().collect())
        .unwrap_or_default()
}

/// The user settings' `env` block (claude `Hr("userSettings")?.env` —
/// `<config-home>/settings.json`), a deny-side fallback tier.
fn user_settings_env() -> IndexMap<String, String> {
    let Some(home) = migrations::global_config::lingxi_config_home() else {
        return IndexMap::new();
    };
    let Ok(raw) = std::fs::read_to_string(home.join("settings.json")) else {
        return IndexMap::new();
    };
    let Ok(Value::Object(m)) = serde_json::from_str::<Value>(&raw) else {
        return IndexMap::new();
    };
    m.get("env")
        .map(|v| settings_env_pairs(v).into_iter().collect())
        .unwrap_or_default()
}

/// The environments enterprise policy predicates expand against — claude
/// `U__()` (whose `env` field is exactly `cWu()`, the allow-side env).
pub struct PolicyExpansionEnv {
    /// claude `cWu()` — frozen startup env snapshot overlaid by the managed
    /// settings sources' `env` blocks (managed values win). The ONLY env the
    /// allow side sees, and the deny side's primary env.
    pub env: IndexMap<String, String>,
    /// claude `U__().fallbackEnv` — globalConfig → userSettings →
    /// (flagSettings, unplumbed) → policySettings `env` blocks, later wins.
    /// Deny-side only.
    pub fallback_env: IndexMap<String, String>,
}

/// Compose the production [`PolicyExpansionEnv`] (claude `U__()`/`cWu()`).
#[must_use]
pub fn policy_expansion_env() -> PolicyExpansionEnv {
    policy_expansion_env_with(
        &managed_dir(),
        crate::env_expansion::startup_env_snapshot(),
        &global_config_env(),
        &user_settings_env(),
    )
}

/// The [`policy_expansion_env`] core, parameterized on every input (testable
/// without touching the process env or the real managed dir).
#[must_use]
pub fn policy_expansion_env_with(
    dir: &Path,
    startup_snapshot: &IndexMap<String, String>,
    global_config: &IndexMap<String, String>,
    user_settings: &IndexMap<String, String>,
) -> PolicyExpansionEnv {
    let managed = managed_sources_env_in(dir);
    // cWu: `{...NQr(), ...e}` — the managed-source env OVERRIDES the snapshot.
    let mut env = startup_snapshot.clone();
    for (k, v) in &managed {
        env.insert(k.clone(), v.clone());
    }
    // U__: `Object.assign({}, globalConfig, userSettings, flagSettings,
    // policySettings)` — later wins.
    let mut fallback_env = IndexMap::new();
    for tier in [global_config, user_settings] {
        for (k, v) in tier {
            fallback_env.insert(k.clone(), v.clone());
        }
    }
    for (k, v) in managed {
        fallback_env.insert(k, v);
    }
    PolicyExpansionEnv { env, fallback_env }
}

/// claude `uWu(value, env, fallback)` — expand a NON-URL policy string,
/// warning (byte-exact) when it references variables absent from the policy
/// expansion env. The unresolved `${...}` stays literal in the result.
fn expand_policy_string(
    value: &str,
    env: &IndexMap<String, String>,
    fallback: Option<&IndexMap<String, String>>,
) -> String {
    let r = crate::env_expansion::expand_with_env(value, env, fallback);
    if !r.missing_vars.is_empty() {
        tracing::warn!(
            "MCP policy predicate references environment variable(s) not present in the policy expansion env: {}",
            r.missing_vars.join(", ")
        );
    }
    r.expanded
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

// ── Policy URL expansion with unsafe-expansion detection (claude `dWu`) ──

/// claude `VLt` — the sentinel protecting PATTERN-AUTHORED `*` through env
/// expansion, so a `*` INJECTED by an expanded value can be told apart from
/// one the policy author wrote. Random per-process in claude
/// (`zzadminwc<hex>zz`); fixed here for the same determinism/collision
/// reasoning as [`WILDCARD_SENTINEL`] (it must survive URL parsing, so
/// lowercase-alnum).
const ADMIN_WILDCARD_SENTINEL: &str = "zzadminwcsentinelzz";

/// claude `q__` — the stand-in for arbitrary env values in the masked
/// expansion pass (`iSs`'s fallback arm).
const ENV_VALUE_MASK: &str = "zzenvsubzz";

/// claude `iSs(value)` — mask an env value with a shape-preserving stand-in:
/// trailing dots are kept verbatim (recursing on the prefix), an all-numeric
/// dotted value keeps its dots with digit runs zeroed (IPv4-ish), a
/// hex-and-colon value becomes `::` (IPv6-ish), anything else becomes
/// [`ENV_VALUE_MASK`]. The masked pass parses the URL with these stand-ins to
/// detect values that RESTRUCTURE the URL.
fn mask_value(value: &str) -> String {
    static NUMERIC: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static DIGIT_RUN: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static IPV6ISH: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let trimmed = value.trim_end_matches('.');
    if trimmed.len() < value.len() && !trimmed.is_empty() {
        return format!("{}{}", mask_value(trimmed), &value[trimmed.len()..]);
    }
    let numeric = NUMERIC.get_or_init(|| regex::Regex::new(r"^[0-9]+(\.[0-9]+)*$").unwrap());
    if numeric.is_match(value) {
        let digit_run = DIGIT_RUN.get_or_init(|| regex::Regex::new(r"[0-9]+").unwrap());
        return digit_run.replace_all(value, "0").into_owned();
    }
    let ipv6ish = IPV6ISH.get_or_init(|| regex::Regex::new(r"^[0-9a-fA-F:.]+$").unwrap());
    if value.contains(':') && ipv6ish.is_match(value) {
        return "::".to_string();
    }
    ENV_VALUE_MASK.to_string()
}

/// The JS `URL` component view claude's `pyo` returns: `protocol` keeps its
/// `:`, `search` its `?`, `hash` its `#`, and `port` is `""` when
/// absent/default — matching the `js_*` helpers used by [`url_matches`].
struct JsUrlParts {
    protocol: String,
    hostname: String,
    username: String,
    password: String,
    port: String,
    pathname: String,
    search: String,
    hash: String,
}

/// First-match-only `:(?:<sentinel>)(?=[/?#]|$)` → `:0` (claude `pyo`'s retry
/// regex is non-global). Among the sentinels, the EARLIEST valid occurrence
/// wins. `None` when nothing replaced.
fn replace_first_port_sentinel(s: &str, sentinels: &[&str]) -> Option<String> {
    let mut best: Option<(usize, usize)> = None;
    for needle in sentinels {
        let pat = format!(":{needle}");
        let mut from = 0;
        while let Some(rel) = s[from..].find(&pat) {
            let pos = from + rel;
            let after = &s[pos + pat.len()..];
            if after.is_empty() || after.starts_with(['/', '?', '#']) {
                if best.is_none_or(|(b, _)| pos < b) {
                    best = Some((pos, pat.len()));
                }
                break;
            }
            from = pos + 1;
        }
    }
    let (pos, len) = best?;
    Some(format!("{}:0{}", &s[..pos], &s[pos + len..]))
}

/// claude `pyo(str, VLt)` — parse a URL whose VALUES may have injected `*`
/// (protected via [`WILDCARD_SENTINEL`] so the parse survives) and whose
/// pattern may carry a wildcard PORT (`:<sentinel>` retried as `:0`). Returns
/// the JS component view with the value-wildcard sentinel restored to `*`
/// ([`ADMIN_WILDCARD_SENTINEL`] is deliberately left in place — the caller
/// compares its survival across the real and masked passes). `None` when the
/// string does not parse as a URL either way.
fn parse_url_with_wildcards(input: &str, extra_port_sentinel: Option<&str>) -> Option<JsUrlParts> {
    let r = input.replace('*', WILDCARD_SENTINEL);
    let parsed = match url::Url::parse(&r) {
        Ok(u) => u,
        Err(_) => {
            let mut sentinels = vec![WILDCARD_SENTINEL];
            sentinels.extend(extra_port_sentinel);
            let retried = replace_first_port_sentinel(&r, &sentinels)?;
            url::Url::parse(&retried).ok()?
        }
    };
    let restore = |x: String| x.replace(WILDCARD_SENTINEL, "*");
    Some(JsUrlParts {
        protocol: restore(js_protocol(&parsed)),
        hostname: restore(parsed.host_str().unwrap_or("").to_string()),
        username: restore(parsed.username().to_string()),
        password: restore(parsed.password().unwrap_or("").to_string()),
        port: restore(js_port(&parsed)),
        pathname: restore(parsed.path().to_string()),
        search: restore(js_search(&parsed)),
        hash: restore(
            parsed
                .fragment()
                .map(|f| format!("#{f}"))
                .unwrap_or_default(),
        ),
    })
}

/// Where an env var lands inside the expanded URL (claude `j__`'s
/// `"scheme" | "authority" | "rest"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UrlVarPosition {
    Scheme,
    Authority,
    Rest,
}

/// claude `j__(pattern, env)` — classify every env var the pattern references
/// by URL position. Each var is probed with a positional sentinel
/// (`zzv<idx>zz`) while OTHER vars get masked values; a second pass re-probes
/// with scheme-classified vars at their REAL values (a scheme var changes how
/// the rest of the URL parses).
fn classify_url_var_positions(
    pattern: &str,
    env: &IndexMap<String, String>,
) -> Vec<(String, UrlVarPosition)> {
    let mut referenced: Vec<String> = Vec::new();
    for caps in crate::env_expansion::env_ref_regex().captures_iter(pattern) {
        let name = &caps[1];
        if !referenced.iter().any(|r| r == name) && env.contains_key(name) {
            referenced.push(name.to_string());
        }
    }
    let classify = |target: &str, real: &std::collections::HashSet<String>| -> UrlVarPosition {
        let idx = referenced
            .iter()
            .position(|x| x == target)
            .unwrap_or_default();
        let c = format!("zzv{idx}zz");
        let lookup = |name: &str| -> Option<String> {
            if name == target {
                return Some(c.clone());
            }
            let v = env.get(name)?;
            Some(if real.contains(name) {
                v.clone()
            } else {
                mask_value(v)
            })
        };
        let d = crate::env_expansion::expand_with_lookups(pattern, &lookup, None).expanded;
        let Some(p) = parse_url_with_wildcards(&d, Some(ADMIN_WILDCARD_SENTINEL)) else {
            return UrlVarPosition::Authority;
        };
        if p.protocol.contains(&c) {
            return UrlVarPosition::Scheme;
        }
        if p.username.contains(&c)
            || p.password.contains(&c)
            || p.hostname.contains(&c)
            || p.port.contains(&c)
        {
            return UrlVarPosition::Authority;
        }
        if p.pathname.contains(&c) || p.search.contains(&c) || p.hash.contains(&c) {
            return UrlVarPosition::Rest;
        }
        UrlVarPosition::Scheme
    };
    let empty = std::collections::HashSet::new();
    let scheme_vars: std::collections::HashSet<String> = referenced
        .iter()
        .filter(|a| classify(a, &empty) == UrlVarPosition::Scheme)
        .cloned()
        .collect();
    referenced
        .iter()
        .map(|a| {
            let pos = if scheme_vars.contains(a) {
                UrlVarPosition::Scheme
            } else {
                classify(a, &scheme_vars)
            };
            (a.clone(), pos)
        })
        .collect()
}

/// claude `V__(classes, env)` — does any scheme/authority-position var carry a
/// value with URL-structure characters? Scheme position rejects
/// `[:/@#?\\\s]` (`G__`), authority `[/@#?\\\s]` (`W__` — a `:` is a legal
/// port separator there). (JS `\s` and Rust `\s` differ only on exotica like
/// U+FEFF.)
fn value_injects_url_structure(
    classes: &[(String, UrlVarPosition)],
    env: &IndexMap<String, String>,
) -> bool {
    static SCHEME_UNSAFE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static AUTHORITY_UNSAFE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    for (name, pos) in classes {
        if *pos == UrlVarPosition::Rest {
            continue;
        }
        let Some(v) = env.get(name) else { continue };
        let re = if *pos == UrlVarPosition::Scheme {
            SCHEME_UNSAFE.get_or_init(|| regex::Regex::new(r"[:/@#?\\\s]").unwrap())
        } else {
            AUTHORITY_UNSAFE.get_or_init(|| regex::Regex::new(r"[/@#?\\\s]").unwrap())
        };
        if re.is_match(v) {
            return true;
        }
    }
    false
}

/// claude `K__(pattern, env)` — does any referenced var's value smuggle a
/// query/fragment (`[?#]`) or a dot-segment traversal? The value is
/// normalized first (strip `\t\n\r`, backslashes → `/`, `%2e` → `.`), then
/// tested against `z__` `(^|/)(\.|%2e)(\.|%2e)?(/|$)` case-insensitively.
fn value_injects_path_traversal(pattern: &str, env: &IndexMap<String, String>) -> bool {
    static PERCENT_2E: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static TRAVERSAL: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    for caps in crate::env_expansion::env_ref_regex().captures_iter(pattern) {
        let Some(v) = env.get(&caps[1]) else { continue };
        if v.contains('?') || v.contains('#') {
            return true;
        }
        let stripped: String = v
            .chars()
            .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
            .collect();
        let slashed = stripped.replace('\\', "/");
        let normalized = PERCENT_2E
            .get_or_init(|| regex::Regex::new(r"(?i)%2e").unwrap())
            .replace_all(&slashed, ".");
        if TRAVERSAL
            .get_or_init(|| regex::Regex::new(r"(?i)(^|/)(\.|%2e)(\.|%2e)?(/|$)").unwrap())
            .is_match(&normalized)
        {
            return true;
        }
    }
    false
}

/// Result of a policy `serverUrl` expansion (claude `dWu`'s
/// `{expanded, unsafeExpansion}`).
struct PolicyUrlExpansion {
    expanded: String,
    unsafe_expansion: bool,
}

/// claude `dWu(pattern, env, fallback)` — expand a policy URL pattern with
/// unsafe-expansion detection. Pattern-authored `*` is sentinel-protected,
/// then the pattern is expanded twice — real values vs shape-masked values —
/// and the two parses are compared: a value that injects wildcard semantics,
/// restructures the URL (scheme/authority characters, dropped hash/search,
/// unparseable masked form), or smuggles traversal marks the expansion
/// UNSAFE. Allowlist entries using an unsafe expansion fail closed
/// ([`is_allowed_with_env`] skips them); denylist entries keep using the
/// expanded pattern. Both warn messages are byte-exact.
fn expand_policy_url(
    pattern: &str,
    env: &IndexMap<String, String>,
    fallback: Option<&IndexMap<String, String>>,
) -> PolicyUrlExpansion {
    let o: String = pattern
        .chars()
        .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
        .collect::<String>()
        .replace('*', ADMIN_WILDCARD_SENTINEL);
    let i = crate::env_expansion::expand_with_env(&o, env, fallback);
    // Masked pass: every env value shape-masked, NO fallback (claude's proxy
    // covers `t` only — a fallback-resolved var stays literal here).
    let masked_lookup = |name: &str| env.get(name).map(|v| mask_value(v));
    let a = crate::env_expansion::expand_with_lookups(&o, &masked_lookup, None);
    let l = parse_url_with_wildcards(&i.expanded, Some(ADMIN_WILDCARD_SENTINEL));
    let c = parse_url_with_wildcards(&a.expanded, Some(ADMIN_WILDCARD_SENTINEL));
    let u = l.as_ref().map(|p| p.hostname.clone());
    let d = c.as_ref().map(|p| p.hostname.clone());
    let value_changed = a.expanded != o; // claude `p`
    let classes = if value_changed {
        classify_url_var_positions(&o, env)
    } else {
        Vec::new()
    };
    let unsafe_expansion = !i.wildcard_vars.is_empty()
        || (value_changed && value_injects_url_structure(&classes, env))
        || (u.is_none() != d.is_none())
        || (value_changed && u.is_none())
        || l.as_ref().is_some_and(|lp| {
            let c_hash = c.as_ref().map_or("", |p| p.hash.as_str());
            let c_search = c.as_ref().map_or("", |p| p.search.as_str());
            (!lp.hash.is_empty() && c_hash.is_empty())
                || (!lp.search.is_empty() && c_search.is_empty())
        })
        || (value_changed && value_injects_path_traversal(&o, env))
        || u.as_deref().is_some_and(|uh| {
            let dh = d.as_deref().unwrap_or("");
            (uh.contains('*') && !dh.contains('*'))
                || (uh.ends_with('.') && !dh.ends_with('.'))
                || (uh.contains(ADMIN_WILDCARD_SENTINEL) && !dh.contains(ADMIN_WILDCARD_SENTINEL))
        });
    if !i.missing_vars.is_empty() {
        tracing::warn!(
            "MCP policy predicate references environment variable(s) not present in the policy expansion env: {}",
            i.missing_vars.join(", ")
        );
    }
    if unsafe_expansion {
        let reason = if i.wildcard_vars.is_empty() {
            "a value restructured the URL, or the expansion is unparseable as a URL (e.g. a whole-URL ${VAR}: rewrite as https://${HOST}/path \u{2014} hostname-position variables are fully supported)"
        } else {
            "a value injected wildcard semantics"
        };
        let vars = if i.wildcard_vars.is_empty() {
            env.keys()
                .filter(|g| {
                    pattern.contains(&format!("${{{g}}}")) || pattern.contains(&format!("${{{g}:"))
                })
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        } else {
            i.wildcard_vars.join(", ")
        };
        let vars = if vars.is_empty() {
            "unknown".to_string()
        } else {
            vars
        };
        tracing::warn!(
            "MCP policy URL predicate expansion was unsafe \u{2014} {reason} (variables: {vars}) \u{2014} allowlist URL entries using it fail closed; denylist entries are unaffected"
        );
    }
    PolicyUrlExpansion {
        expanded: i.expanded.replace(ADMIN_WILDCARD_SENTINEL, "*"),
        unsafe_expansion,
    }
}

/// claude `edt(name, config)` — is the server *explicitly denied* by policy?
/// Composes the production [`PolicyExpansionEnv`] per call (as `edt` calls
/// `U__()`); [`is_denied_with_env`] is the injectable core.
#[must_use]
pub fn is_denied(name: &str, config: &Value, policy: &McpPolicy) -> bool {
    is_denied_with_env(name, config, policy, &policy_expansion_env())
}

/// The [`is_denied`] core against an explicit [`PolicyExpansionEnv`]. POLICY
/// strings expand with `envs.env` + `envs.fallback_env` (`uWu(c, n, o)` /
/// `dWu(l.serverUrl, n, o)`); the CANDIDATE config side stays on the live
/// process env (`byo`). Deny URL entries use the expansion even when it was
/// unsafe — the fail-closed skip is allow-side only.
#[must_use]
pub fn is_denied_with_env(
    name: &str,
    config: &Value,
    policy: &McpPolicy,
    envs: &PolicyExpansionEnv,
) -> bool {
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
                let sc_e: Vec<String> = sc
                    .iter()
                    .map(|s| expand_policy_string(s, &envs.env, Some(&envs.fallback_env)))
                    .collect();
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
                let expansion = expand_policy_url(su, &envs.env, Some(&envs.fallback_env));
                if url_matches(&iu, &expansion.expanded) {
                    return true;
                }
            }
        }
    }
    false
}

/// claude `ZFe(name, config)` — is the server *allowed* by policy? Composes
/// the production [`PolicyExpansionEnv`] per call (as `ZFe` calls `cWu()`);
/// [`is_allowed_with_env`] is the injectable core.
#[must_use]
pub fn is_allowed(name: &str, config: &Value, policy: &McpPolicy) -> bool {
    is_allowed_with_env(name, config, policy, &policy_expansion_env())
}

/// The [`is_allowed`] core against an explicit [`PolicyExpansionEnv`]. The
/// allow side expands policy strings with `envs.env` ONLY — no fallback
/// (`uWu(u, i)` / `dWu(c.serverUrl, i)`) — and an allowlist URL entry whose
/// expansion was UNSAFE is skipped entirely (fail closed).
#[must_use]
pub fn is_allowed_with_env(
    name: &str,
    config: &Value,
    policy: &McpPolicy,
    envs: &PolicyExpansionEnv,
) -> bool {
    if is_denied_with_env(name, config, policy, envs) {
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
    let name_match = || {
        allowed
            .iter()
            .any(|m| m.server_name.as_deref() == Some(name))
    };

    if let Some(cmd) = config_command(config) {
        if has_cmd {
            let a: Vec<String> = cmd.iter().map(|s| get(s)).collect();
            for m in allowed {
                if let Some(sc) = m.server_command.as_ref() {
                    let sc_e: Vec<String> = sc
                        .iter()
                        .map(|s| expand_policy_string(s, &envs.env, None))
                        .collect();
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
                    let expansion = expand_policy_url(su, &envs.env, None);
                    if expansion.unsafe_expansion {
                        continue; // fail closed (claude `if(d)continue`)
                    }
                    if url_matches(&a, &expansion.expanded) {
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

/// A fixed wildcard sentinel (claude's `L7t`/`YVe`, which is random
/// per-process only to avoid colliding with real URL content; a fixed
/// lowercase-alnum string is equally collision-free and deterministic, and
/// parses as a URL scheme so the protocol-wildcard branch works).
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

// ────────────────────────────────────────────────────────────────────────────
// Stage 3 — load-time enforcement (claude `Ree` / `Qme` / `bC("enterprise")`).
//
// At MCP load time claude drops any server the policy blocks (`Ree`: keep iff
// `type === "sdk" || gPe(name, config)`), and when a managed MCP config is
// active it takes *exclusive* control — only its own servers load (`Qme`'s
// `D1()` branch). Both reuse the matchers above.
// ────────────────────────────────────────────────────────────────────────────

use crate::connection::{ConfigScope, McpServerConfig};
use traits::McpTransportSpec;

/// Project a loaded [`McpTransportSpec`] onto the `{type, command, args, url}`
/// config view the matchers extract from (claude `oVn`/`iVn` read those keys).
fn spec_matcher_view(spec: &McpTransportSpec) -> Value {
    match spec {
        McpTransportSpec::Stdio { command, args, .. } => serde_json::json!({
            "type": "stdio",
            "command": command,
            "args": args,
        }),
        McpTransportSpec::Sse { url, .. } | McpTransportSpec::SseIde { url, .. } => {
            serde_json::json!({ "type": "sse", "url": url })
        }
        McpTransportSpec::Http { url, .. } => serde_json::json!({ "type": "http", "url": url }),
        McpTransportSpec::WebSocket { url, .. } => serde_json::json!({ "type": "ws", "url": url }),
        // No command/url to match — a name-only matcher still applies; a plain
        // object with a non-stdio type makes `oVn` return `None`.
        McpTransportSpec::InProcess { .. } => serde_json::json!({ "type": "inprocess" }),
        McpTransportSpec::SdkControl { .. } => serde_json::json!({ "type": "sdk" }),
    }
}

/// claude `Ree`'s per-server predicate — a loaded server is kept iff it is an
/// SDK-control server or the allow/deny policy permits it.
#[must_use]
pub fn is_server_allowed(config: &McpServerConfig, policy: &McpPolicy) -> bool {
    if matches!(config.spec, McpTransportSpec::SdkControl { .. }) {
        return true; // claude `i.type === "sdk"` short-circuit
    }
    is_allowed(&config.name, &spec_matcher_view(&config.spec), policy)
}

/// Parse the managed MCP config (`managed-mcp.json`) into server configs
/// (claude `bC("enterprise")`), scoped [`ConfigScope::Enterprise`]. Empty on a
/// missing/malformed file.
#[must_use]
pub fn load_enterprise_servers() -> Vec<McpServerConfig> {
    let Ok(raw) = std::fs::read_to_string(managed_mcp_config_path()) else {
        return Vec::new();
    };
    crate::json_config::parse_mcp_json_string(&raw, ConfigScope::Enterprise).unwrap_or_default()
}

/// Apply the enterprise MCP policy to the assembled to-connect list, in place —
/// the load-site equivalent of claude's `Qme`/`Ree`:
///
/// - When [`enterprise_mcp_active`], replace the list with the managed
///   (`managed-mcp.json`) servers that pass the allow policy — exclusive
///   control (`D1()` branch).
/// - Otherwise drop every server the allow/deny policy blocks (`Ree`).
///
/// Inert when no managed config or policy is present (nothing is removed), so a
/// default deployment is byte-identical.
pub fn apply_enterprise_mcp_policy(configs: &mut Vec<McpServerConfig>) {
    let policy = read_managed_mcp_policy();
    if enterprise_mcp_active() {
        *configs = load_enterprise_servers()
            .into_iter()
            .filter(|c| is_server_allowed(c, &policy))
            .collect();
    } else {
        configs.retain(|c| is_server_allowed(c, &policy));
    }
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
        assert!(!enterprise_mcp_active_at(
            &dir.path().join("managed-mcp.json")
        ));
    }

    #[test]
    fn valid_json_object_is_active() {
        let dir = tempfile::tempdir().unwrap();
        // Any valid JSON object → config non-null → active (matches `$7t`,
        // which does not require an `mcpServers` key at this gate).
        let p = write(
            &dir,
            "managed-mcp.json",
            r#"{"mcpServers":{"corp":{"type":"stdio","command":"c"}}}"#,
        );
        assert!(enterprise_mcp_active_at(&p));
        let p2 = write(&dir, "empty-obj.json", "{}");
        assert!(enterprise_mcp_active_at(&p2));
    }

    #[test]
    fn empty_or_malformed_or_non_object_is_not_active() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!enterprise_mcp_active_at(&write(&dir, "empty.json", "")));
        assert!(!enterprise_mcp_active_at(&write(&dir, "ws.json", "   \n ")));
        assert!(!enterprise_mcp_active_at(&write(
            &dir,
            "bad.json",
            "{not json"
        )));
        // A valid JSON value that is not an object → null config.
        assert!(!enterprise_mcp_active_at(&write(
            &dir, "arr.json", "[1,2,3]"
        )));
        assert!(!enterprise_mcp_active_at(&write(
            &dir, "str.json", "\"hi\""
        )));
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
        assert_eq!(
            managed_mcp_config_path(),
            dir.path().join("managed-mcp.json")
        );
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
            &McpPolicy {
                denied: None,
                allowed: Some(vec![])
            }
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
        assert!(url_matches(
            "https://api.example.com/mcp",
            "https://*.example.com"
        ));
        assert!(!url_matches(
            "https://example.com/mcp",
            "https://*.example.com"
        ));
        // protocol wildcard
        assert!(url_matches("http://a.com/x", "*://a.com"));
        assert!(url_matches("https://a.com/x", "*://a.com"));
        // port wildcard
        assert!(url_matches("https://a.com:1234/x", "https://a.com:*"));
        assert!(url_matches("https://a.com:9999/x", "https://a.com:*"));
        // path wildcard
        assert!(url_matches(
            "https://a.com/team/mcp",
            "https://a.com/team/*"
        ));
        assert!(!url_matches(
            "https://a.com/other/mcp",
            "https://a.com/team/*"
        ));
    }

    #[test]
    fn ufr_case_insensitive_host_and_trailing_dot() {
        assert!(url_matches(
            "https://API.Example.COM/x",
            "https://api.example.com"
        ));
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

    // ── policy expansion env (2.1.219 `cWu`/`U__`/`uWu`/`dWu`) ──

    fn map(pairs: &[(&str, &str)]) -> IndexMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn policy_env_snapshot_overlaid_by_managed_sources() {
        let dir = tempfile::tempdir().unwrap();
        // Base file env + a drop-in that overrides one key; NO_COLOR is
        // stripped (`ELt`) and non-string values never participate.
        std::fs::write(
            dir.path().join("managed-settings.json"),
            r#"{"env":{"CORP_HOST":"base.example.com","KEEP":"base","NO_COLOR":"1","NUM":7}}"#,
        )
        .unwrap();
        let d = dir.path().join("managed-settings.d");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("10-org.json"),
            r#"{"env":{"CORP_HOST":"drop.example.com"}}"#,
        )
        .unwrap();
        let snapshot = map(&[("CORP_HOST", "startup"), ("ONLY_SNAP", "snap")]);
        let envs = policy_expansion_env_with(dir.path(), &snapshot, &map(&[]), &map(&[]));
        // cWu: `{...NQr(), ...settingsEnv}` — managed values WIN over the
        // snapshot, drop-ins win over the base file.
        assert_eq!(
            envs.env.get("CORP_HOST").map(String::as_str),
            Some("drop.example.com")
        );
        assert_eq!(envs.env.get("KEEP").map(String::as_str), Some("base"));
        assert_eq!(envs.env.get("ONLY_SNAP").map(String::as_str), Some("snap"));
        assert!(!envs.env.contains_key("NO_COLOR"));
        assert!(!envs.env.contains_key("NUM"));
    }

    #[test]
    fn policy_env_fallback_fold_order() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("managed-settings.json"),
            r#"{"env":{"X":"managed"}}"#,
        )
        .unwrap();
        // U__: Object.assign(globalConfig, userSettings, …, policySettings) —
        // later (higher-trust) tiers win.
        let envs = policy_expansion_env_with(
            dir.path(),
            &map(&[]),
            &map(&[("X", "global"), ("G", "global")]),
            &map(&[("X", "user"), ("U", "user")]),
        );
        assert_eq!(
            envs.fallback_env.get("X").map(String::as_str),
            Some("managed")
        );
        assert_eq!(
            envs.fallback_env.get("G").map(String::as_str),
            Some("global")
        );
        assert_eq!(envs.fallback_env.get("U").map(String::as_str), Some("user"));
    }

    #[test]
    fn deny_expands_against_policy_env_not_live_process_env() {
        let p = McpPolicy {
            denied: Some(matcher(r#"[{"serverCommand":["${CORP_CMD}","-y","corp"]}]"#)),
            allowed: None,
        };
        // The var lives ONLY in the injected policy env (the live process env
        // has no CORP_CMD) — the deny predicate still resolves.
        let envs = PolicyExpansionEnv {
            env: map(&[("CORP_CMD", "npx")]),
            fallback_env: map(&[]),
        };
        assert!(is_denied_with_env(
            "x",
            &stdio("npx", &["-y", "corp"]),
            &p,
            &envs
        ));
        // Conversely a var set in the LIVE env (PATH always is) must NOT leak
        // into the policy side: with an empty policy env the `${PATH}x`
        // predicate stays literal and can never equal the live expansion.
        let p = McpPolicy {
            denied: Some(matcher(r#"[{"serverCommand":["${PATH}x"]}]"#)),
            allowed: None,
        };
        let live = format!("{}x", std::env::var("PATH").unwrap());
        let empty = PolicyExpansionEnv {
            env: map(&[]),
            fallback_env: map(&[]),
        };
        assert!(!is_denied_with_env("x", &stdio(&live, &[]), &p, &empty));
    }

    #[test]
    fn deny_consults_fallback_env_but_allow_does_not() {
        // `edt` expands with {env, fallbackEnv} (`uWu(c, n, o)`); `ZFe` gets
        // `cWu()` alone (`uWu(u, i)`).
        let denied = McpPolicy {
            denied: Some(matcher(r#"[{"serverCommand":["${TOOL}"]}]"#)),
            allowed: None,
        };
        let allowed = McpPolicy {
            denied: None,
            allowed: Some(matcher(r#"[{"serverCommand":["${TOOL}"]}]"#)),
        };
        let envs = PolicyExpansionEnv {
            env: map(&[]),
            fallback_env: map(&[("TOOL", "corp-tool")]),
        };
        let cfg = stdio("corp-tool", &[]);
        assert!(is_denied_with_env("x", &cfg, &denied, &envs));
        // Allow side: TOOL unresolved ⇒ `${TOOL}` stays literal ⇒ no match.
        assert!(!is_allowed_with_env("x", &cfg, &allowed, &envs));
    }

    #[test]
    fn allow_url_fails_closed_on_wildcard_valued_var_deny_unaffected() {
        // dWu: a `*` INJECTED by an expanded value marks the expansion unsafe
        // — the allowlist entry is skipped (`if(d)continue`), while a deny
        // entry keeps using the expanded pattern.
        let envs = PolicyExpansionEnv {
            env: map(&[("H", "*")]),
            fallback_env: map(&[]),
        };
        let allowed = McpPolicy {
            denied: None,
            allowed: Some(matcher(r#"[{"serverUrl":"https://${H}/mcp"}]"#)),
        };
        let cfg = http("https://anything.example.com/mcp");
        assert!(!is_allowed_with_env("x", &cfg, &allowed, &envs));
        let denied = McpPolicy {
            denied: Some(matcher(r#"[{"serverUrl":"https://${H}/mcp"}]"#)),
            allowed: None,
        };
        assert!(is_denied_with_env("x", &cfg, &denied, &envs));
    }

    #[test]
    fn allow_url_hostname_position_var_is_safe() {
        // The documented supported shape: `https://${HOST}/path` with a clean
        // hostname value expands safely and matches.
        let envs = PolicyExpansionEnv {
            env: map(&[("HOST", "corp.example.com")]),
            fallback_env: map(&[]),
        };
        let p = McpPolicy {
            denied: None,
            allowed: Some(matcher(r#"[{"serverUrl":"https://${HOST}/mcp"}]"#)),
        };
        assert!(is_allowed_with_env(
            "x",
            &http("https://corp.example.com/mcp"),
            &p,
            &envs
        ));
        assert!(!is_allowed_with_env(
            "x",
            &http("https://other.example.com/mcp"),
            &p,
            &envs
        ));
    }

    #[test]
    fn allow_whole_url_var_fails_closed() {
        // A whole-URL `${VAR}`: the masked pass (`zzenvsubzz`) no longer
        // parses as a URL while the real pass does ⇒ restructuring ⇒ unsafe.
        let envs = PolicyExpansionEnv {
            env: map(&[("URL", "https://corp.example.com")]),
            fallback_env: map(&[]),
        };
        let p = McpPolicy {
            denied: None,
            allowed: Some(matcher(r#"[{"serverUrl":"${URL}"}]"#)),
        };
        assert!(!is_allowed_with_env(
            "x",
            &http("https://corp.example.com/mcp"),
            &p,
            &envs
        ));
    }

    #[test]
    fn read_managed_policy_drops_invalid_entries() {
        let dir = tempfile::tempdir().unwrap();
        // gqn: allowed serverName must match /^[a-zA-Z0-9_-]+$/; _qn: denied
        // serverName must equal its trim; both: exactly one predicate field.
        std::fs::write(
            dir.path().join("managed-settings.json"),
            r#"{
                "allowedMcpServers":[
                    {"serverName":"bad name!"},
                    {"serverName":"ok","serverUrl":"https://x.com"},
                    {"serverName":"good_1"}
                ],
                "deniedMcpServers":[
                    {"serverName":" corp "},
                    {"serverName":"corp"}
                ]
            }"#,
        )
        .unwrap();
        let p = read_managed_mcp_policy_in(dir.path());
        assert_eq!(
            p.allowed
                .as_ref()
                .unwrap()
                .iter()
                .filter_map(|m| m.server_name.as_deref())
                .collect::<Vec<_>>(),
            ["good_1"]
        );
        assert_eq!(
            p.denied
                .as_ref()
                .unwrap()
                .iter()
                .filter_map(|m| m.server_name.as_deref())
                .collect::<Vec<_>>(),
            ["corp"]
        );
    }

    // ── load-time (Ree) enforcement ──

    fn cfg(name: &str, spec: McpTransportSpec) -> McpServerConfig {
        McpServerConfig {
            name: name.to_string(),
            spec,
            scope: ConfigScope::User,
            disabled: false,
            timeout_ms: None,
            always_load: false,
            config_error: None,
        }
    }
    fn stdio_spec(command: &str, args: &[&str]) -> McpTransportSpec {
        McpTransportSpec::Stdio {
            command: command.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
            env: Default::default(),
        }
    }

    #[test]
    fn is_server_allowed_reuses_matchers_and_sdk_shortcircuits() {
        let policy = McpPolicy {
            denied: Some(matcher(r#"[{"serverName":"corp"}]"#)),
            allowed: None,
        };
        assert!(!is_server_allowed(
            &cfg("corp", stdio_spec("c", &[])),
            &policy
        ));
        assert!(is_server_allowed(&cfg("ok", stdio_spec("c", &[])), &policy));
        // SDK-control servers are always kept (claude `type === "sdk"`).
        assert!(is_server_allowed(
            &cfg(
                "corp",
                McpTransportSpec::SdkControl {
                    control_channel_id: "x".into()
                }
            ),
            &policy
        ));
    }

    #[test]
    fn ree_retain_drops_denied_and_keeps_rest() {
        let mut list = vec![
            cfg("corp", stdio_spec("c", &[])),
            cfg("keep", stdio_spec("c", &[])),
        ];
        let policy = McpPolicy {
            denied: Some(matcher(r#"[{"serverName":"corp"}]"#)),
            allowed: None,
        };
        list.retain(|c| is_server_allowed(c, &policy));
        assert_eq!(
            list.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            ["keep"]
        );
    }
}

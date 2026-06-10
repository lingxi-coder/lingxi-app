# Sandbox Net-Filter P2 — Parent-Proxy + NO_PROXY (pure)

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]`.

**Goal:** Port the pure parent-proxy chaining + `NO_PROXY` bypass logic (`resolve_parent_proxy`, `parse_no_proxy`, `should_bypass_parent_proxy`, `select_parent_proxy_url`) into the `sandbox-runtime` crate. Pure, unit-testable on macOS, no async/Docker. Used by P3's proxy to chain through a corporate/upstream proxy + honor NO_PROXY.

**Architecture:** New module `lingxi-code/sandbox-runtime/src/parent_proxy.rs`. CIDR matching via `ipnet` (already in lock). URL parsing via `url` (already a dep).

**Reference of truth:** `docs/superpowers/references/sandbox-runtime-0.0.54/dist/sandbox/parent-proxy.js:45-212` (`resolveParentProxy`, `parseNoProxy`, `shouldBypassParentProxy`, `selectParentProxyUrl`, `LOOPBACK`). Umbrella: `docs/superpowers/specs/2026-06-10-sandbox-netfilter-umbrella-design.md`.

**Branch:** `parity-sandbox-netfilter-p2p3` (created off main; P1 already merged to main).

**Conventions:** Cargo root `lingxi-code/`; git from repo root, `lingxi-code/...` paths. `-D missing-docs` + clippy pedantic `-D warnings`. Commit `git commit -F`, footer `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. `#![forbid(unsafe_code)]` already on the crate. Pure resolver reads env via an INJECTED map (not std::env) so tests stay env-free.

---

### Task 1: add ipnet dep + the resolved types + NO_PROXY parser

**Files:** Modify `lingxi-code/sandbox-runtime/Cargo.toml` (+`ipnet`); Create `lingxi-code/sandbox-runtime/src/parent_proxy.rs`; Modify `src/lib.rs` (+`pub mod parent_proxy;`).

- [ ] **Step 1: Dep.** Add to `sandbox-netfilter/Cargo.toml` `[dependencies]`: `ipnet = "2"` (in the workspace lock already).

- [ ] **Step 2: Failing tests** (bottom of `parent_proxy.rs`):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect()
    }

    #[test]
    fn parse_no_proxy_splits_suffix_cidr_and_star() {
        let r = parse_no_proxy("*");
        assert!(r.all);
        let r = parse_no_proxy("example.com, .internal, 10.0.0.0/8, 127.0.0.1, host:8080, [::1]:443");
        assert!(!r.all);
        assert!(r.suffixes.contains(&"example.com".to_string()));
        assert!(r.suffixes.contains(&".internal".to_string()));
        assert!(r.suffixes.contains(&"host".to_string())); // :8080 stripped
        // 10.0.0.0/8 + 127.0.0.1 + ::1 go to cidr
        assert!(cidr_contains(&r, "10.1.2.3"));
        assert!(cidr_contains(&r, "127.0.0.1"));
        assert!(cidr_contains(&r, "::1"));
        assert!(!cidr_contains(&r, "11.0.0.1"));
    }

    #[test]
    fn resolve_reads_env_with_https_falling_back_to_http() {
        let e = env(&[("HTTP_PROXY", "http://up:3128")]);
        let r = resolve_parent_proxy(None, &e).expect("some");
        assert_eq!(r.http_url.as_ref().unwrap().as_str(), "http://up:3128/");
        // https falls back to http
        assert_eq!(r.https_url.as_ref().unwrap().as_str(), "http://up:3128/");
        // schemeless host:port accepted
        let r2 = resolve_parent_proxy(None, &env(&[("HTTPS_PROXY", "up:8080")])).expect("some");
        assert_eq!(r2.https_url.as_ref().unwrap().as_str(), "http://up:8080/");
        // none set → None
        assert!(resolve_parent_proxy(None, &env(&[])).is_none());
    }

    #[test]
    fn bypass_loopback_all_cidr_and_suffix_golang_semantics() {
        let r = resolve_parent_proxy(
            None,
            &env(&[("HTTP_PROXY", "http://up:3128"), ("NO_PROXY", "example.com, .corp, 10.0.0.0/8")]),
        )
        .unwrap();
        // loopback always bypassed
        assert!(should_bypass_parent_proxy(&r, "localhost"));
        assert!(should_bypass_parent_proxy(&r, "127.0.0.1"));
        assert!(should_bypass_parent_proxy(&r, "::1"));
        // exact + subdomain (golang: example.com matches foo.example.com)
        assert!(should_bypass_parent_proxy(&r, "example.com"));
        assert!(should_bypass_parent_proxy(&r, "foo.example.com"));
        // leading-dot suffix: .corp matches foo.corp AND corp
        assert!(should_bypass_parent_proxy(&r, "foo.corp"));
        assert!(should_bypass_parent_proxy(&r, "corp"));
        // CIDR
        assert!(should_bypass_parent_proxy(&r, "10.9.9.9"));
        assert!(!should_bypass_parent_proxy(&r, "other.net"));
        // star = bypass all
        let all = resolve_parent_proxy(None, &env(&[("HTTP_PROXY", "http://up:3128"), ("NO_PROXY", "*")])).unwrap();
        assert!(should_bypass_parent_proxy(&all, "anything.net"));
    }

    #[test]
    fn select_url_https_prefers_https_http_only_uses_http() {
        let r = resolve_parent_proxy(None, &env(&[("HTTPS_PROXY", "http://sec:3128"), ("HTTP_PROXY", "http://plain:3128")])).unwrap();
        assert_eq!(select_parent_proxy_url(&r, true).unwrap().as_str(), "http://sec:3128/");
        assert_eq!(select_parent_proxy_url(&r, false).unwrap().as_str(), "http://plain:3128/");
    }
}
```

- [ ] **Step 3: Verify fail**, then **implement** `parent_proxy.rs`:

```rust
//! Parent/upstream proxy resolution + `NO_PROXY` bypass (pure port of
//! `parent-proxy.js:45-212`). Reads env via an injected map so the resolver
//! stays pure/testable. CIDR matching via `ipnet`.

use std::collections::HashMap;

use ipnet::IpNet;
use url::Url;

/// Parsed `NO_PROXY` ruleset (`parseNoProxy`, parent-proxy.js:86-148).
#[derive(Debug, Clone, Default)]
pub struct NoProxy {
    /// `NO_PROXY=*` — bypass everything.
    pub all: bool,
    /// Hostname suffixes (normalized: lowercased, `*.`/`:port` stripped).
    pub suffixes: Vec<String>,
    /// CIDR subnets + exact IP literals (stored as /32 or /128).
    pub cidr: Vec<IpNet>,
}

/// Resolved parent-proxy config (`resolveParentProxy` result).
#[derive(Debug, Clone)]
pub struct ResolvedParentProxy {
    /// Proxy URL for plain HTTP destinations (`None` ⇒ direct).
    pub http_url: Option<Url>,
    /// Proxy URL for HTTPS destinations (falls back to HTTP_PROXY).
    pub https_url: Option<Url>,
    /// `NO_PROXY` bypass ruleset.
    pub no_proxy: NoProxy,
}

/// Explicit config overrides (mirrors the TS `cfg` arg). `None` fields fall
/// back to env.
#[derive(Debug, Clone, Default)]
pub struct ParentProxyConfig {
    /// Override for `HTTP_PROXY`.
    pub http: Option<String>,
    /// Override for `HTTPS_PROXY`.
    pub https: Option<String>,
    /// Override for `NO_PROXY`.
    pub no_proxy: Option<String>,
}

fn env_first<'a>(env: &'a HashMap<String, String>, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| env.get(*k).map(String::as_str))
}

/// Parse a proxy URL, accepting schemeless `host:port` (curl-style), rejecting
/// any non-http/https scheme or empty host. (`resolveParentProxy`'s `parse`.)
fn parse_proxy_url(u: &str) -> Option<Url> {
    let has_scheme = u.split_once("://").is_some_and(|(s, _)| {
        let mut cs = s.chars();
        cs.next().is_some_and(|c| c.is_ascii_alphabetic())
            && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
    });
    let with_scheme = if has_scheme { u.to_string() } else { format!("http://{u}") };
    let parsed = Url::parse(&with_scheme).ok()?;
    if (parsed.scheme() != "http" && parsed.scheme() != "https") || parsed.host_str().is_none() {
        return None;
    }
    Some(parsed)
}

/// `resolveParentProxy` (parent-proxy.js:45-84). `None` if neither HTTP nor
/// HTTPS proxy is configured (after parsing).
#[must_use]
pub fn resolve_parent_proxy(
    cfg: Option<&ParentProxyConfig>,
    env: &HashMap<String, String>,
) -> Option<ResolvedParentProxy> {
    let http = cfg
        .and_then(|c| c.http.clone())
        .or_else(|| env_first(env, &["HTTP_PROXY", "http_proxy"]).map(String::from));
    let https = cfg
        .and_then(|c| c.https.clone())
        .or_else(|| env_first(env, &["HTTPS_PROXY", "https_proxy"]).map(String::from))
        .or_else(|| http.clone()); // HTTPS falls back to HTTP_PROXY (curl behaviour)
    let no_proxy_raw = cfg
        .and_then(|c| c.no_proxy.clone())
        .or_else(|| env_first(env, &["NO_PROXY", "no_proxy"]).map(String::from))
        .unwrap_or_default();
    if http.is_none() && https.is_none() {
        return None;
    }
    let http_url = http.as_deref().and_then(parse_proxy_url);
    let https_url = https.as_deref().and_then(parse_proxy_url);
    if http_url.is_none() && https_url.is_none() {
        return None;
    }
    Some(ResolvedParentProxy {
        http_url,
        https_url,
        no_proxy: parse_no_proxy(&no_proxy_raw),
    })
}

/// `parseNoProxy` (parent-proxy.js:86-148).
#[must_use]
pub fn parse_no_proxy(raw: &str) -> NoProxy {
    let mut rules = NoProxy::default();
    for entry in raw.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        if entry == "*" {
            rules.all = true;
            continue;
        }
        if let Some(slash) = entry.find('/') {
            // CIDR (ignore malformed; never fall through to suffix).
            if let Ok(net) = entry.parse::<IpNet>() {
                rules.cidr.push(net);
            } else {
                let _ = slash;
            }
            continue;
        }
        let mut v = entry.to_ascii_lowercase();
        // `[v6]:port` → v6
        if let Some(inner) = v.strip_prefix('[').and_then(|s| s.split(']').next()) {
            v = inner.to_string();
        }
        if let Some(stripped) = v.strip_prefix("*.") {
            v = format!(".{stripped}"); // TS slices off the `*`, leaving `.suffix`
        }
        if let Ok(ip) = v.parse::<std::net::IpAddr>() {
            // Bare IP literal → exact /32 or /128.
            let net = match ip {
                std::net::IpAddr::V4(a) => IpNet::from(ipnet::Ipv4Net::new(a, 32).unwrap()),
                std::net::IpAddr::V6(a) => IpNet::from(ipnet::Ipv6Net::new(a, 128).unwrap()),
            };
            rules.cidr.push(net);
            continue;
        }
        // Strip a trailing `:port` (non-IP only).
        if let Some(colon) = v.rfind(':') {
            if v[colon + 1..].chars().all(|c| c.is_ascii_digit()) && colon + 1 < v.len() {
                v.truncate(colon);
            }
        }
        rules.suffixes.push(v);
    }
    rules
}

/// `shouldBypassParentProxy` (parent-proxy.js:160-188). Loopback always
/// bypasses; then `*`, CIDR, then golang-suffix semantics.
#[must_use]
pub fn should_bypass_parent_proxy(resolved: &ResolvedParentProxy, host: &str) -> bool {
    let h = crate::host::strip_brackets(&host.to_ascii_lowercase().trim_end_matches('.').to_string());
    if h == "localhost" {
        return true;
    }
    if let Ok(ip) = h.parse::<std::net::IpAddr>() {
        if is_loopback(ip) {
            return true;
        }
        if resolved.no_proxy.all {
            return true;
        }
        if resolved.no_proxy.cidr.iter().any(|net| net.contains(&ip)) {
            return true;
        }
        // IP host: suffix rules don't apply.
        return false;
    }
    if resolved.no_proxy.all {
        return true;
    }
    for v in &resolved.no_proxy.suffixes {
        if let Some(bare) = v.strip_prefix('.') {
            // `.example.com` matches `foo.example.com` AND `example.com`
            if h == bare || h.ends_with(v.as_str()) {
                return true;
            }
        } else {
            // `example.com` matches `example.com` AND `foo.example.com`
            if &h == v || h.ends_with(&format!(".{v}")) {
                return true;
            }
        }
    }
    false
}

/// Loopback set (`LOOPBACK`, parent-proxy.js:190-196): 127/8 + ::1 + v4-mapped.
fn is_loopback(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(a) => a.octets()[0] == 127,
        std::net::IpAddr::V6(a) => {
            a == std::net::Ipv6Addr::LOCALHOST
                || a.to_ipv4_mapped().is_some_and(|v4| v4.octets()[0] == 127)
        }
    }
}

/// `selectParentProxyUrl` (parent-proxy.js:201-209): HTTPS prefers
/// `https_url` then `http_url`; plain HTTP only uses `http_url`.
#[must_use]
pub fn select_parent_proxy_url(resolved: &ResolvedParentProxy, is_https: bool) -> Option<&Url> {
    if is_https {
        resolved.https_url.as_ref().or(resolved.http_url.as_ref())
    } else {
        resolved.http_url.as_ref()
    }
}

/// Test-only CIDR membership helper.
#[cfg(test)]
fn cidr_contains(r: &NoProxy, ip: &str) -> bool {
    let addr: std::net::IpAddr = ip.parse().unwrap();
    r.cidr.iter().any(|net| net.contains(&addr))
}
```

NOTE for the implementer: verify `IpNet::contains` mixed-family behavior (an IPv4 addr against an IPv6 subnet must be false, not panic) — `ipnet` handles this. Also verify the `*.` → `.suffix` normalization matches the TS (`v.slice(1)` turns `*.example.com` into `.example.com`). And the golang suffix semantics in the tests must pass exactly. If `url::Url::parse` normalizes the proxy URL differently than `http://up:3128/` (e.g. trailing slash), adjust the test's expected `.as_str()` to the ACTUAL url output (it appends `/` for a path-less URL — the tests already expect the trailing slash).

- [ ] **Step 4: Run `cargo test -p sandbox-runtime` → PASS. Add `pub mod parent_proxy;` to lib.rs. Gate + commit** (`feat(sandbox-netfilter): parent-proxy resolution + NO_PROXY bypass (P2)`). Clippy `-p sandbox-runtime`.

---

### Task 2: final gates

- [ ] **Step 1:**

```bash
cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-code
cargo test -p sandbox-runtime
cargo clippy -p sandbox-runtime --all-targets --no-deps -- -D warnings
cargo test --workspace --no-run
cargo tree -p engine-mobile -e normal | grep -c "sandbox-runtime"  # 0
```

- [ ] **Step 2: Frozen check** `git diff main -- lingxi-code/traits lingxi-code/protocol` → empty.

## Final verification
1. `cargo test -p sandbox-runtime` green (P1's 8 + P2's 4).
2. engine-mobile 0 sandbox-netfilter; frozen empty.

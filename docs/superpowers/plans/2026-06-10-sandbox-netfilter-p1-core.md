# Sandbox Net-Filter P1 — Pure Core (domain matcher + host primitives + config)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox (`- [ ]`).

**Goal:** Port the PURE foundation of the sandbox network-filtering subsystem — the domain pattern grammar, `matches_domain_pattern`, `filter_network_request` (deny-first/allow/canonicalize/empty=deny-all), the `is_valid_host`/`canonicalize_host` host primitives, and the network config types — into a new isolated `sandbox-netfilter` crate. No proxy, no async, no bwrap — all unit-testable on macOS.

**Architecture:** New leaf crate `lingxi-code/sandbox-netfilter/` (isolates the subsystem's deps from the lean `sandbox` crate + engine-mobile). P1 is pure functions + types; later sub-projects (P3 proxy, P6 MITM) add tokio/rustls here.

**Tech Stack:** Rust 1.82 / edition 2021, `serde`, `url` (2.5, WHATWG host canonicalization — already in lock), `std::net` (IP literals). No new heavy deps in P1.

**Spec:** umbrella `docs/superpowers/specs/2026-06-10-sandbox-netfilter-umbrella-design.md`. Reference of truth: `docs/superpowers/references/sandbox-runtime-0.0.54/dist/sandbox/{sandbox-manager.js,sandbox-config.js,parent-proxy.js}`.

**Branch:** `parity-sandbox-netfilter` (created; umbrella spec committed).

**Conventions:** Cargo root `lingxi-code/`; git from repo root with `lingxi-code/...` paths. `-D missing-docs` + clippy pedantic `-D warnings`. Commit `git commit -F <file>`, footer `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. No `cargo test --workspace` runtime.

---

### Task 1: crate scaffold + config types

**Files:** Modify `lingxi-code/Cargo.toml` (members + default-members); Create `lingxi-code/sandbox-netfilter/Cargo.toml`, `src/lib.rs`, `src/config.rs`.

- [ ] **Step 1: Workspace member.** Add `"sandbox-netfilter",` to BOTH `[workspace] members` and `default-members` (after `"sandbox",` in each).

- [ ] **Step 2: Manifest** `sandbox-netfilter/Cargo.toml`:

```toml
[package]
name = "sandbox-netfilter"
version = "0.12.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[dependencies]
serde = { workspace = true, features = ["derive"] }
serde_json.workspace = true
url = "2.5"

[dev-dependencies]
# (P3+ add tokio etc.)

[lints]
workspace = true
```

- [ ] **Step 3: lib.rs** (`#![forbid(unsafe_code)]` per workspace convention):

```rust
//! Sandbox network-filtering subsystem (Linux bwrap `--unshare-net` + host
//! forward-proxy domain allowlisting). Faithful port of
//! `@anthropic-ai/sandbox-runtime@0.0.54` (vendored at
//! `docs/superpowers/references/sandbox-runtime-0.0.54/`).
//!
//! P1 (this): the pure core — domain pattern grammar, the host matcher, the
//! host validators/canonicalizers, and the network config. No proxy/async/bwrap.

#![forbid(unsafe_code)]

pub mod config;
pub mod host;
pub mod matcher;
```

- [ ] **Step 4: Config types + pattern validation — failing test** in `config.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pattern_grammar_matches_ts() {
        // sandbox-config.js:11-43 domainPatternSchema
        assert!(is_valid_domain_pattern("localhost"));
        assert!(is_valid_domain_pattern("example.com"));
        assert!(is_valid_domain_pattern("api.example.com"));
        assert!(is_valid_domain_pattern("*.example.com"));
        // rejected: protocol/path/port
        assert!(!is_valid_domain_pattern("https://example.com"));
        assert!(!is_valid_domain_pattern("example.com/path"));
        assert!(!is_valid_domain_pattern("example.com:443"));
        // rejected: too-broad wildcards
        assert!(!is_valid_domain_pattern("*.com"));
        assert!(!is_valid_domain_pattern("*"));
        assert!(!is_valid_domain_pattern("*."));
        assert!(!is_valid_domain_pattern("ex*ample.com"));
        // rejected: no dot / leading-trailing dot
        assert!(!is_valid_domain_pattern("nodot"));
        assert!(!is_valid_domain_pattern(".example.com"));
        assert!(!is_valid_domain_pattern("example.com."));
    }
}
```

- [ ] **Step 5: Verify fail**, then **implement** `config.rs`:

```rust
//! Network config + domain-pattern validation. Ports `NetworkConfigSchema`
//! and `domainPatternSchema` (`sandbox-config.js:11-100`).

use serde::{Deserialize, Serialize};

/// Allow/deny domain lists. Empty `allowed_domains` = DENY-ALL (the netns is
/// unshared and no proxy sockets are bound) — NOT allow-all
/// (`sandbox-manager.js:590-599`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkConfig {
    /// Hostname patterns permitted (deny-first, so a denied pattern still wins).
    #[serde(default)]
    pub allowed_domains: Vec<String>,
    /// Hostname patterns always denied (checked before `allowed_domains`).
    #[serde(default)]
    pub denied_domains: Vec<String>,
}

/// Validate a domain pattern (`domainPatternSchema`, `sandbox-config.js:11-43`):
/// `localhost` | `*.dom.tld` (≥2 non-empty labels after `*.`) | exact `dom.tld`
/// (contains a dot, no leading/trailing dot). No protocol/path/port; no other
/// wildcard use.
#[must_use]
pub fn is_valid_domain_pattern(val: &str) -> bool {
    if val.contains("://") || val.contains('/') || val.contains(':') {
        return false;
    }
    if val == "localhost" {
        return true;
    }
    if let Some(domain) = val.strip_prefix("*.") {
        if !domain.contains('.') || domain.starts_with('.') || domain.ends_with('.') {
            return false;
        }
        let parts: Vec<&str> = domain.split('.').collect();
        return parts.len() >= 2 && parts.iter().all(|p| !p.is_empty());
    }
    if val.contains('*') {
        return false;
    }
    val.contains('.') && !val.starts_with('.') && !val.ends_with('.')
}
```

- [ ] **Step 6: Run `cargo test -p sandbox-netfilter` → PASS. Gate + commit** (`feat(sandbox-netfilter): crate scaffold + NetworkConfig + domain-pattern grammar`). Clippy `-p sandbox-netfilter`.

---

### Task 2: host primitives (`host.rs`)

**Files:** Create `lingxi-code/sandbox-netfilter/src/host.rs`.

- [ ] **Step 1: Failing tests:**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_valid_host_rejects_injection_and_zone_ids() {
        // parent-proxy.js:372-384
        assert!(is_valid_host("example.com"));
        assert!(is_valid_host("a.b-c_d.example.com"));
        assert!(is_valid_host("127.0.0.1"));
        assert!(is_valid_host("::1"));
        assert!(!is_valid_host(""));
        assert!(!is_valid_host(&"a".repeat(256)));
        assert!(!is_valid_host("evil.com\u{0}.allowed.com")); // null byte
        assert!(!is_valid_host("evil.com\r\n.allowed.com"));  // CRLF
        assert!(!is_valid_host("fe80::1%eth0"));               // zone id
        assert!(!is_valid_host("has space.com"));
    }

    #[test]
    fn strip_brackets_unwraps_ipv6() {
        assert_eq!(strip_brackets("[::1]"), "::1");
        assert_eq!(strip_brackets("example.com"), "example.com");
    }

    #[test]
    fn canonicalize_host_matches_getaddrinfo() {
        // parent-proxy.js:396-410 — inet_aton shorthand, hex/octal, ipv6, trailing dot
        assert_eq!(canonicalize_host("127.1").as_deref(), Some("127.0.0.1"));
        assert_eq!(canonicalize_host("2130706433").as_deref(), Some("127.0.0.1"));
        assert_eq!(canonicalize_host("0x7f.0.0.1").as_deref(), Some("127.0.0.1"));
        assert_eq!(canonicalize_host("0:0:0:0:0:0:0:1").as_deref(), Some("::1"));
        assert_eq!(canonicalize_host("Example.COM.").as_deref(), Some("example.com"));
        assert_eq!(canonicalize_host("[::1]").as_deref(), Some("::1"));
        assert!(canonicalize_host("evil\u{0}.com").is_none());
    }
}
```

- [ ] **Step 2: Verify fail**, then **implement** `host.rs`:

```rust
//! Host validators + canonicalizers (`parent-proxy.js:372-410`). Security-
//! critical: they make the allowlist agree with what `getaddrinfo()` dials
//! (denylist-evasion defense) and reject CRLF/null/zone-id injection.

use std::net::IpAddr;

/// Strip surrounding `[ ]` from a bracketed IPv6 literal.
#[must_use]
pub fn strip_brackets(h: &str) -> String {
    h.strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(h)
        .to_string()
}

/// True if `h` parses as an IP literal.
fn is_ip(h: &str) -> bool {
    h.parse::<IpAddr>().is_ok()
}

/// `isValidHost` (`parent-proxy.js:372-384`): non-empty, ≤255 chars; reject
/// zone IDs (`%`); accept IP literals; else require the DNS label charset
/// `[A-Za-z0-9._-]+` (underscore allowed for `_dmarc` etc.). This rejects
/// control chars / CRLF / null / spaces.
#[must_use]
pub fn is_valid_host(h: &str) -> bool {
    if h.is_empty() || h.len() > 255 {
        return false;
    }
    let bare = strip_brackets(h);
    if bare.contains('%') {
        return false;
    }
    if is_ip(&bare) {
        return true;
    }
    bare.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// `canonicalizeHost` (`parent-proxy.js:396-410`): normalize via the WHATWG URL
/// parser so allowlist comparisons match `getaddrinfo()` — collapses inet_aton
/// shorthand, hex/octal octets, IPv6 compression, trailing dots, case,
/// brackets. `None` if the input is not a valid URL host.
#[must_use]
pub fn canonicalize_host(h: &str) -> Option<String> {
    let bare = strip_brackets(h);
    // WHATWG parses bare IPv6 only when bracketed.
    let bracketed = if matches!(bare.parse::<IpAddr>(), Ok(IpAddr::V6(_))) {
        format!("[{bare}]")
    } else {
        bare.clone()
    };
    let url = url::Url::parse(&format!("http://{bracketed}/")).ok()?;
    let host = url.host_str()?;
    Some(strip_brackets(host).trim_end_matches('.').to_string())
}
```

NOTE for the implementer: verify the `url` crate's IPv4 normalization matches the test expectations IN PRACTICE (`127.1`→`127.0.0.1`, `2130706433`→`127.0.0.1`, `0x7f.0.0.1`→`127.0.0.1`). The `url` crate implements the WHATWG IPv4 parser which does these. If any case differs (e.g. `url` rejects a form Node accepts or vice-versa), adjust the test to the ACTUAL faithful-to-getaddrinfo behavior and DOCUMENT the divergence in a comment — the security property is "canonical form matches what the OS dials," and `url`'s WHATWG parser is the same standard Node's URL uses. Report any divergence.

- [ ] **Step 3: Run → PASS. Gate + commit** (`feat(sandbox-netfilter): is_valid_host + canonicalize_host host primitives`).

---

### Task 3: the matcher (`matcher.rs`)

**Files:** Create `lingxi-code/sandbox-netfilter/src/matcher.rs`.

- [ ] **Step 1: Failing tests:**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::NetworkConfig;

    #[test]
    fn wildcard_and_exact_matching() {
        // sandbox-manager.js:46-61
        assert!(matches_domain_pattern("a.example.com", "*.example.com"));
        assert!(matches_domain_pattern("a.b.example.com", "*.example.com"));
        assert!(!matches_domain_pattern("example.com", "*.example.com")); // bare doesn't match wildcard
        assert!(matches_domain_pattern("Example.COM", "example.com"));    // case-insensitive exact
        assert!(!matches_domain_pattern("evil.com", "example.com"));
        // wildcard never matches an IP literal
        assert!(!matches_domain_pattern("1.2.3.4", "*.3.4"));
    }

    #[test]
    fn deny_precedes_allow_and_empty_is_deny_all() {
        // empty allowed = deny-all
        let empty = NetworkConfig::default();
        assert!(!filter_network_request(443, "example.com", &empty));
        // allow works
        let allow = NetworkConfig { allowed_domains: vec!["*.example.com".into()], denied_domains: vec![] };
        assert!(filter_network_request(443, "api.example.com", &allow));
        assert!(!filter_network_request(443, "other.com", &allow));
        // deny precedes allow
        let both = NetworkConfig {
            allowed_domains: vec!["*.example.com".into()],
            denied_domains: vec!["evil.example.com".into()],
        };
        assert!(filter_network_request(443, "ok.example.com", &both));
        assert!(!filter_network_request(443, "evil.example.com", &both));
    }

    #[test]
    fn canonicalization_defeats_denylist_evasion() {
        // a denylist for the dotted form must catch inet_aton shorthand
        let cfg = NetworkConfig {
            allowed_domains: vec![],
            denied_domains: vec!["169.254.169.254".into()],
        };
        // even though allowed is empty (deny-all), assert the deny path canonicalizes:
        // the allow case — allow everything, deny the metadata IP in shorthand
        let cfg2 = NetworkConfig {
            allowed_domains: vec!["*.example.com".into(), "2852039166".into()], // not a valid pattern, but host side
            denied_domains: vec!["169.254.169.254".into()],
        };
        let _ = cfg;
        // 2852039166 == 169.254.169.254 ; the denylist (dotted) must catch the shorthand host
        assert!(!filter_network_request(80, "2852039166", &cfg2));
    }

    #[test]
    fn malformed_host_denied() {
        let allow_all = NetworkConfig { allowed_domains: vec!["*.example.com".into()], denied_domains: vec![] };
        assert!(!filter_network_request(443, "evil.com\u{0}.example.com", &allow_all));
    }
}
```

- [ ] **Step 2: Verify fail**, then **implement** `matcher.rs`:

```rust
//! The allow/deny brain (`sandbox-manager.js:46-119`). `filter_network_request`
//! is the synchronous, ask-callback-free core (the interactive
//! `sandboxAskCallback` branch is a later wiring concern; absent ⇒ deny, which
//! is the unmatched default anyway).

use crate::config::NetworkConfig;
use crate::host::{canonicalize_host, is_valid_host, strip_brackets};

/// `matchesDomainPattern` (`sandbox-manager.js:46-61`): `*.base` ⇒
/// `host.ends_with(".base")` (never for IP literals); else case-insensitive
/// exact equality.
#[must_use]
pub fn matches_domain_pattern(hostname: &str, pattern: &str) -> bool {
    let h = hostname.to_ascii_lowercase();
    if let Some(base) = pattern.strip_prefix("*.") {
        if strip_brackets(&h).parse::<std::net::IpAddr>().is_ok() {
            return false;
        }
        let base = base.to_ascii_lowercase();
        return h.ends_with(&format!(".{base}"));
    }
    h == pattern.to_ascii_lowercase()
}

/// `filterNetworkRequest` (`sandbox-manager.js:62-119`) without the async
/// ask-callback: reject malformed hosts, canonicalize, deny-first, then allow,
/// else deny. Empty `allowed_domains` ⇒ deny-all.
#[must_use]
pub fn filter_network_request(port: u16, host: &str, config: &NetworkConfig) -> bool {
    let _ = port; // not part of the decision (patterns are hostname-only); kept for signature parity
    if !is_valid_host(host) {
        return false;
    }
    let canonical = canonicalize_host(host).unwrap_or_else(|| host.to_string());
    for denied in &config.denied_domains {
        if matches_domain_pattern(&canonical, denied) {
            return false;
        }
    }
    for allowed in &config.allowed_domains {
        if matches_domain_pattern(&canonical, allowed) {
            return true;
        }
    }
    false
}
```

(NOTE: the TS `filterNetworkRequest` is `async` with the ask-callback. P1 ports the synchronous decision core — the ask-callback is an interactive-wiring concern for a later sub-project; in its absence TS denies the unmatched case, identical to this. Document that on the fn.)

- [ ] **Step 3: Run → PASS. Gate + commit** (`feat(sandbox-netfilter): filter_network_request matcher (deny-first, empty=deny-all)`).

---

### Task 4: final gates

- [ ] **Step 1: Full gate ritual.**

```bash
cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-code
cargo test -p sandbox-netfilter
cargo clippy -p sandbox-netfilter --all-targets --no-deps -- -D warnings
cargo test --workspace --no-run        # struct-trap (the new crate compiles into the graph)
cargo build -p engine-mobile
cargo tree -p engine-mobile -e normal | grep -c "sandbox-netfilter"  # MUST be 0 (mobile pulls none of it)
```

- [ ] **Step 2: Frozen check.** `git diff main -- lingxi-code/traits lingxi-code/protocol` → empty.

- [ ] **Step 3: Commit** any gate fixups (`chore(sandbox-netfilter): P1 gate pass`).

## Final verification

1. `cargo test -p sandbox-netfilter` green (expect ~10 tests).
2. engine-mobile pulls ZERO `sandbox-netfilter`.
3. Frozen surfaces empty diff.
4. Memory: note P1 done; P2-P7 remain (umbrella spec lists them).

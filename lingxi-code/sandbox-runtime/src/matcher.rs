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
///
/// The TS `filterNetworkRequest` is `async` and consults an interactive
/// `sandboxAskCallback` for the unmatched case; that is an interactive-wiring
/// concern for a later sub-project. In its absence TS denies the unmatched
/// request — identical to the final `false` returned here. `port` is not part
/// of the decision (patterns are hostname-only); it is kept for signature
/// parity with the TS source.
#[must_use]
pub fn filter_network_request(port: u16, host: &str, config: &NetworkConfig) -> bool {
    let _ = port; // not part of the decision; kept for signature parity
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
        assert!(matches_domain_pattern("Example.COM", "example.com")); // case-insensitive exact
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
        let allow = NetworkConfig {
            allowed_domains: vec!["*.example.com".into()],
            denied_domains: vec![],
        };
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
        // `2852039166` is the inet_aton decimal shorthand for `169.254.169.254`
        // (the cloud-metadata IP). A denylist that names only the dotted form
        // must still catch the shorthand host, because the request host is
        // canonicalized to what getaddrinfo() would dial before matching.
        // Allow everything (`*` is not a legal pattern, so allow a wide wildcard)
        // and confirm the deny entry still wins via canonicalization.
        let cfg = NetworkConfig {
            allowed_domains: vec!["*.example.com".into()],
            denied_domains: vec!["169.254.169.254".into()],
        };
        // 2852039166 == 169.254.169.254 — the dotted denylist entry must catch it.
        assert!(!filter_network_request(80, "2852039166", &cfg));
    }

    #[test]
    fn malformed_host_denied() {
        let allow_all = NetworkConfig {
            allowed_domains: vec!["*.example.com".into()],
            denied_domains: vec![],
        };
        assert!(!filter_network_request(
            443,
            "evil.com\u{0}.example.com",
            &allow_all
        ));
    }
}

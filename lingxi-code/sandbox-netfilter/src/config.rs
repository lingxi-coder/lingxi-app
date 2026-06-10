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

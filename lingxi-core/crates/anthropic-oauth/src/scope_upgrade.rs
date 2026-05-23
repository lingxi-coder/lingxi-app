//! OAuth scope upgrade: 403-with-`required_scopes` re-triggers PKCE preserving
//! the existing refresh_token. See spec §3 / §8 M3-04 phase 4.
//!
//! M3-04 populates this in Task 7.

#![allow(dead_code)]

use serde::Deserialize;

/// Decoded shape of a 403 response body announcing a required scope upgrade.
#[derive(Debug, Clone, Deserialize)]
pub struct ScopeUpgradeRequired {
    /// Scopes the provider now requires.
    pub required: Vec<String>,
    /// Scopes currently granted on the token.
    pub granted: Vec<String>,
}

/// Parse a 403 body for `required_scopes`. Returns `None` if the body is not
/// a scope-upgrade signal (other 403 reasons exist).
#[must_use]
pub fn parse_scope_upgrade(body: &str) -> Option<ScopeUpgradeRequired> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let required = v.get("required_scopes")?.as_array()?;
    let granted = v.get("granted_scopes").and_then(|g| g.as_array()).cloned().unwrap_or_default();
    let required: Vec<String> = required
        .iter()
        .filter_map(|s| s.as_str().map(str::to_string))
        .collect();
    let granted: Vec<String> = granted
        .iter()
        .filter_map(|s| s.as_str().map(str::to_string))
        .collect();
    Some(ScopeUpgradeRequired { required, granted })
}

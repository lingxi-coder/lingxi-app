//! Managed plugin marketplace policy shared by every CLI plugin flow.
//!
//! Policy is evaluated against a structured source identity before any remote
//! fetch, clone, archive extraction, or cache write. `blockedMarketplaces`
//! always wins. An absent `strictKnownMarketplaces` is unrestricted, while an
//! explicitly empty array blocks every marketplace.

use std::collections::BTreeSet;
use std::fmt;
use std::path::PathBuf;

use serde_json::Value;

/// Canonical identity of a marketplace source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarketplaceSourceIdentity {
    /// GitHub `owner/repository`, optionally pinned to a ref and subdirectory.
    Github {
        repo: String,
        git_ref: Option<String>,
        path: Option<String>,
    },
    /// Arbitrary git URL, optionally pinned to a ref and subdirectory.
    Git {
        url: String,
        git_ref: Option<String>,
        path: Option<String>,
    },
    /// Hosted marketplace JSON URL.
    Url { url: String },
    /// npm package source.
    Npm { package: String },
    /// Local marketplace JSON file.
    File { path: String },
    /// Local marketplace directory.
    Directory { path: String },
}

impl MarketplaceSourceIdentity {
    /// Parse a catalog/registry source object. Unknown shapes fail closed.
    pub fn from_value(value: &Value) -> Option<Self> {
        let value = value
            .get("source")
            .filter(|nested| nested.is_object())
            .unwrap_or(value);
        let object = value.as_object()?;
        let kind = object.get("source")?.as_str()?;
        let string = |key: &str| {
            object
                .get(key)
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        };
        match kind {
            "github" => Some(Self::Github {
                repo: string("repo")?,
                git_ref: string("ref"),
                path: string("path"),
            }),
            "git" => Some(Self::Git {
                url: string("url")?,
                git_ref: string("ref"),
                path: string("path"),
            }),
            "url" => Some(Self::Url {
                url: string("url")?,
            }),
            "npm" => Some(Self::Npm {
                package: string("package").or_else(|| string("name"))?,
            }),
            "file" => Some(Self::File {
                path: normalize_path(&string("path")?),
            }),
            "directory" => Some(Self::Directory {
                path: normalize_path(&string("path")?),
            }),
            _ => None,
        }
    }

    fn host_path(&self) -> (String, String) {
        match self {
            Self::Github { repo, path, .. } => (
                "github.com".to_string(),
                format!("/{}{}", repo, path_suffix(path.as_deref())),
            ),
            Self::Git { url, path, .. } => {
                let (host, mut source_path) = split_host_path(url);
                source_path.push_str(&path_suffix(path.as_deref()));
                (host, source_path)
            }
            Self::Url { url } => split_host_path(url),
            Self::Npm { package } => ("npm".to_string(), package.clone()),
            Self::File { path } | Self::Directory { path } => (String::new(), normalize_path(path)),
        }
    }
}

fn path_suffix(path: Option<&str>) -> String {
    path.filter(|path| !path.is_empty())
        .map(|path| format!("/{}", path.trim_start_matches('/')))
        .unwrap_or_default()
}

fn normalize_path(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    if normalized == "/" {
        normalized
    } else {
        normalized.trim_end_matches('/').to_string()
    }
}

fn split_host_path(locator: &str) -> (String, String) {
    if let Some(rest) = locator
        .strip_prefix("https://")
        .or_else(|| locator.strip_prefix("http://"))
    {
        let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let authority = &rest[..end];
        let host = authority
            .rsplit('@')
            .next()
            .unwrap_or(authority)
            .split(':')
            .next()
            .unwrap_or(authority)
            .to_ascii_lowercase();
        let path = &rest[end..];
        let path_end = path.find(['?', '#']).unwrap_or(path.len());
        return (host, path[..path_end].to_string());
    }
    if let Some((_, rest)) = locator.split_once('@') {
        if let Some((host, path)) = rest.split_once(':') {
            return (host.to_ascii_lowercase(), format!("/{path}"));
        }
    }
    (String::new(), locator.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum MarketplaceRule {
    Name(String),
    Exact(MarketplaceSourceIdentity),
    Pattern {
        host_pattern: Option<String>,
        path_pattern: Option<String>,
    },
}

impl MarketplaceRule {
    fn parse(value: &Value) -> Option<Self> {
        if let Some(name) = value.as_str().filter(|name| !name.is_empty()) {
            return Some(Self::Name(name.to_string()));
        }
        let object = value.as_object()?;
        let host_pattern = object
            .get("hostPattern")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let path_pattern = object
            .get("pathPattern")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        if host_pattern.is_some() || path_pattern.is_some() {
            return Some(Self::Pattern {
                host_pattern,
                path_pattern,
            });
        }
        if let Some(name) = object
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
        {
            return Some(Self::Name(name.to_string()));
        }
        MarketplaceSourceIdentity::from_value(value).map(Self::Exact)
    }

    fn matches(&self, name: Option<&str>, source: Option<&MarketplaceSourceIdentity>) -> bool {
        match self {
            Self::Name(expected) => name == Some(expected.as_str()),
            Self::Exact(expected) => source == Some(expected),
            Self::Pattern {
                host_pattern,
                path_pattern,
            } => source.is_some_and(|source| {
                let (host, path) = source.host_path();
                host_pattern
                    .as_deref()
                    .is_none_or(|pattern| wildcard_matches(pattern, &host))
                    && path_pattern
                        .as_deref()
                        .is_none_or(|pattern| wildcard_matches(pattern, &path))
            }),
        }
    }

    fn is_name_only(&self) -> bool {
        matches!(self, Self::Name(_))
    }
}

/// Why a marketplace operation was rejected before side effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarketplacePolicyBlockReason {
    /// A deny rule matched the name or source.
    Blocked,
    /// A strict allowlist exists and no rule matched.
    NotKnown,
}

impl fmt::Display for MarketplacePolicyBlockReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Blocked => formatter.write_str("blocked by managed settings"),
            Self::NotKnown => formatter.write_str("not allowed by strictKnownMarketplaces"),
        }
    }
}

/// Effective managed marketplace policy.
#[derive(Debug, Clone, Default)]
pub struct MarketplacePolicy {
    strict_known: Option<Vec<MarketplaceRule>>,
    blocked: Vec<MarketplaceRule>,
}

impl MarketplacePolicy {
    /// Load policy with last-write-wins semantics for each managed field.
    #[must_use]
    pub fn from_managed_settings() -> Self {
        Self::from_raw_tiers(managed_settings_raw_tiers())
    }

    fn from_raw_tiers(tiers: impl IntoIterator<Item = String>) -> Self {
        let mut policy = Self::default();
        for raw in tiers {
            let Ok(value) = serde_json::from_str::<Value>(&raw) else {
                continue;
            };
            let strict = value
                .get("strictKnownMarketplaces")
                .and_then(Value::as_array);
            let allowed_alias = value.get("allowedMarketplaces").and_then(Value::as_array);
            if strict.is_some() && allowed_alias.is_some() {
                tracing::warn!(
                    "allowedMarketplaces is ignored because strictKnownMarketplaces is also set"
                );
            }
            if let Some(entries) = strict.or(allowed_alias) {
                policy.strict_known =
                    Some(entries.iter().filter_map(MarketplaceRule::parse).collect());
            }
            if let Some(entries) = value.get("blockedMarketplaces").and_then(Value::as_array) {
                policy.blocked = entries.iter().filter_map(MarketplaceRule::parse).collect();
            }
        }
        policy
    }

    /// Evaluate a marketplace name and/or structured source identity.
    pub fn check(
        &self,
        name: Option<&str>,
        source: Option<&MarketplaceSourceIdentity>,
    ) -> Result<(), MarketplacePolicyBlockReason> {
        if self.blocked.iter().any(|rule| rule.matches(name, source)) {
            return Err(MarketplacePolicyBlockReason::Blocked);
        }
        if self
            .strict_known
            .as_ref()
            .is_some_and(|rules| !rules.iter().any(|rule| rule.matches(name, source)))
        {
            return Err(MarketplacePolicyBlockReason::NotKnown);
        }
        Ok(())
    }

    /// Evaluate the source before its catalog can reveal a marketplace name.
    /// Name-only allow rules are deferred, but source denies and an explicitly
    /// empty strict allowlist still fail before any I/O.
    pub fn check_source_preflight(
        &self,
        source: &MarketplaceSourceIdentity,
    ) -> Result<(), MarketplacePolicyBlockReason> {
        if self
            .blocked
            .iter()
            .any(|rule| rule.matches(None, Some(source)))
        {
            return Err(MarketplacePolicyBlockReason::Blocked);
        }
        if let Some(rules) = &self.strict_known {
            if rules.is_empty() {
                return Err(MarketplacePolicyBlockReason::NotKnown);
            }
            if !rules.iter().any(MarketplaceRule::is_name_only)
                && !rules.iter().any(|rule| rule.matches(None, Some(source)))
            {
                return Err(MarketplacePolicyBlockReason::NotKnown);
            }
        }
        Ok(())
    }

    fn blocked_names(self) -> BTreeSet<String> {
        self.blocked
            .into_iter()
            .filter_map(|rule| match rule {
                MarketplaceRule::Name(name) => Some(name),
                _ => None,
            })
            .collect()
    }
}

fn wildcard_matches(pattern: &str, candidate: &str) -> bool {
    let pattern: Vec<char> = pattern.to_ascii_lowercase().chars().collect();
    let candidate: Vec<char> = candidate.to_ascii_lowercase().chars().collect();
    let mut previous = vec![false; candidate.len() + 1];
    previous[0] = true;
    for token in pattern {
        let mut current = vec![false; candidate.len() + 1];
        if token == '*' {
            current[0] = previous[0];
        }
        for index in 1..=candidate.len() {
            current[index] = match token {
                '*' => previous[index] || current[index - 1],
                '?' => previous[index - 1],
                literal => previous[index - 1] && literal == candidate[index - 1],
            };
        }
        previous = current;
    }
    previous[candidate.len()]
}

/// The managed settings root, honoring the engine's test override.
fn managed_settings_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("LINGXI_MANAGED_DIR").filter(|value| !value.is_empty()) {
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

fn managed_settings_raw_tiers() -> Vec<String> {
    let managed = managed_settings_dir();
    let mut out = Vec::new();
    if let Ok(raw) = std::fs::read_to_string(managed.join("managed-settings.json")) {
        out.push(raw);
    }
    let drop_in = managed.join("managed-settings.d");
    if let Ok(entries) = std::fs::read_dir(&drop_in) {
        let mut names: Vec<std::ffi::OsString> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.file_name())
            .filter(|name| {
                let name = name.to_string_lossy();
                name.ends_with(".json") && !name.starts_with('.')
            })
            .collect();
        names.sort();
        for name in names {
            if let Ok(raw) = std::fs::read_to_string(drop_in.join(name)) {
                out.push(raw);
            }
        }
    }
    out
}

/// Compatibility view used by older call sites and output tests.
#[must_use]
pub fn blocked_marketplaces() -> BTreeSet<String> {
    MarketplacePolicy::from_managed_settings().blocked_names()
}

/// Reject an operation when its name is denied or outside a name allowlist.
pub fn ensure_marketplace_allowed(marketplace: &str) -> Result<(), String> {
    ensure_marketplace_source_allowed(Some(marketplace), None)
}

/// Reject an operation using the complete name/source policy context.
pub fn ensure_marketplace_source_allowed(
    marketplace: Option<&str>,
    source: Option<&MarketplaceSourceIdentity>,
) -> Result<(), String> {
    MarketplacePolicy::from_managed_settings()
        .check(marketplace, source)
        .map_err(|reason| match marketplace {
            Some(name) => format!("Marketplace '{name}' is {reason}"),
            None => format!("Marketplace source is {reason}"),
        })
}

/// Apply the portion of policy knowable before reading/fetching a catalog.
pub fn ensure_marketplace_source_preflight(
    source: &MarketplaceSourceIdentity,
) -> Result<(), String> {
    MarketplacePolicy::from_managed_settings()
        .check_source_preflight(source)
        .map_err(|reason| format!("Marketplace source is {reason}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(raw: &str) -> MarketplacePolicy {
        MarketplacePolicy::from_raw_tiers([raw.to_string()])
    }

    #[test]
    fn absent_strict_policy_is_unrestricted_but_empty_blocks_all() {
        assert!(policy("{}").check(Some("any"), None).is_ok());
        assert!(policy(r#"{"strictKnownMarketplaces":[]}"#)
            .check(Some("any"), None)
            .is_err());
    }

    #[test]
    fn blocked_rule_wins_over_strict_allow() {
        assert!(policy(
            r#"{"strictKnownMarketplaces":["approved"],"blockedMarketplaces":["approved"]}"#
        )
        .check(Some("approved"), None)
        .is_err());
    }

    #[test]
    fn exact_source_includes_ref_and_path() {
        let allowed = MarketplaceSourceIdentity::Github {
            repo: "acme/plugins".to_string(),
            git_ref: Some("v2".to_string()),
            path: Some("catalog".to_string()),
        };
        let wrong_ref = MarketplaceSourceIdentity::Github {
            repo: "acme/plugins".to_string(),
            git_ref: Some("main".to_string()),
            path: Some("catalog".to_string()),
        };
        let policy = policy(
            r#"{"strictKnownMarketplaces":[{"source":"github","repo":"acme/plugins","ref":"v2","path":"catalog"}]}"#,
        );
        assert!(policy.check(None, Some(&allowed)).is_ok());
        assert!(policy.check(None, Some(&wrong_ref)).is_err());
    }

    #[test]
    fn host_and_path_patterns_are_both_required() {
        let allowed = MarketplaceSourceIdentity::Url {
            url: "https://plugins.example.com/team/marketplace.json".to_string(),
        };
        let denied = MarketplaceSourceIdentity::Url {
            url: "https://plugins.example.com/private/marketplace.json".to_string(),
        };
        let policy = policy(
            r#"{"strictKnownMarketplaces":[{"hostPattern":"*.example.com","pathPattern":"/team/*"}]}"#,
        );
        assert!(policy.check(None, Some(&allowed)).is_ok());
        assert!(policy.check(None, Some(&denied)).is_err());
    }

    #[test]
    fn blocked_marketplaces_uses_last_managed_tier() {
        let blocked = MarketplacePolicy::from_raw_tiers([
            r#"{"blockedMarketplaces":["alpha","beta"]}"#.to_string(),
            r#"{"blockedMarketplaces":["gamma"]}"#.to_string(),
        ])
        .blocked_names();
        assert_eq!(blocked, BTreeSet::from(["gamma".to_string()]));
    }

    #[test]
    fn allowed_marketplaces_alias_feeds_strict_policy() {
        let policy = policy(r#"{"allowedMarketplaces":["approved"]}"#);
        assert!(policy.check(Some("approved"), None).is_ok());
        assert!(policy.check(Some("other"), None).is_err());
    }

    #[test]
    fn canonical_strict_marketplaces_wins_over_alias() {
        let policy =
            policy(r#"{"strictKnownMarketplaces":["canonical"],"allowedMarketplaces":["alias"]}"#);
        assert!(policy.check(Some("canonical"), None).is_ok());
        assert!(policy.check(Some("alias"), None).is_err());
    }
}

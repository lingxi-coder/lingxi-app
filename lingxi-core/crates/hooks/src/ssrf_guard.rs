//! SSRF guard for `HookExecutor::Http` requests (spec §9.7).
//!
//! Blocks loopback, RFC1918 private ranges, and non-`http(s)` schemes by
//! default. DNS resolution is intentionally deferred to M2 (platform DNS
//! resolver); the IP-literal check here catches `http://127.0.0.1/` style
//! escape attempts. An optional allow-list of host strings can be supplied
//! to lock outbound traffic down further.

use std::collections::HashSet;
use std::net::IpAddr;
use thiserror::Error;

/// Outbound URL validator used by the HTTP hook executor.
///
/// Use [`Self::with_defaults`] for the standard configuration (HTTP/HTTPS
/// only, loopback + RFC1918 blocked).
pub struct SsrfGuard {
    allowed_schemes: HashSet<String>,
    blocked_cidrs: Vec<IpRange>,
    allowed_hosts: Option<HashSet<String>>,
}

/// An inclusive IPv4/IPv6 address range used to express blocked CIDR-like
/// rules without depending on a CIDR parsing crate.
#[derive(Debug, Clone)]
pub struct IpRange {
    /// Inclusive lower bound.
    pub start: IpAddr,
    /// Inclusive upper bound.
    pub end: IpAddr,
}

/// Reasons a URL might be rejected by [`SsrfGuard::check_url`].
#[derive(Debug, Clone, Error)]
pub enum SsrfError {
    /// URL used a scheme not in the allow-list (e.g. `file://`).
    #[error("disallowed scheme: {0}")]
    DisallowedScheme(String),
    /// URL host failed the allow-list check (when one is configured).
    #[error("host blocked: {0}")]
    HostBlocked(String),
    /// URL contained an IP literal that fell inside a blocked range.
    #[error("ip blocked: {0}")]
    IpBlocked(IpAddr),
    /// DNS resolution failed (reserved for M2; not produced by M1.4).
    #[error("dns resolution failed: {0}")]
    DnsFailed(String),
    /// URL string could not be parsed.
    #[error("url parse failed: {0}")]
    UrlParseFailed(String),
}

impl SsrfGuard {
    /// Default safe configuration: HTTP + HTTPS only; loopback + RFC1918
    /// blocked; no host allow-list.
    #[must_use]
    pub fn with_defaults() -> Self {
        let mut allowed_schemes = HashSet::new();
        allowed_schemes.insert("http".into());
        allowed_schemes.insert("https".into());

        // Block RFC1918 + loopback (10/8, 172.16/12, 192.168/16, 127/8).
        // IPv6 unique-local (`fc00::/7`) and link-local (`fe80::/10`) are
        // added in M2 alongside the platform DNS resolver.
        let blocked = vec![
            IpRange {
                start: "10.0.0.0".parse().unwrap(),
                end: "10.255.255.255".parse().unwrap(),
            },
            IpRange {
                start: "172.16.0.0".parse().unwrap(),
                end: "172.31.255.255".parse().unwrap(),
            },
            IpRange {
                start: "192.168.0.0".parse().unwrap(),
                end: "192.168.255.255".parse().unwrap(),
            },
            IpRange {
                start: "127.0.0.0".parse().unwrap(),
                end: "127.255.255.255".parse().unwrap(),
            },
        ];

        Self {
            allowed_schemes,
            blocked_cidrs: blocked,
            allowed_hosts: None,
        }
    }

    /// Validate `url` against the configured policy.
    ///
    /// Returns `Ok(())` if the URL is safe to dispatch, or an [`SsrfError`]
    /// describing the reason for rejection.
    pub fn check_url(&self, url: &str) -> Result<(), SsrfError> {
        let parsed = url::Url::parse(url).map_err(|e| SsrfError::UrlParseFailed(e.to_string()))?;
        if !self.allowed_schemes.contains(parsed.scheme()) {
            return Err(SsrfError::DisallowedScheme(parsed.scheme().into()));
        }
        if let Some(allowed) = &self.allowed_hosts {
            if !allowed.contains(parsed.host_str().unwrap_or("")) {
                return Err(SsrfError::HostBlocked(
                    parsed.host_str().unwrap_or("").into(),
                ));
            }
        }
        // IP literal check (no DNS yet — that lands in M2 with the platform
        // DNS resolver). A host like "127.0.0.1" parses as `IpAddr`; a
        // hostname like "localhost" does not, so we accept it here and the
        // platform resolver will catch loopback resolutions later.
        if let Some(host) = parsed.host_str() {
            if let Ok(ip) = host.parse::<IpAddr>() {
                if self
                    .blocked_cidrs
                    .iter()
                    .any(|r| ip >= r.start && ip <= r.end)
                {
                    return Err(SsrfError::IpBlocked(ip));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_localhost_ip() {
        let g = SsrfGuard::with_defaults();
        assert!(g.check_url("http://127.0.0.1/").is_err());
    }

    #[test]
    fn blocks_rfc1918_ip() {
        let g = SsrfGuard::with_defaults();
        assert!(g.check_url("http://192.168.1.1/").is_err());
    }

    #[test]
    fn allows_public_host() {
        let g = SsrfGuard::with_defaults();
        assert!(g.check_url("https://example.com/").is_ok());
    }

    #[test]
    fn rejects_file_scheme() {
        let g = SsrfGuard::with_defaults();
        assert!(g.check_url("file:///etc/passwd").is_err());
    }
}

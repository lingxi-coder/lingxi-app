//! The allow/deny brain (`sandbox-manager.js:46-119`). [`filter_network_request`]
//! is the synchronous, ask-callback-free core (absent callback ⇒ deny, which is
//! the unmatched default anyway). [`filter_network_request_with_ask`] is the
//! full async port that consults the interactive [`AskFn`] for the unmatched
//! case — the security gate the `SandboxManager` wires into the running
//! proxies.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::config::NetworkConfig;
use crate::host::{canonicalize_host, is_valid_host, strip_brackets};

/// Error returned by an [`AskFn`] callback when it cannot reach a decision (the
/// Rust analogue of the TS `sandboxAskCallback` throwing). The manager treats
/// any such error as a denial (`filterNetworkRequest`'s `catch` returns
/// `false`), matching `sandbox-manager.js:114-119`.
#[derive(Debug)]
pub struct AskCallbackError(pub String);

impl std::fmt::Display for AskCallbackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for AskCallbackError {}

/// The interactive permission callback (`sandboxAskCallback` in
/// `sandbox-manager.js`). Invoked only for the *unmatched* case — a host that no
/// `allowed_domains`/`denied_domains` rule decides. Receives `(host, port)` and
/// resolves to `Ok(true)` (allow), `Ok(false)` (deny), or `Err(_)` (treated as
/// deny — the faithful port of the TS `try/catch`).
///
/// DIVERGENCE FROM THE PLAN (documented): the plan sketches `Fn(&str, u16) ->
/// Future<Output = bool>`. The TS `filterNetworkRequest` wraps the `await
/// sandboxAskCallback(...)` in a `try/catch` and denies on *throw* — a path the
/// `ask-error → false` test must exercise. A bare `Output = bool` future has no
/// error channel, so the output is `Result<bool, AskCallbackError>`: `Err`
/// reproduces the TS `catch`-denies branch. This is strictly more faithful than
/// `Output = bool`.
pub type AskFn = Arc<
    dyn Fn(&str, u16) -> Pin<Box<dyn Future<Output = Result<bool, AskCallbackError>> + Send>>
        + Send
        + Sync,
>;

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

/// `filterNetworkRequest` WITH the interactive ask-callback path
/// (`sandbox-manager.js:62-119`). Identical deny-first/allow core as
/// [`filter_network_request`], but the *unmatched* case consults `ask`:
///
/// - malformed host (`!is_valid_host`) → `false` and **never asks** (the host
///   bytes are untrusted; we refuse before any callback);
/// - `denied_domains` match → `false`;
/// - `allowed_domains` match → `true`;
/// - unmatched + `ask = Some(cb)` → `await cb(host, port)`; `Ok(true)` → allow,
///   `Ok(false)` or `Err(_)` → deny (the TS `try/catch` denies on throw);
/// - unmatched + `ask = None` → `false`.
///
/// `host` (not the canonicalized form) is passed to the callback, matching the
/// TS which forwards the original `{ host, port }`.
pub async fn filter_network_request_with_ask(
    port: u16,
    host: &str,
    config: &NetworkConfig,
    ask: Option<&AskFn>,
) -> bool {
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
    // Unmatched — ask the user, or deny if there is no callback. A callback
    // error (the TS callback throwing) denies, matching the TS `catch`.
    match ask {
        Some(cb) => cb(host, port).await.unwrap_or(false),
        None => false,
    }
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
            ..Default::default()
        };
        assert!(filter_network_request(443, "api.example.com", &allow));
        assert!(!filter_network_request(443, "other.com", &allow));
        // deny precedes allow
        let both = NetworkConfig {
            allowed_domains: vec!["*.example.com".into()],
            denied_domains: vec!["evil.example.com".into()],
            ..Default::default()
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
            ..Default::default()
        };
        // 2852039166 == 169.254.169.254 — the dotted denylist entry must catch it.
        assert!(!filter_network_request(80, "2852039166", &cfg));
    }

    #[test]
    fn malformed_host_denied() {
        let allow_all = NetworkConfig {
            allowed_domains: vec!["*.example.com".into()],
            denied_domains: vec![],
            ..Default::default()
        };
        assert!(!filter_network_request(
            443,
            "evil.com\u{0}.example.com",
            &allow_all
        ));
    }

    // ── filter_network_request_with_ask (sandbox-manager.js:62-119) ──

    /// Build an `AskFn` that always resolves `Ok(answer)`, recording that it ran.
    fn ask_const(answer: bool, ran: std::sync::Arc<std::sync::atomic::AtomicBool>) -> AskFn {
        std::sync::Arc::new(move |_host: &str, _port: u16| {
            let ran = std::sync::Arc::clone(&ran);
            Box::pin(async move {
                ran.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(answer)
            }) as std::pin::Pin<Box<dyn std::future::Future<Output = _> + Send>>
        })
    }

    /// An `AskFn` that errors (the TS callback throwing).
    fn ask_err(ran: std::sync::Arc<std::sync::atomic::AtomicBool>) -> AskFn {
        std::sync::Arc::new(move |_host: &str, _port: u16| {
            let ran = std::sync::Arc::clone(&ran);
            Box::pin(async move {
                ran.store(true, std::sync::atomic::Ordering::SeqCst);
                Err(AskCallbackError("boom".into()))
            }) as std::pin::Pin<Box<dyn std::future::Future<Output = _> + Send>>
        })
    }

    fn allow_example() -> NetworkConfig {
        NetworkConfig {
            allowed_domains: vec!["*.example.com".into()],
            denied_domains: vec!["evil.example.com".into()],
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn ask_denied_returns_false_without_asking() {
        let ran = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ask = ask_const(true, std::sync::Arc::clone(&ran));
        // deny rule wins before the ask path is reached.
        assert!(
            !filter_network_request_with_ask(443, "evil.example.com", &allow_example(), Some(&ask))
                .await
        );
        assert!(!ran.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn ask_allowed_returns_true_without_asking() {
        let ran = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ask = ask_const(false, std::sync::Arc::clone(&ran));
        assert!(
            filter_network_request_with_ask(443, "api.example.com", &allow_example(), Some(&ask))
                .await
        );
        assert!(!ran.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn unmatched_no_ask_is_false() {
        assert!(
            !filter_network_request_with_ask(443, "unknown.com", &allow_example(), None).await
        );
    }

    #[tokio::test]
    async fn unmatched_ask_yes_is_true() {
        let ran = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ask = ask_const(true, std::sync::Arc::clone(&ran));
        assert!(
            filter_network_request_with_ask(443, "unknown.com", &allow_example(), Some(&ask)).await
        );
        assert!(ran.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn unmatched_ask_no_is_false() {
        let ran = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ask = ask_const(false, std::sync::Arc::clone(&ran));
        assert!(
            !filter_network_request_with_ask(443, "unknown.com", &allow_example(), Some(&ask))
                .await
        );
        assert!(ran.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn ask_error_is_false() {
        let ran = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ask = ask_err(std::sync::Arc::clone(&ran));
        assert!(
            !filter_network_request_with_ask(443, "unknown.com", &allow_example(), Some(&ask))
                .await
        );
        assert!(ran.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn malformed_host_never_asks() {
        let ran = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ask = ask_const(true, std::sync::Arc::clone(&ran));
        assert!(
            !filter_network_request_with_ask(
                443,
                "evil.com\u{0}.example.com",
                &allow_example(),
                Some(&ask)
            )
            .await
        );
        assert!(!ran.load(std::sync::atomic::Ordering::SeqCst));
    }
}

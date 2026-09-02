//! `mcp xaa` — manage the XAA (SEP-990) IdP connection.
//!
//! XAA lets an MCP server authenticate silently: instead of a per-server OAuth
//! consent, the client caches ONE id_token from a configured identity provider
//! and exchanges it for a per-server access token. This module is the operator
//! surface over that — configure the IdP once (`setup`), obtain the id_token
//! (`login`), inspect (`show`), and revoke (`clear`).
//!
//! The engine underneath (`mcp::xaa_idp`) already existed in full — OIDC
//! discovery, the PKCE browser flow, the id_token cache and its 60s expiry
//! buffer — with no way to reach it from a terminal. Only the writers this
//! surface needs (`save_id_token_from_jwt`, `save_idp_client_secret`,
//! `clear_idp_client_secret`) were added alongside it.
//!
//! Ports `mcpXaaIdp.ts` (`claude mcp xaa`, 2.1.220 @238626210). Message text is
//! byte-faithful to the oracle except that command names in remediation hints
//! are rebranded (`lingxi-cli mcp xaa setup`) — telling a user to run a binary
//! that does not exist would be a defect, not fidelity.

use clap::{Args, Subcommand};
use platform_api::CredentialStoragePolicy;
use platform_posix::{PosixClock, PosixHttp};
use std::sync::Arc;

use crate::exit_codes::{RUNTIME_ERROR, SUCCESS};

/// Env var carrying the IdP client secret. Reading the secret from the
/// environment rather than argv keeps it out of the process table and shell
/// history; `--client-secret` is only the opt-IN.
const CLIENT_SECRET_ENV: &str = "MCP_XAA_IDP_CLIENT_SECRET";

/// `mcp xaa <sub>`.
#[derive(Debug, Clone, Subcommand)]
pub enum Sub {
    /// Configure the IdP connection (one-time setup for all XAA-enabled servers)
    Setup(SetupArgs),
    /// Cache an IdP id_token so XAA-enabled MCP servers authenticate silently.
    /// Default: run the OIDC browser login. With --id-token: write a
    /// pre-obtained JWT directly (used by conformance/e2e tests where the mock
    /// IdP does not serve /authorize).
    Login(LoginArgs),
    /// Show the current IdP connection config
    Show,
    /// Clear the IdP connection config and cached id_token
    Clear,
}

/// `mcp xaa setup` options.
#[derive(Debug, Clone, Args)]
pub struct SetupArgs {
    /// IdP issuer URL (OIDC discovery)
    #[arg(long)]
    pub issuer: String,
    /// LingXi's client_id at the IdP
    #[arg(long = "client-id")]
    pub client_id: String,
    /// Read IdP client secret from MCP_XAA_IDP_CLIENT_SECRET env var
    #[arg(long = "client-secret")]
    pub client_secret: bool,
    // Typed as a String rather than a u16 so an out-of-range value produces the
    // oracle's message instead of clap's. Deliberately NOT a doc comment: clap
    // renders those into `--help`, and an implementation note has no business
    // in user-facing output.
    /// Fixed loopback callback port (only if IdP does not honor RFC 8252 port-any matching)
    #[arg(long = "callback-port", value_name = "port")]
    pub callback_port: Option<String>,
}

/// `mcp xaa login` options.
#[derive(Debug, Clone, Args)]
pub struct LoginArgs {
    /// Ignore any cached id_token and re-login (useful after IdP-side
    /// revocation)
    #[arg(long)]
    pub force: bool,
    /// Write this pre-obtained id_token directly to cache, skipping the OIDC
    /// browser login
    #[arg(long = "id-token", value_name = "jwt")]
    pub id_token: Option<String>,
}

/// Env gate for the whole `xaa` group.
///
/// The oracle registers this group only when `CLAUDE_CODE_ENABLE_XAA` is
/// truthy (`vZ()`, 2.1.220 @228861934) — `claude mcp xaa show` on a default
/// install answers `error: unknown command 'xaa'`. Registering it
/// unconditionally would advertise an enterprise IdP surface the oracle keeps
/// behind a flag, so the gate is reproduced here.
///
/// `LINGXI_ENABLE_XAA` is accepted alongside the `CLAUDE_CODE_` name, matching
/// how this port handles its other dual-named env flags.
#[must_use]
pub fn xaa_enabled() -> bool {
    ["LINGXI_ENABLE_XAA", "CLAUDE_CODE_ENABLE_XAA"]
        .iter()
        .any(|k| platform_api::env::is_env_truthy(std::env::var(k).ok().as_deref()))
}

/// Dispatch `mcp xaa`.
pub async fn run(sub: &Sub) -> i32 {
    if !xaa_enabled() {
        // Byte-matches the oracle's commander error for an unregistered group.
        eprintln!("error: unknown command 'xaa'");
        return RUNTIME_ERROR;
    }
    match sub {
        Sub::Setup(a) => run_setup(a).await,
        Sub::Login(a) => run_login(a).await,
        Sub::Show => run_show().await,
        Sub::Clear => run_clear().await,
    }
}

// ---------------------------------------------------------------------------
// Issuer validation (mcpXaaIdp.ts, the `setup` action).
// ---------------------------------------------------------------------------

/// A parsed issuer, reduced to the two pieces the validation messages need.
struct ParsedIssuer {
    scheme: String,
    host: String,
    /// Host without a port, lowercased — what the loopback exemption tests.
    hostname: String,
}

/// Minimal absolute-URL split. Avoids a `url` dep in `apps/cli` for what is a
/// scheme/authority check; anything without a `scheme://host` is rejected as
/// invalid, which is the only distinction the caller draws.
fn parse_issuer(raw: &str) -> Option<ParsedIssuer> {
    let (scheme, rest) = raw.split_once("://")?;
    if scheme.is_empty()
        || !scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.')
    {
        return None;
    }
    // Authority ends at the first `/`, `?` or `#`.
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.is_empty() {
        return None;
    }
    // Strip userinfo, then split off the port. An IPv6 literal keeps its
    // brackets, since that is how the oracle's `hostname` compares `[::1]`.
    let hostport = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let hostname = if let Some(end) = hostport.find(']') {
        &hostport[..=end]
    } else {
        hostport.split(':').next().unwrap_or(hostport)
    };
    if hostname.is_empty() {
        return None;
    }
    Some(ParsedIssuer {
        scheme: scheme.to_ascii_lowercase(),
        host: hostport.to_string(),
        hostname: hostname.to_ascii_lowercase(),
    })
}

/// Is this a loopback host, for which plain `http://` is permitted?
///
/// RFC 8252 §7.3: a loopback redirect cannot be intercepted off-host, so the
/// https requirement is relaxed there and only there.
fn is_loopback(hostname: &str) -> bool {
    matches!(hostname, "localhost" | "127.0.0.1" | "[::1]")
}

/// Validation outcome for `setup`, so the ordering is testable without a
/// filesystem, a keychain, or a settings file.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SetupCheck {
    Ok { callback_port: Option<u16> },
    Err(String),
}

/// Validate `setup` inputs in the oracle's order: issuer parse → issuer scheme
/// → callback port → client-secret env.
///
/// The order is behavioural, not cosmetic. A caller who passes both a bad
/// issuer and a bad port must be told about the issuer first, because fixing
/// the port would not have helped.
pub(crate) fn check_setup(
    issuer: &str,
    callback_port: Option<&str>,
    want_client_secret: bool,
    client_secret_env: Option<&str>,
) -> SetupCheck {
    let Some(parsed) = parse_issuer(issuer) else {
        return SetupCheck::Err(format!(
            "Error: --issuer must be a valid URL (got \"{issuer}\")"
        ));
    };
    let scheme_ok =
        parsed.scheme == "https" || (parsed.scheme == "http" && is_loopback(&parsed.hostname));
    if !scheme_ok {
        return SetupCheck::Err(format!(
            "Error: --issuer must use https:// (got \"{}://{}\")",
            parsed.scheme, parsed.host
        ));
    }
    let port = match callback_port {
        None => None,
        Some(raw) => match raw.trim().parse::<u32>() {
            Ok(n) if n >= 1 && n <= 65_535 => Some(n as u16),
            _ => {
                return SetupCheck::Err(
                    "Error: --callback-port must be an integer in [1, 65535]".to_string(),
                )
            }
        },
    };
    if want_client_secret && client_secret_env.is_none_or(str::is_empty) {
        return SetupCheck::Err(format!(
            "Error: --client-secret requires {CLIENT_SECRET_ENV} env var"
        ));
    }
    SetupCheck::Ok {
        callback_port: port,
    }
}

/// Does switching from `prev` to `next` invalidate the cached credentials?
///
/// Either a different issuer (normalized — a trailing slash or a host-case
/// change is not a switch) or a different client_id at the same issuer. In both
/// cases the cached id_token was minted for something that no longer applies,
/// so keeping it would silently authenticate as the wrong principal.
pub(crate) fn credentials_invalidated(
    prev: &mcp::xaa_idp::XaaIdpSettings,
    next_issuer: &str,
    next_client_id: &str,
) -> bool {
    mcp::xaa_idp::issuer_key(&prev.issuer) != mcp::xaa_idp::issuer_key(next_issuer)
        || prev.client_id != next_client_id
}

// ---------------------------------------------------------------------------
// Subcommand bodies.
// ---------------------------------------------------------------------------

async fn run_setup(a: &SetupArgs) -> i32 {
    let env_secret = std::env::var(CLIENT_SECRET_ENV).ok();
    let callback_port = match check_setup(
        &a.issuer,
        a.callback_port.as_deref(),
        a.client_secret,
        env_secret.as_deref(),
    ) {
        SetupCheck::Ok { callback_port } => callback_port,
        SetupCheck::Err(msg) => {
            eprintln!("{msg}");
            return RUNTIME_ERROR;
        }
    };

    let previous = read_xaa_settings();

    let mut entry = serde_json::Map::new();
    entry.insert("issuer".into(), serde_json::Value::String(a.issuer.clone()));
    entry.insert(
        "clientId".into(),
        serde_json::Value::String(a.client_id.clone()),
    );
    if let Some(p) = callback_port {
        entry.insert("callbackPort".into(), serde_json::Value::from(p));
    }
    if let Err(e) = write_xaa_settings(Some(serde_json::Value::Object(entry))) {
        eprintln!("Error writing settings: {e}");
        return RUNTIME_ERROR;
    }

    // Settings are committed. Everything past here touches the keychain, which
    // is OPTIONAL unless a secret was actually requested: reporting a failure
    // when the user never asked for a secret would say the connection was not
    // configured when in fact it was.
    let storage_and_clock = storage_and_clock().await;

    if let Ok((storage, _)) = &storage_and_clock {
        if let Some(prev) = previous {
            if credentials_invalidated(&prev, &a.issuer, &a.client_id) {
                let _ = mcp::xaa_idp::clear_cached_id_token(storage, &prev.issuer).await;
                let _ = mcp::xaa_idp::clear_idp_client_secret(storage, &prev.issuer).await;
            }
        }
    }

    if a.client_secret {
        if let Some(secret) = env_secret.as_deref() {
            let saved = match &storage_and_clock {
                Ok((storage, clock)) => {
                    mcp::xaa_idp::save_idp_client_secret(storage, clock, &a.issuer, secret).await
                }
                Err(e) => Err(platform_api::McpError::OAuth(e.clone())),
            };
            if let Err(e) = saved {
                eprintln!(
                    "Error: settings written but keychain save failed \u{2014} {e}. \
                     Re-run with --client-secret once keychain is available."
                );
                return RUNTIME_ERROR;
            }
        }
    }

    println!("XAA IdP connection configured for {}", a.issuer);
    SUCCESS
}

async fn run_login(a: &LoginArgs) -> i32 {
    let Some(cfg) = read_xaa_settings() else {
        eprintln!("Error: no XAA IdP connection. Run 'lingxi-cli mcp xaa setup' first.");
        return RUNTIME_ERROR;
    };
    let (storage, clock) = match storage_and_clock().await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("IdP login failed: {e}");
            return RUNTIME_ERROR;
        }
    };

    // `--id-token` short-circuits the browser entirely: cache the JWT as given
    // and report when it expires.
    if let Some(jwt) = a.id_token.as_deref() {
        return match mcp::xaa_idp::save_id_token_from_jwt(&storage, &clock, &cfg.issuer, jwt).await
        {
            Ok(expires_at) => {
                println!(
                    "id_token cached for {} (expires {})",
                    cfg.issuer,
                    protocol::iso8601::iso8601_utc(expires_at)
                );
                SUCCESS
            }
            Err(e) => {
                eprintln!("id_token cache write failed: {e}");
                RUNTIME_ERROR
            }
        };
    }

    if a.force {
        let _ = mcp::xaa_idp::clear_cached_id_token(&storage, &cfg.issuer).await;
    }

    match mcp::xaa_idp::get_cached_id_token(&storage, &clock, &cfg.issuer).await {
        Ok(Some(_)) => {
            println!(
                "Already logged in to {} (cached id_token still valid). Use --force to re-login.",
                cfg.issuer
            );
            return SUCCESS;
        }
        Ok(None) => {}
        Err(e) => {
            eprintln!("IdP login failed: {e}");
            return RUNTIME_ERROR;
        }
    }

    print!("Opening browser for IdP login at {}\u{2026}\n", cfg.issuer);
    let _ = std::io::Write::flush(&mut std::io::stdout());

    let http = match http_transport() {
        Ok(h) => h,
        Err(e) => {
            eprintln!("IdP login failed: {e}");
            return RUNTIME_ERROR;
        }
    };
    let idp_secret = mcp::xaa_idp::get_idp_client_secret(&storage, &cfg.issuer)
        .await
        .unwrap_or(None);
    let on_url: mcp::oauth::OnAuthorizationUrl = Arc::new(|url: &str| {
        print!("If the browser did not open, visit:\n  {url}\n");
        let _ = std::io::Write::flush(&mut std::io::stdout());
    });

    match mcp::xaa_idp::acquire_idp_id_token(
        &http,
        &clock,
        &storage,
        &on_url,
        &cfg,
        idp_secret.as_deref(),
    )
    .await
    {
        Ok(_) => {
            println!("Logged in. MCP servers with --xaa will now authenticate silently.");
            SUCCESS
        }
        Err(e) => {
            eprintln!("IdP login failed: {e}");
            RUNTIME_ERROR
        }
    }
}

async fn run_show() -> i32 {
    let Some(cfg) = read_xaa_settings() else {
        println!("No XAA IdP connection configured.");
        return SUCCESS;
    };
    let (storage, clock) = match storage_and_clock().await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error reading credential storage: {e}");
            return RUNTIME_ERROR;
        }
    };
    // A storage ERROR is not the same as an absent entry. `unwrap_or(None)`
    // would render a timing-out keychain as "not logged in" / "no secret",
    // which is a claim the command cannot support — and the user would go
    // re-run a login that was never the problem.
    let has_secret = mcp::xaa_idp::get_idp_client_secret(&storage, &cfg.issuer)
        .await
        .map(|v| v.is_some())
        .ok();
    let logged_in = mcp::xaa_idp::get_cached_id_token(&storage, &clock, &cfg.issuer)
        .await
        .map(|v| v.is_some())
        .ok();

    print!("{}", render_show(&cfg, has_secret, logged_in));
    SUCCESS
}

/// The `show` block, as a string so its exact layout is testable.
///
/// Field labels are padded to a fixed column; `Callback port` appears only when
/// one is configured, matching the oracle's conditional write.
/// `has_secret` / `logged_in` are `None` when credential storage could not be
/// read at all — rendered as an explicit unknown rather than as a "no".
pub(crate) fn render_show(
    cfg: &mcp::xaa_idp::XaaIdpSettings,
    has_secret: Option<bool>,
    logged_in: Option<bool>,
) -> String {
    let mut out = String::new();
    out.push_str(&format!("Issuer:        {}\n", cfg.issuer));
    out.push_str(&format!("Client ID:     {}\n", cfg.client_id));
    if let Some(p) = cfg.callback_port {
        out.push_str(&format!("Callback port: {p}\n"));
    }
    out.push_str(&format!(
        "Client secret: {}\n",
        match has_secret {
            Some(true) => "(stored in keychain)",
            Some(false) => "(not set \u{2014} PKCE-only)",
            None => "(unknown \u{2014} credential storage unavailable)",
        }
    ));
    out.push_str(&format!(
        "Logged in:     {}\n",
        match logged_in {
            Some(true) => "yes (id_token cached)",
            Some(false) => "no \u{2014} run 'lingxi-cli mcp xaa login'",
            None => "unknown \u{2014} credential storage unavailable",
        }
    ));
    out.push('\n');
    out
}

async fn run_clear() -> i32 {
    let previous = read_xaa_settings();
    if let Err(e) = write_xaa_settings(None) {
        eprintln!("Error writing settings: {e}");
        return RUNTIME_ERROR;
    }
    if let Some(cfg) = previous {
        if let Ok((storage, _clock)) = storage_and_clock().await {
            let _ = mcp::xaa_idp::clear_cached_id_token(&storage, &cfg.issuer).await;
            let _ = mcp::xaa_idp::clear_idp_client_secret(&storage, &cfg.issuer).await;
        }
    }
    println!("XAA IdP connection cleared");
    SUCCESS
}

// ---------------------------------------------------------------------------
// Settings + runtime plumbing.
// ---------------------------------------------------------------------------

fn user_settings_path() -> std::path::PathBuf {
    crate::run::lingxi_home_dir().join("settings.json")
}

/// Read the `xaaIdp` slice of user settings.
fn read_xaa_settings() -> Option<mcp::xaa_idp::XaaIdpSettings> {
    let raw = std::fs::read_to_string(user_settings_path()).ok()?;
    mcp::xaa_idp::XaaIdpSettings::from_settings_tiers(&[&raw])
}

/// Set (or with `None`, remove) the `xaaIdp` key in user settings, preserving
/// every other key.
///
/// Read-modify-write of the whole document: a settings file carries unrelated
/// user configuration, so writing only our key would destroy it. A file that is
/// absent or not a JSON object starts from an empty object; a file that is
/// present but unparseable is an error rather than something to overwrite,
/// since silently discarding a user's malformed settings loses data they can
/// still fix by hand.
pub(crate) fn apply_xaa_to_settings(
    existing: Option<&str>,
    value: Option<serde_json::Value>,
) -> Result<String, String> {
    let mut doc = match existing {
        None => serde_json::Map::new(),
        Some(raw) if raw.trim().is_empty() => serde_json::Map::new(),
        Some(raw) => match serde_json::from_str::<serde_json::Value>(raw) {
            Ok(serde_json::Value::Object(m)) => m,
            Ok(_) => serde_json::Map::new(),
            Err(e) => return Err(format!("{} is not valid JSON: {e}", "settings.json")),
        },
    };
    match value {
        Some(v) => {
            doc.insert("xaaIdp".into(), v);
        }
        None => {
            doc.remove("xaaIdp");
        }
    }
    serde_json::to_string_pretty(&serde_json::Value::Object(doc))
        .map(|s| s + "\n")
        .map_err(|e| e.to_string())
}

fn write_xaa_settings(value: Option<serde_json::Value>) -> Result<(), String> {
    let path = user_settings_path();
    let existing = std::fs::read_to_string(&path).ok();
    let next = apply_xaa_to_settings(existing.as_deref(), value)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, next).map_err(|e| e.to_string())
}

async fn storage_and_clock() -> Result<
    (
        Arc<dyn platform_api::SecureStorage>,
        Arc<dyn platform_api::Clock>,
    ),
    String,
> {
    let home = crate::run::lingxi_home_dir();
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "default".to_string());
    let storage = credential_storage(user, home.clone(), home.join(".credentials.json"))
        .await
        .map_err(|e| e.to_string())?;
    let clock: Arc<dyn platform_api::Clock> = Arc::new(PosixClock::new());
    Ok((storage, clock))
}

async fn credential_storage(
    user: String,
    home: std::path::PathBuf,
    credentials_path: std::path::PathBuf,
) -> Result<Arc<dyn platform_api::SecureStorage>, platform_api::SecureStorageError> {
    #[cfg(windows)]
    {
        platform_windows::secure_storage_for_policy(
            user,
            home,
            credentials_path,
            CredentialStoragePolicy::NativePreferred,
        )
        .await
    }
    #[cfg(not(windows))]
    {
        platform_posix::secure_storage_for_policy(
            user,
            home,
            credentials_path,
            CredentialStoragePolicy::NativePreferred,
        )
        .await
    }
}

fn http_transport() -> Result<Arc<dyn platform_api::HttpTransport>, String> {
    Ok(Arc::new(PosixHttp::new()) as Arc<dyn platform_api::HttpTransport>)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(issuer: &str, client_id: &str) -> mcp::xaa_idp::XaaIdpSettings {
        mcp::xaa_idp::XaaIdpSettings {
            issuer: issuer.into(),
            client_id: client_id.into(),
            callback_port: None,
        }
    }

    // ---- issuer validation ------------------------------------------------

    #[test]
    fn https_issuer_is_accepted() {
        assert_eq!(
            check_setup("https://idp.example", None, false, None),
            SetupCheck::Ok {
                callback_port: None
            }
        );
    }

    #[test]
    fn plain_http_is_rejected_with_scheme_and_host() {
        assert_eq!(
            check_setup("http://idp.example:8080/x", None, false, None),
            SetupCheck::Err(
                "Error: --issuer must use https:// (got \"http://idp.example:8080\")".into()
            )
        );
    }

    #[test]
    fn http_loopback_is_exempt() {
        for host in ["localhost", "127.0.0.1", "[::1]"] {
            assert_eq!(
                check_setup(&format!("http://{host}:9000"), None, false, None),
                SetupCheck::Ok {
                    callback_port: None
                },
                "{host} must be exempt"
            );
        }
    }

    #[test]
    fn a_loopback_lookalike_host_is_not_exempt() {
        // `localhost.evil.com` merely starts with the loopback name.
        match check_setup("http://localhost.evil.com", None, false, None) {
            SetupCheck::Err(m) => assert!(m.contains("must use https://"), "{m}"),
            other => panic!("expected rejection, got {other:?}"),
        }
    }

    #[test]
    fn unparseable_issuer_is_reported_before_scheme() {
        assert_eq!(
            check_setup("not a url", None, false, None),
            SetupCheck::Err("Error: --issuer must be a valid URL (got \"not a url\")".into())
        );
    }

    #[test]
    fn issuer_error_precedes_port_error() {
        // Both are wrong; the issuer must be reported, since fixing the port
        // alone would not have helped.
        match check_setup("nope", Some("0"), false, None) {
            SetupCheck::Err(m) => assert!(m.contains("--issuer"), "{m}"),
            other => panic!("expected issuer error, got {other:?}"),
        }
    }

    // ---- callback port ----------------------------------------------------

    #[test]
    fn callback_port_bounds_are_inclusive() {
        for (raw, want) in [("1", Some(1u16)), ("65535", Some(65_535))] {
            assert_eq!(
                check_setup("https://i.example", Some(raw), false, None),
                SetupCheck::Ok {
                    callback_port: want
                }
            );
        }
    }

    #[test]
    fn callback_port_rejects_zero_overflow_and_non_numeric() {
        for raw in ["0", "65536", "abc", "-1", "1.5", ""] {
            assert_eq!(
                check_setup("https://i.example", Some(raw), false, None),
                SetupCheck::Err("Error: --callback-port must be an integer in [1, 65535]".into()),
                "{raw:?} must be rejected"
            );
        }
    }

    // ---- client secret ----------------------------------------------------

    #[test]
    fn client_secret_flag_requires_the_env_var() {
        assert_eq!(
            check_setup("https://i.example", None, true, None),
            SetupCheck::Err(
                "Error: --client-secret requires MCP_XAA_IDP_CLIENT_SECRET env var".into()
            )
        );
    }

    #[test]
    fn an_empty_env_var_counts_as_absent() {
        // Present-but-empty would otherwise store an empty secret and report
        // success, leaving a confidential client silently unauthenticated.
        assert_eq!(
            check_setup("https://i.example", None, true, Some("")),
            SetupCheck::Err(
                "Error: --client-secret requires MCP_XAA_IDP_CLIENT_SECRET env var".into()
            )
        );
    }

    #[test]
    fn env_var_is_ignored_without_the_flag() {
        assert_eq!(
            check_setup("https://i.example", None, false, Some("s3cret")),
            SetupCheck::Ok {
                callback_port: None
            }
        );
    }

    // ---- credential invalidation -----------------------------------------

    #[test]
    fn same_issuer_and_client_keeps_credentials() {
        let prev = settings("https://idp.example/", "client-a");
        assert!(!credentials_invalidated(
            &prev,
            "https://idp.example",
            "client-a"
        ));
    }

    #[test]
    fn changed_client_id_invalidates_credentials() {
        let prev = settings("https://idp.example", "client-a");
        assert!(credentials_invalidated(
            &prev,
            "https://idp.example",
            "client-b"
        ));
    }

    #[test]
    fn changed_issuer_invalidates_credentials() {
        let prev = settings("https://idp.example", "client-a");
        assert!(credentials_invalidated(
            &prev,
            "https://other.example",
            "client-a"
        ));
    }

    #[test]
    fn host_case_alone_is_not_a_change() {
        let prev = settings("https://IdP.Example", "client-a");
        assert!(!credentials_invalidated(
            &prev,
            "https://idp.example",
            "client-a"
        ));
    }

    // ---- settings read-modify-write --------------------------------------

    #[test]
    fn setting_xaa_preserves_unrelated_keys() {
        let before = r#"{"model":"opus","permissions":{"defaultMode":"plan"}}"#;
        let after = apply_xaa_to_settings(
            Some(before),
            Some(serde_json::json!({"issuer":"https://i.example","clientId":"c"})),
        )
        .expect("write");
        let v: serde_json::Value = serde_json::from_str(&after).unwrap();
        assert_eq!(v["model"], "opus");
        assert_eq!(v["permissions"]["defaultMode"], "plan");
        assert_eq!(v["xaaIdp"]["issuer"], "https://i.example");
    }

    #[test]
    fn clearing_xaa_leaves_the_rest_intact() {
        let before = r#"{"model":"opus","xaaIdp":{"issuer":"https://i.example","clientId":"c"}}"#;
        let after = apply_xaa_to_settings(Some(before), None).expect("write");
        let v: serde_json::Value = serde_json::from_str(&after).unwrap();
        assert_eq!(v["model"], "opus");
        assert!(v.get("xaaIdp").is_none(), "xaaIdp must be removed");
    }

    #[test]
    fn absent_settings_file_starts_from_an_empty_object() {
        let after =
            apply_xaa_to_settings(None, Some(serde_json::json!({"issuer":"x"}))).expect("write");
        let v: serde_json::Value = serde_json::from_str(&after).unwrap();
        assert_eq!(v["xaaIdp"]["issuer"], "x");
    }

    #[test]
    fn malformed_settings_are_an_error_not_an_overwrite() {
        // Overwriting would silently destroy configuration the user can still
        // repair by hand.
        assert!(apply_xaa_to_settings(Some("{ broken"), None).is_err());
    }

    #[test]
    fn clearing_an_absent_key_is_a_no_op_not_a_failure() {
        let after = apply_xaa_to_settings(Some(r#"{"model":"opus"}"#), None).expect("write");
        let v: serde_json::Value = serde_json::from_str(&after).unwrap();
        assert_eq!(v["model"], "opus");
    }

    // ---- show rendering ---------------------------------------------------

    #[test]
    fn show_omits_callback_port_when_unset() {
        let out = render_show(
            &settings("https://i.example", "cid"),
            Some(false),
            Some(false),
        );
        assert!(!out.contains("Callback port"), "{out}");
        assert!(
            out.contains("Client secret: (not set \u{2014} PKCE-only)"),
            "{out}"
        );
        assert!(
            out.contains("Logged in:     no \u{2014} run 'lingxi-cli mcp xaa login'"),
            "{out}"
        );
    }

    #[test]
    fn show_includes_callback_port_when_set() {
        let mut cfg = settings("https://i.example", "cid");
        cfg.callback_port = Some(9000);
        let out = render_show(&cfg, Some(true), Some(true));
        assert!(out.contains("Callback port: 9000\n"), "{out}");
        assert!(out.contains("Client secret: (stored in keychain)"), "{out}");
        assert!(
            out.contains("Logged in:     yes (id_token cached)"),
            "{out}"
        );
    }

    #[test]
    fn show_reports_unknown_rather_than_no_when_storage_is_unreadable() {
        // Rendering a storage ERROR as "no" would send the user to re-run a
        // login that was never the problem.
        let out = render_show(&settings("https://i.example", "cid"), None, None);
        assert!(
            out.contains("Client secret: (unknown \u{2014} credential storage unavailable)"),
            "{out}"
        );
        assert!(
            out.contains("Logged in:     unknown \u{2014} credential storage unavailable"),
            "{out}"
        );
        assert!(
            !out.contains("PKCE-only"),
            "must not claim a state it cannot read: {out}"
        );
    }

    #[test]
    fn show_ends_with_a_blank_line() {
        let out = render_show(
            &settings("https://i.example", "cid"),
            Some(false),
            Some(false),
        );
        assert!(out.ends_with("\n\n"), "{out:?}");
    }
}

//! OAuth 2.1 Authorization-Code + PKCE handshake for remote (SSE/HTTP) MCP
//! servers.
//!
//! Faithful core port of `claude-code/src/services/mcp/auth.ts` (2466 lines):
//!
//! 1. **Discovery** — RFC 9728 (`/.well-known/oauth-protected-resource`) →
//!    RFC 8414 (`/.well-known/oauth-authorization-server`) to locate the
//!    authorization/token/registration endpoints
//!    (`fetchAuthServerMetadata`, auth.ts:256-311).
//! 2. **Dynamic Client Registration** (RFC 7591, public client) when no
//!    `client_id` is configured (`ClaudeAuthProvider.clientMetadata`,
//!    auth.ts:1417-1437).
//! 3. **PKCE** — `code_verifier`/`code_challenge` (S256) generation.
//! 4. **Loopback callback** — bind `127.0.0.1:{port}`, capture the auth code.
//! 5. **Authorization URL** — built for the host/user to open.
//! 6. **Token exchange** — `authorization_code` grant (form-encoded per
//!    RFC 6749 / the MCP SDK).
//! 7. **Refresh** — `refresh_token` grant, run on token expiry / a 401.
//!
//! Tokens are persisted via `lingxi-secret`'s [`traits::SecureStorage`] seam
//! keyed by a `getServerKey`-equivalent (`name|sha256({type,url,headers})[..16]`,
//! auth.ts:325-341), the inner access/refresh strings wrapped in
//! [`protocol::Secret`].
//!
//! The [`OAuthState`] enum models the handshake stages; [`perform_oauth_flow`]
//! drives discovery → (DCR) → PKCE → listener → URL → exchange.
//!
//! **Implemented since the initial port:**
//! - **Token revocation (RFC 7009)** — [`revoke_token`] / [`revoke_server_tokens`]
//!   (port of `revokeToken`/`revokeServerTokens`, auth.ts:365-577), wired into
//!   `McpRegistry::disconnect` (best-effort, refresh-then-access, then always
//!   clear locally). The `preserveStepUpState` re-auth variant (auth.ts:578-617)
//!   is not the logout path and stays residual.
//! - **Step-up scope (403 `insufficient_scope`)** — detection + cached-scope
//!   re-auth + retry in `registry.rs` (port of `wrapFetchWithStepUpDetection` /
//!   `cachedStepUpScope`, auth.ts:1354-1374, 906-935). A structured
//!   403-with-headers path (vs the flattened error string) is a noted residual,
//!   parallel to the existing 401 note.
//! - **XAA cross-app-access (SEP-990)** — the exchange engine + orchestrator
//!   ([`crate::xaa`]) and the `LINGXI_ENABLE_XAA` + `oauth.xaa` gate in
//!   `registry.rs` (port of `xaa.ts` + `performMCPXaaAuth`, auth.ts:847-900).
//!
//! **Residual (noted, not ported):**
//! - The XAA **IdP-login / secret config surface** — `getXaaIdpSettings`,
//!   `acquireIdpIdToken` (the one OIDC browser pop), `discoverOidc`, the
//!   keychain `id_token` cache, the `xaaRefresh` silent path, the AS
//!   `client_secret`/`mcpOAuthClientConfig` config seam, and analytics
//!   (auth.ts:664-744). Supplied via the [`crate::registry::XaaConfigProvider`]
//!   seam; absent it, an XAA-flagged server hard-fails with actionable copy.
//! - CIMD (`client_id_metadata_document`), Slack-style `200`-with-error-body
//!   normalization, cross-process lockfile refresh coordination, and analytics
//!   events.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use protocol::{HttpMethod, HttpRequest, Secret};
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use traits::{Clock, HttpTransport, McpTransportSpec};

pub mod callback;

pub use callback::{CallbackError, CallbackListener, CallbackParams};

/// Timeout for the discovery / DCR / token-exchange / refresh POSTs. Mirrors
/// claude-code's 15-second auth-request deadline.
const OAUTH_HTTP_TIMEOUT: Duration = Duration::from_secs(15);

/// Stages of the OAuth handshake.
///
/// `Secret<T>` deliberately does not implement `Clone`, so neither does
/// `OAuthState`; the registry holds the value behind an `RwLock` and
/// moves between variants when a transition happens.
#[derive(Debug)]
pub enum OAuthState {
    /// Local PKCE pair generated, listener about to start.
    Initiated {
        /// Loopback port the callback listener will bind to.
        callback_port: u16,
        /// PKCE code verifier.
        code_verifier: String,
        /// CSRF state token.
        state_token: String,
    },
    /// Listening on the loopback callback URL for the auth code.
    AwaitingCallback {
        /// Loopback port the callback listener is bound to.
        callback_port: u16,
        /// PKCE code verifier carried forward to token exchange.
        code_verifier: String,
        /// CSRF state token that must match the redirect.
        state_token: String,
        /// Authorization URL shown to the user.
        auth_url: String,
    },
    /// Exchanging the received code for tokens.
    ExchangingCode {
        /// Authorization code returned by the redirect.
        code: String,
    },
    /// Tokens acquired and ready for use.
    Authenticated {
        /// Bearer access token.
        access_token: protocol::Secret<String>,
        /// Optional long-lived refresh token.
        refresh_token: Option<protocol::Secret<String>>,
        /// Expiry of the current access token.
        expires_at: SystemTime,
    },
    /// Refreshing the access token using the refresh token.
    Refreshing {
        /// Refresh token being exchanged.
        refresh_token: protocol::Secret<String>,
    },
}

/// OAuth-flow failures.
#[derive(Debug, thiserror::Error)]
pub enum OAuthError {
    /// `.well-known` discovery failed (network, status, parse, or no AS found).
    #[error("oauth discovery failed: {0}")]
    Discovery(String),
    /// Dynamic client registration (RFC 7591) failed.
    #[error("oauth client registration failed: {0}")]
    Registration(String),
    /// Loopback callback failed (bind, parse, or state mismatch).
    #[error("oauth callback failed: {0}")]
    Callback(String),
    /// Token exchange / refresh against the authorization server failed.
    #[error("oauth token request failed: {0}")]
    Token(String),
    /// Stored `refresh_token` was rejected (`invalid_grant`); re-auth required.
    #[error("oauth refresh rejected: {0}")]
    RefreshRejected(String),
}

impl From<OAuthError> for traits::McpError {
    fn from(e: OAuthError) -> Self {
        traits::McpError::OAuth(e.to_string())
    }
}

impl From<CallbackError> for OAuthError {
    fn from(e: CallbackError) -> Self {
        OAuthError::Callback(e.to_string())
    }
}

// ---------------------------------------------------------------------------
// PKCE (RFC 7636) — copied from anthropic-oauth/src/pkce.rs.
// ---------------------------------------------------------------------------

/// Generate a `(code_verifier, code_challenge)` pair (S256, URL-safe base64,
/// no padding). The challenge goes in the authorize URL; the verifier is held
/// by the client and supplied at the token-exchange step.
#[must_use]
pub fn generate_pkce() -> (String, String) {
    let mut rng = rand::rng();
    let bytes: [u8; 32] = std::array::from_fn(|_| rng.random::<u8>());
    let verifier = URL_SAFE_NO_PAD.encode(bytes);

    let mut h = Sha256::new();
    h.update(verifier.as_bytes());
    let challenge = URL_SAFE_NO_PAD.encode(h.finalize());

    (verifier, challenge)
}

/// Generate a 16-byte CSRF state token, base64url-encoded (no padding). Sent
/// in the authorize URL and validated against the redirect callback's `state`.
#[must_use]
pub fn generate_state_token() -> String {
    let mut rng = rand::rng();
    let bytes: [u8; 16] = std::array::from_fn(|_| rng.random::<u8>());
    URL_SAFE_NO_PAD.encode(bytes)
}

// ---------------------------------------------------------------------------
// Discovery (RFC 9728 → RFC 8414).
// ---------------------------------------------------------------------------

/// `.well-known/oauth-protected-resource` body (RFC 9728). We only need the
/// `authorization_servers` list (the first entry is the AS issuer URL).
#[derive(Debug, Deserialize)]
struct ProtectedResourceMetadata {
    #[serde(default)]
    authorization_servers: Vec<String>,
}

/// Authorization-server metadata (RFC 8414, `.well-known/oauth-authorization-server`).
///
/// Mirrors the subset of `OAuthMetadataSchema` (auth.ts) the flow consumes.
#[derive(Debug, Clone, Deserialize)]
pub struct AuthServerMetadata {
    /// Authorization endpoint (the user opens this; `response_type=code`).
    pub authorization_endpoint: String,
    /// Token endpoint (`authorization_code` / `refresh_token` grants).
    pub token_endpoint: String,
    /// Dynamic-client-registration endpoint (RFC 7591), if advertised.
    #[serde(default)]
    pub registration_endpoint: Option<String>,
    /// Scopes the server advertises (`scopes_supported`), if any.
    #[serde(default)]
    pub scopes_supported: Option<Vec<String>>,
    /// Non-standard curated `scope` string some servers publish (claude-code
    /// `getCuratedMetadataScope` first branch: `if ("scope" in e && typeof
    /// e.scope === "string") return e.scope`). Preferred over the
    /// `scopes_supported` catalog when present.
    #[serde(default)]
    pub scope: Option<String>,
    /// Non-standard curated `default_scope` string (the fallback after
    /// `scope` in claude-code's curated-scope reader).
    #[serde(default)]
    pub default_scope: Option<String>,
    /// Token revocation endpoint (RFC 7009), if advertised (auth.ts:495-498).
    #[serde(default)]
    pub revocation_endpoint: Option<String>,
    /// Client-auth methods the revocation endpoint accepts (RFC 7009,
    /// auth.ts:503-508). Preferred over `token_endpoint_auth_methods_supported`
    /// when present.
    #[serde(default)]
    pub revocation_endpoint_auth_methods_supported: Option<Vec<String>>,
    /// Client-auth methods the token endpoint accepts (RFC 8414). Fallback for
    /// revocation auth-method selection (auth.ts:509-511).
    #[serde(default)]
    pub token_endpoint_auth_methods_supported: Option<Vec<String>>,
}

/// Build a `.well-known` URL by inserting the well-known path between the
/// origin and any path component (RFC 8414 §3: the well-known segment is
/// inserted after the host, before the issuer path).
fn well_known_url(base: &str, suffix: &str) -> String {
    // Split scheme://host[:port] from the path. We avoid a `url` dep (not in
    // mcp's tree); the inputs are validated server URLs from config.
    let without_scheme = base.split_once("://");
    match without_scheme {
        Some((scheme, rest)) => {
            let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
            let path = path.trim_end_matches('/');
            if path.is_empty() {
                format!("{scheme}://{host}/.well-known/{suffix}")
            } else {
                // RFC 8414 path-aware form: /.well-known/{suffix}/{path}
                format!("{scheme}://{host}/.well-known/{suffix}/{path}")
            }
        }
        // No scheme — treat the whole thing as host.
        None => format!("{base}/.well-known/{suffix}"),
    }
}

/// One discovery `GET` returning a parsed JSON body, or `Err` on non-200.
async fn get_json<T: serde::de::DeserializeOwned>(
    http: &Arc<dyn HttpTransport>,
    url: &str,
) -> Result<T, OAuthError> {
    let req = HttpRequest {
        method: HttpMethod::Get,
        url: url.to_string(),
        headers: vec![("accept".into(), "application/json".into())],
        body: None,
        body_bytes: None,
        timeout: Some(OAUTH_HTTP_TIMEOUT),
    };
    let resp = http
        .request(req)
        .await
        .map_err(|e| OAuthError::Discovery(format!("transport: {e}")))?;
    if resp.status != 200 {
        return Err(OAuthError::Discovery(format!(
            "HTTP {} fetching {url}",
            resp.status
        )));
    }
    serde_json::from_str(&resp.body)
        .map_err(|e| OAuthError::Discovery(format!("decode {url}: {e}")))
}

/// Discover the authorization-server metadata for an MCP `server_url`.
///
/// Order (auth.ts:243-310):
/// 1. If `configured_url` is set, fetch it directly (must be https).
/// 2. RFC 9728: probe `/.well-known/oauth-protected-resource` on the server,
///    read `authorization_servers[0]`, then RFC 8414 against that issuer.
/// 3. Fallback: RFC 8414 directly against the server URL (path-aware).
///
/// # Errors
/// [`OAuthError::Discovery`] if no usable metadata can be located.
pub async fn discover_auth_server_metadata(
    http: &Arc<dyn HttpTransport>,
    server_url: &str,
    configured_url: Option<&str>,
) -> Result<AuthServerMetadata, OAuthError> {
    if let Some(cfg) = configured_url {
        if !cfg.starts_with("https://") {
            return Err(OAuthError::Discovery(format!(
                "authServerMetadataUrl must use https:// (got: {cfg})"
            )));
        }
        return get_json::<AuthServerMetadata>(http, cfg).await;
    }

    // RFC 9728: protected-resource probe → issuer → RFC 8414.
    let pr_url = well_known_url(server_url, "oauth-protected-resource");
    if let Ok(pr) = get_json::<ProtectedResourceMetadata>(http, &pr_url).await {
        if let Some(issuer) = pr.authorization_servers.first() {
            let as_url = well_known_url(issuer, "oauth-authorization-server");
            if let Ok(meta) = get_json::<AuthServerMetadata>(http, &as_url).await {
                return Ok(meta);
            }
        }
    }

    // Fallback: RFC 8414 directly against the server URL (path-aware).
    let as_url = well_known_url(server_url, "oauth-authorization-server");
    get_json::<AuthServerMetadata>(http, &as_url).await
}

// ---------------------------------------------------------------------------
// Dynamic Client Registration (RFC 7591, public client).
// ---------------------------------------------------------------------------

/// DCR request body (RFC 7591). Public client per claude-code's
/// `ClaudeAuthProvider.clientMetadata` (auth.ts:1417-1437):
/// `grant_types:[authorization_code, refresh_token]`, `response_types:[code]`,
/// `token_endpoint_auth_method:none`.
#[derive(Debug, Serialize)]
struct RegistrationRequest<'a> {
    redirect_uris: Vec<&'a str>,
    grant_types: Vec<&'a str>,
    response_types: Vec<&'a str>,
    token_endpoint_auth_method: &'a str,
    client_name: String,
    /// Advertised scope, mirrored into the client metadata when the auth server
    /// publishes one (auth.ts:1426-1434, `getScopeFromMetadata`). Omitted (per
    /// RFC 7591) when no scope is available.
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<&'a str>,
}

/// DCR response — we only need the issued `client_id`.
#[derive(Debug, Deserialize)]
struct RegistrationResponse {
    client_id: String,
}

/// Register a public client with the authorization server (RFC 7591).
///
/// # Errors
/// [`OAuthError::Registration`] on transport failure, non-2xx, or a body
/// without a `client_id`.
pub async fn register_client(
    http: &Arc<dyn HttpTransport>,
    registration_endpoint: &str,
    redirect_uri: &str,
    server_name: &str,
    scope: Option<&str>,
) -> Result<String, OAuthError> {
    let body = RegistrationRequest {
        redirect_uris: vec![redirect_uri],
        grant_types: vec!["authorization_code", "refresh_token"],
        response_types: vec!["code"],
        token_endpoint_auth_method: "none",
        // claude-code uses `Claude Code (${serverName})` (auth.ts:1419); keep
        // the LingXi rebrand but mirror the per-server suffix.
        client_name: format!("LingXi ({server_name})"),
        scope,
    };
    let body = serde_json::to_string(&body)
        .map_err(|e| OAuthError::Registration(format!("encode: {e}")))?;
    let req = HttpRequest {
        method: HttpMethod::Post,
        url: registration_endpoint.to_string(),
        headers: vec![
            ("content-type".into(), "application/json".into()),
            ("accept".into(), "application/json".into()),
        ],
        body: Some(body),
        body_bytes: None,
        timeout: Some(OAUTH_HTTP_TIMEOUT),
    };
    let resp = http
        .request(req)
        .await
        .map_err(|e| OAuthError::Registration(format!("transport: {e}")))?;
    // RFC 7591 mandates 201 Created; accept any 2xx for robustness.
    if !(200..300).contains(&resp.status) {
        return Err(OAuthError::Registration(format!(
            "status {}: {}",
            resp.status, resp.body
        )));
    }
    let parsed: RegistrationResponse = serde_json::from_str(&resp.body)
        .map_err(|e| OAuthError::Registration(format!("decode: {e}")))?;
    Ok(parsed.client_id)
}

// ---------------------------------------------------------------------------
// Authorization URL + token exchange + refresh.
// ---------------------------------------------------------------------------

/// Tokens returned by [`exchange_code`] / [`refresh_tokens`].
#[derive(Debug)]
pub struct Tokens {
    /// Bearer access token.
    pub access_token: Secret<String>,
    /// Refresh token, if the server issued one.
    pub refresh_token: Option<Secret<String>>,
    /// Wall-clock expiry instant (`clock.now() + expires_in`).
    pub expires_at: SystemTime,
    /// Effective OAuth `client_id` these tokens were minted with — the
    /// DCR-issued id (when the client was dynamically registered) or the
    /// configured id. Persisted so silent refresh re-sends the SAME `client_id`
    /// most public-client auth servers require (auth.ts `saveClientInformation`
    /// /`clientInformation`, 1482-1538). `None` only when unknown (legacy
    /// stored tokens, or a refresh whose origin client_id wasn't threaded).
    pub client_id: Option<String>,
}

/// Raw token-endpoint response (`authorization_code` / `refresh_token` grants).
#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: u64,
}

/// Build the authorization URL, returning `(url, verifier, state)`.
///
/// The caller (host/TUI) opens `url`, holds `verifier` until the loopback
/// callback fires, and the listener validates the redirect's `state` against
/// the returned token. Mirrors claude-code's authorize-URL construction
/// (`code_challenge_method=S256`, `scope`, `redirect_uri`).
#[must_use]
pub fn build_authorize_url(
    meta: &AuthServerMetadata,
    client_id: &str,
    redirect_uri: &str,
    scope: &str,
) -> (String, String, String) {
    let (verifier, challenge) = generate_pkce();
    let state = generate_state_token();
    let mut url = format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&state={}&code_challenge={}&code_challenge_method=S256",
        meta.authorization_endpoint,
        urlencoding::encode(client_id),
        urlencoding::encode(redirect_uri),
        urlencoding::encode(&state),
        urlencoding::encode(&challenge),
    );
    if !scope.is_empty() {
        url.push_str(&format!("&scope={}", urlencoding::encode(scope)));
    }
    (url, verifier, state)
}

/// POST a form-encoded grant to the token endpoint and parse the response,
/// computing `expires_at` against the injected clock.
async fn post_token_grant(
    http: &Arc<dyn HttpTransport>,
    clock: &Arc<dyn Clock>,
    token_endpoint: &str,
    form: &[(&str, &str)],
) -> Result<(Tokens, u16, String), OAuthError> {
    let body = form
        .iter()
        .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
        .collect::<Vec<_>>()
        .join("&");
    let req = HttpRequest {
        method: HttpMethod::Post,
        url: token_endpoint.to_string(),
        headers: vec![
            (
                "content-type".into(),
                "application/x-www-form-urlencoded".into(),
            ),
            ("accept".into(), "application/json".into()),
        ],
        body: Some(body),
        body_bytes: None,
        timeout: Some(OAUTH_HTTP_TIMEOUT),
    };
    let resp = http
        .request(req)
        .await
        .map_err(|e| OAuthError::Token(format!("transport: {e}")))?;
    if resp.status != 200 {
        // Caller maps the (status, body) to a specific error.
        return Err(OAuthError::Token(format!(
            "status {}: {}",
            resp.status, resp.body
        )));
    }
    let parsed: TokenResponse =
        serde_json::from_str(&resp.body).map_err(|e| OAuthError::Token(format!("decode: {e}")))?;
    let expires_at = clock.now() + Duration::from_secs(parsed.expires_in);
    let tokens = Tokens {
        access_token: Secret::new(parsed.access_token),
        refresh_token: parsed.refresh_token.map(Secret::new),
        expires_at,
        // Filled in by the caller, which knows the `client_id` used for the grant.
        client_id: None,
    };
    Ok((tokens, resp.status, resp.body))
}

/// Exchange an authorization `code` for an access + refresh token pair.
///
/// POSTs the `authorization_code` grant form-encoded (RFC 6749 / MCP SDK) to
/// `meta.token_endpoint`. The `redirect_uri` MUST match the authorize URL.
///
/// # Errors
/// [`OAuthError::Token`] on transport failure, non-200 status, or an
/// undecodable body.
pub async fn exchange_code(
    http: &Arc<dyn HttpTransport>,
    clock: &Arc<dyn Clock>,
    meta: &AuthServerMetadata,
    client_id: &str,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> Result<Tokens, OAuthError> {
    let form = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("client_id", client_id),
        ("code_verifier", verifier),
    ];
    let (mut tokens, _status, _body) =
        post_token_grant(http, clock, &meta.token_endpoint, &form).await?;
    tokens.client_id = Some(client_id.to_string());
    Ok(tokens)
}

/// Refresh an access token using a `refresh_token` (auth.ts `_doRefresh`).
///
/// Public-client form: `client_id` in the body, no `Authorization` header
/// (auth.ts:1421-1423, `token_endpoint_auth_method: none`).
///
/// # Errors
/// [`OAuthError::RefreshRejected`] when the server rejects the refresh token
/// (4xx / `invalid_grant`) — the caller must trigger a fresh interactive flow;
/// [`OAuthError::Token`] on other failures.
pub async fn refresh_tokens(
    http: &Arc<dyn HttpTransport>,
    clock: &Arc<dyn Clock>,
    meta: &AuthServerMetadata,
    client_id: &str,
    refresh_token: &str,
) -> Result<Tokens, OAuthError> {
    let form = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", client_id),
    ];
    match post_token_grant(http, clock, &meta.token_endpoint, &form).await {
        Ok((mut tokens, _status, _body)) => {
            // RFC 6749 §6: a refresh response MAY omit a new refresh token, in
            // which case the existing one stays valid — carry it forward.
            if tokens.refresh_token.is_none() {
                tokens.refresh_token = Some(Secret::new(refresh_token.to_string()));
            }
            // Persist the client_id used so the NEXT refresh re-sends it.
            tokens.client_id = Some(client_id.to_string());
            Ok(tokens)
        }
        Err(OAuthError::Token(msg)) if msg.starts_with("status 4") => {
            Err(OAuthError::RefreshRejected(msg))
        }
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------------------
// Full interactive flow orchestration.
// ---------------------------------------------------------------------------

/// Callback the host uses to surface the authorization URL (open a browser,
/// print a link, etc.). Invoked once, after the loopback listener has bound.
pub type OnAuthorizationUrl = Arc<dyn Fn(&str) + Send + Sync>;

/// The curated scope for a no-explicit-scope request — 1:1 with claude-code's
/// `getCuratedMetadataScope` (cc 2.1.196 fix: "MCP OAuth: no-scope request must
/// not ask for full `scopes_supported` catalog"):
///
/// 1. the non-standard `scope` metadata string when published, else
/// 2. the non-standard `default_scope` metadata string, else
/// 3. the full `scopes_supported` catalog ONLY when the user explicitly
///    configured `authServerMetadataUrl` for this server
///    (`has_explicit_metadata_url`), else
/// 4. EMPTY — the request carries no `scope` parameter at all.
///
/// Before the fix the catalog was requested unconditionally, over-asking the
/// authorization server for every advertised scope.
fn curated_metadata_scope(meta: &AuthServerMetadata, has_explicit_metadata_url: bool) -> String {
    if let Some(s) = meta.scope.as_deref().or(meta.default_scope.as_deref()) {
        return s.to_string();
    }
    if has_explicit_metadata_url {
        return meta
            .scopes_supported
            .as_ref()
            .map(|s| s.join(" "))
            .unwrap_or_default();
    }
    String::new()
}

/// Append `offline_access` to the authorize-URL scope when the server
/// advertises it — 1:1 with the binary's `D$p(e,t)`. `scope` mirrors JS `e`:
/// `None` is JS `null` (no scope resolved at all), `Some(s)` a resolved scope
/// string (possibly empty). The binary:
///
/// ```js
/// function D$p(e,t){
///   if(e!==null && e.split(" ").includes("offline_access")) return e;
///   if(!t?.scopes_supported?.includes("offline_access")) return e;
///   return e===null ? "offline_access" : `${e} offline_access`;
/// }
/// ```
///
/// Applied ONLY to the authorize URL (the binary's `redirectToAuthorization`),
/// never to the DCR client metadata.
fn with_offline_access(scope: Option<&str>, meta: &AuthServerMetadata) -> Option<String> {
    // if(e!==null && e.split(" ").includes("offline_access")) return e;
    if let Some(s) = scope {
        if s.split(' ').any(|x| x == "offline_access") {
            return Some(s.to_string());
        }
    }
    // if(!t?.scopes_supported?.includes("offline_access")) return e;
    let advertised = meta
        .scopes_supported
        .as_ref()
        .is_some_and(|s| s.iter().any(|x| x == "offline_access"));
    if !advertised {
        return scope.map(str::to_string);
    }
    // return e===null ? "offline_access" : `${e} offline_access`
    Some(match scope {
        None => "offline_access".to_string(),
        Some(s) => format!("{s} offline_access"),
    })
}

/// The final `scope` parameter for the authorize URL — 1:1 with the binary's
/// `redirectToAuthorization` scope resolution. `scope` is the already-resolved
/// request scope (step-up override or `curated_metadata_scope`), possibly
/// empty; `has_explicit_metadata_url` is whether the user configured
/// `authServerMetadataUrl`. Returns the string to place in the URL — empty ⇒
/// omit the `scope` parameter entirely.
///
/// ```js
/// t = authServerMetadataUrl ? getCuratedMetadataScope() : undefined
/// n = <scope already in the URL>              // absent when empty
/// r = t ?? n                                  // JS nullish
/// o = r === null ? null : D$p(r, metadata)    // offline_access appender
/// // URL keeps `o` when non-null, else `n`
/// ```
///
/// The `r === null` guard is why a genuine no-scope request (no explicit
/// metadata URL, no curated scope) stays scope-less even when the server
/// advertises `offline_access` — the cc 2.1.196 "don't over-ask" fix.
fn authorize_url_scope(
    scope: &str,
    has_explicit_metadata_url: bool,
    meta: &AuthServerMetadata,
) -> String {
    // n = the scope actually placed in the URL (absent when empty).
    let n: Option<&str> = (!scope.is_empty()).then_some(scope);
    // t = authServerMetadataUrl ? getCuratedMetadataScope() : undefined.
    let t: Option<&str> = has_explicit_metadata_url.then_some(scope);
    // r = t ?? n.
    let r: Option<&str> = t.or(n);
    // o = r === null ? null : D$p(r); URL carries `o` when non-null.
    r.and_then(|rs| with_offline_access(Some(rs), meta))
        .unwrap_or_default()
}

/// Drive the full interactive OAuth flow for a remote MCP server:
/// discovery → (DCR) → PKCE → bind loopback listener → build authorize URL →
/// surface it via `on_auth_url` → accept the redirect → exchange the code.
///
/// `server_url` is the MCP endpoint; `server_name` labels the DCR client
/// (`LingXi (${server_name})`); `oauth` is the static config DTO (its
/// `client_id` skips DCR, its `callback_port` pins the loopback port, its
/// `auth_server_metadata_url` overrides discovery).
///
/// # Errors
/// Any [`OAuthError`] from the constituent steps.
pub async fn perform_oauth_flow(
    http: &Arc<dyn HttpTransport>,
    clock: &Arc<dyn Clock>,
    oauth: &traits::McpOAuthConfigDto,
    server_name: &str,
    server_url: &str,
    on_auth_url: &OnAuthorizationUrl,
    scope_override: Option<&str>,
) -> Result<Tokens, OAuthError> {
    // 1. Discovery.
    let meta =
        discover_auth_server_metadata(http, server_url, oauth.auth_server_metadata_url.as_deref())
            .await?;

    // 2. Bind the loopback listener FIRST so the redirect_uri is known before
    //    the authorize URL is built (claude-code's listen(0) pattern). A
    //    configured `callback_port` pins the port; otherwise the OS assigns one.
    let listener = CallbackListener::bind(oauth.callback_port.unwrap_or(0)).await?;
    let port = listener.port();
    let redirect_uri = format!("http://localhost:{port}/callback");

    // 3. Advertised scope (used both for DCR client metadata and the authorize
    //    URL). A `scope_override` (a cached step-up scope from a prior 403
    //    `insufficient_scope`) takes precedence so the authorize URL requests
    //    the elevated scope (auth.ts:909-935 / 1625-1637). Otherwise the scope
    //    is the CURATED metadata scope — cc 2.1.196 fix, binary
    //    `getCuratedMetadataScope`: the non-standard `scope` / `default_scope`
    //    metadata strings when published, and the full `scopes_supported`
    //    catalog ONLY when the user explicitly configured
    //    `authServerMetadataUrl` (`if(this.serverConfig.oauth?.
    //    authServerMetadataUrl && Array.isArray(this._metadata?.
    //    scopes_supported)) return this._metadata.scopes_supported.join(" ")`).
    //    With no scope specified anywhere the request carries NO scope — it
    //    must NOT ask for the whole advertised catalog.
    let scope = match scope_override {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => curated_metadata_scope(&meta, oauth.auth_server_metadata_url.is_some()),
    };

    // 4. Client id — configured, else dynamic client registration.
    let client_id = if let Some(id) = &oauth.client_id {
        id.clone()
    } else {
        let reg = meta.registration_endpoint.as_deref().ok_or_else(|| {
            OAuthError::Registration(
                "no client_id configured and server advertises no registration_endpoint".into(),
            )
        })?;
        let dcr_scope = (!scope.is_empty()).then_some(scope.as_str());
        register_client(http, reg, &redirect_uri, server_name, dcr_scope).await?
    };

    // 5. Authorize URL (PKCE inside) + surface it to the host. The authorize
    //    scope (unlike the DCR scope) gains `offline_access` when the server
    //    advertises it (binary `redirectToAuthorization` → `D$p`), so refresh
    //    tokens keep being issued now that the catalog is no longer requested.
    //    Mirror the binary's scope resolution 1:1 (`redirectToAuthorization`):
    //      t = authServerMetadataUrl ? getCuratedMetadataScope() : undefined
    //      n = the scope actually placed in the URL (absent when empty)
    //      r = t ?? n            (JS nullish: t="" is kept; undefined → n)
    //      o = r === null ? null : D$p(r, metadata)
    //    Only when `o` is non-null does the URL carry a scope, so a genuine
    //    no-scope request (no explicit metadata URL + no curated scope) stays
    //    scope-less even when the server advertises `offline_access`.
    let authorize_scope =
        authorize_url_scope(&scope, oauth.auth_server_metadata_url.is_some(), &meta);
    let (auth_url, verifier, state) =
        build_authorize_url(&meta, &client_id, &redirect_uri, &authorize_scope);
    on_auth_url(&auth_url);

    // 6. Wait for the redirect, validate state, capture the code. `redirect_uri`
    //    is echoed into the listener's 404 page ("registered redirect_uri must
    //    be {T}").
    let params = listener.accept(&state, &redirect_uri).await?;

    // 7. Exchange the code for tokens.
    exchange_code(
        http,
        clock,
        &meta,
        &client_id,
        &params.code,
        &verifier,
        &redirect_uri,
    )
    .await
}

// ---------------------------------------------------------------------------
// getServerKey (auth.ts:325-341) + secure-storage persistence.
// ---------------------------------------------------------------------------

/// `lingxi-secret` service name under which MCP OAuth tokens are stored.
pub const MCP_OAUTH_SERVICE: &str = "mcp-oauth";

/// Stable per-server credential key: `name|sha256({type,url,headers})[..16]`.
///
/// Byte-for-byte the shape of claude-code's `getServerKey` (auth.ts:325-341):
/// the config hash is `sha256` of a JSON object with exactly `{type, url,
/// headers}` (headers default to `{}`), hex, first 16 chars.
#[must_use]
pub fn server_key(name: &str, spec: &McpTransportSpec) -> String {
    let empty: traits::McpHeaders = traits::McpHeaders::new();
    let (kind, url, headers) = match spec {
        McpTransportSpec::Sse { url, headers, .. } => ("sse", url.as_str(), headers),
        McpTransportSpec::Http { url, headers, .. } => ("http", url.as_str(), headers),
        // Non-remote specs never reach OAuth; fall back to the kind label.
        other => (other.kind(), "", &empty),
    };
    // claude-code: `jsonStringify({type, url, headers})` == plain
    // `JSON.stringify`, which serializes object keys in insertion order
    // (auth.ts:329-333, slowOperations.ts:189). Object key order is therefore
    // `type, url, headers`, and the `headers` object preserves the config's
    // header insertion order (NOT sorted). `serde_json` with `preserve_order`
    // keeps the top-level `json!` keys in source order, and `McpHeaders`
    // (`IndexMap`) serializes its entries in insertion order — so this
    // byte-matches `getServerKey` for any header count/ordering.
    let config_json = serde_json::json!({
        "type": kind,
        "url": url,
        "headers": headers,
    });
    let config_str = serde_json::to_string(&config_json).unwrap_or_default();
    let mut h = Sha256::new();
    h.update(config_str.as_bytes());
    let hash = format!("{:x}", h.finalize());
    format!("{name}|{}", &hash[..16])
}

/// On-disk JSON shape for a stored MCP OAuth token set. The secret strings are
/// the byte payload of a [`protocol::SecureStorageData`].
#[derive(Debug, Serialize, Deserialize)]
pub struct StoredTokens {
    /// Bearer access token (plain string inside the encrypted blob).
    pub access_token: String,
    /// Optional refresh token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// Expiry as seconds since the Unix epoch.
    pub expires_at_unix: u64,
    /// Effective OAuth `client_id` (DCR-issued or configured) these tokens were
    /// minted with — re-sent on silent refresh so public-client auth servers
    /// don't reject an empty `client_id` and force a fresh interactive flow
    /// (auth.ts `saveClientInformation`/`clientInformation`). `#[serde(default)]`
    /// makes legacy blobs without the field deserialize to `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// Confidential-client secret these tokens were minted with. Only set for
    /// XAA (cross-app-access) tokens, whose AS uses a confidential client —
    /// strict ASes reject public-client revocation of confidential tokens
    /// (auth.ts:408-410, `revokeToken` `clientSecret`). `None` for ordinary
    /// public-client OAuth. `#[serde(default)]` keeps legacy blobs valid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    /// Elevated scope cached when a 403 `insufficient_scope` step-up is pending
    /// (auth.ts `stepUpScope`, 1896 / 909). The next interactive flow requests
    /// this scope instead of (re-)probing; cleared once a fresh grant succeeds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_up_scope: Option<String>,
}

impl StoredTokens {
    /// Project to live [`Tokens`] (wraps the strings back into `Secret`).
    #[must_use]
    pub fn into_tokens(self) -> Tokens {
        Tokens {
            access_token: Secret::new(self.access_token),
            refresh_token: self.refresh_token.map(Secret::new),
            expires_at: SystemTime::UNIX_EPOCH + Duration::from_secs(self.expires_at_unix),
            client_id: self.client_id,
        }
    }

    /// Wall-clock expiry of the stored access token (borrowing, no move).
    #[must_use]
    pub fn expires_at(&self) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(self.expires_at_unix)
    }

    /// Build from live [`Tokens`].
    #[must_use]
    pub fn from_tokens(t: &Tokens) -> Self {
        let expires_at_unix = t
            .expires_at
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs();
        Self {
            access_token: t.access_token.expose_secret().clone(),
            refresh_token: t.refresh_token.as_ref().map(|s| s.expose_secret().clone()),
            expires_at_unix,
            client_id: t.client_id.clone(),
            // Ordinary OAuth tokens are public-client and carry no step-up
            // scope; XAA wiring sets these directly on the stored blob.
            client_secret: None,
            step_up_scope: None,
        }
    }
}

/// Load the stored token set for `key`, if any.
///
/// # Errors
/// [`OAuthError::Token`] on a storage backend error.
pub async fn load_tokens(
    storage: &Arc<dyn traits::SecureStorage>,
    key: &str,
) -> Result<Option<StoredTokens>, OAuthError> {
    let data = storage
        .retrieve(MCP_OAUTH_SERVICE, key)
        .await
        .map_err(|e| OAuthError::Token(format!("storage retrieve: {e}")))?;
    let Some(data) = data else { return Ok(None) };
    let parsed: StoredTokens = serde_json::from_slice(data.expose_secret_bytes())
        .map_err(|e| OAuthError::Token(format!("decode stored tokens: {e}")))?;
    Ok(Some(parsed))
}

/// Persist an already-built [`StoredTokens`] blob for `key` (used to update
/// side fields like `step_up_scope` without minting fresh [`Tokens`]).
///
/// # Errors
/// [`OAuthError::Token`] on encode or a storage backend error.
pub async fn store_tokens(
    storage: &Arc<dyn traits::SecureStorage>,
    clock: &Arc<dyn Clock>,
    key: &str,
    stored: &StoredTokens,
) -> Result<(), OAuthError> {
    let bytes =
        serde_json::to_vec(stored).map_err(|e| OAuthError::Token(format!("encode tokens: {e}")))?;
    let metadata = protocol::SecureStorageMetadata {
        created_at: clock.now(),
        last_accessed: None,
        kind: protocol::SecretKindDto("mcp_oauth_tokens".into()),
    };
    let data = protocol::SecureStorageData::new(bytes, metadata);
    storage
        .store(MCP_OAUTH_SERVICE, key, data)
        .await
        .map_err(|e| OAuthError::Token(format!("storage store: {e}")))?;
    Ok(())
}

/// Persist a token set for `key` (`mcp-oauth` service, account = server key).
///
/// # Errors
/// [`OAuthError::Token`] on encode or a storage backend error.
pub async fn save_tokens(
    storage: &Arc<dyn traits::SecureStorage>,
    clock: &Arc<dyn Clock>,
    key: &str,
    tokens: &Tokens,
) -> Result<(), OAuthError> {
    let stored = StoredTokens::from_tokens(tokens);
    let bytes = serde_json::to_vec(&stored)
        .map_err(|e| OAuthError::Token(format!("encode tokens: {e}")))?;
    let metadata = protocol::SecureStorageMetadata {
        created_at: clock.now(),
        last_accessed: None,
        kind: protocol::SecretKindDto("mcp_oauth_tokens".into()),
    };
    let data = protocol::SecureStorageData::new(bytes, metadata);
    storage
        .store(MCP_OAUTH_SERVICE, key, data)
        .await
        .map_err(|e| OAuthError::Token(format!("storage store: {e}")))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Token revocation (RFC 7009) — auth.ts:365-618.
// ---------------------------------------------------------------------------

/// Which credential a revocation request targets (RFC 7009 `token_type_hint`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenTypeHint {
    /// The bearer access token.
    AccessToken,
    /// The long-lived refresh token.
    RefreshToken,
}

impl TokenTypeHint {
    fn as_str(self) -> &'static str {
        match self {
            TokenTypeHint::AccessToken => "access_token",
            TokenTypeHint::RefreshToken => "refresh_token",
        }
    }
}

/// Revoke a single token at the AS revocation endpoint (RFC 7009).
///
/// Byte-for-byte port of `revokeToken` (auth.ts:381-459):
/// 1. RFC-7009-compliant attempt — `token` + `token_type_hint`, client auth via
///    `client_secret_basic` (base64 `Authorization: Basic`) or
///    `client_secret_post` (creds in the form), else bare `client_id` in the
///    body for a public client.
/// 2. On a `401`, retry once with `Authorization: Bearer <access_token>` for
///    non-compliant servers, having cleared the client creds from the body
///    (RFC 6749 §2.3.1: at most one auth method).
///
/// Best-effort; returns `Err` only when both attempts fail (the caller logs and
/// continues per auth.ts).
#[allow(clippy::too_many_arguments)]
pub async fn revoke_token(
    http: &Arc<dyn HttpTransport>,
    endpoint: &str,
    token: &str,
    token_type_hint: TokenTypeHint,
    client_id: Option<&str>,
    client_secret: Option<&str>,
    access_token: Option<&str>,
    auth_method: &str,
) -> Result<(), OAuthError> {
    // Base form (`token`, `token_type_hint`) + optional client_secret_post creds.
    let hint = token_type_hint.as_str();
    let mut form: Vec<(String, String)> = vec![
        ("token".into(), token.into()),
        ("token_type_hint".into(), hint.into()),
    ];
    let mut headers: Vec<(String, String)> = vec![(
        "content-type".into(),
        "application/x-www-form-urlencoded".into(),
    )];

    // auth.ts:411-428 client-auth precedence.
    match (client_id, client_secret) {
        (Some(id), Some(secret)) => {
            if auth_method == "client_secret_post" {
                form.push(("client_id".into(), id.into()));
                form.push(("client_secret".into(), secret.into()));
            } else {
                // client_secret_basic: base64(urlencode(id):urlencode(secret)).
                let basic = format!(
                    "{}:{}",
                    urlencoding::encode(id),
                    urlencoding::encode(secret)
                );
                headers.push((
                    "authorization".into(),
                    format!(
                        "Basic {}",
                        base64::engine::general_purpose::STANDARD.encode(basic)
                    ),
                ));
            }
        }
        (Some(id), None) => form.push(("client_id".into(), id.into())),
        // No client_id — server may reject (auth.ts:424-427); attempt anyway.
        (None, _) => {}
    }

    let encode_body = |form: &[(String, String)]| {
        form.iter()
            .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
            .collect::<Vec<_>>()
            .join("&")
    };

    let req = HttpRequest {
        method: HttpMethod::Post,
        url: endpoint.to_string(),
        headers: headers.clone(),
        body: Some(encode_body(&form)),
        body_bytes: None,
        timeout: Some(OAUTH_HTTP_TIMEOUT),
    };
    let resp = http
        .request(req)
        .await
        .map_err(|e| OAuthError::Token(format!("revoke transport: {e}")))?;
    if (200..300).contains(&resp.status) {
        return Ok(());
    }

    // auth.ts:434-457: 401 fallback → retry with Bearer, clearing client creds.
    if resp.status == 401 {
        if let Some(at) = access_token {
            form.retain(|(k, _)| k != "client_id" && k != "client_secret");
            let mut retry_headers: Vec<(String, String)> = headers
                .into_iter()
                .filter(|(k, _)| !k.eq_ignore_ascii_case("authorization"))
                .collect();
            retry_headers.push(("authorization".into(), format!("Bearer {at}")));
            let req = HttpRequest {
                method: HttpMethod::Post,
                url: endpoint.to_string(),
                headers: retry_headers,
                body: Some(encode_body(&form)),
                body_bytes: None,
                timeout: Some(OAUTH_HTTP_TIMEOUT),
            };
            let resp = http
                .request(req)
                .await
                .map_err(|e| OAuthError::Token(format!("revoke retry transport: {e}")))?;
            if (200..300).contains(&resp.status) {
                return Ok(());
            }
            return Err(OAuthError::Token(format!(
                "revoke retry status {}: {}",
                resp.status, resp.body
            )));
        }
    }
    Err(OAuthError::Token(format!(
        "revoke status {}: {}",
        resp.status, resp.body
    )))
}

/// Revoke a server's tokens at the AS, then unconditionally clear them locally.
///
/// Faithful core of `revokeServerTokens` (auth.ts:467-577): load the stored
/// tokens, discover AS metadata, read `revocation_endpoint` (skip on absent),
/// pick the auth method per auth.ts:503-517 (prefer the revocation list, else
/// the token-endpoint list; `client_secret_post` only when `basic` is absent
/// and `post` present), then revoke **refresh first, then access** — each
/// best-effort. The local token blob is ALWAYS deleted afterwards regardless of
/// the server-side result (auth.ts:575-576).
///
/// `preserveStepUpState` (auth.ts:578-617) is a re-auth-only variant — not the
/// logout/disconnect path wired here — and is a noted residual.
pub async fn revoke_server_tokens(
    storage: &Arc<dyn traits::SecureStorage>,
    http: &Arc<dyn HttpTransport>,
    key: &str,
    server_url: &str,
    oauth_cfg: &traits::McpOAuthConfigDto,
) {
    let stored = match load_tokens(storage, key).await {
        Ok(Some(s)) => Some(s),
        Ok(None) => None,
        Err(e) => {
            tracing::debug!(error = %e, "mcp oauth: failed to load tokens for revocation");
            None
        }
    };

    if let Some(stored) = &stored {
        let has_access = !stored.access_token.is_empty();
        let has_refresh = stored
            .refresh_token
            .as_deref()
            .is_some_and(|s| !s.is_empty());
        if has_access || has_refresh {
            // Best-effort server-side revocation; never propagate failures.
            if let Err(e) = revoke_at_endpoint(http, server_url, oauth_cfg, stored).await {
                tracing::debug!(error = %e, "mcp oauth: token revocation failed (best-effort)");
            }
        } else {
            tracing::debug!("mcp oauth: no tokens to revoke");
        }
    }

    // Always clear local tokens, regardless of server-side result (auth.ts:575).
    if let Err(e) = storage.delete(MCP_OAUTH_SERVICE, key).await {
        tracing::debug!(error = %e, "mcp oauth: failed to clear local tokens after revocation");
    }
}

/// Inner discovery + per-token revocation (the `try` block of auth.ts:481-570).
async fn revoke_at_endpoint(
    http: &Arc<dyn HttpTransport>,
    server_url: &str,
    oauth_cfg: &traits::McpOAuthConfigDto,
    stored: &StoredTokens,
) -> Result<(), OAuthError> {
    let meta = discover_auth_server_metadata(
        http,
        server_url,
        oauth_cfg.auth_server_metadata_url.as_deref(),
    )
    .await?;

    let Some(endpoint) = meta.revocation_endpoint.as_deref() else {
        tracing::debug!("mcp oauth: server does not support token revocation");
        return Ok(());
    };

    // auth.ts:503-517 auth-method selection.
    let methods = meta
        .revocation_endpoint_auth_methods_supported
        .as_ref()
        .or(meta.token_endpoint_auth_methods_supported.as_ref());
    let auth_method = match methods {
        Some(m)
            if !m.iter().any(|s| s == "client_secret_basic")
                && m.iter().any(|s| s == "client_secret_post") =>
        {
            "client_secret_post"
        }
        _ => "client_secret_basic",
    };

    let client_id = stored
        .client_id
        .as_deref()
        .or(oauth_cfg.client_id.as_deref());
    let client_secret = stored.client_secret.as_deref();
    let access_token = (!stored.access_token.is_empty()).then_some(stored.access_token.as_str());

    // Refresh token first (auth.ts:523-543), best-effort.
    if let Some(refresh) = stored.refresh_token.as_deref().filter(|s| !s.is_empty()) {
        if let Err(e) = revoke_token(
            http,
            endpoint,
            refresh,
            TokenTypeHint::RefreshToken,
            client_id,
            client_secret,
            access_token,
            auth_method,
        )
        .await
        {
            tracing::debug!(error = %e, "mcp oauth: failed to revoke refresh token");
        }
    }

    // Then access token (auth.ts:545-564), best-effort.
    if let Some(at) = access_token {
        if let Err(e) = revoke_token(
            http,
            endpoint,
            at,
            TokenTypeHint::AccessToken,
            client_id,
            client_secret,
            access_token,
            auth_method,
        )
        .await
        {
            tracing::debug!(error = %e, "mcp oauth: failed to revoke access token");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifier_and_challenge_distinct_and_url_safe() {
        let (v1, c1) = generate_pkce();
        let (v2, _c2) = generate_pkce();
        assert_ne!(v1, v2, "verifier differs per call");
        assert_ne!(v1, c1, "verifier != challenge");
        assert!(!c1.contains('+'));
        assert!(!c1.contains('/'));
        assert!(!c1.contains('='));
    }

    #[test]
    fn well_known_url_root_and_path_aware() {
        assert_eq!(
            well_known_url("https://mcp.example.com", "oauth-protected-resource"),
            "https://mcp.example.com/.well-known/oauth-protected-resource"
        );
        assert_eq!(
            well_known_url("https://mcp.example.com/", "oauth-authorization-server"),
            "https://mcp.example.com/.well-known/oauth-authorization-server"
        );
        // Path-aware form inserts well-known after host, appends path.
        assert_eq!(
            well_known_url(
                "https://mcp.example.com/tenant/a",
                "oauth-authorization-server"
            ),
            "https://mcp.example.com/.well-known/oauth-authorization-server/tenant/a"
        );
    }

    #[test]
    fn server_key_matches_claude_code_shape() {
        // name|sha256({type,url,headers})[..16]; headers default {}.
        let spec = McpTransportSpec::Http {
            url: "https://mcp.example.com/v1".into(),
            headers: traits::McpHeaders::new(),
            oauth: None,
        };
        let key = server_key("acme", &spec);
        let (name, hash) = key.split_once('|').unwrap();
        assert_eq!(name, "acme");
        assert_eq!(hash.len(), 16);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));

        // Deterministic for identical config; differs when the URL changes.
        let key2 = server_key("acme", &spec);
        assert_eq!(key, key2);
        let spec_other = McpTransportSpec::Http {
            url: "https://mcp.example.com/v2".into(),
            headers: traits::McpHeaders::new(),
            oauth: None,
        };
        assert_ne!(server_key("acme", &spec_other), key);
    }

    #[test]
    fn build_authorize_url_carries_pkce_state_and_redirect() {
        let meta = AuthServerMetadata {
            authorization_endpoint: "https://as.example.com/authorize".into(),
            token_endpoint: "https://as.example.com/token".into(),
            registration_endpoint: None,
            scopes_supported: None,
            scope: None,
            default_scope: None,
            revocation_endpoint: None,
            revocation_endpoint_auth_methods_supported: None,
            token_endpoint_auth_methods_supported: None,
        };
        let (url, verifier, state) =
            build_authorize_url(&meta, "client-123", "http://localhost:5000/callback", "");
        assert!(url.starts_with("https://as.example.com/authorize?"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("client_id=client-123"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains(&format!("state={state}")));
        assert!(url.contains("redirect_uri=http%3A%2F%2Flocalhost%3A5000%2Fcallback"));
        // No scope param when empty.
        assert!(!url.contains("scope="));
        assert!(!verifier.is_empty());
    }

    /// Metadata builder for the curated-scope tests (all optional fields off).
    fn meta_with(
        scopes_supported: Option<Vec<&str>>,
        scope: Option<&str>,
        default_scope: Option<&str>,
    ) -> AuthServerMetadata {
        AuthServerMetadata {
            authorization_endpoint: "https://as.example.com/authorize".into(),
            token_endpoint: "https://as.example.com/token".into(),
            registration_endpoint: None,
            scopes_supported: scopes_supported
                .map(|v| v.into_iter().map(String::from).collect()),
            scope: scope.map(String::from),
            default_scope: default_scope.map(String::from),
            revocation_endpoint: None,
            revocation_endpoint_auth_methods_supported: None,
            token_endpoint_auth_methods_supported: None,
        }
    }

    /// cc 2.1.196 fix (`getCuratedMetadataScope`): with no scope specified and
    /// no explicit `authServerMetadataUrl`, the full `scopes_supported`
    /// catalog must NOT be requested — the resolved scope is empty.
    #[test]
    fn no_scope_request_does_not_ask_for_scopes_supported_catalog() {
        let meta = meta_with(Some(vec!["read", "write", "admin"]), None, None);
        assert_eq!(curated_metadata_scope(&meta, false), "");
    }

    /// The catalog IS used when the user explicitly configured
    /// `authServerMetadataUrl` (the binary's `serverConfig.oauth?.
    /// authServerMetadataUrl` gate).
    #[test]
    fn explicit_metadata_url_still_uses_the_advertised_catalog() {
        let meta = meta_with(Some(vec!["read", "write"]), None, None);
        assert_eq!(curated_metadata_scope(&meta, true), "read write");
        // No catalog advertised → still empty.
        let bare = meta_with(None, None, None);
        assert_eq!(curated_metadata_scope(&bare, true), "");
    }

    /// The non-standard curated `scope` / `default_scope` metadata strings win
    /// over the catalog regardless of the metadata-url gate (`Q$a` order:
    /// `scope`, then `default_scope`).
    #[test]
    fn curated_scope_and_default_scope_strings_take_precedence() {
        let meta = meta_with(Some(vec!["a", "b"]), Some("curated"), Some("dflt"));
        assert_eq!(curated_metadata_scope(&meta, false), "curated");
        assert_eq!(curated_metadata_scope(&meta, true), "curated");
        let meta = meta_with(Some(vec!["a", "b"]), None, Some("dflt"));
        assert_eq!(curated_metadata_scope(&meta, false), "dflt");
    }

    /// `D$p`: `offline_access` is appended to the authorize scope only when the
    /// server advertises it, never duplicated, and used bare (JS `e===null`
    /// branch) when no scope was resolved at all. `None` mirrors JS `null`.
    #[test]
    fn offline_access_appended_only_when_advertised() {
        let advertises = meta_with(Some(vec!["read", "offline_access"]), None, None);
        let not_advertised = meta_with(Some(vec!["read"]), None, None);
        // Appended when advertised.
        assert_eq!(
            with_offline_access(Some("read"), &advertises),
            Some("read offline_access".to_string())
        );
        // Bare `offline_access` when the resolved scope is null (`e===null`).
        assert_eq!(
            with_offline_access(None, &advertises),
            Some("offline_access".to_string())
        );
        // An explicit EMPTY-STRING scope is NOT null: `${e} offline_access`
        // keeps the leading space, matching `D$p("")`.
        assert_eq!(
            with_offline_access(Some(""), &advertises),
            Some(" offline_access".to_string())
        );
        // Never duplicated.
        assert_eq!(
            with_offline_access(Some("read offline_access"), &advertises),
            Some("read offline_access".to_string())
        );
        // Untouched when not advertised (null stays null).
        assert_eq!(
            with_offline_access(Some("read"), &not_advertised),
            Some("read".to_string())
        );
        assert_eq!(with_offline_access(None, &not_advertised), None);
        let none = meta_with(None, None, None);
        assert_eq!(
            with_offline_access(Some("read"), &none),
            Some("read".to_string())
        );
    }

    /// Caller-level (`redirectToAuthorization`): a genuine no-scope request (no
    /// explicit metadata URL, no curated scope) carries NO `scope` param even
    /// when the server advertises `offline_access`. This is the `r === null`
    /// guard — without it the port would over-ask `scope=offline_access`.
    #[test]
    fn authorize_scope_no_scope_no_url_stays_scopeless_even_with_offline_access() {
        // Server advertises offline_access, user set no scope & no metadata URL.
        let meta = meta_with(Some(vec!["read", "offline_access"]), None, None);
        assert_eq!(authorize_url_scope("", false, &meta), "");
        // Even with an empty catalog it stays scope-less.
        let bare = meta_with(None, None, None);
        assert_eq!(authorize_url_scope("", false, &bare), "");
    }

    /// A resolved non-empty scope (curated string / step-up override) DOES gain
    /// `offline_access` when advertised, regardless of the metadata-URL gate.
    #[test]
    fn authorize_scope_nonempty_gets_offline_access_when_advertised() {
        let meta = meta_with(Some(vec!["read", "offline_access"]), None, None);
        assert_eq!(authorize_url_scope("read", false, &meta), "read offline_access");
        assert_eq!(authorize_url_scope("read", true, &meta), "read offline_access");
        // Not advertised → left untouched.
        let no_off = meta_with(Some(vec!["read"]), None, None);
        assert_eq!(authorize_url_scope("read", false, &no_off), "read");
    }

    /// With an explicit metadata URL the catalog is requested (t = curated) and
    /// still gains offline_access when advertised.
    #[test]
    fn authorize_scope_explicit_url_requests_catalog() {
        let meta = meta_with(Some(vec!["read", "write", "offline_access"]), None, None);
        // curated_metadata_scope(explicit=true) joins the catalog; feed that in.
        let curated = curated_metadata_scope(&meta, true);
        assert_eq!(curated, "read write offline_access");
        assert_eq!(
            authorize_url_scope(&curated, true, &meta),
            "read write offline_access"
        );
    }

    // -- FIX 2: server_key uses config (insertion) header order, not sorted. ---

    /// Byte-parity: `getServerKey` hashes `JSON.stringify({type,url,headers})`
    /// in header *insertion* order (auth.ts:329-333, slowOperations.ts:189),
    /// NOT sorted. A config with headers `{Z, A}` must hash the `Z`-first
    /// material — pinned here against the reference value computed from
    /// claude-code's exact stringify, and shown to differ from the old sorted
    /// hash the BTreeMap path produced.
    #[test]
    fn server_key_uses_insertion_order_not_sorted() {
        let mut headers = traits::McpHeaders::new();
        headers.insert("Z-Header".to_string(), "z".to_string());
        headers.insert("A-Header".to_string(), "a".to_string());
        let spec = McpTransportSpec::Http {
            url: "https://mcp.example.com/v1".into(),
            headers,
            oauth: None,
        };
        let key = server_key("acme", &spec);
        // Reference: sha256(JSON.stringify({type:"http",url:"…/v1",
        //   headers:{"Z-Header":"z","A-Header":"a"}}))[..16]  (Node, see test).
        assert_eq!(key, "acme|b555b45e666ffa13");
        // The OLD sorted (BTreeMap) path would have produced this — must differ.
        assert_ne!(key, "acme|08e07ebc60543bed");
    }

    /// Empty-headers material still matches claude-code (`headers:{}`).
    #[test]
    fn server_key_empty_headers_matches_reference() {
        let spec = McpTransportSpec::Http {
            url: "https://mcp.example.com/v1".into(),
            headers: traits::McpHeaders::new(),
            oauth: None,
        };
        assert_eq!(server_key("acme", &spec), "acme|f729261a8041fc55");
    }

    // -- FIX 1: DCR client_id round-trips through StoredTokens + into refresh. --

    /// A DCR-minted token set persists its `client_id` and projects it back.
    #[test]
    fn stored_tokens_round_trip_client_id() {
        let tokens = Tokens {
            access_token: Secret::new("at".into()),
            refresh_token: Some(Secret::new("rt".into())),
            expires_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1000),
            client_id: Some("dcr-client-xyz".into()),
        };
        let stored = StoredTokens::from_tokens(&tokens);
        assert_eq!(stored.client_id.as_deref(), Some("dcr-client-xyz"));
        // Survives a JSON round-trip (the on-disk blob shape).
        let json = serde_json::to_string(&stored).unwrap();
        assert!(json.contains("\"client_id\":\"dcr-client-xyz\""));
        let back: StoredTokens = serde_json::from_str(&json).unwrap();
        assert_eq!(back.client_id.as_deref(), Some("dcr-client-xyz"));
        assert_eq!(
            back.into_tokens().client_id.as_deref(),
            Some("dcr-client-xyz")
        );
    }

    /// Legacy blobs (no `client_id` field) deserialize to `None`, not an error.
    #[test]
    fn stored_tokens_legacy_without_client_id_is_none() {
        let legacy = r#"{"access_token":"at","refresh_token":"rt","expires_at_unix":1000}"#;
        let parsed: StoredTokens = serde_json::from_str(legacy).unwrap();
        assert_eq!(parsed.client_id, None);
        assert_eq!(parsed.into_tokens().client_id, None);
    }

    /// `refresh_tokens` carries the `client_id` it was given onto the result so
    /// it persists for the next refresh, AND the form it POSTs is non-empty.
    /// (Exercised against the live wire in `tests/oauth_flow_test.rs`; here we
    /// assert the in-struct propagation that backs FIX 1.)
    #[test]
    fn exchange_and_refresh_propagate_client_id_into_tokens() {
        // Construct a Tokens as exchange_code/refresh_tokens would, then verify
        // from_tokens persists the id that resolve_oauth_spec/reauth re-send.
        let exchanged = Tokens {
            access_token: Secret::new("at".into()),
            refresh_token: Some(Secret::new("rt".into())),
            expires_at: SystemTime::UNIX_EPOCH + Duration::from_secs(10),
            client_id: Some("the-client".into()),
        };
        assert_eq!(
            StoredTokens::from_tokens(&exchanged).client_id.as_deref(),
            Some("the-client"),
            "client_id from a grant must persist so refresh re-sends it (not empty)"
        );
    }
}

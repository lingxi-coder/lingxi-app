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
//! **Residual (noted, not ported — see task scope):** XAA cross-app-access
//! (`performMCPXaaAuth`), CIMD (`client_id_metadata_document`), step-up scope
//! (403 `insufficient_scope`), token revocation (RFC 7009), Slack-style
//! `200`-with-error-body normalization, cross-process lockfile refresh
//! coordination, and analytics events.

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
    client_name: &'a str,
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
) -> Result<String, OAuthError> {
    let body = RegistrationRequest {
        redirect_uris: vec![redirect_uri],
        grant_types: vec!["authorization_code", "refresh_token"],
        response_types: vec!["code"],
        token_endpoint_auth_method: "none",
        client_name: "LingXi",
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
    let parsed: TokenResponse = serde_json::from_str(&resp.body)
        .map_err(|e| OAuthError::Token(format!("decode: {e}")))?;
    let expires_at = clock.now() + Duration::from_secs(parsed.expires_in);
    let tokens = Tokens {
        access_token: Secret::new(parsed.access_token),
        refresh_token: parsed.refresh_token.map(Secret::new),
        expires_at,
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
    let (tokens, _status, _body) =
        post_token_grant(http, clock, &meta.token_endpoint, &form).await?;
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

/// Drive the full interactive OAuth flow for a remote MCP server:
/// discovery → (DCR) → PKCE → bind loopback listener → build authorize URL →
/// surface it via `on_auth_url` → accept the redirect → exchange the code.
///
/// `server_url` is the MCP endpoint; `oauth` is the static config DTO (its
/// `client_id` skips DCR, its `callback_port` pins the loopback port, its
/// `auth_server_metadata_url` overrides discovery).
///
/// # Errors
/// Any [`OAuthError`] from the constituent steps.
pub async fn perform_oauth_flow(
    http: &Arc<dyn HttpTransport>,
    clock: &Arc<dyn Clock>,
    oauth: &traits::McpOAuthConfigDto,
    server_url: &str,
    on_auth_url: &OnAuthorizationUrl,
) -> Result<Tokens, OAuthError> {
    // 1. Discovery.
    let meta = discover_auth_server_metadata(
        http,
        server_url,
        oauth.auth_server_metadata_url.as_deref(),
    )
    .await?;

    // 2. Bind the loopback listener FIRST so the redirect_uri is known before
    //    the authorize URL is built (claude-code's listen(0) pattern). A
    //    configured `callback_port` pins the port; otherwise the OS assigns one.
    let listener = CallbackListener::bind(oauth.callback_port.unwrap_or(0)).await?;
    let port = listener.port();
    let redirect_uri = format!("http://localhost:{port}/callback");

    // 3. Client id — configured, else dynamic client registration.
    let client_id = if let Some(id) = &oauth.client_id {
        id.clone()
    } else {
        let reg = meta.registration_endpoint.as_deref().ok_or_else(|| {
            OAuthError::Registration(
                "no client_id configured and server advertises no registration_endpoint".into(),
            )
        })?;
        register_client(http, reg, &redirect_uri).await?
    };

    // 4. Authorize URL (PKCE inside) + surface it to the host.
    let scope = meta
        .scopes_supported
        .as_ref()
        .map(|s| s.join(" "))
        .unwrap_or_default();
    let (auth_url, verifier, state) =
        build_authorize_url(&meta, &client_id, &redirect_uri, &scope);
    on_auth_url(&auth_url);

    // 5. Wait for the redirect, validate state, capture the code.
    let params = listener.accept(&state).await?;

    // 6. Exchange the code for tokens.
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
    let (kind, url, headers) = match spec {
        McpTransportSpec::Sse { url, headers, .. } => ("sse", url.clone(), headers.clone()),
        McpTransportSpec::Http { url, headers, .. } => {
            ("http", url.clone(), headers.clone())
        }
        // Non-remote specs never reach OAuth; fall back to the kind label.
        other => (other.kind(), String::new(), std::collections::HashMap::new()),
    };
    // claude-code serializes {type, url, headers} (object key order: type, url,
    // headers; headers is a sorted JSON object — serde_json sorts BTreeMap keys
    // but HashMap is arbitrary, so use a BTreeMap to match the stable shape).
    let headers_sorted: std::collections::BTreeMap<&String, &String> = headers.iter().collect();
    let config_json = serde_json::json!({
        "type": kind,
        "url": url,
        "headers": headers_sorted,
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
}

impl StoredTokens {
    /// Project to live [`Tokens`] (wraps the strings back into `Secret`).
    #[must_use]
    pub fn into_tokens(self) -> Tokens {
        Tokens {
            access_token: Secret::new(self.access_token),
            refresh_token: self.refresh_token.map(Secret::new),
            expires_at: SystemTime::UNIX_EPOCH + Duration::from_secs(self.expires_at_unix),
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
            well_known_url("https://mcp.example.com/tenant/a", "oauth-authorization-server"),
            "https://mcp.example.com/.well-known/oauth-authorization-server/tenant/a"
        );
    }

    #[test]
    fn server_key_matches_claude_code_shape() {
        // name|sha256({type,url,headers})[..16]; headers default {}.
        let spec = McpTransportSpec::Http {
            url: "https://mcp.example.com/v1".into(),
            headers: std::collections::HashMap::new(),
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
            headers: std::collections::HashMap::new(),
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
}

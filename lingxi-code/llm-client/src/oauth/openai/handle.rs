//! [`OpenAiOAuthHandle`] — browser-PKCE and device-code login for the
//! `OpenAI` / `ChatGPT` OAuth flow.
//!
//! Drives either:
//!   A. PKCE Authorization-Code flow (`login`):
//!      1. [`CallbackListener::bind`] on ports 1455/1457
//!      2. [`OpenAiOAuthClient::build_authorize_url_with_redirect`]
//!      3. Open browser (injectable — no-op in tests)
//!      4. [`CallbackListener::accept`] — validate state
//!      5. [`OpenAiOAuthClient::exchange_code_with_redirect`]
//!      6. [`parse_id_token`] for `account_id` / fedramp
//!      7. [`OpenAiOAuthClient::obtain_api_key`] — mint an `sk-...` key
//!      8. Persist via [`secret::CredentialManager::store_openai_oauth_tokens`]
//!      9. Persist the minted API key via
//!         [`secret::CredentialManager::set_provider_key`] (under id `"chatgpt"`)
//!      10. Return [`OpenAiLoginInfo`]
//!   B. Device-code flow (`login_device_code`):
//!      [`device_code::run_device_code_login`] → same persist steps.
//!
//! **Trait note:** we expose INHERENT async methods rather than implementing
//! `traits::AuthHandle`, because that trait's `LoginInfo` type carries
//! `email`+`org_id` (Anthropic-shaped) whereas `ChatGPT` carries `account_id`+
//! `fedramp`. The engine M8 wrapper (`ChatGptConnectDriver`) will call these
//! inherent methods directly.

use crate::oauth::openai::callback::{CallbackError, CallbackListener};
use crate::oauth::openai::client::OpenAiOAuthClient;
use crate::oauth::openai::device_code;
use crate::oauth::openai::token_data::parse_id_token;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use thiserror::Error;
use traits::Clock;

/// Wall-clock source for production use in the device-code path.
struct RealClock;
impl Clock for RealClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

/// Login-flow deadline for the browser-PKCE path.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(60);

/// Injectable browser opener. Production shells the platform "open" command;
/// tests pass a no-op (optionally recording the URL).
pub type BrowserOpener = Arc<dyn Fn(&str) -> Result<(), OpenAiAuthError> + Send + Sync>;

/// Error surface for the `OpenAI` handle.
#[derive(Debug, Error)]
pub enum OpenAiAuthError {
    /// Network or server-side failure.
    #[error("server error: {0}")]
    ServerError(String),
    /// User cancelled (state mismatch / Ctrl-C).
    #[error("login cancelled")]
    Cancelled,
    /// The 60-second browser-flow deadline elapsed.
    #[error("login timed out")]
    Timeout,
    /// Could not open the browser.
    #[error("browser open failed: {0}")]
    BrowserOpen(String),
}

/// What we know about the signed-in `ChatGPT` account after a successful login.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiLoginInfo {
    /// `chatgpt_account_id` from the `id_token` JWT (may be absent on older tokens).
    pub account_id: Option<String>,
    /// `FedRAMP` account flag from the `id_token` JWT.
    pub fedramp: bool,
    /// Minted `sk-...` API key from the RFC-8693 exchange.
    pub api_key: String,
}

/// Handle that drives either the browser-PKCE or device-code `OpenAI` OAuth flow
/// and persists the resulting tokens in the `secret` keychain.
pub struct OpenAiOAuthHandle {
    client: Arc<OpenAiOAuthClient>,
    credentials: Arc<secret::CredentialManager>,
    browser_open: BrowserOpener,
}

impl OpenAiOAuthHandle {
    /// Construct a handle that opens a real browser for the login redirect.
    #[must_use]
    pub fn new(
        client: Arc<OpenAiOAuthClient>,
        credentials: Arc<secret::CredentialManager>,
    ) -> Self {
        Self {
            client,
            credentials,
            browser_open: Arc::new(real_browser_open),
        }
    }

    /// Override the browser opener (tests inject a no-op so the flow runs
    /// headlessly).
    #[must_use]
    pub fn with_browser_opener(mut self, opener: BrowserOpener) -> Self {
        self.browser_open = opener;
        self
    }

    // ── Browser-PKCE flow ─────────────────────────────────────────────────

    /// Run the full browser-PKCE Authorization-Code flow.
    ///
    /// Wraps [`run_login`] in a 60-second deadline.
    pub async fn login(&self) -> Result<OpenAiLoginInfo, OpenAiAuthError> {
        match tokio::time::timeout(LOGIN_TIMEOUT, self.run_login()).await {
            Ok(result) => result,
            Err(_elapsed) => Err(OpenAiAuthError::Timeout),
        }
    }

    /// Steps 1–10 without the outer timeout.
    async fn run_login(&self) -> Result<OpenAiLoginInfo, OpenAiAuthError> {
        // (1) Bind the loopback listener so the redirect target exists before
        // the browser opens. The fixed-port bind (1455 / 1457) mirrors codex.
        let listener = CallbackListener::bind()
            .await
            .map_err(callback_to_auth_err)?;
        let bound_port = listener.port();
        let redirect_uri = self.client.config().redirect_uri(bound_port);

        // (2) Build the authorize URL.
        let (url, verifier, state) = self.client.build_authorize_url_with_redirect(&redirect_uri);

        // Start listening BEFORE opening the browser.
        let accept_state = state.clone();
        let accept = tokio::spawn(async move { listener.accept(&accept_state).await });

        // (3) Open the browser (no-op under test).
        (self.browser_open)(&url)?;

        // (4) Await the redirect.
        let params = accept
            .await
            .map_err(|e| OpenAiAuthError::ServerError(format!("callback task: {e}")))?
            .map_err(callback_to_auth_err)?;

        // (5) Exchange the code for tokens.
        let tokens = self
            .client
            .exchange_code_with_redirect(&params.code, &verifier, &redirect_uri)
            .await
            .map_err(|e| OpenAiAuthError::ServerError(e.to_string()))?;

        // (6) Parse id_token for account_id / fedramp.
        let claims = tokens
            .id_token
            .as_deref()
            .and_then(parse_id_token)
            .unwrap_or_default();

        // (7) Mint an API key via RFC-8693 token exchange (requires id_token).
        let api_key = if let Some(id_token) = tokens.id_token.as_deref() {
            self.client
                .obtain_api_key(id_token)
                .await
                .map_err(|e| OpenAiAuthError::ServerError(e.to_string()))?
        } else {
            // No id_token — cannot mint; surface a clear error.
            return Err(OpenAiAuthError::ServerError(
                "token exchange did not return an id_token; cannot mint API key".into(),
            ));
        };

        // (8) Persist OAuth tokens.
        let refresh = tokens
            .refresh_token
            .as_ref()
            .map(|s| s.expose_secret().clone());
        self.credentials
            .store_openai_oauth_tokens(
                tokens.access_token.expose_secret(),
                refresh.as_deref(),
                tokens.expires_at,
                vec![], // scopes not returned in exchange response body
                claims.account_id.as_deref(),
                claims.fedramp,
            )
            .await
            .map_err(|e| OpenAiAuthError::ServerError(format!("persist tokens: {e}")))?;

        // (9) Persist the minted API key under credential id "chatgpt".
        self.credentials
            .set_provider_key("chatgpt", &api_key)
            .await
            .map_err(|e| OpenAiAuthError::ServerError(format!("persist api_key: {e}")))?;

        // (10) Return resolved identity.
        Ok(OpenAiLoginInfo {
            account_id: claims.account_id,
            fedramp: claims.fedramp,
            api_key,
        })
    }

    // ── Device-code flow ──────────────────────────────────────────────────

    /// Run the device-code login flow.
    ///
    /// Calls `device_code::run_device_code_login`, parses the `id_token` claims,
    /// mints an API key, and persists everything — same storage as `login()`.
    pub async fn login_device_code(&self) -> Result<OpenAiLoginInfo, OpenAiAuthError> {
        let tokens = device_code::run_device_code_login(
            self.client.config().clone(),
            self.client.http(),
            // `run_device_code_login` constructs its own `OpenAiOAuthClient`
            // internally, so we pass a real clock for production use.  Test
            // coverage for the device-code flow itself lives in `device_code.rs`.
            Arc::new(RealClock),
        )
        .await
        .map_err(|e| OpenAiAuthError::ServerError(e.to_string()))?;

        let claims = tokens
            .id_token
            .as_deref()
            .and_then(parse_id_token)
            .unwrap_or_default();

        let api_key = if let Some(id_token) = tokens.id_token.as_deref() {
            // For the device-code path we need to call obtain_api_key through
            // a separate client configured the same way.
            self.client
                .obtain_api_key(id_token)
                .await
                .map_err(|e| OpenAiAuthError::ServerError(e.to_string()))?
        } else {
            return Err(OpenAiAuthError::ServerError(
                "device-code exchange did not return an id_token".into(),
            ));
        };

        let refresh = tokens
            .refresh_token
            .as_ref()
            .map(|s| s.expose_secret().clone());
        self.credentials
            .store_openai_oauth_tokens(
                tokens.access_token.expose_secret(),
                refresh.as_deref(),
                tokens.expires_at,
                vec![],
                claims.account_id.as_deref(),
                claims.fedramp,
            )
            .await
            .map_err(|e| OpenAiAuthError::ServerError(format!("persist tokens: {e}")))?;

        self.credentials
            .set_provider_key("chatgpt", &api_key)
            .await
            .map_err(|e| OpenAiAuthError::ServerError(format!("persist api_key: {e}")))?;

        Ok(OpenAiLoginInfo {
            account_id: claims.account_id,
            fedramp: claims.fedramp,
            api_key,
        })
    }

    // ── Session helpers ───────────────────────────────────────────────────

    /// Retrieve the currently-persisted session identity without any network
    /// call. Returns `None` when no valid session exists.
    pub async fn current_user(&self) -> Option<OpenAiLoginInfo> {
        // We read back what was persisted: account_id, fedramp from the meta,
        // and the api_key from the provider-key slot.
        let tokens = self
            .credentials
            .get_openai_oauth_tokens()
            .await
            .ok()
            .flatten()?;
        let api_key = self
            .credentials
            .get_provider_key("chatgpt")
            .await
            .ok()
            .flatten()
            .map(|s| s.expose_secret().clone())
            .unwrap_or_default();
        Some(OpenAiLoginInfo {
            account_id: tokens.account_id,
            fedramp: tokens.fedramp,
            api_key,
        })
    }

    /// Clear all persisted `OpenAI` OAuth credentials. Idempotent.
    pub async fn logout(&self) -> Result<(), OpenAiAuthError> {
        self.credentials
            .delete_openai_oauth_tokens()
            .await
            .map_err(|e| OpenAiAuthError::ServerError(format!("logout tokens: {e}")))?;
        // Best-effort: also clear the minted API key.
        // set_provider_key("chatgpt", "") would overwrite; delete is cleaner but
        // there's no `delete_provider_key` on CredentialManager — so we
        // overwrite with an empty string (the engine already guards empty keys).
        // A future task can add delete_provider_key for cleanliness.
        let _ = self.credentials.set_provider_key("chatgpt", "").await;
        Ok(())
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn callback_to_auth_err(e: CallbackError) -> OpenAiAuthError {
    match e {
        CallbackError::StateMismatch => OpenAiAuthError::Cancelled,
        CallbackError::Bind(m) => OpenAiAuthError::ServerError(format!("loopback bind: {m}")),
        CallbackError::InvalidRequest(m) => OpenAiAuthError::ServerError(format!("callback: {m}")),
    }
}

/// Open `url` in the user's default browser via the platform command.
fn real_browser_open(url: &str) -> Result<(), OpenAiAuthError> {
    #[cfg(target_os = "macos")]
    let (cmd, args): (&str, Vec<&str>) = ("open", vec![url]);
    #[cfg(target_os = "windows")]
    let (cmd, args): (&str, Vec<&str>) = ("cmd", vec!["/C", "start", "", url]);
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    let (cmd, args): (&str, Vec<&str>) = ("xdg-open", vec![url]);

    std::process::Command::new(cmd)
        .args(args)
        .spawn()
        .map(|_| ())
        .map_err(|e| OpenAiAuthError::BrowserOpen(e.to_string()))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oauth::openai::testsupport::{
        mem_credential_manager, port_guard, Canned, MemStorage, MockHttp, TestClock,
    };
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    use traits::HttpTransport;

    #[allow(dead_code)]
    fn _openai_oauth_handle_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<OpenAiOAuthHandle>();
    }

    /// Build a handle whose token endpoint returns `token_body`, plus an
    /// api-key exchange that returns `"sk-test-key"`.
    ///
    /// Returns `(handle, storage, browser_opened_flag)`.
    fn handle_with_token_body(
        token_body: &str,
    ) -> (OpenAiOAuthHandle, Arc<MemStorage>, Arc<AtomicBool>) {
        let http = MockHttp::new(vec![(
            "oauth/token",
            Canned {
                status: 200,
                body: token_body.into(),
            },
        )]);
        let clock = TestClock::new(1_000);
        let storage = MemStorage::new();
        let cm = mem_credential_manager(storage.clone(), clock.clone());
        let cfg = crate::oauth::openai::config::OpenAiOAuthConfig::default();
        let client =
            Arc::new(OpenAiOAuthClient::new(cfg, http as Arc<dyn HttpTransport>).with_clock(clock));

        let was_opened = Arc::new(AtomicBool::new(false));
        let browser_flag = was_opened.clone();

        // The test "browser opener" drives the loopback callback headlessly:
        // it parses the redirect_uri + state out of the authorize URL and
        // sends a synthetic GET /auth/callback?code=AUTHCODE&state=<state>
        // to the listener.
        let opener: BrowserOpener = Arc::new(move |url: &str| {
            browser_flag.store(true, Ordering::SeqCst);
            let url = url.to_string();
            tokio::spawn(async move {
                let (port, state) = parse_authorize_url(&url);
                for _ in 0..50 {
                    if let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)).await {
                        let req = format!(
                            "GET /auth/callback?code=AUTHCODE&state={state} HTTP/1.1\r\nHost: localhost\r\n\r\n"
                        );
                        let _ = s.write_all(req.as_bytes()).await;
                        let mut buf = Vec::new();
                        let _ = s.read_to_end(&mut buf).await;
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            });
            Ok(())
        });

        (
            OpenAiOAuthHandle::new(client, cm).with_browser_opener(opener),
            storage,
            was_opened,
        )
    }

    /// Parse `redirect_uri` port and `state` out of an authorize URL.
    fn parse_authorize_url(url: &str) -> (u16, String) {
        let query = url.split_once('?').map_or("", |(_, q)| q);
        let mut redirect = String::new();
        let mut state = String::new();
        for kv in query.split('&') {
            if let Some(v) = kv.strip_prefix("redirect_uri=") {
                redirect = urlencoding::decode(v)
                    .map(std::borrow::Cow::into_owned)
                    .unwrap_or_default();
            } else if let Some(v) = kv.strip_prefix("state=") {
                state = urlencoding::decode(v)
                    .map(std::borrow::Cow::into_owned)
                    .unwrap_or_default();
            }
        }
        // redirect_uri: "http://localhost:{port}/auth/callback"
        let port = redirect
            .rsplit_once(':')
            .and_then(|(_, rest)| rest.split('/').next())
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(0);
        (port, state)
    }

    // ── PKCE browser-flow tests ───────────────────────────────────────────

    /// A complete token-response body that includes an id_token with
    /// account_id, plus a second "call" for the api-key exchange.
    /// MockHttp matches by URL substring; "oauth/token" hits for both the
    /// code-exchange and the api-key RFC-8693 exchange, so we return a body
    /// that works for both (the api-key exchange ignores refresh_token/id_token
    /// and just reads `access_token`).
    fn full_token_body() -> String {
        // id_token payload: {"https://api.openai.com/auth":{"chatgpt_account_id":"acct_test","chatgpt_account_is_fedramp":false},"email":"u@example.com"}
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
        let hdr = URL_SAFE_NO_PAD.encode(br#"{"alg":"none"}"#);
        let payload = URL_SAFE_NO_PAD.encode(
            br#"{"https://api.openai.com/auth":{"chatgpt_account_id":"acct_test","chatgpt_account_is_fedramp":false},"email":"u@example.com"}"#,
        );
        let id_token = format!("{hdr}.{payload}.sig");
        format!(
            r#"{{"access_token":"bearer-tok","refresh_token":"ref-tok","expires_in":3600,"id_token":"{id_token}"}}"#
        )
    }

    /// Body for the api-key mint call (RFC-8693).
    /// We need the mock to return different bodies for code-exchange vs
    /// api-key-mint. Since MockHttp returns the SAME route for both (both hit
    /// "oauth/token"), we build a body that serves both:
    /// - `exchange_code_with_redirect` reads: access_token, refresh_token, id_token, expires_in
    /// - `obtain_api_key` reads: access_token (the minted key)
    ///
    /// We can't distinguish the two calls by URL alone, so we accept that
    /// `exchange_code_with_redirect` will parse "sk-test-key" as the access
    /// token (fine — handle only uses the result from the first call), and
    /// `obtain_api_key` will also get "sk-test-key" as access_token (the
    /// minted key). However, the id_token and refresh_token in the first call
    /// are what matter; the second call just needs `access_token`.
    ///
    /// SOLUTION: use two separate routes — the code exchange hits
    /// "oauth/token" first; the api-key mint hits "oauth/token" as well, so
    /// MockHttp returns the same body both times. We arrange the handle's
    /// flow so the first call (exchange) decodes the full body correctly, and
    /// the second call (obtain_api_key) also sees a valid `access_token` field.
    /// This works because `ExchangeResponse` uses `#[serde(default)]` for
    /// optional fields and `obtain_api_key` only reads `access_token`.
    ///
    /// So we only need ONE canned response that satisfies both parsers:
    fn unified_token_body() -> String {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
        let hdr = URL_SAFE_NO_PAD.encode(br#"{"alg":"none"}"#);
        let payload = URL_SAFE_NO_PAD.encode(
            br#"{"https://api.openai.com/auth":{"chatgpt_account_id":"acct_test","chatgpt_account_is_fedramp":false}}"#,
        );
        let id_token = format!("{hdr}.{payload}.sig");
        // access_token serves as the "minted api key" for obtain_api_key,
        // and as the bearer token for exchange_code_with_redirect.
        format!(
            r#"{{"access_token":"sk-test-key","refresh_token":"ref-tok","expires_in":3600,"id_token":"{id_token}"}}"#
        )
    }

    fn handle_for_login() -> (OpenAiOAuthHandle, Arc<MemStorage>, Arc<AtomicBool>) {
        handle_with_token_body(&unified_token_body())
    }

    #[tokio::test]
    async fn login_persists_tokens_and_api_key() {
        let _g = port_guard().await;
        let (handle, storage, opened) = handle_for_login();
        let info = handle.login().await.expect("login ok");
        assert!(opened.load(Ordering::SeqCst), "browser opener fired");
        assert_eq!(info.account_id.as_deref(), Some("acct_test"));
        assert!(!info.fedramp);
        assert_eq!(info.api_key, "sk-test-key");

        // 3 OpenAI OAuth entries + 1 provider-key entry = 4 total under "lingxi".
        // (openai-oauth-access, openai-oauth-refresh, openai-oauth-meta, provider-key-chatgpt)
        assert_eq!(storage.count("lingxi"), 4);
    }

    #[tokio::test]
    async fn current_user_reads_back_without_network() {
        let _g = port_guard().await;
        let (handle, _storage, _) = handle_for_login();
        handle.login().await.expect("login ok");

        let cu = handle.current_user().await.expect("current_user present");
        assert_eq!(cu.account_id.as_deref(), Some("acct_test"));
        assert_eq!(cu.api_key, "sk-test-key");
    }

    #[tokio::test]
    async fn logout_clears_and_current_user_is_none() {
        let _g = port_guard().await;
        let (handle, storage, _) = handle_for_login();
        handle.login().await.expect("login ok");
        assert_eq!(storage.count("lingxi"), 4);

        handle.logout().await.expect("logout ok");
        // OpenAI OAuth slots cleared; provider-key-chatgpt overwritten with "".
        // Storage still has the overwritten provider-key entry, but it's empty.
        assert!(handle.current_user().await.is_none());

        // Idempotent second logout.
        handle.logout().await.expect("second logout ok");
    }

    #[tokio::test]
    async fn current_user_none_when_not_logged_in() {
        let (handle, _storage, _) = handle_for_login();
        assert!(handle.current_user().await.is_none());
    }
}

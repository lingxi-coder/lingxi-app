//! [`AuthHandle`] implementation backed by the [`ClaudeAiOAuthClient`].
//!
//! Drives the full interactive PKCE Authorization-Code flow:
//!   1. [`ClaudeAiOAuthClient::build_authorize_url_with_redirect`]
//!   2. open the browser (via an injectable opener — no-op in tests)
//!   3. [`crate::oauth::anthropic::callback::CallbackListener::accept`] on the loopback listener
//!   4. [`ClaudeAiOAuthClient::exchange_code_with_redirect`]
//!   5. resolve `email` + `org_id` from the exchange response (`account` /
//!      `organization`) or, failing that, the profile endpoint — there is no
//!      JWT id-token to decode for this provider
//!   6. persist via [`secret::CredentialManager::store_oauth_tokens`]
//!   7. return [`LoginInfo`]
//!
//! The whole flow runs under a 60-second deadline (the trait contract);
//! exceeding it yields [`AuthError::Timeout`].

use crate::oauth::anthropic::callback::{CallbackError, CallbackListener};
use crate::oauth::anthropic::client::ClaudeAiOAuthClient;
use async_trait::async_trait;
use protocol::{HttpMethod, HttpRequest};
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;
use traits::{AuthError, AuthHandle, LoginInfo};

/// Login-flow deadline (trait contract: §login docs).
const LOGIN_TIMEOUT: Duration = Duration::from_secs(60);

/// Injectable browser opener. Production shells the platform "open" command;
/// tests pass a no-op (optionally recording the URL).
pub type BrowserOpener = Arc<dyn Fn(&str) -> Result<(), AuthError> + Send + Sync>;

/// Profile-endpoint shapes. `account.email` (note: the token-exchange response
/// uses `email_address` instead) and `organization.uuid`.
#[derive(Debug, Deserialize)]
struct ProfileResponse {
    account: Option<ProfileAccount>,
    organization: Option<ProfileOrganization>,
}
#[derive(Debug, Deserialize)]
struct ProfileAccount {
    #[serde(default)]
    email: String,
}
#[derive(Debug, Deserialize)]
struct ProfileOrganization {
    #[serde(default)]
    uuid: String,
}

/// `AuthHandle` impl wrapping the [`ClaudeAiOAuthClient`].
pub struct OAuthHandle {
    client: Arc<ClaudeAiOAuthClient>,
    browser_open: BrowserOpener,
}

impl OAuthHandle {
    /// Construct a handle that opens a real browser for the login redirect.
    #[must_use]
    pub fn new(client: Arc<ClaudeAiOAuthClient>) -> Self {
        Self {
            client,
            browser_open: Arc::new(real_browser_open),
        }
    }

    /// Override the browser opener (tests inject a no-op so the flow runs
    /// headless).
    #[must_use]
    pub fn with_browser_opener(mut self, opener: BrowserOpener) -> Self {
        self.browser_open = opener;
        self
    }

    /// Run steps 1-7 (without the outer timeout, which `login` applies).
    async fn run_login(&self) -> Result<LoginInfo, AuthError> {
        // (1+) Bind the loopback listener first so the redirect target exists
        // before the browser opens. Port 0 → OS-assigned; read it back and bake
        // the real port into the redirect_uri so authorize + exchange agree.
        let listener = CallbackListener::bind(callback_port(&self.client.config().redirect_uri))
            .await
            .map_err(callback_to_auth_err)?;
        let redirect_uri = format!("http://127.0.0.1:{}/callback", listener.port());

        let (url, verifier, state) = self
            .client
            .build_authorize_url_with_redirect(&redirect_uri);

        // Start waiting for the callback BEFORE opening the browser.
        let accept_state = state.clone();
        let accept = tokio::spawn(async move { listener.accept(&accept_state).await });

        // (2) Open the browser (no-op under test).
        (self.browser_open)(&url)?;

        // (3) Await the redirect.
        let params = accept
            .await
            .map_err(|e| AuthError::ServerError(format!("callback task: {e}")))?
            .map_err(callback_to_auth_err)?;

        // (4) Exchange the code for tokens.
        let tokens = self
            .client
            .exchange_code_with_redirect(&params.code, &verifier, &params.state, &redirect_uri)
            .await
            .map_err(|e| AuthError::ServerError(e.to_string()))?;

        // (5) Resolve email + org. Prefer the exchange response; otherwise fetch
        //     the profile endpoint with the bearer token.
        let (email, org_id) = match (&tokens.account, &tokens.organization) {
            (Some(acc), Some(org)) if !acc.email_address.is_empty() => {
                (acc.email_address.clone(), org.uuid.clone())
            }
            _ => {
                self.fetch_profile(tokens.access_token.expose_secret())
                    .await?
            }
        };

        // (6) Persist tokens + identity.
        let refresh = tokens
            .refresh_token
            .as_ref()
            .map(|s| s.expose_secret().clone());
        self.client
            .credentials()
            .store_oauth_tokens(
                tokens.access_token.expose_secret(),
                refresh.as_deref(),
                tokens.expires_at,
                tokens.scopes.clone(),
                &email,
                &org_id,
            )
            .await
            .map_err(|e| AuthError::ServerError(format!("persist: {e}")))?;

        // (6b) Resolve + publish the subscription tier (claude-code
        // `getOauthAccountInfo`, written at login from the profile + roles
        // endpoints) into the process-global `traits::subscription` cache, so
        // subscription-gated prompt logic (e.g. the `AgentTool` pro-plan gate)
        // reflects the signed-in plan. Best-effort + scope-gated
        // (`hasProfileScope`): a token without `user:profile`, or any fetch
        // failure, leaves the cache unchanged.
        let transport = self.client.http();
        crate::oauth::anthropic::subscription::publish_subscription(
            tokens.access_token.expose_secret(),
            &tokens.scopes,
            &transport,
        )
        .await;

        // (7) Return the resolved identity.
        Ok(LoginInfo { email, org_id })
    }

    /// GET the profile endpoint with `Authorization: Bearer <access>`.
    async fn fetch_profile(&self, access_token: &str) -> Result<(String, String), AuthError> {
        let req = HttpRequest {
            method: HttpMethod::Get,
            url: self.client.config().profile_endpoint.clone(),
            headers: vec![
                ("authorization".into(), format!("Bearer {access_token}")),
                ("accept".into(), "application/json".into()),
            ],
            body: None,
            body_bytes: None,
            timeout: Some(Duration::from_secs(15)),
        };
        let resp = self
            .client
            .http()
            .request(req)
            .await
            .map_err(|e| AuthError::Network(e.to_string()))?;
        if resp.status == 401 || resp.status == 403 {
            return Err(AuthError::ServerError(format!(
                "profile fetch rejected: status {}",
                resp.status
            )));
        }
        if resp.status != 200 {
            return Err(AuthError::ServerError(format!(
                "profile fetch failed: status {}",
                resp.status
            )));
        }
        let profile: ProfileResponse = serde_json::from_str(&resp.body)
            .map_err(|e| AuthError::ServerError(format!("profile decode: {e}")))?;
        let email = profile.account.map(|a| a.email).unwrap_or_default();
        let org_id = profile.organization.map(|o| o.uuid).unwrap_or_default();
        Ok((email, org_id))
    }
}

#[async_trait]
impl AuthHandle for OAuthHandle {
    async fn login(&self) -> Result<LoginInfo, AuthError> {
        match tokio::time::timeout(LOGIN_TIMEOUT, self.run_login()).await {
            Ok(result) => result,
            Err(_elapsed) => Err(AuthError::Timeout),
        }
    }

    async fn logout(&self) -> Result<(), AuthError> {
        // Idempotent — deleting absent entries is not an error.
        self.client
            .credentials()
            .delete_oauth_tokens()
            .await
            .map_err(|e| AuthError::ServerError(format!("logout: {e}")))
    }

    async fn current_user(&self) -> Option<LoginInfo> {
        match self.client.credentials().get_oauth_tokens().await {
            Ok(Some(t)) => Some(LoginInfo {
                email: t.email,
                org_id: t.org_id,
            }),
            _ => None,
        }
    }
}

/// Parse the loopback port out of a `http://127.0.0.1:{port}/callback`-style
/// redirect URI. Falls back to `0` (OS-assigned) when the URI is `:0` or
/// unparseable.
fn callback_port(redirect_uri: &str) -> u16 {
    redirect_uri
        .rsplit_once(':')
        .and_then(|(_, rest)| rest.split('/').next())
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(0)
}

/// Map a callback-layer failure onto the auth surface. A state mismatch /
/// cancellation reads as a user cancellation; bind/parse failures are server
/// errors.
fn callback_to_auth_err(e: CallbackError) -> AuthError {
    match e {
        CallbackError::StateMismatch => AuthError::Cancelled,
        CallbackError::Bind(m) => AuthError::ServerError(format!("loopback bind: {m}")),
        CallbackError::InvalidRequest(m) => AuthError::ServerError(format!("callback: {m}")),
    }
}

/// Open `url` in the user's default browser via the platform command. Best
/// effort — a spawn failure surfaces as a server error so the CLI can fall back
/// to printing the URL.
fn real_browser_open(url: &str) -> Result<(), AuthError> {
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
        .map_err(|e| AuthError::ServerError(format!("could not open browser: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oauth::anthropic::config::ClaudeAiOAuthConfig;
    use crate::oauth::anthropic::testsupport::{mem_credential_manager, Canned, MemStorage, MockHttp, TestClock};
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    use traits::HttpTransport;

    #[allow(dead_code)]
    fn _oauth_handle_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<OAuthHandle>();
    }

    #[test]
    fn auth_handle_trait_object_compiles() {
        fn _accept(_h: &Arc<dyn AuthHandle>) {}
    }

    /// Build a handle whose token endpoint returns `token_body`, over an
    /// in-memory keychain. Returns the handle, the storage (for assertions),
    /// and a flag set when the browser opener fired.
    fn handle_with_token_body(
        token_body: &str,
    ) -> (OAuthHandle, Arc<MemStorage>, Arc<AtomicBool>) {
        let http = MockHttp::new(vec![
            (
                "oauth/token",
                Canned {
                    status: 200,
                    body: token_body.into(),
                },
            ),
            // Profile endpoint fallback (used only when the token body omits
            // account/org).
            (
                "/me",
                Canned {
                    status: 200,
                    body: r#"{"account":{"email":"profile@example.com"},"organization":{"uuid":"org-from-profile"}}"#
                        .into(),
                },
            ),
        ]);
        let clock = TestClock::new(1_000);
        let storage = MemStorage::new();
        let cm = mem_credential_manager(storage.clone(), clock.clone());
        let cfg = ClaudeAiOAuthConfig::default_with_port(0);
        let client = Arc::new(
            ClaudeAiOAuthClient::new(cfg, http as Arc<dyn HttpTransport>, cm).with_clock(clock),
        );

        let was_opened = Arc::new(AtomicBool::new(false));
        let browser_flag = was_opened.clone();
        // The "browser opener" instead drives the loopback callback: it parses
        // the redirect_uri + state out of the authorize URL and POSTs the GET
        // to the listener, completing the flow headlessly.
        let opener: BrowserOpener = Arc::new(move |url: &str| {
            browser_flag.store(true, Ordering::SeqCst);
            let url = url.to_string();
            // Spawn so the opener returns immediately while the listener accepts.
            tokio::spawn(async move {
                let (port, state) = parse_authorize_url(&url);
                // Small spin until the listener is ready.
                for _ in 0..50 {
                    if let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)).await {
                        let req = format!(
                            "GET /callback?code=AUTHCODE&state={state} HTTP/1.1\r\nHost: localhost\r\n\r\n"
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
            OAuthHandle::new(client).with_browser_opener(opener),
            storage,
            was_opened,
        )
    }

    fn parse_authorize_url(url: &str) -> (u16, String) {
        // redirect_uri is percent-encoded; find it and pull the port. state is
        // a plain query param.
        let query = url.split_once('?').map_or("", |(_, q)| q);
        let mut redirect = String::new();
        let mut state = String::new();
        for kv in query.split('&') {
            if let Some(v) = kv.strip_prefix("redirect_uri=") {
                redirect = urlencoding::decode(v).map(std::borrow::Cow::into_owned).unwrap_or_default();
            } else if let Some(v) = kv.strip_prefix("state=") {
                state = urlencoding::decode(v).map(std::borrow::Cow::into_owned).unwrap_or_default();
            }
        }
        let port = redirect
            .rsplit_once(':')
            .and_then(|(_, rest)| rest.split('/').next())
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(0);
        (port, state)
    }

    #[tokio::test]
    async fn login_uses_account_org_from_token_response() {
        let body = r#"{
            "access_token":"acc","refresh_token":"ref","expires_in":3600,
            "scope":"read:user",
            "account":{"uuid":"acc-uuid","email_address":"token@example.com"},
            "organization":{"uuid":"org-token"}
        }"#;
        let (handle, storage, opened) = handle_with_token_body(body);
        let info = handle.login().await.expect("login ok");
        assert_eq!(info.email, "token@example.com");
        assert_eq!(info.org_id, "org-token");
        assert!(opened.load(Ordering::SeqCst), "browser opener fired");
        // 3 OAuth entries persisted (access + refresh + meta).
        assert_eq!(storage.count("lingxi"), 3);

        // current_user reads them back without a network call.
        let cu = handle.current_user().await.expect("current user");
        assert_eq!(cu.email, "token@example.com");
        assert_eq!(cu.org_id, "org-token");
    }

    #[tokio::test]
    async fn login_falls_back_to_profile_endpoint() {
        // Token body omits account/organization → profile GET is used.
        let body = r#"{"access_token":"acc","refresh_token":"ref","expires_in":3600}"#;
        let (handle, _storage, _opened) = handle_with_token_body(body);
        let info = handle.login().await.expect("login ok");
        assert_eq!(info.email, "profile@example.com");
        assert_eq!(info.org_id, "org-from-profile");
    }

    #[tokio::test]
    async fn logout_clears_and_current_user_is_none() {
        let body = r#"{"access_token":"acc","refresh_token":"ref","expires_in":3600,
            "account":{"uuid":"u","email_address":"e@x"},"organization":{"uuid":"o"}}"#;
        let (handle, storage, _) = handle_with_token_body(body);
        handle.login().await.expect("login ok");
        assert_eq!(storage.count("lingxi"), 3);

        handle.logout().await.expect("logout ok");
        assert_eq!(storage.count("lingxi"), 0);
        assert!(handle.current_user().await.is_none());

        // Idempotent.
        handle.logout().await.expect("second logout ok");
    }

    #[tokio::test]
    async fn current_user_none_when_not_logged_in() {
        let body = r#"{"access_token":"acc","expires_in":1}"#;
        let (handle, _storage, _) = handle_with_token_body(body);
        assert!(handle.current_user().await.is_none());
    }
}

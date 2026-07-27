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
//! The whole flow runs under a five-minute deadline (the trait contract);
//! exceeding it yields [`AuthError::Timeout`].

use crate::oauth::anthropic::callback::{CallbackError, CallbackListener};
use crate::oauth::anthropic::client::{AuthorizeOptions, ClaudeAiOAuthClient, ExchangedTokens};
use crate::oauth::anthropic::config::{
    CLAUDE_CODE_INFERENCE_SCOPE, LONG_LIVED_OAUTH_TOKEN_TTL_SECONDS,
};
use async_trait::async_trait;
use protocol::{HttpMethod, HttpRequest};
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;
use traits::{AuthError, AuthHandle, LoginInfo};

/// Login-flow deadline (trait contract: §login docs).
const LOGIN_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// Injectable browser opener. Production shells the platform "open" command;
/// tests pass a no-op (optionally recording the URL).
pub type BrowserOpener = Arc<dyn Fn(&str) -> Result<(), AuthError> + Send + Sync>;

/// Host callback invoked with the MANUAL authorize URL before the browser
/// opens (oracle `startOAuthFlow(async (url) => ...)`): the CLI prints the
/// "If the browser didn't open, visit:" fallback trio from it.
pub type UrlSink = Arc<dyn Fn(&str) + Send + Sync>;

/// Host IO seams for the interactive code flow (oracle `startOAuthFlow`
/// callback + `handleManualAuthCodeInput`). Default = fully headless: no URL
/// surfaced, no manual entry (the pre-existing programmatic behaviour).
#[derive(Default)]
pub struct CodeFlowIo {
    /// Receives the MANUAL authorize URL before the browser opens.
    pub on_url: Option<UrlSink>,
    /// Manual `(code, state)` pairs pasted by the user (`code#state`). Raced
    /// against the loopback listener; a closed channel (stdin EOF) simply
    /// leaves the listener waiting.
    pub manual_rx: Option<tokio::sync::mpsc::Receiver<(String, String)>>,
}

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

/// Options shared by the top-level `auth login` flow and the interactive
/// `/login` default.
#[derive(Debug, Clone, Default)]
pub struct OAuthLoginOptions {
    /// Pre-populate the account email in the provider UI.
    pub login_hint: Option<String>,
    /// Force SSO in the provider UI.
    pub sso: bool,
    /// Managed organization UUID forwarded to the provider.
    pub org_uuid: Option<String>,
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

    async fn run_code_flow(
        &self,
        authorize: AuthorizeOptions,
        expires_in: Option<u64>,
        io: CodeFlowIo,
    ) -> Result<ExchangedTokens, AuthError> {
        // (1+) Bind the loopback listener first so the redirect target exists
        // before the browser opens. Port 0 → OS-assigned; read it back and bake
        // the real port into the redirect_uri so authorize + exchange agree.
        let listener = CallbackListener::bind(callback_port(&self.client.config().redirect_uri))
            .await
            .map_err(callback_to_auth_err)?;
        let redirect_uri = format!("http://localhost:{}/callback", listener.port());

        let urls = self
            .client
            .build_authorize_url_pair_with_options(&redirect_uri, &authorize);

        // (1b) Surface the MANUAL URL variant to the host FIRST (oracle
        // `await e(i)` before `await Cc(s)`), so the fallback instructions are
        // visible even when the browser spawn below fails.
        if let Some(sink) = &io.on_url {
            sink(&urls.manual_url);
        }

        // (2) Open the browser on the AUTOMATIC variant (no-op under test).
        // Best-effort: the oracle's opener (`Cc`) reports failure as a value
        // instead of throwing, so a spawn error must NOT abort the flow — the
        // surfaced URL + manual `code#state` paste is the recovery path.
        if let Err(error) = (self.browser_open)(&urls.automatic_url) {
            tracing::warn!(
                %error,
                "could not open browser for OAuth login; complete the flow via the printed URL"
            );
        }

        // (3) Await the redirect, racing any manual `code#state` paste. The
        // socket was already bound before the browser was opened, so the kernel
        // backlog safely holds an immediate callback. Keeping the accept future
        // in this task also means a caller timeout cancels and closes the
        // listener instead of leaking a detached callback task. A manual win
        // drops (closes) the listener — oracle `handleManualAuthCodeInput`
        // closes the `authCodeListener`.
        let accept = listener.accept(&urls.state);
        tokio::pin!(accept);
        let mut manual_rx = io.manual_rx;
        let (code, via_manual) = loop {
            if let Some(rx) = manual_rx.as_mut() {
                tokio::select! {
                    res = accept.as_mut() => {
                        break (res.map_err(callback_to_auth_err)?.code, false);
                    }
                    pasted = rx.recv() => match pasted {
                        // Oracle `handleManualAuthCodeInput` consumes only the
                        // CODE; the exchange runs with the flow's own state and
                        // the manual redirect_uri (`useManualRedirect`), so the
                        // pasted state is syntax-validated by the CLI but not
                        // re-checked here (the token endpoint enforces it).
                        Some((code, _state)) => break (code, true),
                        // Manual channel closed (stdin EOF) — keep waiting on
                        // the loopback listener alone.
                        None => manual_rx = None,
                    },
                }
            } else {
                break (
                    accept
                        .as_mut()
                        .await
                        .map_err(callback_to_auth_err)?
                        .code,
                    false,
                );
            }
        };

        // (4) Exchange the code for tokens. A manually-pasted code was minted
        // against the hosted code page, so the exchange must present THAT
        // redirect_uri (oracle `useManualRedirect: !automatic`).
        let exchange_redirect = if via_manual {
            self.client.config().manual_redirect_uri.clone()
        } else {
            redirect_uri
        };
        self.client
            .exchange_code_with_options(
                &code,
                &urls.verifier,
                &urls.state,
                &exchange_redirect,
                expires_in,
            )
            .await
            .map_err(|e| AuthError::ServerError(e.to_string()))
    }

    /// Run the normal login flow and persist the resulting credential.
    pub async fn login_with_options(
        &self,
        options: OAuthLoginOptions,
    ) -> Result<LoginInfo, AuthError> {
        self.login_with_options_and_io(options, CodeFlowIo::default())
            .await
    }

    /// [`Self::login_with_options`] with host IO seams: `io.on_url` receives
    /// the MANUAL authorize URL before the browser opens, and `io.manual_rx`
    /// feeds pasted `code#state` pairs that race the loopback callback (the
    /// `claude auth login` browser-failure fallback).
    pub async fn login_with_options_and_io(
        &self,
        options: OAuthLoginOptions,
        io: CodeFlowIo,
    ) -> Result<LoginInfo, AuthError> {
        let authorize = AuthorizeOptions {
            org_uuid: options.org_uuid,
            login_hint: options.login_hint,
            login_method: options.sso.then(|| "sso".to_string()),
            scopes: None,
        };
        let tokens = self.run_code_flow(authorize, None, io).await?;

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

    /// Mint a one-year, inference-only token without persisting it locally.
    /// The caller must print/store it exactly once.
    pub async fn mint_long_lived_token(
        &self,
        org_uuid: Option<String>,
    ) -> Result<protocol::Secret<String>, AuthError> {
        let tokens = self
            .run_code_flow(
                AuthorizeOptions {
                    scopes: Some(vec![CLAUDE_CODE_INFERENCE_SCOPE.to_string()]),
                    org_uuid,
                    login_hint: None,
                    login_method: None,
                },
                Some(LONG_LIVED_OAUTH_TOKEN_TTL_SECONDS),
                CodeFlowIo::default(),
            )
            .await?;
        Ok(tokens.access_token)
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
        match tokio::time::timeout(
            LOGIN_TIMEOUT,
            self.login_with_options(OAuthLoginOptions::default()),
        )
        .await
        {
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
/// effort — a spawn failure surfaces as a server error, which `run_code_flow`
/// logs and IGNORES (oracle `Cc` reports failure as a value): the surfaced
/// manual URL + `code#state` paste is the recovery path.
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
    use crate::oauth::anthropic::testsupport::{
        mem_credential_manager, Canned, MemStorage, MockHttp, TestClock,
    };
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
    fn handle_with_token_body(token_body: &str) -> (OAuthHandle, Arc<MemStorage>, Arc<AtomicBool>) {
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
                "/api/oauth/profile",
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
                redirect = urlencoding::decode(v)
                    .map(std::borrow::Cow::into_owned)
                    .unwrap_or_default();
            } else if let Some(v) = kv.strip_prefix("state=") {
                state = urlencoding::decode(v)
                    .map(std::borrow::Cow::into_owned)
                    .unwrap_or_default();
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
        // Binds the fixed loopback port 45321 — same machine-global resource
        // contention as the OpenAI ports, and these three raced each other.
        let _g = crate::oauth::openai::testsupport::port_guard().await;
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
        // Binds the fixed loopback port 45321 — same machine-global resource
        // contention as the OpenAI ports, and these three raced each other.
        let _g = crate::oauth::openai::testsupport::port_guard().await;
        // Token body omits account/organization → profile GET is used.
        let body = r#"{"access_token":"acc","refresh_token":"ref","expires_in":3600}"#;
        let (handle, _storage, _opened) = handle_with_token_body(body);
        let info = handle.login().await.expect("login ok");
        assert_eq!(info.email, "profile@example.com");
        assert_eq!(info.org_id, "org-from-profile");
    }

    #[tokio::test]
    async fn logout_clears_and_current_user_is_none() {
        // Binds the fixed loopback port 45321 — same machine-global resource
        // contention as the OpenAI ports, and these three raced each other.
        let _g = crate::oauth::openai::testsupport::port_guard().await;
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

    /// M8: a failing browser opener no longer aborts the flow, the host URL
    /// sink receives the MANUAL authorize URL, and a pasted `code#state` pair
    /// completes the login — with the token exchange presenting the MANUAL
    /// redirect_uri (oracle `useManualRedirect`).
    #[tokio::test]
    async fn manual_code_entry_completes_login_when_browser_fails() {
        // Binds the fixed loopback port 45321 — same machine-global resource
        // contention as the OpenAI ports, and these tests raced each other.
        let _g = crate::oauth::openai::testsupport::port_guard().await;
        let body = r#"{
            "access_token":"acc","refresh_token":"ref","expires_in":3600,
            "scope":"read:user",
            "account":{"uuid":"acc-uuid","email_address":"token@example.com"},
            "organization":{"uuid":"org-token"}
        }"#;
        let (handle, _storage, opened) = handle_with_token_body(body);
        // Failing opener: never drives the loopback callback.
        let handle = handle.with_browser_opener(Arc::new(|_url: &str| {
            Err(AuthError::ServerError(
                "could not open browser: boom".into(),
            ))
        }));

        let seen_url = Arc::new(std::sync::Mutex::new(String::new()));
        let sink_url = seen_url.clone();
        let on_url: UrlSink = Arc::new(move |url: &str| {
            *sink_url.lock().unwrap() = url.to_string();
        });

        let (tx, rx) = tokio::sync::mpsc::channel::<(String, String)>(1);
        tx.send(("PASTEDCODE".into(), "pasted-state".into()))
            .await
            .expect("queue manual code");

        let info = handle
            .login_with_options_and_io(
                OAuthLoginOptions::default(),
                CodeFlowIo {
                    on_url: Some(on_url),
                    manual_rx: Some(rx),
                },
            )
            .await
            .expect("login completes via manual entry despite browser failure");
        assert_eq!(info.email, "token@example.com");
        assert!(!opened.load(Ordering::SeqCst), "flag opener replaced");

        // The sink got the MANUAL URL variant (hosted code page redirect).
        let url = seen_url.lock().unwrap().clone();
        assert!(
            url.contains("redirect_uri=https%3A%2F%2Fplatform.claude.com%2Foauth%2Fcode%2Fcallback"),
            "sink got the manual variant, was: {url}"
        );
    }

    /// M8: with NO manual channel and a dead opener the flow still waits on
    /// the listener (regression: it used to abort with the opener error). The
    /// caller timeout is what ends it.
    #[tokio::test]
    async fn browser_failure_alone_keeps_waiting_not_error() {
        let _g = crate::oauth::openai::testsupport::port_guard().await;
        let body = r#"{"access_token":"acc","expires_in":3600}"#;
        let (handle, _storage, _) = handle_with_token_body(body);
        let handle = handle.with_browser_opener(Arc::new(|_url: &str| {
            Err(AuthError::ServerError("could not open browser".into()))
        }));
        let out = tokio::time::timeout(
            Duration::from_millis(200),
            handle.login_with_options(OAuthLoginOptions::default()),
        )
        .await;
        assert!(out.is_err(), "flow keeps waiting instead of aborting");
    }
}

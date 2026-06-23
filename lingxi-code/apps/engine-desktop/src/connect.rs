//! Engine implementations of the `/connect` seams (`command_core::connect`).
//!
//! [`EngineCredentialWriter`] bridges the command-layer `ConnectCredentialWriter`
//! onto a host secure-input port + the keychain (`CredentialManager::set_provider_key`).
//! The secure prompt is rendered by the tui; tests inject a mock prompt.

use async_trait::async_trait;
use command_core::{ConnectCredentialWriter, ConnectError};
use secret::CredentialManager;
use std::sync::Arc;

/// Host port for a masked secret prompt. The tui implements this against its
/// secure-input widget; headless/test callers inject a canned value.
#[async_trait]
pub trait SecureKeyPrompt: Send + Sync {
    /// Prompt the user (masked) for the API key of `provider_label`.
    /// Returns `None` if the user cancelled.
    async fn prompt(&self, provider_label: &str) -> Option<String>;
}

/// A headless no-op prompt (returns `None` — cancels). The default when no tui
/// prompt is wired (`DesktopConfig.connect_prompt == None`).
pub struct NoopKeyPrompt;
#[async_trait]
impl SecureKeyPrompt for NoopKeyPrompt {
    async fn prompt(&self, _provider_label: &str) -> Option<String> {
        None
    }
}

/// Engine writer: prompt via the host port, then persist under `credential_id`.
pub struct EngineCredentialWriter {
    credentials: Arc<CredentialManager>,
    prompt: Arc<dyn SecureKeyPrompt>,
}

impl EngineCredentialWriter {
    /// Construct over the shared credential manager + host prompt port.
    #[must_use]
    pub fn new(credentials: Arc<CredentialManager>, prompt: Arc<dyn SecureKeyPrompt>) -> Self {
        Self { credentials, prompt }
    }
}

#[async_trait]
impl ConnectCredentialWriter for EngineCredentialWriter {
    async fn prompt_and_store_key(&self, credential_id: &str) -> Result<(), ConnectError> {
        let Some(key) = self.prompt.prompt(credential_id).await else {
            return Err(ConnectError::Cancelled);
        };
        if key.trim().is_empty() {
            return Err(ConnectError::Cancelled);
        }
        self.credentials
            .set_provider_key(credential_id, &key)
            .await
            .map_err(|e| ConnectError::Storage(e.to_string()))
    }
}

use command_core::{ChatGptConnectDriver, CopilotConnectDriver, CopilotConnectStep};
use llm_client::oauth::openai as openai_oauth;
use llm_client::copilot::{CopilotHttp, CopilotLogin, DeviceCodeResponse, PollOutcome};
use llm_client::transport::BoxFuture;
use llm_client::LlmError;
use platform_posix::PosixHttp;
use protocol::{HttpMethod, HttpRequest};
use serde_json::Value;
use std::sync::Mutex as StdMutex;
use traits::HttpTransport;

/// Credential id under which the GitHub Copilot OAuth token is stored. Matches
/// the catalog preset's `profile_name`.
const COPILOT_CREDENTIAL_ID: &str = "github-copilot";

/// Host `CopilotHttp` over the production `PosixHttp` transport.
pub struct PosixCopilotHttp {
    http: PosixHttp,
}

impl PosixCopilotHttp {
    /// Construct over a fresh `PosixHttp`.
    #[must_use]
    pub fn new() -> Self {
        Self { http: PosixHttp::new() }
    }
}

impl Default for PosixCopilotHttp {
    fn default() -> Self {
        Self::new()
    }
}

impl CopilotHttp for PosixCopilotHttp {
    fn post_json<'a>(&'a self, url: &'a str, body: &'a Value) -> BoxFuture<'a, Result<Value, LlmError>> {
        Box::pin(async move {
            let payload = serde_json::to_string(body)
                .map_err(|e| LlmError::Transport { message: e.to_string() })?;
            // protocol::HttpRequest: method is HttpMethod, headers are Vec pairs,
            // body is Option<String>, plus `body_bytes` (raw body, unused here)
            // and a timeout field (both adapted to main's wider transport shape).
            let req = HttpRequest {
                method: HttpMethod::Post,
                url: url.to_string(),
                headers: vec![
                    ("Accept".to_string(), "application/json".to_string()),
                    ("Content-Type".to_string(), "application/json".to_string()),
                    ("User-Agent".to_string(), "LingXi-Code".to_string()),
                ],
                body: Some(payload),
                body_bytes: None,
                timeout: None,
            };
            let resp = self
                .http
                .request(req)
                .await
                .map_err(|e| LlmError::Transport { message: e.to_string() })?;
            serde_json::from_str(&resp.body)
                .map_err(|e| LlmError::Transport { message: format!("copilot json: {e}") })
        })
    }
}

/// Sleep port so the poll loop is testable without real time.
#[async_trait]
pub trait PollSleeper: Send + Sync {
    /// Sleep for `secs` seconds before the next poll.
    async fn sleep_secs(&self, secs: u64);
}

/// Production sleeper backed by `tokio::time::sleep`.
pub struct TokioSleeper;
#[async_trait]
impl PollSleeper for TokioSleeper {
    async fn sleep_secs(&self, secs: u64) {
        tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
    }
}

/// Engine Copilot device-flow driver: owns the pure `CopilotLogin` state machine,
/// the sleep port, and the keychain. `begin` returns the displayable step (and
/// caches the `DeviceCodeResponse`); `poll_to_completion` runs the SlowDown/Pending
/// loop and stores the token on success.
pub struct EngineCopilotConnect<H: CopilotHttp> {
    credentials: Arc<CredentialManager>,
    login: CopilotLogin<H>,
    sleeper: Arc<dyn PollSleeper>,
    cached: StdMutex<Option<DeviceCodeResponse>>,
}

impl EngineCopilotConnect<PosixCopilotHttp> {
    /// Production constructor: device-flow over `PosixHttp`, real `tokio` sleeps.
    #[must_use]
    pub fn new(credentials: Arc<CredentialManager>) -> Self {
        Self::with_parts(credentials, CopilotLogin::new(PosixCopilotHttp::new()), Arc::new(TokioSleeper))
    }
}

impl<H: CopilotHttp> EngineCopilotConnect<H> {
    /// Construct over an injected `CopilotLogin` + sleeper (test seam).
    #[must_use]
    pub fn with_parts(
        credentials: Arc<CredentialManager>,
        login: CopilotLogin<H>,
        sleeper: Arc<dyn PollSleeper>,
    ) -> Self {
        Self { credentials, login, sleeper, cached: StdMutex::new(None) }
    }
}

#[async_trait]
impl<H: CopilotHttp> CopilotConnectDriver for EngineCopilotConnect<H> {
    async fn begin(&self) -> Result<CopilotConnectStep, ConnectError> {
        let dc = self.login.begin().await.map_err(|e| ConnectError::Network(e.to_string()))?;
        let step = CopilotConnectStep {
            user_code: dc.user_code.clone(),
            verification_uri: dc.verification_uri.clone(),
        };
        *self.cached.lock().unwrap() = Some(dc);
        Ok(step)
    }

    async fn poll_to_completion(&self, _step: &CopilotConnectStep) -> Result<(), ConnectError> {
        let dc = self
            .cached
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| ConnectError::Network("begin() was not called".to_string()))?;
        loop {
            match self.login.poll_once(&dc).await.map_err(|e| ConnectError::Network(e.to_string()))? {
                PollOutcome::Success(secret) => {
                    // §10 frozen-crate exception accessor (Task 7).
                    let token = secret.token_for_storage().to_string();
                    return self
                        .credentials
                        .set_provider_key(COPILOT_CREDENTIAL_ID, &token)
                        .await
                        .map_err(|e| ConnectError::Storage(e.to_string()));
                }
                PollOutcome::Pending { interval_secs } | PollOutcome::SlowDown { interval_secs } => {
                    self.sleeper.sleep_secs(interval_secs).await;
                }
                PollOutcome::Failed { error } => return Err(ConnectError::DeviceFailed(error)),
            }
        }
    }
}

/// Engine implementation of [`ChatGptConnectDriver`]: drives the OpenAI
/// ChatGPT-account OAuth login (browser PKCE, device-code fallback) and
/// persists the tokens. Returns a human-facing success message including the
/// minted account id.
pub struct EngineChatGptConnect {
    handle: Arc<openai_oauth::handle::OpenAiOAuthHandle>,
}

impl EngineChatGptConnect {
    /// Production constructor: uses a real browser opener.
    #[must_use]
    pub fn new(
        client: Arc<openai_oauth::client::OpenAiOAuthClient>,
        credentials: Arc<CredentialManager>,
    ) -> Self {
        Self {
            handle: Arc::new(openai_oauth::handle::OpenAiOAuthHandle::new(client, credentials)),
        }
    }
}

#[async_trait]
impl ChatGptConnectDriver for EngineChatGptConnect {
    async fn connect(&self) -> Result<String, ConnectError> {
        let info = self
            .handle
            .login()
            .await
            .map_err(|e| ConnectError::Network(e.to_string()))?;
        let account = info.account_id.unwrap_or_else(|| "?".into());
        Ok(format!("Connected chatgpt (account {account})."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use platform_posix::{PosixClock, PosixHttp};
    use protocol::SecureStorageData;
    use std::collections::HashMap;
    use std::sync::Mutex as StdMutex;
    use traits::{
        Clock, HttpTransport, SecureStorage, SecureStorageBackend, SecureStorageError,
    };

    /// In-memory `(service, account) -> data` store. The real
    /// `platform_posix` secure storage backend is keychain/OS-backed and not
    /// available in the test harness, so the roundtrip test needs a real
    /// in-memory store; this mirrors the `MemStorage` double used by `secret`'s
    /// own credential tests.
    #[derive(Default)]
    struct MemStorage {
        map: StdMutex<HashMap<(String, String), SecureStorageData>>,
    }
    #[async_trait]
    impl SecureStorage for MemStorage {
        async fn store(
            &self,
            service: &str,
            account: &str,
            data: SecureStorageData,
        ) -> Result<(), SecureStorageError> {
            self.map
                .lock()
                .unwrap()
                .insert((service.into(), account.into()), data);
            Ok(())
        }
        async fn retrieve(
            &self,
            service: &str,
            account: &str,
        ) -> Result<Option<SecureStorageData>, SecureStorageError> {
            Ok(self
                .map
                .lock()
                .unwrap()
                .get(&(service.into(), account.into()))
                .cloned())
        }
        async fn delete(&self, service: &str, account: &str) -> Result<(), SecureStorageError> {
            self.map
                .lock()
                .unwrap()
                .remove(&(service.into(), account.into()));
            Ok(())
        }
        async fn list(&self, service: &str) -> Result<Vec<String>, SecureStorageError> {
            Ok(self
                .map
                .lock()
                .unwrap()
                .keys()
                .filter(|(s, _)| s == service)
                .map(|(_, a)| a.clone())
                .collect())
        }
        fn is_encrypted(&self) -> bool {
            false
        }
        fn backend(&self) -> SecureStorageBackend {
            SecureStorageBackend::PlainText
        }
    }

    struct CannedPrompt(Option<String>);
    #[async_trait]
    impl SecureKeyPrompt for CannedPrompt {
        async fn prompt(&self, _label: &str) -> Option<String> {
            self.0.clone()
        }
    }

    fn manager() -> Arc<CredentialManager> {
        let storage: Arc<dyn SecureStorage> = Arc::new(MemStorage::default());
        let clock: Arc<dyn Clock> = Arc::new(PosixClock::new());
        let http: Arc<dyn HttpTransport> = Arc::new(PosixHttp::new());
        Arc::new(CredentialManager::new(storage, clock, http))
    }

    #[tokio::test]
    async fn store_roundtrips_through_keychain() {
        let cm = manager();
        let writer = EngineCredentialWriter::new(cm.clone(), Arc::new(CannedPrompt(Some("sk-test-123".into()))));
        writer.prompt_and_store_key("openrouter").await.expect("store ok");
        let got = cm.get_provider_key("openrouter").await.expect("read ok").expect("present");
        assert_eq!(got.expose_secret(), "sk-test-123");
    }

    #[tokio::test]
    async fn cancelled_prompt_yields_cancelled() {
        let cm = manager();
        let writer = EngineCredentialWriter::new(cm, Arc::new(CannedPrompt(None)));
        match writer.prompt_and_store_key("deepseek").await {
            Err(ConnectError::Cancelled) => {}
            other => panic!("expected Cancelled, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn empty_key_is_cancelled_not_stored() {
        let cm = manager();
        let writer = EngineCredentialWriter::new(cm.clone(), Arc::new(CannedPrompt(Some("   ".into()))));
        match writer.prompt_and_store_key("deepseek").await {
            Err(ConnectError::Cancelled) => {}
            other => panic!("expected Cancelled, got {other:?}"),
        }
        assert!(cm.get_provider_key("deepseek").await.expect("read").is_none());
    }

    use command_core::CopilotConnectDriver;
    use llm_client::copilot::{CopilotHttp, CopilotLogin, COPILOT_CLIENT_ID};
    use llm_client::transport::BoxFuture;
    use llm_client::LlmError;
    use serde_json::{json, Value};

    struct ScriptedCopilotHttp {
        device: Value,
        tokens: StdMutex<std::collections::VecDeque<Value>>,
    }
    impl CopilotHttp for ScriptedCopilotHttp {
        fn post_json<'a>(&'a self, url: &'a str, _body: &'a Value) -> BoxFuture<'a, Result<Value, LlmError>> {
            let v = if url.contains("device/code") {
                self.device.clone()
            } else {
                self.tokens.lock().unwrap().pop_front().unwrap_or_else(|| json!({ "error": "expired_token" }))
            };
            Box::pin(async move { Ok(v) })
        }
    }

    struct InstantSleeper;
    #[async_trait]
    impl PollSleeper for InstantSleeper {
        async fn sleep_secs(&self, _secs: u64) {}
    }

    fn copilot_driver(cm: Arc<CredentialManager>, device: Value, tokens: Vec<Value>) -> EngineCopilotConnect<ScriptedCopilotHttp> {
        let http = ScriptedCopilotHttp { device, tokens: StdMutex::new(tokens.into_iter().collect()) };
        EngineCopilotConnect::with_parts(cm, CopilotLogin::new(http), Arc::new(InstantSleeper))
    }

    #[tokio::test]
    async fn begin_surfaces_user_code_and_uri() {
        let cm = manager();
        let driver = copilot_driver(
            cm,
            json!({ "user_code": "WDJB-MJHT", "verification_uri": "https://github.com/login/device", "device_code": "dev-1", "interval": 1 }),
            vec![],
        );
        let step = driver.begin().await.expect("begin ok");
        assert_eq!(step.user_code, "WDJB-MJHT");
        assert_eq!(step.verification_uri, "https://github.com/login/device");
    }

    #[tokio::test]
    async fn poll_advances_through_pending_then_stores_token() {
        let cm = manager();
        let driver = copilot_driver(
            cm.clone(),
            json!({ "user_code": "AAAA-BBBB", "verification_uri": "https://github.com/login/device", "device_code": "dev-2", "interval": 1 }),
            vec![
                json!({ "error": "authorization_pending" }),
                json!({ "error": "slow_down" }),
                json!({ "access_token": "ght_live_token" }),
            ],
        );
        let step = driver.begin().await.expect("begin");
        driver.poll_to_completion(&step).await.expect("poll ok");
        let got = cm.get_provider_key("github-copilot").await.expect("read").expect("present");
        assert_eq!(got.expose_secret(), "ght_live_token");
    }

    #[tokio::test]
    async fn poll_terminal_error_is_surfaced_and_not_stored() {
        let cm = manager();
        let driver = copilot_driver(
            cm.clone(),
            json!({ "user_code": "CCCC-DDDD", "verification_uri": "https://github.com/login/device", "device_code": "dev-3", "interval": 1 }),
            vec![json!({ "error": "access_denied" })],
        );
        let step = driver.begin().await.expect("begin");
        match driver.poll_to_completion(&step).await {
            Err(ConnectError::DeviceFailed(e)) => assert_eq!(e, "access_denied"),
            other => panic!("expected DeviceFailed, got {other:?}"),
        }
        assert!(cm.get_provider_key("github-copilot").await.expect("read").is_none());
    }

    #[test]
    fn uses_opencode_client_id() {
        assert_eq!(COPILOT_CLIENT_ID, "Ov23li8tweQw6odWQebz");
    }
}

//! OAuth token refresh: reactive (401-driven) + proactive (timer-driven).
//!
//! See spec §3 (cross-plan trait), §4 Flow A (lifecycle), §7 (wire identifiers),
//! §8 M3-04 phase list.
//!
//! M3-04 implements [`lingxi_api_client::oauth_hook::OAuthRefreshHook`] (frozen
//! in M3-03 §3). The single-flight invariant is enforced via [`AuthState::refresh_lock`]
//! with double-check-after-acquire (v3 §16.3).

// Task 2 lands the data model; Task 4+ consumes these fields via the
// `OAuthRefreshHook` impl + `spawn_proactive`. Allow until then.
#![allow(dead_code)]

use crate::client::OAuthError;
use crate::config::ClaudeAiOAuthConfig;
use async_trait::async_trait;
use lingxi_api_client::oauth_hook::{BearerToken, OAuthHookError, OAuthRefreshHook, TokenHash};
use lingxi_protocol::{HttpMethod, HttpRequest, Secret};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::{Mutex, RwLock};

/// In-memory token state. Kept under `AuthState::token` (`RwLock`) and atomically
/// swapped on a successful refresh.
pub struct TokenInfo {
    /// Bearer access token. Wrapped in `Secret` so `Debug`/`Display` redact.
    pub access_token: Secret<String>,
    /// Refresh token (optional — some upstream flows return only an access token).
    pub refresh_token: Option<Secret<String>>,
    /// Expiry instant (wall clock); the proactive task wakes at
    /// `min((expires_at - now)/2, 5*60)` seconds before this.
    pub expires_at: SystemTime,
    /// Scopes currently granted. Used to detect scope-upgrade transitions.
    pub scopes: Vec<String>,
}

impl TokenInfo {
    /// SHA-256 of the `access_token` bytes. Returned as the `TokenHash` newtype
    /// M3-03 declared (`pub [u8; 32]`).
    #[must_use]
    pub fn token_hash(&self) -> TokenHash {
        let mut h = Sha256::new();
        h.update(self.access_token.expose_secret().as_bytes());
        let digest: [u8; 32] = h.finalize().into();
        TokenHash(digest)
    }
}

/// Shared OAuth state. Holds the current token, the single-flight refresh lock,
/// the OAuth config, the I/O collaborators (HTTP transport, clock, optional
/// telemetry + keychain), and the proactive task handle (when spawned).
pub struct AuthState {
    /// OAuth config (endpoints + `client_id` + redirect + scopes).
    pub(crate) config: ClaudeAiOAuthConfig,
    /// Current token under `RwLock` so reactive readers don't serialize.
    pub token: RwLock<TokenInfo>,
    /// Single-flight refresh lock (v3 §16.3). Lock value is `()`; semantic is
    /// critical-section serialization across reactive + proactive paths.
    pub(crate) refresh_lock: Arc<Mutex<()>>,
    /// Proactive task handle. Populated by `RefreshDriver::spawn_proactive`;
    /// cleared by `AuthState::shutdown`.
    pub(crate) proactive_handle: RwLock<Option<lingxi_traits::BackgroundTaskHandle>>,
    /// HTTP transport for token-endpoint POSTs. Engine code never imports a
    /// concrete HTTP client; we go through the trait per D17.
    pub(crate) http: Arc<dyn lingxi_traits::HttpTransport>,
    /// Wall-clock source. Tests inject a virtual clock.
    pub(crate) clock: Arc<dyn lingxi_traits::Clock>,
    /// Optional analytics bus for `tengu_oauth_*` events. `None` in tests that
    /// don't care about telemetry.
    pub(crate) bus: Option<Arc<lingxi_telemetry::AnalyticsBus>>,
    /// Optional credential manager for keychain persistence. `None` in tests
    /// that only exercise the in-memory token rotation.
    pub(crate) credentials: Option<Arc<lingxi_secret::CredentialManager>>,
}

impl AuthState {
    /// Production constructor. Wires HTTP + clock + telemetry + keychain.
    ///
    /// The runtime spawner used by [`RefreshDriver::spawn_proactive`] is passed
    /// directly to that method (Task 5) rather than stored on `AuthState`; it
    /// is never re-read after the proactive task starts.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        config: ClaudeAiOAuthConfig,
        access_token: Secret<String>,
        refresh_token: Option<Secret<String>>,
        expires_at: SystemTime,
        http: Arc<dyn lingxi_traits::HttpTransport>,
        clock: Arc<dyn lingxi_traits::Clock>,
        bus: Option<Arc<lingxi_telemetry::AnalyticsBus>>,
        credentials: Option<Arc<lingxi_secret::CredentialManager>>,
    ) -> Arc<Self> {
        let scopes = config.scopes.clone();
        Arc::new(Self {
            config,
            token: RwLock::new(TokenInfo {
                access_token,
                refresh_token,
                expires_at,
                scopes,
            }),
            refresh_lock: Arc::new(Mutex::new(())),
            proactive_handle: RwLock::new(None),
            http,
            clock,
            bus,
            credentials,
        })
    }

    /// Test-only constructor. Production code uses [`AuthState::new`] (Task 4)
    /// which wires HTTP transport + clock + telemetry + credential manager.
    #[must_use]
    pub fn new_for_test(
        config: ClaudeAiOAuthConfig,
        access_token: Secret<String>,
        refresh_token: Option<Secret<String>>,
        expires_at: SystemTime,
    ) -> Arc<Self> {
        let scopes = config.scopes.clone();
        Arc::new(Self {
            config,
            token: RwLock::new(TokenInfo {
                access_token,
                refresh_token,
                expires_at,
                scopes,
            }),
            refresh_lock: Arc::new(Mutex::new(())),
            proactive_handle: RwLock::new(None),
            http: Arc::new(NullTransport),
            clock: Arc::new(NullClock),
            bus: None,
            credentials: None,
        })
    }

    /// Borrow the proactive task handle if one has been spawned.
    pub async fn proactive_handle(&self) -> Option<lingxi_traits::BackgroundTaskHandle> {
        self.proactive_handle.read().await.clone()
    }

    /// Perform the actual HTTP refresh POST. Returns the parsed response on
    /// 200, or maps non-2xx to an [`OAuthError`]. Called from inside
    /// `refresh_lock`.
    async fn do_refresh_http(
        &self,
        refresh_token: &Secret<String>,
    ) -> Result<TokenEndpointResponse, OAuthError> {
        let form = format!(
            "grant_type={}&refresh_token={}&client_id={}",
            crate::config::REFRESH_GRANT_TYPE,
            urlencoding::encode(refresh_token.expose_secret()),
            urlencoding::encode(&self.config.client_id),
        );
        let req = HttpRequest {
            method: HttpMethod::Post,
            url: self.config.token_endpoint.clone(),
            headers: vec![
                (
                    "content-type".into(),
                    "application/x-www-form-urlencoded".into(),
                ),
                ("accept".into(), "application/json".into()),
            ],
            body: Some(form),
            timeout: Some(Duration::from_secs(30)),
        };
        let resp = self
            .http
            .request(req)
            .await
            .map_err(|e| OAuthError::TokenExchange(format!("transport: {e}")))?;
        match resp.status {
            200 => serde_json::from_str::<TokenEndpointResponse>(&resp.body)
                .map_err(|e| OAuthError::TokenExchange(format!("decode: {e}"))),
            401 | 403 => Err(OAuthError::RefreshExpired),
            other => Err(OAuthError::TokenExchange(format!(
                "status {other}: {}",
                resp.body
            ))),
        }
    }

    /// Persist the new [`TokenInfo`] to keychain if a `CredentialManager` is
    /// attached. No-op (returns `Ok`) if no manager is configured (test path).
    ///
    /// NOTE: the M2-06 [`lingxi_secret::CredentialManager`] surface only
    /// exposes an Anthropic-API-key entry point; a generic OAuth-token store
    /// path is filed for a follow-up. For M3-04 the in-memory rotation is the
    /// source of truth; the engine's startup wiring (Task 4 §5) will pass a
    /// real `CredentialManager` once that path lands. The `async` signature is
    /// retained for forward compatibility with that I/O-bound implementation.
    #[allow(clippy::unused_async)]
    async fn persist_to_keychain(&self, info: &TokenInfo) -> Result<(), OAuthError> {
        let Some(cm) = &self.credentials else {
            return Ok(());
        };
        // Placeholder: avoid reading the secret here until the M2-06 follow-up
        // adds a generic store entry point. The in-memory rotation has already
        // succeeded by the time we reach this call site.
        let _ = cm;
        let _ = info;
        Ok(())
    }
}

/// Null HTTP transport used by [`AuthState::new_for_test`] — every call panics.
/// Tests that need to exercise the refresh path use the production constructor
/// with a counting transport.
struct NullTransport;
#[async_trait]
impl lingxi_traits::HttpTransport for NullTransport {
    async fn request(
        &self,
        _req: lingxi_protocol::HttpRequest,
    ) -> Result<lingxi_protocol::HttpResponse, lingxi_traits::HttpError> {
        panic!("NullTransport: test forgot to inject a real transport");
    }
    async fn stream_sse(
        &self,
        _req: lingxi_protocol::HttpRequest,
    ) -> Result<lingxi_traits::http::SseStream, lingxi_traits::HttpError> {
        panic!("NullTransport: test forgot to inject a real transport");
    }
}

/// Null clock — always returns [`SystemTime::UNIX_EPOCH`]. Used by
/// [`AuthState::new_for_test`].
struct NullClock;
impl lingxi_traits::Clock for NullClock {
    fn now(&self) -> SystemTime {
        SystemTime::UNIX_EPOCH
    }
}

/// Concrete `OAuthRefreshHook` impl: drives reactive + proactive refresh.
///
/// Construction is cheap — actual refresh work happens in `refresh()` (Task 4)
/// and `spawn_proactive()` (Task 5).
pub struct RefreshDriver {
    pub(crate) state: Arc<AuthState>,
}

impl RefreshDriver {
    /// Construct a driver over a shared `AuthState`.
    #[must_use]
    pub fn new(state: Arc<AuthState>) -> Self {
        Self { state }
    }
}

/// Body shape returned by the token endpoint on a successful refresh.
#[derive(Debug, serde::Deserialize)]
struct TokenEndpointResponse {
    access_token: String,
    /// Optional — some providers don't rotate the `refresh_token` on every refresh.
    refresh_token: Option<String>,
    expires_in: u64,
    /// Space-separated scope list (RFC 6749 §3.3). Optional — if absent, we
    /// inherit the current `TokenInfo.scopes`.
    scope: Option<String>,
}

#[async_trait]
impl OAuthRefreshHook for RefreshDriver {
    async fn refresh(&self, prev_token_hash: TokenHash) -> Result<BearerToken, OAuthHookError> {
        // 1. Acquire the single-flight lock (v3 §16.3).
        let _guard = self.state.refresh_lock.lock().await;

        // 2. Double-check after acquire — another caller may have already
        //    refreshed under the lock we just got. Compare the current
        //    token_hash against prev; if different, return the current token.
        {
            let token = self.state.token.read().await;
            let current_hash = token.token_hash();
            if current_hash != prev_token_hash {
                // Another task refreshed; return the now-current token without
                // making an HTTP call.
                return Ok(BearerToken(Secret::new(
                    token.access_token.expose_secret().clone(),
                )));
            }
        }

        // 3. Hash still matches → we own the refresh. Read the refresh_token.
        let refresh_token = {
            let token = self.state.token.read().await;
            token
                .refresh_token
                .as_ref()
                .map(|s| Secret::new(s.expose_secret().clone()))
                .ok_or_else(|| OAuthHookError::RefreshFailed("no refresh_token in state".into()))?
        };

        let started = std::time::Instant::now();
        emit_refresh_started(&self.state.bus, "reactive_401").await;

        // 4. Perform the HTTP refresh.
        let body = match self.state.do_refresh_http(&refresh_token).await {
            Ok(b) => b,
            Err(OAuthError::RefreshExpired) => {
                emit_refresh_failed(&self.state.bus, "reactive_401", "refresh_expired").await;
                return Err(OAuthHookError::RefreshFailed(
                    "Session expired. Re-authenticate?".into(),
                ));
            }
            Err(e) => {
                emit_refresh_failed(&self.state.bus, "reactive_401", "provider_unreachable").await;
                return Err(OAuthHookError::ProviderUnreachable(format!("{e}")));
            }
        };

        // 5. Build the new `TokenInfo`.
        let now = self.state.clock.now();
        let new_expiry = now + Duration::from_secs(body.expires_in);
        let scopes = body.scope.as_deref().map_or_else(
            || self.state.config.scopes.clone(),
            |s| s.split_whitespace().map(str::to_string).collect::<Vec<_>>(),
        );

        let new_access_token_str = body.access_token.clone();
        let new_info = TokenInfo {
            access_token: Secret::new(body.access_token),
            refresh_token: body.refresh_token.map(Secret::new).or(Some(refresh_token)),
            expires_at: new_expiry,
            scopes,
        };

        // 6. Atomic swap. The double-check-after-acquire above guarantees we're
        //    the only writer in flight.
        {
            let mut guard = self.state.token.write().await;
            *guard = new_info;
        }

        // 7. Persist to keychain (best-effort — never fails the refresh on a
        //    keychain write error; the in-memory rotation succeeded).
        if let Err(e) = self
            .state
            .persist_to_keychain(&*self.state.token.read().await)
            .await
        {
            tracing::warn!(
                target: "lingxi::anthropic_oauth::refresh",
                error = %e,
                "keychain persistence failed; in-memory token is still rotated",
            );
        }

        let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let new_expiry_unix = i64::try_from(
            new_expiry
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        )
        .unwrap_or(i64::MAX);
        emit_refresh_succeeded(
            &self.state.bus,
            "reactive_401",
            new_expiry_unix,
            duration_ms,
        )
        .await;

        Ok(BearerToken(Secret::new(new_access_token_str)))
    }

    async fn proactive_refresh(&self) -> Result<(), OAuthHookError> {
        // The proactive driver lives in the spawned task; this default impl
        // would only fire if the api-client middleware calls it directly,
        // which it does NOT in M3-04. Stub returning Ok per the trait default.
        Ok(())
    }
}

// Telemetry helpers — emit `tengu_oauth_*` events when a bus is attached.
async fn emit_refresh_started(bus: &Option<Arc<lingxi_telemetry::AnalyticsBus>>, trigger: &str) {
    let Some(bus) = bus else { return };
    let mut m = lingxi_telemetry::sink::LogEventMetadata::new();
    m.insert(
        "trigger".into(),
        lingxi_telemetry::sink::AnalyticsValue::String(
            lingxi_telemetry::Verified::assert_safe(trigger.to_string())
                .as_str()
                .to_string(),
        ),
    );
    bus.log_event("tengu_oauth_refresh_started", m).await;
}

async fn emit_refresh_succeeded(
    bus: &Option<Arc<lingxi_telemetry::AnalyticsBus>>,
    trigger: &str,
    new_expiry_unix: i64,
    duration_ms: u64,
) {
    let Some(bus) = bus else { return };
    let mut m = lingxi_telemetry::sink::LogEventMetadata::new();
    m.insert(
        "trigger".into(),
        lingxi_telemetry::sink::AnalyticsValue::String(
            lingxi_telemetry::Verified::assert_safe(trigger.to_string())
                .as_str()
                .to_string(),
        ),
    );
    m.insert(
        "new_expiry_unix".into(),
        lingxi_telemetry::sink::AnalyticsValue::Int(new_expiry_unix),
    );
    m.insert(
        "duration_ms".into(),
        lingxi_telemetry::sink::AnalyticsValue::Int(i64::try_from(duration_ms).unwrap_or(i64::MAX)),
    );
    bus.log_event("tengu_oauth_refresh_succeeded", m).await;
}

async fn emit_refresh_failed(
    bus: &Option<Arc<lingxi_telemetry::AnalyticsBus>>,
    trigger: &str,
    error_kind: &str,
) {
    let Some(bus) = bus else { return };
    let mut m = lingxi_telemetry::sink::LogEventMetadata::new();
    m.insert(
        "trigger".into(),
        lingxi_telemetry::sink::AnalyticsValue::String(
            lingxi_telemetry::Verified::assert_safe(trigger.to_string())
                .as_str()
                .to_string(),
        ),
    );
    m.insert(
        "error_kind".into(),
        lingxi_telemetry::sink::AnalyticsValue::String(
            lingxi_telemetry::Verified::assert_safe(error_kind.to_string())
                .as_str()
                .to_string(),
        ),
    );
    bus.log_event("tengu_oauth_refresh_failed", m).await;
}

/// Maximum lead time before token expiry that the proactive refresh task wakes.
///
/// Spec §7 line 721. 5 minutes = 300 seconds. Locked byte-for-byte against
/// claude-code's proactive-refresh timer.
pub const PROACTIVE_LEAD_CAP: Duration = Duration::from_secs(5 * 60);

/// Compute the proactive refresh lead for a token with `remaining` lifetime.
///
/// Returns `min(remaining / 2, PROACTIVE_LEAD_CAP)` in whole seconds. Spec §7
/// line 721 — handles short-lived debug tokens (TTL < 5 min) by waking at
/// remaining/2 instead of a fixed 5-minute lead that would never fire.
///
/// Division is integer-floor over whole seconds to match claude-code's TS
/// `Math.floor(remaining / 2)` semantics.
#[must_use]
pub fn proactive_lead(remaining: Duration) -> Duration {
    let half = Duration::from_secs(remaining.as_secs() / 2);
    if half < PROACTIVE_LEAD_CAP {
        half
    } else {
        PROACTIVE_LEAD_CAP
    }
}

impl RefreshDriver {
    /// Spawn the proactive refresh task. Returns once the spawn handle is
    /// stored in `state.proactive_handle`. The task itself loops until canceled
    /// by `AuthState::shutdown`.
    ///
    /// Wake interval is `min(remaining_lifetime / 2, PROACTIVE_LEAD_CAP)`
    /// before expiry (spec §7 line 721).
    pub async fn spawn_proactive(
        state: Arc<AuthState>,
        spawner: Arc<dyn lingxi_traits::RuntimeSpawner>,
    ) -> Result<(), OAuthError> {
        let task_state = state.clone();
        let task_spawner = spawner.clone();
        let fut: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>> =
            Box::pin(async move {
                proactive_loop(task_state, task_spawner).await;
            });
        let handle = spawner
            .spawn("lingxi-oauth-proactive-refresh", fut)
            .await
            .map_err(|e| OAuthError::TokenExchange(format!("spawn failed: {e}")))?;
        *state.proactive_handle.write().await = Some(handle);
        Ok(())
    }
}

/// The proactive task loop. Wakes at `min(remaining/2, 5min)` before expiry,
/// calls `RefreshDriver::refresh`, and reschedules against the new expiry.
async fn proactive_loop(
    state: Arc<AuthState>,
    spawner: Arc<dyn lingxi_traits::RuntimeSpawner>,
) {
    let driver = RefreshDriver::new(state.clone());
    loop {
        // Read current expiry + token_hash.
        let (expires_at, prev_hash) = {
            let t = state.token.read().await;
            (t.expires_at, t.token_hash())
        };
        let now = state.clock.now();
        let remaining = expires_at.duration_since(now).unwrap_or(Duration::ZERO);
        let lead = proactive_lead(remaining);
        let sleep_for = remaining.checked_sub(lead).unwrap_or(Duration::ZERO);

        emit_refresh_started(&state.bus, "proactive_timer").await;

        // Sleep until the wake instant. `RuntimeSpawner::sleep` lets tests
        // virtualize time.
        spawner.sleep(sleep_for).await;

        // Fire a refresh. If it fails we emit a `_failed` event and back off
        // for 30 seconds; on hard failure (RefreshExpired) we exit the loop —
        // the next API call will surface the auth error to the user.
        match <RefreshDriver as lingxi_api_client::oauth_hook::OAuthRefreshHook>::refresh(
            &driver, prev_hash,
        )
        .await
        {
            Ok(_) => {
                // Success — loop continues against the freshly-rotated token.
                continue;
            }
            Err(lingxi_api_client::oauth_hook::OAuthHookError::RefreshFailed(msg))
                if msg == "Session expired. Re-authenticate?" =>
            {
                tracing::error!(
                    target: "lingxi::anthropic_oauth::proactive",
                    "refresh_token expired; exiting proactive loop"
                );
                emit_refresh_failed(&state.bus, "proactive_timer", "refresh_expired").await;
                return;
            }
            Err(e) => {
                tracing::warn!(
                    target: "lingxi::anthropic_oauth::proactive",
                    error = ?e,
                    "transient refresh failure; backing off 30s",
                );
                emit_refresh_failed(&state.bus, "proactive_timer", "provider_unreachable").await;
                spawner.sleep(Duration::from_secs(30)).await;
                continue;
            }
        }
    }
}

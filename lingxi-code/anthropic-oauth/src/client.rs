//! Claude.ai OAuth client skeleton.
//!
//! See spec §30. M1.19 ships the contract + PKCE/state generation + the
//! authorize URL builder; browser-open + token exchange land with the
//! cli-demo in Plan 16.

use crate::config::ClaudeAiOAuthConfig;
use crate::pkce::{generate_pkce, generate_state_token};
use crate::refresh::{AuthState, RefreshDriver};
use api_client::oauth_hook::register_oauth_hook;
use protocol::Secret;
use secret::CredentialManager;
use std::sync::Arc;
use std::time::SystemTime;
use thiserror::Error;
use traits::HttpTransport;

/// OAuth-flow failures.
#[derive(Debug, Error)]
pub enum OAuthError {
    /// Loopback callback failed (bind, parse, or state mismatch).
    #[error("callback failed: {0}")]
    Callback(String),
    /// Token exchange against the `IdP` failed.
    #[error("token exchange failed: {0}")]
    TokenExchange(String),
    /// Stored `refresh_token` has expired or was revoked. User must re-authenticate.
    ///
    /// Display string is locked byte-for-byte against claude-code @ 6a25909
    /// (`"Session expired. Re-authenticate?"`).
    #[error("Session expired. Re-authenticate?")]
    RefreshExpired,
    /// Scope upgrade attempt was denied by the provider.
    #[error("Scope upgrade denied by provider")]
    ScopeRejected {
        /// Scopes the provider required.
        required: Vec<String>,
        /// Scopes the token currently holds.
        granted: Vec<String>,
    },
    /// Proactive refresh task failed and is shutting down.
    #[error("proactive refresh failed: {source}")]
    ProactiveFailed {
        /// The underlying error that caused the proactive task to fail.
        source: Box<OAuthError>,
    },
}

/// Stateful client for the Claude.ai Authorization Code flow.
///
/// The `http` transport and `credentials` manager are stored for the full
/// implementation in Plan 16; M1.19 only exercises [`Self::build_authorize_url`].
pub struct ClaudeAiOAuthClient {
    config: ClaudeAiOAuthConfig,
    #[allow(dead_code)]
    http: Arc<dyn HttpTransport>,
    #[allow(dead_code)]
    credentials: Arc<CredentialManager>,
}

impl ClaudeAiOAuthClient {
    /// Construct a client with a static config, HTTP transport, and credential store.
    #[must_use]
    pub fn new(
        config: ClaudeAiOAuthConfig,
        http: Arc<dyn HttpTransport>,
        credentials: Arc<CredentialManager>,
    ) -> Self {
        Self {
            config,
            http,
            credentials,
        }
    }

    /// Build the authorize URL, returning `(url, verifier, state)`.
    ///
    /// Caller opens `url` in a browser, holds `verifier` until the loopback
    /// callback fires, and validates the redirect's `state` against the
    /// returned token. Full impl lands in Plan 16 cli-demo.
    #[must_use]
    pub fn build_authorize_url(&self) -> (String, String, String) {
        let (verifier, challenge) = generate_pkce();
        let state = generate_state_token();
        let scopes = self.config.scopes.join(" ");
        let url = format!(
            "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&state={}&code_challenge={}&code_challenge_method=S256",
            self.config.authorization_endpoint,
            urlencoding::encode(&self.config.client_id),
            urlencoding::encode(&self.config.redirect_uri),
            urlencoding::encode(&scopes),
            urlencoding::encode(&state),
            urlencoding::encode(&challenge),
        );
        (url, verifier, state)
    }
}

/// Wire the OAuth refresh subsystem: construct `AuthState`, register the
/// `OAuthRefreshHook` impl process-globally, and spawn the proactive task.
///
/// Returns the `Arc<AuthState>` so the engine can call `shutdown()` at
/// `Engine::shutdown` time. Re-calling `init_refresh_driver` is a bug
/// (process-global hook is already registered); the second call logs a WARN
/// and returns the freshly-constructed `AuthState` so callers see consistent
/// behaviour but stale token data won't be auto-refreshed (the original
/// proactive task is still running on the original state).
///
/// # Errors
/// * [`OAuthError::TokenExchange`] if the runtime spawner fails to spawn.
#[allow(clippy::too_many_arguments)]
pub async fn init_refresh_driver(
    config: ClaudeAiOAuthConfig,
    access_token: Secret<String>,
    refresh_token: Option<Secret<String>>,
    expires_at: SystemTime,
    http: Arc<dyn traits::HttpTransport>,
    clock: Arc<dyn traits::Clock>,
    bus: Option<Arc<telemetry::AnalyticsBus>>,
    credentials: Option<Arc<secret::CredentialManager>>,
    spawner: Arc<dyn traits::RuntimeSpawner>,
) -> Result<Arc<AuthState>, OAuthError> {
    let state = AuthState::new(
        config,
        access_token,
        refresh_token,
        expires_at,
        http,
        clock,
        bus,
        credentials,
    );
    let driver: Arc<dyn api_client::oauth_hook::OAuthRefreshHook> =
        Arc::new(RefreshDriver::new(state.clone()));
    // Process-global registration. Second-call-in-same-process is a logic
    // bug but not fatal in tests; tolerate by warning.
    if let Err(e) = register_oauth_hook(driver) {
        tracing::warn!(
            target: "lingxi::anthropic_oauth::init",
            error = ?e,
            "OAuth hook already registered (likely a second init in same process)",
        );
    }
    RefreshDriver::spawn_proactive(state.clone(), spawner)
        .await
        .map_err(|e| OAuthError::TokenExchange(format!("spawn_proactive: {e}")))?;
    Ok(state)
}

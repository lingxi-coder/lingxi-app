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

use crate::config::ClaudeAiOAuthConfig;
use lingxi_api_client::oauth_hook::TokenHash;
use lingxi_protocol::Secret;
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
/// the OAuth config, and the proactive task handle (when spawned).
pub struct AuthState {
    /// OAuth config (endpoints + `client_id` + redirect + scopes).
    pub(crate) config: ClaudeAiOAuthConfig,
    /// Current token under `RwLock` so reactive readers don't serialize.
    pub(crate) token: RwLock<TokenInfo>,
    /// Single-flight refresh lock (v3 §16.3). Lock value is `()`; semantic is
    /// critical-section serialization across reactive + proactive paths.
    pub(crate) refresh_lock: Arc<Mutex<()>>,
    /// Proactive task handle. Populated by `RefreshDriver::spawn_proactive`;
    /// cleared by `AuthState::shutdown`.
    pub(crate) proactive_handle: RwLock<Option<lingxi_traits::BackgroundTaskHandle>>,
}

impl AuthState {
    /// Test-only constructor. Production code uses `init_refresh_driver` in
    /// `client.rs` (Task 4) which wires HTTP transport + clock + spawner +
    /// credential manager.
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
        })
    }

    /// Borrow the proactive task handle if one has been spawned.
    pub async fn proactive_handle(&self) -> Option<lingxi_traits::BackgroundTaskHandle> {
        self.proactive_handle.read().await.clone()
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

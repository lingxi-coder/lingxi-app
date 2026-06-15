//! Cached credential manager for the Anthropic API key.
//!
//! Wraps the platform [`SecureStorage`] behind a small in-memory cache so the
//! engine's hot path does not touch the OS keychain on every request. M1
//! exposes only the Anthropic API key; OAuth access/refresh tokens land in a
//! follow-up task.

use protocol::{Secret, SecureStorageData, SecureStorageMetadata};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use thiserror::Error;
use tokio::sync::{Mutex, RwLock};
use traits::{Clock, HttpTransport, SecureStorage, SecureStorageError};

use crate::kinds::SecretKind;

/// `(service, account)` keys under which the Anthropic OAuth credentials are
/// stored. The access + refresh tokens live in separate secure-storage entries
/// (each labelled with its own [`SecretKind`]); the non-secret session metadata
/// (email / org / expiry / scopes) lives in a third entry so `current_user`
/// can answer without a network round-trip.
const OAUTH_SERVICE: &str = "lingxi";
const OAUTH_ACCESS_ACCOUNT: &str = "anthropic-oauth-access";
const OAUTH_REFRESH_ACCOUNT: &str = "anthropic-oauth-refresh";
const OAUTH_META_ACCOUNT: &str = "anthropic-oauth-meta";

/// Keychain account name for a per-provider key, namespaced by credential `id`.
fn provider_key_account(id: &str) -> String {
    format!("provider-key-{id}")
}

/// A full Anthropic OAuth credential set as returned by [`CredentialManager::get_oauth_tokens`].
///
/// `access_token` / `refresh_token` are wrapped in [`Secret`] so they redact in
/// logs; the remaining fields are non-secret session metadata persisted in the
/// `anthropic-oauth-meta` entry.
pub struct OAuthTokens {
    /// Bearer access token.
    pub access_token: Secret<String>,
    /// Long-lived refresh token (absent if the provider never issued one).
    pub refresh_token: Option<Secret<String>>,
    /// Wall-clock expiry instant of the access token.
    pub expires_at: SystemTime,
    /// Scopes granted on the access token.
    pub scopes: Vec<String>,
    /// Signed-in user's email address.
    pub email: String,
    /// Anthropic organization id.
    pub org_id: String,
}

/// Non-secret session metadata persisted alongside the OAuth tokens. Serialized
/// to JSON and stored in the `anthropic-oauth-meta` entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct OAuthSessionMeta {
    expires_at: SystemTime,
    scopes: Vec<String>,
    email: String,
    org_id: String,
}

/// Failure modes for [`CredentialManager`] operations.
#[derive(Debug, Clone, Error)]
pub enum CredentialError {
    /// Underlying [`SecureStorage`] call failed.
    #[error(transparent)]
    Storage(#[from] SecureStorageError),
    /// Stored token had expired and the refresh attempt failed.
    #[error("token expired and refresh failed: {0}")]
    RefreshFailed(String),
    /// No credential is available (and storage returned no entry).
    #[error("no credential available")]
    Unavailable,
}

/// Caches the Anthropic API key in process memory and refreshes from
/// [`SecureStorage`] on TTL expiry.
///
/// The cache uses a `RwLock` for fast read-mostly access; the `refresh_lock`
/// is reserved for future OAuth-refresh serialization. `http` and `clock` are
/// captured at construction so the manager remains runtime-agnostic.
pub struct CredentialManager {
    storage: Arc<dyn SecureStorage>,
    clock: Arc<dyn Clock>,
    #[allow(dead_code)]
    http: Arc<dyn HttpTransport>,
    #[allow(dead_code)]
    refresh_lock: Mutex<()>,
    // caches are kept under RwLock; M1 only exposes api_key getter.
    api_key_cache: RwLock<Option<(Secret<String>, std::time::SystemTime)>>,
    api_key_ttl: Duration,
}

impl CredentialManager {
    /// Build a `CredentialManager` over the supplied platform implementations.
    ///
    /// The default API-key TTL is 300 seconds.
    #[must_use]
    pub fn new(
        storage: Arc<dyn SecureStorage>,
        clock: Arc<dyn Clock>,
        http: Arc<dyn HttpTransport>,
    ) -> Self {
        Self {
            storage,
            clock,
            http,
            refresh_lock: Mutex::new(()),
            api_key_cache: RwLock::new(None),
            api_key_ttl: Duration::from_secs(300),
        }
    }

    /// Returns the Anthropic API key, loading from [`SecureStorage`] on cache
    /// miss.
    ///
    /// Returns `Ok(None)` when no key is present in storage.
    pub async fn get_anthropic_api_key(&self) -> Result<Option<Secret<String>>, CredentialError> {
        // Fast path: cached and fresh.
        if let Some((s, cached_at)) = self.api_key_cache.read().await.as_ref() {
            if self.clock.elapsed_since(*cached_at) < self.api_key_ttl {
                return Ok(Some(Secret::new(s.expose_secret().clone())));
            }
        }
        // Slow path: load from storage.
        let raw = self.storage.retrieve("lingxi", "anthropic-api-key").await?;
        let Some(raw) = raw else {
            return Ok(None);
        };
        let s = String::from_utf8(raw.expose_secret_bytes().to_vec())
            .map_err(|_| CredentialError::Unavailable)?;
        let now = self.clock.now();
        let secret = Secret::new(s.clone());
        *self.api_key_cache.write().await = Some((Secret::new(s), now));
        Ok(Some(secret))
    }

    /// Persist the Anthropic API key in [`SecureStorage`] and invalidate the
    /// in-memory cache.
    pub async fn store_anthropic_api_key(&self, key: &str) -> Result<(), CredentialError> {
        let metadata = SecureStorageMetadata {
            created_at: self.clock.now(),
            last_accessed: None,
            kind: SecretKind::AnthropicApiKey.as_dto(),
        };
        let data = SecureStorageData::new(key.as_bytes().to_vec(), metadata);
        self.storage
            .store("lingxi", "anthropic-api-key", data)
            .await?;
        // invalidate cache
        *self.api_key_cache.write().await = None;
        Ok(())
    }

    /// Persist a per-provider API key (or bearer token) in [`SecureStorage`],
    /// keyed by credential `id`. Stored under the shared `service = "lingxi"`
    /// keychain at an account namespaced by `id` (`provider-key-<id>`), labelled
    /// with [`SecretKind::GenericApiKey`]. Overwrites any existing entry for `id`
    /// so re-running `/connect` rotates the key. Not cached: the composite reads
    /// the keychain live so a freshly connected key takes effect on the next
    /// request without a restart.
    pub async fn set_provider_key(&self, id: &str, secret: &str) -> Result<(), CredentialError> {
        let metadata = SecureStorageMetadata {
            created_at: self.clock.now(),
            last_accessed: None,
            kind: SecretKind::GenericApiKey {
                provider: id.to_string(),
            }
            .as_dto(),
        };
        let data = SecureStorageData::new(secret.as_bytes().to_vec(), metadata);
        self.storage
            .store("lingxi", &provider_key_account(id), data)
            .await?;
        Ok(())
    }

    /// Load the per-provider key stored under credential `id`. Returns `Ok(None)`
    /// when no key has been stored (the composite then falls through to env, then
    /// `Authentication`).
    pub async fn get_provider_key(
        &self,
        id: &str,
    ) -> Result<Option<Secret<String>>, CredentialError> {
        let Some(raw) = self
            .storage
            .retrieve("lingxi", &provider_key_account(id))
            .await?
        else {
            return Ok(None);
        };
        let s = String::from_utf8(raw.expose_secret_bytes().to_vec())
            .map_err(|_| CredentialError::Unavailable)?;
        Ok(Some(Secret::new(s)))
    }

    /// Persist a full Anthropic OAuth credential set.
    ///
    /// Writes three secure-storage entries under `service = "lingxi"`:
    /// - `anthropic-oauth-access`  — the access token (`AnthropicOAuthAccessToken`)
    /// - `anthropic-oauth-refresh` — the refresh token (`AnthropicOAuthRefreshToken`),
    ///   deleted if `refresh` is `None`
    /// - `anthropic-oauth-meta`    — JSON session metadata (email / org / expiry / scopes)
    ///
    /// Each `store` overwrites any existing entry, so this is also the rotation
    /// path used by the reactive / proactive refresh driver.
    #[allow(clippy::too_many_arguments)]
    pub async fn store_oauth_tokens(
        &self,
        access: &str,
        refresh: Option<&str>,
        expires_at: SystemTime,
        scopes: Vec<String>,
        email: &str,
        org_id: &str,
    ) -> Result<(), CredentialError> {
        let now = self.clock.now();

        let access_meta = SecureStorageMetadata {
            created_at: now,
            last_accessed: None,
            kind: SecretKind::AnthropicOAuthAccessToken.as_dto(),
        };
        self.storage
            .store(
                OAUTH_SERVICE,
                OAUTH_ACCESS_ACCOUNT,
                SecureStorageData::new(access.as_bytes().to_vec(), access_meta),
            )
            .await?;

        match refresh {
            Some(refresh) => {
                let refresh_meta = SecureStorageMetadata {
                    created_at: now,
                    last_accessed: None,
                    kind: SecretKind::AnthropicOAuthRefreshToken.as_dto(),
                };
                self.storage
                    .store(
                        OAUTH_SERVICE,
                        OAUTH_REFRESH_ACCOUNT,
                        SecureStorageData::new(refresh.as_bytes().to_vec(), refresh_meta),
                    )
                    .await?;
            }
            None => {
                // No refresh token this rotation — clear any stale entry.
                self.storage
                    .delete(OAUTH_SERVICE, OAUTH_REFRESH_ACCOUNT)
                    .await?;
            }
        }

        let meta = OAuthSessionMeta {
            expires_at,
            scopes,
            email: email.to_string(),
            org_id: org_id.to_string(),
        };
        // Serialization of this fixed-shape struct cannot fail; fall back to an
        // empty object rather than panicking.
        let meta_json = serde_json::to_vec(&meta).unwrap_or_else(|_| b"{}".to_vec());
        let meta_meta = SecureStorageMetadata {
            created_at: now,
            last_accessed: None,
            kind: SecretKind::AnthropicOAuthSessionMeta.as_dto(),
        };
        self.storage
            .store(
                OAUTH_SERVICE,
                OAUTH_META_ACCOUNT,
                SecureStorageData::new(meta_json, meta_meta),
            )
            .await?;
        Ok(())
    }

    /// Load the persisted Anthropic OAuth credential set.
    ///
    /// Returns `Ok(None)` if no access token or no session metadata is present
    /// (a partially-written state is treated as "not logged in").
    pub async fn get_oauth_tokens(&self) -> Result<Option<OAuthTokens>, CredentialError> {
        let Some(access_raw) = self
            .storage
            .retrieve(OAUTH_SERVICE, OAUTH_ACCESS_ACCOUNT)
            .await?
        else {
            return Ok(None);
        };
        let Some(meta_raw) = self
            .storage
            .retrieve(OAUTH_SERVICE, OAUTH_META_ACCOUNT)
            .await?
        else {
            return Ok(None);
        };

        let access = String::from_utf8(access_raw.expose_secret_bytes().to_vec())
            .map_err(|_| CredentialError::Unavailable)?;
        let meta: OAuthSessionMeta = serde_json::from_slice(meta_raw.expose_secret_bytes())
            .map_err(|_| CredentialError::Unavailable)?;

        let refresh = match self
            .storage
            .retrieve(OAUTH_SERVICE, OAUTH_REFRESH_ACCOUNT)
            .await?
        {
            Some(raw) => Some(Secret::new(
                String::from_utf8(raw.expose_secret_bytes().to_vec())
                    .map_err(|_| CredentialError::Unavailable)?,
            )),
            None => None,
        };

        Ok(Some(OAuthTokens {
            access_token: Secret::new(access),
            refresh_token: refresh,
            expires_at: meta.expires_at,
            scopes: meta.scopes,
            email: meta.email,
            org_id: meta.org_id,
        }))
    }

    /// Delete every persisted Anthropic OAuth entry. Idempotent — deleting a
    /// missing entry is not an error.
    pub async fn delete_oauth_tokens(&self) -> Result<(), CredentialError> {
        self.storage
            .delete(OAUTH_SERVICE, OAUTH_ACCESS_ACCOUNT)
            .await?;
        self.storage
            .delete(OAUTH_SERVICE, OAUTH_REFRESH_ACCOUNT)
            .await?;
        self.storage
            .delete(OAUTH_SERVICE, OAUTH_META_ACCOUNT)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod oauth_tests {
    use super::*;
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::sync::Mutex as StdMutex;
    use traits::SecureStorageBackend;

    /// In-memory `(service, account) -> data` store for credential tests.
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

    struct FixedClock;
    impl Clock for FixedClock {
        fn now(&self) -> SystemTime {
            SystemTime::UNIX_EPOCH + Duration::from_secs(1_000)
        }
    }

    /// HTTP transport that panics — credential tests never make HTTP calls.
    struct NoHttp;
    #[async_trait]
    impl HttpTransport for NoHttp {
        async fn request(
            &self,
            _req: protocol::HttpRequest,
        ) -> Result<protocol::HttpResponse, traits::HttpError> {
            panic!("credential tests must not perform HTTP");
        }
        async fn stream_sse(
            &self,
            _req: protocol::HttpRequest,
        ) -> Result<traits::http::SseStream, traits::HttpError> {
            panic!("credential tests must not perform HTTP");
        }
    }

    fn manager() -> (Arc<MemStorage>, CredentialManager) {
        let storage = Arc::new(MemStorage::default());
        let cm = CredentialManager::new(
            storage.clone() as Arc<dyn SecureStorage>,
            Arc::new(FixedClock),
            Arc::new(NoHttp),
        );
        (storage, cm)
    }

    #[tokio::test]
    async fn oauth_tokens_round_trip() {
        let (_storage, cm) = manager();
        let expires = SystemTime::UNIX_EPOCH + Duration::from_secs(5_000);
        cm.store_oauth_tokens(
            "access-abc",
            Some("refresh-xyz"),
            expires,
            vec!["read:user".into(), "write:messages".into()],
            "user@example.com",
            "org-uuid-1",
        )
        .await
        .expect("store");

        let got = cm.get_oauth_tokens().await.expect("get").expect("present");
        assert_eq!(got.access_token.expose_secret(), "access-abc");
        assert_eq!(
            got.refresh_token.as_ref().map(|s| s.expose_secret().clone()),
            Some("refresh-xyz".to_string())
        );
        assert_eq!(got.expires_at, expires);
        assert_eq!(got.scopes, vec!["read:user", "write:messages"]);
        assert_eq!(got.email, "user@example.com");
        assert_eq!(got.org_id, "org-uuid-1");
    }

    #[tokio::test]
    async fn get_returns_none_when_absent() {
        let (_storage, cm) = manager();
        assert!(cm.get_oauth_tokens().await.expect("get").is_none());
    }

    #[tokio::test]
    async fn store_without_refresh_clears_stale_refresh() {
        let (_storage, cm) = manager();
        let expires = SystemTime::UNIX_EPOCH + Duration::from_secs(5_000);
        // First write a refresh token.
        cm.store_oauth_tokens("a1", Some("r1"), expires, vec![], "e@x", "o")
            .await
            .expect("store with refresh");
        // Rotate to a token set with no refresh token.
        cm.store_oauth_tokens("a2", None, expires, vec![], "e@x", "o")
            .await
            .expect("store without refresh");
        let got = cm.get_oauth_tokens().await.expect("get").expect("present");
        assert_eq!(got.access_token.expose_secret(), "a2");
        assert!(got.refresh_token.is_none(), "stale refresh must be cleared");
    }

    #[tokio::test]
    async fn delete_is_idempotent_and_clears() {
        let (_storage, cm) = manager();
        let expires = SystemTime::UNIX_EPOCH + Duration::from_secs(5_000);
        cm.store_oauth_tokens("a", Some("r"), expires, vec![], "e@x", "o")
            .await
            .expect("store");
        cm.delete_oauth_tokens().await.expect("delete");
        assert!(cm.get_oauth_tokens().await.expect("get").is_none());
        // Second delete on an empty store is not an error.
        cm.delete_oauth_tokens().await.expect("idempotent delete");
    }

    // ── Per-provider key round-trips (catalog/provider-config support) ──────

    #[tokio::test]
    async fn provider_key_round_trips() {
        let (_storage, cm) = manager();
        cm.set_provider_key("openrouter", "sk-or-secret").await.expect("set");
        let got = cm.get_provider_key("openrouter").await.expect("get").expect("present");
        assert_eq!(got.expose_secret(), "sk-or-secret");
    }

    #[tokio::test]
    async fn get_provider_key_returns_none_when_absent() {
        let (_storage, cm) = manager();
        assert!(cm.get_provider_key("deepseek").await.expect("get").is_none());
    }

    #[tokio::test]
    async fn set_provider_key_overwrites_previous() {
        let (_storage, cm) = manager();
        cm.set_provider_key("glm-coding", "old-key").await.expect("first set");
        cm.set_provider_key("glm-coding", "new-key").await.expect("second set");
        let got = cm.get_provider_key("glm-coding").await.expect("get").expect("present");
        assert_eq!(got.expose_secret(), "new-key");
    }

    #[tokio::test]
    async fn provider_keys_are_isolated_by_id() {
        let (_storage, cm) = manager();
        cm.set_provider_key("openrouter", "key-a").await.expect("set a");
        cm.set_provider_key("deepseek", "key-b").await.expect("set b");
        assert_eq!(
            cm.get_provider_key("openrouter").await.expect("get a").expect("present a").expose_secret(),
            "key-a"
        );
        assert_eq!(
            cm.get_provider_key("deepseek").await.expect("get b").expect("present b").expose_secret(),
            "key-b"
        );
    }

    #[tokio::test]
    async fn provider_key_persisted_under_lingxi_service_with_generic_kind() {
        let (storage, cm) = manager();
        cm.set_provider_key("github-copilot", "ghu_token").await.expect("set");
        let raw = storage
            .retrieve("lingxi", "provider-key-github-copilot")
            .await
            .expect("retrieve")
            .expect("present");
        assert_eq!(raw.expose_secret_bytes(), b"ghu_token");
        let expected_kind = SecretKind::GenericApiKey {
            provider: "github-copilot".to_string(),
        }
        .as_dto();
        assert_eq!(raw.metadata.kind, expected_kind);
    }
}

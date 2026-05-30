//! Cached credential manager for the Anthropic API key.
//!
//! Wraps the platform [`SecureStorage`] behind a small in-memory cache so the
//! engine's hot path does not touch the OS keychain on every request. M1
//! exposes only the Anthropic API key; OAuth access/refresh tokens land in a
//! follow-up task.

use protocol::{Secret, SecureStorageData, SecureStorageMetadata};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::{Mutex, RwLock};
use traits::{Clock, HttpTransport, SecureStorage, SecureStorageError};

use crate::kinds::SecretKind;

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
}

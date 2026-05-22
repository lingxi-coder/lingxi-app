//! Platform secure-storage abstraction. Engine code stores OAuth tokens, API
//! keys, and other long-lived secrets via this trait; platform crates wrap the
//! OS keychain / credential vault while host-test builds use an in-memory or
//! encrypted-file fallback. See spec §6 (Secrets) and D17 (Runtime boundary).

use async_trait::async_trait;
use lingxi_protocol::SecureStorageData;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Persistent secure store for opaque secret payloads keyed by
/// `(service, account)`.
///
/// Implementations live in platform crates and front the OS-native credential
/// store. Engine code receives an `Arc<dyn SecureStorage>` and never touches a
/// concrete backend directly.
#[async_trait]
pub trait SecureStorage: Send + Sync {
    /// Persist `data` under the `(service, account)` key, overwriting any
    /// existing entry.
    async fn store(
        &self,
        service: &str,
        account: &str,
        data: SecureStorageData,
    ) -> Result<(), SecureStorageError>;

    /// Look up the entry for `(service, account)`. Returns `Ok(None)` when no
    /// such entry exists (distinguishing a missing key from a backend error).
    async fn retrieve(
        &self,
        service: &str,
        account: &str,
    ) -> Result<Option<SecureStorageData>, SecureStorageError>;

    /// Remove the entry for `(service, account)`. Removing a non-existent entry
    /// is not an error.
    async fn delete(&self, service: &str, account: &str) -> Result<(), SecureStorageError>;

    /// List every `account` currently stored under `service`.
    async fn list(&self, service: &str) -> Result<Vec<String>, SecureStorageError>;

    /// True when the underlying backend encrypts secrets at rest (e.g. an OS
    /// keychain or an encrypted file). False for the plaintext development
    /// fallback.
    fn is_encrypted(&self) -> bool;

    /// Concrete backend in use. Useful for logging, telemetry, and user-facing
    /// "where are my secrets stored?" diagnostics.
    fn backend(&self) -> SecureStorageBackend;
}

/// Identifies the concrete secure-storage backend in use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SecureStorageBackend {
    /// macOS Keychain Services.
    MacOsKeychain,
    /// Linux Secret Service via `libsecret` (`GNOME` Keyring, `KWallet`, ...).
    LinuxLibsecret,
    /// Windows Credential Vault (`CredRead`/`CredWrite`).
    WindowsCredVault,
    /// Android `Keystore` system.
    AndroidKeystore,
    /// iOS Keychain Services.
    IosKeychain,
    /// File-backed store with at-rest encryption (development / Linux fallback).
    EncryptedFile,
    /// File-backed store without encryption. Development only.
    PlainText,
}

/// Failure modes for [`SecureStorage`] calls.
#[derive(Debug, Clone, Error)]
pub enum SecureStorageError {
    /// No entry exists for the requested `(service, account)` pair.
    #[error("not found: {service}/{account}")]
    NotFound {
        /// Service identifier that was queried.
        service: String,
        /// Account identifier that was queried.
        account: String,
    },
    /// Caller lacks the OS- or sandbox-level permission required for the
    /// operation (e.g. keychain unlock prompt was declined).
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    /// Backend is reachable in principle but currently unusable (locked
    /// keychain, missing daemon, etc.).
    #[error("backend unavailable: {0}")]
    BackendUnavailable(String),
    /// Catch-all for underlying I/O failures.
    #[error("io error: {0}")]
    Io(String),
}

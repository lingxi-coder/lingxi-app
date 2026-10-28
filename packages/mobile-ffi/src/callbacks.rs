//! Shared conversions for native share and secure-storage callbacks.

use super::{PlatformSecureStorage, PlatformShare, PLATFORM_SECURE_STORAGE_BACKEND};

/// FFI error surface for the native share callback interface. A flat enum so `UniFFI`
/// can render it for an async `callback_interface` method; the bridge fans it
/// back out onto the richer [`lingxi_core::host::ShareError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Error))]
#[derive(Debug, thiserror::Error)]
pub enum ShareFfiError {
    /// Sharing is unsupported on this device / for this payload.
    #[error("sharing unsupported")]
    Unsupported,
    /// Any other native failure.
    #[error("share error: {message}")]
    Other {
        /// Human-readable detail from the native side.
        message: String,
    },
}

/// FFI carrier for the outcome of a native share — whether the user completed
/// or dismissed the system share sheet. Mapped to [`lingxi_core::host::ShareResult`].
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[derive(Debug, Clone)]
pub enum ShareResultFfi {
    /// The user completed the share (chose a target app).
    Success,
    /// The user dismissed the share sheet without sharing.
    Cancelled,
}

/// Adapts the crate-local [`PlatformShare`] callback interface to the shared
/// [`lingxi_core::host::SharingService`] seam the engine consumes. Destructures
/// [`lingxi_core::host::SharePayload`] into the flat `text` / `url` / `image_bytes` args
/// and fans [`ShareResultFfi`] / [`ShareFfiError`] back out onto
/// [`lingxi_core::host::ShareResult`] / [`lingxi_core::host::ShareError`].
#[cfg(feature = "uniffi")]
pub(crate) struct ShareBridge {
    pub(crate) inner: Box<dyn PlatformShare>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl lingxi_core::host::SharingService for ShareBridge {
    async fn share(
        &self,
        payload: lingxi_core::host::SharePayload,
    ) -> Result<lingxi_core::host::ShareResult, lingxi_core::host::ShareError> {
        let lingxi_core::host::SharePayload {
            text,
            url,
            image_bytes,
        } = payload;
        match self.inner.share(text, url, image_bytes).await {
            Ok(ShareResultFfi::Success) => Ok(lingxi_core::host::ShareResult::Success),
            Ok(ShareResultFfi::Cancelled) => Ok(lingxi_core::host::ShareResult::Cancelled),
            Err(ShareFfiError::Unsupported) => Err(lingxi_core::host::ShareError::Unsupported),
            Err(ShareFfiError::Other { message }) => {
                Err(lingxi_core::host::ShareError::Other(message))
            }
        }
    }
}

/// FFI error surface for the native secure-storage callback interface. A flat enum
/// so `UniFFI` can render it for an async `callback_interface` method; the bridge
/// fans it back out onto the richer [`lingxi_core::host::SecureStorageError`].
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Error))]
#[derive(Debug, thiserror::Error)]
pub enum SecureStorageFfiError {
    /// The OS denied access (e.g. user authentication or device unlock is required).
    #[error("secure storage permission denied: {message}")]
    PermissionDenied {
        /// Human-readable detail from the native side.
        message: String,
    },
    /// The secure storage backend is currently unusable.
    #[error("secure storage backend unavailable: {message}")]
    BackendUnavailable {
        /// Human-readable detail from the native side.
        message: String,
    },
    /// Any other native failure.
    #[error("secure storage io error: {message}")]
    Io {
        /// Human-readable detail from the native side.
        message: String,
    },
}

/// Adapts the crate-local [`PlatformSecureStorage`] (opaque-blob FFI) to the shared
/// [`lingxi_core::host::SecureStorage`] seam: serde-encodes `SecureStorageData` to a blob on
/// store, decodes on retrieve, and reports the platform backend as encrypted.
#[cfg(feature = "uniffi")]
pub(crate) struct SecureStorageBridge {
    pub(crate) inner: Box<dyn PlatformSecureStorage>,
}

#[cfg(feature = "uniffi")]
#[async_trait::async_trait]
impl lingxi_core::host::SecureStorage for SecureStorageBridge {
    async fn store(
        &self,
        service: &str,
        account: &str,
        data: lingxi_core::types::SecureStorageData,
    ) -> Result<(), lingxi_core::host::SecureStorageError> {
        let blob = serde_json::to_vec(&data)
            .map_err(|e| lingxi_core::host::SecureStorageError::Io(format!("serialize: {e}")))?;
        self.inner
            .store(service.to_string(), account.to_string(), blob)
            .await
            .map_err(securestorage_error_from_ffi)
    }
    async fn retrieve(
        &self,
        service: &str,
        account: &str,
    ) -> Result<Option<lingxi_core::types::SecureStorageData>, lingxi_core::host::SecureStorageError>
    {
        match self
            .inner
            .retrieve(service.to_string(), account.to_string())
            .await
            .map_err(securestorage_error_from_ffi)?
        {
            Some(blob) => {
                let data = serde_json::from_slice(&blob).map_err(|e| {
                    lingxi_core::host::SecureStorageError::Io(format!("deserialize: {e}"))
                })?;
                Ok(Some(data))
            }
            None => Ok(None),
        }
    }
    async fn delete(
        &self,
        service: &str,
        account: &str,
    ) -> Result<(), lingxi_core::host::SecureStorageError> {
        self.inner
            .delete(service.to_string(), account.to_string())
            .await
            .map_err(securestorage_error_from_ffi)
    }
    async fn list(
        &self,
        service: &str,
    ) -> Result<Vec<String>, lingxi_core::host::SecureStorageError> {
        self.inner
            .list(service.to_string())
            .await
            .map_err(securestorage_error_from_ffi)
    }
    fn is_encrypted(&self) -> bool {
        true
    }
    fn backend(&self) -> lingxi_core::host::SecureStorageBackend {
        PLATFORM_SECURE_STORAGE_BACKEND
    }
}

/// Fan a flat [`SecureStorageFfiError`] back out onto [`lingxi_core::host::SecureStorageError`].
#[cfg(feature = "uniffi")]
fn securestorage_error_from_ffi(e: SecureStorageFfiError) -> lingxi_core::host::SecureStorageError {
    match e {
        SecureStorageFfiError::PermissionDenied { message } => {
            lingxi_core::host::SecureStorageError::PermissionDenied(message)
        }
        SecureStorageFfiError::BackendUnavailable { message } => {
            lingxi_core::host::SecureStorageError::BackendUnavailable(message)
        }
        SecureStorageFfiError::Io { message } => lingxi_core::host::SecureStorageError::Io(message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lingxi_core::host::{SecureStorage, SecureStorageError};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    struct MalformedStorage {
        writes: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl PlatformSecureStorage for MalformedStorage {
        async fn store(
            &self,
            _service: String,
            _account: String,
            _blob: Vec<u8>,
        ) -> Result<(), SecureStorageFfiError> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn retrieve(
            &self,
            _service: String,
            _account: String,
        ) -> Result<Option<Vec<u8>>, SecureStorageFfiError> {
            Ok(Some(b"not valid JSON".to_vec()))
        }
        async fn delete(
            &self,
            _service: String,
            _account: String,
        ) -> Result<(), SecureStorageFfiError> {
            Ok(())
        }
        async fn list(&self, _service: String) -> Result<Vec<String>, SecureStorageFfiError> {
            Ok(Vec::new())
        }
    }

    #[tokio::test]
    async fn malformed_secret_remains_untouched_and_reports_io_error() {
        let writes = Arc::new(AtomicUsize::new(0));
        let storage = SecureStorageBridge {
            inner: Box::new(MalformedStorage {
                writes: writes.clone(),
            }),
        };
        let error = storage.retrieve("service", "account").await.unwrap_err();
        assert!(
            matches!(error, SecureStorageError::Io(message) if message.starts_with("deserialize: "))
        );
        assert_eq!(writes.load(Ordering::SeqCst), 0);
        assert!(storage.is_encrypted());
        assert_eq!(storage.backend(), PLATFORM_SECURE_STORAGE_BACKEND);
    }
}

//! Pre-warm the macOS keychain access prompt at startup so it doesn't
//! interrupt the first interactive moment.
//!
//! Spawns a background task that immediately retrieves the Anthropic API key
//! from [`SecureStorage`]; the first OS-level prompt happens during that
//! warm-up rather than during the user's first request. The eventual result
//! is delivered through a `oneshot` channel that callers can drain via
//! [`KeychainPrefetch::consume`].

use lingxi_protocol::SecureStorageData;
use lingxi_traits::{RuntimeError, RuntimeSpawner, SecureStorage, SecureStorageError};
use std::sync::Arc;
use tokio::sync::oneshot;

/// Result delivered by the background prefetch task.
type PrefetchResult = Result<Option<SecureStorageData>, SecureStorageError>;

/// Handle to the background keychain warm-up task.
pub struct KeychainPrefetch {
    rx: tokio::sync::Mutex<Option<oneshot::Receiver<PrefetchResult>>>,
}

impl KeychainPrefetch {
    /// Spawn the prefetch task on `runtime` and return a handle whose
    /// [`Self::consume`] yields the eventual storage result.
    ///
    /// The returned `BackgroundTaskHandle` from the spawner is intentionally
    /// dropped — prefetch is fire-and-forget and is not cancelled explicitly.
    pub async fn start(
        storage: Arc<dyn SecureStorage>,
        runtime: &dyn RuntimeSpawner,
    ) -> Result<Self, RuntimeError> {
        let (tx, rx) = oneshot::channel();
        runtime
            .spawn(
                "keychain-prefetch",
                Box::pin(async move {
                    let _ = tx.send(storage.retrieve("lingxi", "anthropic-api-key").await);
                }),
            )
            .await?;
        Ok(Self {
            rx: tokio::sync::Mutex::new(Some(rx)),
        })
    }

    /// Consume the prefetched storage result, if available.
    ///
    /// Returns `None` if the result has already been consumed or if the
    /// background task panicked / was cancelled before sending.
    pub async fn consume(&self) -> Option<PrefetchResult> {
        let rx = self.rx.lock().await.take()?;
        rx.await.ok()
    }
}

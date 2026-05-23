//! `OpenFileTracker` — server-aware dedup of `textDocument/didOpen`.
//!
//! Many LSP servers return empty results from requests like
//! `textDocument/hover` unless the document has been opened via
//! `textDocument/didOpen` first. claude-code's `LSPServerManager`
//! (`claude-code/src/services/lsp/LSPServerManager.ts:64,277`) tracks open
//! files in a `Map<string, string>` keyed by URI. We key the *tuple*
//! `(server_name, uri)` so the same URI can be tracked independently on
//! multiple servers (multi-LSP setups).

use lsp_types::Url;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Thread-safe `(server_name, uri)` open-file set.
#[derive(Debug, Default, Clone)]
pub struct OpenFileTracker {
    inner: Arc<RwLock<HashSet<(String, Url)>>>,
}

impl OpenFileTracker {
    /// Empty tracker.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// `true` when `uri` has been opened on `server_name`.
    pub async fn is_open(&self, server_name: &str, uri: &Url) -> bool {
        let key = (server_name.to_string(), uri.clone());
        self.inner.read().await.contains(&key)
    }

    /// Mark `uri` as open on `server_name`. Idempotent.
    pub async fn mark_open(&self, server_name: &str, uri: Url) {
        self.inner
            .write()
            .await
            .insert((server_name.to_string(), uri));
    }

    /// Remove the open-file tracking for one specific `(server, uri)` pair.
    pub async fn clear(&self, server_name: &str, uri: &Url) {
        let key = (server_name.to_string(), uri.clone());
        self.inner.write().await.remove(&key);
    }

    /// Remove every open-file entry for `server_name` (e.g. when the
    /// server is shutting down or restarting). Returns the number of
    /// entries dropped.
    pub async fn clear_server(&self, server_name: &str) -> usize {
        let mut guard = self.inner.write().await;
        let before = guard.len();
        guard.retain(|(s, _)| s != server_name);
        before - guard.len()
    }

    /// Total entries (across all servers); for diagnostics / tests.
    pub async fn len(&self) -> usize {
        self.inner.read().await.len()
    }

    /// Whether the tracker is empty.
    pub async fn is_empty(&self) -> bool {
        self.inner.read().await.is_empty()
    }
}

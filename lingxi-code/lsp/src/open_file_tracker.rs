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
use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Maximum number of open documents retained per registry, matching Claude Code
/// 2.1.208's didOpen LRU cap.
pub const MAX_OPEN_DOCUMENTS: usize = 50;

#[derive(Debug, Default)]
struct OpenFileTrackerInner {
    open: HashSet<(String, Url)>,
    order: VecDeque<(String, Url)>,
}

/// Thread-safe `(server_name, uri)` open-file set.
#[derive(Debug, Default, Clone)]
pub struct OpenFileTracker {
    inner: Arc<RwLock<OpenFileTrackerInner>>,
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
        self.inner.read().await.open.contains(&key)
    }

    /// Mark `uri` as open on `server_name`. Idempotent.
    ///
    /// Returns entries evicted by the 50-document LRU cap; callers should send
    /// `textDocument/didClose` for each returned URI.
    pub async fn mark_open(&self, server_name: &str, uri: Url) -> Vec<(String, Url)> {
        let key = (server_name.to_string(), uri);
        let mut guard = self.inner.write().await;
        if guard.open.contains(&key) {
            guard.order.retain(|existing| existing != &key);
            guard.order.push_back(key);
            return Vec::new();
        }

        guard.open.insert(key.clone());
        guard.order.push_back(key);
        let mut evicted = Vec::new();
        while guard.open.len() > MAX_OPEN_DOCUMENTS {
            let Some(oldest) = guard.order.pop_front() else {
                break;
            };
            if guard.open.remove(&oldest) {
                evicted.push(oldest);
            }
        }
        evicted
    }

    /// Remove the open-file tracking for one specific `(server, uri)` pair.
    pub async fn clear(&self, server_name: &str, uri: &Url) {
        let key = (server_name.to_string(), uri.clone());
        let mut guard = self.inner.write().await;
        guard.open.remove(&key);
        guard.order.retain(|existing| existing != &key);
    }

    /// Remove every open-file entry for `server_name` (e.g. when the
    /// server is shutting down or restarting). Returns the number of
    /// entries dropped.
    pub async fn clear_server(&self, server_name: &str) -> usize {
        let mut guard = self.inner.write().await;
        let before = guard.open.len();
        guard.open.retain(|(s, _)| s != server_name);
        guard.order.retain(|(s, _)| s != server_name);
        before - guard.open.len()
    }

    /// Total entries (across all servers); for diagnostics / tests.
    pub async fn len(&self) -> usize {
        self.inner.read().await.open.len()
    }

    /// Whether the tracker is empty.
    pub async fn is_empty(&self) -> bool {
        self.inner.read().await.open.is_empty()
    }
}

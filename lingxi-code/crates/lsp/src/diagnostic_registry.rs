//! `LspDiagnosticRegistry` — accumulates `textDocument/publishDiagnostics`
//! notifications keyed by document URI.
//!
//! Mirrors the "latest snapshot wins" semantics of LSP: each
//! `publishDiagnostics` notification fully replaces the previous diagnostic
//! list for that URI. We store the optional `version` per LSP spec so the
//! consumer can ignore stale snapshots if the server happens to interleave
//! them with edits.

use lsp_types::{Diagnostic, Url};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// One snapshot for one document.
#[derive(Debug, Clone)]
pub struct DiagnosticEntry {
    /// Optional version (LSP `PublishDiagnosticsParams.version`). `None`
    /// when the server does not track versions.
    pub version: Option<i32>,
    /// Diagnostics list, in server-provided order.
    pub diagnostics: Vec<Diagnostic>,
}

/// Thread-safe map from file URI to latest diagnostic snapshot.
#[derive(Debug, Default, Clone)]
pub struct LspDiagnosticRegistry {
    inner: Arc<RwLock<HashMap<Url, DiagnosticEntry>>>,
}

impl LspDiagnosticRegistry {
    /// Empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the diagnostics for `uri` with `entry`.
    ///
    /// If `entry.version` is `Some` and an existing entry has a strictly
    /// greater version, the update is dropped (out-of-order delivery).
    pub async fn publish(&self, uri: Url, entry: DiagnosticEntry) {
        let mut guard = self.inner.write().await;
        if let (Some(new_v), Some(existing)) = (entry.version, guard.get(&uri)) {
            if let Some(existing_v) = existing.version {
                if existing_v > new_v {
                    return; // stale
                }
            }
        }
        guard.insert(uri, entry);
    }

    /// Snapshot the diagnostics for `uri` (empty if none).
    pub async fn get(&self, uri: &Url) -> Vec<Diagnostic> {
        self.inner
            .read()
            .await
            .get(uri)
            .map(|e| e.diagnostics.clone())
            .unwrap_or_default()
    }

    /// Clear any diagnostics for `uri`. Idempotent.
    pub async fn clear(&self, uri: &Url) {
        self.inner.write().await.remove(uri);
    }

    /// Snapshot every URI's diagnostics. Order is unspecified.
    pub async fn all_diagnostics(&self) -> Vec<(Url, Vec<Diagnostic>)> {
        self.inner
            .read()
            .await
            .iter()
            .map(|(uri, entry)| (uri.clone(), entry.diagnostics.clone()))
            .collect()
    }

    /// Total number of URIs tracked.
    pub async fn len(&self) -> usize {
        self.inner.read().await.len()
    }

    /// Whether the registry is empty.
    pub async fn is_empty(&self) -> bool {
        self.inner.read().await.is_empty()
    }
}

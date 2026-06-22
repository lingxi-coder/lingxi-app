//! `LspDiagnosticRegistry` — accumulates `textDocument/publishDiagnostics`
//! notifications keyed by document URI.
//!
//! Mirrors the "latest snapshot wins" semantics of LSP: each
//! `publishDiagnostics` notification fully replaces the previous diagnostic
//! list for that URI. We store the optional `version` per LSP spec so the
//! consumer can ignore stale snapshots if the server happens to interleave
//! them with edits.

use crate::diagnostics_format::{dedup_key, format_diagnostics_block, DiagnosticFile};
use lsp_types::{Diagnostic, Url};
use std::collections::{HashMap, HashSet};
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
///
/// In addition to the latest snapshot, the registry tracks a per-URI set of
/// already-surfaced diagnostic keys (claude-code `ike`) so the passive
/// `<new-diagnostics>` reminder shows each distinct diagnostic to the model at
/// most once — [`Self::take_new_diagnostics_block`].
#[derive(Debug, Default, Clone)]
pub struct LspDiagnosticRegistry {
    inner: Arc<RwLock<HashMap<Url, DiagnosticEntry>>>,
    /// Per-URI set of [`dedup_key`]s already surfaced to the model (`ike`).
    sent: Arc<RwLock<HashMap<Url, HashSet<String>>>>,
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

    /// Compute the passive `<new-diagnostics>` reminder for any diagnostics not
    /// yet surfaced to the model, marking them surfaced. Returns `None` when no
    /// new diagnostics exist (the byte-faithful "nothing to inject" case).
    ///
    /// Mirrors claude-code's `zqd` dedup + `formatDiagnosticsBlock`: for every
    /// URI with diagnostics, each diagnostic's [`dedup_key`] is checked against
    /// the per-URI sent set (`ike`); only NEW keys are included (and then added
    /// to the set, so a subsequent call won't re-surface them). Files with no
    /// new diagnostics are skipped entirely. URIs are visited in sorted order so
    /// the block is deterministic.
    pub async fn take_new_diagnostics_block(&self) -> Option<String> {
        let snapshots = self.inner.read().await;
        let mut sent = self.sent.write().await;

        let mut uris: Vec<&Url> = snapshots.keys().collect();
        uris.sort_by(|a, b| a.as_str().cmp(b.as_str()));

        let mut files: Vec<DiagnosticFile> = Vec::new();
        for uri in uris {
            let Some(entry) = snapshots.get(uri) else {
                continue;
            };
            let seen = sent.entry(uri.clone()).or_default();
            let mut fresh: Vec<Diagnostic> = Vec::new();
            for d in &entry.diagnostics {
                let key = dedup_key(d);
                // `insert` returns false when the key was already present →
                // already surfaced, skip. New keys are recorded as surfaced.
                if seen.insert(key) {
                    fresh.push(d.clone());
                }
            }
            if !fresh.is_empty() {
                files.push(DiagnosticFile {
                    uri: uri.to_string(),
                    diagnostics: fresh,
                });
            }
        }

        if files.is_empty() {
            None
        } else {
            Some(format_diagnostics_block(&files))
        }
    }
}

#[cfg(test)]
mod new_diagnostics_tests {
    use super::*;
    use lsp_types::{DiagnosticSeverity, Position, Range};

    fn err(line: u32, msg: &str) -> Diagnostic {
        Diagnostic {
            range: Range::new(Position::new(line, 0), Position::new(line, 1)),
            severity: Some(DiagnosticSeverity::ERROR),
            message: msg.to_string(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn surfaces_new_once_then_skips_until_new_arrive() {
        let reg = LspDiagnosticRegistry::new();
        let uri = Url::parse("file:///repo/a.ts").unwrap();

        // Nothing yet.
        assert!(reg.take_new_diagnostics_block().await.is_none());

        // First publish → surfaced once.
        reg.publish(
            uri.clone(),
            DiagnosticEntry { version: None, diagnostics: vec![err(0, "e1")] },
        )
        .await;
        let block = reg.take_new_diagnostics_block().await.expect("a block");
        assert!(block.contains("<new-diagnostics>"));
        assert!(block.contains("a.ts:"));
        assert!(block.contains("[Line 1:1] e1"));

        // Second call, same snapshot → already surfaced → None.
        assert!(reg.take_new_diagnostics_block().await.is_none());

        // A new snapshot adding e2 (e1 still present) → only e2 surfaces.
        reg.publish(
            uri.clone(),
            DiagnosticEntry { version: None, diagnostics: vec![err(0, "e1"), err(1, "e2")] },
        )
        .await;
        let block = reg.take_new_diagnostics_block().await.expect("e2 block");
        assert!(block.contains("e2"), "got: {block}");
        assert!(!block.contains("e1"), "e1 already surfaced: {block}");
        assert!(reg.take_new_diagnostics_block().await.is_none());
    }
}

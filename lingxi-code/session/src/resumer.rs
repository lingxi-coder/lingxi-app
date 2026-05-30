//! Session resume orchestration (spec §22.5 / C7).
//!
//! `SessionResumer` ties [`SessionStorage`] (transcript + metadata) to the
//! file-state cache. M1.16 only restores the session bones; full re-attach
//! of plugins, MCP servers, permission state, cost ledger, and output style
//! is handled by the host wrapper in Plans 15 and 16.

use crate::storage::{LoadedSession, SessionStorage};
use lingxi_filestate::FileStateCache;
use lingxi_traits::FileSystem;
use std::sync::Arc;
use thiserror::Error;

/// Output of [`SessionResumer::resume`].
pub struct ResumedSession {
    /// Loaded metadata and transcript messages.
    pub loaded: LoadedSession,
    /// Fresh, empty file-state cache (we deliberately do not rehydrate from
    /// stale historical Reads — see B6).
    pub file_state_cache: FileStateCache,
}

/// Failure modes for [`SessionResumer::resume`].
#[derive(Debug, Clone, Error)]
pub enum ResumeError {
    /// Underlying storage error, stringified to keep `ResumeError` `Clone`.
    #[error("storage: {0}")]
    Storage(String),
}

/// Glue object that resumes a session from disk.
pub struct SessionResumer {
    storage: Arc<SessionStorage>,
    #[allow(dead_code)]
    fs: Arc<dyn FileSystem>,
}

impl SessionResumer {
    /// Construct a new resumer.
    #[must_use]
    pub fn new(storage: Arc<SessionStorage>, fs: Arc<dyn FileSystem>) -> Self {
        Self { storage, fs }
    }

    /// M1.16 restores the session bones; Plugin / MCP / Permission / Cost /
    /// `OutputStyle` re-attachment is done by the host wrapper in Plans 15
    /// and 16. This function returns the loaded transcript plus an empty
    /// `FileStateCache`.
    ///
    /// Note: `SessionResumer` deliberately does NOT rebuild `FileStateCache`
    /// from stale historical Reads (B6) — the cache restarts empty and the
    /// agent re-Reads any file it needs.
    pub async fn resume(
        &self,
        session_id: &lingxi_protocol::SessionId,
    ) -> Result<ResumedSession, ResumeError> {
        let loaded = self
            .storage
            .load(session_id)
            .await
            .map_err(|e| ResumeError::Storage(e.to_string()))?;
        let cache = FileStateCache::new(lingxi_filestate::MAX_ENTRIES, lingxi_filestate::MAX_BYTES);
        Ok(ResumedSession {
            loaded,
            file_state_cache: cache,
        })
    }
}

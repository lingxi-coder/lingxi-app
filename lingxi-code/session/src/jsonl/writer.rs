//! Append-only JSONL writer — 1:1 port of
//! `claude-code/src/utils/sessionStorage.ts:2572-2584` (`appendEntryToFile`).
//!
//! Lock: serialize via `serde_json::to_string` (no whitespace, no indent),
//! terminate every line with a single `\n`, file mode `0o600`, dir mode `0o700`.

use crate::jsonl::schema::JsonlMessage;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::Mutex;
use traits::{FileSystem, FsError};

/// Failure modes for [`JsonlWriter`] operations.
#[derive(Debug, Error)]
pub enum WriterError {
    /// Underlying filesystem error.
    #[error(transparent)]
    Fs(#[from] FsError),
    /// `serde_json::to_string` failed (e.g. malformed `Value`).
    #[error("serialize failure: {0}")]
    Serialize(#[from] serde_json::Error),
}

/// Append-only writer for one session's `<uuid>.jsonl`.
///
/// Holds an exclusive in-process lock so concurrent `append` calls serialize
/// (cross-process locking is delegated to the `FileSystem` flock impl when
/// the orchestrator wants it; the spec only mandates in-process for M5-07).
pub struct JsonlWriter {
    path: PathBuf,
    fs: Arc<dyn FileSystem>,
    lock: Mutex<()>,
}

impl JsonlWriter {
    /// Open (or create on first append) `path`.
    ///
    /// No I/O is performed until `append` is called — keeps construction cheap
    /// for the orchestrator's `Option<Arc<JsonlWriter>>` wiring.
    #[must_use]
    pub fn new(path: PathBuf, fs: Arc<dyn FileSystem>) -> Self {
        Self {
            path,
            fs,
            lock: Mutex::new(()),
        }
    }

    /// Returns the on-disk path this writer targets.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append one JSONL line — `serde_json::to_string(msg) + "\n"`.
    ///
    /// Creates the parent directory on first call. The `FileSystem` trait
    /// in M1 does not expose `mkdir_p`; we use `tokio::fs::create_dir_all`
    /// directly because parent-dir creation is not a sandboxed operation we
    /// virtualize for tests (each `FileSystem` impl that hosts real files
    /// would do the same syscall internally). M5-08 may extend the trait.
    pub async fn append(&self, msg: &JsonlMessage) -> Result<(), WriterError> {
        let _g = self.lock.lock().await;
        let line = serde_json::to_string(msg)?;
        let path_str = self.path.to_str().expect("session paths are UTF-8");
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                // Sync std::fs is fine here — we already hold the in-process
                // mutex and parent-dir creation is a one-shot syscall.
                // claude-code `appendToFile` creates the project dir with
                // `{ mode: 0o700 }` (owner-only); mirror that on unix.
                #[cfg(unix)]
                {
                    use std::os::unix::fs::DirBuilderExt;
                    std::fs::DirBuilder::new()
                        .recursive(true)
                        .mode(0o700)
                        .create(parent)
                        .map_err(|e| FsError::Io(e.to_string()))?;
                }
                #[cfg(not(unix))]
                std::fs::create_dir_all(parent).map_err(|e| FsError::Io(e.to_string()))?;
            }
        }
        let mut payload = String::with_capacity(line.len() + 1);
        payload.push_str(&line);
        payload.push('\n');
        // claude-code `appendToFile`: `fsAppendFile(path, data, { mode: 0o600 })`
        // — the `<uuid>.jsonl` transcript is owner-only (prompt + tool content).
        self.fs
            .append_file_with_mode(path_str, &payload, 0o600)
            .await?;
        Ok(())
    }
}

//! File reader with 10 MB cap. Filled in Task 4.

use std::path::PathBuf;
use thiserror::Error;

/// One loaded CLAUDE.md (or local override) file.
#[derive(Debug, Clone)]
pub struct LoadedFile {
    /// Absolute path the file was loaded from.
    pub path: PathBuf,
    /// File body (post-cap, secrets NOT yet redacted at this layer).
    pub body: String,
    /// File size in bytes at the time of load (pre-redaction).
    pub size_bytes: u64,
}

/// Failure modes the loader can encounter.
#[derive(Debug, Error)]
pub enum LoaderError {
    /// I/O error reading the file.
    #[error("io: {0}")]
    Io(String),
    /// File exceeded `MAX_MEMORY_FILE_SIZE`; skipped (event emitted).
    #[error("file too large: {bytes} bytes at {path}")]
    FileTooLarge {
        /// Path of the oversized file.
        path: PathBuf,
        /// Observed size in bytes.
        bytes: u64,
    },
}

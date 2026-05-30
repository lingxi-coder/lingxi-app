//! Sandboxed filesystem abstraction. Implementations live in platform crates
//! and enforce workspace-root containment (see spec §6 / D17).

use async_trait::async_trait;
use futures_core::stream::Stream;
use serde::{Deserialize, Serialize};
use std::pin::Pin;
use thiserror::Error;

/// Sandboxed read/write access to the workspace.
///
/// Engine and tool code receive an `Arc<dyn FileSystem>` rather than calling
/// `std::fs` directly so the host can sandbox, virtualize, or audit access.
#[async_trait]
pub trait FileSystem: Send + Sync {
    /// Read a UTF-8 file, optionally constrained to a byte `offset`/`limit`
    /// window. Returns [`FsError::BinaryFile`] for non-text files.
    async fn read_file(
        &self,
        path: &str,
        offset: Option<u64>,
        limit: Option<u64>,
    ) -> Result<FileContent, FsError>;

    /// Write `content` to `path`, creating or truncating as needed.
    async fn write_file(&self, path: &str, content: &str) -> Result<(), FsError>;

    /// True iff `path` resolves inside the sandboxed workspace root.
    fn is_within_workspace(&self, path: &str) -> bool;

    /// Watch `dir` for changes and emit a stream of [`FileEvent`]s.
    ///
    /// Used by the team-memory watcher (spec §6.4); the platform is
    /// responsible for debouncing and resolving symlinks.
    async fn watch(
        &self,
        dir: &str,
    ) -> Result<Pin<Box<dyn Stream<Item = FileEvent> + Send>>, FsError>;

    /// Append `content` to `path`, creating the file if it does not exist.
    ///
    /// Used by session storage to grow append-only JSONL transcripts without
    /// rewriting prior bytes (spec §22).
    async fn append_file(&self, path: &str, content: &str) -> Result<(), FsError>;

    /// Truncate `path` to exactly `len` bytes.
    ///
    /// Used by the crash-safe JSONL reader to drop a torn tail after a power
    /// loss (spec §22.4 / B5).
    async fn truncate(&self, path: &str, len: u64) -> Result<(), FsError>;

    /// Return the last-modified time of `path`.
    ///
    /// Used by `verify_file_state` to detect Read↔Edit races (spec §23.2 / C5).
    async fn file_mtime(&self, path: &str) -> Result<std::time::SystemTime, FsError>;

    /// Return the size in bytes of `path`.
    ///
    /// Used to enforce size budgets and to validate JSONL recovery offsets.
    async fn file_size(&self, path: &str) -> Result<u64, FsError>;

    /// Delete the regular file at `path`.
    ///
    /// Used by session storage and file-state eviction.
    async fn delete_file(&self, path: &str) -> Result<(), FsError>;

    /// Create a symbolic link `link` pointing at `target`.
    ///
    /// Used to materialize project-level shortcuts (e.g. `last-session ->`).
    async fn symlink(&self, target: &str, link: &str) -> Result<(), FsError>;

    /// Acquire an OS-level advisory exclusive lock on `path`. The returned
    /// [`FlockGuard`] releases the lock when dropped.
    ///
    /// Mobile platforms (Android/iOS) may stub this with app-internal locking;
    /// the trait method is required so engine code can express the intent.
    async fn flock_exclusive(&self, path: &str) -> Result<Box<dyn FlockGuard>, FsError>;

    /// `fsync` the file at `path`, flushing OS buffers to durable storage.
    ///
    /// Engine code calls this after every important write so a power loss
    /// cannot leave a partially-flushed transcript visible (spec §22.3 / B5).
    async fn fsync(&self, path: &str) -> Result<(), FsError>;
}

/// Guard for an OS advisory file lock acquired via
/// [`FileSystem::flock_exclusive`]. Releasing the lock happens in `Drop`.
pub trait FlockGuard: Send + Sync {
    /// The path the guard locks. Implementations may use this for diagnostics.
    fn path(&self) -> &str;
}

/// Result of [`FileSystem::read_file`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileContent {
    /// The file body (possibly truncated; see `truncated`).
    pub content: String,
    /// Total line count of the underlying file before any truncation.
    pub total_lines: u64,
    /// True when `content` is a prefix of the full file.
    pub truncated: bool,
}

/// Failure modes for [`FileSystem`] calls.
#[derive(Debug, Clone, Error)]
pub enum FsError {
    /// Requested path does not exist.
    #[error("file not found: {0}")]
    NotFound(String),
    /// Caller lacks permission on the underlying OS or sandbox.
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    /// Path resolves outside the workspace root.
    #[error("path outside workspace: {0}")]
    OutsideWorkspace(String),
    /// File contents are not valid UTF-8 / look like binary data.
    #[error("file is binary: {0}")]
    BinaryFile(String),
    /// File or read window exceeds the configured size limit.
    #[error("size exceeds limit: {actual} > {limit}")]
    TooLarge {
        /// Actual size encountered, in bytes.
        actual: u64,
        /// Configured maximum, in bytes.
        limit: u64,
    },
    /// Catch-all for underlying I/O failures.
    #[error("io error: {0}")]
    Io(String),
}

/// One file-system change event emitted by [`FileSystem::watch`].
#[derive(Debug, Clone)]
pub struct FileEvent {
    /// Path that changed.
    pub path: std::path::PathBuf,
    /// What kind of change.
    pub kind: FileEventKind,
}

/// Kind of file-system change reported by [`FileEvent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileEventKind {
    /// New file created.
    Created,
    /// Existing file modified.
    Modified,
    /// File deleted.
    Deleted,
}

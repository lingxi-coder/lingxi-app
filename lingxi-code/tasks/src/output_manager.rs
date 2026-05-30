//! Sandboxed spool-file manager for task stdout/stderr.
//!
//! See spec §6.6 / D8 — task output is materialized as files under a
//! sandbox directory, with a per-file and total byte budget.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use thiserror::Error;
use traits::FileSystem;

/// Owner of the task-output sandbox directory.
pub struct TaskOutputManager {
    output_dir: PathBuf,
    fs: Arc<dyn FileSystem>,
    /// Maximum size of a single task's spool file.
    pub max_file_size: u64,
    /// Total byte budget across all task spool files.
    pub total_budget: u64,
    #[allow(dead_code)]
    used: AtomicU64,
}

/// Errors produced by [`TaskOutputManager`].
#[derive(Debug, Clone, Error)]
pub enum OutputError {
    /// I/O failure forwarded from the [`FileSystem`] trait.
    #[error("io: {0}")]
    Io(String),
    /// Refusal to write to a path that escapes the sandbox root.
    #[error("path escapes output dir: {0}")]
    PathEscape(String),
}

/// Read options for [`TaskOutputManager::read`].
#[derive(Debug, Clone, Default)]
pub struct OutputOptions {
    /// Byte offset to start reading from.
    pub offset: Option<u64>,
    /// Maximum number of bytes to return.
    pub limit: Option<u64>,
}

/// Output payload returned by [`TaskOutputManager::read`].
#[derive(Debug, Clone)]
pub struct TaskOutput {
    /// Raw text content (already trimmed by `limit`).
    pub content: String,
    /// Total line count of the underlying file.
    pub total_lines: u64,
    /// True if `content` is a prefix of the spool file.
    pub truncated: bool,
}

impl TaskOutputManager {
    /// Construct a manager rooted at `output_dir`.
    #[must_use]
    pub fn new(output_dir: PathBuf, fs: Arc<dyn FileSystem>) -> Self {
        Self {
            output_dir,
            fs,
            max_file_size: 10 * 1024 * 1024,
            total_budget: 100 * 1024 * 1024,
            used: AtomicU64::new(0),
        }
    }

    /// Allocate a path inside `output_dir`. Refuse any `..` or absolute leak (D8).
    pub async fn allocate(&self, task_id: &str) -> Result<PathBuf, OutputError> {
        let filename = format!("{task_id}.txt");
        let path = self.output_dir.join(&filename);
        if !path.starts_with(&self.output_dir) {
            return Err(OutputError::PathEscape(filename));
        }
        self.fs
            .write_file(path.to_str().expect("utf-8 output path"), "")
            .await
            .map_err(|e| OutputError::Io(e.to_string()))?;
        Ok(path)
    }

    /// Test-only accessor for the backing filesystem (so M5-01 tests can
    /// seed spool content directly without going through a handler).
    #[doc(hidden)]
    pub fn fs_for_test(&self) -> Arc<dyn FileSystem> {
        self.fs.clone()
    }

    /// Read a window of the task's spool file.
    pub async fn read(
        &self,
        output_file: &Path,
        opts: OutputOptions,
    ) -> Result<TaskOutput, OutputError> {
        let fc = self
            .fs
            .read_file(
                output_file.to_str().expect("utf-8 output path"),
                opts.offset,
                opts.limit,
            )
            .await
            .map_err(|e| OutputError::Io(e.to_string()))?;
        Ok(TaskOutput {
            content: fc.content,
            total_lines: fc.total_lines,
            truncated: fc.truncated,
        })
    }
}

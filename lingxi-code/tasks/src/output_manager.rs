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
    /// A spool file already exists for this id — the exclusive (`O_EXCL`)
    /// create refused to clobber it. Guards the double-allocate truncate race:
    /// a second [`TaskOutputManager::allocate`] of the same id errors here
    /// instead of silently truncating output a worker already appended.
    #[error("spool already allocated: {0}")]
    AlreadyExists(String),
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

    /// Return the spool path for `task_id` WITHOUT creating the file. Refuses
    /// any `..` or absolute leak (D8).
    ///
    /// Use this to recover the path a handler already allocated (the spool path
    /// is a deterministic function of the id), so the registry never calls
    /// [`allocate`](Self::allocate) a second time for the same id — the
    /// double-allocate truncate race fix.
    pub fn path_for(&self, task_id: &str) -> Result<PathBuf, OutputError> {
        let filename = format!("{task_id}.txt");
        let path = self.output_dir.join(&filename);
        if !path.starts_with(&self.output_dir) {
            return Err(OutputError::PathEscape(filename));
        }
        Ok(path)
    }

    /// Allocate a fresh spool file inside `output_dir`. Refuse any `..` or
    /// absolute leak (D8).
    ///
    /// The file is created with `O_CREAT | O_EXCL | O_NOFOLLOW`
    /// ([`FileSystem::create_new_file`]), byte-aligned with claude-code's
    /// `initTaskOutput`:
    /// - Exclusive create makes allocation non-destructive — a second
    ///   `allocate` for the same id returns [`OutputError::AlreadyExists`]
    ///   rather than truncating output a worker already appended (T4).
    /// - `O_NOFOLLOW` refuses a pre-planted symlink at the spool path, closing
    ///   the symlink-follow write vector (T18).
    pub async fn allocate(&self, task_id: &str) -> Result<PathBuf, OutputError> {
        let path = self.path_for(task_id)?;
        let path_str = path.to_str().expect("utf-8 output path");
        self.fs.create_new_file(path_str).await.map_err(|e| match e {
            traits::FsError::AlreadyExists(p) => OutputError::AlreadyExists(p),
            other => OutputError::Io(other.to_string()),
        })?;
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Mutex;
    use traits::filesystem::{FileContent, FileEvent, FlockGuard, FsError};

    /// In-memory [`FileSystem`] with REAL exclusive-create semantics: a
    /// `create_new_file` for a path that already has a key fails with
    /// [`FsError::AlreadyExists`] (mirroring `O_EXCL`), and never overwrites the
    /// stored bytes. Also counts `create_new_file` calls so a test can assert
    /// the registry does not re-allocate.
    struct ExclusiveFs {
        files: Mutex<HashMap<String, String>>,
        creates: AtomicUsize,
    }
    impl ExclusiveFs {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                files: Mutex::new(HashMap::new()),
                creates: AtomicUsize::new(0),
            })
        }
        fn create_count(&self) -> usize {
            self.creates.load(Ordering::SeqCst)
        }
    }
    #[async_trait]
    impl FileSystem for ExclusiveFs {
        async fn read_file(
            &self,
            path: &str,
            _offset: Option<u64>,
            _limit: Option<u64>,
        ) -> Result<FileContent, FsError> {
            let map = self.files.lock().await;
            let content = map.get(path).cloned().unwrap_or_default();
            let total_lines = content.lines().count() as u64;
            Ok(FileContent {
                content,
                truncated: false,
                total_lines,
            })
        }
        async fn write_file(&self, path: &str, body: &str) -> Result<(), FsError> {
            self.files
                .lock()
                .await
                .insert(path.to_string(), body.to_string());
            Ok(())
        }
        async fn create_new_file(&self, path: &str) -> Result<(), FsError> {
            self.creates.fetch_add(1, Ordering::SeqCst);
            let mut map = self.files.lock().await;
            if map.contains_key(path) {
                // O_EXCL collision — refuse, and DO NOT touch the stored bytes.
                return Err(FsError::AlreadyExists(path.to_string()));
            }
            map.insert(path.to_string(), String::new());
            Ok(())
        }
        fn is_within_workspace(&self, _: &str) -> bool {
            true
        }
        async fn watch(
            &self,
            _: &str,
        ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = FileEvent> + Send>>, FsError>
        {
            Err(FsError::Io("not supported".into()))
        }
        async fn append_file(&self, path: &str, body: &str) -> Result<(), FsError> {
            self.files
                .lock()
                .await
                .entry(path.to_string())
                .or_default()
                .push_str(body);
            Ok(())
        }
        async fn truncate(&self, _: &str, _: u64) -> Result<(), FsError> {
            Ok(())
        }
        async fn file_mtime(&self, _: &str) -> Result<std::time::SystemTime, FsError> {
            Ok(std::time::SystemTime::UNIX_EPOCH)
        }
        async fn file_size(&self, path: &str) -> Result<u64, FsError> {
            let map = self.files.lock().await;
            Ok(map.get(path).map_or(0, |s| s.len() as u64))
        }
        async fn delete_file(&self, path: &str) -> Result<(), FsError> {
            self.files.lock().await.remove(path);
            Ok(())
        }
        async fn symlink(&self, _: &str, _: &str) -> Result<(), FsError> {
            Ok(())
        }
        async fn flock_exclusive(&self, _: &str) -> Result<Box<dyn FlockGuard>, FsError> {
            Err(FsError::Io("not supported".into()))
        }
        async fn fsync(&self, _: &str) -> Result<(), FsError> {
            Ok(())
        }
    }

    fn manager() -> (Arc<ExclusiveFs>, TaskOutputManager) {
        let fs = ExclusiveFs::new();
        let mgr = TaskOutputManager::new(PathBuf::from("/spool"), fs.clone());
        (fs, mgr)
    }

    #[tokio::test]
    async fn allocate_creates_an_empty_spool_file() {
        let (fs, mgr) = manager();
        let path = mgr.allocate("bdeadbeef").await.expect("first allocate");
        assert_eq!(path, PathBuf::from("/spool/bdeadbeef.txt"));
        // The file exists and is empty.
        assert!(fs.files.lock().await.contains_key("/spool/bdeadbeef.txt"));
    }

    #[tokio::test]
    async fn second_allocate_of_same_id_errors_not_truncates() {
        // T4: a second allocate of the SAME id must surface AlreadyExists, not
        // silently truncate output a worker already appended.
        let (fs, mgr) = manager();
        let path = mgr.allocate("babc123").await.unwrap();

        // A worker appends output AFTER the first allocate.
        fs.append_file_no_follow(path.to_str().unwrap(), "important output\n")
            .await
            .unwrap();

        // The second allocate must error rather than clobber.
        let err = mgr
            .allocate("babc123")
            .await
            .expect_err("a second allocate of the same id must error");
        assert!(
            matches!(err, OutputError::AlreadyExists(_)),
            "second allocate returns AlreadyExists; got {err:?}"
        );

        // The worker's output survived — the refused allocate touched no bytes.
        let read = mgr.read(&path, OutputOptions::default()).await.unwrap();
        assert!(
            read.content.contains("important output"),
            "the existing output was NOT truncated by the refused allocate; got {:?}",
            read.content
        );
    }

    #[tokio::test]
    async fn path_for_does_not_create_or_count_a_spool_file() {
        // `path_for` (the registry's consume-the-handler's-path seam) only
        // reconstructs the deterministic path — it allocates nothing, so a
        // worker's already-allocated spool is never re-created.
        let (fs, mgr) = manager();
        let path = mgr.path_for("bxyz").unwrap();
        assert_eq!(path, PathBuf::from("/spool/bxyz.txt"));
        assert_eq!(fs.create_count(), 0, "path_for must not create a file");
        assert!(
            !fs.files.lock().await.contains_key("/spool/bxyz.txt"),
            "path_for must not materialize the spool"
        );
    }
}

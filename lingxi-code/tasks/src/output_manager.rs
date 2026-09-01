//! Sandboxed spool-file manager for task stdout/stderr.
//!
//! See spec §6.6 / D8 — task output is materialized as files under a
//! sandbox directory, with a per-file and total byte budget.

use platform_api::FileSystem;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::Mutex;

/// Disk cap for a single task's output file. Mirrors claude-code's
/// `MAX_TASK_OUTPUT_BYTES = 5 * 1024 * 1024 * 1024` (`diskOutput.ts:30`).
/// Past this, [`TaskOutputManager::append`] drops further chunks and writes a
/// single truncation marker, matching `DiskTaskOutput.append`.
pub const MAX_TASK_OUTPUT_BYTES: u64 = 5 * 1024 * 1024 * 1024;

/// Display string for [`MAX_TASK_OUTPUT_BYTES`] used in the truncation marker
/// (claude-code `MAX_TASK_OUTPUT_BYTES_DISPLAY = '5GB'`, `diskOutput.ts:31`).
pub const MAX_TASK_OUTPUT_BYTES_DISPLAY: &str = "5GB";

/// Owner of the task-output sandbox directory.
pub struct TaskOutputManager {
    output_dir: PathBuf,
    fs: Arc<dyn FileSystem>,
    /// Identity of the output directory observed by this manager. The
    /// platform rooted operations still perform handle-relative I/O; this
    /// lightweight pin catches a directory that was moved and replaced
    /// between calls before a new rooted handle is opened.
    root_pin: Mutex<OutputRootState>,
    /// Per-spool bytes-written counter + capped flag, keyed by spool path.
    /// Backs the write-side 5GB disk cap ([`MAX_TASK_OUTPUT_BYTES`]): once a
    /// spool crosses the cap its entry is marked capped and further appends are
    /// dropped (after a single truncation marker is written), mirroring
    /// claude-code's `DiskTaskOutput.#bytesWritten` / `#capped`.
    caps: Mutex<HashMap<PathBuf, CapState>>,
}

/// Per-spool write-side cap state (claude-code `DiskTaskOutput` `#bytesWritten`
/// + `#capped`).
#[derive(Debug, Default, Clone, Copy)]
struct CapState {
    bytes_written: u64,
    capped: bool,
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
    /// The output directory no longer names the directory owned by this
    /// manager, or an unsafe symlink was introduced. The recovery guidance is
    /// intentionally explicit about removing the link itself, never its target.
    #[error("{0}")]
    SwapRefused(String),
}

#[derive(Debug, Default)]
struct OutputRootState {
    initialized: bool,
    identity: Option<platform_api::rooted_fs::RootIdentity>,
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
            root_pin: Mutex::new(OutputRootState::default()),
            caps: Mutex::new(HashMap::new()),
        }
    }

    /// The absolute spool directory this manager owns.
    #[must_use]
    pub fn output_dir(&self) -> &Path {
        &self.output_dir
    }

    /// Return the spool path for `task_id` WITHOUT creating the file. Refuses
    /// any `..` or absolute leak (D8).
    ///
    /// Use this to recover the path a handler already allocated (the spool path
    /// is a deterministic function of the id), so the registry never calls
    /// [`allocate`](Self::allocate) a second time for the same id — the
    /// double-allocate truncate race fix.
    pub fn path_for(&self, task_id: &str) -> Result<PathBuf, OutputError> {
        // Extension `.output` byte-aligns with claude-code's
        // `getTaskOutputPath` (`diskOutput.ts:72-74` → `${taskId}.output`).
        let filename = format!("{task_id}.output");
        let path = self.output_dir.join(&filename);
        if !path.starts_with(&self.output_dir) {
            return Err(OutputError::PathEscape(filename));
        }
        Ok(path)
    }

    fn relative_path_for(&self, output_file: &Path) -> Result<PathBuf, OutputError> {
        let relative = output_file
            .strip_prefix(&self.output_dir)
            .map_err(|_| OutputError::PathEscape(output_file.display().to_string()))?
            .to_path_buf();
        platform_api::rooted_fs::validate_relative_path(&relative)
            .map_err(|_| OutputError::PathEscape(relative.display().to_string()))?;
        Ok(relative)
    }

    fn swap_refused(&self, reason: &str) -> OutputError {
        OutputError::SwapRefused(format!(
            "task output swap refused ({reason}): {}. To recover: restart the application with its temporary-directory setting pointed at a fresh directory; or, if {} is a stray directory or a symbolic link that should not be there, remove that entry itself (not what it points to) and restart.",
            self.output_dir.display(),
            self.output_dir.display(),
        ))
    }

    async fn check_output_root(
        &self,
    ) -> Result<Option<platform_api::rooted_fs::RootIdentity>, OutputError> {
        let mut state = self.root_pin.lock().await;
        let current = match self.fs.root_identity_no_follow(&self.output_dir).await {
            Ok(identity) => identity,
            Err(error) if state.initialized => {
                return Err(self.swap_refused(&format!("directory identity check failed: {error}")))
            }
            Err(error) => return Err(self.map_rooted_error(error)),
        };
        if state.initialized {
            if state.identity != current {
                return Err(self.swap_refused("directory identity changed"));
            }
        } else {
            state.initialized = true;
            state.identity = current;
        }
        Ok(state.identity)
    }

    fn map_rooted_error(&self, error: platform_api::FsError) -> OutputError {
        match error {
            platform_api::FsError::OutsideWorkspace(_) => {
                self.swap_refused("a parent or final path component is unsafe")
            }
            other => OutputError::Io(other.to_string()),
        }
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
        let relative = self.relative_path_for(&path)?;
        let root_identity = self.check_output_root().await?;
        self.fs
            .create_new_file_rooted_no_follow_pinned(
                &self.output_dir,
                &relative,
                root_identity.as_ref(),
            )
            .await
            .map_err(|e| match e {
                platform_api::FsError::AlreadyExists(p) => OutputError::AlreadyExists(p),
                other => self.map_rooted_error(other),
            })?;
        Ok(path)
    }

    /// Append a chunk to a task's spool, enforcing the per-file 5GB disk cap
    /// ([`MAX_TASK_OUTPUT_BYTES`]) on the WRITE side.
    ///
    /// Byte-aligned with claude-code's `DiskTaskOutput.append`
    /// (`diskOutput.ts:110-131`): the running byte count uses the chunk's UTF-8
    /// byte length, and once it would cross the cap the spool is marked capped —
    /// a single truncation marker
    /// `\n[output truncated: exceeded 5GB disk cap]\n` is written and all
    /// subsequent appends are dropped. The write itself uses
    /// [`FileSystem::append_file_no_follow`] so a symlink planted at the spool
    /// path from inside the sandbox cannot redirect it (T18).
    pub async fn append(&self, output_file: &Path, content: &str) -> Result<(), OutputError> {
        let relative = self.relative_path_for(output_file)?;
        let root_identity = self.check_output_root().await?;
        // Determine what to write under the cap, holding the per-path state lock
        // only across the cheap bookkeeping (not the await on the fs write).
        let to_write = {
            let mut caps = self.caps.lock().await;
            let state = caps.entry(output_file.to_path_buf()).or_default();
            if state.capped {
                // Already capped — drop further output (claude `if (capped) return`).
                None
            } else {
                state.bytes_written = state.bytes_written.saturating_add(content.len() as u64);
                if state.bytes_written > MAX_TASK_OUTPUT_BYTES {
                    state.capped = true;
                    Some(format!(
                        "\n[output truncated: exceeded {MAX_TASK_OUTPUT_BYTES_DISPLAY} disk cap]\n"
                    ))
                } else {
                    Some(content.to_string())
                }
            }
        };
        if let Some(body) = to_write {
            self.fs
                .append_file_rooted_no_follow_pinned(
                    &self.output_dir,
                    &relative,
                    &body,
                    root_identity.as_ref(),
                )
                .await
                .map_err(|e| self.map_rooted_error(e))?;
        }
        Ok(())
    }

    /// Test-only accessor for the backing filesystem (so M5-01 tests can
    /// seed spool content directly without going through a handler).
    #[doc(hidden)]
    pub fn fs_for_test(&self) -> Arc<dyn FileSystem> {
        self.fs.clone()
    }

    /// Test-only: pre-seed the per-path write-side byte counter, so the 5GB cap
    /// can be exercised without materializing 5GB of output.
    #[cfg(test)]
    async fn seed_bytes_for_test(&self, output_file: &Path, bytes: u64) {
        let mut caps = self.caps.lock().await;
        caps.entry(output_file.to_path_buf())
            .or_default()
            .bytes_written = bytes;
    }

    /// Read a window of the task's spool file.
    pub async fn read(
        &self,
        output_file: &Path,
        opts: OutputOptions,
    ) -> Result<TaskOutput, OutputError> {
        let relative = self.relative_path_for(output_file)?;
        let root_identity = self.check_output_root().await?;
        let fc = self
            .fs
            .read_file_rooted_no_follow_window_pinned(
                &self.output_dir,
                &relative,
                opts.offset,
                opts.limit,
                root_identity.as_ref(),
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
    use platform_api::filesystem::{FileContent, FileEvent, FlockGuard, FsError};
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Mutex;

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
        async fn root_identity_no_follow(
            &self,
            root: &Path,
        ) -> Result<Option<platform_api::rooted_fs::RootIdentity>, FsError> {
            if root.exists() {
                platform_api::rooted_fs::root_identity(root).map(Some)
            } else {
                Ok(None)
            }
        }

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
        assert_eq!(path, PathBuf::from("/spool/bdeadbeef.output"));
        // The file exists and is empty.
        assert!(fs
            .files
            .lock()
            .await
            .contains_key("/spool/bdeadbeef.output"));
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
        assert_eq!(path, PathBuf::from("/spool/bxyz.output"));
        assert_eq!(fs.create_count(), 0, "path_for must not create a file");
        assert!(
            !fs.files.lock().await.contains_key("/spool/bxyz.output"),
            "path_for must not materialize the spool"
        );
    }

    #[tokio::test]
    async fn append_writes_through_and_under_the_cap_is_unmarked() {
        // Below the 5GB cap, `append` writes the chunk verbatim — no marker.
        let (_fs, mgr) = manager();
        let path = mgr.allocate("bappend01").await.unwrap();
        mgr.append(&path, "hello ").await.unwrap();
        mgr.append(&path, "world\n").await.unwrap();
        let read = mgr.read(&path, OutputOptions::default()).await.unwrap();
        assert_eq!(read.content, "hello world\n");
        assert!(
            !read.content.contains("disk cap"),
            "no truncation marker under the cap; got {:?}",
            read.content
        );
    }

    #[tokio::test]
    async fn append_caps_at_5gb_and_writes_marker_then_drops() {
        // T17: claude-code `DiskTaskOutput.append` caps a single spool at
        // MAX_TASK_OUTPUT_BYTES (5GB). The byte counter uses chunk length, so
        // we exercise the boundary with a pre-seeded counter instead of
        // materializing 5GB. The first chunk that crosses the cap writes the
        // EXACT marker `\n[output truncated: exceeded 5GB disk cap]\n`;
        // subsequent appends are dropped entirely.
        let (_fs, mgr) = manager();
        let path = mgr.allocate("bcap00001").await.unwrap();

        // Seed the counter one byte below the cap, then append two bytes — the
        // running total crosses MAX_TASK_OUTPUT_BYTES in a single append.
        mgr.seed_bytes_for_test(&path, MAX_TASK_OUTPUT_BYTES - 1)
            .await;
        mgr.append(&path, "ab").await.unwrap();

        // A further write must be dropped (the spool is now capped).
        mgr.append(&path, "this must be dropped\n").await.unwrap();

        let read = mgr.read(&path, OutputOptions::default()).await.unwrap();
        assert_eq!(
            read.content, "\n[output truncated: exceeded 5GB disk cap]\n",
            "the crossing append is REPLACED by the exact marker (the raw \"ab\" \
             is not written), and post-cap appends drop"
        );
        assert_eq!(
            MAX_TASK_OUTPUT_BYTES,
            5 * 1024 * 1024 * 1024,
            "cap constant is 5GB"
        );
        assert_eq!(MAX_TASK_OUTPUT_BYTES_DISPLAY, "5GB");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn output_root_swap_is_refused_with_recovery_guidance() {
        let parent = tempfile::tempdir().unwrap();
        let output_dir = parent.path().join("tasks");
        std::fs::create_dir(&output_dir).unwrap();
        let victim = tempfile::tempdir().unwrap();
        let fs = ExclusiveFs::new();
        let manager = TaskOutputManager::new(output_dir.clone(), fs.clone());

        // The first operation pins the real task-output directory identity.
        manager.allocate("bpin0001").await.unwrap();
        std::fs::rename(&output_dir, parent.path().join("tasks-moved")).unwrap();
        std::os::unix::fs::symlink(victim.path(), &output_dir).unwrap();

        let error = manager
            .allocate("bpin0002")
            .await
            .expect_err("a moved/symlinked task directory must fail closed");
        let OutputError::SwapRefused(message) = error else {
            panic!("expected SwapRefused, got {error:?}");
        };
        assert!(message.contains("task output swap refused"));
        assert!(message.contains("fresh directory"));
        assert!(message.contains("remove that entry itself"));
        assert!(!victim.path().join("bpin0002.output").exists());
        assert!(!fs
            .files
            .lock()
            .await
            .contains_key(&output_dir.join("bpin0002.output").display().to_string()));
    }
}

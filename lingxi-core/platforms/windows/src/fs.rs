//! `tokio::fs`-backed [`FileSystem`] for Windows hosts.
//!
//! Implements the engine's sandboxed filesystem trait using the real OS
//! filesystem. Path containment is enforced via prefix-match against the
//! workspace root supplied at construction. [`FileSystem::watch`] returns
//! an empty stream pending a `ReadDirectoryChangesW` binding (deferred to
//! a follow-up).

use async_trait::async_trait;
use fs2::FileExt;
use futures::stream::Stream;
use lingxi_traits::{FileContent, FileEvent, FileSystem, FlockGuard, FsError};
use std::path::PathBuf;
use std::pin::Pin;

/// Concrete [`FileSystem`] backed by `tokio::fs`.
///
/// Construction takes the workspace root used by
/// [`FileSystem::is_within_workspace`].
pub struct WindowsFileSystem {
    workspace_root: PathBuf,
}

impl WindowsFileSystem {
    /// Build a new `WindowsFileSystem` rooted at `workspace_root`.
    #[must_use]
    pub fn new(workspace_root: PathBuf) -> Self {
        Self { workspace_root }
    }
}

#[async_trait]
impl FileSystem for WindowsFileSystem {
    async fn read_file(
        &self,
        path: &str,
        offset: Option<u64>,
        limit: Option<u64>,
    ) -> Result<FileContent, FsError> {
        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| FsError::Io(e.to_string()))?;
        let mut lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len() as u64;
        if let Some(off) = offset {
            let off_usize = usize::try_from(off).unwrap_or(usize::MAX);
            lines = lines.into_iter().skip(off_usize).collect();
        }
        if let Some(lim) = limit {
            let lim_usize = usize::try_from(lim).unwrap_or(usize::MAX);
            lines.truncate(lim_usize);
        }
        Ok(FileContent {
            content: lines.join("\n"),
            total_lines,
            truncated: false,
        })
    }

    async fn write_file(&self, path: &str, content: &str) -> Result<(), FsError> {
        tokio::fs::write(path, content)
            .await
            .map_err(|e| FsError::Io(e.to_string()))
    }

    async fn append_file(&self, path: &str, content: &str) -> Result<(), FsError> {
        use tokio::io::AsyncWriteExt;
        let mut f = tokio::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(path)
            .await
            .map_err(|e| FsError::Io(e.to_string()))?;
        f.write_all(content.as_bytes())
            .await
            .map_err(|e| FsError::Io(e.to_string()))
    }

    async fn truncate(&self, path: &str, len: u64) -> Result<(), FsError> {
        let f = std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .map_err(|e| FsError::Io(e.to_string()))?;
        f.set_len(len).map_err(|e| FsError::Io(e.to_string()))
    }

    async fn file_mtime(&self, path: &str) -> Result<std::time::SystemTime, FsError> {
        let meta = tokio::fs::metadata(path)
            .await
            .map_err(|e| FsError::Io(e.to_string()))?;
        meta.modified().map_err(|e| FsError::Io(e.to_string()))
    }

    async fn file_size(&self, path: &str) -> Result<u64, FsError> {
        let meta = tokio::fs::metadata(path)
            .await
            .map_err(|e| FsError::Io(e.to_string()))?;
        Ok(meta.len())
    }

    async fn delete_file(&self, path: &str) -> Result<(), FsError> {
        tokio::fs::remove_file(path)
            .await
            .map_err(|e| FsError::Io(e.to_string()))
    }

    async fn symlink(&self, target: &str, link: &str) -> Result<(), FsError> {
        #[cfg(unix)]
        {
            tokio::fs::symlink(target, link)
                .await
                .map_err(|e| FsError::Io(e.to_string()))
        }
        #[cfg(windows)]
        {
            tokio::fs::symlink_file(target, link)
                .await
                .map_err(|e| FsError::Io(e.to_string()))
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (target, link);
            Err(FsError::Io("symlink: unsupported on this OS".into()))
        }
    }

    async fn flock_exclusive(&self, path: &str) -> Result<Box<dyn FlockGuard>, FsError> {
        let f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(|e| FsError::Io(e.to_string()))?;
        f.lock_exclusive().map_err(|e| FsError::Io(e.to_string()))?;
        Ok(Box::new(WindowsFlockGuard {
            _file: f,
            path: path.to_string(),
        }))
    }

    async fn fsync(&self, path: &str) -> Result<(), FsError> {
        let f = std::fs::OpenOptions::new()
            .read(true)
            .open(path)
            .map_err(|e| FsError::Io(e.to_string()))?;
        f.sync_all().map_err(|e| FsError::Io(e.to_string()))
    }

    fn is_within_workspace(&self, path: &str) -> bool {
        std::path::Path::new(path).starts_with(&self.workspace_root)
    }

    async fn watch(
        &self,
        _dir: &str,
    ) -> Result<Pin<Box<dyn Stream<Item = FileEvent> + Send>>, FsError> {
        use futures::stream::empty;
        // TODO(M2-followup): ReadDirectoryChangesW binding for Windows.
        Ok(Box::pin(empty()))
    }
}

/// Guard holding the `fs2` advisory lock. Dropping the file releases the
/// lock — exactly the semantics [`FlockGuard`] documents. On Windows this
/// uses `LockFileEx` under the hood via `fs2::FileExt::lock_exclusive`.
struct WindowsFlockGuard {
    _file: std::fs::File,
    path: String,
}

impl FlockGuard for WindowsFlockGuard {
    fn path(&self) -> &str {
        &self.path
    }
}

//! `tokio::fs`-backed [`FileSystem`] for desktop hosts.
//!
//! Implements the engine's sandboxed filesystem trait using the real OS
//! filesystem. Path containment is enforced via prefix-match against the
//! workspace root supplied at construction; this is intentionally minimal —
//! a hardened POSIX platform (Plan 17+) layers symlink-canonicalisation and
//! sandbox-exec on top.

use async_trait::async_trait;
use fs2::FileExt;
use futures_core::stream::Stream;
use futures_util::stream::empty;
use std::path::PathBuf;
use std::pin::Pin;
use traits::{FileContent, FileEvent, FileSystem, FlockGuard, FsError};

/// Concrete [`FileSystem`] backed by `tokio::fs`.
///
/// Construction takes the workspace root used by
/// [`FileSystem::is_within_workspace`]; pass the directory the CLI was
/// launched in (`std::env::current_dir()`) for the M1 demo.
pub struct PosixFileSystem {
    workspace_root: PathBuf,
}

impl PosixFileSystem {
    /// Build a new filesystem rooted at `workspace_root`.
    #[must_use]
    pub fn new(workspace_root: PathBuf) -> Self {
        Self { workspace_root }
    }
}

#[async_trait]
impl FileSystem for PosixFileSystem {
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
        Ok(Box::new(PosixFlockGuard {
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
        // posix-minimal: emit an empty stream. Real `notify`-backed watcher
        // lands in `platforms/posix` (Plan 17+).
        Ok(Box::pin(empty()))
    }
}

/// Guard holding the `fs2` advisory lock. Dropping the file releases the
/// lock — exactly the semantics [`FlockGuard`] documents.
struct PosixFlockGuard {
    _file: std::fs::File,
    path: String,
}

impl FlockGuard for PosixFlockGuard {
    fn path(&self) -> &str {
        &self.path
    }
}

//! Tool result storage — overflow-to-disk for large results.

use protocol::ToolUseId;
use std::path::PathBuf;
use std::sync::Arc;
use traits::FileSystem;

/// Storage for large tool results that overflow the message budget.
pub struct ToolResultStorage {
    storage_dir: PathBuf,
    fs: Arc<dyn FileSystem>,
}

impl ToolResultStorage {
    /// Create a new storage rooted at `storage_dir`.
    #[must_use]
    pub fn new(storage_dir: PathBuf, fs: Arc<dyn FileSystem>) -> Self {
        Self { storage_dir, fs }
    }

    /// Persist `content` under the tool-use id; returns the storage path.
    ///
    /// # Errors
    /// Returns the underlying `FsError` if the write fails.
    pub async fn store(
        &self,
        tool_use_id: &ToolUseId,
        content: &str,
    ) -> Result<PathBuf, traits::FsError> {
        let path = self.storage_dir.join(format!("{tool_use_id}.txt"));
        self.fs
            .write_file(path.to_str().expect("utf8 path"), content)
            .await?;
        Ok(path)
    }
}

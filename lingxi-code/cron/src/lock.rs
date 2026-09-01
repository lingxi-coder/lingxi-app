//! Cross-process file lock with PID-liveness detection (A9).
//!
//! Stale locks (whose holder PID has exited) are silently overridden so that
//! a crashed scheduler does not block future ticks indefinitely.

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use thiserror::Error;
use platform_api::FileSystem;

/// Failure modes for [`try_acquire_lock`] / [`release_lock`].
#[derive(Debug, Clone, Error)]
pub enum CronLockError {
    /// Another live process currently owns this job's lock.
    #[error("lock held by live process {pid}")]
    HeldByLivePid {
        /// PID of the live holder.
        pid: u32,
    },
    /// Underlying file-system I/O error.
    #[error("io: {0}")]
    Io(String),
    /// Failed to deserialize a [`LockRecord`] from disk.
    #[error("parse: {0}")]
    Parse(String),
}

/// On-disk lock contents written by [`try_acquire_lock`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockRecord {
    /// Process ID of the holder.
    pub pid: u32,
    /// Wall-clock time when the lock was acquired.
    pub acquired_at: std::time::SystemTime,
    /// Cron job identifier the lock protects.
    pub job_id: String,
}

/// Try to take the per-job lock.
///
/// - Returns `Ok(())` if we successfully wrote our PID into the lock file
///   (either no prior holder, or the prior holder was stale).
/// - Returns [`CronLockError::HeldByLivePid`] when another live process holds
///   the lock.
/// - Returns [`CronLockError::Io`] on filesystem failure.
pub async fn try_acquire_lock(
    fs: Arc<dyn FileSystem>,
    lock_path: &Path,
    our_pid: u32,
    job_id: &str,
    pid_is_alive: impl Fn(u32) -> bool,
) -> Result<(), CronLockError> {
    let state_dir = lock_path
        .parent()
        .ok_or_else(|| CronLockError::Io("lock path has no parent".into()))?;
    let project_root = state_dir
        .parent()
        .ok_or_else(|| CronLockError::Io("lock path has no project root".into()))?;
    if state_dir.file_name() != Some(std::ffi::OsStr::new(branding::DOT_DIR)) {
        return Err(CronLockError::Io(
            "lock path is outside the project state directory".into(),
        ));
    }
    let relative = Path::new(branding::DOT_DIR).join(
        lock_path
            .file_name()
            .ok_or_else(|| CronLockError::Io("lock path has no file name".into()))?,
    );

    if let Ok(content) = fs
        .read_file_rooted_no_follow(project_root, &relative)
        .await
        .map(|c| c.content)
    {
        if let Ok(existing) = serde_json::from_str::<LockRecord>(&content) {
            if existing.pid != our_pid && pid_is_alive(existing.pid) {
                return Err(CronLockError::HeldByLivePid { pid: existing.pid });
            }
        }
    }

    let record = LockRecord {
        pid: our_pid,
        acquired_at: std::time::SystemTime::now(),
        job_id: job_id.to_string(),
    };
    let body = serde_json::to_string(&record).expect("serialize lock record");
    fs.write_file_rooted_atomic(project_root, &relative, &body)
        .await
        .map_err(|e| CronLockError::Io(e.to_string()))?;
    Ok(())
}

/// Drop the per-job lock file (best-effort).
pub async fn release_lock(fs: Arc<dyn FileSystem>, lock_path: &Path) -> Result<(), CronLockError> {
    let state_dir = lock_path
        .parent()
        .ok_or_else(|| CronLockError::Io("lock path has no parent".into()))?;
    let project_root = state_dir
        .parent()
        .ok_or_else(|| CronLockError::Io("lock path has no project root".into()))?;
    if state_dir.file_name() != Some(std::ffi::OsStr::new(branding::DOT_DIR)) {
        return Err(CronLockError::Io(
            "lock path is outside the project state directory".into(),
        ));
    }
    let relative = Path::new(branding::DOT_DIR).join(
        lock_path
            .file_name()
            .ok_or_else(|| CronLockError::Io("lock path has no file name".into()))?,
    );
    fs.delete_file_rooted_no_follow(project_root, &relative)
        .await
        .map_err(|e| CronLockError::Io(e.to_string()))
}

#[cfg(test)]
mod tests {
    // Full mock-fs based tests in test-harness/tests/cron_lock.rs.
}

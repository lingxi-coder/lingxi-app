//! Test-only helpers: process-env serialization + temp config dirs.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

/// Process-wide lock for tests that mutate env vars (`HOME`,
/// `CLAUDE_CONFIG_DIR`, `DISABLE_AUTOUPDATER`, provider gates). Cargo runs
/// tests in parallel threads sharing the process env; hold this for the
/// test's whole body.
pub fn env_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A throwaway config universe: `dir` is a tempdir acting as the Claude
/// config home; `global` is the `~/.claude.json`-equivalent path inside it.
pub struct TempConfig {
    /// Owns the tempdir (deleted on drop).
    pub _tmp: tempfile::TempDir,
    /// Stand-in for `~/.claude` (claude config home).
    pub home: PathBuf,
    /// Stand-in for `~/.claude.json` (global config file).
    pub global: PathBuf,
    /// Stand-in project directory (for settings.local.json).
    pub project: PathBuf,
}

/// Build a fresh [`TempConfig`]. No env mutation — APIs take explicit paths.
pub fn temp_config() -> TempConfig {
    let tmp = tempfile::tempdir().expect("tempdir");
    let home = tmp.path().join("claude-home");
    let global = tmp.path().join("claude.json");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&home).expect("mk home");
    std::fs::create_dir_all(&project).expect("mk project");
    TempConfig { _tmp: tmp, home, global, project }
}

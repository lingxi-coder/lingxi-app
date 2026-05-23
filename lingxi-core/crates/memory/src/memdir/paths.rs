//! `~/.claude/memdir/` and `~/.claude/team-mem/` resolution. Filled in Task 5.

use std::path::PathBuf;

/// Subdirectory under `~/.claude/` for individual memdir entries.
pub const MEMDIR_SUBDIR: &str = "memdir";
/// Subdirectory under `~/.claude/` for team-shared entries.
pub const TEAM_MEM_SUBDIR: &str = "team-mem";

/// Resolved roots for the memdir scan.
#[derive(Debug, Clone)]
pub struct MemdirRoots {
    /// `~/.claude/memdir/` (always set; may not exist on disk).
    pub user_memdir: PathBuf,
    /// `~/.claude/team-mem/` — `None` when `team_memory.enabled == false`.
    pub team_memdir: Option<PathBuf>,
}

/// Stub returning `MemdirRoots` rooted at `home` with optional team dir.
/// Real impl lands in Task 5.
#[must_use]
pub fn memdir_path(_home: &std::path::Path, _team_enabled: bool) -> MemdirRoots {
    unimplemented!("filled in Task 5")
}

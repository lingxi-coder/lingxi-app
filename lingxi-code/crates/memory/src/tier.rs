//! Memory-tier model.
//!
//! Each loaded memory file belongs to one of four tiers (spec §6.1). The
//! tier determines its discovery rules, lifetime, and trust level.

use lingxi_protocol::SessionId;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Source location for a set of memory files.
///
/// The runtime walks each active tier to build the available memory set
/// before invoking the LLM selector.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MemoryTier {
    /// Repository-local memory (e.g. `.claude/CLAUDE.md`).
    Project {
        /// Repository root containing the memory tree.
        repo_root: PathBuf,
    },
    /// Per-user memory checked into the user's home directory.
    User {
        /// Root of the user's memory directory (e.g. `~/.claude/`).
        user_memory_dir: PathBuf,
    },
    /// Ephemeral notes that live only for the current session.
    Session {
        /// Session identifier this memory tier is bound to.
        session_id: SessionId,
    },
    /// Shared team memory pulled from a central directory.
    Team {
        /// Directory containing team-shared memory.
        team_dir: PathBuf,
        /// Whether to attach a filesystem watcher and hot-reload changes.
        watcher_enabled: bool,
    },
}

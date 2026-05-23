//! Team-mem path resolution, gated on `settings.team_memory.enabled`.
//! Filled in Task 11.

use std::path::{Path, PathBuf};

/// Resolve `~/.claude/team-mem/` when explicitly enabled.
///
/// Returns `None` when `enabled == false`. Auto-detection from
/// filesystem presence is intentionally NOT used — caller must opt in
/// via the `settings.team_memory.enabled` bool.
#[must_use]
pub fn resolve_team_memory_dir(home: &Path, enabled: bool) -> Option<PathBuf> {
    if !enabled {
        return None;
    }
    Some(home.join(".claude").join(super::paths::TEAM_MEM_SUBDIR))
}

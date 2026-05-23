//! `~/.claude/memdir/` and `~/.claude/team-mem/` resolution.

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

/// Resolve memdir roots under `home`. Team root is `Some` only when
/// `team_enabled == true` (the bool comes from `settings.team_memory.enabled`;
/// auto-detection from filesystem presence is intentionally NOT used).
#[must_use]
pub fn memdir_path(home: &std::path::Path, team_enabled: bool) -> MemdirRoots {
    let dot_claude = home.join(".claude");
    let user_memdir = dot_claude.join(MEMDIR_SUBDIR);
    let team_memdir = if team_enabled {
        Some(dot_claude.join(TEAM_MEM_SUBDIR))
    } else {
        None
    };
    MemdirRoots {
        user_memdir,
        team_memdir,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn user_memdir_is_dot_claude_memdir() {
        let home = PathBuf::from("/home/u");
        let roots = memdir_path(&home, false);
        assert_eq!(roots.user_memdir, PathBuf::from("/home/u/.claude/memdir"));
        assert_eq!(roots.team_memdir, None);
    }

    #[test]
    fn team_memdir_is_dot_claude_team_mem_when_enabled() {
        let home = PathBuf::from("/home/u");
        let roots = memdir_path(&home, true);
        assert_eq!(roots.team_memdir, Some(PathBuf::from("/home/u/.claude/team-mem")));
    }

    #[test]
    fn subdir_constants_match_spec_wire_identifiers() {
        assert_eq!(MEMDIR_SUBDIR, "memdir");
        assert_eq!(TEAM_MEM_SUBDIR, "team-mem");
    }
}

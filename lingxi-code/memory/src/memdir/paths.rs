//! Config-home memdir resolution: `<config-home>/{memdir, agents/session-memory,
//! team-mem}`, where config-home honors `$LINGXI_CONFIG_DIR` (else `~/.claude`).

use std::path::{Path, PathBuf};

/// Subdirectory under the config-home for individual memdir entries.
pub const MEMDIR_SUBDIR: &str = "memdir";
/// Subdirectory under the config-home for team-shared entries.
pub const TEAM_MEM_SUBDIR: &str = "team-mem";
/// Subdirectory under the config-home holding agent artifacts (session memory).
pub const AGENTS_SUBDIR: &str = "agents";
/// Subdirectory under `agents/` holding per-session memory files (`<id>.md`) —
/// 1:1 with the `session_memory` WRITE path and `detect_session_file_type`.
pub const SESSION_MEMORY_SUBDIR: &str = "session-memory";

/// Resolved roots for the memdir scan.
#[derive(Debug, Clone)]
pub struct MemdirRoots {
    /// `<config-home>/memdir/` (always set; may not exist on disk).
    pub user_memdir: PathBuf,
    /// `<config-home>/agents/session-memory/` — the Session tier, holding the
    /// per-session memory files `session_memory` writes. Always set; the dir may
    /// not exist until a session-memory extraction has run.
    pub session_memdir: PathBuf,
    /// `<config-home>/team-mem/` — `None` when `team_memory.enabled == false`.
    pub team_memdir: Option<PathBuf>,
}

/// Resolve memdir roots under `home`, honoring `$LINGXI_CONFIG_DIR` for the
/// config-home (matching the `session_memory` WRITE path, so the Session-tier
/// scan reads exactly what writes produce). Team root is `Some` only when
/// `team_enabled == true` (the bool comes from `settings.team_memory.enabled`;
/// auto-detection from filesystem presence is intentionally NOT used).
#[must_use]
pub fn memdir_path(home: &Path, team_enabled: bool) -> MemdirRoots {
    memdir_roots_at(&crate::lingxi_md::user_config_dir(home), team_enabled)
}

/// Pure roots resolver from an already-resolved `config_home` (the `.claude`
/// dir). Env-free, so unit tests are deterministic; [`memdir_path`] is the thin
/// `$LINGXI_CONFIG_DIR`-honoring wrapper.
#[must_use]
pub fn memdir_roots_at(config_home: &Path, team_enabled: bool) -> MemdirRoots {
    MemdirRoots {
        user_memdir: config_home.join(MEMDIR_SUBDIR),
        session_memdir: config_home.join(AGENTS_SUBDIR).join(SESSION_MEMORY_SUBDIR),
        team_memdir: team_enabled.then(|| config_home.join(TEAM_MEM_SUBDIR)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn roots_are_derived_under_config_home() {
        let cfg = PathBuf::from("/home/u/.lingxi");
        let roots = memdir_roots_at(&cfg, false);
        assert_eq!(roots.user_memdir, PathBuf::from("/home/u/.lingxi/memdir"));
        assert_eq!(
            roots.session_memdir,
            PathBuf::from("/home/u/.lingxi/agents/session-memory")
        );
        assert_eq!(roots.team_memdir, None);
    }

    #[test]
    fn team_memdir_is_team_mem_when_enabled() {
        let cfg = PathBuf::from("/home/u/.lingxi");
        let roots = memdir_roots_at(&cfg, true);
        assert_eq!(
            roots.team_memdir,
            Some(PathBuf::from("/home/u/.lingxi/team-mem"))
        );
    }

    #[test]
    fn subdir_constants_match_spec_wire_identifiers() {
        assert_eq!(MEMDIR_SUBDIR, "memdir");
        assert_eq!(TEAM_MEM_SUBDIR, "team-mem");
        assert_eq!(AGENTS_SUBDIR, "agents");
        assert_eq!(SESSION_MEMORY_SUBDIR, "session-memory");
    }
}

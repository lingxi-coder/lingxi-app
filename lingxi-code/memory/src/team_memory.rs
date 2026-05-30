//! Team-memory directory watcher (full hot-reload watcher lands in Plan 10).
//!
//! v0.4.0 (M3-02) replaces the v0.3.0 stubs with a real path resolver
//! backed by [`crate::memdir::team_paths::resolve_team_memory_dir`].
//! The `Watcher` body itself is still scaffold — hot-reload + secret-scan-on-event
//! ship in Plan 10. The resolver, however, is production code from M3-02 onward.

use crate::memdir::team_paths::resolve_team_memory_dir;
use std::path::{Path, PathBuf};

/// Watches a team-memory directory for additions and edits.
///
/// Construction resolves the team dir via the opt-in
/// `team_memory.enabled` settings flag. When disabled, the watcher is
/// constructed in a no-op state.
pub struct TeamMemoryWatcher {
    dir: Option<PathBuf>,
}

impl TeamMemoryWatcher {
    /// Construct a watcher rooted at `<home>/.claude/team-mem/` when
    /// `enabled == true`, otherwise no-op.
    #[must_use]
    pub fn new(home: &Path, enabled: bool) -> Self {
        Self {
            dir: resolve_team_memory_dir(home, enabled),
        }
    }

    /// The resolved directory if the watcher is active.
    #[must_use]
    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    /// Whether the watcher will surface any events (Plan 10 wiring).
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.dir.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn watcher_inactive_when_disabled() {
        let w = TeamMemoryWatcher::new(&PathBuf::from("/home/u"), false);
        assert!(!w.is_active());
        assert!(w.dir().is_none());
    }

    #[test]
    fn watcher_active_resolves_to_team_mem() {
        let w = TeamMemoryWatcher::new(&PathBuf::from("/home/u"), true);
        assert_eq!(
            w.dir(),
            Some(std::path::Path::new("/home/u/.claude/team-mem"))
        );
    }
}

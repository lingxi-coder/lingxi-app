//! Team-memory directory watcher: poll-based hot-reload + event-time secret
//! scan over `<home>/.lingxi/team-mem/`, gated by the opt-in `team_memory.enabled`
//! flag. Disabled ⇒ inert no-op. Path resolver from M3-02; poll/scan from M9.

use crate::memdir::team_paths::resolve_team_memory_dir;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Watches a team-memory directory for additions and edits.
///
/// Construction resolves the team dir via the opt-in
/// `team_memory.enabled` settings flag. When disabled, the watcher is
/// constructed in a no-op state.
pub struct TeamMemoryWatcher {
    dir: Option<PathBuf>,
    /// Last-seen mtimes per `.md` file, for poll-based change detection.
    seen: HashMap<PathBuf, SystemTime>,
}

impl TeamMemoryWatcher {
    /// Construct a watcher rooted at `<home>/.lingxi/team-mem/` when
    /// `enabled == true`, otherwise no-op.
    #[must_use]
    pub fn new(home: &Path, enabled: bool) -> Self {
        Self {
            dir: resolve_team_memory_dir(home, enabled),
            seen: HashMap::new(),
        }
    }

    /// The resolved directory if the watcher is active.
    #[must_use]
    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    /// Whether the watcher will surface any events.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.dir.is_some()
    }

    /// Poll the team dir once: detect added/edited `.md` files since the last
    /// poll (mtime-keyed, no new deps), secret-scan each changed file at event
    /// time, and return the changed paths. Inactive watcher ⇒ empty (no-op).
    /// A directory-read or per-file-read failure DEGRADES to a warn log + skip,
    /// never panics. The first poll seeds baseline state (every existing file
    /// counts as changed once, like a cold reload).
    pub fn poll_changes(&mut self) -> Vec<PathBuf> {
        let Some(dir) = self.dir.clone() else {
            return Vec::new();
        };
        let mut changed = Vec::new();
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!("team-memory watch read_dir failed, degrading: {e}");
                return changed;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|x| x.to_str()) != Some("md") {
                continue;
            }
            let Ok(mtime) = entry.metadata().and_then(|m| m.modified()) else {
                continue;
            };
            if self.seen.get(&path) != Some(&mtime) {
                self.seen.insert(path.clone(), mtime);
                self.scan_for_secrets(&path);
                changed.push(path);
            }
        }
        changed
    }

    /// Event-time secret scan: redact + emit telemetry for a changed file.
    /// Read failure degrades to a warn log (never blocks the reload).
    fn scan_for_secrets(&self, path: &Path) {
        match std::fs::read_to_string(path) {
            Ok(content) => {
                let (_redacted, detections) = crate::secret_scan::redact_with_detections(&content);
                if !detections.is_empty() {
                    tracing::warn!(
                        "team-memory {} carries {} redacted secret(s)",
                        path.display(),
                        detections.len()
                    );
                }
            }
            Err(e) => tracing::warn!("team-memory secret scan skipped for {}: {e}", path.display()),
        }
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
            Some(std::path::Path::new("/home/u/.lingxi/team-mem"))
        );
    }

    #[test]
    fn inactive_poll_is_noop() {
        let mut w = TeamMemoryWatcher::new(&PathBuf::from("/home/u"), false);
        assert!(w.poll_changes().is_empty());
    }

    #[test]
    fn poll_detects_added_then_stable() {
        let tmp = std::env::temp_dir().join(format!("tmw_{}", std::process::id()));
        let team = tmp.join(".lingxi/team-mem");
        std::fs::create_dir_all(&team).unwrap();
        std::fs::write(team.join("a.md"), "hi").unwrap();
        let mut w = TeamMemoryWatcher::new(&tmp, true);
        assert_eq!(w.poll_changes().len(), 1, "first poll seeds + reports file");
        assert!(w.poll_changes().is_empty(), "unchanged ⇒ no event");
        std::fs::remove_dir_all(&tmp).ok();
    }
}

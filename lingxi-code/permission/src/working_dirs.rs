//! The session's additional working directories, with the SOURCE that
//! contributed each one.
//!
//! 1:1 with claude-code's `ToolPermissionContext.additionalWorkingDirectories`,
//! which is a `Map<path, {path, source}>` — keyed by path, carrying the source
//! that added it:
//!
//! ```js
//! kEn(ctx, dirs, source) {                       // addDirectories
//!   let m = new Map(ctx.additionalWorkingDirectories);
//!   for (let d of dirs) m.set(d, {path: d, source});
//!   return {...ctx, additionalWorkingDirectories: m};
//! }
//! rb(ctx)  = new Set([cwd, ...ctx.additionalWorkingDirectories.keys()]);
//! mEt(ctx) = new Set([cwd, ...[...ctx.additionalWorkingDirectories.values()]
//!                              .filter(t => t.source !== "projectSettings")
//!                              .map(t => t.path)]);
//! ```
//!
//! The source is not decoration: [`Self::read_block_paths`] (`mEt`) drops
//! `projectSettings`-sourced entries, so a directory a checked-in settings file
//! adds cannot widen `permissions.blockReadsOutsideWorkingDirectories`. The
//! oracle also partitions on source elsewhere — `Kt(s)` (`cliArg`/`command`/
//! `session`) marks the ephemeral sources that are never persisted back to a
//! settings file, and the background-session snapshot keeps only `session`
//! entries — so the source must live on the entry, not in a side table.

use crate::rule::PermissionRuleSource;
use std::path::{Path, PathBuf};

/// One additional working directory and the source that contributed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkingDirectory {
    /// The directory itself (the map KEY in claude-code).
    pub path: PathBuf,
    /// Which settings tier / runtime action added it.
    pub source: PermissionRuleSource,
}

/// Insertion-ordered, path-keyed set of [`WorkingDirectory`] — the port of the
/// `additionalWorkingDirectories` Map. Iteration order is insertion order, which
/// keeps the `${[...dirs].join(", ")}` confinement message deterministic.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdditionalWorkingDirs {
    entries: Vec<WorkingDirectory>,
}

impl AdditionalWorkingDirs {
    /// Empty set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// `Map.set(path, {path, source})` — a repeated path keeps its position and
    /// takes the NEW source, exactly like re-setting a JS Map key.
    pub fn insert(&mut self, path: impl Into<PathBuf>, source: PermissionRuleSource) {
        let path = path.into();
        if let Some(existing) = self.entries.iter_mut().find(|entry| entry.path == path) {
            existing.source = source;
            return;
        }
        self.entries.push(WorkingDirectory { path, source });
    }

    /// `kEn(ctx, dirs, source)` — add every directory under one source.
    pub fn extend_from_source<P: Into<PathBuf>>(
        &mut self,
        dirs: impl IntoIterator<Item = P>,
        source: PermissionRuleSource,
    ) {
        for dir in dirs {
            self.insert(dir, source);
        }
    }

    /// Build from `(dirs, source)` groups, applied in order.
    #[must_use]
    pub fn from_sources<P: Into<PathBuf>>(
        groups: impl IntoIterator<Item = (Vec<P>, PermissionRuleSource)>,
    ) -> Self {
        let mut out = Self::new();
        for (dirs, source) in groups {
            out.extend_from_source(dirs, source);
        }
        out
    }

    /// `Map.delete(path)`.
    pub fn remove(&mut self, path: &Path) {
        self.entries.retain(|entry| entry.path != path);
    }

    /// `Map.has(path)`.
    #[must_use]
    pub fn contains(&self, path: &Path) -> bool {
        self.entries.iter().any(|entry| entry.path == path)
    }

    /// Whether the set is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// How many directories are held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Entries in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = &WorkingDirectory> {
        self.entries.iter()
    }

    /// `Map.keys()` — every path regardless of source. This is the set every
    /// ordinary working-dir consumer wants (the `rb` union minus cwd).
    #[must_use]
    pub fn paths(&self) -> Vec<PathBuf> {
        self.entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect()
    }

    /// `mEt(ctx)` minus cwd — the paths the READ BLOCK honours: everything
    /// EXCEPT what `projectSettings` contributed.
    #[must_use]
    pub fn read_block_paths(&self) -> Vec<PathBuf> {
        self.entries
            .iter()
            .filter(|entry| entry.source != PermissionRuleSource::ProjectSettings)
            .map(|entry| entry.path.clone())
            .collect()
    }
}

impl FromIterator<WorkingDirectory> for AdditionalWorkingDirs {
    fn from_iter<T: IntoIterator<Item = WorkingDirectory>>(iter: T) -> Self {
        let mut out = Self::new();
        for entry in iter {
            out.insert(entry.path, entry.source);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_semantics_key_by_path_last_source_wins() {
        let mut dirs = AdditionalWorkingDirs::new();
        dirs.insert("/a", PermissionRuleSource::ProjectSettings);
        dirs.insert("/b", PermissionRuleSource::CliArg);
        // Re-setting an existing key keeps its POSITION and takes the new source.
        dirs.insert("/a", PermissionRuleSource::Session);
        assert_eq!(dirs.len(), 2);
        assert_eq!(dirs.paths(), vec![PathBuf::from("/a"), PathBuf::from("/b")]);
        assert_eq!(
            dirs.iter().next().unwrap().source,
            PermissionRuleSource::Session
        );
        dirs.remove(Path::new("/a"));
        assert!(!dirs.contains(Path::new("/a")));
    }

    /// 🚨 `mEt` drops `projectSettings` entries; `rb` keeps everything.
    #[test]
    fn read_block_paths_drop_project_settings_only() {
        let dirs = AdditionalWorkingDirs::from_sources([
            (vec!["/from-project"], PermissionRuleSource::ProjectSettings),
            (vec!["/from-cli"], PermissionRuleSource::CliArg),
            (vec!["/from-local"], PermissionRuleSource::LocalSettings),
        ]);
        assert_eq!(dirs.paths().len(), 3);
        assert_eq!(
            dirs.read_block_paths(),
            vec![PathBuf::from("/from-cli"), PathBuf::from("/from-local")]
        );
    }
}

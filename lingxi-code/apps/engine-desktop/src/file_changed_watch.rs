//! File-changed watcher → `FileChanged` hook fire (parity: claude-code
//! `src/utils/hooks/fileChangedWatcher.ts`).
//!
//! claude-code resolves a set of watch paths from the user's `FileChanged` hook
//! config — each hook's `matcher` is a pipe-separated list of filenames to watch
//! in `cwd` (e.g. `".envrc|.env"`, `fileChangedWatcher.ts:48-65`) — opens a
//! chokidar watcher over them (`ignoreInitial`, `awaitWriteFinish`,
//! `:67-78`), and on every debounced `change` / `add` / `unlink` fires the
//! `FileChanged` hook with the path + the chokidar event name
//! (`handleFileEvent`, `:80-106` → `executeFileChangedHooks`,
//! `utils/hooks.ts:4278-4294`). The Rust port had no such watcher; this module
//! adds it at the desktop composition root, where the orchestrator
//! (`orch.hooks`), the `cwd`, and a `FileSystem` are all in scope.
//!
//! ## What this does (and does NOT do)
//! SCOPE is firing the hook only. This watcher resolves the watch paths from the
//! registered `FileChanged` hooks, watches the PARENT directories of those paths
//! via the in-tree `fs_watch` primitive ([`traits::FileSystem::watch`]), and —
//! for every changed path that is one of the resolved watch paths — fires
//! [`FileChangedFirer::fire`] best-effort with the chokidar-style event kind.
//!
//! The "dynamic watch paths" feedback loop (`fileChangedWatcher.ts:108-131`,
//! where a hook's stdout adds more paths and the watcher restarts) and the
//! `onCwdChangedForHooks` re-resolution (`:133-175`) are NOT ported here: those
//! depend on a hook-output `watchPaths` channel and a mid-session cwd-change
//! signal that the Rust port does not yet surface. The static matcher-path
//! resolution — the common case (`.envrc` / `.env` direnv-style watches) — is
//! faithfully reproduced.
//!
//! ## Directory-watch vs file-watch (chokidar parity)
//! chokidar watches the exact file paths; the in-tree primitive
//! ([`platforms/posix::watch_helper`]) watches a DIRECTORY recursively and
//! errors on a missing target. So this watcher watches the deduplicated PARENT
//! directories of the resolved file paths (so a file created after init is still
//! observed — matching chokidar's `add` event under `ignoreInitial`) and fires
//! ONLY for events whose path is in the resolved watch set — the same observable
//! result as chokidar's per-file watch.
//!
//! ## Change-kind mapping (chokidar event names)
//! The in-tree [`traits::FileEventKind`] maps onto the chokidar event strings
//! claude-code fires (`fileChangedWatcher.ts:75-77`):
//! - [`FileEventKind::Created`]  → `"add"`
//! - [`FileEventKind::Modified`] → `"change"`
//! - [`FileEventKind::Deleted`]  → `"unlink"`
//!
//! ## Lifecycle
//! [`FileChangedWatcher::spawn`] starts one background task per watched
//! directory and returns a [`FileChangedWatcherHandle`]. Dropping the handle
//! aborts every task (RAII); the underlying `notify` watcher is released when
//! the [`traits::FileSystem::watch`] stream is dropped — a clean teardown with
//! no lingering OS handles.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures_core::Stream;
use hooks::file_changed_firer::{FileChangedFire, FileChangedFirer};
use tokio::task::JoinHandle;
use tokio_stream::StreamExt;
use traits::{FileEvent, FileEventKind, FileSystem};

/// Map an in-tree [`FileEventKind`] onto the chokidar event-name string
/// claude-code fires (`fileChangedWatcher.ts:75-77`).
#[must_use]
fn kind_to_event(kind: FileEventKind) -> &'static str {
    match kind {
        FileEventKind::Created => "add",
        FileEventKind::Modified => "change",
        FileEventKind::Deleted => "unlink",
    }
}

/// Resolve the set of watch paths from the registered `FileChanged` hooks.
///
/// Mirrors claude-code's `resolveWatchPaths` (`fileChangedWatcher.ts:48-65`):
/// each `FileChanged` hook's `matcher` is a pipe-separated list of filenames;
/// each name is resolved against `cwd` unless it is already absolute. Returns a
/// deduplicated, sorted set (a hook with no matcher contributes nothing — the
/// dynamic-path channel claude-code also folds in is not ported).
#[must_use]
pub fn resolve_watch_paths(matchers: &[&str], cwd: &Path) -> Vec<PathBuf> {
    let mut set: BTreeSet<PathBuf> = BTreeSet::new();
    for m in matchers {
        for name in m.split('|').map(str::trim) {
            if name.is_empty() {
                continue;
            }
            let p = Path::new(name);
            let resolved = if p.is_absolute() {
                p.to_path_buf()
            } else {
                cwd.join(name)
            };
            set.insert(resolved);
        }
    }
    set.into_iter().collect()
}

/// The deduplicated parent directories of `paths` — the targets the in-tree
/// directory watcher actually watches (a file created later is still observed).
/// A path with no parent (e.g. a bare filename) contributes `cwd` as fallback.
#[must_use]
fn watch_dirs_for(paths: &[PathBuf], cwd: &Path) -> Vec<PathBuf> {
    let mut set: BTreeSet<PathBuf> = BTreeSet::new();
    for p in paths {
        match p.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => {
                set.insert(parent.to_path_buf());
            }
            _ => {
                set.insert(cwd.to_path_buf());
            }
        }
    }
    set.into_iter().collect()
}

/// Handle owning the spawned watcher tasks. Dropping it aborts every task
/// (RAII teardown); each aborted task drops its `FileSystem::watch` stream,
/// releasing the underlying `notify` OS handle.
#[derive(Debug)]
pub struct FileChangedWatcherHandle {
    tasks: Vec<JoinHandle<()>>,
}

impl FileChangedWatcherHandle {
    /// An empty handle that owns no tasks (e.g. when no `FileChanged` hook is
    /// configured). Dropping it is a no-op.
    #[must_use]
    pub fn empty() -> Self {
        Self { tasks: Vec::new() }
    }

    /// Number of live watch tasks (one per successfully-watched directory).
    #[must_use]
    pub fn task_count(&self) -> usize {
        self.tasks.len()
    }
}

impl Drop for FileChangedWatcherHandle {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

/// The file-changed watcher. Owns the resolved watch paths + the fire seam and
/// spawns the per-directory watch loops.
pub struct FileChangedWatcher {
    /// The set of file paths to fire for (resolved from the hook matchers).
    watch_paths: Vec<PathBuf>,
    /// The deduplicated parent directories actually watched.
    watch_dirs: Vec<PathBuf>,
    firer: Arc<dyn FileChangedFirer>,
}

impl FileChangedWatcher {
    /// Construct a watcher from the `FileChanged` hook matchers, resolved against
    /// `cwd`, firing through `firer` (the orchestrator firer in production).
    #[must_use]
    pub fn new(matchers: &[&str], cwd: &Path, firer: Arc<dyn FileChangedFirer>) -> Self {
        let watch_paths = resolve_watch_paths(matchers, cwd);
        let watch_dirs = watch_dirs_for(&watch_paths, cwd);
        Self {
            watch_paths,
            watch_dirs,
            firer,
        }
    }

    /// The resolved watch file paths (exposed for tests / diagnostics).
    #[must_use]
    pub fn watch_paths(&self) -> &[PathBuf] {
        &self.watch_paths
    }

    /// The deduplicated parent directories watched (exposed for tests).
    #[must_use]
    pub fn watch_dirs(&self) -> &[PathBuf] {
        &self.watch_dirs
    }

    /// Spawn the background watch loops via the injected `FileSystem` and return
    /// the owning [`FileChangedWatcherHandle`].
    ///
    /// Only directories that currently exist are watched (the in-tree
    /// `watch_dir_with_debounce` errors on a missing target); a directory that
    /// appears later is simply not observed until the next boot — matching
    /// claude-code's init-time path resolution. Best-effort: a directory that
    /// fails to watch is logged and skipped, never fatal. When the resolved
    /// watch-path set is empty (no usable matcher), NO task is spawned — the
    /// no-watch case is byte-identical to claude-code's
    /// `if (paths.length === 0) return` (`fileChangedWatcher.ts:43`).
    pub async fn spawn(self, fs: Arc<dyn FileSystem>) -> FileChangedWatcherHandle {
        let Self {
            watch_paths,
            watch_dirs,
            firer,
        } = self;
        if watch_paths.is_empty() {
            return FileChangedWatcherHandle::empty();
        }
        let watch_paths: Arc<BTreeSet<PathBuf>> = Arc::new(watch_paths.into_iter().collect());
        let mut tasks = Vec::new();
        for dir in watch_dirs {
            if !dir.is_dir() {
                continue;
            }
            let dir_str = dir.to_string_lossy().into_owned();
            let stream = match fs.watch(&dir_str).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(error = %e, dir = %dir_str, "file-changed watch failed");
                    continue;
                }
            };
            let watch_paths = watch_paths.clone();
            let firer = firer.clone();
            tasks.push(tokio::spawn(async move {
                run_watch_loop(stream, watch_paths, firer).await;
            }));
        }
        FileChangedWatcherHandle { tasks }
    }
}

/// Drive one directory's change stream, firing per matching event. Split out so
/// tests can drive it with a synthetic stream (no real `FSEvents`).
pub async fn run_watch_loop(
    mut stream: std::pin::Pin<Box<dyn Stream<Item = FileEvent> + Send>>,
    watch_paths: Arc<BTreeSet<PathBuf>>,
    firer: Arc<dyn FileChangedFirer>,
) {
    while let Some(event) = stream.next().await {
        handle_event(&event, &watch_paths, firer.as_ref()).await;
    }
}

/// Fire the `FileChanged` hook for a single [`FileEvent`] when its path is one
/// of the resolved watch paths. A non-watched path is silently ignored
/// (mirrors chokidar firing only for the exact watched paths). Exposed for
/// deterministic unit tests.
pub async fn handle_event(
    event: &FileEvent,
    watch_paths: &BTreeSet<PathBuf>,
    firer: &dyn FileChangedFirer,
) {
    if !watch_paths.contains(&event.path) {
        return;
    }
    firer
        .fire(FileChangedFire {
            path: event.path.clone(),
            kind: kind_to_event(event.kind).to_string(),
        })
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Recording fake firer — captures every `(path, kind)` the watcher fires so
    /// tests can assert deterministically without a real orchestrator.
    #[derive(Default)]
    struct RecordingFirer {
        fired: Mutex<Vec<(PathBuf, String)>>,
    }

    #[async_trait::async_trait]
    impl FileChangedFirer for RecordingFirer {
        async fn fire(&self, fire: FileChangedFire) {
            self.fired.lock().unwrap().push((fire.path, fire.kind));
        }
    }

    fn ev(path: &str, kind: FileEventKind) -> FileEvent {
        FileEvent {
            path: PathBuf::from(path),
            kind,
        }
    }

    #[test]
    fn resolve_watch_paths_splits_pipes_and_resolves_relative() {
        let cwd = Path::new("/work/proj");
        // Mirrors `.envrc|.env` plus an absolute path; whitespace is trimmed.
        let paths = resolve_watch_paths(&[".envrc | .env", "/etc/abs.conf"], cwd);
        assert!(paths.contains(&PathBuf::from("/work/proj/.envrc")));
        assert!(paths.contains(&PathBuf::from("/work/proj/.env")));
        assert!(paths.contains(&PathBuf::from("/etc/abs.conf")));
        assert_eq!(paths.len(), 3, "deduped + all three resolved");
    }

    #[test]
    fn resolve_watch_paths_dedups_and_skips_empty() {
        let cwd = Path::new("/work");
        // Empty segments (leading/trailing pipe) are skipped; duplicates dedup.
        let paths = resolve_watch_paths(&["|.env|", ".env"], cwd);
        assert_eq!(paths, vec![PathBuf::from("/work/.env")]);
    }

    #[test]
    fn no_matchers_yields_no_watch_paths() {
        // A matcher-less config (or only empty matchers) resolves to nothing —
        // the no-watch case (`if (paths.length === 0) return`).
        assert!(resolve_watch_paths(&[], Path::new("/work")).is_empty());
        assert!(resolve_watch_paths(&["", "  "], Path::new("/work")).is_empty());
    }

    #[test]
    fn watch_dirs_are_parents_deduped() {
        let cwd = Path::new("/work");
        let paths = vec![
            PathBuf::from("/work/.env"),
            PathBuf::from("/work/.envrc"),
            PathBuf::from("/work/sub/cfg.toml"),
        ];
        let dirs = watch_dirs_for(&paths, cwd);
        // `/work` (shared by .env + .envrc) appears once; `/work/sub` once.
        assert!(dirs.contains(&PathBuf::from("/work")));
        assert!(dirs.contains(&PathBuf::from("/work/sub")));
        assert_eq!(dirs.len(), 2);
    }

    #[test]
    fn kind_maps_to_chokidar_event_names() {
        assert_eq!(kind_to_event(FileEventKind::Created), "add");
        assert_eq!(kind_to_event(FileEventKind::Modified), "change");
        assert_eq!(kind_to_event(FileEventKind::Deleted), "unlink");
    }

    #[tokio::test]
    async fn handle_event_fires_only_for_watched_paths_with_mapped_kind() {
        let watch: BTreeSet<PathBuf> =
            [PathBuf::from("/work/.env")].into_iter().collect();
        let firer = RecordingFirer::default();

        // Watched path, Modified → fires "change".
        handle_event(&ev("/work/.env", FileEventKind::Modified), &watch, &firer).await;
        // Watched path, Created → fires "add".
        handle_event(&ev("/work/.env", FileEventKind::Created), &watch, &firer).await;
        // Watched path, Deleted → fires "unlink".
        handle_event(&ev("/work/.env", FileEventKind::Deleted), &watch, &firer).await;
        // Sibling un-watched path in the same dir → no fire.
        handle_event(&ev("/work/other.txt", FileEventKind::Modified), &watch, &firer).await;

        let recorded = firer.fired.lock().unwrap();
        assert_eq!(recorded.len(), 3, "only the three watched-path events fire");
        assert_eq!(recorded[0], (PathBuf::from("/work/.env"), "change".into()));
        assert_eq!(recorded[1], (PathBuf::from("/work/.env"), "add".into()));
        assert_eq!(recorded[2], (PathBuf::from("/work/.env"), "unlink".into()));
    }

    #[tokio::test]
    async fn run_watch_loop_drives_synthetic_stream() {
        // Inject a synthetic change stream (no real FSEvents) and assert the loop
        // fires each watched event then exits cleanly when the stream ends —
        // deterministic, no FS timing dependency.
        let watch: Arc<BTreeSet<PathBuf>> =
            Arc::new([PathBuf::from("/work/.envrc")].into_iter().collect());
        let firer: Arc<RecordingFirer> = Arc::new(RecordingFirer::default());
        let events = vec![
            ev("/work/.envrc", FileEventKind::Modified),
            ev("/work/unrelated.rs", FileEventKind::Modified),
            ev("/work/.envrc", FileEventKind::Deleted),
        ];
        let stream = tokio_stream::iter(events);
        run_watch_loop(Box::pin(stream), watch, firer.clone()).await;

        let recorded = firer.fired.lock().unwrap();
        assert_eq!(recorded.len(), 2, "only the two .envrc events fire");
        assert_eq!(recorded[0].1, "change");
        assert_eq!(recorded[1].1, "unlink");
    }

    #[tokio::test]
    async fn new_resolves_paths_and_dirs() {
        let firer: Arc<dyn FileChangedFirer> = Arc::new(RecordingFirer::default());
        let w = FileChangedWatcher::new(&[".env|.envrc"], Path::new("/work"), firer);
        assert_eq!(
            w.watch_paths(),
            &[PathBuf::from("/work/.env"), PathBuf::from("/work/.envrc")]
        );
        assert_eq!(w.watch_dirs(), &[PathBuf::from("/work")]);
    }
}

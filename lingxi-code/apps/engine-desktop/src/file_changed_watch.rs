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
//! where a fired hook's `hookSpecificOutput.watchPaths` adds more paths and the
//! watcher restarts over the union) IS ported: [`FileChangedFirer::fire`]
//! returns the fired hooks' resolved `watchPaths`, the per-directory watch loop
//! forwards a non-empty set to the supervisor, and the supervisor folds them in
//! and restarts (`updateWatchPaths`, also exposed as
//! [`FileChangedWatcherHandle::update_watch_paths`]).
//!
//! The `onCwdChangedForHooks` re-resolution (`:133-175`, firing `CwdChanged`
//! hooks + re-resolving matchers against a new cwd + restarting) is NOT ported
//! here: it depends on a mid-session cwd-change signal threaded from the Bash
//! tool's persistent-cwd tracking through the orchestrator (the adjacent cwd
//! seam) plus a `CwdChanged`-firer that returns `watchPaths`, neither of which
//! this composition root yet surfaces. The static matcher-path resolution — the
//! common case (`.envrc` / `.env` direnv-style watches) — is faithfully
//! reproduced.
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
//! [`FileChangedWatcher::spawn`] starts a single supervisor task (which owns one
//! per-directory watch loop for each watched directory) and returns a
//! [`FileChangedWatcherHandle`]. Dropping the handle aborts the supervisor,
//! whose owned per-directory tasks each abort via an [`AbortOnDrop`] guard (a
//! bare `JoinHandle` drop would only *detach*), so every `notify` watcher is
//! released — a clean teardown with no lingering OS handles.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures_core::Stream;
use hooks::file_changed_firer::{FileChangedFire, FileChangedFirer};
use tokio::sync::mpsc;
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

/// A spawned per-directory watch task that aborts when dropped.
///
/// A bare [`tokio::task::JoinHandle`] drop only *detaches* the task — it keeps
/// running. Wrapping it guarantees that a watcher restart (or the supervisor's
/// own teardown) actually cancels the old per-directory loop and releases its
/// `notify` OS handle instead of leaking it.
#[derive(Debug)]
struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// A control message to the watcher supervisor.
#[derive(Debug)]
enum WatcherControl {
    /// Fold these (already-resolved, absolute) paths into the watch set and, if
    /// any are new, restart the per-directory loops over the union — the
    /// `updateWatchPaths` seam (claude-code `fileChangedWatcher.ts` `f`, the
    /// dynamic-`watchPaths` feedback loop `:108-131` and the exposed interface
    /// method).
    AddWatchPaths(Vec<PathBuf>),
}

/// Handle owning the spawned watcher supervisor. Dropping it aborts the
/// supervisor, whose owned per-directory tasks each abort via [`AbortOnDrop`],
/// releasing every underlying `notify` OS handle — a clean RAII teardown.
#[derive(Debug)]
pub struct FileChangedWatcherHandle {
    /// Control channel to the supervisor (`None` for an empty handle).
    control: Option<mpsc::UnboundedSender<WatcherControl>>,
    /// The supervisor task; aborting it (on drop) cascades to its dir loops.
    _supervisor: Option<AbortOnDrop>,
}

impl FileChangedWatcherHandle {
    /// An empty handle that owns no tasks (e.g. when no `FileChanged` hook is
    /// configured). Dropping it is a no-op.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            control: None,
            _supervisor: None,
        }
    }

    /// `updateWatchPaths`: add resolved (absolute) paths to the live watch set
    /// and restart over the union when any are new. Mirrors claude-code's
    /// returned-interface `updateWatchPaths` (`fileChangedWatcher.ts`). A no-op
    /// on an empty handle or once the supervisor has exited (the send fails and
    /// is swallowed). The paths must already be absolute — callers resolve
    /// relative matchers against the current cwd before calling.
    pub fn update_watch_paths(&self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        if let Some(tx) = &self.control {
            let _ = tx.send(WatcherControl::AddWatchPaths(paths));
        }
    }
}

/// The file-changed watcher. Owns the resolved watch paths + the fire seam and
/// spawns the supervisor that drives the per-directory watch loops.
pub struct FileChangedWatcher {
    /// The set of file paths to fire for (resolved from the hook matchers).
    watch_paths: Vec<PathBuf>,
    /// The deduplicated parent directories initially watched.
    watch_dirs: Vec<PathBuf>,
    /// The cwd the matchers resolved against — the parent-directory fallback for
    /// paths added later via the dynamic `watchPaths` feedback loop.
    cwd: PathBuf,
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
            cwd: cwd.to_path_buf(),
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

    /// Spawn the supervisor (and its per-directory watch loops) via the injected
    /// `FileSystem` and return the owning [`FileChangedWatcherHandle`].
    ///
    /// Only directories that currently exist are watched (the in-tree
    /// `watch_dir_with_debounce` errors on a missing target); a directory that
    /// appears later is simply not observed until it is (re)added — matching
    /// claude-code's init-time path resolution. Best-effort: a directory that
    /// fails to watch is logged and skipped, never fatal. When the resolved
    /// watch-path set is empty (no usable matcher), NO supervisor is spawned —
    /// the no-watch case is byte-identical to claude-code's
    /// `if (paths.length === 0) return` (`fileChangedWatcher.ts:43`).
    pub async fn spawn(self, fs: Arc<dyn FileSystem>) -> FileChangedWatcherHandle {
        let Self {
            watch_paths,
            watch_dirs: _,
            cwd,
            firer,
        } = self;
        if watch_paths.is_empty() {
            return FileChangedWatcherHandle::empty();
        }
        let (control_tx, control_rx) = mpsc::unbounded_channel();
        let supervisor = Supervisor {
            fs,
            firer,
            control_tx: control_tx.clone(),
            cwd,
            watch_paths: watch_paths.into_iter().collect(),
            dir_tasks: Vec::new(),
        };
        let handle = tokio::spawn(supervisor.run(control_rx));
        FileChangedWatcherHandle {
            control: Some(control_tx),
            _supervisor: Some(AbortOnDrop(handle)),
        }
    }
}

/// The long-lived supervisor: owns the current watch set + the per-directory
/// loops, and restarts them on `updateWatchPaths`. Mirrors the closure state in
/// claude-code's `startFileChangedWatcher` (the watcher `e`, its dispose `y`,
/// and `updateWatchPaths` `f`).
struct Supervisor {
    fs: Arc<dyn FileSystem>,
    firer: Arc<dyn FileChangedFirer>,
    /// Cloned into each watch loop so a fired hook's `watchPaths` can drive a
    /// restart without a back-reference to the handle.
    control_tx: mpsc::UnboundedSender<WatcherControl>,
    cwd: PathBuf,
    watch_paths: BTreeSet<PathBuf>,
    dir_tasks: Vec<AbortOnDrop>,
}

impl Supervisor {
    /// Drive the supervisor: arm the initial watch set, then service control
    /// messages until every sender (the handle + all watch loops) is dropped.
    async fn run(mut self, mut control_rx: mpsc::UnboundedReceiver<WatcherControl>) {
        self.restart().await;
        while let Some(cmd) = control_rx.recv().await {
            match cmd {
                WatcherControl::AddWatchPaths(paths) => {
                    let mut changed = false;
                    for p in paths {
                        if self.watch_paths.insert(p) {
                            changed = true;
                        }
                    }
                    // `if (v.length > 0) updateWatchPaths(v)` restarts the
                    // watcher; a no-op union (all paths already watched) leaves
                    // the live loops untouched (no needless FD churn).
                    if changed {
                        self.restart().await;
                    }
                }
            }
        }
    }

    /// Tear down the current per-directory loops and re-arm them over the
    /// current `watch_paths` set (the parent directories, deduped). Aborting the
    /// old [`AbortOnDrop`] tasks first releases their `notify` handles before the
    /// new set opens — mirroring `dispose` (`y`) then re-`watch`.
    async fn restart(&mut self) {
        // Drop the old loops (each aborts via AbortOnDrop) before re-arming.
        self.dir_tasks.clear();
        let paths: Vec<PathBuf> = self.watch_paths.iter().cloned().collect();
        let dirs = watch_dirs_for(&paths, &self.cwd);
        let watch_set: Arc<BTreeSet<PathBuf>> = Arc::new(self.watch_paths.clone());
        for dir in dirs {
            if !dir.is_dir() {
                continue;
            }
            let dir_str = dir.to_string_lossy().into_owned();
            let stream = match self.fs.watch(&dir_str).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(error = %e, dir = %dir_str, "file-changed watch failed");
                    continue;
                }
            };
            let watch_set = watch_set.clone();
            let firer = self.firer.clone();
            let control_tx = self.control_tx.clone();
            self.dir_tasks.push(AbortOnDrop(tokio::spawn(async move {
                run_watch_loop(stream, watch_set, firer, control_tx).await;
            })));
        }
    }
}

/// Drive one directory's change stream, firing per matching event and forwarding
/// any fired-hook `watchPaths` back to the supervisor for a restart. Split out so
/// tests can drive it with a synthetic stream (no real `FSEvents`).
async fn run_watch_loop(
    mut stream: std::pin::Pin<Box<dyn Stream<Item = FileEvent> + Send>>,
    watch_paths: Arc<BTreeSet<PathBuf>>,
    firer: Arc<dyn FileChangedFirer>,
    control_tx: mpsc::UnboundedSender<WatcherControl>,
) {
    while let Some(event) = stream.next().await {
        let added = handle_event(&event, &watch_paths, firer.as_ref()).await;
        // `if (v.length > 0) updateWatchPaths(v)` — a fired hook that returned
        // `watchPaths` restarts the watcher over the added paths.
        if !added.is_empty() {
            let _ = control_tx.send(WatcherControl::AddWatchPaths(added));
        }
    }
}

/// Fire the `FileChanged` hook for a single [`FileEvent`] when its path is one
/// of the resolved watch paths, returning any `watchPaths` the fired hooks
/// produced (empty when the path is not watched or no hook added paths). A
/// non-watched path is silently ignored (mirrors chokidar firing only for the
/// exact watched paths). Exposed for deterministic unit tests.
async fn handle_event(
    event: &FileEvent,
    watch_paths: &BTreeSet<PathBuf>,
    firer: &dyn FileChangedFirer,
) -> Vec<PathBuf> {
    if !watch_paths.contains(&event.path) {
        return Vec::new();
    }
    firer
        .fire(FileChangedFire {
            path: event.path.clone(),
            kind: kind_to_event(event.kind).to_string(),
        })
        .await
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Recording fake firer — captures every `(path, kind)` the watcher fires so
    /// tests can assert deterministically without a real orchestrator. `respond`
    /// is the set of (already-resolved) `watchPaths` each fire returns, letting a
    /// test drive the dynamic `updateWatchPaths` feedback loop.
    #[derive(Default)]
    struct RecordingFirer {
        fired: Mutex<Vec<(PathBuf, String)>>,
        respond: Vec<PathBuf>,
    }

    impl RecordingFirer {
        fn responding_with(paths: Vec<PathBuf>) -> Self {
            Self {
                fired: Mutex::new(Vec::new()),
                respond: paths,
            }
        }
    }

    #[async_trait::async_trait]
    impl FileChangedFirer for RecordingFirer {
        async fn fire(&self, fire: FileChangedFire) -> Vec<PathBuf> {
            self.fired.lock().unwrap().push((fire.path, fire.kind));
            self.respond.clone()
        }
    }

    fn ev(path: &str, kind: FileEventKind) -> FileEvent {
        FileEvent {
            path: PathBuf::from(path),
            kind,
        }
    }

    /// Minimal fake `FileSystem` for the supervisor restart test: records every
    /// directory `watch` is asked to observe and returns an immediately-ending
    /// empty event stream (the test only observes which directories are watched,
    /// not real FSEvents). Every other method is an inert stub.
    #[derive(Default)]
    struct RecordingFs {
        watched: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait::async_trait]
    impl FileSystem for RecordingFs {
        async fn read_file(
            &self,
            _p: &str,
            _o: Option<u64>,
            _l: Option<u64>,
        ) -> Result<traits::FileContent, traits::FsError> {
            Err(traits::FsError::Io("unused".into()))
        }
        async fn write_file(&self, _p: &str, _c: &str) -> Result<(), traits::FsError> {
            Ok(())
        }
        fn is_within_workspace(&self, _p: &str) -> bool {
            true
        }
        async fn watch(
            &self,
            dir: &str,
        ) -> Result<std::pin::Pin<Box<dyn Stream<Item = FileEvent> + Send>>, traits::FsError>
        {
            self.watched.lock().unwrap().push(dir.to_string());
            Ok(Box::pin(tokio_stream::iter(Vec::<FileEvent>::new())))
        }
        async fn append_file(&self, _p: &str, _c: &str) -> Result<(), traits::FsError> {
            Ok(())
        }
        async fn truncate(&self, _p: &str, _l: u64) -> Result<(), traits::FsError> {
            Ok(())
        }
        async fn file_mtime(&self, _p: &str) -> Result<std::time::SystemTime, traits::FsError> {
            Ok(std::time::SystemTime::UNIX_EPOCH)
        }
        async fn file_size(&self, _p: &str) -> Result<u64, traits::FsError> {
            Ok(0)
        }
        async fn delete_file(&self, _p: &str) -> Result<(), traits::FsError> {
            Ok(())
        }
        async fn symlink(&self, _t: &str, _l: &str) -> Result<(), traits::FsError> {
            Ok(())
        }
        async fn flock_exclusive(
            &self,
            _p: &str,
        ) -> Result<Box<dyn traits::FlockGuard>, traits::FsError> {
            Err(traits::FsError::Io("unused".into()))
        }
        async fn fsync(&self, _p: &str) -> Result<(), traits::FsError> {
            Ok(())
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
        let watch: BTreeSet<PathBuf> = [PathBuf::from("/work/.env")].into_iter().collect();
        let firer = RecordingFirer::default();

        // Watched path, Modified → fires "change".
        handle_event(&ev("/work/.env", FileEventKind::Modified), &watch, &firer).await;
        // Watched path, Created → fires "add".
        handle_event(&ev("/work/.env", FileEventKind::Created), &watch, &firer).await;
        // Watched path, Deleted → fires "unlink".
        handle_event(&ev("/work/.env", FileEventKind::Deleted), &watch, &firer).await;
        // Sibling un-watched path in the same dir → no fire.
        handle_event(
            &ev("/work/other.txt", FileEventKind::Modified),
            &watch,
            &firer,
        )
        .await;

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
        let (control_tx, mut control_rx) = mpsc::unbounded_channel();
        run_watch_loop(Box::pin(stream), watch, firer.clone(), control_tx).await;

        let recorded = firer.fired.lock().unwrap();
        assert_eq!(recorded.len(), 2, "only the two .envrc events fire");
        assert_eq!(recorded[0].1, "change");
        assert_eq!(recorded[1].1, "unlink");
        // A firer that returns no watchPaths never enqueues a restart.
        assert!(
            control_rx.try_recv().is_err(),
            "no AddWatchPaths when the firer returns no watchPaths"
        );
    }

    #[tokio::test]
    async fn handle_event_returns_fired_hook_watch_paths() {
        // A fired hook that returns `watchPaths` surfaces them from handle_event
        // (the dynamic feedback loop's source). A non-watched path returns empty.
        let watch: BTreeSet<PathBuf> = [PathBuf::from("/work/.env")].into_iter().collect();
        let added = vec![PathBuf::from("/work/extra.cfg")];
        let firer = RecordingFirer::responding_with(added.clone());

        let got = handle_event(&ev("/work/.env", FileEventKind::Modified), &watch, &firer).await;
        assert_eq!(got, added, "fired-hook watchPaths flow through handle_event");

        let none = handle_event(
            &ev("/work/unwatched.txt", FileEventKind::Modified),
            &watch,
            &firer,
        )
        .await;
        assert!(none.is_empty(), "non-watched path fires nothing, adds nothing");
    }

    #[tokio::test]
    async fn run_watch_loop_forwards_fired_watch_paths_to_supervisor() {
        // The dynamic `updateWatchPaths` feedback loop: when a fired hook returns
        // watchPaths, the loop enqueues an AddWatchPaths control message.
        let watch: Arc<BTreeSet<PathBuf>> =
            Arc::new([PathBuf::from("/work/.env")].into_iter().collect());
        let added = vec![PathBuf::from("/work/added.cfg")];
        let firer: Arc<RecordingFirer> =
            Arc::new(RecordingFirer::responding_with(added.clone()));
        let stream = tokio_stream::iter(vec![ev("/work/.env", FileEventKind::Modified)]);
        let (control_tx, mut control_rx) = mpsc::unbounded_channel();
        run_watch_loop(Box::pin(stream), watch, firer, control_tx).await;

        match control_rx.try_recv() {
            Ok(WatcherControl::AddWatchPaths(paths)) => assert_eq!(paths, added),
            other => panic!("expected AddWatchPaths({added:?}), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn update_watch_paths_restarts_over_the_new_directory() {
        // End-to-end supervisor restart: spawn over dir A, then push a path in a
        // new dir B via update_watch_paths and assert the fake FS is asked to
        // watch B (the union restart). Real temp dirs so `is_dir()` passes.
        let root = tempfile::tempdir().unwrap();
        let dir_a = root.path().join("a");
        let dir_b = root.path().join("b");
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();

        let fs = Arc::new(RecordingFs::default());
        let watched = fs.watched.clone();
        let firer: Arc<dyn FileChangedFirer> = Arc::new(RecordingFirer::default());

        // Watch a file in dir A initially.
        let a_env = dir_a.join(".env");
        let watcher =
            FileChangedWatcher::new(&[a_env.to_str().unwrap()], root.path(), firer);
        let handle = watcher.spawn(fs as Arc<dyn FileSystem>).await;

        // Wait for the initial watch of dir A.
        let wait_for = |needle: PathBuf| {
            let watched = watched.clone();
            async move {
                for _ in 0..400 {
                    if watched
                        .lock()
                        .unwrap()
                        .iter()
                        .any(|d| Path::new(d) == needle)
                    {
                        return true;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
                false
            }
        };
        assert!(wait_for(dir_a.clone()).await, "dir A watched on spawn");

        // Add a path in dir B → supervisor unions + restarts → dir B watched.
        handle.update_watch_paths(vec![dir_b.join("extra.cfg")]);
        assert!(
            wait_for(dir_b.clone()).await,
            "dir B watched after update_watch_paths restart"
        );

        drop(handle);
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

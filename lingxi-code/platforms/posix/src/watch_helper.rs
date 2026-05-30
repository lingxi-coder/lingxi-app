//! Shared notify + debouncer file-watch helper.
//!
//! Mirrors claude-code's chokidar 4 `awaitWriteFinish` semantics:
//! - `stabilityThreshold = 500ms` (events queued; emitted once writes settle)
//! - `pollInterval = 200ms` (debouncer wake-up rate)
//! - `.git` segment always excluded
//! - Editor swap files (`*.swp`, `~$*`, `4913`) always excluded
//!
//! See: claude-code `src/utils/settings/changeDetector.ts:103-141`,
//! `src/utils/skills/skillChangeDetector.ts:110-131`,
//! `src/utils/hooks/fileChangedWatcher.ts:69-77`.

use futures_core::stream::Stream;
use notify::RecursiveMode;
use notify_debouncer_mini::{new_debouncer, DebouncedEvent, DebouncedEventKind, Debouncer};
use std::path::Path;
use std::pin::Pin;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use traits::{FileEvent, FileEventKind, FsError};

/// Default stability threshold (chokidar 4 parity).
pub const DEFAULT_STABILITY_THRESHOLD_MS: u64 = 500;

/// Default poll interval (chokidar 4 parity).
pub const DEFAULT_POLL_INTERVAL_MS: u64 = 200;

/// Watch `dir` recursively with chokidar-like debounce. Returns a stream of
/// [`FileEvent`]s with `.git/` and editor-swap paths filtered out.
///
/// The watcher is kept alive by the spawned task — when the consumer drops
/// the stream the underlying mpsc channel closes, the task drops the
/// `Debouncer` (RAII), and the OS handle is released.
///
/// `async` is required by the `FileSystem::watch` trait signature even
/// though no `.await` happens before the spawn — keep the signature stable
/// for downstream callers.
#[allow(clippy::unused_async)]
pub async fn watch_dir_with_debounce(
    dir: &str,
    stability_threshold_ms: u64,
    poll_interval_ms: u64,
) -> Result<Pin<Box<dyn Stream<Item = FileEvent> + Send>>, FsError> {
    let dir_path = std::path::PathBuf::from(dir);
    if !dir_path.exists() {
        return Err(FsError::Io(format!("watch target does not exist: {dir}")));
    }

    // mpsc bridge from the (blocking) notify callback into async land.
    let (tx, rx) = mpsc::channel::<FileEvent>(128);

    // Spawn a blocking task to host the debouncer; notify's callback runs on
    // its own thread, so we can't keep the Debouncer on the async runtime
    // directly. The task exits when `tx` is dropped (consumer closed).
    let watch_root = dir_path.clone();
    tokio::task::spawn_blocking(move || {
        let stability = Duration::from_millis(stability_threshold_ms);
        let poll = Duration::from_millis(poll_interval_ms);

        // notify-debouncer-mini 0.6 uses a single `timeout` (stability
        // window). We pick the larger of stability + poll to approximate
        // chokidar's behavior where pollInterval governs *re-checks* until
        // stability is reached.
        let tick = stability.max(poll);

        let (event_tx, event_rx) =
            std::sync::mpsc::channel::<notify_debouncer_mini::DebounceEventResult>();
        let mut debouncer: Debouncer<notify::RecommendedWatcher> =
            match new_debouncer(tick, event_tx) {
                Ok(d) => d,
                Err(e) => {
                    tracing::error!("notify debouncer init failed: {e}");
                    return;
                }
            };

        if let Err(e) = debouncer
            .watcher()
            .watch(&watch_root, RecursiveMode::Recursive)
        {
            tracing::error!("notify watch({:?}) failed: {e}", watch_root);
            return;
        }

        // Pump events until the async receiver closes.
        while let Ok(res) = event_rx.recv() {
            let events = match res {
                Ok(ev) => ev,
                Err(err) => {
                    tracing::warn!("notify error: {err:?}");
                    continue;
                }
            };
            for ev in events {
                if should_skip(&ev.path) {
                    continue;
                }
                // Mirror chokidar 4 `awaitWriteFinish`: only emit a final
                // settled event. `AnyContinuous` indicates writes are still
                // arriving inside the stability window — suppress it so
                // rapid bursts collapse into one terminal event.
                if matches!(ev.kind, DebouncedEventKind::AnyContinuous) {
                    continue;
                }
                let kind = map_kind(&ev);
                if tx
                    .blocking_send(FileEvent {
                        path: ev.path.clone(),
                        kind,
                    })
                    .is_err()
                {
                    // Consumer dropped — exit cleanly.
                    return;
                }
            }
        }
        drop(debouncer); // explicit RAII release
    });

    Ok(Box::pin(ReceiverStream::new(rx)))
}

/// Path filter mirroring chokidar's `ignored` predicate in claude-code.
///
/// - Excludes any path whose components contain a `.git` segment.
/// - Excludes editor swap/lock files: `*.swp` (vim), `~$*` (Word/Excel),
///   `4913` (vim's open-test file).
fn should_skip(path: &Path) -> bool {
    // .git segment anywhere in the path
    if path.components().any(|c| c.as_os_str() == ".git") {
        return true;
    }
    if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
        // vim swap file (case-insensitive: `.swp`, `.SWP`)
        if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("swp"))
        {
            return true;
        }
        if name.starts_with("~$") {
            return true;
        }
        if name == "4913" {
            return true;
        }
    }
    false
}

fn map_kind(ev: &DebouncedEvent) -> FileEventKind {
    // notify-debouncer-mini 0.6 collapses Created/Modified/Removed into
    // a single DebouncedEventKind::Any. We probe the path's existence to
    // distinguish — same approach chokidar uses internally.
    //
    // `DebouncedEventKind` is `#[non_exhaustive]`; AnyContinuous and the
    // catch-all both fall through to `Modified` (most conservative).
    if matches!(ev.kind, DebouncedEventKind::Any) {
        if ev.path.exists() {
            FileEventKind::Modified
        } else {
            FileEventKind::Deleted
        }
    } else {
        FileEventKind::Modified
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn should_skip_dot_git_root() {
        assert!(should_skip(&PathBuf::from("/repo/.git/HEAD")));
        assert!(should_skip(&PathBuf::from(".git/config")));
        assert!(!should_skip(&PathBuf::from("/repo/src/lib.rs")));
    }

    #[test]
    fn should_skip_editor_swap_files() {
        assert!(should_skip(&PathBuf::from("/repo/foo.txt.swp")));
        assert!(should_skip(&PathBuf::from("/repo/~$report.docx")));
        assert!(should_skip(&PathBuf::from("/repo/4913")));
        assert!(!should_skip(&PathBuf::from("/repo/swp_file.txt")));
    }
}

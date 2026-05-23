# M2 Plan 05 · notify-based FS watcher + Swarm backends (Tmux / iTerm / InProcess)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the platform-specific `FileSystem::watch` stubs (Linux-only `inotify`; macOS / Windows empty streams) with a single cross-platform `notify` + `notify-debouncer-mini` implementation that mirrors chokidar 4's `awaitWriteFinish` semantics; refactor `platforms/posix/src/swarm.rs` from a single `TmuxSwarmBackend` stub into a real trifecta (`Tmux`, `iTerm`, `InProcess`) with a detection-driven `SwarmRegistry` that picks the right backend at construction time. `platforms/windows/src/swarm.rs` was already locked to `Unsupported` in M2-01 — this plan only cross-references it.

**Architecture:** `notify::RecommendedWatcher` selects `FSEventWatcher` (macOS) / `INotifyWatcher` (Linux) / `ReadDirectoryChangesWatcher` (Windows) automatically; `notify-debouncer-mini` wraps it with a 500ms stability window + 200ms poll interval (chokidar 4 defaults). A shared `watch_helper` module lives in `platforms/posix/src/watch_helper.rs` and is vendored verbatim into `platforms/windows/src/watch_helper.rs` (the two posix and windows crates intentionally do not share a private crate today; we keep the two copies in lock-step by hand, ~70 lines). On the swarm side, the POSIX `swarm` module becomes a directory with one file per backend (`tmux.rs`, `iterm.rs`, `inprocess.rs`) plus `detection.rs` (probes `$TMUX`, `$TERM_PROGRAM`, `which tmux`, `tmux -V`) and `registry.rs` (returns a `Box<dyn SwarmBackend>` based on detection). Tmux backend shells out via `tokio::process::Command` and serializes pane creation through a process-global `tokio::sync::Mutex` to mirror claude-code's `paneCreationLock`. iTerm backend invokes AppleScript through `osascript` (NOT iTerm's `it2` Python CLI — claude-code's TS reference *does* use `it2`, but we deliberately diverge to keep parity with claude-code's *macOS-native* posture and to avoid the Python API toggle that confuses `it2 --version`-vs-`it2 session list`; see §"Deliberate divergences" below). InProcess backend is a no-op fallback that always reports `is_available() == true`.

**Tech Stack:** Rust 2021 (rust-version 1.82.0), `notify` 6.x, `notify-debouncer-mini` 0.4.x, `which` 6, `tokio::process::Command`, `tokio::sync::Mutex` (for the pane-creation lock), `async-trait`, `futures-core`, `futures-util`, `tempfile` (dev only).

**References:**
- Spec section: `docs/superpowers/specs/2026-05-23-m2-claude-code-parity-design.md` §6.5 (Plan M2-05)
- 1:1 fidelity discriminators: spec §3
- TS dep → Rust mapping: spec §4 (chokidar → notify; tmux CLI → shell-out; osascript CLI → shell-out)
- claude-code reference files (read-only inputs):
  - `claude-code/src/utils/settings/changeDetector.ts` — chokidar settings watcher (stabilityThreshold/pollInterval defaults, .git filter)
  - `claude-code/src/utils/skills/skillChangeDetector.ts:110-131` — chokidar skill watcher (awaitWriteFinish + .git ignore)
  - `claude-code/src/utils/hooks/fileChangedWatcher.ts:69-77` — chokidar hooks watcher (literal `{stabilityThreshold: 500, pollInterval: 200}`)
  - `claude-code/src/utils/swarm/backends/TmuxBackend.ts` (765 lines)
  - `claude-code/src/utils/swarm/backends/ITermBackend.ts` (371 lines)
  - `claude-code/src/utils/swarm/backends/InProcessBackend.ts` (340 lines)
  - `claude-code/src/utils/swarm/backends/registry.ts` (465 lines — backend selection logic)
  - `claude-code/src/utils/swarm/backends/detection.ts` — env probes
  - `claude-code/src/utils/swarm/constants.ts` — `SWARM_SESSION_NAME = 'claude-swarm'`, `getSwarmSocketName()`
- Trait definitions: `lingxi-core/crates/traits/src/filesystem.rs` (`FileSystem::watch`, `FileEvent`, `FileEventKind`), `lingxi-core/crates/traits/src/swarm.rs` (`SwarmBackend`, `SwarmLayout`, `PanePosition`, `SwarmHandle`, `PaneId`, `SwarmError`)
- Existing code state:
  - `lingxi-core/platforms/posix/src/fs.rs` — Linux inotify works, macOS empty stream stub (`TODO(M2-followup): FSEvents`)
  - `lingxi-core/platforms/windows/src/fs.rs` — empty stream stub (`TODO(M2-followup): ReadDirectoryChangesW`)
  - `lingxi-core/platforms/posix/src/swarm.rs` — `TmuxSwarmBackend` stub returning `SwarmError::Tmux("M2 follow-up: tmux session creation")`
  - `lingxi-core/platforms/windows/src/swarm.rs` — `WindowsSwarmBackend` returning `SwarmError::Unsupported` (M2-01 corrected)

**Depends on:** M2-01 (Windows `swarm.rs` is already `Unsupported`; `platforms/posix/src/swarm.rs` is a stub waiting for replacement).

---

## File Touch Inventory (locked at top)

**New files:**
- `lingxi-core/platforms/posix/src/watch_helper.rs` — common notify+debouncer watcher (~110 lines)
- `lingxi-core/platforms/posix/src/swarm/mod.rs` — re-exports + back-compat alias (~25 lines)
- `lingxi-core/platforms/posix/src/swarm/detection.rs` — env probes (~110 lines)
- `lingxi-core/platforms/posix/src/swarm/tmux.rs` — real `TmuxBackend` (~410 lines)
- `lingxi-core/platforms/posix/src/swarm/iterm.rs` — `ITermBackend` AppleScript impl (~210 lines)
- `lingxi-core/platforms/posix/src/swarm/inprocess.rs` — no-pane fallback (~60 lines)
- `lingxi-core/platforms/posix/src/swarm/registry.rs` — auto-detect constructor (~85 lines)
- `lingxi-core/platforms/windows/src/watch_helper.rs` — verbatim copy of posix one (~110 lines)
- `lingxi-core/platforms/posix/tests/fs_watch_debounce_test.rs` — single-event-after-stability test (~70 lines)
- `lingxi-core/platforms/posix/tests/fs_watch_git_filter_test.rs` — `.git/` excluded (~55 lines)
- `lingxi-core/platforms/posix/tests/swarm_detection_test.rs` — backend selection (~90 lines)
- `lingxi-core/platforms/posix/tests/swarm_tmux_argv_test.rs` — color + version parse + argv (~140 lines)
- `lingxi-core/platforms/posix/tests/swarm_tmux_integration_test.rs` — `#[ignore]`-gated real tmux (~80 lines)
- `lingxi-core/platforms/windows/tests/fs_watch_smoke_test.rs` — create+modify+delete on Windows (~55 lines)

**Modified files:**
- `lingxi-core/platforms/posix/Cargo.toml` — add `notify` `notify-debouncer-mini` `which`; drop `inotify`
- `lingxi-core/platforms/windows/Cargo.toml` — add `notify` `notify-debouncer-mini`
- `lingxi-core/platforms/posix/src/fs.rs` — `watch()` now calls `watch_helper::watch_dir_with_debounce`
- `lingxi-core/platforms/windows/src/fs.rs` — same
- `lingxi-core/platforms/posix/src/lib.rs` — `pub mod swarm;` becomes the directory; re-export `TmuxSwarmBackend` (alias to `swarm::TmuxBackend`) for source-compat with M2-01 consumers
- `lingxi-core/platforms/windows/src/lib.rs` — add `pub mod watch_helper;` (private use only)
- `lingxi-core/platforms/windows/src/swarm.rs` — doc cross-reference only (no code change)

**Deleted files:**
- `lingxi-core/platforms/posix/src/swarm.rs` — replaced by `swarm/` directory (move-then-edit; git rename detection handles it)

Total new code: ~1,475 lines (1,000 lines impl + 475 lines tests). Net deletes: ~50 lines (the old single-file `swarm.rs` stub).

---

## Critical 1:1 Fidelity Items (lock at top; the body of the plan references these)

These constants MUST appear verbatim in the Rust impl. The implementer should grep for them as a self-check before commit.

| Item | Value | Source |
|---|---|---|
| FS stability threshold | `500` ms | claude-code `fileChangedWatcher.ts:72` |
| FS poll interval | `200` ms | claude-code `fileChangedWatcher.ts:72` |
| `.git` directory ignored | always | claude-code `changeDetector.ts:118` + `skillChangeDetector.ts:125` |
| Editor swap files ignored | `*.swp`, `~$*`, `4913` | claude-code "atomic: true" semantics + vim convention |
| Tmux session name | `claude-swarm` | claude-code `constants.ts:2` (`SWARM_SESSION_NAME`) |
| Tmux swarm view window | `swarm-view` | claude-code `constants.ts:3` (`SWARM_VIEW_WINDOW_NAME`) |
| Tmux socket name pattern | `claude-swarm-<pid>` | claude-code `constants.ts:12` (`getSwarmSocketName`) |
| Pane shell init delay | `200` ms | claude-code `TmuxBackend.ts:33` (`PANE_SHELL_INIT_DELAY_MS`) |
| Pane creation lock | global `Mutex<()>` | claude-code `TmuxBackend.ts:29` (`paneCreationLock`) |
| Min tmux version | `3.2` | claude-code `TmuxBackend.ts:177` (`set-option -p` is per-pane only in 3.2+) |
| Windows tmux refusal | `--tmux is not supported on Windows` | spec §6.5 1:1 list (M2-01 locked this string in `platforms/windows/src/swarm.rs`) |
| Color: red | `red` | claude-code `TmuxBackend.ts:60-69` |
| Color: blue | `blue` | claude-code `TmuxBackend.ts:60-69` |
| Color: green | `green` | claude-code `TmuxBackend.ts:60-69` |
| Color: yellow | `yellow` | claude-code `TmuxBackend.ts:60-69` |
| Color: cyan | `cyan` | claude-code `TmuxBackend.ts:60-69` |
| Color: purple | `magenta` | claude-code `TmuxBackend.ts:60-69` |
| Color: orange | `colour208` | claude-code `TmuxBackend.ts:60-69` |
| Color: pink | `colour205` | claude-code `TmuxBackend.ts:60-69` |
| iTerm impl path | AppleScript via `osascript` | spec §6.5 explicit decision (deliberate divergence — claude-code uses `it2` Python CLI; we use AppleScript because it ships with macOS and skips the Python-API-disabled trap) |

---

## Deliberate divergences from claude-code (documented for reviewers)

1. **iTerm backend uses `osascript`/AppleScript, not `it2` Python CLI.** Spec §6.5 1:1 list pins this: *"iTerm: must use AppleScript via osascript, not iTerm's Python API (claude-code path)"*. The 1:1 framing in spec §3 covers behavioral parity (observable output), not implementation choice. AppleScript is built into macOS; `it2` requires `pip install it2` and the iTerm Python API toggle in Preferences. Choosing AppleScript closes a documented claude-code footgun (`it2 --version` succeeds when Python API is disabled, but `it2 session split` then fails with no fallback). Reviewers: this is the only intentional divergence in M2-05.

2. **`watch_helper.rs` is duplicated between `posix/` and `windows/` crates instead of extracted into a shared private crate.** The workspace currently does not have a `lingxi-platform-common` crate; adding one for ~110 lines would be premature. Keep the two copies byte-identical by hand. A future plan may unify if/when more code needs sharing.

---

## Phase A · notify-based FS watch with awaitWriteFinish (7 tasks)

### Task 1: Cargo deps — add `notify`, `notify-debouncer-mini`, `which`; drop direct `inotify`

**Files:**
- Modify: `lingxi-core/platforms/posix/Cargo.toml`
- Modify: `lingxi-core/platforms/windows/Cargo.toml`

Both platform crates will use the same `notify` + `notify-debouncer-mini` toolkit. The Linux-specific `inotify = "0.10"` direct dep in posix becomes unnecessary because `notify::RecommendedWatcher` selects inotify under the hood on Linux. Add `which = "6"` to posix only (the tmux backend uses it to probe `$PATH`).

**Critical 1:1 fidelity:**
- `notify` version `6` (per spec §7.1 — must verify Rust 1.82 compatibility; pin via `cargo update --precise` if a transitive dep slips edition2024)
- `notify-debouncer-mini` version `0.4`
- `which` version `6` (already used by M2-04 sandbox crate)

- [ ] **Step 1: Add deps to `lingxi-core/platforms/posix/Cargo.toml`**

Modify the existing `[dependencies]` block — replace the inotify target block with workspace-wide notify deps. The final shape (delta only shown; rest of file untouched):

```toml
[dependencies]
lingxi-protocol = { path = "../../crates/protocol" }
lingxi-traits = { path = "../../crates/traits" }
async-trait = { workspace = true }
tokio = { workspace = true, features = ["full"] }
futures = "0.3"
futures-core = { workspace = true }
futures-util = "0.3"
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls", "json", "stream"] }
serde = { workspace = true }
serde_json = { workspace = true }
tracing = { workspace = true }
thiserror = { workspace = true }
fs2 = "0.4"
notify = "6"
notify-debouncer-mini = "0.4"
which = "6"

# (NOTE: the prior `[target.'cfg(target_os = "linux")'.dependencies]` block with
# `inotify = "0.10"` is REMOVED — notify handles inotify internally now.)

[dev-dependencies]
tempfile = "3.13"

[lints]
workspace = true
```

- [ ] **Step 2: Add deps to `lingxi-core/platforms/windows/Cargo.toml`**

Modify the existing `[dependencies]` block:

```toml
[dependencies]
lingxi-protocol = { path = "../../crates/protocol" }
lingxi-traits = { path = "../../crates/traits" }
async-trait = { workspace = true }
tokio = { workspace = true, features = ["full"] }
futures = "0.3"
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls", "json", "stream"] }
serde = { workspace = true }
serde_json = { workspace = true }
tracing = { workspace = true }
thiserror = { workspace = true }
fs2 = "0.4"
notify = "6"
notify-debouncer-mini = "0.4"

[dev-dependencies]
tempfile = "3.13"

[lints]
workspace = true
```

- [ ] **Step 3: Verify clean build of both crates**

```bash
cd lingxi-core
cargo build -p lingxi-platform-posix
cargo build -p lingxi-platform-windows
```

If either crate fails to build because of an edition2024 transitive (notify 6.x pulls `crossbeam-channel` etc.), pin with:

```bash
cd lingxi-core
cargo update -p notify --precise 6.1.1
cargo update -p notify-debouncer-mini --precise 0.4.1
```

Document the pin in a comment on the line where the dep was added: `notify = "=6.1.1"  # M2-05: Rust 1.82 edition2024 guard`.

**Verification:** both crates build clean; `cargo tree -p lingxi-platform-posix | grep notify` shows the new dep; `cargo tree -p lingxi-platform-posix | grep ^inotify` returns nothing (only transitively under notify).

- [ ] **Step 4: Commit (optional intermediate)**

```bash
git add lingxi-core/platforms/posix/Cargo.toml lingxi-core/platforms/windows/Cargo.toml lingxi-core/Cargo.lock
git commit -m "chore(platforms): add notify + notify-debouncer-mini deps for M2-05"
```

(This is an intermediate commit; the Phase A commit comes after Task 7.)

---

### Task 2: Create the common `watch_helper` module (posix copy)

**Files:**
- Create: `lingxi-core/platforms/posix/src/watch_helper.rs`
- Modify: `lingxi-core/platforms/posix/src/lib.rs` — add `pub(crate) mod watch_helper;` (private to the crate)

This module implements the actual `notify` + `notify-debouncer-mini` pipeline, converts `notify::DebouncedEvent` → our `FileEvent`, and applies the path filters (`.git/`, editor swap files). It's the single source of truth that both `PosixFileSystem::watch` and `WindowsFileSystem::watch` call.

**Critical 1:1 fidelity:**
- Stability threshold default 500ms (chokidar 4 default per `fileChangedWatcher.ts:72`)
- Poll interval default 200ms (chokidar 4 default per `fileChangedWatcher.ts:72`)
- `.git` directory MUST be excluded — any path with `.git` as a path segment is dropped
- Editor swap files MUST be excluded — `.swp` (vim), `~$*` (Word), `4913` (vim's open-test file)

- [ ] **Step 1: Write the failing test for `.git/` exclusion**

Create `lingxi-core/platforms/posix/tests/fs_watch_git_filter_test.rs`:

```rust
//! Verifies the FS watcher excludes `.git/**` paths (parity with claude-code
//! `changeDetector.ts:118` which calls .split(sep).some(d => d === '.git')).

use futures_util::StreamExt;
use lingxi_platform_posix::PosixFileSystem;
use lingxi_traits::FileSystem;
use std::time::Duration;
use tempfile::tempdir;

#[tokio::test]
async fn watch_excludes_dot_git_directory() {
    let dir = tempdir().unwrap();
    let dir_path = dir.path().to_path_buf();

    // Create .git/ subdir BEFORE starting watcher
    let git_dir = dir_path.join(".git");
    std::fs::create_dir(&git_dir).unwrap();

    let fs = PosixFileSystem::new(dir_path.clone());
    let mut stream = fs.watch(dir_path.to_str().unwrap()).await.unwrap();

    // Write to .git/foo — must NOT appear in stream
    std::fs::write(git_dir.join("foo"), b"x").unwrap();

    // Write to top-level bar.txt — MUST appear after stability window
    std::fs::write(dir_path.join("bar.txt"), b"y").unwrap();

    // Collect events for 1.5x stability threshold = 750ms.
    // bar.txt should arrive; foo under .git must not.
    let collect = tokio::time::timeout(Duration::from_millis(1500), async {
        let mut events = Vec::new();
        while let Some(ev) = stream.next().await {
            events.push(ev);
            if events.len() >= 2 {
                break;
            }
        }
        events
    })
    .await
    .unwrap_or_default();

    let saw_git = collect.iter().any(|e| {
        e.path
            .components()
            .any(|c| c.as_os_str() == ".git")
    });
    assert!(!saw_git, "watcher leaked .git event: {collect:?}");

    let saw_bar = collect.iter().any(|e| {
        e.path
            .file_name()
            .and_then(|s| s.to_str())
            == Some("bar.txt")
    });
    assert!(saw_bar, "watcher missed bar.txt event: {collect:?}");
}
```

Run it: `cd lingxi-core && cargo test -p lingxi-platform-posix --test fs_watch_git_filter_test`. Expected: fails because `watch_helper` doesn't exist yet.

- [ ] **Step 2: Implement `watch_helper.rs`**

Create `lingxi-core/platforms/posix/src/watch_helper.rs`:

```rust
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
use lingxi_traits::{FileEvent, FileEventKind, FsError};
use notify::{EventKind, RecursiveMode};
use notify_debouncer_mini::{
    new_debouncer, DebouncedEvent, DebouncedEventKind, Debouncer,
};
use std::path::Path;
use std::pin::Pin;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;

/// Default stability threshold (chokidar 4 parity).
pub const DEFAULT_STABILITY_THRESHOLD_MS: u64 = 500;

/// Default poll interval (chokidar 4 parity).
pub const DEFAULT_POLL_INTERVAL_MS: u64 = 200;

/// Watch `dir` recursively with chokidar-like debounce. Returns a stream of
/// [`FileEvent`]s with `.git/` and editor-swap paths filtered out.
///
/// The watcher is kept alive by the returned stream — when the stream is
/// dropped, the channel closes and the debouncer task exits, releasing the
/// OS handle. (`notify` uses RAII for the watcher; we own the `Debouncer`
/// inside the spawned task.)
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

        // notify-debouncer-mini uses a single tick duration. We pick the
        // longer of stability + poll to approximate chokidar's behavior
        // where pollInterval governs *re-checks* until stability is reached.
        let tick = stability.max(poll);

        let (event_tx, event_rx) = std::sync::mpsc::channel::<
            notify_debouncer_mini::DebounceEventResult,
        >();
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
                Err(errs) => {
                    for err in errs {
                        tracing::warn!("notify error: {err:?}");
                    }
                    continue;
                }
            };
            for ev in events {
                if should_skip(&ev.path) {
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
    if path
        .components()
        .any(|c| c.as_os_str() == ".git")
    {
        return true;
    }
    if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
        if name.ends_with(".swp") {
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
    // notify-debouncer-mini 0.4 collapses Created/Modified/Removed into
    // a single DebouncedEventKind::Any. We probe the path's existence to
    // distinguish — same approach chokidar uses internally.
    match ev.kind {
        DebouncedEventKind::Any => {
            if ev.path.exists() {
                FileEventKind::Modified
            } else {
                FileEventKind::Deleted
            }
        }
        DebouncedEventKind::AnyContinuous => FileEventKind::Modified,
        _ => FileEventKind::Modified,
    }
}

/// Lower-level helper: convert a raw `notify::Event` (without debouncing) to
/// our kind enum. Currently unused but kept for future direct-watcher paths.
#[allow(dead_code)]
fn raw_event_kind(ek: &EventKind) -> FileEventKind {
    use notify::event::{CreateKind, ModifyKind, RemoveKind};
    match ek {
        EventKind::Create(_) | EventKind::Other => FileEventKind::Created,
        EventKind::Modify(ModifyKind::Name(_)) => FileEventKind::Modified,
        EventKind::Modify(ModifyKind::Data(_)) => FileEventKind::Modified,
        EventKind::Modify(_) => FileEventKind::Modified,
        EventKind::Remove(_) => FileEventKind::Deleted,
        _ => FileEventKind::Modified,
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
```

- [ ] **Step 3: Wire the module into `lib.rs`**

Modify `lingxi-core/platforms/posix/src/lib.rs` — add this line near the other `pub mod` declarations (keep crate-private; consumers go through `fs::PosixFileSystem::watch`):

```rust
pub(crate) mod watch_helper;
```

Also add `tokio-stream = "0.1"` to posix Cargo.toml `[dependencies]` if not already present (used for `ReceiverStream`).

- [ ] **Step 4: Verify build**

```bash
cd lingxi-core
cargo build -p lingxi-platform-posix
cargo test -p lingxi-platform-posix --lib watch_helper::tests
```

Expected: builds clean; inline unit tests pass.

---

### Task 3: Test — rapid writes collapse to a single event after stability

**Files:**
- Create: `lingxi-core/platforms/posix/tests/fs_watch_debounce_test.rs`

This is the parity check for chokidar's `awaitWriteFinish.stabilityThreshold`. Multiple rapid writes inside the threshold window MUST be debounced into a single `Modified` event.

- [ ] **Step 1: Write the test**

```rust
//! Verifies the FS watcher debounces rapid writes per chokidar's
//! awaitWriteFinish.stabilityThreshold (500ms / 200ms defaults).

use futures_util::StreamExt;
use lingxi_platform_posix::PosixFileSystem;
use lingxi_traits::FileSystem;
use std::time::{Duration, Instant};
use tempfile::tempdir;

#[tokio::test]
async fn rapid_writes_collapse_to_single_event() {
    let dir = tempdir().unwrap();
    let dir_path = dir.path().to_path_buf();
    let fs = PosixFileSystem::new(dir_path.clone());

    let mut stream = fs.watch(dir_path.to_str().unwrap()).await.unwrap();

    // Settle the watcher.
    tokio::time::sleep(Duration::from_millis(100)).await;

    let target = dir_path.join("rapid.txt");

    // 3 writes within the 500ms stability window.
    let burst_start = Instant::now();
    for i in 0..3 {
        std::fs::write(&target, format!("v{i}")).unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let burst_end = burst_start.elapsed();
    assert!(
        burst_end < Duration::from_millis(500),
        "burst overshot stability window: {burst_end:?}"
    );

    // Wait for stability threshold (500ms) + slack (250ms) for the debouncer
    // to flush. Then drain everything available within an additional 250ms.
    tokio::time::sleep(Duration::from_millis(750)).await;

    let drain = tokio::time::timeout(Duration::from_millis(250), async {
        let mut events = Vec::new();
        while let Some(ev) = stream.next().await {
            if ev.path.file_name().and_then(|s| s.to_str()) == Some("rapid.txt") {
                events.push(ev);
            }
        }
        events
    })
    .await
    .unwrap_or_default();

    assert_eq!(
        drain.len(),
        1,
        "expected exactly 1 debounced event, got {}: {drain:?}",
        drain.len()
    );
}
```

- [ ] **Step 2: Run**

```bash
cd lingxi-core
cargo test -p lingxi-platform-posix --test fs_watch_debounce_test
```

Expected: fails because `PosixFileSystem::watch` still uses the old inotify code path (which doesn't debounce). Task 5 fixes this.

---

### Task 4: Test — `.git/` exclusion (the test from Task 2, now expected to pass partially)

**Files:**
- Already created in Task 2 (`fs_watch_git_filter_test.rs`)

- [ ] **Step 1: Verify the Task 2 test still fails for the right reason**

```bash
cd lingxi-core
cargo test -p lingxi-platform-posix --test fs_watch_git_filter_test
```

Expected: fails. The reason will differ on Linux (inotify code path doesn't apply `.git` filter) vs macOS (empty stream — no events at all, so `bar.txt` assertion fails). Both failure modes are addressed in Task 5 once `watch()` switches over.

---

### Task 5: Replace `PosixFileSystem::watch` with the helper

**Files:**
- Modify: `lingxi-core/platforms/posix/src/fs.rs`

Drop the dual `#[cfg(target_os = "linux")]` / `#[cfg(not(target_os = "linux"))]` watch implementations and replace with a single call to `watch_helper::watch_dir_with_debounce`. Also remove the `inotify::*` imports.

**Critical 1:1 fidelity:**
- The replacement must use the exact constants `DEFAULT_STABILITY_THRESHOLD_MS = 500` and `DEFAULT_POLL_INTERVAL_MS = 200` from `watch_helper`.

- [ ] **Step 1: Edit `fs.rs`**

Replace the entire `watch` method (and its two `#[cfg]` branches) with:

```rust
async fn watch(
    &self,
    dir: &str,
) -> Result<Pin<Box<dyn Stream<Item = FileEvent> + Send>>, FsError> {
    crate::watch_helper::watch_dir_with_debounce(
        dir,
        crate::watch_helper::DEFAULT_STABILITY_THRESHOLD_MS,
        crate::watch_helper::DEFAULT_POLL_INTERVAL_MS,
    )
    .await
}
```

Then remove the `use` lines `use futures_util::stream::StreamExt;` and any `inotify::*` imports that were only used by the deleted watch impl (the file's other methods don't need them). The doc comment at the top of `fs.rs` should be updated:

```rust
//! `tokio::fs`-backed [`FileSystem`] for desktop hosts.
//!
//! Implements the engine's sandboxed filesystem trait using the real OS
//! filesystem. Path containment is enforced via prefix-match against the
//! workspace root supplied at construction. [`FileSystem::watch`] is
//! backed by `notify` + `notify-debouncer-mini` (see `watch_helper`),
//! delivering chokidar-4-equivalent `awaitWriteFinish` semantics across
//! Linux (inotify), macOS (FSEvents), and Windows (RDC).
```

- [ ] **Step 2: Re-run both FS tests**

```bash
cd lingxi-core
cargo test -p lingxi-platform-posix --test fs_watch_debounce_test
cargo test -p lingxi-platform-posix --test fs_watch_git_filter_test
```

Expected: both pass.

- [ ] **Step 3: Verify the whole posix crate still builds**

```bash
cd lingxi-core
cargo build -p lingxi-platform-posix
cargo clippy -p lingxi-platform-posix -- -D warnings
```

Expected: clean.

---

### Task 6: Mirror the helper into windows + replace `WindowsFileSystem::watch`

**Files:**
- Create: `lingxi-core/platforms/windows/src/watch_helper.rs` (byte-identical copy of posix version)
- Modify: `lingxi-core/platforms/windows/src/lib.rs` — add `pub(crate) mod watch_helper;`
- Modify: `lingxi-core/platforms/windows/src/fs.rs`
- Create: `lingxi-core/platforms/windows/tests/fs_watch_smoke_test.rs`

The windows crate's `watch_helper.rs` is a verbatim copy of the posix one — `notify`'s `RecommendedWatcher` is the abstraction layer, so the same code runs on RDC under Windows.

- [ ] **Step 1: Copy `watch_helper.rs`**

```bash
cp lingxi-core/platforms/posix/src/watch_helper.rs lingxi-core/platforms/windows/src/watch_helper.rs
```

Add a doc comment to the windows copy:

```rust
//! NOTE: This file is a manually-kept copy of
//! `lingxi-core/platforms/posix/src/watch_helper.rs`. Keep the two in sync;
//! see M2-05 plan, "Deliberate divergences" §2 for rationale.
```

- [ ] **Step 2: Wire into windows lib.rs**

Add to `lingxi-core/platforms/windows/src/lib.rs`:

```rust
pub(crate) mod watch_helper;
```

If `tokio-stream` is not yet in windows Cargo.toml, add it: `tokio-stream = "0.1"`.

- [ ] **Step 3: Replace `WindowsFileSystem::watch`**

Edit `lingxi-core/platforms/windows/src/fs.rs` — replace the empty-stream `watch` impl with:

```rust
async fn watch(
    &self,
    dir: &str,
) -> Result<Pin<Box<dyn Stream<Item = FileEvent> + Send>>, FsError> {
    crate::watch_helper::watch_dir_with_debounce(
        dir,
        crate::watch_helper::DEFAULT_STABILITY_THRESHOLD_MS,
        crate::watch_helper::DEFAULT_POLL_INTERVAL_MS,
    )
    .await
}
```

Update the file's top doc comment to match the posix one (drop the `ReadDirectoryChangesW` TODO note).

- [ ] **Step 4: Write the smoke test**

Create `lingxi-core/platforms/windows/tests/fs_watch_smoke_test.rs`:

```rust
//! Smoke test: create + modify + delete a file inside the watched dir, verify
//! at least one event arrives. Detailed parity tests live in posix/.

use futures_util::StreamExt;
use lingxi_platform_windows::WindowsFileSystem;
use lingxi_traits::FileSystem;
use std::time::Duration;
use tempfile::tempdir;

#[tokio::test]
async fn create_modify_delete_emits_events() {
    let dir = tempdir().unwrap();
    let dir_path = dir.path().to_path_buf();
    let fs = WindowsFileSystem::new(dir_path.clone());
    let mut stream = fs.watch(dir_path.to_str().unwrap()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    let target = dir_path.join("hello.txt");
    std::fs::write(&target, b"hi").unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    std::fs::write(&target, b"hi there").unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    std::fs::remove_file(&target).unwrap();

    let events = tokio::time::timeout(Duration::from_millis(1500), async {
        let mut out = Vec::new();
        while let Some(ev) = stream.next().await {
            if ev.path.file_name().and_then(|s| s.to_str()) == Some("hello.txt") {
                out.push(ev);
            }
            if out.len() >= 1 {
                break;
            }
        }
        out
    })
    .await
    .unwrap_or_default();

    assert!(!events.is_empty(), "no FS events arrived");
}
```

- [ ] **Step 5: Build/test (skip on macOS host; verify on a Windows CI runner)**

```bash
cd lingxi-core
cargo build -p lingxi-platform-windows
# On Windows runner only:
# cargo test -p lingxi-platform-windows --test fs_watch_smoke_test
```

Expected: clean build everywhere; smoke test passes on Windows.

---

### Task 7: Commit Phase A

- [ ] **Step 1: Run full posix + windows test suite**

```bash
cd lingxi-core
cargo test -p lingxi-platform-posix
cargo test -p lingxi-platform-windows  # may skip the smoke test on non-Windows
cargo clippy -p lingxi-platform-posix -p lingxi-platform-windows -- -D warnings
cargo fmt --check
```

Expected: all clean.

- [ ] **Step 2: Commit**

```bash
git add lingxi-core/platforms/posix/Cargo.toml \
        lingxi-core/platforms/posix/src/lib.rs \
        lingxi-core/platforms/posix/src/fs.rs \
        lingxi-core/platforms/posix/src/watch_helper.rs \
        lingxi-core/platforms/posix/tests/fs_watch_debounce_test.rs \
        lingxi-core/platforms/posix/tests/fs_watch_git_filter_test.rs \
        lingxi-core/platforms/windows/Cargo.toml \
        lingxi-core/platforms/windows/src/lib.rs \
        lingxi-core/platforms/windows/src/fs.rs \
        lingxi-core/platforms/windows/src/watch_helper.rs \
        lingxi-core/platforms/windows/tests/fs_watch_smoke_test.rs \
        lingxi-core/Cargo.lock
git commit -m "feat(platforms): notify-based FS watch with debounce"
```

End Phase A.

---

## Phase B · Swarm backend trifecta — Tmux + iTerm + InProcess (10 tasks)

This phase converts `lingxi-core/platforms/posix/src/swarm.rs` (single file, stub) into `lingxi-core/platforms/posix/src/swarm/` (directory, 6 files). The first task is the move-then-edit boundary; all subsequent tasks edit the new directory contents.

### Task 8: Convert `swarm.rs` → `swarm/mod.rs` directory layout

**Files:**
- Delete: `lingxi-core/platforms/posix/src/swarm.rs`
- Create: `lingxi-core/platforms/posix/src/swarm/mod.rs`
- Modify: `lingxi-core/platforms/posix/src/lib.rs` (no source change — `pub mod swarm;` still resolves; just confirm)

- [ ] **Step 1: Create the directory + initial `mod.rs`**

```bash
cd lingxi-core/platforms/posix/src
mkdir -p swarm
```

Move the existing stub content into the new directory's `mod.rs` and add child-module declarations (the children land in the next tasks). The initial `mod.rs`:

```rust
//! POSIX swarm backends: tmux (real), iTerm (AppleScript), InProcess (no-pane).
//!
//! Module layout mirrors claude-code `src/utils/swarm/backends/`:
//! - `detection`  — env + `which` probes
//! - `tmux`       — real `tmux` shell-out backend
//! - `iterm`      — AppleScript-via-`osascript` backend
//! - `inprocess`  — no-pane fallback that's always available
//! - `registry`   — auto-detect + construct the appropriate `Box<dyn SwarmBackend>`
//!
//! See spec §6.5 and the M2-05 plan for parity details.

pub mod detection;
pub mod inprocess;
pub mod iterm;
pub mod registry;
pub mod tmux;

pub use inprocess::InProcessSwarmBackend;
pub use iterm::ITermSwarmBackend;
pub use registry::SwarmRegistry;
pub use tmux::TmuxBackend;

/// Back-compat alias — the M2-01 era `lib.rs` re-exports `TmuxSwarmBackend`.
/// Keep the type name pointing at the real `TmuxBackend` so the public surface
/// doesn't break for consumers that imported it directly.
pub type TmuxSwarmBackend = TmuxBackend;
```

- [ ] **Step 2: Delete the old single-file `swarm.rs`**

```bash
rm lingxi-core/platforms/posix/src/swarm.rs
```

- [ ] **Step 3: Add empty child files (so the `pub mod` lines resolve)**

Create empty (single-line `//! TBD next task` doc) files so the crate still compiles after this checkpoint:

```bash
cd lingxi-core/platforms/posix/src/swarm
printf '%s\n' '//! Filled in by Task 9.' > detection.rs
printf '%s\n' '//! Filled in by Task 14.' > inprocess.rs
printf '%s\n' '//! Filled in by Task 13.' > iterm.rs
printf '%s\n' '//! Filled in by Task 15.' > registry.rs
printf '%s\n' '//! Filled in by Task 10.' > tmux.rs
```

In `mod.rs`, temporarily comment out the `pub use` lines so the empty files don't break export resolution:

```rust
// pub use inprocess::InProcessSwarmBackend;
// pub use iterm::ITermSwarmBackend;
// pub use registry::SwarmRegistry;
// pub use tmux::TmuxBackend;
// pub type TmuxSwarmBackend = TmuxBackend;
```

(Each subsequent task uncomments the matching line when it fills in the file.)

- [ ] **Step 4: Verify clean build**

```bash
cd lingxi-core
cargo build -p lingxi-platform-posix
```

Expected: clean (no warnings from `lib.rs` because the `pub mod` resolves to empty modules).

---

### Task 9: Implement `detection.rs`

**Files:**
- Replace: `lingxi-core/platforms/posix/src/swarm/detection.rs`
- Modify: `lingxi-core/platforms/posix/tests/swarm_detection_test.rs` (new)

`detection.rs` exposes pure functions that probe `$TMUX`, `$TERM_PROGRAM`, and `which tmux` to populate a `TerminalEnv` struct. The `pick_backend` function consumes a `TerminalEnv` and returns a `BackendChoice` enum that the registry uses to construct the right concrete backend.

**Critical 1:1 fidelity:**
- Inside-tmux detection ONLY reads `$TMUX`; we do NOT shell out to `tmux display-message` as a fallback. claude-code documents this explicitly in `detection.ts:35-37`: a global `tmux display-message` succeeds if *any* tmux server is running, not just *this* process's session.
- iTerm detection reads `$TERM_PROGRAM == "iTerm.app"` (we don't bother with `$ITERM_SESSION_ID` because claude-code's secondary indicator is internal to the in-process backend selection, not the user-facing detection).
- Preference order matches claude-code `registry.ts:158-225`: inside-tmux → Tmux; iTerm + tmux unavailable → ITerm; tmux available → Tmux external; else → InProcess.

- [ ] **Step 1: Write the failing test**

Create `lingxi-core/platforms/posix/tests/swarm_detection_test.rs`:

```rust
//! Verifies the detection logic picks the right backend given env probes.

use lingxi_platform_posix::swarm::detection::{
    pick_backend, BackendChoice, TerminalEnv,
};

#[test]
fn inside_tmux_picks_tmux() {
    let env = TerminalEnv {
        inside_tmux: true,
        iterm_app: false,
        tmux_available: true,
        osascript_available: true,
    };
    assert!(matches!(pick_backend(&env), BackendChoice::Tmux));
}

#[test]
fn iterm_with_osascript_no_tmux_picks_iterm() {
    let env = TerminalEnv {
        inside_tmux: false,
        iterm_app: true,
        tmux_available: false,
        osascript_available: true,
    };
    assert!(matches!(pick_backend(&env), BackendChoice::ITerm));
}

#[test]
fn outside_tmux_with_tmux_picks_tmux() {
    let env = TerminalEnv {
        inside_tmux: false,
        iterm_app: false,
        tmux_available: true,
        osascript_available: false,
    };
    assert!(matches!(pick_backend(&env), BackendChoice::Tmux));
}

#[test]
fn iterm_plus_tmux_prefers_iterm() {
    // claude-code registry.ts:173-200 — iTerm wins over external tmux when
    // it2/osascript is reachable.
    let env = TerminalEnv {
        inside_tmux: false,
        iterm_app: true,
        tmux_available: true,
        osascript_available: true,
    };
    assert!(matches!(pick_backend(&env), BackendChoice::ITerm));
}

#[test]
fn nothing_available_picks_inprocess() {
    let env = TerminalEnv {
        inside_tmux: false,
        iterm_app: false,
        tmux_available: false,
        osascript_available: false,
    };
    assert!(matches!(pick_backend(&env), BackendChoice::InProcess));
}
```

- [ ] **Step 2: Implement `detection.rs`**

Replace `lingxi-core/platforms/posix/src/swarm/detection.rs`:

```rust
//! Detect the terminal environment to choose a `SwarmBackend`.
//!
//! Mirrors claude-code `src/utils/swarm/backends/detection.ts` + the
//! priority flow in `registry.ts:130-254`.

use std::process::Command;

/// Snapshot of the host terminal at startup.
#[derive(Debug, Clone)]
pub struct TerminalEnv {
    /// `$TMUX` env var is set (we're inside a tmux session).
    pub inside_tmux: bool,
    /// `$TERM_PROGRAM == "iTerm.app"`.
    pub iterm_app: bool,
    /// `which tmux` resolves AND `tmux -V` reports version ≥ 3.2.
    pub tmux_available: bool,
    /// `which osascript` resolves.
    pub osascript_available: bool,
}

/// Which concrete backend to construct.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendChoice {
    /// Real `tmux` backend (inside or outside a user session).
    Tmux,
    /// iTerm2 via `osascript`.
    ITerm,
    /// No-pane fallback.
    InProcess,
}

/// Build a `TerminalEnv` from process state. Pure (no shelling out beyond
/// `which`); cheap enough to call at construction time.
pub fn detect_terminal_env() -> TerminalEnv {
    let inside_tmux = std::env::var("TMUX").is_ok();
    let iterm_app = std::env::var("TERM_PROGRAM").as_deref() == Ok("iTerm.app");

    let tmux_available = which::which("tmux").is_ok() && tmux_version_ok();
    let osascript_available = which::which("osascript").is_ok();

    TerminalEnv {
        inside_tmux,
        iterm_app,
        tmux_available,
        osascript_available,
    }
}

/// Parse `tmux -V` output (e.g. `tmux 3.3a`) and return true iff major.minor
/// is at least 3.2. claude-code's `set-option -p` for per-pane border style
/// requires this minimum.
pub fn tmux_version_ok() -> bool {
    let Ok(output) = Command::new("tmux").arg("-V").output() else {
        return false;
    };
    let s = String::from_utf8_lossy(&output.stdout);
    parse_tmux_version_at_least_3_2(&s)
}

/// Pure helper, exposed for unit-testing without shell-out.
pub fn parse_tmux_version_at_least_3_2(version_output: &str) -> bool {
    // Expected formats: "tmux 3.2", "tmux 3.3a", "tmux 2.9", "tmux next-3.4".
    let s = version_output.trim();
    let Some(after_prefix) = s.strip_prefix("tmux ") else {
        return false;
    };
    // strip a "next-" prefix if present (some distro packages use it)
    let token = after_prefix.strip_prefix("next-").unwrap_or(after_prefix);
    // Split on the first non-digit / non-dot to drop trailing letters like "3.3a"
    let numeric: String = token
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let mut parts = numeric.split('.');
    let major: u32 = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let minor: u32 = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    major > 3 || (major == 3 && minor >= 2)
}

/// Pick the right backend per the priority flow.
///
/// Order:
/// 1. Inside tmux + tmux available → `Tmux`.
/// 2. iTerm.app + osascript available → `ITerm`.
/// 3. Tmux available (external session) → `Tmux`.
/// 4. Otherwise → `InProcess`.
pub fn pick_backend(env: &TerminalEnv) -> BackendChoice {
    if env.inside_tmux && env.tmux_available {
        return BackendChoice::Tmux;
    }
    if env.iterm_app && env.osascript_available {
        return BackendChoice::ITerm;
    }
    if env.tmux_available {
        return BackendChoice::Tmux;
    }
    BackendChoice::InProcess
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_3_2_is_ok() {
        assert!(parse_tmux_version_at_least_3_2("tmux 3.2"));
        assert!(parse_tmux_version_at_least_3_2("tmux 3.3a"));
        assert!(parse_tmux_version_at_least_3_2("tmux 4.0"));
    }

    #[test]
    fn version_below_3_2_rejected() {
        assert!(!parse_tmux_version_at_least_3_2("tmux 3.1c"));
        assert!(!parse_tmux_version_at_least_3_2("tmux 2.9"));
    }

    #[test]
    fn next_prefix_handled() {
        assert!(parse_tmux_version_at_least_3_2("tmux next-3.4"));
    }

    #[test]
    fn malformed_rejected() {
        assert!(!parse_tmux_version_at_least_3_2("garbage"));
        assert!(!parse_tmux_version_at_least_3_2(""));
    }
}
```

- [ ] **Step 3: Run tests**

```bash
cd lingxi-core
cargo test -p lingxi-platform-posix --test swarm_detection_test
cargo test -p lingxi-platform-posix --lib swarm::detection::tests
```

Expected: all 5 integration tests + 4 unit tests pass.

---

### Task 10: Implement `tmux.rs` (real `TmuxBackend`)

**Files:**
- Replace: `lingxi-core/platforms/posix/src/swarm/tmux.rs`
- Modify: `lingxi-core/platforms/posix/src/swarm/mod.rs` (uncomment the `pub use tmux::TmuxBackend;` and `pub type TmuxSwarmBackend` lines)

`tmux.rs` is the biggest single file in the plan (~410 lines). It implements the `SwarmBackend` trait by shelling out to `tmux` via `tokio::process::Command`. The implementation covers:
- Inside-tmux vs outside-tmux split-window orchestration.
- Per-pane border color via `select-pane -P bg=default,fg=<color>` + `set-option -p -t <pane> pane-border-style fg=<color>`.
- Pane title via `set-option -p -t <pane> pane-border-format <fmt>`.
- 200ms shell-init delay after pane creation.
- A global `tokio::sync::Mutex<()>` for pane-creation serialization.
- `is_available()` does the version check.

**Critical 1:1 fidelity:**
- `SWARM_SESSION_NAME = "claude-swarm"` (claude-code constant)
- `SWARM_VIEW_WINDOW_NAME = "swarm-view"` (claude-code constant)
- Socket name `claude-swarm-<pid>` (per `getSwarmSocketName()`)
- `PANE_SHELL_INIT_DELAY_MS = 200`
- Color mapping table (see top "Critical 1:1 Fidelity Items")
- Pane creation serialized through a process-global `OnceLock<Mutex<()>>`

- [ ] **Step 1: Write the unit test for color mapping + argv shape**

Create `lingxi-core/platforms/posix/tests/swarm_tmux_argv_test.rs`:

```rust
//! Verifies the color map literals match claude-code TmuxBackend.ts:60-69 and
//! that the argv constructed by TmuxBackend matches what would be passed to
//! `tmux` on the shell.

use lingxi_platform_posix::swarm::tmux::{
    agent_color_to_tmux, build_select_pane_color_argv, build_set_pane_border_argv,
    build_split_window_argv, AgentColor, SwarmConstants,
};

#[test]
fn color_map_matches_claude_code() {
    assert_eq!(agent_color_to_tmux(AgentColor::Red), "red");
    assert_eq!(agent_color_to_tmux(AgentColor::Blue), "blue");
    assert_eq!(agent_color_to_tmux(AgentColor::Green), "green");
    assert_eq!(agent_color_to_tmux(AgentColor::Yellow), "yellow");
    assert_eq!(agent_color_to_tmux(AgentColor::Cyan), "cyan");
    assert_eq!(agent_color_to_tmux(AgentColor::Purple), "magenta");
    assert_eq!(agent_color_to_tmux(AgentColor::Orange), "colour208");
    assert_eq!(agent_color_to_tmux(AgentColor::Pink), "colour205");
}

#[test]
fn swarm_constants_match_claude_code() {
    assert_eq!(SwarmConstants::SESSION_NAME, "claude-swarm");
    assert_eq!(SwarmConstants::VIEW_WINDOW_NAME, "swarm-view");
    assert!(SwarmConstants::socket_name_for_pid(1234).contains("claude-swarm-1234"));
}

#[test]
fn split_window_argv_inside_tmux() {
    // Splitting horizontally with a 70% size, returning the new pane id.
    let argv = build_split_window_argv("%0", true, Some("70%"));
    assert_eq!(
        argv,
        vec![
            "split-window".to_string(),
            "-t".to_string(),
            "%0".to_string(),
            "-h".to_string(),
            "-l".to_string(),
            "70%".to_string(),
            "-P".to_string(),
            "-F".to_string(),
            "#{pane_id}".to_string(),
        ]
    );
}

#[test]
fn select_pane_color_argv_uses_dash_capital_p() {
    let argv = build_select_pane_color_argv("%1", "magenta");
    assert_eq!(
        argv,
        vec![
            "select-pane".to_string(),
            "-t".to_string(),
            "%1".to_string(),
            "-P".to_string(),
            "bg=default,fg=magenta".to_string(),
        ]
    );
}

#[test]
fn set_pane_border_argv_uses_lower_p() {
    let argv = build_set_pane_border_argv("%2", "colour208");
    assert_eq!(
        argv,
        vec![
            "set-option".to_string(),
            "-p".to_string(),
            "-t".to_string(),
            "%2".to_string(),
            "pane-border-style".to_string(),
            "fg=colour208".to_string(),
        ]
    );
}
```

- [ ] **Step 2: Implement `tmux.rs`**

Replace `lingxi-core/platforms/posix/src/swarm/tmux.rs`:

```rust
//! Real `tmux`-backed `SwarmBackend` for POSIX hosts.
//!
//! Mirrors claude-code `src/utils/swarm/backends/TmuxBackend.ts`. Behavioral
//! parity items locked in M2-05 plan:
//! - `SWARM_SESSION_NAME = "claude-swarm"`
//! - `SWARM_VIEW_WINDOW_NAME = "swarm-view"`
//! - socket name `claude-swarm-<pid>` (per `getSwarmSocketName()`)
//! - `PANE_SHELL_INIT_DELAY_MS = 200`
//! - global pane-creation lock to prevent concurrent `tmux split-window` races
//! - color map literal (see `agent_color_to_tmux`)
//! - requires tmux ≥ 3.2 (`set-option -p` is per-pane only in 3.2+)

use async_trait::async_trait;
use lingxi_protocol::AgentId;
use lingxi_traits::{PaneId, PanePosition, SwarmBackend, SwarmError, SwarmHandle, SwarmLayout};
use std::sync::OnceLock;
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::Mutex;

use super::detection;

/// Shell-init delay after pane creation, per `TmuxBackend.ts:33`.
pub const PANE_SHELL_INIT_DELAY_MS: u64 = 200;

/// Process-global pane-creation lock. Mirrors claude-code's `paneCreationLock`
/// promise chain — prevents two concurrent `tmux split-window` calls from
/// racing for the same target pane.
fn pane_creation_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Constants mirrored verbatim from `claude-code/src/utils/swarm/constants.ts`.
pub struct SwarmConstants;

impl SwarmConstants {
    pub const SESSION_NAME: &'static str = "claude-swarm";
    pub const VIEW_WINDOW_NAME: &'static str = "swarm-view";
    pub const TMUX_COMMAND: &'static str = "tmux";

    /// Per-pid socket name so multiple Claude instances don't collide.
    pub fn socket_name_for_pid(pid: u32) -> String {
        format!("claude-swarm-{pid}")
    }

    pub fn current_socket_name() -> String {
        Self::socket_name_for_pid(std::process::id())
    }
}

/// Agent colors we support (matches `AgentColorName` in claude-code's
/// `agentColorManager.ts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentColor {
    Red,
    Blue,
    Green,
    Yellow,
    Cyan,
    Purple,
    Orange,
    Pink,
}

/// Color literal mapping — see `TmuxBackend.ts:60-69`.
pub fn agent_color_to_tmux(c: AgentColor) -> &'static str {
    match c {
        AgentColor::Red => "red",
        AgentColor::Blue => "blue",
        AgentColor::Green => "green",
        AgentColor::Yellow => "yellow",
        AgentColor::Cyan => "cyan",
        AgentColor::Purple => "magenta",
        AgentColor::Orange => "colour208",
        AgentColor::Pink => "colour205",
    }
}

// --- argv builders (pure, unit-testable) ---

pub fn build_split_window_argv(
    target_pane: &str,
    horizontal: bool,
    size_pct: Option<&str>,
) -> Vec<String> {
    let mut a = vec![
        "split-window".to_string(),
        "-t".to_string(),
        target_pane.to_string(),
        (if horizontal { "-h" } else { "-v" }).to_string(),
    ];
    if let Some(pct) = size_pct {
        a.push("-l".to_string());
        a.push(pct.to_string());
    }
    a.push("-P".to_string());
    a.push("-F".to_string());
    a.push("#{pane_id}".to_string());
    a
}

pub fn build_select_pane_color_argv(pane_id: &str, tmux_color: &str) -> Vec<String> {
    vec![
        "select-pane".to_string(),
        "-t".to_string(),
        pane_id.to_string(),
        "-P".to_string(),
        format!("bg=default,fg={tmux_color}"),
    ]
}

pub fn build_set_pane_border_argv(pane_id: &str, tmux_color: &str) -> Vec<String> {
    vec![
        "set-option".to_string(),
        "-p".to_string(),
        "-t".to_string(),
        pane_id.to_string(),
        "pane-border-style".to_string(),
        format!("fg={tmux_color}"),
    ]
}

pub fn build_set_pane_border_format_argv(pane_id: &str, fmt: &str) -> Vec<String> {
    vec![
        "set-option".to_string(),
        "-p".to_string(),
        "-t".to_string(),
        pane_id.to_string(),
        "pane-border-format".to_string(),
        fmt.to_string(),
    ]
}

pub fn build_send_keys_argv(pane_id: &str, cmd: &str) -> Vec<String> {
    vec![
        "send-keys".to_string(),
        "-t".to_string(),
        pane_id.to_string(),
        cmd.to_string(),
        "Enter".to_string(),
    ]
}

// --- real backend impl ---

/// POSIX `SwarmBackend` using `tmux` shell-out.
#[derive(Default)]
pub struct TmuxBackend {
    /// Optional per-pid socket name; defaults to `current_socket_name()`.
    socket_name: Option<String>,
}

impl TmuxBackend {
    #[must_use]
    pub fn new() -> Self {
        Self { socket_name: None }
    }

    fn socket(&self) -> String {
        self.socket_name
            .clone()
            .unwrap_or_else(SwarmConstants::current_socket_name)
    }

    /// Internal helper: shell out to `tmux` with the requested argv.
    async fn run_tmux(&self, in_swarm_socket: bool, args: &[String]) -> Result<String, SwarmError> {
        let mut cmd = Command::new(SwarmConstants::TMUX_COMMAND);
        if in_swarm_socket {
            cmd.arg("-L").arg(self.socket());
        }
        cmd.args(args);
        let out = cmd
            .output()
            .await
            .map_err(|e| SwarmError::Tmux(format!("spawn tmux failed: {e}")))?;
        if !out.status.success() {
            return Err(SwarmError::Tmux(format!(
                "tmux exited {}: stderr={}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }
}

#[async_trait]
impl SwarmBackend for TmuxBackend {
    async fn start_swarm(&self, _layout: SwarmLayout) -> Result<SwarmHandle, SwarmError> {
        let inside = TmuxBackend::is_running_inside();
        if inside {
            // Inside tmux: we don't create a new session — we'll split the
            // current window when teammates are added. Return the user's
            // current session name as the handle (best-effort).
            let session_name = std::env::var("TMUX")
                .ok()
                .and_then(|_| {
                    // `tmux display-message -p '#S'` returns current session.
                    std::process::Command::new(SwarmConstants::TMUX_COMMAND)
                        .args(["display-message", "-p", "#S"])
                        .output()
                        .ok()
                })
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_else(|| "user-session".to_string());
            return Ok(SwarmHandle { session_name });
        }

        // Outside tmux: create the external claude-swarm session on a per-pid socket.
        // `tmux -L <socket> new-session -d -s claude-swarm -n swarm-view`
        let args = vec![
            "new-session".to_string(),
            "-d".to_string(),
            "-s".to_string(),
            SwarmConstants::SESSION_NAME.to_string(),
            "-n".to_string(),
            SwarmConstants::VIEW_WINDOW_NAME.to_string(),
        ];
        self.run_tmux(true, &args).await?;
        Ok(SwarmHandle {
            session_name: SwarmConstants::SESSION_NAME.to_string(),
        })
    }

    async fn create_teammate_pane(
        &self,
        _agent_id: &AgentId,
        position: PanePosition,
    ) -> Result<PaneId, SwarmError> {
        // Serialize pane creation per claude-code paneCreationLock.
        let _guard = pane_creation_lock().lock().await;

        let inside = TmuxBackend::is_running_inside();
        let horizontal = matches!(position, PanePosition::Left | PanePosition::Right);

        // Target: inside-tmux uses the leader's pane id (from `$TMUX_PANE`),
        // outside-tmux uses the swarm session's first pane.
        let target_pane = if inside {
            std::env::var("TMUX_PANE")
                .map_err(|_| SwarmError::Tmux("TMUX_PANE not set inside tmux".into()))?
        } else {
            // Look up the first pane in the external swarm session.
            let listing = self
                .run_tmux(
                    true,
                    &[
                        "list-panes".to_string(),
                        "-t".to_string(),
                        format!(
                            "{}:{}",
                            SwarmConstants::SESSION_NAME,
                            SwarmConstants::VIEW_WINDOW_NAME
                        ),
                        "-F".to_string(),
                        "#{pane_id}".to_string(),
                    ],
                )
                .await?;
            listing
                .lines()
                .next()
                .ok_or_else(|| SwarmError::Tmux("no panes in swarm session".into()))?
                .to_string()
        };

        // For the first inside-tmux teammate, claude-code uses 70% width split.
        let split_argv = build_split_window_argv(&target_pane, horizontal, Some("70%"));
        let new_pane = self.run_tmux(!inside, &split_argv).await?;

        // Apply default color (red as a placeholder; production callers pass
        // the agent's color via a separate API the engine layers on top).
        let tmux_color = agent_color_to_tmux(AgentColor::Red);
        let color_argv = build_select_pane_color_argv(&new_pane, tmux_color);
        self.run_tmux(!inside, &color_argv).await?;

        let border_argv = build_set_pane_border_argv(&new_pane, tmux_color);
        self.run_tmux(!inside, &border_argv).await?;

        // Wait for shell init.
        tokio::time::sleep(Duration::from_millis(PANE_SHELL_INIT_DELAY_MS)).await;

        Ok(PaneId { raw: new_pane })
    }

    async fn destroy_swarm(&self, handle: SwarmHandle) -> Result<(), SwarmError> {
        let inside = TmuxBackend::is_running_inside();
        if inside {
            // Inside tmux: leave the user's session intact; the engine will
            // kill individual panes via break-pane during teammate teardown.
            tracing::debug!(
                "TmuxBackend::destroy_swarm: inside-tmux mode is a no-op for session {}",
                handle.session_name
            );
            return Ok(());
        }
        // Outside tmux: kill the entire claude-swarm session on our socket.
        let args = vec![
            "kill-session".to_string(),
            "-t".to_string(),
            handle.session_name,
        ];
        self.run_tmux(true, &args).await?;
        Ok(())
    }

    fn is_available(&self) -> bool {
        which::which("tmux").is_ok() && detection::tmux_version_ok()
    }
}

impl TmuxBackend {
    /// Synchronous "are we inside a tmux session" probe — claude-code only
    /// reads `$TMUX`, never shells out (see `detection.ts:35-37`).
    pub fn is_running_inside() -> bool {
        std::env::var("TMUX").is_ok()
    }
}
```

- [ ] **Step 3: Uncomment the `pub use` line in `mod.rs`**

Edit `lingxi-core/platforms/posix/src/swarm/mod.rs` — uncomment:

```rust
pub use tmux::TmuxBackend;
pub type TmuxSwarmBackend = TmuxBackend;
```

- [ ] **Step 4: Run argv tests**

```bash
cd lingxi-core
cargo test -p lingxi-platform-posix --test swarm_tmux_argv_test
cargo build -p lingxi-platform-posix
cargo clippy -p lingxi-platform-posix -- -D warnings
```

Expected: argv tests pass; build clean.

---

### Task 11: Test — color mapping + version parse unit tests

**Files:**
- Already created in Task 10 (`swarm_tmux_argv_test.rs`)
- Already created in Task 9 (inline `parse_tmux_version_at_least_3_2` tests)

- [ ] **Step 1: Re-run + verify coverage**

```bash
cd lingxi-core
cargo test -p lingxi-platform-posix --test swarm_tmux_argv_test
cargo test -p lingxi-platform-posix --lib swarm::detection::tests
```

Expected: all pass.

---

### Task 12: Integration test — real tmux shell-out (gated `#[ignore]`)

**Files:**
- Create: `lingxi-core/platforms/posix/tests/swarm_tmux_integration_test.rs`

A real end-to-end check that requires `tmux` ≥ 3.2 on the host. Marked `#[ignore]` so it doesn't run in default `cargo test`. CI matrix opts in with `cargo test -- --ignored` when the runner has tmux installed.

- [ ] **Step 1: Write the test**

```rust
//! Real tmux integration test. Run manually with:
//!   cargo test -p lingxi-platform-posix --test swarm_tmux_integration_test -- --ignored
//! Requires tmux >= 3.2 on PATH.

use lingxi_platform_posix::swarm::TmuxBackend;
use lingxi_protocol::AgentId;
use lingxi_traits::{PanePosition, SwarmBackend, SwarmLayout};

#[tokio::test]
#[ignore = "requires real tmux >= 3.2 on host"]
async fn start_swarm_outside_tmux_creates_external_session() {
    if std::env::var("TMUX").is_ok() {
        eprintln!("skip: this test must run OUTSIDE tmux");
        return;
    }
    if !TmuxBackend::new().is_available() {
        eprintln!("skip: tmux not available or version < 3.2");
        return;
    }

    let backend = TmuxBackend::new();
    let handle = backend.start_swarm(SwarmLayout::Tiled).await.unwrap();
    assert_eq!(handle.session_name, "claude-swarm");

    // Create one pane, verify destroy.
    let _pane = backend
        .create_teammate_pane(&AgentId::new(), PanePosition::Right)
        .await
        .expect("create_teammate_pane");

    backend.destroy_swarm(handle).await.unwrap();
}
```

- [ ] **Step 2: Verify it compiles + is correctly ignored**

```bash
cd lingxi-core
cargo test -p lingxi-platform-posix --test swarm_tmux_integration_test
```

Expected: build clean; test reported as `ignored` (not run).

---

### Task 13: Implement `iterm.rs` (AppleScript via `osascript`)

**Files:**
- Replace: `lingxi-core/platforms/posix/src/swarm/iterm.rs`
- Modify: `lingxi-core/platforms/posix/src/swarm/mod.rs` — uncomment `pub use iterm::ITermSwarmBackend;`

The iTerm backend builds AppleScript snippets and invokes them via `osascript -e <script>`. We don't try to replicate every visual feature of the tmux backend (no per-pane border colors — iTerm draws those at the OS level via tab colors and we skip them, matching claude-code's `ITermBackend.ts:270-289` "skip for performance" comment).

**Critical 1:1 fidelity:**
- `is_available()` checks `$TERM_PROGRAM == "iTerm.app"` AND `which osascript`.
- AppleScript invocations use `tell application "iTerm" to ...` (NOT `to "iTerm2"` — older AppleScript dictionary key).
- We skip color/title setters with a `tracing::debug!` log line, matching claude-code's "no-op for performance" pattern.

- [ ] **Step 1: Implement `iterm.rs`**

Replace `lingxi-core/platforms/posix/src/swarm/iterm.rs`:

```rust
//! iTerm2 `SwarmBackend` using AppleScript via `osascript`.
//!
//! Deliberate divergence from claude-code: claude-code uses the `it2` Python
//! CLI (`src/utils/swarm/backends/ITermBackend.ts`). We use AppleScript
//! instead because it's built into macOS and avoids the Python-API-disabled
//! trap (`it2 --version` succeeds even when iTerm2's API toggle is off,
//! causing `it2 session split` to fail with no fallback). See M2-05 plan
//! "Deliberate divergences" §1.
//!
//! Color + title setters are intentionally no-ops to match claude-code's
//! `ITermBackend.ts:270-300` performance posture.

use async_trait::async_trait;
use lingxi_protocol::AgentId;
use lingxi_traits::{PaneId, PanePosition, SwarmBackend, SwarmError, SwarmHandle, SwarmLayout};
use std::sync::OnceLock;
use tokio::process::Command;
use tokio::sync::Mutex;

/// Per-process pane-creation lock, matching the tmux backend's serialization.
fn pane_creation_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// iTerm2 backend using `osascript`.
#[derive(Default)]
pub struct ITermSwarmBackend;

impl ITermSwarmBackend {
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Run an AppleScript snippet through `osascript -e <script>`.
    async fn run_osascript(&self, script: &str) -> Result<String, SwarmError> {
        let out = Command::new("osascript")
            .arg("-e")
            .arg(script)
            .output()
            .await
            .map_err(|e| SwarmError::Tmux(format!("spawn osascript failed: {e}")))?;
        if !out.status.success() {
            return Err(SwarmError::Tmux(format!(
                "osascript exited {}: stderr={}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    /// Pure helper, exposed for unit testing — assemble the AppleScript for
    /// "open a new iTerm window, return its session id".
    pub fn build_new_window_script(default_command: &str) -> String {
        // AppleScript single-quote-safety: callers must pre-escape any single
        // quotes in `default_command`. The engine never lets agent-provided
        // strings reach this function, so we keep escape rules simple.
        format!(
            r#"tell application "iTerm"
    set newWindow to (create window with default profile)
    tell current session of newWindow
        write text "{default_command}"
        return id
    end tell
end tell"#
        )
    }

    /// Pure helper — assemble the AppleScript for "split current session".
    pub fn build_split_script(vertical: bool) -> String {
        let direction = if vertical { "vertically" } else { "horizontally" };
        format!(
            r#"tell application "iTerm"
    tell current session of current window
        set newSession to (split {direction} with default profile)
        return id of newSession
    end tell
end tell"#
        )
    }
}

#[async_trait]
impl SwarmBackend for ITermSwarmBackend {
    async fn start_swarm(&self, _layout: SwarmLayout) -> Result<SwarmHandle, SwarmError> {
        // Open a fresh iTerm window to host the swarm.
        let id = self
            .run_osascript(&Self::build_new_window_script(":"))
            .await?;
        Ok(SwarmHandle { session_name: id })
    }

    async fn create_teammate_pane(
        &self,
        _agent_id: &AgentId,
        position: PanePosition,
    ) -> Result<PaneId, SwarmError> {
        let _guard = pane_creation_lock().lock().await;
        let vertical = matches!(position, PanePosition::Top | PanePosition::Bottom);
        let id = self
            .run_osascript(&Self::build_split_script(vertical))
            .await?;
        tracing::debug!("ITermSwarmBackend: created pane {}", id);
        Ok(PaneId { raw: id })
    }

    async fn destroy_swarm(&self, handle: SwarmHandle) -> Result<(), SwarmError> {
        // Closing the host window is the user's choice; we don't force-close
        // (matching claude-code's iTerm backend posture).
        tracing::debug!(
            "ITermSwarmBackend::destroy_swarm: leaving iTerm window {} intact",
            handle.session_name
        );
        Ok(())
    }

    fn is_available(&self) -> bool {
        std::env::var("TERM_PROGRAM").as_deref() == Ok("iTerm.app")
            && which::which("osascript").is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_window_script_contains_tell_iterm() {
        let s = ITermSwarmBackend::build_new_window_script(":");
        assert!(s.contains("tell application \"iTerm\""));
        assert!(s.contains("create window with default profile"));
    }

    #[test]
    fn split_script_horizontal_vs_vertical() {
        let v = ITermSwarmBackend::build_split_script(true);
        let h = ITermSwarmBackend::build_split_script(false);
        assert!(v.contains("split vertically"));
        assert!(h.contains("split horizontally"));
    }
}
```

- [ ] **Step 2: Uncomment `pub use` in `mod.rs`**

```rust
pub use iterm::ITermSwarmBackend;
```

- [ ] **Step 3: Build + unit tests**

```bash
cd lingxi-core
cargo build -p lingxi-platform-posix
cargo test -p lingxi-platform-posix --lib swarm::iterm::tests
```

Expected: clean build; 2 unit tests pass.

---

### Task 14: Implement `inprocess.rs` (no-pane fallback)

**Files:**
- Replace: `lingxi-core/platforms/posix/src/swarm/inprocess.rs`
- Modify: `lingxi-core/platforms/posix/src/swarm/mod.rs` — uncomment `pub use inprocess::InProcessSwarmBackend;`

The InProcess backend is the always-available fallback. Every operation succeeds with a synthetic `PaneId` and logs a debug line — there is no actual pane.

- [ ] **Step 1: Implement `inprocess.rs`**

Replace `lingxi-core/platforms/posix/src/swarm/inprocess.rs`:

```rust
//! No-pane `SwarmBackend` fallback.
//!
//! Always available. Methods succeed with synthetic identifiers and emit
//! a `tracing::debug!` log line — there is no actual terminal-pane
//! visualization. The engine drives multi-agent coordination through the
//! same effect-handler / mailbox machinery either way, so this backend
//! is fully functional from the agent's perspective; only the operator's
//! visual feedback is missing. Mirrors claude-code
//! `src/utils/swarm/backends/InProcessBackend.ts`.

use async_trait::async_trait;
use lingxi_protocol::AgentId;
use lingxi_traits::{PaneId, PanePosition, SwarmBackend, SwarmError, SwarmHandle, SwarmLayout};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Default)]
pub struct InProcessSwarmBackend;

impl InProcessSwarmBackend {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SwarmBackend for InProcessSwarmBackend {
    async fn start_swarm(&self, _layout: SwarmLayout) -> Result<SwarmHandle, SwarmError> {
        tracing::debug!("swarm running in-process; no pane visualization");
        Ok(SwarmHandle {
            session_name: "in-process".to_string(),
        })
    }

    async fn create_teammate_pane(
        &self,
        agent_id: &AgentId,
        _position: PanePosition,
    ) -> Result<PaneId, SwarmError> {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        tracing::debug!(
            "swarm running in-process; no pane visualization (agent={agent_id:?}, synthetic pane #{n})"
        );
        Ok(PaneId {
            raw: format!("in-process-pane-{n}"),
        })
    }

    async fn destroy_swarm(&self, _handle: SwarmHandle) -> Result<(), SwarmError> {
        tracing::debug!("swarm running in-process; no pane visualization (destroy no-op)");
        Ok(())
    }

    fn is_available(&self) -> bool {
        true
    }
}
```

- [ ] **Step 2: Uncomment `pub use` in `mod.rs`**

```rust
pub use inprocess::InProcessSwarmBackend;
```

- [ ] **Step 3: Build**

```bash
cd lingxi-core
cargo build -p lingxi-platform-posix
```

Expected: clean.

---

### Task 15: Implement `registry.rs` (auto-detect constructor)

**Files:**
- Replace: `lingxi-core/platforms/posix/src/swarm/registry.rs`
- Modify: `lingxi-core/platforms/posix/src/swarm/mod.rs` — uncomment `pub use registry::SwarmRegistry;`

`registry.rs` exposes the single entry point that the engine calls: `SwarmRegistry::detect_and_construct() -> Box<dyn SwarmBackend>`. It runs `detection::detect_terminal_env()` + `detection::pick_backend()` and instantiates the matching concrete type.

- [ ] **Step 1: Implement `registry.rs`**

Replace `lingxi-core/platforms/posix/src/swarm/registry.rs`:

```rust
//! Single source of truth for which `SwarmBackend` the POSIX platform uses.
//!
//! Detection runs once at construction time. Re-detecting mid-session is
//! intentionally not supported — claude-code caches its choice for the
//! lifetime of the process (`registry.ts:26,140-145`) for the same reason:
//! the environment doesn't change while Claude is running.

use std::sync::OnceLock;

use lingxi_traits::SwarmBackend;

use super::detection::{detect_terminal_env, pick_backend, BackendChoice, TerminalEnv};
use super::inprocess::InProcessSwarmBackend;
use super::iterm::ITermSwarmBackend;
use super::tmux::TmuxBackend;

/// Process-level cache for the detected backend choice. Lets tests use
/// `set_for_testing()` to override.
static CHOICE_CACHE: OnceLock<BackendChoice> = OnceLock::new();

/// Construct the appropriate `Box<dyn SwarmBackend>` based on the host
/// environment. First call probes; subsequent calls reuse the cached choice.
pub struct SwarmRegistry;

impl SwarmRegistry {
    /// Probe env + tools and return a freshly-constructed backend. Caches the
    /// detection result for the process lifetime.
    pub fn detect_and_construct() -> Box<dyn SwarmBackend> {
        let choice = *CHOICE_CACHE.get_or_init(|| pick_backend(&detect_terminal_env()));
        Self::construct(choice)
    }

    /// Force a specific backend choice (test hook).
    pub fn detect_and_construct_with(env: TerminalEnv) -> Box<dyn SwarmBackend> {
        let choice = pick_backend(&env);
        Self::construct(choice)
    }

    fn construct(choice: BackendChoice) -> Box<dyn SwarmBackend> {
        match choice {
            BackendChoice::Tmux => Box::new(TmuxBackend::new()),
            BackendChoice::ITerm => Box::new(ITermSwarmBackend::new()),
            BackendChoice::InProcess => Box::new(InProcessSwarmBackend::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn construct_with_inprocess_env() {
        let env = TerminalEnv {
            inside_tmux: false,
            iterm_app: false,
            tmux_available: false,
            osascript_available: false,
        };
        let backend = SwarmRegistry::detect_and_construct_with(env);
        assert!(backend.is_available()); // InProcess is always available
    }
}
```

- [ ] **Step 2: Uncomment `pub use` in `mod.rs`**

Final `mod.rs` should look like:

```rust
//! POSIX swarm backends: tmux (real), iTerm (AppleScript), InProcess (no-pane).

pub mod detection;
pub mod inprocess;
pub mod iterm;
pub mod registry;
pub mod tmux;

pub use inprocess::InProcessSwarmBackend;
pub use iterm::ITermSwarmBackend;
pub use registry::SwarmRegistry;
pub use tmux::TmuxBackend;

/// Back-compat alias for the M2-01 era `lib.rs` re-export.
pub type TmuxSwarmBackend = TmuxBackend;
```

- [ ] **Step 3: Build + unit test**

```bash
cd lingxi-core
cargo build -p lingxi-platform-posix
cargo test -p lingxi-platform-posix --lib swarm::registry::tests
```

Expected: clean build; unit test passes.

---

### Task 16: Cross-reference doc for `platforms/windows/src/swarm.rs`

**Files:**
- Modify: `lingxi-core/platforms/windows/src/swarm.rs` (doc cross-ref only — no code change)

M2-01 already locked the windows swarm impl to `SwarmError::Unsupported`. M2-05 only adds a doc comment pointing at the POSIX trifecta for code archeology purposes.

**Critical 1:1 fidelity:**
- The error string from M2-01, `--tmux is not supported on Windows`, must remain unchanged. We only add module-level doc.

- [ ] **Step 1: Edit the module doc comment**

Edit `lingxi-core/platforms/windows/src/swarm.rs` — update the top doc block:

```rust
//! Swarm backend — Windows (no tmux).
//!
//! claude-code refuses `--tmux` on Windows; we match. Linux + macOS get the
//! tmux/iTerm/InProcess trifecta in `platforms/posix/src/swarm/`. See M2-05
//! plan for the cross-platform parity story.
//!
//! Future: if Windows ever gains a swarm story, it would land here as a
//! Windows Terminal / wezterm CLI shell-out. Out of scope for v0.3.0.
```

Leave the rest of the file untouched.

- [ ] **Step 2: Verify nothing else changed**

```bash
cd lingxi-core
cargo build -p lingxi-platform-windows
cargo test -p lingxi-platform-windows
```

Expected: clean.

---

### Task 17: Commit Phase B

- [ ] **Step 1: Full test sweep**

```bash
cd lingxi-core
cargo test -p lingxi-platform-posix
cargo test -p lingxi-platform-windows
cargo clippy -p lingxi-platform-posix -p lingxi-platform-windows -- -D warnings
cargo fmt --check
```

Expected: everything clean. New tests added in this phase:
- `swarm_detection_test.rs` — 5 tests
- `swarm_tmux_argv_test.rs` — 5 tests
- `swarm_tmux_integration_test.rs` — 1 ignored test
- Inline unit tests in `detection.rs`, `iterm.rs`, `registry.rs` — 4 + 2 + 1 tests

- [ ] **Step 2: Two commits — POSIX trifecta + Windows doc**

```bash
git add lingxi-core/platforms/posix/src/swarm/ \
        lingxi-core/platforms/posix/tests/swarm_detection_test.rs \
        lingxi-core/platforms/posix/tests/swarm_tmux_argv_test.rs \
        lingxi-core/platforms/posix/tests/swarm_tmux_integration_test.rs \
        lingxi-core/platforms/posix/src/lib.rs
# (also captures the removal of the old single-file swarm.rs via git rename detection)
git commit -m "feat(platforms/posix): tmux + iTerm + InProcess swarm backends"

git add lingxi-core/platforms/windows/src/swarm.rs
git commit -m "refactor(platforms/windows): swarm explicitly Unsupported"
```

End Phase B.

---

## Phase C · Verification + commits (3 tasks)

### Task 18: Run full workspace test suite

- [ ] **Step 1: All tests**

```bash
cd lingxi-core
cargo test --workspace
```

Expected: baseline + ~22 new tests (see breakdown below). Pre-M2-05 baseline must remain green.

New test count by file:
- `fs_watch_debounce_test.rs` — 1
- `fs_watch_git_filter_test.rs` — 1
- inline `watch_helper::tests` — 2
- `fs_watch_smoke_test.rs` (Windows only) — 1
- `swarm_detection_test.rs` — 5
- inline `swarm::detection::tests` — 4
- `swarm_tmux_argv_test.rs` — 5
- `swarm_tmux_integration_test.rs` — 1 (ignored)
- inline `swarm::iterm::tests` — 2
- inline `swarm::registry::tests` — 1

Total: ~22 new tests (1 ignored).

- [ ] **Step 2: Long-form full sweep**

```bash
cd lingxi-core
cargo test --workspace -- --include-ignored  # opt-in, only run if tmux installed
```

Expected: integration tmux test runs and passes when tmux ≥ 3.2 is on PATH; otherwise it self-skips via the `if !is_available()` guard.

---

### Task 19: clippy + fmt

- [ ] **Step 1: clippy**

```bash
cd lingxi-core
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: zero warnings.

- [ ] **Step 2: fmt**

```bash
cd lingxi-core
cargo fmt --all --check
```

Expected: zero diffs. If anything is unformatted, run `cargo fmt --all` and amend.

---

### Task 20: Final commits per spec §6.5

The spec calls for **three commits** for this plan. Phases A + B already produced two; the windows-side refactor is the third. By the end of this plan, the git log should show:

1. `feat(platforms): notify-based FS watch with debounce` (Phase A — Task 7)
2. `feat(platforms/posix): tmux + iTerm + InProcess swarm backends` (Phase B — Task 17 commit 1)
3. `refactor(platforms/windows): swarm explicitly Unsupported` (Phase B — Task 17 commit 2)

- [ ] **Step 1: Verify the three commits**

```bash
cd lingxi-core
git log --oneline -3
```

Expected output (last 3 commits, top is newest):

```
<sha3> refactor(platforms/windows): swarm explicitly Unsupported
<sha2> feat(platforms/posix): tmux + iTerm + InProcess swarm backends
<sha1> feat(platforms): notify-based FS watch with debounce
```

- [ ] **Step 2: If commits got merged or out of order, fix with non-destructive operations**

```bash
# If the order is wrong (rare), use a single soft reset + redo from the staging area.
# Do NOT use git reset --hard or force-push.
```

- [ ] **Step 3: Plan exit**

The plan is complete when:
1. `cargo test --workspace` is green.
2. `cargo clippy --workspace --all-targets -- -D warnings` is clean.
3. `cargo fmt --all --check` is clean.
4. The three commits above appear in `git log`.
5. `git status` is clean (no untracked / uncommitted files).

---

## Self-Review

- §6.5 FS watch via notify + debouncer → Phase A Tasks 1–7 ✓
- §6.5 chokidar defaults (500ms / 200ms) → Task 2 (`DEFAULT_STABILITY_THRESHOLD_MS`, `DEFAULT_POLL_INTERVAL_MS`) + Task 3 test ✓
- §6.5 `.git` always excluded → Task 2 (`should_skip`) + Task 4 test ✓
- §6.5 Tmux backend (real impl) → Task 10 ✓
- §6.5 iTerm backend (AppleScript via `osascript`) → Task 13 ✓
- §6.5 InProcess backend (no-pane fallback) → Task 14 ✓
- §6.5 Backend registry (auto-detect) → Task 15 ✓
- §6.5 Pane creation lock → Task 10 (`pane_creation_lock()` global `OnceLock<Mutex<()>>`) ✓
- §6.5 200ms shell init delay → Task 10 (`PANE_SHELL_INIT_DELAY_MS = 200`) ✓
- §6.5 Color map literals → Task 10 (`agent_color_to_tmux`) + Task 11 test ✓
- §6.5 Tmux ≥ 3.2 requirement → Task 9 (`parse_tmux_version_at_least_3_2`) ✓
- §6.5 Windows swarm Unsupported (cross-ref only) → Task 16 ✓
- 3 commits per spec → Tasks 7, 17, 17 (split) ✓

## Execution Handoff

After this plan lands, the next milestones are:
- **M2-06** — SecureStorage macOS + HTTP SSE + Process polish.
- **M2-07** — Test infra + docs + tag v0.3.0.

Plans M2-04, M2-05, M2-06 can run in parallel after M2-01; M2-07 runs last.

## Notes for the implementer

- **`notify` 6.x Rust 1.82 compatibility**: per spec §7.1, this is a known risk. If `cargo build` fails after Task 1 with an edition2024 error from a transitive dep (commonly `crossbeam-channel` or `parking_lot`), use `cargo update --precise` to pin the exact problem dep. Document the pin in M2-07 release notes. Suggested starting pins if needed:
  - `notify = "=6.1.1"`
  - `notify-debouncer-mini = "=0.4.1"`
- **`tokio-stream` dep**: the `watch_helper` uses `tokio_stream::wrappers::ReceiverStream`. If not already in the workspace, add `tokio-stream = "0.1"` to both `platforms/posix/Cargo.toml` and `platforms/windows/Cargo.toml` in Task 1.
- **Pane-creation lock surface area**: the lock is process-global via `OnceLock<Mutex<()>>`. Don't be tempted to put it on the `TmuxBackend` struct — claude-code's lock is also module-scoped (`TmuxBackend.ts:29`) for the same reason: two `TmuxBackend` instances would still need to serialize against each other if they target the same tmux server.
- **AppleScript escaping**: `build_new_window_script` and `build_split_script` interpolate strings that callers MUST pre-validate. The engine's effect-handler layer never lets agent-controlled strings reach these helpers, but future contributors should treat any new caller as untrusted by default and quote-escape.
- **Integration test gating**: `swarm_tmux_integration_test` is `#[ignore]` and additionally self-skips when `$TMUX` is set or tmux is unavailable. CI matrix opts in with `--include-ignored` only on jobs that pre-install tmux.
- **The deleted `platforms/posix/src/swarm.rs` single file**: git's rename detection should follow it to `swarm/mod.rs`. If `git log --follow` doesn't pick it up, that's cosmetic — the code is functionally equivalent.

## End of plan

//! `read_file_state` — the per-path read-state registry shared between the
//! orchestrator and the file tools.
//!
//! 1:1 port of claude-code's `readFileState` cache
//! (`FileReadTool.ts:1032` `readFileState.set(fullFilePath, {content,
//! timestamp, offset, limit})`). Each successful `Read` records the file's
//! decoded content, its floor-truncated mtime (in milliseconds, matching TS
//! `Math.floor(mtimeMs)`), and the `offset`/`limit` the read was performed
//! with. Future staleness guards (Edit/Write/NotebookEdit) and the Read
//! dedup path consume this registry.
//!
//! ## Byte-budgeted LRU (1:1 with `FileStateCache`)
//!
//! Backed by a byte-budgeted LRU matching claude-code's `FileStateCache`
//! (`utils/fileStateCache.ts`), constructed
//! `new LRUCache({ max, maxSize, sizeCalculation: n => Math.max(1,
//! Buffer.byteLength(n.content)) })`:
//! - `max` entries = `READ_FILE_STATE_MAX_ENTRIES` (`fileStateCache.ts:18`,
//!   `READ_FILE_STATE_CACHE_SIZE = 100`).
//! - `maxSize` bytes = `READ_FILE_STATE_MAX_BYTES` (`fileStateCache.ts:22`,
//!   `DEFAULT_MAX_CACHE_SIZE_BYTES = 25 * 1024 * 1024`).
//! - per-entry size = `entry.content.len().max(1)` (UTF-8 byte length, the Rust
//!   equal of `Math.max(1, Buffer.byteLength(content))`).
//! - MRU-promote on [`ReadFileStateLru::get`]; on [`ReadFileStateLru::set`],
//!   least-recently-used entries are evicted while the entry count exceeds
//!   `max` OR the accumulated bytes exceed `maxSize`. A lone entry larger than
//!   `maxSize` is retained (lru-cache never evicts the key it just set).
//!
//! Zero new dependency: a `HashMap` + monotonic recency counter (the cap is 100
//! entries, so the O(n) LRU scan on eviction is trivial and deterministic).
//!
//! DEFERRED follow-ups (separate file-tracking items, out of scope here): the
//! `dump()`/`load()` + `cloneFileStateCache` persistence used to snapshot
//! read-state into forked agents (`AgentTool/runAgent.ts:377`),
//! `mergeFileStateCaches` (timestamp merge), the `normalize(key)` path
//! normalization (Rust callers already pass canonicalized absolute keys — see
//! `tools/file/src/lib.rs`), and the `file_state_cache:{entries, bytes}`
//! telemetry.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Default max entries (claude-code `READ_FILE_STATE_CACHE_SIZE`,
/// `fileStateCache.ts:18`).
pub const READ_FILE_STATE_MAX_ENTRIES: usize = 100;

/// Default byte budget: 25 MiB (claude-code `DEFAULT_MAX_CACHE_SIZE_BYTES`,
/// `fileStateCache.ts:22` — `25 * 1024 * 1024`).
pub const READ_FILE_STATE_MAX_BYTES: u64 = 25 * 1024 * 1024;

/// A single read-state entry — the faithful port of TS
/// `readFileState.set(fullFilePath, {content, timestamp, offset, limit})`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadFileEntry {
    /// The decoded file content as read (the same string the model saw).
    pub content: String,
    /// File modification time in **milliseconds**, floor-truncated to match
    /// TS `Math.floor(mtimeMs)`.
    pub mtime_ms: i64,
    /// The `offset` the read used (`None` when the read had no `offset`).
    pub offset: Option<u64>,
    /// The `limit` the read used (`None` when the read had no `limit`).
    pub limit: Option<u64>,
    /// Whether this entry was written by the `Read` tool (as opposed to
    /// `Write`/`Edit`/`NotebookEdit`, which update the registry post-write).
    ///
    /// This is the Rust stand-in for TS's `existingState.offset !== undefined`
    /// dedup gate (`FileReadTool.ts:550`). TS distinguishes Read entries from
    /// Edit/Write entries because Read always stores a defaulted numeric
    /// `offset` while Edit/Write store `offset: undefined`. The Rust `Read`
    /// records the *un-defaulted* `offset` (so a full read stores `None`,
    /// matching the staleness guard's full-read discriminator), making
    /// `offset` unusable for the Read-vs-write distinction — hence this
    /// explicit flag. The Read-dedup path (`FileReadTool.ts:547-573`) consults
    /// it to avoid deduping a re-read against a post-edit entry (which would
    /// wrongly point the model at pre-edit content). The staleness guard does
    /// NOT read this field.
    pub from_read: bool,
}

/// One LRU slot: the entry plus its recency stamp and cached byte size.
#[derive(Clone, Debug)]
struct Node {
    entry: ReadFileEntry,
    /// Monotonic recency stamp; higher = more recently used.
    last_used: u64,
    /// Cached `entry.content.len().max(1)` (the lru-cache `sizeCalculation`).
    size: u64,
}

/// The byte-budgeted LRU behind the shared read-state registry.
///
/// Eviction happens on [`Self::set`]: while the entry count exceeds
/// `max_entries` OR the accumulated bytes exceed `max_bytes`, the
/// least-recently-used entry (lowest `last_used`) is dropped — never the key
/// just inserted (it always carries the highest stamp), so a lone over-budget
/// entry is retained, matching lru-cache.
#[derive(Debug)]
pub struct ReadFileStateLru {
    map: HashMap<PathBuf, Node>,
    /// Monotonic clock; every `get`/`set` bumps it to stamp recency.
    clock: u64,
    /// Running sum of every entry's `size` (kept in sync with `map`).
    total_bytes: u64,
    max_entries: usize,
    max_bytes: u64,
}

impl ReadFileStateLru {
    /// Construct with explicit limits.
    #[must_use]
    fn with_limits(max_entries: usize, max_bytes: u64) -> Self {
        Self {
            map: HashMap::new(),
            clock: 0,
            total_bytes: 0,
            max_entries,
            max_bytes,
        }
    }

    /// Fetch a clone of the entry for `path`, promoting it to most-recently-used
    /// (1:1 with lru-cache's MRU-on-`get`).
    pub fn get(&mut self, path: &Path) -> Option<ReadFileEntry> {
        let next = self.clock + 1;
        let node = self.map.get_mut(path)?;
        self.clock = next;
        node.last_used = next;
        Some(node.entry.clone())
    }

    /// Insert (or overwrite) the entry for `path` as most-recently-used, then
    /// evict least-recently-used entries while over the entry/byte budget.
    pub fn set(&mut self, path: PathBuf, entry: ReadFileEntry) {
        let size = (entry.content.len() as u64).max(1);
        self.clock += 1;
        let last_used = self.clock;
        if let Some(old) = self.map.insert(
            path,
            Node {
                entry,
                last_used,
                size,
            },
        ) {
            // Overwrite: drop the replaced entry's bytes before adding the new.
            self.total_bytes = self.total_bytes.saturating_sub(old.size);
        }
        self.total_bytes += size;
        self.evict();
    }

    /// Remove the entry for `path`, returning it if present and keeping byte
    /// accounting in sync. Used when a shell command writes a file that may have
    /// been cached from a previous read.
    pub fn remove(&mut self, path: &Path) -> Option<ReadFileEntry> {
        let node = self.map.remove(path)?;
        self.total_bytes = self.total_bytes.saturating_sub(node.size);
        Some(node.entry)
    }

    /// Drop LRU entries while over budget. Never evicts the last remaining entry
    /// (so a lone over-`maxSize` entry is retained, matching lru-cache), and
    /// never the just-set key (it carries the highest `last_used`, so the
    /// min-stamp scan can't select it while another entry exists).
    fn evict(&mut self) {
        while self.map.len() > 1
            && (self.map.len() > self.max_entries || self.total_bytes > self.max_bytes)
        {
            let Some(victim) = self
                .map
                .iter()
                .min_by_key(|(_, n)| n.last_used)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            if let Some(n) = self.map.remove(&victim) {
                self.total_bytes = self.total_bytes.saturating_sub(n.size);
            }
        }
    }

    /// Drain every entry as `(path, entry)` and reset the registry — the Rust
    /// equal of the post-compact `eOt` snapshot + `readFileState.clear()` step
    /// (`orchestrator::restore_post_compact_attachments`). Preserves the prior
    /// `HashMap::drain` call-site (`map.drain().collect()`).
    pub fn drain(&mut self) -> impl Iterator<Item = (PathBuf, ReadFileEntry)> + '_ {
        self.total_bytes = 0;
        self.clock = 0;
        self.map.drain().map(|(k, node)| (k, node.entry))
    }

    /// Number of cached entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Whether the registry is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Accumulated byte size across all entries (the lru-cache
    /// `calculatedSize`). Exposed for tests + future `file_state_cache` telemetry.
    #[must_use]
    pub fn total_bytes(&self) -> u64 {
        self.total_bytes
    }
}

/// The shared, cheaply-clonable read-state registry: absolute path → entry,
/// evicted by a byte-budgeted LRU ([`ReadFileStateLru`]).
///
/// Cloning an `Arc` is cheap; every holder shares the same underlying LRU so a
/// write from one file tool is visible to every other holder.
pub type ReadFileStateMap = Arc<Mutex<ReadFileStateLru>>;

/// Construct a fresh, empty read-state registry with the claude-code defaults
/// (100 entries / 25 MiB).
#[must_use]
pub fn new_read_file_state_map() -> ReadFileStateMap {
    Arc::new(Mutex::new(ReadFileStateLru::with_limits(
        READ_FILE_STATE_MAX_ENTRIES,
        READ_FILE_STATE_MAX_BYTES,
    )))
}

/// Construct a read-state registry with explicit limits (used by eviction
/// tests; also the seam a future config override would use).
#[must_use]
pub fn new_read_file_state_map_with_limits(max_entries: usize, max_bytes: u64) -> ReadFileStateMap {
    Arc::new(Mutex::new(ReadFileStateLru::with_limits(
        max_entries,
        max_bytes,
    )))
}

/// Compute the floor-truncated mtime in milliseconds from a [`SystemTime`],
/// matching TS `Math.floor(mtimeMs)`.
///
/// Returns `0` for the Unix epoch and clamps pre-epoch times to `0` (the file
/// tools always read existing files, so pre-epoch is not expected in practice).
///
/// [`SystemTime`]: std::time::SystemTime
#[must_use]
pub fn mtime_ms_floor(mtime: std::time::SystemTime) -> i64 {
    match mtime.duration_since(std::time::UNIX_EPOCH) {
        // `as_millis()` already floors to whole milliseconds, matching
        // `Math.floor(mtimeMs)`.
        Ok(d) => i64::try_from(d.as_millis()).unwrap_or(i64::MAX),
        Err(_) => 0,
    }
}

/// Insert (or overwrite) the read-state entry for `path`.
///
/// 1:1 with TS `readFileState.set(fullFilePath, …)`. The key is the absolute
/// path the caller resolved; this helper does no normalization of its own.
pub fn set(map: &ReadFileStateMap, path: PathBuf, entry: ReadFileEntry) {
    if let Ok(mut guard) = map.lock() {
        guard.set(path, entry);
    }
}

/// Fetch a clone of the read-state entry for `path`, if present, promoting it to
/// most-recently-used.
///
/// 1:1 with TS `readFileState.get(fullFilePath)`.
#[must_use]
pub fn get(map: &ReadFileStateMap, path: &Path) -> Option<ReadFileEntry> {
    map.lock().ok().and_then(|mut guard| guard.get(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    fn entry(content: &str) -> ReadFileEntry {
        ReadFileEntry {
            content: content.to_string(),
            mtime_ms: 1,
            offset: None,
            limit: None,
            from_read: true,
        }
    }

    #[test]
    fn set_then_get_roundtrips() {
        let map = new_read_file_state_map();
        let path = PathBuf::from("/tmp/a.txt");
        let e = ReadFileEntry {
            content: "hello\nworld\n".to_string(),
            mtime_ms: 1_234,
            offset: Some(2),
            limit: Some(10),
            from_read: true,
        };
        set(&map, path.clone(), e.clone());
        assert_eq!(get(&map, &path), Some(e));
    }

    #[test]
    fn get_missing_is_none() {
        let map = new_read_file_state_map();
        assert_eq!(get(&map, Path::new("/nope")), None);
    }

    #[test]
    fn set_overwrites_existing_entry() {
        let map = new_read_file_state_map();
        let path = PathBuf::from("/tmp/a.txt");
        set(
            &map,
            path.clone(),
            ReadFileEntry {
                content: "v1".into(),
                mtime_ms: 1,
                offset: None,
                limit: None,
                from_read: true,
            },
        );
        set(
            &map,
            path.clone(),
            ReadFileEntry {
                content: "v2".into(),
                mtime_ms: 2,
                offset: Some(5),
                limit: Some(7),
                from_read: true,
            },
        );
        let got = get(&map, &path).unwrap();
        assert_eq!(got.content, "v2");
        assert_eq!(got.mtime_ms, 2);
        assert_eq!(got.offset, Some(5));
        assert_eq!(got.limit, Some(7));
    }

    #[test]
    fn arc_clone_shares_underlying_map() {
        let map = new_read_file_state_map();
        let clone = Arc::clone(&map);
        let path = PathBuf::from("/tmp/shared.txt");
        set(
            &clone,
            path.clone(),
            ReadFileEntry {
                content: "shared".into(),
                mtime_ms: 9,
                offset: None,
                limit: None,
                from_read: true,
            },
        );
        // A write through `clone` is visible through the original handle.
        assert_eq!(get(&map, &path).unwrap().content, "shared");
    }

    #[test]
    fn mtime_ms_floor_truncates_to_whole_ms() {
        // 1.5009 s -> 1500 ms (sub-millisecond fraction floored away).
        let t = UNIX_EPOCH + Duration::new(1, 500_900_000);
        assert_eq!(mtime_ms_floor(t), 1_500);
    }

    #[test]
    fn mtime_ms_floor_epoch_is_zero() {
        assert_eq!(mtime_ms_floor(UNIX_EPOCH), 0);
    }

    #[test]
    fn evicts_lru_when_entry_count_exceeds_max() {
        // Entry-count cap only (huge byte budget): the 101st insert evicts the
        // first-inserted key; size clamps to `max`.
        let map = new_read_file_state_map_with_limits(100, u64::MAX);
        for i in 0..101 {
            set(&map, PathBuf::from(format!("/f/{i}")), entry("x"));
        }
        assert_eq!(map.lock().unwrap().len(), 100);
        // First-inserted (LRU) evicted; last present.
        assert_eq!(get(&map, Path::new("/f/0")), None);
        assert!(get(&map, Path::new("/f/100")).is_some());
    }

    #[test]
    fn evicts_by_byte_budget() {
        // Byte budget only (huge entry cap): 10-byte entries, 25-byte budget →
        // at most 2 survive; total_bytes stays within budget.
        let map = new_read_file_state_map_with_limits(usize::MAX, 25);
        for i in 0..5 {
            set(&map, PathBuf::from(format!("/b/{i}")), entry("0123456789")); // 10 bytes
        }
        let guard = map.lock().unwrap();
        assert!(
            guard.total_bytes() <= 25,
            "over budget: {}",
            guard.total_bytes()
        );
        assert!(guard.len() <= 2, "too many survived: {}", guard.len());
    }

    #[test]
    fn get_promotes_mru_so_it_survives_eviction() {
        // Cap = 2. Insert A, B. get(A) promotes A. Insert C → B (now LRU) evicted;
        // A survives.
        let map = new_read_file_state_map_with_limits(2, u64::MAX);
        set(&map, PathBuf::from("/A"), entry("a"));
        set(&map, PathBuf::from("/B"), entry("b"));
        assert!(get(&map, Path::new("/A")).is_some()); // promote A to MRU
        set(&map, PathBuf::from("/C"), entry("c")); // evicts LRU = B
        assert!(
            get(&map, Path::new("/A")).is_some(),
            "A promoted, must survive"
        );
        assert_eq!(
            get(&map, Path::new("/B")),
            None,
            "B was LRU, must be evicted"
        );
        assert!(get(&map, Path::new("/C")).is_some());
    }

    #[test]
    fn overwrite_updates_byte_total_not_count() {
        // Overwriting a key subtracts the old size, not double-counts: set P=10B
        // then P=3B → 1 entry, total_bytes = 3.
        let map = new_read_file_state_map_with_limits(100, u64::MAX);
        set(&map, PathBuf::from("/p"), entry("0123456789")); // 10 bytes
        set(&map, PathBuf::from("/p"), entry("abc")); // 3 bytes
        let guard = map.lock().unwrap();
        assert_eq!(guard.len(), 1);
        assert_eq!(guard.total_bytes(), 3);
    }

    #[test]
    fn remove_deletes_entry_and_updates_byte_total() {
        let map = new_read_file_state_map_with_limits(100, u64::MAX);
        let path = PathBuf::from("/tmp/remove-me.txt");
        set(&map, path.clone(), entry("0123456789"));

        let mut guard = map.lock().unwrap();
        assert!(guard.remove(&path).is_some());
        assert_eq!(guard.len(), 0);
        assert_eq!(guard.total_bytes(), 0);
    }

    #[test]
    fn single_oversized_entry_is_retained() {
        // A lone entry larger than the byte budget is kept (lru-cache never
        // evicts the key it just set when it's the only one).
        let map = new_read_file_state_map_with_limits(100, 4);
        set(
            &map,
            PathBuf::from("/big"),
            entry("this content far exceeds four bytes"),
        );
        assert!(
            get(&map, Path::new("/big")).is_some(),
            "lone over-budget entry must be retained"
        );
        assert_eq!(map.lock().unwrap().len(), 1);
    }

    #[test]
    fn drain_yields_all_entries_and_resets() {
        // The post-compact snapshot+clear seam: drain returns every (path, entry)
        // and leaves the registry empty with zero accounted bytes.
        let map = new_read_file_state_map();
        set(&map, PathBuf::from("/d/1"), entry("one"));
        set(&map, PathBuf::from("/d/2"), entry("two"));
        let drained: Vec<(PathBuf, ReadFileEntry)> = map.lock().unwrap().drain().collect();
        assert_eq!(drained.len(), 2);
        let guard = map.lock().unwrap();
        assert!(guard.is_empty());
        assert_eq!(guard.total_bytes(), 0);
    }
}

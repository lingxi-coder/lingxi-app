//! `read_file_state` — the per-path read-state registry shared between the
//! orchestrator and the file tools.
//!
//! 1:1 port of claude-code's `readFileState` map
//! (`FileReadTool.ts:1032` `readFileState.set(fullFilePath, {content,
//! timestamp, offset, limit})`). Each successful `Read` records the file's
//! decoded content, its floor-truncated mtime (in milliseconds, matching TS
//! `Math.floor(mtimeMs)`), and the `offset`/`limit` the read was performed
//! with. Future staleness guards (Edit/Write/NotebookEdit) and the Read
//! dedup path consume this registry.
//!
//! This module is intentionally minimal — it stores the `{content, mtime_ms,
//! offset, limit}` tuple keyed by absolute [`PathBuf`] in a plain map; the
//! entry shape is a faithful port.
//!
//! DEFERRED divergence (verified vs 2.1.195, low impact). claude-code's
//! `readFileState` is an `lru-cache`, constructed
//! `new LRUCache({ max, maxSize, sizeCalculation: n => Math.max(1,
//! Buffer.byteLength(n.content)) })` — a BYTE-budgeted LRU (entry cap `max` +
//! byte budget `maxSize`) with MRU-promote on `get` and `dump()`/`load()`
//! cross-session persistence (telemetry `file_state_cache:{entries, bytes}`).
//! This port uses an unbounded [`HashMap`] with no eviction or MRU ordering.
//! The divergence only manifests once a session's cached content exceeds the
//! byte budget (claude then evicts least-recently-used entries; this port keeps
//! them). A faithful port needs the exact `max`/`maxSize` values (config-
//! indirected in the binary) plus the dump/load persistence, so it is deferred
//! rather than guessed. NOTE: an earlier comment here called this a "100-entry
//! LRU" — that was inaccurate; the cap is byte-budgeted, not a fixed count.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

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

/// The shared, cheaply-clonable read-state map: absolute path → entry.
///
/// Cloning an `Arc` is cheap; every holder shares the same underlying
/// `HashMap` so a write from one file tool is visible to every other holder.
pub type ReadFileStateMap = Arc<Mutex<HashMap<PathBuf, ReadFileEntry>>>;

/// Construct a fresh, empty read-state map.
#[must_use]
pub fn new_read_file_state_map() -> ReadFileStateMap {
    Arc::new(Mutex::new(HashMap::new()))
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
        guard.insert(path, entry);
    }
}

/// Fetch a clone of the read-state entry for `path`, if present.
///
/// 1:1 with TS `readFileState.get(fullFilePath)`.
#[must_use]
pub fn get(map: &ReadFileStateMap, path: &Path) -> Option<ReadFileEntry> {
    map.lock().ok().and_then(|guard| guard.get(path).cloned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn set_then_get_roundtrips() {
        let map = new_read_file_state_map();
        let path = PathBuf::from("/tmp/a.txt");
        let entry = ReadFileEntry {
            content: "hello\nworld\n".to_string(),
            mtime_ms: 1_234,
            offset: Some(2),
            limit: Some(10),
            from_read: true,
        };
        set(&map, path.clone(), entry.clone());
        assert_eq!(get(&map, &path), Some(entry));
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
}

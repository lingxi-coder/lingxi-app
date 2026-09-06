//! Root-pinned append-only journal primitives for durable session state.
//!
//! The journal is deliberately generic.  The application layer owns the
//! event payload (cost vectors, terminal Fusion facts, and outbox projections)
//! while this module owns ordering, stable event identity, fsync boundaries,
//! and fail-closed recovery.  All methods are synchronous: callers must run
//! them on a blocking worker when used from an async runtime.

use platform_api::rooted_fs::{
    atomic_write_pinned, lock_exclusive_pinned, open_append_file_pinned, open_read_file_pinned,
    root_identity, sync_parent_pinned, truncate_file_pinned, AtomicWriteOptions, RootIdentity,
};
use platform_api::FsError;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Current envelope schema.  Unknown versions are never skipped: doing so
/// could make a replayed cost or terminal record appear durable when it was
/// written under a different interpretation.
pub const JOURNAL_SCHEMA_VERSION: u16 = 1;
/// Authoritative append-only state log name.
pub const JOURNAL_FILE_NAME: &str = "ledger.v1.jsonl";
/// Derivative snapshot name.
pub const SNAPSHOT_FILE_NAME: &str = "snapshot.v1.json";
/// Stable transaction lock shared by journal and snapshot operations.
pub const JOURNAL_LOCK_FILE_NAME: &str = "ledger.lock";
/// Owner-only session state directory mode.
pub const SESSION_STATE_DIR_MODE: u32 = 0o700;
/// Owner-only journal/snapshot mode.
pub const SESSION_STATE_FILE_MODE: u32 = 0o600;
/// Default maximum encoded envelope size.  Recovery never reads an
/// unbounded attacker-controlled line into memory.
pub const DEFAULT_MAX_RECORD_BYTES: usize = 4 * 1024 * 1024;

/// One versioned journal envelope.  `event_id` is the idempotency key and is
/// retained even when the caller's acknowledgement receiver has been dropped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalEnvelope<E> {
    /// Envelope schema version.
    pub schema_version: u16,
    /// Strictly increasing sequence assigned by the journal owner.
    pub journal_revision: u64,
    /// Stable mutation/delivery identity.
    pub event_id: String,
    /// Typed event payload.
    pub event: E,
}

/// A validated journal event after replay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalEntry {
    /// Monotonic journal revision.
    pub journal_revision: u64,
    /// Stable event identity.
    pub event_id: String,
    /// JSON payload retained for app-tier decoding.
    pub event: Value,
}

/// Replay result, including whether a malformed final tail was repaired.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalReplay {
    /// Valid events in revision order.
    pub entries: Vec<JournalEntry>,
    /// Last authoritative revision (zero for an empty journal).
    pub last_revision: u64,
    /// True only when a malformed, non-newline-terminated final tail was
    /// truncated under the journal lock.
    pub repaired_final_tail: bool,
    /// Whether the authoritative WAL file existed, including an empty file.
    /// Recovery uses this to reject a derivative snapshot that has no WAL.
    pub journal_present: bool,
}

/// Result of an append-once operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JournalAppend {
    /// Revision assigned to the new event, or the existing event on a
    /// duplicate idempotent append.
    pub journal_revision: u64,
    /// Whether the event was already present with identical content.
    pub duplicate: bool,
}

/// Snapshot envelope.  Snapshots are derivatives and can always be rebuilt
/// from a valid WAL prefix; they are never treated as authoritative alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalSnapshot<S> {
    /// Snapshot schema version.
    pub schema_version: u16,
    /// Last journal revision folded into `state`.
    pub last_journal_revision: u64,
    /// Typed folded state.
    pub state: S,
}

/// Fail-closed journal errors.  The original bytes are left untouched for
/// interior/newline-final corruption, version mismatches, gaps, and event-id
/// conflicts.
#[derive(Debug, Error)]
pub enum JournalError {
    /// Rooted filesystem operation failed.
    #[error(transparent)]
    Fs(#[from] FsError),
    /// JSON envelope or snapshot could not be decoded.
    #[error("journal JSON error: {0}")]
    Json(#[from] serde_json::Error),
    /// A malformed final non-newline tail may be repaired only when this is
    /// the final physical record.
    #[error("malformed final journal tail at byte {offset}")]
    MalformedFinalTail {
        /// Byte offset of the invalid tail.
        offset: u64,
    },
    /// Corruption in an interior or newline-terminated record.
    #[error("journal corruption at byte {offset}: {reason}")]
    Corrupted {
        /// Byte offset of the offending line.
        offset: u64,
        /// Stable diagnostic.
        reason: String,
    },
    /// Envelope schema is not understood by this binary.
    #[error("unsupported journal schema version {version}")]
    UnsupportedVersion {
        /// Unknown version observed on disk.
        version: u16,
    },
    /// A revision was skipped or regressed.
    #[error("journal revision {actual} does not follow {expected}")]
    RevisionGap {
        /// Revision found on disk.
        actual: u64,
        /// Revision required by the prefix.
        expected: u64,
    },
    /// The same event id was reused for a different payload.
    #[error("journal event id conflict: {event_id}")]
    EventConflict {
        /// Conflicting stable identity.
        event_id: String,
    },
    /// A record exceeds the bounded scanner limit.
    #[error("journal record exceeds {limit} bytes")]
    RecordTooLarge {
        /// Configured limit.
        limit: usize,
    },
    /// No next journal revision can be represented.
    #[error("journal revision overflow")]
    RevisionOverflow,
    /// The session-state root is not a real directory.
    #[error("session-state root is not a regular directory: {0}")]
    InvalidRoot(String),
}

/// Root-pinned journal owner.  A `DurableJournal` is cheap to clone by value
/// and carries the exact identity of the opened session directory.
#[derive(Debug, Clone)]
pub struct DurableJournal {
    root: PathBuf,
    identity: RootIdentity,
    max_record_bytes: usize,
}

struct JournalScan {
    replay: JournalReplay,
    matching_event: Option<JournalEntry>,
}

struct JournalScanState {
    entries: Vec<JournalEntry>,
    /// Thin metadata only. Cumulative cost vectors can make each event large;
    /// retaining every decoded payload would recreate a whole-WAL allocation.
    /// On the exceptional repeated-id path the scanner rereads just the one
    /// prior bounded record for an exact comparison.
    seen: std::collections::HashMap<String, JournalRecordLocation>,
    next_revision: Option<u64>,
    last_revision: u64,
    matching_event: Option<JournalEntry>,
}

#[derive(Debug, Clone, Copy)]
struct JournalRecordLocation {
    offset: u64,
    content_len: usize,
}

impl JournalScanState {
    fn new() -> Self {
        Self {
            entries: Vec::new(),
            seen: std::collections::HashMap::new(),
            next_revision: Some(1),
            last_revision: 0,
            matching_event: None,
        }
    }

    fn accept<F>(
        &mut self,
        envelope: JournalEnvelope<Value>,
        matching_id: Option<&str>,
        location: JournalRecordLocation,
        compare_prior: F,
    ) -> Result<Option<JournalEntry>, JournalError>
    where
        F: FnOnce(JournalRecordLocation, &Value) -> Result<bool, JournalError>,
    {
        if envelope.schema_version != JOURNAL_SCHEMA_VERSION {
            return Err(JournalError::UnsupportedVersion {
                version: envelope.schema_version,
            });
        }
        let Some(expected_revision) = self.next_revision else {
            return Err(JournalError::RevisionOverflow);
        };
        if envelope.journal_revision != expected_revision {
            return Err(JournalError::RevisionGap {
                actual: envelope.journal_revision,
                expected: expected_revision,
            });
        }
        self.last_revision = envelope.journal_revision;
        self.next_revision = envelope.journal_revision.checked_add(1);

        let duplicate = if let Some(previous) = self.seen.get(&envelope.event_id).copied() {
            if !compare_prior(previous, &envelope.event)? {
                return Err(JournalError::EventConflict {
                    event_id: envelope.event_id.clone(),
                });
            }
            true
        } else {
            self.seen.insert(envelope.event_id.clone(), location);
            false
        };
        if matching_id.is_some_and(|event_id| event_id == envelope.event_id)
            && self.matching_event.is_none()
        {
            self.matching_event = Some(JournalEntry {
                journal_revision: envelope.journal_revision,
                event_id: envelope.event_id.clone(),
                event: envelope.event.clone(),
            });
        }
        if duplicate {
            Ok(None)
        } else {
            Ok(Some(JournalEntry {
                journal_revision: envelope.journal_revision,
                event_id: envelope.event_id,
                event: envelope.event,
            }))
        }
    }
}

impl DurableJournal {
    /// Open a session-state directory and capture its identity.  Creation is
    /// intentionally limited to the final directory path; callers that derive
    /// the path from untrusted input should first use a rooted parent helper.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, JournalError> {
        let root = root.into();
        if let Ok(metadata) = fs::symlink_metadata(&root) {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(JournalError::InvalidRoot(root.display().to_string()));
            }
        } else {
            fs::create_dir_all(&root).map_err(|error| FsError::Io(error.to_string()))?;
        }
        let identity = root_identity(&root)?;
        Ok(Self::from_pinned(root, identity))
    }

    /// Create/open a session directory below an already trusted root using
    /// the platform's no-follow directory-handle walk. The returned root path
    /// and identity refer to the same opened directory.
    pub fn open_under(root: &Path, relative: &Path) -> Result<Self, JournalError> {
        let identity = platform_api::rooted_fs::ensure_private_directory(
            root,
            relative,
            SESSION_STATE_DIR_MODE,
        )?;
        Ok(Self::from_pinned(root.join(relative), identity))
    }

    /// Construct from an already verified session directory identity.
    #[must_use]
    pub fn from_pinned(root: PathBuf, identity: RootIdentity) -> Self {
        Self {
            root,
            identity,
            max_record_bytes: DEFAULT_MAX_RECORD_BYTES,
        }
    }

    /// Set a smaller/larger bounded record scanner for tests and hosts.
    #[must_use]
    pub fn with_max_record_bytes(mut self, max_record_bytes: usize) -> Self {
        self.max_record_bytes = max_record_bytes.max(1);
        self
    }

    /// Session directory used as the root for every operation.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Captured identity of [`Self::root`].
    #[must_use]
    pub fn root_identity(&self) -> RootIdentity {
        self.identity
    }

    fn lock(&self) -> Result<platform_api::RootedFileLock, JournalError> {
        Ok(lock_exclusive_pinned(
            &self.root,
            Path::new(JOURNAL_LOCK_FILE_NAME),
            SESSION_STATE_DIR_MODE,
            SESSION_STATE_FILE_MODE,
            Some(&self.identity),
        )?)
    }

    fn read_locked(&self) -> Result<JournalReplay, JournalError> {
        Ok(self.scan_locked(true, None, None)?.replay)
    }

    fn scan_locked(
        &self,
        collect_entries: bool,
        matching_id: Option<&str>,
        mut visitor: Option<&mut dyn FnMut(JournalEntry)>,
    ) -> Result<JournalScan, JournalError> {
        let relative = Path::new(JOURNAL_FILE_NAME);
        let file = match open_read_file_pinned(&self.root, relative, Some(&self.identity)) {
            Ok(file) => file,
            Err(FsError::NotFound(_)) => {
                return Ok(JournalScan {
                    replay: JournalReplay {
                        entries: Vec::new(),
                        last_revision: 0,
                        repaired_final_tail: false,
                        journal_present: false,
                    },
                    matching_event: None,
                });
            }
            Err(error) => return Err(error.into()),
        };
        let mut reader = BufReader::with_capacity(16 * 1024, file);
        let mut state = JournalScanState::new();
        let mut line = Vec::new();
        let mut line_offset = 0_u64;
        let mut repaired_final_tail = false;
        let mut missing_final_delimiter = false;
        let mut truncate_final_tail_at = None;

        loop {
            let available = reader
                .fill_buf()
                .map_err(|error| FsError::Io(error.to_string()))?;
            if available.is_empty() {
                break;
            }
            let newline = available.iter().position(|byte| *byte == b'\n');
            let take = newline.map_or(available.len(), |index| index + 1);
            let content_len = newline.unwrap_or(take);
            let record_len = line
                .len()
                .checked_add(content_len)
                .and_then(|len| len.checked_add(usize::from(newline.is_some())))
                .ok_or(JournalError::RecordTooLarge {
                    limit: self.max_record_bytes,
                })?;
            if record_len > self.max_record_bytes {
                return Err(JournalError::RecordTooLarge {
                    limit: self.max_record_bytes,
                });
            }
            line.extend_from_slice(&available[..content_len]);
            reader.consume(take);
            if newline.is_some() {
                let envelope =
                    serde_json::from_slice::<JournalEnvelope<Value>>(&line).map_err(|error| {
                        JournalError::Corrupted {
                            offset: line_offset,
                            reason: error.to_string(),
                        }
                    })?;
                let location = JournalRecordLocation {
                    offset: line_offset,
                    content_len: line.len(),
                };
                if let Some(entry) =
                    state.accept(envelope, matching_id, location, |prior, event| {
                        self.event_at(prior).map(|previous| previous == *event)
                    })?
                {
                    if collect_entries {
                        state.entries.push(entry);
                    } else if let Some(visitor) = visitor.as_deref_mut() {
                        visitor(entry);
                    }
                }
                let physical_record_len =
                    line.len()
                        .checked_add(1)
                        .ok_or(JournalError::RecordTooLarge {
                            limit: self.max_record_bytes,
                        })?;
                line_offset = line_offset
                    .checked_add(u64::try_from(physical_record_len).unwrap_or(u64::MAX))
                    .ok_or(JournalError::RevisionOverflow)?;
                line.clear();
            }
        }

        if !line.is_empty() {
            match serde_json::from_slice::<JournalEnvelope<Value>>(&line) {
                Ok(envelope) => {
                    let location = JournalRecordLocation {
                        offset: line_offset,
                        content_len: line.len(),
                    };
                    if let Some(entry) =
                        state.accept(envelope, matching_id, location, |prior, event| {
                            self.event_at(prior).map(|previous| previous == *event)
                        })?
                    {
                        if collect_entries {
                            state.entries.push(entry);
                        } else if let Some(visitor) = visitor.as_deref_mut() {
                            visitor(entry);
                        }
                    }
                    missing_final_delimiter = true;
                }
                Err(_) => {
                    truncate_final_tail_at = Some(line_offset);
                    repaired_final_tail = true;
                }
            }
        }

        drop(reader);
        if let Some(offset) = truncate_final_tail_at {
            truncate_file_pinned(&self.root, relative, offset, Some(&self.identity))?;
        }
        if missing_final_delimiter {
            let mut file = open_append_file_pinned(&self.root, relative, Some(&self.identity))?;
            file.write_all(b"\n")
                .map_err(|error| FsError::Io(error.to_string()))?;
            file.sync_all()
                .map_err(|error| FsError::Io(error.to_string()))?;
        }
        Ok(JournalScan {
            replay: JournalReplay {
                entries: state.entries,
                last_revision: state.last_revision,
                repaired_final_tail,
                journal_present: true,
            },
            matching_event: state.matching_event,
        })
    }

    fn event_at(&self, location: JournalRecordLocation) -> Result<Value, JournalError> {
        let relative = Path::new(JOURNAL_FILE_NAME);
        let mut file = open_read_file_pinned(&self.root, relative, Some(&self.identity))?;
        file.seek(SeekFrom::Start(location.offset))
            .map_err(|error| FsError::Io(error.to_string()))?;
        let mut bytes = vec![0_u8; location.content_len];
        file.read_exact(&mut bytes)
            .map_err(|error| FsError::Io(error.to_string()))?;
        let envelope =
            serde_json::from_slice::<JournalEnvelope<Value>>(&bytes).map_err(|error| {
                JournalError::Corrupted {
                    offset: location.offset,
                    reason: error.to_string(),
                }
            })?;
        Ok(envelope.event)
    }

    /// Make a validated, already-present prefix durable before it can seed
    /// success acknowledgements after process recovery. Syncing the directory
    /// as well closes the crash window where complete file bytes were visible
    /// but the first directory entry had never been persisted.
    fn sync_durable_prefix_locked(&self, journal_present: bool) -> Result<(), JournalError> {
        if !journal_present {
            return Ok(());
        }
        let relative = Path::new(JOURNAL_FILE_NAME);
        let file = open_append_file_pinned(&self.root, relative, Some(&self.identity))?;
        file.sync_all()
            .map_err(|error| FsError::Io(error.to_string()))?;
        sync_parent_pinned(&self.root, relative, Some(&self.identity))?;
        Ok(())
    }

    /// Validate/replay the authoritative WAL.  A recoverable tail is repaired
    /// while the same transaction lock is held.
    pub fn replay(&self) -> Result<JournalReplay, JournalError> {
        let _lock = self.lock()?;
        self.read_locked()
    }

    /// Validate and durably sync the authoritative WAL while delivering each
    /// unique event to `visitor` one record at a time. The returned replay
    /// carries summary metadata with an empty `entries` vector; callers that
    /// need all payloads can continue to use [`Self::replay`].
    pub fn replay_durable_with<F>(&self, mut visitor: F) -> Result<JournalReplay, JournalError>
    where
        F: FnMut(JournalEntry),
    {
        let _lock = self.lock()?;
        let scan = self.scan_locked(false, None, Some(&mut visitor))?;
        self.sync_durable_prefix_locked(scan.replay.journal_present)?;
        Ok(scan.replay)
    }

    /// Find one exact event without materializing unrelated payloads, and sync
    /// the validated prefix before returning it as durable truth.
    pub fn find_event_durable(&self, event_id: &str) -> Result<Option<JournalEntry>, JournalError> {
        let _lock = self.lock()?;
        let scan = self.scan_locked(false, Some(event_id), None)?;
        self.sync_durable_prefix_locked(scan.replay.journal_present)?;
        Ok(scan.matching_event)
    }

    /// Append a typed event exactly once, assigning the next journal revision.
    /// The filesystem handle is fsynced before this method returns success.
    pub fn append_once<E: Serialize>(
        &self,
        event_id: impl Into<String>,
        event: &E,
    ) -> Result<JournalAppend, JournalError> {
        let event_id = event_id.into();
        let event = serde_json::to_value(event)?;
        let _lock = self.lock()?;
        let scan = self.scan_locked(false, Some(&event_id), None)?;
        if let Some(previous) = scan.matching_event {
            if previous.event == event {
                self.sync_durable_prefix_locked(scan.replay.journal_present)?;
                return Ok(JournalAppend {
                    journal_revision: previous.journal_revision,
                    duplicate: true,
                });
            }
            return Err(JournalError::EventConflict { event_id });
        }
        let envelope = JournalEnvelope {
            schema_version: JOURNAL_SCHEMA_VERSION,
            journal_revision: scan
                .replay
                .last_revision
                .checked_add(1)
                .ok_or(JournalError::RevisionOverflow)?,
            event_id,
            event,
        };
        let mut line = serde_json::to_vec(&envelope)?;
        line.push(b'\n');
        if line.len() > self.max_record_bytes {
            return Err(JournalError::RecordTooLarge {
                limit: self.max_record_bytes,
            });
        }
        let mut file = open_append_file_pinned(
            &self.root,
            Path::new(JOURNAL_FILE_NAME),
            Some(&self.identity),
        )?;
        file.write_all(&line)
            .map_err(|error| FsError::Io(error.to_string()))?;
        file.sync_all()
            .map_err(|error| FsError::Io(error.to_string()))?;
        if !scan.replay.journal_present {
            sync_parent_pinned(
                &self.root,
                Path::new(JOURNAL_FILE_NAME),
                Some(&self.identity),
            )?;
        }
        Ok(JournalAppend {
            journal_revision: envelope.journal_revision,
            duplicate: false,
        })
    }

    /// Read a derivative snapshot through the pinned root.  Snapshot parse or
    /// revision mismatch is reported to the caller, which can rebuild it from
    /// [`Self::replay`].
    pub fn read_snapshot<S: DeserializeOwned>(
        &self,
    ) -> Result<Option<JournalSnapshot<S>>, JournalError> {
        let _lock = self.lock()?;
        let file = match open_read_file_pinned(
            &self.root,
            Path::new(SNAPSHOT_FILE_NAME),
            Some(&self.identity),
        ) {
            Ok(file) => file,
            Err(FsError::NotFound(_)) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let mut bytes = Vec::new();
        file.take(
            u64::try_from(self.max_record_bytes)
                .unwrap_or(u64::MAX)
                .saturating_add(1),
        )
        .read_to_end(&mut bytes)
        .map_err(|error| FsError::Io(error.to_string()))?;
        if bytes.len() > self.max_record_bytes {
            return Err(JournalError::RecordTooLarge {
                limit: self.max_record_bytes,
            });
        }
        let snapshot = serde_json::from_slice::<JournalSnapshot<S>>(&bytes)?;
        if snapshot.schema_version != JOURNAL_SCHEMA_VERSION {
            return Err(JournalError::UnsupportedVersion {
                version: snapshot.schema_version,
            });
        }
        Ok(Some(snapshot))
    }

    /// Write a derivative snapshot.  WAL append remains the authoritative ack
    /// boundary; callers should log/repair a snapshot failure rather than
    /// freezing an already acknowledged mutation.
    pub fn write_snapshot<S: Serialize>(
        &self,
        last_journal_revision: u64,
        state: &S,
    ) -> Result<(), JournalError> {
        let snapshot = JournalSnapshot {
            schema_version: JOURNAL_SCHEMA_VERSION,
            last_journal_revision,
            state,
        };
        let bytes = serde_json::to_vec(&snapshot)?;
        let _lock = self.lock()?;
        atomic_write_pinned(
            &self.root,
            Path::new(SNAPSHOT_FILE_NAME),
            &bytes,
            AtomicWriteOptions {
                overwrite: true,
                create_parents: false,
                dir_mode: SESSION_STATE_DIR_MODE,
                file_mode: SESSION_STATE_FILE_MODE,
            },
            Some(&self.identity),
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn journal() -> (tempfile::TempDir, DurableJournal) {
        let dir = tempfile::tempdir().expect("tempdir");
        let journal = DurableJournal::open(dir.path()).expect("journal");
        (dir, journal)
    }

    #[test]
    fn append_once_reuses_revision_for_identical_id() {
        let (_dir, journal) = journal();
        let first = journal.append_once("m1", &json!({"total": 1})).unwrap();
        let duplicate = journal.append_once("m1", &json!({"total": 1})).unwrap();
        assert!(!first.duplicate);
        assert!(duplicate.duplicate);
        assert_eq!(duplicate.journal_revision, first.journal_revision);
        assert_eq!(journal.replay().unwrap().entries.len(), 1);
    }

    #[test]
    fn conflicting_duplicate_and_interior_corruption_fail_closed() {
        let (dir, journal) = journal();
        journal.append_once("m1", &json!({"total": 1})).unwrap();
        assert!(matches!(
            journal.append_once("m1", &json!({"total": 2})),
            Err(JournalError::EventConflict { .. })
        ));
        std::fs::write(dir.path().join(JOURNAL_FILE_NAME), b"not-json\n").unwrap();
        assert!(matches!(
            journal.replay(),
            Err(JournalError::Corrupted { .. })
        ));
        assert_eq!(
            std::fs::read(dir.path().join(JOURNAL_FILE_NAME)).unwrap(),
            b"not-json\n"
        );
    }

    #[test]
    fn malformed_non_newline_tail_is_the_only_repaired_tail() {
        let (dir, journal) = journal();
        journal.append_once("m1", &json!({"total": 1})).unwrap();
        let path = dir.path().join(JOURNAL_FILE_NAME);
        let mut bytes = std::fs::read(&path).unwrap();
        bytes.extend_from_slice(b"{broken");
        std::fs::write(&path, &bytes).unwrap();
        let replay = journal.replay().unwrap();
        assert!(replay.repaired_final_tail);
        assert_eq!(replay.entries.len(), 1);
        assert_eq!(std::fs::read(&path).unwrap(), bytes[..bytes.len() - 7]);
    }

    #[test]
    fn invalid_utf8_final_tail_is_repaired_without_decoding_the_whole_wal() {
        let (dir, journal) = journal();
        journal.append_once("m1", &json!({"total": 1})).unwrap();
        let path = dir.path().join(JOURNAL_FILE_NAME);
        let mut bytes = std::fs::read(&path).unwrap();
        let valid_prefix_len = bytes.len();
        bytes.extend_from_slice(&[0xff, 0xfe, 0xfd]);
        std::fs::write(&path, &bytes).unwrap();

        let replay = journal.replay().unwrap();

        assert!(replay.repaired_final_tail);
        assert_eq!(replay.entries.len(), 1);
        assert_eq!(std::fs::read(path).unwrap().len(), valid_prefix_len);
    }

    #[test]
    fn valid_final_record_without_newline_gets_a_safe_delimiter() {
        let (dir, journal) = journal();
        let first = JournalEnvelope {
            schema_version: JOURNAL_SCHEMA_VERSION,
            journal_revision: 1,
            event_id: "m1".to_string(),
            event: json!({"total": 1}),
        };
        let path = dir.path().join(JOURNAL_FILE_NAME);
        std::fs::write(&path, serde_json::to_vec(&first).unwrap()).unwrap();

        journal.append_once("m2", &json!({"total": 2})).unwrap();

        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.ends_with(b"\n"));
        assert_eq!(bytes.iter().filter(|byte| **byte == b'\n').count(), 2);
        let replay = journal.replay().unwrap();
        assert_eq!(replay.entries.len(), 2);
        assert_eq!(replay.last_revision, 2);
    }

    #[test]
    fn long_wal_is_scanned_with_a_per_record_bound() {
        let (dir, journal) = journal();
        let journal = journal.with_max_record_bytes(256);
        let path = dir.path().join(JOURNAL_FILE_NAME);
        let mut file = std::fs::File::create(&path).unwrap();
        for revision in 1..=2_000_u64 {
            let envelope = JournalEnvelope {
                schema_version: JOURNAL_SCHEMA_VERSION,
                journal_revision: revision,
                event_id: format!("m{revision}"),
                event: json!({"total": revision}),
            };
            serde_json::to_writer(&mut file, &envelope).unwrap();
            file.write_all(b"\n").unwrap();
        }
        file.sync_all().unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() > 256);

        let replay = journal.replay().unwrap();

        assert!(replay.journal_present);
        assert_eq!(replay.entries.len(), 2_000);
        assert_eq!(replay.last_revision, 2_000);
    }

    #[test]
    fn durable_streaming_replay_does_not_collect_cumulative_payloads() {
        let (_dir, journal) = journal();
        journal.append_once("m1", &json!({"total": 1})).unwrap();
        journal.append_once("m2", &json!({"total": 2})).unwrap();
        let mut visited = Vec::new();

        let replay = journal
            .replay_durable_with(|entry| visited.push(entry))
            .unwrap();

        assert!(replay.entries.is_empty());
        assert_eq!(replay.last_revision, 2);
        assert_eq!(
            visited
                .iter()
                .map(|entry| entry.event_id.as_str())
                .collect::<Vec<_>>(),
            vec!["m1", "m2"]
        );
    }

    #[test]
    fn streaming_scan_rereads_one_prior_record_for_exact_duplicate_comparison() {
        let (dir, journal) = journal();
        let path = dir.path().join(JOURNAL_FILE_NAME);
        let envelopes = [
            JournalEnvelope {
                schema_version: JOURNAL_SCHEMA_VERSION,
                journal_revision: 1,
                event_id: "m1".to_string(),
                event: json!({"total": 1}),
            },
            JournalEnvelope {
                schema_version: JOURNAL_SCHEMA_VERSION,
                journal_revision: 2,
                event_id: "m1".to_string(),
                event: json!({"total": 1}),
            },
            JournalEnvelope {
                schema_version: JOURNAL_SCHEMA_VERSION,
                journal_revision: 3,
                event_id: "m2".to_string(),
                event: json!({"total": 2}),
            },
        ];
        let mut file = std::fs::File::create(&path).unwrap();
        for envelope in envelopes {
            serde_json::to_writer(&mut file, &envelope).unwrap();
            file.write_all(b"\n").unwrap();
        }
        file.sync_all().unwrap();
        let mut visited = Vec::new();

        let replay = journal
            .replay_durable_with(|entry| visited.push(entry.event_id))
            .unwrap();

        assert_eq!(replay.last_revision, 3);
        assert_eq!(visited, vec!["m1", "m2"]);

        let conflicting = JournalEnvelope {
            schema_version: JOURNAL_SCHEMA_VERSION,
            journal_revision: 4,
            event_id: "m1".to_string(),
            event: json!({"total": 9}),
        };
        serde_json::to_writer(&mut file, &conflicting).unwrap();
        file.write_all(b"\n").unwrap();
        file.sync_all().unwrap();
        assert!(matches!(
            journal.replay_durable_with(|_| {}),
            Err(JournalError::EventConflict { event_id }) if event_id == "m1"
        ));
    }

    #[test]
    fn revision_gaps_are_not_repaired() {
        let (dir, journal) = journal();
        let envelope = JournalEnvelope {
            schema_version: JOURNAL_SCHEMA_VERSION,
            journal_revision: 2,
            event_id: "m2".to_string(),
            event: json!({"total": 2}),
        };
        let mut bytes = serde_json::to_vec(&envelope).unwrap();
        bytes.push(b'\n');
        std::fs::write(dir.path().join(JOURNAL_FILE_NAME), &bytes).unwrap();
        assert!(matches!(
            journal.replay(),
            Err(JournalError::RevisionGap { .. })
        ));
        assert_eq!(
            std::fs::read(dir.path().join(JOURNAL_FILE_NAME)).unwrap(),
            bytes
        );
    }
}

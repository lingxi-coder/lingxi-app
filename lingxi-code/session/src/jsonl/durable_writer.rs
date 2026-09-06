//! Effectively-once transcript delivery under a session-state lock.
//!
//! `JsonlWriter` remains the compatibility writer used by older embedders.
//! This writer is the durable path for terminal/outbox records: it uses the
//! pinned session-state directory as the lock root, checks the stable delivery
//! id under that same lock, appends through the exact no-follow handle, and
//! fsyncs before acknowledging success.

use crate::jsonl::journal::{SESSION_STATE_DIR_MODE, SESSION_STATE_FILE_MODE};
use platform_api::rooted_fs::{
    lock_exclusive_pinned, open_append_file_pinned, open_read_file_pinned, root_identity,
    sync_parent_pinned, RootIdentity,
};
use platform_api::FsError;
use serde_json::{Map, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Stable lock filename shared by ordinary transcript appends, `/cd`
/// relocation, and durable outbox delivery.
pub const TRANSCRIPT_LOCK_FILE_NAME: &str = "transcript.lock";
/// Maximum size of one transcript record while checking a delivery id. The
/// total transcript may grow without bound; scanning keeps only one record in
/// memory at a time.
pub const DEFAULT_MAX_TRANSCRIPT_SCAN_BYTES: usize = 2 * 1024 * 1024;

/// Append-once result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptAppendOutcome {
    /// A new line was fsynced.
    Appended,
    /// An identical stable delivery id was already present.
    AlreadyPresent,
}

/// Durable transcript errors.  A conflict is never repaired by appending a
/// second interpretation of the same delivery id.
#[derive(Debug, Error)]
pub enum TranscriptWriterError {
    /// Rooted filesystem failure.
    #[error(transparent)]
    Fs(#[from] FsError),
    /// Payload must be an object so the host can attach the delivery id.
    #[error("transcript payload must be a JSON object")]
    PayloadNotObject,
    /// Durable outbox messages must carry their deterministic transcript UUID.
    #[error("transcript payload is missing a string uuid")]
    MissingMessageUuid,
    /// A stable delivery id was found with different content.
    #[error("transcript delivery id conflict: {delivery_id}")]
    DeliveryConflict {
        /// Conflicting id.
        delivery_id: String,
    },
    /// Bounded duplicate scan refused an oversized individual record.
    #[error("transcript record exceeds {limit} bytes")]
    ScanTooLarge {
        /// Scan bound.
        limit: usize,
    },
    /// Existing line was not valid JSON while checking idempotency.
    #[error("transcript line at byte {offset} is not valid JSON")]
    CorruptLine {
        /// Byte offset.
        offset: u64,
    },
}

/// Root-pinned transcript append owner.
#[derive(Debug, Clone)]
pub struct DurableTranscriptWriter {
    root: PathBuf,
    identity: RootIdentity,
    max_record_bytes: usize,
    #[cfg(test)]
    fail_next_existing_sync: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

/// One stable transcript transaction. Target resolution, relocation, and the
/// append itself can all run while this guard owns the session-state lock,
/// without attempting to acquire the same lock recursively.
pub struct DurableTranscriptTransaction<'a> {
    writer: &'a DurableTranscriptWriter,
    _lock: platform_api::RootedFileLock,
}

impl DurableTranscriptWriter {
    /// Open a pre-created session-state directory and capture its identity.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, TranscriptWriterError> {
        let root = root.into();
        let identity = root_identity(&root)?;
        Ok(Self::from_pinned(root, identity))
    }

    /// Construct from a caller-owned root identity.
    #[must_use]
    pub fn from_pinned(root: PathBuf, identity: RootIdentity) -> Self {
        Self {
            root,
            identity,
            max_record_bytes: DEFAULT_MAX_TRANSCRIPT_SCAN_BYTES,
            #[cfg(test)]
            fail_next_existing_sync: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Bound the duplicate scan.
    #[must_use]
    pub fn with_max_scan_bytes(mut self, max_scan_bytes: usize) -> Self {
        self.max_record_bytes = max_scan_bytes.max(1);
        self
    }

    /// Begin one stable transcript transaction.
    pub fn begin_transaction(
        &self,
    ) -> Result<DurableTranscriptTransaction<'_>, TranscriptWriterError> {
        let lock = lock_exclusive_pinned(
            &self.root,
            Path::new(TRANSCRIPT_LOCK_FILE_NAME),
            SESSION_STATE_DIR_MODE,
            SESSION_STATE_FILE_MODE,
            Some(&self.identity),
        )?;
        Ok(DurableTranscriptTransaction {
            writer: self,
            _lock: lock,
        })
    }

    /// Hold the stable session-state transcript lock while a caller resolves
    /// a cwd/project target, performs relocation, and appends through the
    /// supplied transaction guard. The closure is synchronous by design; a
    /// host should run it on a blocking worker and must not cancel it after
    /// filesystem IO begins.
    pub fn with_transaction<T, F>(&self, operation: F) -> Result<T, TranscriptWriterError>
    where
        F: FnOnce(&DurableTranscriptTransaction<'_>) -> Result<T, TranscriptWriterError>,
    {
        let transaction = self.begin_transaction()?;
        operation(&transaction)
    }

    /// Compatibility name for [`Self::with_transaction`]. The callback must
    /// append through its guard rather than reacquiring the transcript lock.
    pub fn with_lock<T, F>(&self, operation: F) -> Result<T, TranscriptWriterError>
    where
        F: FnOnce(&DurableTranscriptTransaction<'_>) -> Result<T, TranscriptWriterError>,
    {
        self.with_transaction(operation)
    }

    /// Append a JSON object exactly once by `delivery_id`.
    pub fn append_json_once(
        &self,
        transcript_relative: &Path,
        delivery_id: &str,
        payload: Value,
    ) -> Result<TranscriptAppendOutcome, TranscriptWriterError> {
        self.append_json_once_at(
            &self.root,
            &self.identity,
            transcript_relative,
            delivery_id,
            payload,
        )
    }

    /// Append under a separate approved transcript root while retaining this
    /// writer's stable session-state lock.  The caller must pass the identity
    /// of the exact transcript parent opened during target resolution; this is
    /// what lets `/cd` relocation and outbox delivery share one lock without
    /// forcing the existing cwd-dependent transcript layout under session
    /// state.
    pub fn append_json_once_at(
        &self,
        transcript_root: &Path,
        transcript_identity: &RootIdentity,
        transcript_relative: &Path,
        delivery_id: &str,
        payload: Value,
    ) -> Result<TranscriptAppendOutcome, TranscriptWriterError> {
        self.begin_transaction()?.append_json_once_at(
            transcript_root,
            transcript_identity,
            transcript_relative,
            delivery_id,
            payload,
        )
    }

    fn append_json_once_at_locked(
        &self,
        transcript_root: &Path,
        transcript_identity: &RootIdentity,
        transcript_relative: &Path,
        delivery_id: &str,
        mut payload: Value,
    ) -> Result<TranscriptAppendOutcome, TranscriptWriterError> {
        let Value::Object(object) = &mut payload else {
            return Err(TranscriptWriterError::PayloadNotObject);
        };
        let message_uuid = object
            .get("uuid")
            .and_then(Value::as_str)
            .filter(|uuid| !uuid.is_empty())
            .ok_or(TranscriptWriterError::MissingMessageUuid)?
            .to_string();
        object.insert(
            "deliveryId".to_string(),
            Value::String(delivery_id.to_string()),
        );
        let (file_present, missing_final_delimiter, existing_match) = match open_read_file_pinned(
            transcript_root,
            transcript_relative,
            Some(transcript_identity),
        ) {
            Ok(file) => {
                let (missing_delimiter, matching) =
                    self.scan_existing(file, &message_uuid, &payload)?;
                (true, missing_delimiter, matching)
            }
            Err(FsError::NotFound(_)) => (false, false, None),
            Err(error) => return Err(error.into()),
        };
        if let Some(outcome) = existing_match {
            if outcome
                .as_ref()
                .is_ok_and(|value| *value == TranscriptAppendOutcome::AlreadyPresent)
            {
                // A complete line can be visible after a process died between
                // write and fsync. Never promote that observation to a durable
                // duplicate acknowledgement until both file and directory
                // have crossed the same persistence boundary as a new append.
                self.sync_existing_match(
                    transcript_root,
                    transcript_identity,
                    transcript_relative,
                )?;
            }
            return outcome;
        }
        let mut line =
            serde_json::to_vec(&payload).map_err(|error| FsError::Io(error.to_string()))?;
        line.push(b'\n');
        if line.len() > self.max_record_bytes {
            return Err(TranscriptWriterError::ScanTooLarge {
                limit: self.max_record_bytes,
            });
        }
        if missing_final_delimiter {
            line.insert(0, b'\n');
        }
        let mut file = open_append_file_pinned(
            transcript_root,
            transcript_relative,
            Some(transcript_identity),
        )?;
        file.write_all(&line)
            .map_err(|error| FsError::Io(error.to_string()))?;
        file.sync_all()
            .map_err(|error| FsError::Io(error.to_string()))?;
        if !file_present {
            sync_parent_pinned(
                transcript_root,
                transcript_relative,
                Some(transcript_identity),
            )?;
        }
        Ok(TranscriptAppendOutcome::Appended)
    }

    fn sync_existing_match(
        &self,
        transcript_root: &Path,
        transcript_identity: &RootIdentity,
        transcript_relative: &Path,
    ) -> Result<(), TranscriptWriterError> {
        #[cfg(test)]
        if self
            .fail_next_existing_sync
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err(FsError::Io("synthetic existing transcript sync failure".into()).into());
        }
        let file = open_append_file_pinned(
            transcript_root,
            transcript_relative,
            Some(transcript_identity),
        )?;
        file.sync_all()
            .map_err(|error| FsError::Io(error.to_string()))?;
        sync_parent_pinned(
            transcript_root,
            transcript_relative,
            Some(transcript_identity),
        )?;
        Ok(())
    }

    #[cfg(test)]
    fn fail_next_existing_sync_for_test(&self) {
        self.fail_next_existing_sync
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    fn scan_existing(
        &self,
        file: std::fs::File,
        message_uuid: &str,
        payload: &Value,
    ) -> Result<
        (
            bool,
            Option<Result<TranscriptAppendOutcome, TranscriptWriterError>>,
        ),
        TranscriptWriterError,
    > {
        let expected = payload_without_delivery_id(payload.clone());
        let mut reader = BufReader::with_capacity(16 * 1024, file);
        let mut line = Vec::new();
        let mut line_offset = 0_u64;
        let mut matching = None;

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
                .ok_or(TranscriptWriterError::ScanTooLarge {
                    limit: self.max_record_bytes,
                })?;
            if record_len > self.max_record_bytes {
                return Err(TranscriptWriterError::ScanTooLarge {
                    limit: self.max_record_bytes,
                });
            }
            line.extend_from_slice(&available[..content_len]);
            reader.consume(take);
            if newline.is_some() {
                self.inspect_line(&line, line_offset, message_uuid, &expected, &mut matching)?;
                let physical_len =
                    line.len()
                        .checked_add(1)
                        .ok_or(TranscriptWriterError::ScanTooLarge {
                            limit: self.max_record_bytes,
                        })?;
                line_offset = line_offset
                    .checked_add(u64::try_from(physical_len).unwrap_or(u64::MAX))
                    .unwrap_or(u64::MAX);
                line.clear();
            }
        }

        let missing_final_delimiter = !line.is_empty();
        if missing_final_delimiter {
            self.inspect_line(&line, line_offset, message_uuid, &expected, &mut matching)?;
        }
        Ok((missing_final_delimiter, matching))
    }

    fn inspect_line(
        &self,
        line: &[u8],
        offset: u64,
        message_uuid: &str,
        expected: &Value,
        matching: &mut Option<Result<TranscriptAppendOutcome, TranscriptWriterError>>,
    ) -> Result<(), TranscriptWriterError> {
        if line.is_empty() {
            return Ok(());
        }
        let value: Value = serde_json::from_slice(line)
            .map_err(|_| TranscriptWriterError::CorruptLine { offset })?;
        if value.get("uuid").and_then(Value::as_str) == Some(message_uuid) {
            let outcome = if payload_without_delivery_id(value) == *expected {
                Ok(TranscriptAppendOutcome::AlreadyPresent)
            } else {
                Err(TranscriptWriterError::DeliveryConflict {
                    delivery_id: message_uuid.to_string(),
                })
            };
            if matching.is_none() {
                *matching = Some(outcome);
            } else if matching.as_ref().is_some_and(Result::is_ok) && outcome.is_err() {
                *matching = Some(outcome);
            }
        }
        Ok(())
    }

    /// Append a pre-serialized JSON object exactly once.  This is useful for
    /// callers that already have a stable wire DTO and want to avoid a second
    /// serialization pass; the method still canonicalizes the delivery id.
    pub fn append_object_once(
        &self,
        transcript_relative: &Path,
        delivery_id: &str,
        payload: Map<String, Value>,
    ) -> Result<TranscriptAppendOutcome, TranscriptWriterError> {
        self.append_json_once(transcript_relative, delivery_id, Value::Object(payload))
    }
}

impl DurableTranscriptTransaction<'_> {
    /// Append under the writer's own pinned root without reacquiring the lock.
    pub fn append_json_once(
        &self,
        transcript_relative: &Path,
        delivery_id: &str,
        payload: Value,
    ) -> Result<TranscriptAppendOutcome, TranscriptWriterError> {
        self.append_json_once_at(
            &self.writer.root,
            &self.writer.identity,
            transcript_relative,
            delivery_id,
            payload,
        )
    }

    /// Append under an approved transcript root while retaining the one
    /// session-state transaction lock acquired by this guard.
    pub fn append_json_once_at(
        &self,
        transcript_root: &Path,
        transcript_identity: &RootIdentity,
        transcript_relative: &Path,
        delivery_id: &str,
        payload: Value,
    ) -> Result<TranscriptAppendOutcome, TranscriptWriterError> {
        self.writer.append_json_once_at_locked(
            transcript_root,
            transcript_identity,
            transcript_relative,
            delivery_id,
            payload,
        )
    }
}

fn payload_without_delivery_id(mut payload: Value) -> Value {
    if let Value::Object(object) = &mut payload {
        object.remove("deliveryId");
    }
    payload
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn stable_delivery_id_is_append_once() {
        let dir = tempfile::tempdir().unwrap();
        let writer = DurableTranscriptWriter::open(dir.path()).unwrap();
        let path = Path::new("transcript.jsonl");
        assert_eq!(
            writer
                .append_json_once(path, "delivery-1", json!({"uuid":"message-1","text":"ok"}),)
                .unwrap(),
            TranscriptAppendOutcome::Appended
        );
        assert_eq!(
            writer
                .append_json_once(path, "delivery-2", json!({"uuid":"message-1","text":"ok"}),)
                .unwrap(),
            TranscriptAppendOutcome::AlreadyPresent
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join(path))
                .unwrap()
                .lines()
                .count(),
            1
        );
    }

    #[test]
    fn identical_match_is_not_acknowledged_when_its_durability_sync_fails() {
        let dir = tempfile::tempdir().unwrap();
        let writer = DurableTranscriptWriter::open(dir.path()).unwrap();
        let path = Path::new("transcript.jsonl");
        let payload = json!({"uuid":"message-1","text":"ok"});
        assert_eq!(
            writer
                .append_json_once(path, "delivery-1", payload.clone())
                .unwrap(),
            TranscriptAppendOutcome::Appended
        );

        writer.fail_next_existing_sync_for_test();
        assert!(matches!(
            writer.append_json_once(path, "delivery-1", payload.clone()),
            Err(TranscriptWriterError::Fs(FsError::Io(message)))
                if message.contains("existing transcript sync failure")
        ));

        assert_eq!(
            writer
                .append_json_once(path, "delivery-1", payload)
                .unwrap(),
            TranscriptAppendOutcome::AlreadyPresent
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join(path))
                .unwrap()
                .lines()
                .count(),
            1
        );
    }

    #[test]
    fn conflicting_delivery_id_fails_without_second_line() {
        let dir = tempfile::tempdir().unwrap();
        let writer = DurableTranscriptWriter::open(dir.path()).unwrap();
        let path = Path::new("transcript.jsonl");
        writer
            .append_json_once(path, "delivery-1", json!({"uuid":"message-1","text":"ok"}))
            .unwrap();
        assert!(matches!(
            writer.append_json_once(
                path,
                "delivery-2",
                json!({"uuid":"message-1","text":"different"}),
            ),
            Err(TranscriptWriterError::DeliveryConflict { .. })
        ));
        assert_eq!(
            std::fs::read_to_string(dir.path().join(path))
                .unwrap()
                .lines()
                .count(),
            1
        );
    }

    #[test]
    fn one_transaction_resolves_target_and_appends_without_recursive_locking() {
        let state_dir = tempfile::tempdir().unwrap();
        let transcript_dir = tempfile::tempdir().unwrap();
        let writer = DurableTranscriptWriter::open(state_dir.path()).unwrap();
        let path = Path::new("transcript.jsonl");

        let first = writer
            .with_transaction(|transaction| {
                let transcript_identity = root_identity(transcript_dir.path())?;
                transaction.append_json_once_at(
                    transcript_dir.path(),
                    &transcript_identity,
                    path,
                    "delivery-1",
                    json!({"uuid":"message-1","text":"ok"}),
                )
            })
            .unwrap();
        assert_eq!(first, TranscriptAppendOutcome::Appended);

        let duplicate = writer
            .with_lock(|transaction| {
                let transcript_identity = root_identity(transcript_dir.path())?;
                transaction.append_json_once_at(
                    transcript_dir.path(),
                    &transcript_identity,
                    path,
                    "delivery-1",
                    json!({"uuid":"message-1","text":"ok"}),
                )
            })
            .unwrap();
        assert_eq!(duplicate, TranscriptAppendOutcome::AlreadyPresent);
        assert_eq!(
            std::fs::read_to_string(transcript_dir.path().join(path))
                .unwrap()
                .lines()
                .count(),
            1
        );
    }

    #[test]
    fn long_transcript_is_not_rejected_when_each_record_is_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let writer = DurableTranscriptWriter::open(dir.path())
            .unwrap()
            .with_max_scan_bytes(128);
        let path = Path::new("transcript.jsonl");
        let mut existing = std::fs::File::create(dir.path().join(path)).unwrap();
        for index in 0..200_u32 {
            serde_json::to_writer(
                &mut existing,
                &json!({"uuid": format!("old-{index}"), "text":"ok"}),
            )
            .unwrap();
            existing.write_all(b"\n").unwrap();
        }
        existing.sync_all().unwrap();
        assert!(std::fs::metadata(dir.path().join(path)).unwrap().len() > 128);

        assert_eq!(
            writer
                .append_json_once(
                    path,
                    "delivery-new",
                    json!({"uuid":"message-new","text":"ok"}),
                )
                .unwrap(),
            TranscriptAppendOutcome::Appended
        );
    }

    #[cfg(unix)]
    #[test]
    fn transcript_read_errors_never_bypass_idempotency_scan() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let writer = DurableTranscriptWriter::open(dir.path()).unwrap();
        let path = Path::new("transcript.jsonl");
        symlink("missing-target", dir.path().join(path)).unwrap();

        assert!(matches!(
            writer.append_json_once(path, "delivery-1", json!({"uuid":"message-1","text":"ok"}),),
            Err(TranscriptWriterError::Fs(_))
        ));
    }
}

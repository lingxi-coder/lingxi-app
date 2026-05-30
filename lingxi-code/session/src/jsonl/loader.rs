//! Resume-time session enumeration + load + chain validation + interactive picker.
//!
//! 1:1 port of `claude-code/src/commands/resume/` + `sessionStorage.ts::loadSameRepoMessageLogs`,
//! with the Ink TUI replaced by a stdio line-based picker (OQ-6 fallback).
//!
//! See plan `docs/superpowers/plans/2026-05-25-m5-08-resume.md` Task 0 for byte-locks.

use crate::jsonl::path::{project_dir_name, session_path};
use crate::jsonl::reader::JsonlReader;
use crate::jsonl::schema::JsonlMessage;
use crate::jsonl::title::extract_title;
use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use traits::FileSystem;
use uuid::Uuid;

/// Metadata for one resumable session row (uuid + title + mtime + line count).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionMetadata {
    /// The session UUID parsed from the filename stem.
    pub uuid: Uuid,
    /// The session title (extracted via [`crate::jsonl::title::extract_title`]; ≤ 50 chars + ellipsis).
    pub title: String,
    /// File mtime (UTC `SystemTime`).
    pub modified: SystemTime,
    /// Number of JSONL lines in the file.
    pub message_count: usize,
    /// Absolute path to the `.jsonl` file (kept so callers can re-load without re-resolving).
    pub path: PathBuf,
}

impl Ord for SessionMetadata {
    fn cmp(&self, other: &Self) -> Ordering {
        // Newest-first (mtime desc), tie-break by filename asc.
        other
            .modified
            .cmp(&self.modified)
            .then_with(|| self.path.cmp(&other.path))
    }
}

impl PartialOrd for SessionMetadata {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// All errors raised by the resume layer.
#[derive(Debug, Error)]
pub enum LoaderError {
    /// The given session id / arg did not resolve to a `.jsonl` file under the cwd's project dir.
    #[error("Session {arg} was not found.")]
    SessionNotFound {
        /// The arg (UUID string) that was looked up.
        arg: String,
    },
    /// The `parentUuid` chain is broken at the named message.
    #[error("Session {arg} corrupted: parentUuid chain broken at message {at_uuid}.")]
    ChainBroken {
        /// The session arg.
        arg: String,
        /// UUID of the offending message.
        at_uuid: Uuid,
    },
    /// Two or more messages in the file claim different `sessionId` values.
    #[error("Session {arg} corrupted: sessionId mismatch (expected {expected}, got {got}).")]
    SessionIdMismatch {
        /// The session arg.
        arg: String,
        /// The session id expected (filename-derived).
        expected: Uuid,
        /// The session id observed in the offending row.
        got: Uuid,
    },
    /// The interactive picker received 3 invalid inputs in a row.
    #[error("Invalid selection (3 attempts). Aborting.")]
    InvalidSelection,
    /// I/O error during dir listing / file open / file read.
    #[error("Session {arg} I/O error: {source}")]
    Io {
        /// The path / arg the I/O was attempted against.
        arg: String,
        /// Wrapped I/O failure.
        #[source]
        source: std::io::Error,
    },
    /// The cwd's project dir has no `.jsonl` files at all.
    #[error("No conversations found to resume.")]
    EmptyDirectory,
    /// A `.jsonl` file failed to deserialize (delegates to the reader's error).
    #[error("Session {arg} parse error: {source}")]
    Parse {
        /// The arg / path.
        arg: String,
        /// Wrapped parse failure.
        #[source]
        source: serde_json::Error,
    },
}

/// Resolve `<claude_home>/projects/<sanitize(cwd)>[-djb2]` for a given cwd.
fn project_dir_for_cwd(claude_home: &Path, cwd: &str) -> PathBuf {
    claude_home.join("projects").join(project_dir_name(cwd))
}

/// Resolve the project dir for `cwd` and return up to `limit` most-recently-modified
/// `.jsonl` files as [`SessionMetadata`] rows, sorted by mtime desc (filename asc on tie).
///
/// Errors:
/// - [`LoaderError::EmptyDirectory`] if the project dir doesn't exist OR contains no `.jsonl`.
/// - [`LoaderError::Io`] on any other I/O failure.
///
/// Each row's `title` is read via [`crate::jsonl::title::extract_title`] from the **full**
/// JSONL content (we open + parse every candidate, then sort + truncate). This is O(N * lines)
/// for N sessions; for the typical N ≤ 5 case (the picker limit) the cost is trivial.
///
/// Locked against `claude-code/src/utils/sessionStorage.ts::loadSameRepoMessageLogs` — except:
/// - claude-code uses a 16-KiB head-only `enrichLogs` scan for the first user message; we
///   open + fully-parse because our `JsonlReader::read_all` is already in hand from M5-07.
/// - claude-code includes worktrees; we list ONLY the exact cwd's project dir (cross-worktree
///   resume is deferred to a follow-up — spec §3 M5-08 row does not require it).
pub async fn list_recent_sessions(
    claude_home: &Path,
    cwd: &str,
    limit: usize,
    fs: Arc<dyn FileSystem>,
) -> Result<Vec<SessionMetadata>, LoaderError> {
    let project_dir = project_dir_for_cwd(claude_home, cwd);
    let mut entries = match tokio::fs::read_dir(&project_dir).await {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(LoaderError::EmptyDirectory);
        }
        Err(source) => {
            return Err(LoaderError::Io {
                arg: project_dir.display().to_string(),
                source,
            });
        }
    };

    let mut rows: Vec<SessionMetadata> = Vec::new();
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|source| LoaderError::Io {
            arg: project_dir.display().to_string(),
            source,
        })?
    {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            continue;
        }
        let metadata = entry.metadata().await.map_err(|source| LoaderError::Io {
            arg: path.display().to_string(),
            source,
        })?;
        let modified = metadata.modified().map_err(|source| LoaderError::Io {
            arg: path.display().to_string(),
            source,
        })?;

        // Parse uuid from filename stem; silently skip non-UUID files.
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let Ok(uuid) = Uuid::parse_str(stem) else {
            continue;
        };

        let reader = JsonlReader::new(path.clone(), fs.clone());
        let messages = reader.read_all().await.map_err(|e| LoaderError::Io {
            arg: path.display().to_string(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()),
        })?;
        let title = extract_title(&messages);
        rows.push(SessionMetadata {
            uuid,
            title,
            modified,
            message_count: messages.len(),
            path,
        });
    }

    if rows.is_empty() {
        return Err(LoaderError::EmptyDirectory);
    }

    rows.sort();
    rows.truncate(limit);
    Ok(rows)
}

/// Load a session by UUID, validate its `parentUuid` chain + `sessionId` consistency,
/// and return the deserialized `Vec<JsonlMessage>` in file order.
///
/// Validation rules (spec §4.x):
/// 1. The first message's `parent_uuid` is `None` (root of the chain).
/// 2. Every subsequent message's `parent_uuid` MUST equal the previous message's `uuid`.
/// 3. All messages MUST share the same `session_id` (matching the requested `session_id` arg).
///
/// Errors:
/// - [`LoaderError::SessionNotFound`] if the file doesn't exist.
/// - [`LoaderError::ChainBroken`] if rule 1 or 2 fails.
/// - [`LoaderError::SessionIdMismatch`] if rule 3 fails.
/// - [`LoaderError::Io`] on disk-or-format issues.
pub async fn load_session(
    claude_home: &Path,
    cwd: &str,
    session_id: Uuid,
    fs: Arc<dyn FileSystem>,
) -> Result<Vec<JsonlMessage>, LoaderError> {
    let arg = session_id.to_string();
    let path = session_path(claude_home, cwd, &arg);
    if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
        return Err(LoaderError::SessionNotFound { arg });
    }
    let reader = JsonlReader::new(path, fs);
    let messages = reader.read_all().await.map_err(|e| LoaderError::Io {
        arg: arg.clone(),
        source: std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()),
    })?;
    validate_chain(&messages, session_id, &arg)?;
    Ok(messages)
}

/// Interactive line-based session picker (OQ-6 stdio fallback for the Ink TUI).
///
/// Renders:
/// ```text
/// Resume which session?
///   1. {title} [{modified}]
///   2. {title} [{modified}]
///   ...
/// >
/// ```
///
/// Behavior:
/// - Empty input line → `Ok(None)` (cancel).
/// - `1..=sessions.len()` (1-indexed) → `Ok(Some(uuid))`.
/// - Non-numeric, out-of-range, or `> sessions.len()` → print retry feedback,
///   try again up to **3 total attempts** (initial + 2 retries).
/// - After 3 failed attempts → `Err(LoaderError::InvalidSelection)`.
/// - EOF (zero-byte read) → `Ok(None)` (cancel).
///
/// `stdin` is any `AsyncBufRead`, `stdout` is any `AsyncWrite` — both passed
/// in so tests can supply `tokio::io::duplex` pairs (mirrors M5-05 permission
/// UX pattern).
///
/// Errors:
/// - [`LoaderError::EmptyDirectory`] if `sessions` is empty.
/// - [`LoaderError::InvalidSelection`] after 3 invalid inputs.
/// - [`LoaderError::Io`] on stdio read/write/flush failure.
pub async fn select_session_interactive<R, W>(
    sessions: &[SessionMetadata],
    stdin: &mut BufReader<R>,
    stdout: &mut W,
) -> Result<Option<Uuid>, LoaderError>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    if sessions.is_empty() {
        return Err(LoaderError::EmptyDirectory);
    }
    let limit = sessions.len();

    // Render header + rows once.
    let mut out_buf = String::from("Resume which session?\n");
    for (i, row) in sessions.iter().enumerate() {
        let modified_rfc3339 = format_rfc3339_seconds(row.modified);
        out_buf.push_str(&format!(
            "  {}. {} [{}]\n",
            i + 1,
            row.title,
            modified_rfc3339
        ));
    }
    stdout
        .write_all(out_buf.as_bytes())
        .await
        .map_err(|source| LoaderError::Io {
            arg: "stdout".into(),
            source,
        })?;

    for _attempt in 0..3 {
        stdout
            .write_all(b"> ")
            .await
            .map_err(|source| LoaderError::Io {
                arg: "stdout".into(),
                source,
            })?;
        stdout.flush().await.map_err(|source| LoaderError::Io {
            arg: "stdout".into(),
            source,
        })?;

        let mut line = String::new();
        let n = stdin
            .read_line(&mut line)
            .await
            .map_err(|source| LoaderError::Io {
                arg: "stdin".into(),
                source,
            })?;
        if n == 0 {
            // EOF — treat as cancel.
            return Ok(None);
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }
        match trimmed.parse::<usize>() {
            Ok(n) if (1..=limit).contains(&n) => {
                return Ok(Some(sessions[n - 1].uuid));
            }
            _ => {
                let msg = format!("Please enter a number from 1 to {limit}, or empty to cancel.\n");
                stdout
                    .write_all(msg.as_bytes())
                    .await
                    .map_err(|source| LoaderError::Io {
                        arg: "stdout".into(),
                        source,
                    })?;
            }
        }
    }
    Err(LoaderError::InvalidSelection)
}

/// RFC 3339 with second precision and `Z` suffix — e.g. `2026-05-24T19:03:12Z`.
///
/// Shared formatter for the resume surfaces: the M5-08 stdio picker above and
/// the M7-12 iocraft Resume screen (`tui::screens::resume`) both call
/// this so the two surfaces render timestamps byte-for-byte identically. Pre-1970
/// inputs (never produced by file mtime on the platforms we target) fall back to
/// the Unix epoch literal.
#[must_use]
pub fn format_rfc3339_seconds(t: SystemTime) -> String {
    use std::time::UNIX_EPOCH;
    let secs = t
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Cast to i64; pre-1970 timestamps are not produced by the OS for files we
    // care about (mtime). The session picker only ever sees positive offsets.
    #[allow(clippy::cast_possible_wrap)]
    let secs_i64 = secs as i64;
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs_i64, 0).map_or_else(
        || "1970-01-01T00:00:00Z".to_string(),
        |dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
    )
}

fn validate_chain(
    messages: &[JsonlMessage],
    expected_session_id: Uuid,
    arg: &str,
) -> Result<(), LoaderError> {
    let mut prev_uuid: Option<Uuid> = None;
    for (i, m) in messages.iter().enumerate() {
        // (Rule 3) session_id consistency.
        let msg_session =
            Uuid::parse_str(&m.session_id).map_err(|_| LoaderError::SessionIdMismatch {
                arg: arg.to_string(),
                expected: expected_session_id,
                got: Uuid::nil(),
            })?;
        if msg_session != expected_session_id {
            return Err(LoaderError::SessionIdMismatch {
                arg: arg.to_string(),
                expected: expected_session_id,
                got: msg_session,
            });
        }
        let msg_uuid = Uuid::parse_str(&m.uuid).map_err(|_| LoaderError::ChainBroken {
            arg: arg.to_string(),
            at_uuid: Uuid::nil(),
        })?;
        let msg_parent = m
            .parent_uuid
            .as_deref()
            .map(Uuid::parse_str)
            .transpose()
            .map_err(|_| LoaderError::ChainBroken {
                arg: arg.to_string(),
                at_uuid: msg_uuid,
            })?;
        if i == 0 {
            // (Rule 1) first message must have parent_uuid == None.
            if msg_parent.is_some() {
                return Err(LoaderError::ChainBroken {
                    arg: arg.to_string(),
                    at_uuid: msg_uuid,
                });
            }
        } else {
            // (Rule 2) parent_uuid must equal previous message's uuid.
            if msg_parent != prev_uuid {
                return Err(LoaderError::ChainBroken {
                    arg: arg.to_string(),
                    at_uuid: msg_uuid,
                });
            }
        }
        prev_uuid = Some(msg_uuid);
    }
    Ok(())
}

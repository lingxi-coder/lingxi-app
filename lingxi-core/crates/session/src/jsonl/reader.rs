//! JSONL reader — full parse + lite head-only metadata.
//!
//! Lite read mirrors `claude-code/src/utils/sessionStoragePortable.ts:215-282`
//! (`readSessionLite` head path) — we only need the head because the fields
//! we extract (`sessionId`, `cwd`, `type`) live on line 1.

use crate::jsonl::schema::JsonlMessage;
use lingxi_traits::{FileSystem, FsError};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;

/// Failure modes for [`JsonlReader`].
#[derive(Debug, Error)]
pub enum ReaderError {
    /// Underlying filesystem error.
    #[error(transparent)]
    Fs(#[from] FsError),
    /// Line `n` (0-indexed) failed to parse as `JsonlMessage`.
    #[error("parse failure at line {0}: {1}")]
    Parse(usize, String),
    /// First line didn't contain a required metadata field.
    #[error("lite read: missing field {0}")]
    LiteMissing(&'static str),
}

/// Lite metadata — populated from the first JSON line only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionMetadata {
    /// `sessionId` from the first line's outer object.
    pub session_id: String,
    /// `cwd` from the first line's outer object.
    pub cwd: String,
    /// `type` from the first line's outer object (e.g. `"user"`).
    pub first_type: String,
}

/// Reader for one session's `<uuid>.jsonl`.
pub struct JsonlReader {
    path: PathBuf,
    fs: Arc<dyn FileSystem>,
}

impl JsonlReader {
    /// Construct a reader. No I/O until `read_all`/`read_lite` is called.
    #[must_use]
    pub fn new(path: PathBuf, fs: Arc<dyn FileSystem>) -> Self {
        Self { path, fs }
    }

    /// Path on disk.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read every line, parse each as `JsonlMessage`, return in file order.
    pub async fn read_all(&self) -> Result<Vec<JsonlMessage>, ReaderError> {
        let path_str = self.path.to_str().expect("UTF-8 path");
        let content = self.fs.read_file(path_str, None, None).await?.content;
        let mut out = Vec::new();
        for (idx, line) in content.lines().enumerate() {
            if line.is_empty() {
                continue;
            }
            let msg: JsonlMessage =
                serde_json::from_str(line).map_err(|e| ReaderError::Parse(idx, e.to_string()))?;
            out.push(msg);
        }
        Ok(out)
    }

    /// Read up to `LITE_READ_BUF_SIZE` bytes from the file head and extract
    /// `sessionId` / `cwd` / `type` from line 1 using
    /// [`extract_json_string_field`] (no full parse — works even if line 1
    /// is the only complete line in the buffer).
    pub async fn read_lite(&self) -> Result<SessionMetadata, ReaderError> {
        let path_str = self.path.to_str().expect("UTF-8 path");
        // Read first ~64 KiB worth of lines. The trait's `read_file` window
        // is line-indexed (offset/limit are line counts, not byte offsets),
        // so we ask for the full file and inspect line 1 only — line 1 is
        // always small enough for the lite path to be cheap. The
        // `LITE_READ_BUF_SIZE` byte budget is preserved here as a guard:
        // we slice the first N bytes of the read result for downstream
        // extraction so very long line-1 payloads still cap at 64 KiB.
        let read = self.fs.read_file(path_str, None, None).await?;
        let head = if read.content.len() > super::LITE_READ_BUF_SIZE {
            &read.content[..super::LITE_READ_BUF_SIZE]
        } else {
            read.content.as_str()
        };
        let line1 = head.split('\n').next().unwrap_or("");
        let session_id = extract_json_string_field(line1, "sessionId")
            .ok_or(ReaderError::LiteMissing("sessionId"))?;
        let cwd = extract_json_string_field(line1, "cwd").ok_or(ReaderError::LiteMissing("cwd"))?;
        let first_type =
            extract_json_string_field(line1, "type").ok_or(ReaderError::LiteMissing("type"))?;
        Ok(SessionMetadata {
            session_id,
            cwd,
            first_type,
        })
    }
}

/// 1:1 port of `claude-code/src/utils/sessionStoragePortable.ts:53-76`.
///
/// Looks for `"key":"value"` or `"key": "value"` (one optional space after
/// the colon). Returns the first match. `\` escapes the next char inside
/// the value. The closing `"` ends the value.
#[must_use]
pub fn extract_json_string_field(text: &str, key: &str) -> Option<String> {
    let patterns = [format!("\"{key}\":\""), format!("\"{key}\": \"")];
    let bytes = text.as_bytes();
    for pat in &patterns {
        let pat_bytes = pat.as_bytes();
        if let Some(idx) = find_subslice(bytes, pat_bytes) {
            let value_start = idx + pat_bytes.len();
            let mut i = value_start;
            while i < bytes.len() {
                if bytes[i] == b'\\' {
                    i = i.saturating_add(2);
                    continue;
                }
                if bytes[i] == b'"' {
                    let raw = &text[value_start..i];
                    return Some(unescape_json_string(raw));
                }
                i += 1;
            }
        }
    }
    None
}

/// 1:1 port of `claude-code/src/utils/sessionStoragePortable.ts:39-46`.
#[must_use]
pub fn unescape_json_string(raw: &str) -> String {
    if !raw.contains('\\') {
        return raw.to_string();
    }
    let wrapped = format!("\"{raw}\"");
    serde_json::from_str::<String>(&wrapped).unwrap_or_else(|_| raw.to_string())
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    (0..=haystack.len() - needle.len()).find(|&i| &haystack[i..i + needle.len()] == needle)
}

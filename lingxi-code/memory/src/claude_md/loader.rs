//! File reader with 10 MB cap.

use crate::MAX_MEMORY_FILE_SIZE;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;

/// One loaded CLAUDE.md (or local override) file.
#[derive(Debug, Clone)]
pub struct LoadedFile {
    /// Absolute path the file was loaded from.
    pub path: PathBuf,
    /// File body (post-cap, secrets NOT yet redacted at this layer).
    pub body: String,
    /// File size in bytes at the time of load (pre-redaction).
    pub size_bytes: u64,
}

/// Failure modes the loader can encounter.
#[derive(Debug, Error)]
pub enum LoaderError {
    /// I/O error reading the file.
    #[error("io: {0}")]
    Io(String),
    /// File exceeded `MAX_MEMORY_FILE_SIZE`; skipped (event emitted).
    #[error("file too large: {bytes} bytes at {path}")]
    FileTooLarge {
        /// Path of the oversized file.
        path: PathBuf,
        /// Observed size in bytes.
        bytes: u64,
    },
}

/// Telemetry event name emitted when a file exceeds `MAX_MEMORY_FILE_SIZE`.
pub const TENGU_MEMORY_FILE_TOO_LARGE: &str = "tengu_memory_file_too_large";

/// Load one CLAUDE.md (or local override) with the 10 MB cap.
///
/// Returns `Err(FileTooLarge)` when the file exceeds the cap. The caller
/// is expected to log the event via [`emit_file_too_large`] and continue
/// processing the remaining files — oversized files are skipped, never
/// fatal.
///
/// # Errors
///
/// - [`LoaderError::Io`] for filesystem errors (file unreadable, perms).
/// - [`LoaderError::FileTooLarge`] when the size exceeds the cap.
pub fn load_file(
    path: &Path,
    _bus: Option<&Arc<telemetry::AnalyticsBus>>,
) -> Result<LoadedFile, LoaderError> {
    let meta = std::fs::metadata(path).map_err(|e| LoaderError::Io(e.to_string()))?;
    let size = meta.len();
    // On 32-bit targets a >4 GB file overflows `usize`; in that case it
    // is by definition over the 10 MB cap, so treat the conversion failure
    // as "too large" rather than rejecting it as an I/O error.
    let over_cap = match usize::try_from(size) {
        Ok(n) => n > MAX_MEMORY_FILE_SIZE,
        Err(_) => true,
    };
    if over_cap {
        return Err(LoaderError::FileTooLarge {
            path: path.to_path_buf(),
            bytes: size,
        });
    }
    let body = std::fs::read_to_string(path).map_err(|e| LoaderError::Io(e.to_string()))?;
    Ok(LoadedFile {
        path: path.to_path_buf(),
        body,
        size_bytes: size,
    })
}

/// Emit `tengu_memory_file_too_large` when a file was skipped.
///
/// No-op when `bus` is `None`. Payload keys (locked):
/// `_PROTO_path: PiiTagged(<path>)`, `size_bytes: Int(N)`.
pub async fn emit_file_too_large(
    bus: Option<&Arc<telemetry::AnalyticsBus>>,
    path: &Path,
    bytes: u64,
) {
    let Some(bus) = bus else {
        return;
    };
    let mut md = telemetry::sink::LogEventMetadata::new();
    md.insert(
        "_PROTO_path".into(),
        telemetry::sink::AnalyticsValue::String(
            telemetry::pii::PiiTagged::assert_pii_tagged_column(path.display().to_string())
                .into_inner(),
        ),
    );
    // Files this big are pathological — saturate rather than wrap so the
    // event still carries a sensible value if `bytes > i64::MAX`.
    let size_int = i64::try_from(bytes).unwrap_or(i64::MAX);
    md.insert(
        "size_bytes".into(),
        telemetry::sink::AnalyticsValue::Int(size_int),
    );
    bus.log_event(TENGU_MEMORY_FILE_TOO_LARGE, md).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MAX_MEMORY_FILE_SIZE;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn loads_small_file_into_loadedfile() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("CLAUDE.md");
        fs::write(&p, b"# notes\nhello\n").unwrap();
        let out = load_file(&p, None).unwrap();
        assert_eq!(out.path, p);
        assert!(out.body.contains("hello"));
        assert_eq!(out.size_bytes, 14);
    }

    #[test]
    fn skips_oversized_file_returns_file_too_large() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("CLAUDE.md");
        let bytes = vec![b'a'; MAX_MEMORY_FILE_SIZE + 1];
        fs::write(&p, &bytes).unwrap();
        match load_file(&p, None) {
            Err(LoaderError::FileTooLarge { path, bytes }) => {
                assert_eq!(path, p);
                assert_eq!(bytes, u64::try_from(MAX_MEMORY_FILE_SIZE + 1).unwrap());
            }
            other => panic!("expected FileTooLarge, got {other:?}"),
        }
    }

    #[test]
    fn file_at_exact_cap_is_loaded() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("CLAUDE.md");
        fs::write(&p, vec![b'a'; MAX_MEMORY_FILE_SIZE]).unwrap();
        let out = load_file(&p, None).unwrap();
        assert_eq!(out.size_bytes, u64::try_from(MAX_MEMORY_FILE_SIZE).unwrap());
    }

    #[tokio::test]
    async fn emit_file_too_large_writes_event_with_size() {
        use std::sync::{Arc, Mutex};
        use telemetry::{sink::LogEventMetadata, AnalyticsBus, AnalyticsSink, AnalyticsValue};

        struct Cap {
            events: Mutex<Vec<(String, LogEventMetadata)>>,
        }
        #[async_trait::async_trait]
        impl AnalyticsSink for Cap {
            async fn log_event(&self, n: &str, m: LogEventMetadata) {
                self.events.lock().unwrap().push((n.into(), m));
            }
            async fn log_event_async(&self, n: &str, m: LogEventMetadata) {
                self.log_event(n, m).await;
            }
            fn name(&self) -> &str {
                "cap"
            }
        }

        let bus = Arc::new(AnalyticsBus::new());
        let sink = Arc::new(Cap {
            events: Mutex::new(Vec::new()),
        });
        bus.attach_sink(sink.clone()).await;
        emit_file_too_large(
            Some(&bus),
            std::path::Path::new("/x/CLAUDE.md"),
            11 * 1024 * 1024,
        )
        .await;
        let ev = sink.events.lock().unwrap();
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].0, "tengu_memory_file_too_large");
        matches!(ev[0].1.get("size_bytes"), Some(AnalyticsValue::Int(_)));
    }
}

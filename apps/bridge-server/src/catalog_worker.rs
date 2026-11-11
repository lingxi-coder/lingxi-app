//! Private, bounded Desktop metadata worker. No provider/runtime initialization.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use lingxi_core::host::{FileSystem, FileSystemCacheIdentity};
use serde::{Deserialize, Serialize};
use session::jsonl::SessionDiscoveryScope;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

const MAX_REQUEST_BYTES: usize = 64 * 1024;
const MAX_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
const MAX_FILESYSTEMS: usize = 16;
const EMPTY_MARKER_CACHE_BYTES: usize = 1024 * 1024;
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    version: u8,
    id: u64,
    cwd: PathBuf,
    session_id: Option<String>,
    scope: Option<String>,
    transcript_path: Option<PathBuf>,
}

#[derive(Serialize)]
struct Response {
    version: u8,
    id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

struct Adapter {
    cwd: PathBuf,
    home: PathBuf,
    fs: Arc<dyn FileSystem>,
}

struct CatalogWorker {
    home: PathBuf,
    adapters: VecDeque<Adapter>,
    empty_markers: EmptyMarkerCache,
}

#[derive(PartialEq, Eq)]
struct Revision {
    len: u64,
    modified: SystemTime,
    created: Option<SystemTime>,
}

impl Revision {
    async fn read(path: &std::path::Path) -> Option<Self> {
        let metadata = tokio::fs::metadata(path).await.ok()?;
        Some(Self {
            len: metadata.len(),
            modified: metadata.modified().ok()?,
            created: metadata.created().ok(),
        })
    }
}

struct EmptyMarkerEntry {
    identity: FileSystemCacheIdentity,
    path: PathBuf,
    revision: Revision,
    empty: bool,
    bytes: usize,
}

/// Only a derived anchor bit is retained; exact-source captures remain uncached.
#[derive(Default)]
pub(super) struct EmptyMarkerCache {
    entries: VecDeque<EmptyMarkerEntry>,
    bytes: usize,
}

impl EmptyMarkerCache {
    pub(super) async fn is_empty(
        &mut self,
        row: &session::jsonl::SessionMetadata,
        fs: Arc<dyn FileSystem>,
    ) -> bool {
        let identity = fs.cache_identity();
        let revision = Revision::read(&row.path).await;
        if let (Some(identity), Some(revision)) = (&identity, &revision) {
            if let Some(index) = self.entries.iter().position(|entry| {
                entry.identity == *identity && entry.path == row.path && entry.revision == *revision
            }) {
                let entry = self.entries.remove(index).expect("a located marker exists");
                self.bytes -= entry.bytes;
                // OS timestamps do not report ACL/permission revocation.
                // Require a successful current backend read and stable source
                // revision before reusing the derived anchor bit.
                let readable = if let Some(path) = row.path.to_str() {
                    fs.read_file_prefix(path, 1).await.is_ok()
                } else {
                    false
                };
                if readable && Revision::read(&row.path).await.as_ref() == Some(revision) {
                    let empty = entry.empty;
                    self.bytes += entry.bytes;
                    self.entries.push_back(entry);
                    return empty;
                }
                // Preserve the original full-reader result on a denied or
                // unstable source instead of treating cache metadata as access.
            }
        }
        let Ok(loaded) = session::jsonl::JsonlReader::new(row.path.clone(), fs)
            .read_routed()
            .await
        else {
            return false;
        };
        let empty = loaded.messages_in_order.is_empty()
            && loaded.mobile_empty_sessions.contains(&row.uuid.to_string());
        if let (Some(identity), Some(revision)) = (identity, revision) {
            // Never publish a bit captured across a concurrently changed source.
            if Revision::read(&row.path).await.as_ref() != Some(&revision) {
                return empty;
            }
            if let Some(index) = self
                .entries
                .iter()
                .position(|entry| entry.identity == identity && entry.path == row.path)
            {
                self.bytes -= self
                    .entries
                    .remove(index)
                    .expect("a located marker exists")
                    .bytes;
            }
            let bytes = std::mem::size_of::<EmptyMarkerEntry>()
                + row.path.as_os_str().as_encoded_bytes().len()
                + format!("{identity:?}").len();
            if bytes > EMPTY_MARKER_CACHE_BYTES {
                return empty;
            }
            while self.bytes.saturating_add(bytes) > EMPTY_MARKER_CACHE_BYTES {
                let Some(entry) = self.entries.pop_front() else {
                    break;
                };
                self.bytes -= entry.bytes;
            }
            self.bytes += bytes;
            self.entries.push_back(EmptyMarkerEntry {
                identity,
                path: row.path.clone(),
                revision,
                empty,
                bytes,
            });
        }
        empty
    }
}

impl CatalogWorker {
    fn new(home: PathBuf) -> Self {
        Self {
            home,
            adapters: VecDeque::new(),
            empty_markers: EmptyMarkerCache::default(),
        }
    }

    fn filesystem(&mut self, cwd: PathBuf, home: PathBuf) -> Arc<dyn FileSystem> {
        let adapter = self
            .adapters
            .iter()
            .position(|entry| entry.cwd == cwd && entry.home == home)
            .and_then(|index| self.adapters.remove(index))
            .unwrap_or_else(|| Adapter {
                fs: Arc::new(crate::HostFileSystem::new(cwd.clone())),
                cwd,
                home,
            });
        let fs = Arc::clone(&adapter.fs);
        self.adapters.push_back(adapter);
        if self.adapters.len() > MAX_FILESYSTEMS {
            self.adapters.pop_front();
        }
        fs
    }

    async fn list(&mut self, request: &Request) -> Result<String, String> {
        if request.version != 1 || request.id > 9_007_199_254_740_991 || !request.cwd.is_absolute()
        {
            return Err("invalid session catalog worker request".into());
        }
        let cwd = tokio::fs::canonicalize(&request.cwd)
            .await
            .map_err(|_| "session catalog workspace is unavailable".to_string())?;
        let home = if self.home.is_absolute() {
            self.home.clone()
        } else {
            cwd.join(&self.home)
        };
        let scope = request
            .scope
            .as_deref()
            .map(|scope| match scope {
                "current" => Ok(SessionDiscoveryScope::Current),
                "repository" => Ok(SessionDiscoveryScope::Repository),
                "all" => Ok(SessionDiscoveryScope::All),
                _ => Err("invalid session catalog scope".to_string()),
            })
            .transpose()?;
        let output = if let Some(source) = request.transcript_path.as_deref() {
            let id = request
                .session_id
                .as_deref()
                .ok_or_else(|| "selected session id is required".to_string())?;
            let scope = scope.ok_or_else(|| "selected session scope is required".to_string())?;
            super::validate_selected_transcript_identity(source, id)?;
            // Exact-source requests still capture/revalidate the selected source;
            // they never resolve a cached UUID winner or a substitute transcript.
            super::list_selected_session_json_from_in_scope(&cwd, &home, id, source, scope).await?
        } else {
            let fs = self.filesystem(cwd.clone(), home.clone());
            // Discovery and revision/alias checks still run on every request.
            // Only unchanged derived row metadata survives between requests.
            super::list_sessions_json_using_fs(
                &cwd,
                &home,
                request.session_id.as_deref(),
                scope,
                fs,
                Some(&mut self.empty_markers),
            )
            .await?
        };
        if output.len() > MAX_OUTPUT_BYTES {
            return Err("session catalog output is too large".into());
        }
        Ok(output)
    }
}

async fn read_frame<R: AsyncBufRead + Unpin>(reader: &mut R) -> Result<Option<Vec<u8>>, String> {
    let mut frame = Vec::new();
    loop {
        let available = reader
            .fill_buf()
            .await
            .map_err(|_| "session catalog worker input failed".to_string())?;
        if available.is_empty() {
            return if frame.is_empty() {
                Ok(None)
            } else {
                Err("incomplete session catalog worker request".into())
            };
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let take = newline.unwrap_or(available.len());
        if frame.len().saturating_add(take) > MAX_REQUEST_BYTES {
            return Err("session catalog worker request is too large".into());
        }
        frame.extend_from_slice(&available[..take]);
        reader.consume(take + usize::from(newline.is_some()));
        if newline.is_some() {
            return Ok(Some(frame));
        }
    }
}

pub(super) async fn run<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    input: R,
    output: W,
) -> Result<(), String> {
    let home =
        super::brand_home().ok_or_else(|| "session catalog requires a config home".to_string())?;
    serve(input, output, home).await
}

async fn serve<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    input: R,
    mut output: W,
    home: PathBuf,
) -> Result<(), String> {
    let mut input = BufReader::new(input);
    let mut worker = CatalogWorker::new(home);
    loop {
        let frame = match tokio::time::timeout(IDLE_TIMEOUT, read_frame(&mut input)).await {
            Ok(frame) => frame?,
            Err(_) => return Ok(()),
        };
        let Some(frame) = frame else { return Ok(()) };
        let request: Request = serde_json::from_slice(&frame)
            .map_err(|_| "invalid session catalog worker request".to_string())?;
        let response = match worker.list(&request).await {
            Ok(result) => Response {
                version: 1,
                id: request.id,
                output: Some(result),
                error: None,
            },
            Err(error) => Response {
                version: 1,
                id: request.id,
                output: None,
                error: Some(error),
            },
        };
        // Carry the original oneshot JSON as a string. Do not parse or lower
        // its source/UTF-16 representation through a second Rust JSON model.
        let response = serde_json::to_vec(&response)
            .map_err(|_| "failed to encode session catalog worker response".to_string())?;
        output
            .write_all(&response)
            .await
            .map_err(|_| "session catalog worker output failed".to_string())?;
        output
            .write_all(b"\n")
            .await
            .map_err(|_| "session catalog worker output failed".to_string())?;
        output
            .flush()
            .await
            .map_err(|_| "session catalog worker output failed".to_string())?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filesystem_adapters_reuse_identity_and_have_a_bound() {
        let mut worker = CatalogWorker::new(PathBuf::from("/config"));
        let first = worker.filesystem(PathBuf::from("/first"), PathBuf::from("/config"));
        let again = worker.filesystem(PathBuf::from("/first"), PathBuf::from("/config"));
        assert!(Arc::ptr_eq(&first, &again));
        assert_eq!(first.cache_identity(), again.cache_identity());
        let other_home = worker.filesystem(PathBuf::from("/first"), PathBuf::from("/other"));
        assert!(!Arc::ptr_eq(&first, &other_home));
        for index in 0..MAX_FILESYSTEMS {
            worker.filesystem(
                PathBuf::from(format!("/workspace-{index}")),
                PathBuf::from("/config"),
            );
        }
        assert_eq!(worker.adapters.len(), MAX_FILESYSTEMS);
        let evicted = worker.filesystem(PathBuf::from("/first"), PathBuf::from("/config"));
        assert!(!Arc::ptr_eq(&first, &evicted));
    }

    #[tokio::test]
    async fn frames_are_bounded_before_allocating_an_unterminated_request() {
        let oversized = vec![b'x'; MAX_REQUEST_BYTES + 1];
        let mut input = BufReader::new(oversized.as_slice());
        assert!(read_frame(&mut input)
            .await
            .unwrap_err()
            .contains("too large"));
        let mut truncated = BufReader::new(b"{}".as_slice());
        assert!(read_frame(&mut truncated)
            .await
            .unwrap_err()
            .contains("incomplete"));
    }

    #[tokio::test]
    async fn protocol_keeps_errors_scoped_and_reuses_the_worker() {
        let root = tempfile::tempdir().unwrap();
        let cwd = root.path().canonicalize().unwrap();
        let requests = [
            serde_json::json!({"version":1,"id":1,"cwd":cwd,"scope":"invalid"}),
            serde_json::json!({"version":1,"id":2,"cwd":cwd,"scope":"current"}),
        ];
        let input = format!("{}\n{}\n", requests[0], requests[1]);
        let mut output = Vec::new();
        serve(input.as_bytes(), &mut output, cwd.join("empty-home"))
            .await
            .unwrap();
        let lines: Vec<serde_json::Value> = output
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice(line).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["id"], 1);
        assert!(lines[0]["error"].as_str().unwrap().contains("scope"));
        assert_eq!(lines[1]["id"], 2);
        let catalog: serde_json::Value =
            serde_json::from_str(lines[1]["output"].as_str().unwrap()).unwrap();
        assert_eq!(catalog, serde_json::json!({"version":1,"sessions":[]}));
    }

    #[tokio::test]
    async fn empty_anchor_projection_reuses_and_replaces_its_file_revision() {
        use std::io::Write;
        let root = tempfile::tempdir().unwrap();
        let cwd = root.path().canonicalize().unwrap();
        let home = cwd.join("home");
        let id = "11111111-2222-4333-8444-555555555555";
        let path = session::jsonl::session_path(&home, &cwd.to_string_lossy(), id);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let fixed = serde_json::json!({"type":"fixed-session-mode","sessionId":id,"sessionMode":"code","instructionEpoch":0});
        let anchor = serde_json::json!({"type":"custom-title","sessionId":id,"customTitle":"empty anchor","mobileEmptySession":1,"cwd":cwd});
        std::fs::write(&path, format!("{fixed}\n{anchor}\n")).unwrap();
        let request: Request = serde_json::from_value(
            serde_json::json!({"version":1,"id":1,"cwd":cwd,"scope":"current"}),
        )
        .unwrap();
        let mut worker = CatalogWorker::new(home.clone());
        for _ in 0..2 {
            let result: serde_json::Value =
                serde_json::from_str(&worker.list(&request).await.unwrap()).unwrap();
            assert_eq!(result["sessions"][0]["empty_session"], true);
            assert_eq!(worker.empty_markers.entries.len(), 1);
        }
        let initial_len = worker.empty_markers.entries[0].revision.len;
        let rename = serde_json::json!({"type":"custom-title","sessionId":id,"customTitle":"revised empty anchor","mobileEmptySession":1,"cwd":cwd});
        writeln!(
            std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap(),
            "{rename}"
        )
        .unwrap();
        let result: serde_json::Value =
            serde_json::from_str(&worker.list(&request).await.unwrap()).unwrap();
        assert_eq!(result["sessions"][0]["title"], "revised empty anchor");
        assert_eq!(result["sessions"][0]["empty_session"], true);
        assert_eq!(worker.empty_markers.entries.len(), 1);
        assert!(worker.empty_markers.entries[0].revision.len > initial_len);
        assert!(worker.empty_markers.bytes <= EMPTY_MARKER_CACHE_BYTES);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let fs = worker.filesystem(cwd.clone(), home.clone());
            let row = session::jsonl::find_session_with_diagnostics(
                &home,
                &cwd.to_string_lossy(),
                uuid::Uuid::parse_str(id).unwrap(),
                fs.clone(),
            )
            .await
            .unwrap()
            .sessions
            .remove(0);
            let permissions = std::fs::metadata(&path).unwrap().permissions();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0)).unwrap();
            // A privileged test runner may still read mode-000 files. Only
            // assert denial where the OS actually enforces that revocation.
            if fs
                .read_file_prefix(path.to_str().unwrap(), 1)
                .await
                .is_err()
            {
                assert!(!worker.empty_markers.is_empty(&row, fs).await);
                assert!(worker.empty_markers.entries.is_empty());
            }
            std::fs::set_permissions(&path, permissions).unwrap();
        }
    }
}

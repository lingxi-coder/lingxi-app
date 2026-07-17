//! Append-only JSONL writer — 1:1 port of
//! `claude-code/src/utils/sessionStorage.ts:2572-2584` (`appendEntryToFile`).
//!
//! Lock: serialize via `serde_json::to_string` (no whitespace, no indent),
//! terminate every line with a single `\n`, file mode `0o600`, dir mode `0o700`.

use crate::jsonl::schema::JsonlMessage;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::Mutex;
use traits::{FileSystem, FsError};

/// Failure modes for [`JsonlWriter`] operations.
#[derive(Debug, Error)]
pub enum WriterError {
    /// Underlying filesystem error.
    #[error(transparent)]
    Fs(#[from] FsError),
    /// `serde_json::to_string` failed (e.g. malformed `Value`).
    #[error("serialize failure: {0}")]
    Serialize(#[from] serde_json::Error),
}

/// Append-only writer for one session's `<uuid>.jsonl`.
///
/// Holds an exclusive in-process lock so concurrent `append` calls serialize
/// (cross-process locking is delegated to the `FileSystem` flock impl when
/// the orchestrator wants it; the spec only mandates in-process for M5-07).
pub struct JsonlWriter {
    path: PathBuf,
    fs: Arc<dyn FileSystem>,
    lock: Mutex<()>,
}

impl JsonlWriter {
    /// Open (or create on first append) `path`.
    ///
    /// No I/O is performed until `append` is called — keeps construction cheap
    /// for the orchestrator's `Option<Arc<JsonlWriter>>` wiring.
    #[must_use]
    pub fn new(path: PathBuf, fs: Arc<dyn FileSystem>) -> Self {
        Self {
            path,
            fs,
            lock: Mutex::new(()),
        }
    }

    /// Returns the on-disk path this writer targets.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append one JSONL line — `serde_json::to_string(msg) + "\n"`.
    ///
    /// Creates the parent directory on first call. The `FileSystem` trait
    /// in M1 does not expose `mkdir_p`; we use `tokio::fs::create_dir_all`
    /// directly because parent-dir creation is not a sandboxed operation we
    /// virtualize for tests (each `FileSystem` impl that hosts real files
    /// would do the same syscall internally). M5-08 may extend the trait.
    pub async fn append(&self, msg: &JsonlMessage) -> Result<(), WriterError> {
        let _g = self.lock.lock().await;
        let line = serde_json::to_string(msg)?;
        let path_str = self.path.to_str().expect("session paths are UTF-8");
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                // Sync std::fs is fine here — we already hold the in-process
                // mutex and parent-dir creation is a one-shot syscall.
                // claude-code `appendToFile` creates the project dir with
                // `{ mode: 0o700 }` (owner-only); mirror that on unix.
                #[cfg(unix)]
                {
                    use std::os::unix::fs::DirBuilderExt;
                    std::fs::DirBuilder::new()
                        .recursive(true)
                        .mode(0o700)
                        .create(parent)
                        .map_err(|e| FsError::Io(e.to_string()))?;
                }
                #[cfg(not(unix))]
                std::fs::create_dir_all(parent).map_err(|e| FsError::Io(e.to_string()))?;
            }
        }
        let mut payload = String::with_capacity(line.len() + 1);
        payload.push_str(&line);
        payload.push('\n');
        // claude-code `appendToFile`: `fsAppendFile(path, data, { mode: 0o600 })`
        // — the `<uuid>.jsonl` transcript is owner-only (prompt + tool content).
        self.fs
            .append_file_with_mode(path_str, &payload, 0o600)
            .await?;
        Ok(())
    }

    /// Append a user-set `custom-title` metadata line for `session_id` — the
    /// `/rename` write path, 1:1 with claude-code `saveCustomTitle`'s
    /// `appendEntryToFile(path, { type: 'custom-title', customTitle, sessionId })`.
    ///
    /// `session_id` MUST be the BARE session uuid (the `<uuid>.jsonl` file stem),
    /// NOT the `sess:`-prefixed `SessionId` display form — the loader keys the
    /// `custom_titles` map by file stem (`loader.rs`), so a prefixed id would
    /// never match on read. Same lock / dir-mode / file-mode contract as
    /// [`Self::append`].
    /// Append a `/rewind` `file-history-snapshot` side-map line (the checkpoint
    /// index for one turn) — same lock / dir-mode / file-mode contract as
    /// [`Self::append`].
    pub async fn append_file_history_snapshot(
        &self,
        value: &serde_json::Value,
    ) -> Result<(), WriterError> {
        let line = serde_json::to_string(value)?;
        let _g = self.lock.lock().await;
        let path_str = self.path.to_str().expect("session paths are UTF-8");
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::DirBuilderExt;
                    std::fs::DirBuilder::new()
                        .recursive(true)
                        .mode(0o700)
                        .create(parent)
                        .map_err(|e| FsError::Io(e.to_string()))?;
                }
                #[cfg(not(unix))]
                std::fs::create_dir_all(parent).map_err(|e| FsError::Io(e.to_string()))?;
            }
        }
        let mut payload = String::with_capacity(line.len() + 1);
        payload.push_str(&line);
        payload.push('\n');
        self.fs
            .append_file_with_mode(path_str, &payload, 0o600)
            .await?;
        Ok(())
    }

    pub async fn append_custom_title(
        &self,
        session_id: &str,
        custom_title: &str,
    ) -> Result<(), WriterError> {
        let value = serde_json::json!({
            "type": "custom-title",
            "customTitle": custom_title,
            "sessionId": session_id,
        });
        self.append_side_record(&value).await
    }

    /// Append an `agent-setting` metadata line for `session_id` — the persisted
    /// main-thread `--agent` selection (`agentSetting` = the agent's `agentType`)
    /// so a later `--resume` (with no `--agent`) can re-adopt it. 1:1 with
    /// claude-code's session persist `appendEntryToFile(path, {type:
    /// 'agent-setting', agentSetting: currentSessionAgentSetting, sessionId})`
    /// (`sessionStorage.ts`; read back by the `agentSettings.set(N.sessionId,
    /// N.agentSetting)` routing and fed to `rVe` on resume).
    ///
    /// `session_id` MUST be the BARE session uuid (the `<uuid>.jsonl` file stem),
    /// NOT the `sess:`-prefixed display form — the loader keys the
    /// `agent_settings` map by that stem, so a prefixed id would never match on
    /// read. Same lock / dir-mode / file-mode contract as [`Self::append`].
    pub async fn append_agent_setting(
        &self,
        session_id: &str,
        agent_setting: &str,
    ) -> Result<(), WriterError> {
        let value = serde_json::json!({
            "type": "agent-setting",
            "agentSetting": agent_setting,
            "sessionId": session_id,
        });
        self.append_side_record(&value).await
    }

    /// Shared body for the metadata side-record appenders ([`Self::append_custom_title`],
    /// [`Self::append_agent_setting`]): serialize one JSON object + `\n` and append
    /// it under the same lock / dir-mode (0o700) / file-mode (0o600) contract as
    /// [`Self::append`].
    async fn append_side_record(&self, value: &serde_json::Value) -> Result<(), WriterError> {
        let line = serde_json::to_string(value)?;
        let _g = self.lock.lock().await;
        let path_str = self.path.to_str().expect("session paths are UTF-8");
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::DirBuilderExt;
                    std::fs::DirBuilder::new()
                        .recursive(true)
                        .mode(0o700)
                        .create(parent)
                        .map_err(|e| FsError::Io(e.to_string()))?;
                }
                #[cfg(not(unix))]
                std::fs::create_dir_all(parent).map_err(|e| FsError::Io(e.to_string()))?;
            }
        }
        let mut payload = String::with_capacity(line.len() + 1);
        payload.push_str(&line);
        payload.push('\n');
        self.fs
            .append_file_with_mode(path_str, &payload, 0o600)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `/rename` write path emits a `custom-title` line whose `sessionId`
    /// is the BARE uuid passed in (the `<uuid>.jsonl` stem the loader keys
    /// `custom_titles` by) and whose `customTitle` round-trips verbatim.
    #[tokio::test]
    async fn append_custom_title_writes_parseable_line() {
        let tmp = std::env::temp_dir().join(format!(
            "lingxi-writer-title-{}-{}",
            std::process::id(),
            "abc"
        ));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let session_id = "11111111-2222-3333-4444-555555555555";
        let session_path = tmp.join(format!("{session_id}.jsonl"));
        let fs: Arc<dyn FileSystem> =
            Arc::new(platform_posix::fs::PosixFileSystem::new(tmp.clone()));
        let writer = JsonlWriter::new(session_path.clone(), fs);

        writer
            .append_custom_title(session_id, "My Title")
            .await
            .expect("append custom title");

        let raw = std::fs::read_to_string(&session_path).expect("read back");
        let value: serde_json::Value =
            serde_json::from_str(raw.trim()).expect("line parses as json");
        assert_eq!(value["type"], "custom-title");
        assert_eq!(value["customTitle"], "My Title");
        assert_eq!(value["sessionId"], session_id);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// (P2-02 cc2.1.207) The `--agent` persist path emits an `agent-setting` line
    /// whose `sessionId` is the BARE uuid (the `<uuid>.jsonl` stem the loader keys
    /// `agent_settings` by) and whose `agentSetting` is the applied `agentType`
    /// verbatim — the record `route_lines` reads back into `agent_settings` and
    /// `rVe` re-adopts on resume. Byte-shape matches claude's persist
    /// `{type:"agent-setting",agentSetting,sessionId}`.
    #[tokio::test]
    async fn append_agent_setting_writes_parseable_line() {
        let tmp = std::env::temp_dir().join(format!(
            "lingxi-writer-agent-{}-{}",
            std::process::id(),
            "xyz"
        ));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let session_id = "22222222-3333-4444-5555-666666666666";
        let session_path = tmp.join(format!("{session_id}.jsonl"));
        let fs: Arc<dyn FileSystem> =
            Arc::new(platform_posix::fs::PosixFileSystem::new(tmp.clone()));
        let writer = JsonlWriter::new(session_path.clone(), fs);

        writer
            .append_agent_setting(session_id, "reviewer")
            .await
            .expect("append agent setting");

        let raw = std::fs::read_to_string(&session_path).expect("read back");
        let value: serde_json::Value =
            serde_json::from_str(raw.trim()).expect("line parses as json");
        assert_eq!(value["type"], "agent-setting");
        assert_eq!(value["agentSetting"], "reviewer");
        assert_eq!(value["sessionId"], session_id);

        // The loader routes it back into the `agent_settings` side-map keyed by
        // `sessionId` (the resume read side `rVe` consumes).
        let loaded = crate::jsonl::reader::route_lines(&raw);
        assert_eq!(
            loaded
                .agent_settings
                .get(session_id)
                .and_then(serde_json::Value::as_str),
            Some("reviewer"),
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }
}

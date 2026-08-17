//! Append-only JSONL writer — 1:1 port of
//! `claude-code/src/utils/sessionStorage.ts:2572-2584` (`appendEntryToFile`).
//!
//! Lock: serialize via `serde_json::to_string` (no whitespace, no indent),
//! terminate every line with a single `\n`, file mode `0o600`, dir mode `0o700`.

use crate::jsonl::re_append::{
    plan_re_append, read_tail, SessionMetadataState, METADATA_REAPPEND_BACKSTOP_BYTES,
};
use crate::jsonl::schema::JsonlMessage;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
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
    active_path: std::sync::RwLock<PathBuf>,
    fs: Arc<dyn FileSystem>,
    lock: Mutex<()>,
    /// Bytes appended to the active transcript since the last metadata
    /// re-append — the oracle's `bytesSinceMetadataReAppend` (increment site
    /// 2.1.220 @237850612: `bytesSinceMetadataReAppend += Buffer.byteLength(t,"utf8")`,
    /// gated on the write target being the CURRENT session file, which is
    /// always true for this writer).
    ///
    /// Once it reaches [`METADATA_REAPPEND_BACKSTOP_BYTES`] the metadata set
    /// must be re-appended so it stays inside the 64 KiB tail window every
    /// session-index reader scans. See [`Self::metadata_re_append_due`].
    bytes_since_metadata_re_append: AtomicUsize,
    /// The metadata this writer will re-append when the backstop fires.
    ///
    /// The writer OWNS this rather than taking it per call: it is the single
    /// funnel every metadata record already goes through
    /// ([`Self::append_custom_title`] and friends), and it already owns the
    /// file, the lock and the byte counter. Any other owner would have to be
    /// threaded from the composition root through the resume path to reach the
    /// same place.
    metadata_state: Mutex<SessionMetadataState>,
}

impl JsonlWriter {
    /// Open (or create on first append) `path`.
    ///
    /// No I/O is performed until `append` is called — keeps construction cheap
    /// for the orchestrator's `Option<Arc<JsonlWriter>>` wiring.
    #[must_use]
    pub fn new(path: PathBuf, fs: Arc<dyn FileSystem>) -> Self {
        Self {
            active_path: std::sync::RwLock::new(path.clone()),
            path,
            fs,
            lock: Mutex::new(()),
            bytes_since_metadata_re_append: AtomicUsize::new(0),
            metadata_state: Mutex::new(SessionMetadataState::default()),
        }
    }

    /// Returns the on-disk path this writer targets.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the path currently receiving appends.
    ///
    /// Most runtimes keep the initial path for the writer's entire lifetime.
    /// Mobile keeps one orchestrator alive across `NewSession` and
    /// `ResumeSession`, so it retargets the writer after the session transition.
    #[must_use]
    pub fn active_path(&self) -> PathBuf {
        self.active_path
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Atomically switch subsequent appends to another session transcript.
    ///
    /// The append mutex makes the boundary explicit: an append already in
    /// progress finishes on the previous file before this method returns.
    pub async fn retarget(&self, path: PathBuf) {
        let _g = self.lock.lock().await;
        *self
            .active_path
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = path;
    }

    /// Append one JSONL line — `serde_json::to_string(msg) + "\n"`.
    ///
    /// Creates the parent directory on first call. The `FileSystem` trait
    /// in M1 does not expose `mkdir_p`; we use `tokio::fs::create_dir_all`
    /// directly because parent-dir creation is not a sandboxed operation we
    /// virtualize for tests (each `FileSystem` impl that hosts real files
    /// would do the same syscall internally). M5-08 may extend the trait.
    pub async fn append(&self, msg: &JsonlMessage) -> Result<(), WriterError> {
        {
            let _g = self.lock.lock().await;
            let line = serde_json::to_string(msg)?;
            let mut payload = String::with_capacity(line.len() + 1);
            payload.push_str(&line);
            payload.push('\n');
            self.append_payload(&payload).await?;
        }
        // Drive the backstop from the ordinary append path. Deliberately AFTER
        // the critical section above: `maybe_re_append_metadata` re-takes
        // `self.lock`, so polling inside would deadlock.
        self.maybe_re_append_metadata().await;
        Ok(())
    }

    /// Write `payload` verbatim to the active transcript and account it against
    /// the metadata-re-append backstop counter.
    ///
    /// The caller MUST already hold [`Self::lock`] — this is the shared body of
    /// every append path and takes no lock of its own so that
    /// [`Self::re_append_session_metadata`] can read the tail and write the
    /// plan under one critical section.
    async fn append_payload(&self, payload: &str) -> Result<(), WriterError> {
        let path = self.active_path();
        let path_str = path.to_str().expect("session paths are UTF-8");
        if let Some(parent) = path.parent() {
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
        // claude-code `appendToFile`: `fsAppendFile(path, data, { mode: 0o600 })`
        // — the `<uuid>.jsonl` transcript is owner-only (prompt + tool content).
        self.fs
            .append_file_with_mode(path_str, payload, 0o600)
            .await?;
        // `bytesSinceMetadataReAppend += Buffer.byteLength(t,"utf8")` — counted
        // only on a SUCCESSFUL write, matching the oracle's post-await position.
        self.bytes_since_metadata_re_append
            .fetch_add(payload.len(), Ordering::Relaxed);
        Ok(())
    }

    /// Bytes appended since the last metadata re-append
    /// (`bytesSinceMetadataReAppend`).
    #[must_use]
    pub fn bytes_since_metadata_re_append(&self) -> usize {
        self.bytes_since_metadata_re_append.load(Ordering::Relaxed)
    }

    /// `true` once [`METADATA_REAPPEND_BACKSTOP_BYTES`] have been appended since
    /// the last re-append — the oracle's periodic backstop condition at the tail
    /// of `drainQueuesOnce` (`if(this.bytesSinceMetadataReAppend>=kI/2)`,
    /// 2.1.220 @237851998), which fires
    /// `reAppendSessionMetadataAsync(false, /*skip_dedup=*/true)`.
    #[must_use]
    pub fn metadata_re_append_due(&self) -> bool {
        self.bytes_since_metadata_re_append() >= METADATA_REAPPEND_BACKSTOP_BYTES
    }

    /// Clear the backstop counter without writing — the oracle's
    /// `resetSessionFile()` also zeroes `bytesSinceMetadataReAppend`.
    pub fn reset_metadata_re_append_counter(&self) {
        self.bytes_since_metadata_re_append
            .store(0, Ordering::Relaxed);
    }

    /// The session id this writer is writing, i.e. the transcript's file stem.
    ///
    /// The oracle passes the id in; here the writer already knows which session
    /// it targets, so nothing has to thread it. `<uuid>.jsonl` -> `<uuid>`,
    /// which is exactly the BARE form the loader keys its maps by (see
    /// [`Self::append_custom_title`]'s contract).
    fn session_id_from_path(&self) -> Option<String> {
        self.active_path()
            .file_stem()
            .and_then(|s| s.to_str())
            .map(ToString::to_string)
    }

    /// Fire the metadata backstop if enough bytes have accumulated.
    ///
    /// THIS is what makes the port live. Without it `re_append_session_metadata`
    /// is a mechanism nobody invokes, and metadata still scrolls out of the
    /// 64 KiB window that every session-index reader scans.
    ///
    /// Uses the backstop polarity `(skip_title_adopt: false, skip_dedup: true)`
    /// — the oracle's periodic/post-compaction call. Best-effort: a write error
    /// here must not fail the append that triggered it, so it is swallowed (the
    /// counter has already been reset, so the next window retries).
    ///
    /// Takes NO lock of its own; `re_append_session_metadata` takes it.
    pub async fn maybe_re_append_metadata(&self) -> usize {
        if !self.metadata_re_append_due() {
            return 0;
        }
        let Some(sid) = self.session_id_from_path() else {
            return 0;
        };
        let mut state = self.metadata_state.lock().await;
        match self
            .re_append_session_metadata(&mut state, &sid, false, true)
            .await
        {
            Ok(n) => n,
            Err(e) => {
                // `catch(e){…w(`Metadata re-append failed (${$t(e)}): ${le(e)}`,
                // {level:"error"})…}` — the oracle LOGS this rather than letting
                // it escape, because the append that triggered the backstop has
                // already succeeded and must not fail retroactively.
                tracing::error!("Metadata re-append failed: {e}");
                0
            }
        }
    }

    /// Re-append the session's metadata sidecar records — 1:1 with
    /// `Isp.reAppendSessionMetadata` (2.1.220 @237852347) and its async twin
    /// `reAppendSessionMetadataAsync` (@237852577).
    ///
    /// Reads the last 64 KiB of the active transcript, runs
    /// [`plan_re_append`] over it (adopt-back → rebuild → dedup, mutating
    /// `state` in place), and appends the surviving records. Follows the ASYNC
    /// variant's write shape: all entries in ONE append (`jsonlJoin`), which is
    /// byte-identical to the sync variant's per-entry appends.
    ///
    /// Both flags are SKIP flags — see [`plan_re_append`]. The three production
    /// polarities at the oracle are:
    /// - resume adopt (`adoptResumedSessionFile`): `(true, false)`
    /// - periodic backstop / post-compaction: `(false, true)`
    /// - process exit (`reAppendSessionMetadataAtExit`): `(false, false)`
    ///
    /// The counter is zeroed FIRST, exactly as the oracle does, so a failure
    /// mid-way does not immediately re-arm the backstop.
    ///
    /// Returns the number of records written (0 when everything deduped away).
    pub async fn re_append_session_metadata(
        &self,
        state: &mut SessionMetadataState,
        session_id: &str,
        skip_title_adopt: bool,
        skip_dedup: bool,
    ) -> Result<usize, WriterError> {
        let _g = self.lock.lock().await;
        self.bytes_since_metadata_re_append
            .store(0, Ordering::Relaxed);
        let path = self.active_path();
        let tail = read_tail(&path);
        let Some(plan) = plan_re_append(&tail, state, session_id, skip_title_adopt, skip_dedup)
        else {
            return Ok(0);
        };
        if plan.is_empty() {
            return Ok(0);
        }
        let count = plan.entries.len();
        self.append_payload(&plan.to_jsonl()).await?;
        // The re-appended bytes are the metadata itself; they must not count
        // toward the next backstop.
        self.bytes_since_metadata_re_append
            .store(0, Ordering::Relaxed);
        Ok(count)
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
        let mut payload = String::with_capacity(line.len() + 1);
        payload.push_str(&line);
        payload.push('\n');
        self.append_payload(&payload).await
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
        // Mirror the oracle's `currentSessionTitle`. NOTE this is belt-and-braces,
        // not the load-bearing path: `plan_re_append` ADOPTS the title back out
        // of the tail, and the backstop (32 KiB) is deliberately half the tail
        // window (64 KiB) so it fires while the record is still readable.
        // Removing this line does NOT fail `appends_alone_drive_the_metadata_backstop`
        // — verified by mutation. It matters only when state is set without a
        // corresponding record already on disk.
        self.metadata_state.lock().await.title = Some(custom_title.to_string());
        self.append_side_record(&value).await
    }

    /// Persist a mobile-created zero-message session before its first turn.
    ///
    /// The record deliberately remains a `custom-title` side record so the
    /// existing session catalog can list it with `message_count == 0`, while the
    /// versioned marker lets the mobile host distinguish a genuine empty session
    /// from an arbitrary metadata-only/corrupt transcript.
    pub async fn append_mobile_empty_session(
        &self,
        session_id: &str,
        title: &str,
    ) -> Result<(), WriterError> {
        let value = serde_json::json!({
            "type": "custom-title",
            "customTitle": title,
            "sessionId": session_id,
            "mobileEmptySession": 1,
        });
        self.append_side_record(&value).await
    }

    /// Persist the active permission mode for this transcript's session.
    ///
    /// The mode is session metadata, not a user-wide default: reopening this
    /// transcript restores its last selected mode without changing other
    /// sessions. The active writer path is the source of the bare session UUID.
    pub async fn append_permission_mode(&self, permission_mode: &str) -> Result<(), WriterError> {
        let Some(session_id) = self.session_id_from_path() else {
            return Ok(());
        };
        self.metadata_state.lock().await.permission_mode = Some(permission_mode.to_string());
        let value = serde_json::json!({
            "type": "permission-mode",
            "permissionMode": permission_mode,
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

    /// Persist the Claude-compatible agent name together with a versioned,
    /// immutable resolved definition. The sibling `agentSnapshot` field is an
    /// additive LingXi extension; old readers continue consuming
    /// `agentSetting`, while new readers can resume even if the catalog entry is
    /// later edited or removed.
    pub async fn append_agent_setting_snapshot(
        &self,
        session_id: &str,
        agent_setting: &str,
        definition: &serde_json::Value,
    ) -> Result<(), WriterError> {
        use sha2::{Digest, Sha256};

        let canonical = serde_json::to_vec(definition).map_err(WriterError::Serialize)?;
        let hash = format!("{:x}", Sha256::digest(&canonical));
        let value = serde_json::json!({
            "type": "agent-setting",
            "agentSetting": agent_setting,
            "agentSnapshot": {
                "schemaVersion": 1,
                "sha256": hash,
                "definition": definition,
            },
            "sessionId": session_id,
        });
        self.append_side_record(&value).await
    }

    /// Append a `worktree-state` metadata line for `session_id` — the persisted
    /// active-worktree record so a later `--continue`/`--resume` can rehydrate
    /// the session's `EnterWorktree` state (making `ExitWorktree` operate instead
    /// of no-oping). 1:1 with claude-code's `saveWorktreeState`
    /// (`gne` → `appendEntryToFile(path, {type:'worktree-state', worktreeSession,
    /// sessionId})`; read back by the `worktreeStates.set(N.sessionId,
    /// N.worktreeSession)` routing).
    ///
    /// `worktree_session` is the serialized session payload for an active
    /// worktree, or [`None`] for the `ExitWorktree` clear record (persisted as
    /// JSON `null`, matching claude's `gne(null)`). `session_id` MUST be the BARE
    /// session uuid (the `<uuid>.jsonl` file stem the loader keys the
    /// `worktree_states` map by). Same lock / dir-mode / file-mode contract as
    /// [`Self::append`].
    pub async fn append_worktree_state(
        &self,
        session_id: &str,
        worktree_session: Option<&serde_json::Value>,
    ) -> Result<(), WriterError> {
        let value = serde_json::json!({
            "type": "worktree-state",
            "worktreeSession": worktree_session.cloned().unwrap_or(serde_json::Value::Null),
            "sessionId": session_id,
        });
        self.append_side_record(&value).await
    }

    /// Shared body for the metadata side-record appenders ([`Self::append_custom_title`],
    /// [`Self::append_agent_setting`]): serialize one JSON object + `\n` and append
    /// it under the same lock / dir-mode (0o700) / file-mode (0o600) contract as
    /// [`Self::append`].
    /// Test-only raw append, so re-append tests can plant arbitrary filler /
    /// hand-written lines without going through a typed appender.
    #[cfg(test)]
    async fn append_payload_for_test(&self, payload: &str) {
        let _g = self.lock.lock().await;
        self.append_payload(payload).await.expect("raw append");
    }

    /// Test-only wrapper over the REAL [`Self::append_side_record`] path, so a
    /// test can generate bulk transcript without bypassing the backstop poll
    /// the way [`Self::append_payload_for_test`] does.
    #[cfg(test)]
    async fn append_side_record_for_test(&self, value: &serde_json::Value) {
        self.append_side_record(value).await.expect("side record");
    }

    async fn append_side_record(&self, value: &serde_json::Value) -> Result<(), WriterError> {
        {
            let line = serde_json::to_string(value)?;
            let _g = self.lock.lock().await;
            let mut payload = String::with_capacity(line.len() + 1);
            payload.push_str(&line);
            payload.push('\n');
            self.append_payload(&payload).await?;
        }
        // Every write path can trip the backstop: the oracle polls once at the
        // end of its write-queue DRAIN (@237852006), which all writes funnel
        // through. LingXi writes immediately, so the analog is a poll at the end
        // of each public append path — outside the critical section, since
        // `maybe_re_append_metadata` re-takes the lock.
        self.maybe_re_append_metadata().await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_writer(tag: &str) -> (PathBuf, PathBuf, JsonlWriter) {
        let dir = std::env::temp_dir().join(format!(
            "lingxi-writer-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("11111111-2222-3333-4444-555555555555.jsonl");
        let fs: Arc<dyn FileSystem> =
            Arc::new(platform_posix::fs::PosixFileSystem::new(dir.clone()));
        (dir, path.clone(), JsonlWriter::new(path, fs))
    }

    /// The backstop counter accounts every appended byte (payload INCLUDING the
    /// trailing newline) and arms at `kI/2` = 32768, matching the oracle's
    /// `bytesSinceMetadataReAppend >= kI/2` gate.
    #[tokio::test]
    async fn appends_accumulate_the_backstop_counter() {
        let (dir, path, writer) = temp_writer("backstop");

        assert_eq!(writer.bytes_since_metadata_re_append(), 0);
        assert!(!writer.metadata_re_append_due());

        writer
            .append_custom_title("11111111-2222-3333-4444-555555555555", "t")
            .await
            .expect("append");
        let on_disk = std::fs::metadata(&path).expect("stat").len() as usize;
        assert_eq!(
            writer.bytes_since_metadata_re_append(),
            on_disk,
            "counter must equal the bytes actually written"
        );
        assert!(!writer.metadata_re_append_due());

        // Push it over the 32 KiB line with one big title.
        writer
            .append_custom_title(
                "11111111-2222-3333-4444-555555555555",
                &"p".repeat(METADATA_REAPPEND_BACKSTOP_BYTES),
            )
            .await
            .expect("append big");
        // CORRECTED. This used to assert `metadata_re_append_due()` is still
        // true here. That held only while nothing polled the backstop: the
        // public append paths now fire `maybe_re_append_metadata` on the way
        // out, so crossing the line SELF-CLEARS the counter. Asserting it stays
        // due would now be asserting that the wiring does not work.
        assert!(
            !writer.metadata_re_append_due(),
            "crossing the backstop must have triggered a re-append and reset the counter"
        );

        writer.reset_metadata_re_append_counter();
        assert_eq!(writer.bytes_since_metadata_re_append(), 0);

        let _ = std::fs::remove_dir_all(&dir);
    }
    /// THE WIRING TEST: a long session re-appends its metadata on its own.
    ///
    /// The mechanism was previously driven only by an explicit call nobody made,
    /// which made the whole port inert. The writer now owns the state and polls
    /// the backstop itself after every append, so metadata that scrolls out of
    /// the 64 KiB tail window comes back without anyone asking.
    ///
    /// `session_id` is derived from the transcript's own file stem — the writer
    /// already knows which session it is writing, so no caller has to thread it.
    #[tokio::test]
    async fn appends_alone_drive_the_metadata_backstop() {
        let (_dir, path, writer) = temp_writer("22222222-3333-4444-5555-666666666666");

        // A title recorded through the writer is remembered in its own state.
        writer
            .append_custom_title("22222222-3333-4444-5555-666666666666", "Long Session")
            .await
            .unwrap();

        // Bury it under more than a full TAIL WINDOW of transcript — not merely
        // the backstop. The backstop (32 KiB) is half the window (64 KiB), so
        // filling only past the backstop leaves the title still visible in the
        // tail and the assertion below would pass without anything being
        // re-appended at all.
        // Through a REAL append path — `append_payload_for_test` bypasses the
        // public entry points and so would never trip the poll, making this test
        // green for the wrong reason.
        let mut wrote = 0usize;
        while wrote < super::METADATA_REAPPEND_BACKSTOP_BYTES * 2 + 8192 {
            let rec = serde_json::json!({ "type": "filler", "pad": "f".repeat(4000) });
            writer.append_side_record_for_test(&rec).await;
            wrote += 4050;
        }

        let tail = crate::jsonl::read_tail(&path);
        assert!(
            tail.contains("custom-title") && tail.contains("Long Session"),
            "the backstop must have restored the title into the tail window \
             without an explicit re_append call"
        );
    }

    /// End-to-end: metadata that has scrolled out of the 64 KiB tail window is
    /// re-appended so a tail-scanning reader sees it again — the entire point
    /// of `reAppendSessionMetadata`.
    #[tokio::test]
    async fn re_append_restores_metadata_into_the_tail_window() {
        let (dir, path, writer) = temp_writer("restore");
        let sid = "11111111-2222-3333-4444-555555555555";

        writer.append_custom_title(sid, "Kept Title").await.unwrap();
        // Bury it under more than a full tail window of transcript.
        let filler = format!("{}\n", "f".repeat(4095));
        for _ in 0..20 {
            writer.append_payload_for_test(&filler).await;
        }
        let scrolled = crate::jsonl::read_tail(&path);
        assert!(
            !scrolled.contains("custom-title"),
            "precondition: the title must have scrolled out of the tail"
        );

        let mut state = SessionMetadataState {
            title: Some("Kept Title".into()),
            mode: Some("default".into()),
            ..Default::default()
        };
        let written = writer
            .re_append_session_metadata(&mut state, sid, true, false)
            .await
            .expect("re-append");
        assert_eq!(written, 2, "custom-title + mode");

        let tail = crate::jsonl::read_tail(&path);
        let routed = crate::jsonl::route_lines(&tail);
        assert_eq!(
            routed.custom_titles.get(sid).map(String::as_str),
            Some("Kept Title")
        );
        assert_eq!(routed.modes.get(sid).map(String::as_str), Some("default"));
        assert_eq!(
            writer.bytes_since_metadata_re_append(),
            0,
            "the counter is zeroed by the re-append"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Called twice back to back, the second call writes NOTHING: the dedup
    /// pass finds its own records in the tail. Without this the 32 KiB backstop
    /// would bloat every transcript without bound.
    #[tokio::test]
    async fn back_to_back_re_appends_write_nothing_the_second_time() {
        let (dir, path, writer) = temp_writer("dedup");
        let sid = "11111111-2222-3333-4444-555555555555";
        writer
            .append_payload_for_test("{\"type\":\"user\"}\n")
            .await;

        let mut state = SessionMetadataState {
            title: Some("T".into()),
            mode: Some("default".into()),
            pr_number: Some(7),
            pr_url: Some("https://example.test/pull/7".into()),
            pr_repository: Some("acme/widgets".into()),
            ..Default::default()
        };
        assert_eq!(
            writer
                .re_append_session_metadata(&mut state, sid, true, false)
                .await
                .unwrap(),
            3
        );
        let after_first = std::fs::read_to_string(&path).unwrap();

        assert_eq!(
            writer
                .re_append_session_metadata(&mut state, sid, true, false)
                .await
                .unwrap(),
            0,
            "identical metadata must not be written again (pr-link included, \
             despite its timestamp differing)"
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            after_first,
            "the file must be byte-identical after the no-op re-append"
        );

        // …but `skip_dedup = true` forces it.
        assert_eq!(
            writer
                .re_append_session_metadata(&mut state, sid, true, true)
                .await
                .unwrap(),
            3
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A writer with nothing to say writes nothing and creates no file.
    #[tokio::test]
    async fn re_append_with_empty_state_is_a_no_op() {
        let (dir, path, writer) = temp_writer("noop");
        let mut state = SessionMetadataState::default();
        assert_eq!(
            writer
                .re_append_session_metadata(
                    &mut state,
                    "11111111-2222-3333-4444-555555555555",
                    true,
                    false
                )
                .await
                .unwrap(),
            0
        );
        assert!(!path.exists(), "no file may be created for an empty plan");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn retarget_moves_subsequent_appends_to_the_new_session_file() {
        let tmp =
            std::env::temp_dir().join(format!("lingxi-writer-retarget-{}", std::process::id(),));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let first = tmp.join("11111111-2222-3333-4444-555555555555.jsonl");
        let second = tmp.join("66666666-7777-4888-8999-aaaaaaaaaaaa.jsonl");
        let fs: Arc<dyn FileSystem> =
            Arc::new(platform_posix::fs::PosixFileSystem::new(tmp.clone()));
        let writer = JsonlWriter::new(first.clone(), fs);

        writer
            .append_custom_title("11111111-2222-3333-4444-555555555555", "first")
            .await
            .expect("append first title");
        writer.retarget(second.clone()).await;
        writer
            .append_custom_title("66666666-7777-4888-8999-aaaaaaaaaaaa", "second")
            .await
            .expect("append second title");

        assert_eq!(writer.active_path(), second);
        assert!(std::fs::read_to_string(first)
            .unwrap()
            .contains("\"first\""));
        assert!(std::fs::read_to_string(second)
            .unwrap()
            .contains("\"second\""));
        let _ = std::fs::remove_dir_all(&tmp);
    }

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

    #[tokio::test]
    async fn append_mobile_empty_session_writes_versioned_catalog_anchor() {
        let tmp = std::env::temp_dir()
            .join(format!("lingxi-writer-mobile-empty-{}", std::process::id(),));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let session_id = "11111111-2222-3333-4444-555555555555";
        let session_path = tmp.join(format!("{session_id}.jsonl"));
        let fs: Arc<dyn FileSystem> =
            Arc::new(platform_posix::fs::PosixFileSystem::new(tmp.clone()));
        let writer = JsonlWriter::new(session_path.clone(), fs);

        writer
            .append_mobile_empty_session(session_id, "新对话")
            .await
            .expect("append mobile empty-session anchor");

        let raw = std::fs::read_to_string(&session_path).expect("read back");
        let value: serde_json::Value =
            serde_json::from_str(raw.trim()).expect("line parses as json");
        assert_eq!(value["type"], "custom-title");
        assert_eq!(value["customTitle"], "新对话");
        assert_eq!(value["sessionId"], session_id);
        assert_eq!(value["mobileEmptySession"], 1);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn append_permission_mode_round_trips_through_transcript_metadata() {
        let tmp = std::env::temp_dir()
            .join(format!("lingxi-writer-permission-mode-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let session_id = "11111111-2222-3333-4444-555555555555";
        let session_path = tmp.join(format!("{session_id}.jsonl"));
        let fs: Arc<dyn FileSystem> =
            Arc::new(platform_posix::fs::PosixFileSystem::new(tmp.clone()));
        let writer = JsonlWriter::new(session_path.clone(), fs);

        writer
            .append_permission_mode("bypassPermissions")
            .await
            .expect("append permission mode");

        let raw = std::fs::read_to_string(&session_path).expect("read back");
        let routed = crate::jsonl::route_lines(&raw);
        assert_eq!(
            routed.permission_modes.get(session_id).map(String::as_str),
            Some("bypassPermissions")
        );

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

    #[tokio::test]
    async fn agent_snapshot_round_trips_with_integrity_check() {
        let tmp = std::env::temp_dir().join(format!(
            "lingxi-writer-agent-snapshot-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let session_id = "22222222-3333-4444-5555-777777777777";
        let path = tmp.join(format!("{session_id}.jsonl"));
        let fs: Arc<dyn FileSystem> =
            Arc::new(platform_posix::fs::PosixFileSystem::new(tmp.clone()));
        let writer = JsonlWriter::new(path.clone(), fs.clone());
        let definition = serde_json::json!({
            "agent_type": "reviewer",
            "system_prompt": "frozen prompt",
            "tools": {"Explicit": ["Read"]}
        });
        writer
            .append_agent_setting_snapshot(session_id, "reviewer", &definition)
            .await
            .expect("append snapshot");

        let restored = crate::jsonl::loader::read_agent_snapshot(&path, fs, session_id)
            .await
            .expect("snapshot restores");
        assert_eq!(restored, definition);
        let raw = std::fs::read_to_string(&path).unwrap();
        let routed = crate::jsonl::reader::route_lines(&raw);
        assert!(routed.agent_snapshots.contains_key(session_id));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// The `EnterWorktree` persist path emits a `worktree-state` line whose
    /// `sessionId` is the BARE uuid and whose `worktreeSession` payload
    /// round-trips; a subsequent `None` (ExitWorktree) writes an explicit
    /// `null`. The loader routes both back into `worktree_states` keyed by
    /// `sessionId`, last-write-wins.
    #[tokio::test]
    async fn append_worktree_state_writes_parseable_lines() {
        let tmp =
            std::env::temp_dir().join(format!("lingxi-writer-wt-{}-{}", std::process::id(), "wt"));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let session_id = "33333333-4444-5555-6666-777777777777";
        let session_path = tmp.join(format!("{session_id}.jsonl"));
        let fs: Arc<dyn FileSystem> =
            Arc::new(platform_posix::fs::PosixFileSystem::new(tmp.clone()));
        let writer = JsonlWriter::new(session_path.clone(), fs);

        let payload = serde_json::json!({
            "worktreePath": "/repo/.lingxi/worktrees/feat",
            "originalCwd": "/repo",
            "worktreeBranch": "worktree-feat",
            "enteredExisting": false,
        });
        writer
            .append_worktree_state(session_id, Some(&payload))
            .await
            .expect("append active worktree state");

        let raw = std::fs::read_to_string(&session_path).expect("read back");
        let value: serde_json::Value =
            serde_json::from_str(raw.trim()).expect("line parses as json");
        assert_eq!(value["type"], "worktree-state");
        assert_eq!(value["sessionId"], session_id);
        assert_eq!(
            value["worktreeSession"]["worktreePath"],
            "/repo/.lingxi/worktrees/feat"
        );

        // Clear record (ExitWorktree) → worktreeSession: null.
        writer
            .append_worktree_state(session_id, None)
            .await
            .expect("append clear worktree state");

        let raw2 = std::fs::read_to_string(&session_path).expect("read back 2");
        let loaded = crate::jsonl::reader::route_lines(&raw2);
        // Last-write-wins: the clear record (null) supersedes the active one.
        assert!(
            loaded
                .worktree_states
                .get(session_id)
                .expect("worktree state present")
                .is_null(),
            "the trailing clear record wins"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }
}

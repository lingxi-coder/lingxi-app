# LingXi Core M1 · Plan 10 · Session Storage + File State Cache + Message Queue

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans.

**Goal:** Three crates that own engine-side state persistence and message routing: `lingxi-session` (append-only JSONL transcripts + fsync + flock + crash-safe reader + SessionResumer integrating all subsystems), `lingxi-filestate` (FileStateCache + Read↔Edit verification + clone/merge), `lingxi-msgqueue` (unified priority queue for user/notification/orphan inputs).

**Depends on:** Plans 01-09.

---

## File Structure

```
crates/session/
├── Cargo.toml
└── src/{lib, metadata, transcript, storage, jsonl, resumer}.rs

crates/filestate/
├── Cargo.toml
└── src/{lib, cache, verify, merge}.rs

crates/msgqueue/
├── Cargo.toml
└── src/{lib, queue, operations}.rs

crates/platform-api/src/filesystem.rs ← MODIFY: add `append_file`, `truncate`, `file_mtime`, `file_size`, `delete_file`, `symlink`
```

---

## Task 1: Extend FileSystem trait

**Files:** `crates/platform-api/src/filesystem.rs`

Add the following methods. They are required by §22 SessionStorage fsync+flock semantics and §23 verify_file_state.

```rust
#[async_trait]
pub trait FileSystem: Send + Sync {
    // ... existing ...

    async fn append_file(&self, path: &str, content: &str) -> Result<(), FsError>;
    async fn truncate(&self, path: &str, len: u64) -> Result<(), FsError>;
    async fn file_mtime(&self, path: &str) -> Result<std::time::SystemTime, FsError>;
    async fn file_size(&self, path: &str) -> Result<u64, FsError>;
    async fn delete_file(&self, path: &str) -> Result<(), FsError>;
    async fn symlink(&self, target: &str, link: &str) -> Result<(), FsError>;

    /// Acquire an OS-level advisory file lock. Returns a guard whose Drop releases.
    /// Mobile platforms (Android/iOS) may stub this with app-internal locking;
    /// the trait method is required so engine code can express the intent.
    async fn flock_exclusive(&self, path: &str) -> Result<Box<dyn FlockGuard>, FsError>;

    /// fsync the file. Engine should call this after every important write.
    async fn fsync(&self, path: &str) -> Result<(), FsError>;
}

pub trait FlockGuard: Send + Sync {
    fn path(&self) -> &str;
}
```

Commit:
```bash
cargo check -p lingxi-traits
git add crates/traits
git commit -m "feat(traits): FileSystem.append_file/truncate/mtime/size/delete/symlink/flock/fsync"
```

---

## Task 2: lingxi-filestate

**Files:** `crates/filestate/{Cargo.toml, src/{lib, cache, verify, merge}.rs}`

- [ ] **Step 1: Cargo.toml**

```toml
[package]
name = "lingxi-filestate"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
lingxi-platform-api = { path = "../platform-api" }
serde.workspace = true
thiserror.workspace = true
sha2 = "0.10"
tracing.workspace = true

[lints]
workspace = true
```

- [ ] **Step 2: cache.rs (LRU with byte counter held under the same lock — B8)**

```rust
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::SystemTime;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileState {
    pub content: String,
    pub timestamp: SystemTime,
    pub offset: Option<u64>,
    pub limit: Option<u64>,
    pub is_partial_view: bool,
}

pub const MAX_ENTRIES: usize = 100;
pub const MAX_BYTES: u64 = 25 * 1024 * 1024;

struct CacheInner {
    map: HashMap<PathBuf, FileState>,
    order: Vec<PathBuf>,
    byte_count: u64,
}

pub struct FileStateCache {
    inner: Mutex<CacheInner>,
    max_entries: usize,
    max_bytes: u64,
}

impl FileStateCache {
    pub fn new(max_entries: usize, max_bytes: u64) -> Self {
        Self {
            inner: Mutex::new(CacheInner { map: HashMap::new(), order: Vec::new(), byte_count: 0 }),
            max_entries, max_bytes,
        }
    }

    pub fn get(&self, path: &str) -> Option<FileState> {
        let inner = self.inner.lock().unwrap();
        let p = normalize(path);
        inner.map.get(&p).cloned()
    }

    pub fn set(&self, path: &str, state: FileState) {
        let mut inner = self.inner.lock().unwrap();
        let p = normalize(path);
        let new_size = state.content.len() as u64;
        if let Some(old) = inner.map.remove(&p) {
            inner.byte_count = inner.byte_count.saturating_sub(old.content.len() as u64);
            inner.order.retain(|k| k != &p);
        }
        inner.map.insert(p.clone(), state);
        inner.order.push(p);
        inner.byte_count = inner.byte_count.saturating_add(new_size);
        // Evict until under both limits.
        while inner.map.len() > self.max_entries || inner.byte_count > self.max_bytes {
            if let Some(victim) = inner.order.first().cloned() {
                if let Some(old) = inner.map.remove(&victim) {
                    inner.byte_count = inner.byte_count.saturating_sub(old.content.len() as u64);
                }
                inner.order.remove(0);
            } else {
                break;
            }
        }
    }

    pub fn delete(&self, path: &str) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let p = normalize(path);
        if let Some(s) = inner.map.remove(&p) {
            inner.byte_count = inner.byte_count.saturating_sub(s.content.len() as u64);
            inner.order.retain(|k| k != &p);
            true
        } else { false }
    }

    pub fn dump(&self) -> Vec<(PathBuf, FileState)> {
        self.inner.lock().unwrap().map.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }

    pub fn load(&self, entries: Vec<(PathBuf, FileState)>) {
        for (k, v) in entries {
            self.set(k.to_str().unwrap_or(""), v);
        }
    }

    pub fn clone_cache(&self) -> Self {
        let out = Self::new(self.max_entries, self.max_bytes);
        let entries: Vec<_> = self.inner.lock().unwrap().map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        out.load(entries);
        out
    }
}

fn normalize(path: &str) -> PathBuf {
    use std::path::Component;
    let p = PathBuf::from(path);
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => { out.pop(); }
            Component::CurDir => {},
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_when_over_byte_limit() {
        let c = FileStateCache::new(100, 100);
        c.set("a.txt", FileState { content: "x".repeat(80), timestamp: SystemTime::now(), offset: None, limit: None, is_partial_view: false });
        c.set("b.txt", FileState { content: "y".repeat(80), timestamp: SystemTime::now(), offset: None, limit: None, is_partial_view: false });
        // a was evicted to keep under 100 bytes
        assert!(c.get("b.txt").is_some());
    }
}
```

- [ ] **Step 3: verify.rs**

```rust
use crate::cache::{FileState, FileStateCache};
use sha2::{Digest, Sha256};
use std::time::SystemTime;

#[derive(Debug, Clone)]
pub enum FileStateVerification {
    Valid,
    NotInCache,
    PartialView,
    ModifiedSinceRead { cached_at: SystemTime, disk_mtime: SystemTime },
    ContentMismatch,
}

pub fn verify_file_state(
    cache: &FileStateCache,
    path: &str,
    current_disk_mtime: SystemTime,
    current_disk_hash: Option<[u8; 32]>,
) -> FileStateVerification {
    let Some(cached) = cache.get(path) else { return FileStateVerification::NotInCache; };
    if cached.is_partial_view { return FileStateVerification::PartialView; }
    if current_disk_mtime > cached.timestamp {
        return FileStateVerification::ModifiedSinceRead { cached_at: cached.timestamp, disk_mtime: current_disk_mtime };
    }
    if let Some(disk_hash) = current_disk_hash {
        let mut h = Sha256::new();
        h.update(cached.content.as_bytes());
        let cached_hash: [u8; 32] = h.finalize().into();
        if disk_hash != cached_hash { return FileStateVerification::ContentMismatch; }
    }
    FileStateVerification::Valid
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_in_cache_returned_when_missing() {
        let c = FileStateCache::new(10, 1024);
        let v = verify_file_state(&c, "missing.txt", SystemTime::now(), None);
        assert!(matches!(v, FileStateVerification::NotInCache));
    }

    #[test]
    fn detects_modification_via_mtime() {
        let c = FileStateCache::new(10, 1024);
        let old = SystemTime::UNIX_EPOCH;
        c.set("a.txt", FileState { content: "hi".into(), timestamp: old, offset: None, limit: None, is_partial_view: false });
        let v = verify_file_state(&c, "a.txt", old + std::time::Duration::from_secs(1), None);
        assert!(matches!(v, FileStateVerification::ModifiedSinceRead { .. }));
    }
}
```

- [ ] **Step 4: merge.rs**

```rust
use crate::cache::{FileState, FileStateCache};

pub fn merge_caches(into: &FileStateCache, from: &FileStateCache) {
    for (path, state) in from.dump() {
        let p = path.to_str().unwrap_or("");
        if let Some(existing) = into.get(p) {
            if state.timestamp > existing.timestamp {
                into.set(p, state);
            }
        } else {
            into.set(p, state);
        }
    }
}
```

- [ ] **Step 5: lib.rs + commit**

```rust
#![forbid(unsafe_code)]
pub mod cache;
pub mod merge;
pub mod verify;

pub use cache::{FileState, FileStateCache, MAX_BYTES, MAX_ENTRIES};
pub use merge::merge_caches;
pub use verify::{verify_file_state, FileStateVerification};
```

```bash
cargo test -p lingxi-filestate
git add crates/filestate
git commit -m "feat(filestate): LRU cache with byte counter + verify + merge"
```

---

## Task 3: lingxi-msgqueue

**Files:** `crates/msgqueue/{Cargo.toml, src/{lib,queue,operations}.rs}`

- [ ] **Step 1: queue.rs (Ord on priority — B3)**

```rust
use lingxi_protocol::{AgentId, HookId, ToolUseId};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::{Notify, RwLock};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuedCommand {
    pub uuid: String,
    pub content: QueuedCommandContent,
    pub priority: QueuePriority,
    pub queued_at: SystemTime,
    pub source: QueueSource,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QueuedCommandContent {
    UserInput { text: String },
    SlashCommand { parsed_json: serde_json::Value },
    TaskNotification { value: String, mode: NotificationMode },
    TeammateMessage { from: AgentId, content: String },
    OrphanedPermission { tool_use_id: ToolUseId, reason: String },
    HookInjected { content: String, hook_id: HookId },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NotificationMode { Normal, TaskNotification }

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum QueuePriority { Later, Next, Now } // Ord: Later < Next < Now (dequeue picks max)

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueueSource { PromptInput, TaskCompletion, AgentSendMessage, Hook, Orphan, Cron }

pub struct MessageQueueManager {
    queue: Arc<RwLock<VecDeque<QueuedCommand>>>,
    notify: Arc<Notify>,
}

impl MessageQueueManager {
    pub fn new() -> Self {
        Self { queue: Arc::new(RwLock::new(VecDeque::new())), notify: Arc::new(Notify::new()) }
    }

    pub async fn enqueue(&self, cmd: QueuedCommand) {
        let mut q = self.queue.write().await;
        // Insert in priority order — find first item with strictly lower priority.
        let pos = q.iter().position(|c| c.priority < cmd.priority).unwrap_or(q.len());
        q.insert(pos, cmd);
        self.notify.notify_one();
    }

    pub async fn dequeue(&self) -> Option<QueuedCommand> {
        self.queue.write().await.pop_front()
    }

    pub async fn drain_now_priority(&self) -> Vec<QueuedCommand> {
        let mut q = self.queue.write().await;
        let mut out = Vec::new();
        while let Some(front) = q.front() {
            if front.priority == QueuePriority::Now {
                out.push(q.pop_front().unwrap());
            } else { break; }
        }
        out
    }

    pub async fn snapshot(&self) -> Vec<QueuedCommand> {
        self.queue.read().await.iter().cloned().collect()
    }

    pub async fn wait_for_message(&self, timeout: std::time::Duration) -> Option<QueuedCommand> {
        tokio::select! {
            _ = self.notify.notified() => self.queue.write().await.pop_front(),
            _ = tokio::time::sleep(timeout) => None,
        }
    }
}

impl Default for MessageQueueManager { fn default() -> Self { Self::new() } }

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(priority: QueuePriority, text: &str) -> QueuedCommand {
        QueuedCommand {
            uuid: text.into(),
            content: QueuedCommandContent::UserInput { text: text.into() },
            priority, queued_at: SystemTime::now(), source: QueueSource::PromptInput,
        }
    }

    #[tokio::test]
    async fn now_priority_dequeues_first() {
        let q = MessageQueueManager::new();
        q.enqueue(mk(QueuePriority::Later, "later")).await;
        q.enqueue(mk(QueuePriority::Now, "now")).await;
        q.enqueue(mk(QueuePriority::Next, "next")).await;
        assert_eq!(q.dequeue().await.unwrap().uuid, "now");
        assert_eq!(q.dequeue().await.unwrap().uuid, "next");
        assert_eq!(q.dequeue().await.unwrap().uuid, "later");
    }

    #[tokio::test]
    async fn drain_now_returns_only_now_items() {
        let q = MessageQueueManager::new();
        q.enqueue(mk(QueuePriority::Now, "n1")).await;
        q.enqueue(mk(QueuePriority::Now, "n2")).await;
        q.enqueue(mk(QueuePriority::Next, "x")).await;
        let drained = q.drain_now_priority().await;
        assert_eq!(drained.len(), 2);
    }
}
```

- [ ] **Step 2: operations.rs (queue op log for crash recovery)**

```rust
use serde::{Deserialize, Serialize};
use crate::queue::{QueuePriority, QueueSource};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QueueOperation {
    Enqueue { uuid: String, priority: QueuePriority, source: QueueSource },
    Dequeue { uuid: String },
    Remove { uuid: String, reason: String },
    Clear { count: usize },
}
```

- [ ] **Step 3: lib.rs + commit**

```rust
#![forbid(unsafe_code)]
pub mod operations;
pub mod queue;

pub use operations::QueueOperation;
pub use queue::*;
```

```bash
cargo test -p lingxi-msgqueue
git add crates/msgqueue
git commit -m "feat(msgqueue): priority queue + ops log"
```

---

## Task 4: lingxi-session — file layout + metadata + storage + resumer

**Files:** `crates/session/{Cargo.toml, src/{lib, metadata, transcript, storage, jsonl, resumer}.rs}`

- [ ] **Step 1: Cargo.toml**

```toml
[package]
name = "lingxi-session"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
lingxi-protocol = { path = "../protocol" }
lingxi-platform-api = { path = "../platform-api" }
lingxi-core = { path = "../core" }
lingxi-filestate = { path = "../filestate" }
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
async-trait.workspace = true
tokio = { version = "1", features = ["sync"] }
tracing.workspace = true

[lints]
workspace = true
```

- [ ] **Step 2: metadata.rs**

```rust
use lingxi_protocol::{PluginId, SessionId};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::SystemTime;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub session_id: SessionId,
    pub parent_session_id: Option<SessionId>,
    pub created_at: SystemTime,
    pub project_dir: PathBuf,
    pub cwd: PathBuf,
    pub agent_type: Option<String>,
    pub model: String,
    pub permission_mode: String,
    pub coordinator_mode: bool,
    pub enabled_plugins: Vec<PluginId>,
    pub mcp_servers_enabled: Vec<String>,
    pub working_directories: Vec<PathBuf>,
    pub current_output_style: String,
    pub claude_md_paths: Vec<PathBuf>,
    pub last_modified: SystemTime,
}
```

- [ ] **Step 3: transcript.rs**

```rust
use lingxi_protocol::{AgentId, ConversationMessage, HookId, MessageId, RequestId, SessionId, ToolUseId};
use serde::{Deserialize, Serialize};
use std::time::SystemTime;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TranscriptEntry {
    Message { uuid: MessageId, timestamp: SystemTime, message: ConversationMessage },
    ToolUseSummary { tool_use_id: ToolUseId, summary: String },
    CompactBoundary { summary: String },
    Tombstone { replaced_uuid: MessageId, reason: String },
    HookResult { hook_id: HookId, outcome: String },
    SessionResumed { previous_session_id: SessionId, resumed_at: SystemTime },
}
```

- [ ] **Step 4: jsonl.rs (crash-safe reader — B5)**

```rust
use crate::transcript::TranscriptEntry;
use lingxi_platform_api::{FileSystem, FsError};
use thiserror::Error;

#[derive(Debug, Clone, Error)]
pub enum StorageError {
    #[error(transparent)]
    Fs(#[from] FsError),
    #[error("corrupted at byte {0}")]
    Corrupted(u64),
}

pub struct RecoveryResult {
    pub entries: Vec<TranscriptEntry>,
    pub truncated_at: u64,
}

pub async fn read_recover(fs: &dyn FileSystem, path: &str) -> Result<RecoveryResult, StorageError> {
    let content = fs.read_file(path, None, None).await?.content;
    let mut entries = Vec::new();
    let mut last_valid_offset: u64 = 0;
    for line in content.lines() {
        match serde_json::from_str::<TranscriptEntry>(line) {
            Ok(e) => {
                entries.push(e);
                last_valid_offset += line.len() as u64 + 1;
            }
            Err(_) => {
                fs.truncate(path, last_valid_offset).await?;
                return Ok(RecoveryResult { entries, truncated_at: last_valid_offset });
            }
        }
    }
    Ok(RecoveryResult { entries, truncated_at: last_valid_offset })
}
```

- [ ] **Step 5: storage.rs (append + fsync + flock)**

```rust
use crate::jsonl::{read_recover, StorageError};
use crate::metadata::SessionMetadata;
use crate::transcript::TranscriptEntry;
use lingxi_platform_api::FileSystem;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct SessionStorage {
    base_dir: PathBuf,
    fs: Arc<dyn FileSystem>,
    write_lock: Mutex<()>,
}

#[derive(Debug, Clone)]
pub struct LoadedSession {
    pub metadata: SessionMetadata,
    pub messages: Vec<lingxi_protocol::ConversationMessage>,
}

impl SessionStorage {
    pub fn new(base_dir: PathBuf, fs: Arc<dyn FileSystem>) -> Self {
        Self { base_dir, fs, write_lock: Mutex::new(()) }
    }

    pub fn session_dir(&self, session_id: &lingxi_protocol::SessionId) -> PathBuf {
        self.base_dir.join(session_id.as_uuid().to_string())
    }

    pub fn transcript_path(&self, session_id: &lingxi_protocol::SessionId) -> PathBuf {
        self.session_dir(session_id).join("transcript.jsonl")
    }

    pub async fn append(&self, session_id: &lingxi_protocol::SessionId, entry: TranscriptEntry) -> Result<(), StorageError> {
        let _guard = self.write_lock.lock().await;
        let path = self.transcript_path(session_id);
        let line = format!("{}\n", serde_json::to_string(&entry).unwrap());
        let path_str = path.to_str().unwrap();
        let _flock = self.fs.flock_exclusive(path_str).await?;
        self.fs.append_file(path_str, &line).await?;
        self.fs.fsync(path_str).await?;
        Ok(())
    }

    pub async fn save_metadata(&self, metadata: &SessionMetadata) -> Result<(), StorageError> {
        let path = self.session_dir(&metadata.session_id).join("metadata.json");
        let path_str = path.to_str().unwrap();
        let _flock = self.fs.flock_exclusive(path_str).await?;
        self.fs.write_file(path_str, &serde_json::to_string_pretty(metadata).unwrap()).await?;
        self.fs.fsync(path_str).await?;
        Ok(())
    }

    pub async fn load(&self, session_id: &lingxi_protocol::SessionId) -> Result<LoadedSession, StorageError> {
        let meta_path = self.session_dir(session_id).join("metadata.json");
        let meta_str = self.fs.read_file(meta_path.to_str().unwrap(), None, None).await?.content;
        let metadata: SessionMetadata = serde_json::from_str(&meta_str).map_err(|e| StorageError::Corrupted(0))?;

        let transcript_path = self.transcript_path(session_id);
        let recovery = read_recover(&*self.fs, transcript_path.to_str().unwrap()).await?;
        let messages: Vec<_> = recovery.entries.into_iter()
            .filter_map(|e| if let TranscriptEntry::Message { message, .. } = e { Some(message) } else { None })
            .collect();
        Ok(LoadedSession { metadata, messages })
    }
}
```

- [ ] **Step 6: resumer.rs (cross-subsystem restore — C7)**

```rust
use crate::storage::{LoadedSession, SessionStorage};
use lingxi_filestate::FileStateCache;
use lingxi_platform_api::FileSystem;
use std::sync::Arc;
use thiserror::Error;

pub struct ResumedSession {
    pub loaded: LoadedSession,
    pub file_state_cache: FileStateCache,
}

#[derive(Debug, Clone, Error)]
pub enum ResumeError {
    #[error("storage: {0}")]
    Storage(String),
}

pub struct SessionResumer {
    storage: Arc<SessionStorage>,
    fs: Arc<dyn FileSystem>,
}

impl SessionResumer {
    pub fn new(storage: Arc<SessionStorage>, fs: Arc<dyn FileSystem>) -> Self {
        Self { storage, fs }
    }

    /// M1.16 restores the session bones; Plug-in/MCP/Permission/Cost/OutputStyle
    /// re-attachment is done by the host wrapper in Plan 15 + Plan 16. This
    /// function returns the loaded transcript + an empty FileStateCache.
    ///
    /// Note: SessionResumer does NOT rebuild FileStateCache from stale
    /// historical Reads (B6) — the cache restarts empty and the agent re-Reads
    /// any file it needs.
    pub async fn resume(&self, session_id: &lingxi_protocol::SessionId) -> Result<ResumedSession, ResumeError> {
        let loaded = self.storage.load(session_id).await.map_err(|e| ResumeError::Storage(e.to_string()))?;
        let cache = FileStateCache::new(lingxi_filestate::MAX_ENTRIES, lingxi_filestate::MAX_BYTES);
        Ok(ResumedSession { loaded, file_state_cache: cache })
    }
}
```

- [ ] **Step 7: lib.rs + commit**

```rust
#![forbid(unsafe_code)]
pub mod jsonl;
pub mod metadata;
pub mod resumer;
pub mod storage;
pub mod transcript;

pub use jsonl::{read_recover, RecoveryResult, StorageError};
pub use metadata::SessionMetadata;
pub use resumer::{ResumeError, ResumedSession, SessionResumer};
pub use storage::{LoadedSession, SessionStorage};
pub use transcript::TranscriptEntry;
```

```bash
cargo test -p lingxi-session
git add crates/session
git commit -m "feat(session): metadata + JSONL crash-safe reader + storage + resumer"
```

---

## Task 5: Plan exit

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
git tag -a m1.16-session-filestate-msgqueue -m "Plan 10 complete"
```

## Self-Review

- §22.1 file layout → storage.rs paths ✓
- §22.2 SessionMetadata → metadata.rs ✓
- §22.3 append + fsync + flock (B5) → storage.rs ✓
- §22.4 crash-safe JSONL → jsonl.rs ✓
- §22.5 SessionResumer (C7) → resumer.rs ✓
- §23.1 FileStateCache (B8 byte counter under same lock) → cache.rs ✓
- §23.2 verify_file_state (C5) → verify.rs ✓
- §23.4 clone/merge → merge.rs ✓
- §27.1 QueuedCommand + 6 variants → msgqueue/queue.rs ✓
- §27.2 MessageQueueManager + Ord on priority (B3, B4) → msgqueue/queue.rs ✓

## Execution Handoff

Next: **Plan 11 — Cron Scheduler** (`2026-05-22-lingxi-core-m1-11-cron.md`).

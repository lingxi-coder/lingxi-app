//! Product-A V2 todo-task store — file-backed 1:1 port of claude-code
//! `src/utils/tasks.ts`.
//!
//! Each task list lives under `<lingxi_config_home>/tasks/<sanitize(list_id)>/`
//! with one `<id>.json` per task (`id` is a decimal string `"1".."N"`) plus a
//! `.highwatermark` file recording the highest id ever assigned (so deleting /
//! resetting never reuses an id). Tasks carry `engine::TodoState`
//! (`pending`/`in_progress`/`completed`) — the V2 status enum — which is a
//! SEPARATE space from the Product-B `TASK_STATUSES` in `task.rs`.
//!
//! ## Cross-process serialisation
//!
//! claude-code serialises concurrent swarm agents with an on-disk
//! `proper-lockfile` (`utils/tasks.ts` `LOCK_OPTIONS` retry budget sized for ~10
//! racing processes). Two LingXi OS processes CAN touch one tasks dir (a
//! detached `--bg` worker plus an `--resume` attach, or an env-shared
//! `LINGXI_TASK_LIST_ID`), so a purely in-process lock would allow duplicate
//! ids (`create`) and lost updates (`update`). We therefore match claude-code:
//!
//! * an in-process per-directory `tokio::sync::Mutex` (a global registry keyed
//!   by dir) as a cheap fast-path so same-process contention never burns the
//!   file-lock retry budget, PLUS
//! * a `mkdir`-based cross-process [`crate::proper_lockfile`] lock around the
//!   read-modify-write, with the SAME on-disk artifacts claude-code produces:
//!   - `create` locks the list-level lock target `<dir>/.lock` (pre-created as
//!     an empty file, TS `writeFile(t,"",{flag:"wx"})`) → lock dir `.lock.lock`;
//!   - `update` locks the per-task file `<id>.json` → lock dir `<id>.json.lock`;
//!   - `delete` bumps the high-water mark and unlinks WITHOUT a lock (matching
//!     claude-code `deleteTask`), cascading through locked `update`s;
//!   - `get`/`list` take no lock (parse failures swallowed), like claude-code.
//!
//! We also resolve `getTaskListId()` to the session id directly (the teammate /
//! team-name branches collapse to the session in single-process).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::sync::Mutex;

use engine::TodoState;

/// High-water-mark file name (claude-code `HIGH_WATER_MARK_FILE`).
const HIGH_WATER_MARK_FILE: &str = ".highwatermark";

/// One V2 task as persisted on disk. 1:1 with claude-code `TaskSchema`
/// (`utils/tasks.ts`); wire keys are camelCase (`activeForm`, `blockedBy`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TodoTask {
    /// Decimal task id (`"1".."N"`), assigned by [`TodoStore::create`].
    pub id: String,
    /// Brief, imperative title for the task.
    pub subject: String,
    /// What needs to be done.
    pub description: String,
    /// Present-continuous spinner form (e.g. `"Running tests"`). Optional;
    /// omitted from disk when absent (matches TS `JSON.stringify` of
    /// `undefined`).
    #[serde(
        rename = "activeForm",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub active_form: Option<String>,
    /// Owning agent id, when claimed. Optional.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// Lifecycle state — the V2 enum (`pending`/`in_progress`/`completed`).
    pub status: TodoState,
    /// Task ids this task blocks.
    #[serde(default)]
    pub blocks: Vec<String>,
    /// Task ids that block this task.
    #[serde(rename = "blockedBy", default)]
    pub blocked_by: Vec<String>,
    /// Arbitrary attached metadata.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub metadata: Map<String, Value>,
}

impl TodoTask {
    /// Construct a fresh `pending` task with empty id (filled in by
    /// [`TodoStore::create`]) and no blocks / owner.
    #[must_use]
    pub fn new(
        subject: String,
        description: String,
        active_form: Option<String>,
        metadata: Map<String, Value>,
    ) -> Self {
        Self {
            id: String::new(),
            subject,
            description,
            active_form,
            owner: None,
            status: TodoState::Pending,
            blocks: Vec::new(),
            blocked_by: Vec::new(),
            metadata,
        }
    }
}

// ── path helpers (port of getClaudeConfigHomeDir / getTasksDir / sanitize) ──

/// Port of claude-code `tr()` (`$LINGXI_CONFIG_DIR ?? join(home, ".lingxi")`):
/// `$LINGXI_CONFIG_DIR` when set is honored verbatim (`??`, incl. an empty value
/// → cwd-relative), else `$HOME/.claude` (falling back to `USERPROFILE` and
/// finally a bare `.claude` so the path is always well-formed).
fn lingxi_config_home_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os(branding::CONFIG_DIR_ENV) {
        return PathBuf::from(dir);
    }
    match std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        Some(home) => PathBuf::from(home).join(branding::DOT_DIR),
        None => PathBuf::from(branding::DOT_DIR),
    }
}

/// `{configHome}/tasks` (parent of every per-list tasks dir).
fn tasks_root() -> PathBuf {
    lingxi_config_home_dir().join("tasks")
}

/// Port of `sanitizePathComponent()` (`utils/tasks.ts`): replace every char
/// outside `[a-zA-Z0-9_-]` with `-`.
#[must_use]
pub fn sanitize_path_component(input: &str) -> String {
    input
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Global registry of per-directory locks. Two stores at the same dir share the
/// same async mutex, serialising mutations in-process (see the module-level
/// single-process divergence note).
static LOCKS: Lazy<std::sync::Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> =
    Lazy::new(|| std::sync::Mutex::new(HashMap::new()));

fn lock_for(dir: &Path) -> Arc<Mutex<()>> {
    let mut map = LOCKS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    map.entry(dir.to_path_buf())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

/// File-backed task store for a single task list.
pub struct TodoStore {
    dir: PathBuf,
    lock: Arc<Mutex<()>>,
}

impl TodoStore {
    /// Open the store for `list_id` under the resolved claude config home
    /// (`<configHome>/tasks/<sanitize(list_id)>/`).
    #[must_use]
    pub fn for_list(list_id: &str) -> Self {
        Self::in_dir(tasks_root().join(sanitize_path_component(list_id)))
    }

    /// Open a store rooted at an explicit tasks directory (used by tests).
    #[must_use]
    pub fn in_dir(dir: PathBuf) -> Self {
        let lock = lock_for(&dir);
        Self { dir, lock }
    }

    /// The directory backing this store.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn task_path(&self, id: &str) -> PathBuf {
        self.dir
            .join(format!("{}.json", sanitize_path_component(id)))
    }

    fn hwm_path(&self) -> PathBuf {
        self.dir.join(HIGH_WATER_MARK_FILE)
    }

    /// List-level lock target (`<dir>/.lock`). claude-code `createTask` locks
    /// this file for the whole list; the actual lock artifact is the directory
    /// `<dir>/.lock.lock` created by [`crate::proper_lockfile`].
    fn list_lock_target(&self) -> PathBuf {
        self.dir.join(".lock")
    }

    /// Ensure the list-level lock target file exists (claude-code
    /// `writeFile(t,"",{flag:"wx"})` — create empty, tolerate `EEXIST`), so the
    /// mkdir-based lock has a stable sibling to guard.
    fn ensure_list_lock_target(&self) {
        let path = self.list_lock_target();
        // `create_new` == flag "wx"; an existing file (or a benign race) is fine.
        let _ = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path);
    }

    fn ensure_dir(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)
    }

    fn read_high_water_mark(&self) -> i64 {
        std::fs::read_to_string(self.hwm_path())
            .ok()
            .and_then(|s| s.trim().parse::<i64>().ok())
            .unwrap_or(0)
    }

    fn write_high_water_mark(&self, value: i64) -> std::io::Result<()> {
        std::fs::write(self.hwm_path(), value.to_string())
    }

    /// Highest id from existing `<id>.json` files (ignores the high-water mark).
    fn highest_id_from_files(&self) -> i64 {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return 0;
        };
        let mut highest = 0_i64;
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Some(stem) = name.strip_suffix(".json") {
                if let Ok(n) = stem.parse::<i64>() {
                    if n > highest {
                        highest = n;
                    }
                }
            }
        }
        highest
    }

    /// Highest id ever assigned (max of existing files and the high-water mark).
    fn highest_id(&self) -> i64 {
        self.highest_id_from_files()
            .max(self.read_high_water_mark())
    }

    fn write_task(&self, task: &TodoTask) -> std::io::Result<()> {
        // TS `jsonStringify(task, null, 2)`.
        let json = serde_json::to_string_pretty(task)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(self.task_path(&task.id), json)
    }

    /// Create a new task with a unique decimal id. Port of `createTask`:
    /// lock → `next_id = highest_id() + 1` → write → return id.
    ///
    /// # Errors
    /// Propagates filesystem errors from directory creation or the task write.
    pub async fn create(&self, mut task: TodoTask) -> std::io::Result<String> {
        let _guard = self.lock.lock().await;
        self.ensure_dir()?;
        // Cross-process serialisation: hold the list-level lock across
        // highest_id() -> write so two processes never assign the same id.
        // Best-effort — a failure to acquire (only under sustained >retry-budget
        // contention) degrades to the previous in-process-only behaviour rather
        // than making create newly fallible.
        self.ensure_list_lock_target();
        let _xlock = crate::proper_lockfile::lock(&self.list_lock_target())
            .await
            .ok();
        let id = (self.highest_id() + 1).to_string();
        task.id.clone_from(&id);
        self.write_task(&task)?;
        Ok(id)
    }

    /// Read a task by id, or `None` if it is missing / fails to parse (port of
    /// `getTask`: a parse failure is swallowed and surfaces as `null`).
    // `async` is kept for store-API uniformity (callers `.await` every method);
    // the single-process read takes no lock so there is nothing to await here.
    #[allow(clippy::unused_async)]
    pub async fn get(&self, id: &str) -> Option<TodoTask> {
        let content = std::fs::read_to_string(self.task_path(id)).ok()?;
        serde_json::from_str::<TodoTask>(&content).ok()
    }

    /// List every parseable task. Port of `listTasks` (readdir → parse each).
    /// claude-code returns readdir order; we sort by numeric id ascending for
    /// determinism (which also matches the tool's "prefer ID order" guidance).
    pub async fn list(&self) -> Vec<TodoTask> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut ids: Vec<String> = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Some(stem) = name.strip_suffix(".json") {
                ids.push(stem.to_string());
            }
        }
        ids.sort_by_key(|id| id.parse::<i64>().unwrap_or(i64::MAX));
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(task) = self.get(&id).await {
                out.push(task);
            }
        }
        out
    }

    /// Apply `mutate` to the task under `id` (read → mutate → write), holding
    /// the list lock. Port of `updateTask`: returns the updated task, or `None`
    /// if the task does not exist / cannot be read. `id` is always preserved.
    pub async fn update(&self, id: &str, mutate: impl FnOnce(&mut TodoTask)) -> Option<TodoTask> {
        let _guard = self.lock.lock().await;
        // Cross-process serialisation: claude-code `updateTask` locks the
        // individual task file (`<id>.json` -> `<id>.json.lock`) around its
        // read-merge-write, so a concurrent process cannot lose this update.
        // Best-effort acquire (see `create`).
        let task_path = self.task_path(id);
        let _xlock = crate::proper_lockfile::lock(&task_path).await.ok();
        let content = std::fs::read_to_string(&task_path).ok()?;
        let mut task = serde_json::from_str::<TodoTask>(&content).ok()?;
        mutate(&mut task);
        task.id = id.to_string();
        self.write_task(&task).ok()?;
        Some(task)
    }

    /// Delete a task. Port of `deleteTask`: bump the high-water mark to prevent
    /// id reuse, unlink the file, then cascade-remove this id from every other
    /// task's `blocks` / `blockedBy`. Returns `false` when the file is absent or
    /// an error occurs.
    pub async fn delete(&self, id: &str) -> bool {
        {
            let _guard = self.lock.lock().await;
            if let Ok(n) = id.parse::<i64>() {
                if n > self.read_high_water_mark() {
                    let _ = self.write_high_water_mark(n);
                }
            }
            match std::fs::remove_file(self.task_path(id)) {
                Ok(()) => {}
                Err(_) => return false,
            }
        }
        // Cascade (re-acquires the lock per `update`, so it must run after the
        // guard above is dropped).
        for task in self.list().await {
            let has_block = task.blocks.iter().any(|b| b == id);
            let has_blocked_by = task.blocked_by.iter().any(|b| b == id);
            if has_block || has_blocked_by {
                self.update(&task.id, |t| {
                    t.blocks.retain(|b| b != id);
                    t.blocked_by.retain(|b| b != id);
                })
                .await;
            }
        }
        true
    }

    /// Establish a bidirectional block edge: `from` blocks `to` (so `from.blocks`
    /// gains `to` and `to.blockedBy` gains `from`). Port of `blockTask`. Returns
    /// `false` if either task is missing.
    pub async fn block_task(&self, from: &str, to: &str) -> bool {
        let (Some(from_task), Some(to_task)) = (self.get(from).await, self.get(to).await) else {
            return false;
        };
        if !from_task.blocks.iter().any(|b| b == to) {
            self.update(from, |t| {
                if !t.blocks.iter().any(|b| b == to) {
                    t.blocks.push(to.to_string());
                }
            })
            .await;
        }
        if !to_task.blocked_by.iter().any(|b| b == from) {
            self.update(to, |t| {
                if !t.blocked_by.iter().any(|b| b == from) {
                    t.blocked_by.push(from.to_string());
                }
            })
            .await;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_store() -> (TodoStore, std::path::PathBuf) {
        // A process-wide counter guarantees per-call uniqueness: the wall-clock
        // nanos alone collide when several test threads call this concurrently
        // on a coarse-resolution clock.
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let mut dir = std::env::temp_dir();
        let unique = format!(
            "lingxi-todo-store-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        );
        dir.push(unique);
        (TodoStore::in_dir(dir.clone()), dir)
    }

    #[test]
    fn sanitize_replaces_unsafe_chars() {
        // ":" "/" "." "." "/" each map to "-" (5 chars between abc and x).
        assert_eq!(sanitize_path_component("sess:abc/../x"), "sess-abc----x");
        assert_eq!(sanitize_path_component("ok_-09AZ"), "ok_-09AZ");
        assert_eq!(sanitize_path_component("space here"), "space-here");
    }

    #[tokio::test]
    async fn create_assigns_incrementing_decimal_ids() {
        let (store, dir) = temp_store();
        let a = store
            .create(TodoTask::new(
                "first".into(),
                "do first".into(),
                None,
                Map::new(),
            ))
            .await
            .unwrap();
        let b = store
            .create(TodoTask::new(
                "second".into(),
                "do second".into(),
                None,
                Map::new(),
            ))
            .await
            .unwrap();
        assert_eq!(a, "1");
        assert_eq!(b, "2");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn get_round_trips_all_fields() {
        let (store, dir) = temp_store();
        let mut meta = Map::new();
        meta.insert("k".into(), json!("v"));
        let id = store
            .create(TodoTask::new(
                "subj".into(),
                "desc".into(),
                Some("Doing subj".into()),
                meta.clone(),
            ))
            .await
            .unwrap();
        let got = store.get(&id).await.unwrap();
        assert_eq!(got.id, "1");
        assert_eq!(got.subject, "subj");
        assert_eq!(got.description, "desc");
        assert_eq!(got.active_form.as_deref(), Some("Doing subj"));
        assert_eq!(got.status, TodoState::Pending);
        assert_eq!(got.metadata, meta);
        assert!(got.blocks.is_empty());
        assert!(got.blocked_by.is_empty());
        assert!(store.get("999").await.is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn list_returns_all_in_id_order() {
        let (store, dir) = temp_store();
        for i in 0..3 {
            store
                .create(TodoTask::new(format!("s{i}"), "d".into(), None, Map::new()))
                .await
                .unwrap();
        }
        let all = store.list().await;
        let ids: Vec<&str> = all.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["1", "2", "3"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn update_mutates_and_preserves_id() {
        let (store, dir) = temp_store();
        let id = store
            .create(TodoTask::new("s".into(), "d".into(), None, Map::new()))
            .await
            .unwrap();
        let updated = store
            .update(&id, |t| {
                t.status = TodoState::InProgress;
                t.subject = "renamed".into();
            })
            .await
            .unwrap();
        assert_eq!(updated.id, "1");
        assert_eq!(updated.status, TodoState::InProgress);
        assert_eq!(updated.subject, "renamed");
        assert!(store.update("999", |_| {}).await.is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn delete_removes_file_cascades_refs_and_bumps_hwm() {
        let (store, dir) = temp_store();
        let a = store
            .create(TodoTask::new("a".into(), "d".into(), None, Map::new()))
            .await
            .unwrap();
        let b = store
            .create(TodoTask::new("b".into(), "d".into(), None, Map::new()))
            .await
            .unwrap();
        // a blocks b.
        assert!(store.block_task(&a, &b).await);
        assert_eq!(store.get(&a).await.unwrap().blocks, vec![b.clone()]);
        assert_eq!(store.get(&b).await.unwrap().blocked_by, vec![a.clone()]);

        // Delete a: file gone, b's blockedBy cleared, hwm bumped so ids don't reuse.
        assert!(store.delete(&a).await);
        assert!(store.get(&a).await.is_none());
        assert!(store.get(&b).await.unwrap().blocked_by.is_empty());

        // Next create skips the deleted id (hwm = max existing id at delete time).
        let c = store
            .create(TodoTask::new("c".into(), "d".into(), None, Map::new()))
            .await
            .unwrap();
        assert_eq!(c, "3");
        assert!(!store.delete("999").await);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn mutations_leave_no_stale_lock_directories() {
        let (store, dir) = temp_store();
        let id = store
            .create(TodoTask::new("s".into(), "d".into(), None, Map::new()))
            .await
            .unwrap();
        // create() locks the list-level target: the empty `.lock` file persists
        // (claude-code leaves it too) but the lock *directory* is released.
        assert!(
            dir.join(".lock").is_file(),
            "list lock target `.lock` should be a persisted empty file"
        );
        assert!(
            !dir.join(".lock.lock").exists(),
            "list lock dir `.lock.lock` must be released after create()"
        );

        store
            .update(&id, |t| t.status = TodoState::InProgress)
            .await
            .unwrap();
        assert!(
            !dir.join(format!("{id}.json.lock")).exists(),
            "per-task lock dir `<id>.json.lock` must be released after update()"
        );

        // The lock artifacts never masquerade as tasks.
        let all = store.list().await;
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, id);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn block_task_is_bidirectional_and_idempotent() {
        let (store, dir) = temp_store();
        let a = store
            .create(TodoTask::new("a".into(), "d".into(), None, Map::new()))
            .await
            .unwrap();
        let b = store
            .create(TodoTask::new("b".into(), "d".into(), None, Map::new()))
            .await
            .unwrap();
        assert!(store.block_task(&a, &b).await);
        // Repeat: no duplicate edges.
        assert!(store.block_task(&a, &b).await);
        assert_eq!(store.get(&a).await.unwrap().blocks, vec![b.clone()]);
        assert_eq!(store.get(&b).await.unwrap().blocked_by, vec![a.clone()]);
        assert!(!store.block_task(&a, "999").await);
        let _ = std::fs::remove_dir_all(dir);
    }
}

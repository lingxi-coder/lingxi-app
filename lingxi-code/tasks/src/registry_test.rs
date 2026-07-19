//! Registry tests.
#![allow(clippy::unwrap_used)]

use super::*;
use crate::task_trait::{Task, TaskContext, TaskHandle};
use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex as StdMutex;
use tempfile::tempdir;
use test_harness::mocks::MockRuntimeSpawner;
use traits::filesystem::{FileContent, FileEvent, FileSystem, FlockGuard, FsError};

// ---- In-memory FileSystem (mirrors the other handler/registry tests) ----

struct InMemoryFs {
    files: tokio::sync::Mutex<HashMap<String, String>>,
}
impl InMemoryFs {
    fn new() -> Self {
        Self {
            files: tokio::sync::Mutex::new(HashMap::new()),
        }
    }
}
#[async_trait]
impl FileSystem for InMemoryFs {
    async fn read_file(
        &self,
        path: &str,
        _offset: Option<u64>,
        _limit: Option<u64>,
    ) -> Result<FileContent, FsError> {
        let map = self.files.lock().await;
        let content = map.get(path).cloned().unwrap_or_default();
        let total_lines = content.lines().count() as u64;
        Ok(FileContent {
            content,
            truncated: false,
            total_lines,
        })
    }
    async fn write_file(&self, path: &str, body: &str) -> Result<(), FsError> {
        self.files
            .lock()
            .await
            .insert(path.to_string(), body.to_string());
        Ok(())
    }
    fn is_within_workspace(&self, _: &str) -> bool {
        true
    }
    async fn watch(
        &self,
        _: &str,
    ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = FileEvent> + Send>>, FsError> {
        Err(FsError::Io("not supported".into()))
    }
    async fn append_file(&self, path: &str, body: &str) -> Result<(), FsError> {
        self.files
            .lock()
            .await
            .entry(path.to_string())
            .or_default()
            .push_str(body);
        Ok(())
    }
    async fn truncate(&self, _: &str, _: u64) -> Result<(), FsError> {
        Ok(())
    }
    async fn file_mtime(&self, _: &str) -> Result<std::time::SystemTime, FsError> {
        Ok(std::time::SystemTime::UNIX_EPOCH)
    }
    async fn file_size(&self, path: &str) -> Result<u64, FsError> {
        let map = self.files.lock().await;
        Ok(map.get(path).map_or(0, |s| s.len() as u64))
    }
    async fn delete_file(&self, path: &str) -> Result<(), FsError> {
        self.files.lock().await.remove(path);
        Ok(())
    }
    async fn symlink(&self, _: &str, _: &str) -> Result<(), FsError> {
        Ok(())
    }
    async fn flock_exclusive(&self, _: &str) -> Result<Box<dyn FlockGuard>, FsError> {
        Err(FsError::Io("not supported".into()))
    }
    async fn fsync(&self, _: &str) -> Result<(), FsError> {
        Ok(())
    }
}

// ---- Recording fake handler --------------------------------------------

/// A fake [`Task`] handler that records that `spawn`/`kill` ran and hands
/// back a known handler-generated `task_id` (NOT a `create()`-style
/// `generate_task_id`), so a test can assert dispatch occurred and that the
/// id round-trips out of [`TaskRegistry::spawn`].
struct RecordingHandler {
    task_type: TaskType,
    task_id: String,
    spawns: AtomicUsize,
    killed: StdMutex<Vec<String>>,
    cleanup_count: Option<Arc<AtomicUsize>>,
}
impl RecordingHandler {
    fn new(task_type: TaskType, task_id: &str) -> Arc<Self> {
        Arc::new(Self {
            task_type,
            task_id: task_id.to_string(),
            spawns: AtomicUsize::new(0),
            killed: StdMutex::new(Vec::new()),
            cleanup_count: None,
        })
    }
    fn with_cleanup_counter(
        task_type: TaskType,
        task_id: &str,
        cleanup_count: Arc<AtomicUsize>,
    ) -> Arc<Self> {
        Arc::new(Self {
            task_type,
            task_id: task_id.to_string(),
            spawns: AtomicUsize::new(0),
            killed: StdMutex::new(Vec::new()),
            cleanup_count: Some(cleanup_count),
        })
    }
    fn spawn_count(&self) -> usize {
        self.spawns.load(Ordering::SeqCst)
    }
    fn killed_ids(&self) -> Vec<String> {
        self.killed.lock().unwrap().clone()
    }
}
#[async_trait]
impl Task for RecordingHandler {
    fn name(&self) -> &str {
        "recording"
    }
    fn task_type(&self) -> TaskType {
        self.task_type
    }
    async fn spawn(
        &self,
        _input: TaskSpawnInput,
        _ctx: TaskContext,
    ) -> Result<TaskHandle, TaskError> {
        self.spawns.fetch_add(1, Ordering::SeqCst);
        let cleanup = self.cleanup_count.clone().map(|count| {
            Arc::new(move || {
                count.fetch_add(1, Ordering::SeqCst);
            }) as Arc<dyn Fn() + Send + Sync>
        });
        Ok(TaskHandle {
            task_id: self.task_id.clone(),
            cleanup,
        })
    }
    async fn kill(&self, task_id: &str, _ctx: TaskContext) -> Result<(), TaskError> {
        self.killed.lock().unwrap().push(task_id.to_string());
        Ok(())
    }
}

fn make_registry() -> (tempfile::TempDir, TaskRegistry) {
    let dir = tempdir().unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
    let runtime = Arc::new(MockRuntimeSpawner::default());
    let out_mgr = Arc::new(crate::output_manager::TaskOutputManager::new(
        PathBuf::from(dir.path()),
        fs.clone(),
    ));
    let registry = TaskRegistry::new(runtime, fs, out_mgr);
    (dir, registry)
}

fn teammate_input() -> TaskSpawnInput {
    TaskSpawnInput::InProcessTeammate {
        agent_id: protocol::AgentId::new(),
        name: "buddy".into(),
        team_name: "alpha".into(),
        description: String::new(),
    }
}

#[tokio::test]
async fn spawn_invokes_handler_and_returns_handler_task_id() {
    let (_d, mut registry) = make_registry();
    let handler = RecordingHandler::new(TaskType::InProcessTeammate, "thandlerid");
    registry.register_handler(TaskType::InProcessTeammate, handler.clone());

    let id = registry
        .spawn(
            TaskType::InProcessTeammate,
            teammate_input(),
            "a teammate".into(),
        )
        .await
        .unwrap();

    // Returns the HANDLER-generated id, not a fresh `generate_task_id`.
    assert_eq!(id, "thandlerid", "spawn returns the handler's task_id");
    assert_eq!(handler.spawn_count(), 1, "handler.spawn ran exactly once");

    // And it differs from a `create()` placeholder id for the same type.
    let created = registry
        .create(
            TaskType::InProcessTeammate,
            teammate_input(),
            "placeholder".into(),
        )
        .await
        .unwrap();
    assert_ne!(
        id, created,
        "spawn id is the handler id, distinct from create()'s generated id"
    );

    // The spawned task is tracked under the handler id.
    assert!(
        registry.get(&id).await.is_some(),
        "spawned task state is registered under the handler id"
    );
}

#[tokio::test]
async fn spawn_unknown_type_errors() {
    let (_d, registry) = make_registry();
    // No handler registered for InProcessTeammate.
    let err = registry
        .spawn(
            TaskType::InProcessTeammate,
            teammate_input(),
            "no handler".into(),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, TaskError::UnknownType),
        "spawn with no registered handler errors; got {err:?}"
    );
}

// ---- T15: LocalAgent dispatches once its handler is registered ----------

fn local_agent_input() -> TaskSpawnInput {
    TaskSpawnInput::LocalAgent {
        agent_id: protocol::AgentId::new(),
        subagent_type: "general-purpose".into(),
        prompt: "do the work".into(),
        is_backgrounded: true,
        tool_use_id: None,
        spawn_request: None,
        inheritance: None,
    }
}

#[tokio::test]
async fn spawn_local_agent_unknown_without_handler() {
    // Pre-T15 baseline: with no `LocalAgent` handler registered (the registry
    // helper was never called from any composition root), a `LocalAgent`
    // spawn fails with `UnknownType`.
    let (_d, registry) = make_registry();
    let err = registry
        .spawn(TaskType::LocalAgent, local_agent_input(), "x".into())
        .await
        .unwrap_err();
    assert!(matches!(err, TaskError::UnknownType), "got {err:?}");
}

#[tokio::test]
async fn spawn_local_agent_dispatches_once_registered() {
    // T15: once the `LocalAgent` handler is registered (as the desktop
    // composition root now does), a `LocalAgent` spawn dispatches to it and
    // the task is tracked under the handler id — so background agents surface
    // in TaskList/Get/Output instead of failing with `UnknownType`.
    let (_d, mut registry) = make_registry();
    let handler = RecordingHandler::new(TaskType::LocalAgent, "alocalagent");
    registry.register_handler(TaskType::LocalAgent, handler.clone());

    let id = registry
        .spawn(TaskType::LocalAgent, local_agent_input(), "research".into())
        .await
        .expect("LocalAgent spawn dispatches to its handler");
    assert_eq!(id, "alocalagent");
    assert_eq!(handler.spawn_count(), 1);

    // The spawned task is tracked under the handler id with the LocalAgent
    // state variant carrying the real input fields.
    let state = registry.get(&id).await.expect("LocalAgent task is tracked");
    match state {
        TaskState::LocalAgent(a) => {
            assert!(
                a.is_backgrounded,
                "is_backgrounded threads through from input"
            );
            assert_eq!(a.prompt, "do the work");
        }
        other => panic!("expected a LocalAgent state, got {other:?}"),
    }
}

#[tokio::test]
async fn spawn_records_handle_for_kill() {
    let (_d, mut registry) = make_registry();
    let handler = RecordingHandler::new(TaskType::InProcessTeammate, "tkillme");
    registry.register_handler(TaskType::InProcessTeammate, handler.clone());

    let id = registry
        .spawn(
            TaskType::InProcessTeammate,
            teammate_input(),
            "killable".into(),
        )
        .await
        .unwrap();

    // kill(task_id) finds the spawned task and dispatches to the handler.
    registry.kill(&id).await.unwrap();
    assert_eq!(
        handler.killed_ids(),
        vec![id.clone()],
        "registry.kill dispatched to the handler's kill with the handler id"
    );
}

// ---- T04: TeamSpawnSeam impl on TaskRegistry ---------------------------

#[tokio::test]
async fn team_spawn_seam_spawns_real_teammate() {
    use traits::team_spawn::TeamSpawnSeam;

    let (_d, mut registry) = make_registry();
    // Reuse the T01 recording handler: it records that `spawn` ran and
    // hands back a known handler-generated id, so we can assert the seam
    // dispatched into `TaskRegistry::spawn` and returned THAT id.
    let handler = RecordingHandler::new(TaskType::InProcessTeammate, "tseamid");
    registry.register_handler(TaskType::InProcessTeammate, handler.clone());

    let seam: &dyn TeamSpawnSeam = &registry;
    let task_id = seam
        .spawn_teammate(
            protocol::AgentId::new(),
            "buddy".into(),
            "alpha".into(),
            "a teammate".into(),
        )
        .await
        .unwrap();

    // Non-empty, handler-generated id (NOT the worker AgentId).
    assert!(!task_id.is_empty(), "seam returns a non-empty task_id");
    assert_eq!(task_id, "tseamid", "seam returns the handler-generated id");
    assert_eq!(
        handler.spawn_count(),
        1,
        "the teammate handler ran exactly once"
    );

    // The spawned task is tracked under the handler id (so a later kill
    // routes back to the owning handler).
    assert!(
        registry.get(&task_id).await.is_some(),
        "spawned teammate state is registered under the handler id"
    );

    // kill via the seam dispatches teardown to the handler.
    seam.kill(&task_id).await.unwrap();
    assert_eq!(
        handler.killed_ids(),
        vec![task_id.clone()],
        "seam.kill routed teardown to the handler with the handler id"
    );
}

#[tokio::test]
async fn team_spawn_seam_unknown_handler_is_unsupported() {
    use traits::team_spawn::{TeamSpawnError, TeamSpawnSeam};

    let (_d, registry) = make_registry();
    // No InProcessTeammate handler registered.
    let seam: &dyn TeamSpawnSeam = &registry;
    let err = seam
        .spawn_teammate(
            protocol::AgentId::new(),
            "buddy".into(),
            "alpha".into(),
            "no handler".into(),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, TeamSpawnError::Unsupported(_)),
        "missing teammate handler maps to TeamSpawnError::Unsupported; got {err:?}"
    );
}

// ---- TeamSpawnSeam::send_message override (the mailbox→runner bridge) ----

/// A fake [`Task`] handler that supports messages and records every
/// `send_message` it receives (as `(task_id, message)`), so a test can
/// assert the registry override routed the inject to the owning handler.
/// `kill_terminates` makes its `send_message` return [`TaskError::TerminatedTask`]
/// so the terminal→stop mapping can be exercised.
struct MsgRecordingHandler {
    task_type: TaskType,
    task_id: String,
    msgs: StdMutex<Vec<(String, String)>>,
    supports: bool,
    terminate: bool,
}
impl MsgRecordingHandler {
    fn new(task_type: TaskType, task_id: &str) -> Arc<Self> {
        Arc::new(Self {
            task_type,
            task_id: task_id.to_string(),
            msgs: StdMutex::new(Vec::new()),
            supports: true,
            terminate: false,
        })
    }
    fn no_messages(task_type: TaskType, task_id: &str) -> Arc<Self> {
        Arc::new(Self {
            task_type,
            task_id: task_id.to_string(),
            msgs: StdMutex::new(Vec::new()),
            supports: false,
            terminate: false,
        })
    }
    fn terminating(task_type: TaskType, task_id: &str) -> Arc<Self> {
        Arc::new(Self {
            task_type,
            task_id: task_id.to_string(),
            msgs: StdMutex::new(Vec::new()),
            supports: true,
            terminate: true,
        })
    }
    fn received(&self) -> Vec<(String, String)> {
        self.msgs.lock().unwrap().clone()
    }
}
#[async_trait]
impl Task for MsgRecordingHandler {
    fn name(&self) -> &str {
        "msg-recording"
    }
    fn task_type(&self) -> TaskType {
        self.task_type
    }
    async fn spawn(
        &self,
        _input: TaskSpawnInput,
        _ctx: TaskContext,
    ) -> Result<TaskHandle, TaskError> {
        Ok(TaskHandle {
            task_id: self.task_id.clone(),
            cleanup: None,
        })
    }
    async fn kill(&self, _task_id: &str, _ctx: TaskContext) -> Result<(), TaskError> {
        Ok(())
    }
    fn supports_messages(&self) -> bool {
        self.supports
    }
    async fn send_message(
        &self,
        task_id: &str,
        message: String,
        _ctx: TaskContext,
    ) -> Result<(), TaskError> {
        if self.terminate {
            return Err(TaskError::TerminatedTask);
        }
        self.msgs
            .lock()
            .unwrap()
            .push((task_id.to_string(), message));
        Ok(())
    }
}

#[tokio::test]
async fn seam_send_message_routes_to_recording_handler() {
    use traits::team_spawn::TeamSpawnSeam;

    let (_d, mut registry) = make_registry();
    let handler = MsgRecordingHandler::new(TaskType::InProcessTeammate, "tmsgid");
    registry.register_handler(TaskType::InProcessTeammate, handler.clone());

    // Spawn so the task is recorded in the spawned-id index (the same index
    // `send_message` resolves the handler through).
    let seam: &dyn TeamSpawnSeam = &registry;
    let task_id = seam
        .spawn_teammate(
            protocol::AgentId::new(),
            "buddy".into(),
            "alpha".into(),
            "a teammate".into(),
        )
        .await
        .unwrap();
    assert_eq!(task_id, "tmsgid");

    seam.send_message(&task_id, "do the thing".into())
        .await
        .expect("send_message routes to the supporting handler");

    assert_eq!(
        handler.received(),
        vec![("tmsgid".to_string(), "do the thing".to_string())],
        "the registry override dispatched to the handler's send_message"
    );
}

#[tokio::test]
async fn seam_send_message_unknown_task_is_terminated() {
    use traits::team_spawn::{TeamSpawnError, TeamSpawnSeam};

    let (_d, registry) = make_registry();
    let seam: &dyn TeamSpawnSeam = &registry;
    // Nothing spawned ⇒ the id is not in the spawned-id index.
    let err = seam.send_message("nope", "hi".into()).await.unwrap_err();
    assert!(
        matches!(err, TeamSpawnError::Terminated),
        "a non-existent task maps to Terminated; got {err:?}"
    );
}

#[tokio::test]
async fn seam_send_message_terminated_handler_maps_to_terminated() {
    use traits::team_spawn::{TeamSpawnError, TeamSpawnSeam};

    let (_d, mut registry) = make_registry();
    let handler = MsgRecordingHandler::terminating(TaskType::InProcessTeammate, "tgone");
    registry.register_handler(TaskType::InProcessTeammate, handler);

    let seam: &dyn TeamSpawnSeam = &registry;
    let task_id = seam
        .spawn_teammate(
            protocol::AgentId::new(),
            "buddy".into(),
            "alpha".into(),
            "x".into(),
        )
        .await
        .unwrap();

    let err = seam.send_message(&task_id, "hi".into()).await.unwrap_err();
    assert!(
        matches!(err, TeamSpawnError::Terminated),
        "a handler that reports TerminatedTask maps to Terminated; got {err:?}"
    );
}

#[tokio::test]
async fn seam_send_message_unsupporting_handler_is_unsupported() {
    use traits::team_spawn::{TeamSpawnError, TeamSpawnSeam};

    let (_d, mut registry) = make_registry();
    let handler = MsgRecordingHandler::no_messages(TaskType::InProcessTeammate, "tnomsg");
    registry.register_handler(TaskType::InProcessTeammate, handler);

    let seam: &dyn TeamSpawnSeam = &registry;
    let task_id = seam
        .spawn_teammate(
            protocol::AgentId::new(),
            "buddy".into(),
            "alpha".into(),
            "x".into(),
        )
        .await
        .unwrap();

    let err = seam.send_message(&task_id, "hi".into()).await.unwrap_err();
    assert!(
        matches!(err, TeamSpawnError::Unsupported(_)),
        "a handler that does not support messages maps to Unsupported; got {err:?}"
    );
}

// ---- TaskCompleted hook firer seam -------------------------------------
//
// Mirrors the `subagent_stop` / `stop_hooks` test patterns: a registered
// firer receives the byte-faithful fire when a task reaches a terminal
// status (completed + failed); a registry with NO firer is a strict no-op.

/// A fake [`TaskCompletedFirer`] that records every fire it receives.
struct RecordingFirer {
    fires: StdMutex<Vec<hooks::TaskCompletedFire>>,
}
impl RecordingFirer {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            fires: StdMutex::new(Vec::new()),
        })
    }
    fn recorded(&self) -> Vec<hooks::TaskCompletedFire> {
        self.fires.lock().unwrap().clone()
    }
}
#[async_trait]
impl hooks::TaskCompletedFirer for RecordingFirer {
    async fn fire(&self, fire: hooks::TaskCompletedFire) {
        self.fires.lock().unwrap().push(fire);
    }
}

/// A [`TaskCompletedFirer`] whose `fire` itself does nothing observable —
/// stand-in for a firer that swallows a failing hook. Proves the registry's
/// status transition succeeds regardless of what the firer does.
struct SwallowingFirer;
#[async_trait]
impl hooks::TaskCompletedFirer for SwallowingFirer {
    async fn fire(&self, _fire: hooks::TaskCompletedFire) {}
}

/// Build a registry with a `RecordingFirer` and seed a single `LocalBash`
/// task with a known description, returning the firer + task id.
async fn registry_with_firer(
    description: &str,
) -> (tempfile::TempDir, TaskRegistry, Arc<RecordingFirer>, String) {
    let dir = tempdir().unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
    let runtime = Arc::new(MockRuntimeSpawner::default());
    let out_mgr = Arc::new(crate::output_manager::TaskOutputManager::new(
        PathBuf::from(dir.path()),
        fs.clone(),
    ));
    let firer = RecordingFirer::new();
    let registry = TaskRegistry::new(runtime, fs, out_mgr).with_task_completed_firer(firer.clone());
    let task_id = registry
        .create(
            TaskType::LocalBash,
            teammate_input(),
            description.to_string(),
        )
        .await
        .unwrap();
    (dir, registry, firer, task_id)
}

#[tokio::test]
async fn completed_transition_fires_byte_faithful_payload() {
    let (_d, registry, firer, task_id) = registry_with_firer("ship the parity port").await;

    let updated = registry
        .set_status(&task_id, TaskStatus::Completed)
        .await
        .unwrap();
    assert_eq!(updated.base().status, TaskStatus::Completed);

    let recorded = firer.recorded();
    assert_eq!(
        recorded.len(),
        1,
        "exactly one TaskCompleted fire: {recorded:?}"
    );
    let f = &recorded[0];
    assert_eq!(f.task_id, task_id);
    assert_eq!(f.status, "completed");
    // Wire payload (`TaskCompletedHookInputSchema`): subject + description
    // both source from the task description (no distinct subject field).
    assert_eq!(f.task_subject, "ship the parity port");
    assert_eq!(f.task_description.as_deref(), Some("ship the parity port"));
    // teammate/team are not stored on the M-surface task state => None.
    assert_eq!(f.teammate_name, None);
    assert_eq!(f.team_name, None);
}

#[tokio::test]
async fn failed_transition_also_fires() {
    // claude-code also fires `executeTaskCompletedHooks` from `stopHooks.ts`
    // when a teammate stops with in-progress tasks — the terminal transition
    // must fire on `Failed`, not just `Completed`.
    let (_d, registry, firer, task_id) = registry_with_firer("do the thing").await;

    registry
        .set_status(&task_id, TaskStatus::Failed)
        .await
        .unwrap();

    let recorded = firer.recorded();
    assert_eq!(
        recorded.len(),
        1,
        "a Failed transition fires TaskCompleted: {recorded:?}"
    );
    assert_eq!(recorded[0].status, "failed");
    assert_eq!(recorded[0].task_subject, "do the thing");
}

#[tokio::test]
async fn non_terminal_and_killed_transitions_do_not_fire() {
    let (_d, registry, firer, task_id) = registry_with_firer("x").await;

    // Running is non-terminal => no fire.
    registry
        .set_status(&task_id, TaskStatus::Running)
        .await
        .unwrap();
    // Killed is terminal but has no claude-code `executeTaskCompletedHooks`
    // counterpart => no fire.
    registry
        .set_status(&task_id, TaskStatus::Killed)
        .await
        .unwrap();

    assert!(
        firer.recorded().is_empty(),
        "neither Running nor Killed fires TaskCompleted: {:?}",
        firer.recorded()
    );
}

#[tokio::test]
async fn no_firer_registered_is_a_noop() {
    // The default registry holds no firer: a terminal transition must still
    // succeed and simply not fire anything (the strict no-op contract).
    let (_d, registry) = make_registry();
    let task_id = registry
        .create(TaskType::LocalBash, teammate_input(), "no firer".into())
        .await
        .unwrap();

    let updated = registry
        .set_status(&task_id, TaskStatus::Completed)
        .await
        .expect("set_status succeeds with no firer registered");
    assert_eq!(updated.base().status, TaskStatus::Completed);
}

#[tokio::test]
async fn firer_that_swallows_does_not_break_transition() {
    // Best-effort contract: whatever the firer does, the status transition
    // succeeds (the firer is responsible for swallowing hook failures).
    let dir = tempdir().unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
    let runtime = Arc::new(MockRuntimeSpawner::default());
    let out_mgr = Arc::new(crate::output_manager::TaskOutputManager::new(
        PathBuf::from(dir.path()),
        fs.clone(),
    ));
    let registry = TaskRegistry::new(runtime, fs, out_mgr)
        .with_task_completed_firer(Arc::new(SwallowingFirer));
    let task_id = registry
        .create(TaskType::LocalBash, teammate_input(), "swallow".into())
        .await
        .unwrap();

    let updated = registry
        .set_status(&task_id, TaskStatus::Completed)
        .await
        .expect("transition succeeds even though the firer is a black hole");
    assert_eq!(updated.base().status, TaskStatus::Completed);
}

// ---- TaskCreated hook firer seam ---------------------------------------
//
// Counterpart to the `TaskCompleted` tests above: a registered firer
// receives the byte-faithful fire when a task is created (both the `create`
// placeholder path and the `spawn` production path); a registry with NO
// firer is a strict no-op.

/// A fake [`TaskCreatedFirer`] that records every fire it receives.
struct RecordingCreatedFirer {
    fires: StdMutex<Vec<hooks::TaskCreatedFire>>,
}
impl RecordingCreatedFirer {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            fires: StdMutex::new(Vec::new()),
        })
    }
    fn recorded(&self) -> Vec<hooks::TaskCreatedFire> {
        self.fires.lock().unwrap().clone()
    }
}
#[async_trait]
impl hooks::TaskCreatedFirer for RecordingCreatedFirer {
    async fn fire(&self, fire: hooks::TaskCreatedFire) {
        self.fires.lock().unwrap().push(fire);
    }
}

fn registry_with_created_firer() -> (tempfile::TempDir, TaskRegistry, Arc<RecordingCreatedFirer>) {
    let dir = tempdir().unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
    let runtime = Arc::new(MockRuntimeSpawner::default());
    let out_mgr = Arc::new(crate::output_manager::TaskOutputManager::new(
        PathBuf::from(dir.path()),
        fs.clone(),
    ));
    let firer = RecordingCreatedFirer::new();
    let registry = TaskRegistry::new(runtime, fs, out_mgr).with_task_created_firer(firer.clone());
    (dir, registry, firer)
}

#[tokio::test]
async fn create_fires_byte_faithful_task_created_payload() {
    let (_d, registry, firer) = registry_with_created_firer();

    let task_id = registry
        .create(TaskType::LocalBash, teammate_input(), "do the work".into())
        .await
        .unwrap();

    let recorded = firer.recorded();
    assert_eq!(
        recorded.len(),
        1,
        "exactly one TaskCreated fire: {recorded:?}"
    );
    let f = &recorded[0];
    assert_eq!(f.task_id, task_id);
    // Wire payload (`TaskCreatedHookInputSchema`): subject sources from the
    // task-type taxonomy bucket; description from the create description.
    assert_eq!(f.task_subject, "LocalBash");
    assert_eq!(f.task_description.as_deref(), Some("do the work"));
    // teammate/team are not stored on the M-surface task state => None.
    assert_eq!(f.teammate_name, None);
    assert_eq!(f.team_name, None);
}

#[tokio::test]
async fn spawn_also_fires_task_created() {
    // The production task-creation path (`spawn`) inserts a new task row, so
    // it fires `TaskCreated` too — not just the `create` placeholder path.
    let (_d, mut registry, firer) = registry_with_created_firer();
    let handler = RecordingHandler::new(TaskType::InProcessTeammate, "tspawnhook");
    registry.register_handler(TaskType::InProcessTeammate, handler);

    let task_id = registry
        .spawn(
            TaskType::InProcessTeammate,
            teammate_input(),
            "a teammate".into(),
        )
        .await
        .unwrap();

    let recorded = firer.recorded();
    assert_eq!(recorded.len(), 1, "spawn fires TaskCreated: {recorded:?}");
    assert_eq!(recorded[0].task_id, task_id);
    assert_eq!(recorded[0].task_subject, "InProcessTeammate");
    assert_eq!(recorded[0].task_description.as_deref(), Some("a teammate"));
}

#[tokio::test]
async fn no_created_firer_registered_is_a_noop() {
    // The default registry holds no firer: creating a task must still
    // succeed and simply not fire anything (the strict no-op contract).
    let (_d, registry) = make_registry();
    let task_id = registry
        .create(TaskType::LocalBash, teammate_input(), "no firer".into())
        .await
        .expect("create succeeds with no firer registered");
    assert!(!task_id.is_empty());
}

// ---- T4: spawn does NOT re-allocate the handler's spool ----------------
//
// An exclusive-create FS (O_EXCL semantics) + a handler that allocates its
// own spool and appends output. `registry.spawn` must consume that spool via
// `path_for` (no second `allocate`), so the worker's output survives and the
// exclusive create fires exactly once.

use std::sync::atomic::{AtomicUsize as Au, Ordering as Ord2};

/// In-memory FS with REAL exclusive-create semantics + a create counter.
struct ExclusiveCountingFs {
    files: tokio::sync::Mutex<HashMap<String, String>>,
    creates: Au,
}
impl ExclusiveCountingFs {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            files: tokio::sync::Mutex::new(HashMap::new()),
            creates: Au::new(0),
        })
    }
}
#[async_trait]
impl FileSystem for ExclusiveCountingFs {
    async fn read_file(
        &self,
        path: &str,
        _o: Option<u64>,
        _l: Option<u64>,
    ) -> Result<traits::filesystem::FileContent, traits::filesystem::FsError> {
        let map = self.files.lock().await;
        let content = map.get(path).cloned().unwrap_or_default();
        let total_lines = content.lines().count() as u64;
        Ok(traits::filesystem::FileContent {
            content,
            truncated: false,
            total_lines,
        })
    }
    async fn write_file(&self, path: &str, body: &str) -> Result<(), traits::filesystem::FsError> {
        self.files
            .lock()
            .await
            .insert(path.to_string(), body.to_string());
        Ok(())
    }
    async fn create_new_file(&self, path: &str) -> Result<(), traits::filesystem::FsError> {
        self.creates.fetch_add(1, Ord2::SeqCst);
        let mut map = self.files.lock().await;
        if map.contains_key(path) {
            return Err(traits::filesystem::FsError::AlreadyExists(path.to_string()));
        }
        map.insert(path.to_string(), String::new());
        Ok(())
    }
    fn is_within_workspace(&self, _: &str) -> bool {
        true
    }
    async fn watch(
        &self,
        _: &str,
    ) -> Result<
        std::pin::Pin<Box<dyn futures::Stream<Item = traits::filesystem::FileEvent> + Send>>,
        traits::filesystem::FsError,
    > {
        Err(traits::filesystem::FsError::Io("nope".into()))
    }
    async fn append_file(&self, path: &str, body: &str) -> Result<(), traits::filesystem::FsError> {
        self.files
            .lock()
            .await
            .entry(path.to_string())
            .or_default()
            .push_str(body);
        Ok(())
    }
    async fn truncate(&self, _: &str, _: u64) -> Result<(), traits::filesystem::FsError> {
        Ok(())
    }
    async fn file_mtime(
        &self,
        _: &str,
    ) -> Result<std::time::SystemTime, traits::filesystem::FsError> {
        Ok(std::time::SystemTime::UNIX_EPOCH)
    }
    async fn file_size(&self, path: &str) -> Result<u64, traits::filesystem::FsError> {
        let map = self.files.lock().await;
        Ok(map.get(path).map_or(0, |s| s.len() as u64))
    }
    async fn delete_file(&self, path: &str) -> Result<(), traits::filesystem::FsError> {
        self.files.lock().await.remove(path);
        Ok(())
    }
    async fn symlink(&self, _: &str, _: &str) -> Result<(), traits::filesystem::FsError> {
        Ok(())
    }
    async fn flock_exclusive(
        &self,
        _: &str,
    ) -> Result<Box<dyn traits::filesystem::FlockGuard>, traits::filesystem::FsError> {
        Err(traits::filesystem::FsError::Io("nope".into()))
    }
    async fn fsync(&self, _: &str) -> Result<(), traits::filesystem::FsError> {
        Ok(())
    }
}

/// A handler that allocates its OWN spool through the shared output manager
/// (like the real handlers) and appends a known line, then hands back a
/// fixed id — so a registry re-allocate would either collide (O_EXCL) or
/// truncate the appended bytes.
struct AllocatingHandler {
    task_type: TaskType,
    task_id: String,
    out: Arc<crate::output_manager::TaskOutputManager>,
}
#[async_trait]
impl crate::task_trait::Task for AllocatingHandler {
    fn name(&self) -> &str {
        "allocating"
    }
    fn task_type(&self) -> TaskType {
        self.task_type
    }
    async fn spawn(
        &self,
        _input: TaskSpawnInput,
        _ctx: crate::task_trait::TaskContext,
    ) -> Result<crate::task_trait::TaskHandle, TaskError> {
        // Allocate the spool ONCE and write output the worker would produce.
        let path = self
            .out
            .allocate(&self.task_id)
            .await
            .map_err(|e| TaskError::Io(e.to_string()))?;
        self.out
            .fs_for_test()
            .append_file_no_follow(path.to_str().unwrap(), "spawned worker output\n")
            .await
            .map_err(|e| TaskError::Io(e.to_string()))?;
        Ok(crate::task_trait::TaskHandle {
            task_id: self.task_id.clone(),
            cleanup: None,
        })
    }
    async fn kill(
        &self,
        _task_id: &str,
        _ctx: crate::task_trait::TaskContext,
    ) -> Result<(), TaskError> {
        Ok(())
    }
}

#[tokio::test]
async fn spawn_does_not_reallocate_and_worker_output_survives() {
    let fs = ExclusiveCountingFs::new();
    let runtime = Arc::new(MockRuntimeSpawner::default());
    let out = Arc::new(crate::output_manager::TaskOutputManager::new(
        PathBuf::from("/spool"),
        fs.clone(),
    ));
    let mut registry = TaskRegistry::new(runtime, fs.clone(), out.clone());
    registry.register_handler(
        TaskType::InProcessTeammate,
        Arc::new(AllocatingHandler {
            task_type: TaskType::InProcessTeammate,
            task_id: "tspool1".into(),
            out: out.clone(),
        }),
    );

    let id = registry
        .spawn(TaskType::InProcessTeammate, teammate_input(), "x".into())
        .await
        .expect("spawn must succeed WITHOUT a second exclusive allocate");
    assert_eq!(id, "tspool1");

    // The handler allocated EXACTLY once; the registry must not re-create.
    assert_eq!(
        fs.creates.load(Ord2::SeqCst),
        1,
        "registry.spawn must consume the handler's spool, not re-allocate it"
    );

    // The worker's output is intact (no truncation by a second allocate).
    let path = out.path_for(&id).unwrap();
    let read = out
        .read(&path, crate::output_manager::OutputOptions::default())
        .await
        .unwrap();
    assert!(
        read.content.contains("spawned worker output"),
        "the spawned worker's appended output survived; got {:?}",
        read.content
    );

    // And the registry recorded the task under the handler id.
    assert!(registry.get(&id).await.is_some());
}

#[tokio::test]
async fn spawned_agent_aliases_resolve_to_task_id_and_kill_routes() {
    let (_d, mut registry) = make_registry();
    let handler = RecordingHandler::new(TaskType::InProcessTeammate, "thandlerid");
    registry.register_handler(TaskType::InProcessTeammate, handler.clone());

    let agent_id = protocol::AgentId::new();
    let id = registry
        .spawn(
            TaskType::InProcessTeammate,
            TaskSpawnInput::InProcessTeammate {
                agent_id,
                name: "buddy".into(),
                team_name: "alpha".into(),
                description: "work".into(),
            },
            "a teammate".into(),
        )
        .await
        .unwrap();

    assert_eq!(id, "thandlerid");
    assert_eq!(
        registry.get(&agent_id.to_string()).await.unwrap().base().id,
        "thandlerid"
    );
    assert_eq!(registry.get("buddy").await.unwrap().base().id, "thandlerid");
    assert_eq!(
        registry.get("buddy@alpha").await.unwrap().base().id,
        "thandlerid"
    );

    registry.kill("buddy").await.unwrap();
    assert_eq!(handler.killed_ids(), vec!["thandlerid".to_string()]);
}

#[tokio::test]
async fn spawned_task_cleanup_hook_is_preserved_and_run_on_kill() {
    let (_d, mut registry) = make_registry();
    let cleanup_count = Arc::new(AtomicUsize::new(0));
    let handler = RecordingHandler::with_cleanup_counter(
        TaskType::InProcessTeammate,
        "thandlerid",
        cleanup_count.clone(),
    );
    registry.register_handler(TaskType::InProcessTeammate, handler);

    registry
        .spawn(
            TaskType::InProcessTeammate,
            teammate_input(),
            "a teammate".into(),
        )
        .await
        .unwrap();
    registry.kill("thandlerid").await.unwrap();

    assert_eq!(
        cleanup_count.load(Ordering::SeqCst),
        1,
        "registry must retain and invoke TaskHandle cleanup"
    );
}

// ---- T9 / T35: mark_notified + terminal-task retention ------------------

#[tokio::test]
async fn mark_notified_on_terminal_task_retains_it() {
    // 2.1.208 keeps completed background tasks available until explicit cleanup.
    let (_d, registry) = make_registry();
    let id = registry
        .create(TaskType::LocalBash, teammate_input(), "x".into())
        .await
        .unwrap();
    // Drive it terminal first.
    registry
        .force_bash_terminal_for_test(&id, TaskStatus::Completed, Some(0))
        .await;

    registry.mark_notified(&id).await.unwrap();

    let state = registry.get(&id).await.expect("terminal task is retained");
    assert!(state.base().notified, "terminal task is marked notified");
}

#[tokio::test]
async fn mark_notified_on_running_task_keeps_it() {
    // A non-terminal (running/pending) task keeps the flag and stays in the
    // map — only terminal tasks are GC'd.
    let (_d, registry) = make_registry();
    let id = registry
        .create(TaskType::LocalBash, teammate_input(), "x".into())
        .await
        .unwrap();
    // Default created status is Pending (non-terminal).
    registry.mark_notified(&id).await.unwrap();

    let state = registry.get(&id).await.expect("non-terminal task survives");
    assert!(
        state.base().notified,
        "the notified flag is set even when kept"
    );
}

#[tokio::test]
async fn mark_notified_unknown_id_is_not_found() {
    let (_d, registry) = make_registry();
    let err = registry.mark_notified("nope").await.unwrap_err();
    assert!(matches!(err, TaskError::NotFound(_)), "got {err:?}");
}

#[tokio::test]
async fn evict_terminal_tasks_sweeps_terminal_notified_only() {
    // Terminal tasks are retained after notification; this compatibility method
    // no longer performs implicit GC.
    let (_d, registry) = make_registry();

    // Task A: notified while pending, THEN driven terminal — eager evict did
    // not fire (still pending then), so the sweep must catch it.
    let a = registry
        .create(TaskType::LocalBash, teammate_input(), "a".into())
        .await
        .unwrap();
    registry.mark_notified(&a).await.unwrap(); // pending ⇒ kept, flag set
    registry
        .force_bash_terminal_for_test(&a, TaskStatus::Completed, Some(0))
        .await;

    // Task B: terminal but NOT notified — must survive the sweep.
    let b = registry
        .create(TaskType::LocalBash, teammate_input(), "b".into())
        .await
        .unwrap();
    registry
        .force_bash_terminal_for_test(&b, TaskStatus::Failed, Some(1))
        .await;

    // Task C: notified but still pending (non-terminal) — must survive.
    let c = registry
        .create(TaskType::LocalBash, teammate_input(), "c".into())
        .await
        .unwrap();
    registry.mark_notified(&c).await.unwrap();

    let evicted = registry.evict_terminal_tasks().await;
    assert!(evicted.is_empty(), "implicit terminal-task GC is disabled");
    assert!(
        registry.get(&a).await.is_some(),
        "terminal+notified is retained"
    );
    assert!(
        registry.get(&b).await.is_some(),
        "terminal but un-notified survives"
    );
    assert!(
        registry.get(&c).await.is_some(),
        "notified but non-terminal survives"
    );
}

// ---- T35: take_pending_task_notifications drain --------------------------

#[tokio::test]
async fn take_pending_drains_terminal_bash_once_with_exit_code() {
    let (_d, registry) = make_registry();
    let id = registry
        .create(TaskType::LocalBash, teammate_input(), "run tests".into())
        .await
        .unwrap();
    registry
        .force_bash_terminal_for_test(&id, TaskStatus::Completed, Some(0))
        .await;

    // Drain 1: exactly one notification carrying the bash fields.
    let drained = registry.take_pending_task_notifications().await;
    assert_eq!(drained.len(), 1, "one terminal task ⇒ one notification");
    let n = &drained[0];
    assert_eq!(n.task_id, id);
    assert_eq!(n.task_type, "local_bash");
    assert_eq!(n.status, "completed");
    assert_eq!(n.description, "run tests");
    assert_eq!(n.exit_code, Some(0), "local_bash carries its exit_code");
    assert!(n.error.is_none());
    assert!(
        n.output_path
            .as_deref()
            .is_some_and(|p| p.ends_with(&format!("{id}.output"))),
        "output_path is the spool path: {:?}",
        n.output_path
    );

    let state = registry.get(&id).await.expect("drained task is retained");
    assert!(state.base().notified, "drained task is marked notified");

    // Drain 2: consume-once — nothing left.
    assert!(
        registry.take_pending_task_notifications().await.is_empty(),
        "a drained completion is not reported a second time"
    );
}

#[test]
fn state_for_spawn_stamps_local_agent_tool_use_id() {
    // A backgrounded LocalAgent carries the originating `tool_use_id` onto its
    // `TaskStateBase`, so its `<task-notification>` later renders the
    // `<tool-use-id>` line (claude-code parity). The caller passes a base with
    // `tool_use_id: None`; `state_for_spawn` stamps it from the input.
    let base = TaskStateBase {
        id: "abg01".into(),
        task_type: TaskType::LocalAgent,
        status: TaskStatus::Running,
        description: "research".into(),
        tool_use_id: None,
        start_time: SystemTime::now(),
        end_time: None,
        total_paused_ms: 0,
        output_file: std::path::PathBuf::from("/tmp/tasks/abg01.output"),
        output_offset: 0,
        notified: false,
    };
    let input = TaskSpawnInput::LocalAgent {
        agent_id: protocol::AgentId::nil(),
        subagent_type: "general-purpose".into(),
        prompt: "go".into(),
        is_backgrounded: true,
        tool_use_id: Some("toolu_bg42".into()),
        spawn_request: None,
        inheritance: None,
    };
    let state = state_for_spawn(base, &input);
    assert_eq!(state.base().tool_use_id.as_deref(), Some("toolu_bg42"));

    // A `None` input tool_use_id leaves the base untouched.
    let base2 = TaskStateBase {
        id: "abg02".into(),
        task_type: TaskType::LocalAgent,
        status: TaskStatus::Running,
        description: "x".into(),
        tool_use_id: None,
        start_time: SystemTime::now(),
        end_time: None,
        total_paused_ms: 0,
        output_file: std::path::PathBuf::from("/tmp/tasks/abg02.output"),
        output_offset: 0,
        notified: false,
    };
    let input2 = TaskSpawnInput::LocalAgent {
        agent_id: protocol::AgentId::nil(),
        subagent_type: "general-purpose".into(),
        prompt: "go".into(),
        is_backgrounded: true,
        tool_use_id: None,
        spawn_request: None,
        inheritance: None,
    };
    assert_eq!(state_for_spawn(base2, &input2).base().tool_use_id, None);
}

#[tokio::test]
async fn take_pending_carries_agent_error() {
    use crate::state::{LocalAgentTaskState, TaskState, TaskStateBase};
    let (_d, registry) = make_registry();
    // Build a terminal (failed) LocalAgent with an error message.
    let base = TaskStateBase {
        id: "afailed01".into(),
        task_type: TaskType::LocalAgent,
        status: TaskStatus::Failed,
        description: "research".into(),
        tool_use_id: Some("toolu_7".into()),
        start_time: SystemTime::now(),
        end_time: None,
        total_paused_ms: 0,
        output_file: std::path::PathBuf::from("/tmp/tasks/afailed01.output"),
        output_offset: 0,
        notified: false,
    };
    registry
        .insert_state_for_test(TaskState::LocalAgent(LocalAgentTaskState {
            base,
            agent_id: protocol::AgentId::nil(),
            subagent_type: String::new(),
            prompt: String::new(),
            error: Some("rate limited".into()),
            messages: vec![],
            pending_messages: vec![],
            is_backgrounded: true,
        }))
        .await;

    let drained = registry.take_pending_task_notifications().await;
    assert_eq!(drained.len(), 1);
    let n = &drained[0];
    assert_eq!(n.task_type, "local_agent");
    assert_eq!(n.status, "failed");
    assert_eq!(n.error.as_deref(), Some("rate limited"));
    assert_eq!(n.tool_use_id.as_deref(), Some("toolu_7"));
    assert!(n.exit_code.is_none(), "agent tasks have no exit_code");
}

#[tokio::test]
async fn find_running_workflow_by_run_id_matches_only_running_same_id() {
    use crate::state::{LocalWorkflowTaskState, TaskState, TaskStateBase};
    let (_d, registry) = make_registry();

    let mk = |id: &str, status: TaskStatus, run_id: Option<&str>| {
        TaskState::LocalWorkflow(LocalWorkflowTaskState {
            base: TaskStateBase {
                id: id.into(),
                task_type: TaskType::LocalWorkflow,
                status,
                description: "wf".into(),
                tool_use_id: None,
                start_time: SystemTime::now(),
                end_time: None,
                total_paused_ms: 0,
                output_file: std::path::PathBuf::from(format!("/tmp/tasks/{id}.output")),
                output_offset: 0,
                notified: false,
            },
            workflow_id: String::new(),
            script: String::new(),
            resume_from_run_id: None,
            args: None,
            run_id: run_id.map(str::to_string),
            current_step: 0,
        })
    };

    registry
        .insert_state_for_test(mk("w-run", TaskStatus::Running, Some("wf_aaa")))
        .await;
    registry
        .insert_state_for_test(mk("w-done", TaskStatus::Completed, Some("wf_bbb")))
        .await;

    // A running workflow with the matching run id is found (resume blocked).
    assert_eq!(
        registry
            .find_running_workflow_by_run_id("wf_aaa")
            .await
            .as_deref(),
        Some("w-run")
    );
    // A completed workflow with that id is NOT found (resume allowed).
    assert_eq!(
        registry.find_running_workflow_by_run_id("wf_bbb").await,
        None
    );
    // Unknown id → None.
    assert_eq!(
        registry.find_running_workflow_by_run_id("wf_zzz").await,
        None
    );
}

#[tokio::test]
async fn rested_agent_surfaces_once_per_rest_without_eviction() {
    use crate::state::{LocalAgentTaskState, TaskState, TaskStateBase};
    let (_d, registry) = make_registry();

    // A PERSISTENT (backgrounded) agent that came to rest: NON-terminal.
    let base = TaskStateBase {
        id: "a-rest-1".into(),
        task_type: TaskType::LocalAgent,
        status: TaskStatus::Running,
        description: "bg agent".into(),
        tool_use_id: Some("toolu_r".into()),
        start_time: SystemTime::now(),
        end_time: None,
        total_paused_ms: 0,
        output_file: std::path::PathBuf::from("/tmp/tasks/a-rest-1.output"),
        output_offset: 0,
        notified: false,
    };
    registry
        .insert_state_for_test(TaskState::LocalAgent(LocalAgentTaskState {
            base,
            agent_id: protocol::AgentId::nil(),
            subagent_type: String::new(),
            prompt: String::new(),
            error: None,
            messages: vec![],
            pending_messages: vec![],
            is_backgrounded: true,
        }))
        .await;

    // No rest armed yet ⇒ a Running task surfaces NOTHING.
    assert!(
        registry.take_pending_task_notifications().await.is_empty(),
        "a running-but-not-rested agent is not notified"
    );

    // Came to rest ⇒ exactly one notification, NON-terminal, NOT evicted.
    registry
        .mark_task_rested(
            "a-rest-1",
            Some("final answer".to_string()),
            Some(traits::task_registry::AgentRunUsage {
                subagent_tokens: 42,
                tool_uses: 3,
                duration_ms: 1500,
            }),
        )
        .await;
    let drained = registry.take_pending_task_notifications().await;
    assert_eq!(drained.len(), 1, "one rest notification");
    assert_eq!(drained[0].task_id, "a-rest-1");
    // DISPLAY status is "completed" so the renderer says "came to rest"
    // (NOT "(stopped by user)"); the task itself stays Running (alive).
    assert_eq!(drained[0].status, "completed", "renders as 'came to rest'");
    // The result text + usage are carried into the optional sections.
    assert_eq!(drained[0].result.as_deref(), Some("final answer"));
    assert_eq!(
        drained[0].usage.as_ref().map(|u| u.subagent_tokens),
        Some(42),
        "usage carried for the <usage> section"
    );
    assert!(
        registry.get("a-rest-1").await.is_some(),
        "the live task stays Running despite the 'completed' display status"
    );
    assert_eq!(
        drained[0].output_path.as_deref(),
        Some("/tmp/tasks/a-rest-1.output"),
        "spool path carried so the model can read the result"
    );
    assert!(
        registry.get("a-rest-1").await.is_some(),
        "a resting agent is NOT evicted — it stays alive for the next message"
    );

    // The arm is one-shot: a second drain (no new rest) is empty.
    assert!(
        registry.take_pending_task_notifications().await.is_empty(),
        "the rest notification fires exactly once until re-armed"
    );

    // Re-armable: the NEXT rest surfaces again (same task-id notifies > once).
    registry.mark_task_rested("a-rest-1", None, None).await;
    assert_eq!(
        registry.take_pending_task_notifications().await.len(),
        1,
        "each subsequent rest re-arms the notification"
    );
}

#[tokio::test]
async fn take_pending_skips_already_notified_and_non_terminal() {
    let (_d, registry) = make_registry();

    // A: terminal but ALREADY notified.
    {
        use crate::state::{LocalBashTaskState, TaskState, TaskStateBase};
        let base = TaskStateBase {
            id: "bnotified".into(),
            task_type: TaskType::LocalBash,
            status: TaskStatus::Completed,
            description: "seen".into(),
            tool_use_id: None,
            start_time: SystemTime::now(),
            end_time: None,
            total_paused_ms: 0,
            output_file: std::path::PathBuf::from("/tmp/tasks/bnotified.output"),
            output_offset: 0,
            notified: true, // already surfaced (e.g. via TaskOutput)
        };
        registry
            .insert_state_for_test(TaskState::LocalBash(LocalBashTaskState {
                base,
                command: String::new(),
                pid: None,
                exit_code: Some(0),
            }))
            .await;
    }

    // B: still pending (non-terminal).
    let pending = registry
        .create(TaskType::LocalBash, teammate_input(), "pending".into())
        .await
        .unwrap();

    // C: terminal and un-notified — the only one that should drain.
    let fresh = registry
        .create(TaskType::LocalBash, teammate_input(), "fresh".into())
        .await
        .unwrap();
    registry
        .force_bash_terminal_for_test(&fresh, TaskStatus::Completed, Some(0))
        .await;

    let drained = registry.take_pending_task_notifications().await;
    assert_eq!(
        drained.len(),
        1,
        "only the terminal+un-notified task drains"
    );
    assert_eq!(drained[0].task_id, fresh);

    // The already-notified task and the pending task both survive untouched.
    assert!(
        registry.get("bnotified").await.is_some(),
        "already-notified survives"
    );
    assert!(registry.get(&pending).await.is_some(), "pending survives");
}

// ---- M8 cc2.1.198: "Task panels: no stuck Running after finish" -----------

/// Minimal happy-path [`ProcessRunner`]: every `run()` succeeds with exit 0.
struct ExitZeroRunner;

#[async_trait]
impl traits::ProcessRunner for ExitZeroRunner {
    async fn run(
        &self,
        _cmd: &traits::SandboxedCommand,
    ) -> Result<traits::ProcessOutput, traits::ProcessError> {
        Ok(traits::ProcessOutput {
            stdout: "done\n".into(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        })
    }
    async fn spawn_background(
        &self,
        _cmd: &traits::SandboxedCommand,
    ) -> Result<traits::ProcessHandle, traits::ProcessError> {
        Err(traits::ProcessError::Unsupported)
    }
    async fn kill(&self, _handle: &traits::ProcessHandle) -> Result<(), traits::ProcessError> {
        Ok(())
    }
    fn is_available(&self) -> bool {
        true
    }
}

/// Pass-through [`traits::Sandbox`] stub (audited bypass tag, like the
/// local_bash unit tests').
struct PassSandbox;

#[async_trait]
impl traits::Sandbox for PassSandbox {
    fn is_available(&self) -> bool {
        true
    }
    fn backend(&self) -> traits::SandboxBackend {
        traits::SandboxBackend::None
    }
    fn prepare(
        &self,
        cmd: traits::ProcessCommand,
        _policy: &traits::SandboxPolicy,
    ) -> Result<traits::SandboxedCommand, traits::SandboxError> {
        Ok(traits::SandboxedCommand::__new_sandboxed(
            cmd,
            traits::SandboxedTag::BypassAuditedWithReason {
                reason: "test".into(),
            },
        ))
    }
    fn bypass_with_audit(
        &self,
        cmd: traits::ProcessCommand,
        reason: &str,
    ) -> traits::SandboxedCommand {
        traits::SandboxedCommand::__new_sandboxed(
            cmd,
            traits::SandboxedTag::BypassAuditedWithReason {
                reason: reason.into(),
            },
        )
    }
    async fn probe_capability(&self) -> traits::SandboxCapability {
        traits::SandboxCapability {
            available: true,
            reason: None,
            features: traits::SandboxFeatures::default(),
        }
    }
}

#[tokio::test]
async fn finished_background_bash_task_does_not_stay_running() {
    // (M8 cc2.1.198 "Task panels no longer get stuck showing Running after
    // the task has finished") THE BUG: `register_self_contained_handlers`
    // registered `LocalBashHandler` with its default `NoopStatusSink`, so the
    // worker's terminal `set_status`/`set_exit_code` never reached the
    // registry — the stored `TaskStateBase.status` stayed `Running` forever.
    // Fixed by threading a deferred `RegistryStatusSink` through the
    // registration (bound once the registry `Arc` exists, exactly like the
    // LocalAgent sink at engine-desktop (5.46f)).
    use crate::registry_status_sink::RegistryStatusSink;

    let dir = tempdir().unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
    let runtime = Arc::new(MockRuntimeSpawner::default());
    let out_mgr = Arc::new(crate::output_manager::TaskOutputManager::new(
        PathBuf::from(dir.path()),
        fs.clone(),
    ));
    let mut registry = TaskRegistry::new(runtime, fs, out_mgr);
    let bash_sink = Arc::new(RegistryStatusSink::new());
    crate::registry::register_self_contained_handlers(
        &mut registry,
        Arc::new(ExitZeroRunner),
        Arc::new(PassSandbox),
        Arc::new(mcp::McpRegistry::new(
            Arc::new(test_harness::mocks::MockMcpTransport::default())
                as Arc<dyn traits::McpTransport>,
        )),
        bash_sink.clone() as Arc<dyn crate::handlers::TaskStatusSink>,
    );
    let registry = Arc::new(registry);
    bash_sink.bind(registry.clone() as Arc<dyn traits::task_registry::TaskRegistryHandle>);

    let id = registry
        .spawn(
            TaskType::LocalBash,
            TaskSpawnInput::LocalBash {
                command: "true".into(),
                timeout: None,
            },
            "background true".into(),
        )
        .await
        .unwrap();

    // The worker runs on the (real tokio-backed) mock runtime; poll until the
    // registry-side status leaves `Running`.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let state = registry.get(&id).await.expect("task exists");
        if state.base().status.is_terminal() {
            assert_eq!(state.base().status, TaskStatus::Completed);
            match state {
                TaskState::LocalBash(b) => assert_eq!(
                    b.exit_code,
                    Some(0),
                    "the worker's exit code writes through to the registry"
                ),
                other => panic!("expected a local_bash state, got {other:?}"),
            }
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "task stuck Running after finish (status = {:?})",
            state.base().status
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

// ---- G07/G08: mcp_task auto-background lifecycle -------------------------

#[tokio::test]
async fn register_mcp_task_inserts_running_working_state() {
    let (_d, registry) = make_registry();
    let cancel = tokio_util::sync::CancellationToken::new();
    let id = registry
        .register_mcp_task("git".into(), "status".into(), Some("tu-1".into()), cancel)
        .await
        .unwrap();
    // `NZu`/`r7h` mints a `k…` id for an mcp_task.
    assert!(id.starts_with('k'), "mcp_task id is `k…`: {id}");
    let state = registry.get(&id).await.expect("registered");
    assert_eq!(state.base().status, TaskStatus::Running);
    assert_eq!(state.base().description, "git/status");
    assert_eq!(state.base().tool_use_id.as_deref(), Some("tu-1"));
    match &state {
        TaskState::McpTask(m) => {
            assert_eq!(m.server_name, "git");
            assert_eq!(m.tool_name, "status");
            assert_eq!(m.mcp_status, "working", "seeded mcpStatus:\"working\"");
        }
        other => panic!("expected McpTask, got {other:?}"),
    }
    // A running task is not terminal → no notification yet.
    assert!(registry.take_pending_task_notifications().await.is_empty());
}

#[tokio::test]
async fn settle_mcp_task_writes_result_and_drains_notification() {
    let (_d, registry) = make_registry();
    let cancel = tokio_util::sync::CancellationToken::new();
    let id = registry
        .register_mcp_task("git".into(), "status".into(), Some("tu-9".into()), cancel)
        .await
        .unwrap();

    registry
        .settle_mcp_task(&id, "clean working tree", false)
        .await
        .unwrap();

    let state = registry.get(&id).await.unwrap();
    assert_eq!(state.base().status, TaskStatus::Completed);
    match &state {
        TaskState::McpTask(m) => assert_eq!(m.mcp_status, "completed"),
        other => panic!("expected McpTask, got {other:?}"),
    }

    // The REAL result was written into the task spool so the notification's
    // `output-file` carries it.
    let path = registry.output_manager.path_for(&id).unwrap();
    let read = registry
        .output_manager
        .read(&path, crate::output_manager::OutputOptions::default())
        .await
        .unwrap();
    assert!(
        read.content.contains("clean working tree"),
        "the settled result is spooled: {:?}",
        read.content
    );

    // Drains exactly one notification carrying the mcp_task fields.
    let drained = registry.take_pending_task_notifications().await;
    assert_eq!(drained.len(), 1, "one settled mcp_task ⇒ one notification");
    let n = &drained[0];
    assert_eq!(n.task_id, id);
    assert_eq!(n.task_type, "mcp_task");
    assert_eq!(n.status, "completed");
    assert_eq!(n.description, "git/status");
    assert_eq!(n.tool_use_id.as_deref(), Some("tu-9"));
    assert!(
        n.output_path
            .as_deref()
            .is_some_and(|p| p.ends_with(&format!("{id}.output"))),
        "output_path is the spool path: {:?}",
        n.output_path
    );
    // consume-once.
    assert!(
        registry.take_pending_task_notifications().await.is_empty(),
        "a settled mcp_task is not reported twice"
    );
}

#[tokio::test]
async fn settle_mcp_task_failed_marks_failed() {
    let (_d, registry) = make_registry();
    let cancel = tokio_util::sync::CancellationToken::new();
    let id = registry
        .register_mcp_task("db".into(), "query".into(), None, cancel)
        .await
        .unwrap();
    registry.settle_mcp_task(&id, "boom", true).await.unwrap();
    let state = registry.get(&id).await.unwrap();
    assert_eq!(state.base().status, TaskStatus::Failed);
    match &state {
        TaskState::McpTask(m) => assert_eq!(m.mcp_status, "failed"),
        other => panic!("expected McpTask, got {other:?}"),
    }
}

#[tokio::test]
async fn kill_mcp_task_fires_cancel_and_later_settle_noops() {
    let (_d, registry) = make_registry();
    let cancel = tokio_util::sync::CancellationToken::new();
    let id = registry
        .register_mcp_task("git".into(), "log".into(), None, cancel.clone())
        .await
        .unwrap();

    assert!(!cancel.is_cancelled());
    registry.kill(&id).await.unwrap();
    assert!(
        cancel.is_cancelled(),
        "kill fires the mcp_task cancel token (the port's abortController)"
    );

    let state = registry.get(&id).await.unwrap();
    assert_eq!(state.base().status, TaskStatus::Killed);
    match &state {
        TaskState::McpTask(m) => assert_eq!(m.mcp_status, "cancelled"),
        other => panic!("expected McpTask, got {other:?}"),
    }

    // A late settle after a kill is a no-op — the `notified`/terminal guard
    // (`if(O.notified) return O`) prevents resurrecting a killed task.
    registry
        .settle_mcp_task(&id, "late result", false)
        .await
        .unwrap();
    let state = registry.get(&id).await.unwrap();
    assert_eq!(
        state.base().status,
        TaskStatus::Killed,
        "a killed mcp_task is not resurrected by a late settle"
    );
}

// F3-1: `settle_mcp_task`'s terminal transition must be ATOMIC vs a concurrent
// kill. Pre-fix, settle set `end_time`/`mcpStatus` under one write guard, DROPPED
// it, awaited `cleanups.lock()`, then flipped the terminal status via a SEPARATE
// `set_status()` acquisition — so a kill racing in that gap set `Killed` /
// `mcpStatus:"cancelled"`, which settle then partially overwrote (`set_status`
// touches only `base.status`, not `mcpStatus`), producing the torn state
// `status==Completed` + `mcpStatus=="cancelled"` AND firing a spurious
// `TaskCompleted` hook a `Killed` transition must never fire.
//
// This stress test races settle against kill on the SAME mcp_task across many
// iterations on a multi-threaded runtime and asserts the terminal state is
// never TORN: the winner is either fully settled (`Completed` + `mcpStatus:
// "completed"`) or fully killed (`Killed` + `mcpStatus:"cancelled"`), never the
// forbidden `Completed` + `mcpStatus:"cancelled"` combination.
//
// That torn combination is produced ONLY by the forward interleave this finding
// targets: pre-fix, a kill landing in settle's guard→set_status gap set
// `mcpStatus:"cancelled"`, then settle's `set_status(Completed)` flipped only
// `base.status` (never touching `mcpStatus`) — leaving `Completed`+`cancelled`
// and firing a spurious `TaskCompleted`. Post-fix the flip is atomic with the
// terminal re-check, so settle either wins wholesale or no-ops on the kill.
//
// (Note: the reverse race — a kill CLOBBERING an already-`Completed` settle —
// is a distinct issue, now guarded by `kill`'s own terminal re-check and
// covered by `kill_after_settle_does_not_demote_completed_task`. This atomic
// invariant permits `Killed`+`cancelled` only when the kill genuinely won.)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn settle_vs_kill_terminal_transition_is_atomic() {
    let dir = tempdir().unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
    let runtime = Arc::new(MockRuntimeSpawner::default());
    let out_mgr = Arc::new(crate::output_manager::TaskOutputManager::new(
        PathBuf::from(dir.path()),
        fs.clone(),
    ));
    let firer = RecordingFirer::new();
    let registry = Arc::new(
        TaskRegistry::new(runtime, fs, out_mgr).with_task_completed_firer(firer.clone()),
    );

    const ITERS: usize = 400;
    for _ in 0..ITERS {
        let cancel = tokio_util::sync::CancellationToken::new();
        let id = registry
            .register_mcp_task("git".into(), "log".into(), None, cancel)
            .await
            .unwrap();

        // Race a settle (call resolved) against a kill (user TaskStop) on the
        // same task, in genuine parallel on the multi-thread runtime.
        let (r1, r2) = {
            let reg_s = registry.clone();
            let id_s = id.clone();
            let settle =
                tokio::spawn(async move { reg_s.settle_mcp_task(&id_s, "done", false).await });
            let reg_k = registry.clone();
            let id_k = id.clone();
            let kill = tokio::spawn(async move { reg_k.kill(&id_k).await });
            (settle.await.unwrap(), kill.await.unwrap())
        };
        assert!(r1.is_ok(), "settle errored: {r1:?}");
        assert!(r2.is_ok(), "kill errored: {r2:?}");

        let state = registry.get(&id).await.unwrap();
        let (status, mcp_status) = match &state {
            TaskState::McpTask(m) => (state.base().status, m.mcp_status.clone()),
            other => panic!("expected McpTask, got {other:?}"),
        };
        // The winner is either fully settled or fully killed — never the torn
        // `Completed` + `cancelled` the forward settle-over-kill race produced.
        match status {
            TaskStatus::Completed => assert_eq!(
                mcp_status, "completed",
                "torn state — status Completed but mcpStatus not completed (id {id})"
            ),
            TaskStatus::Killed => assert_eq!(
                mcp_status, "cancelled",
                "a Killed task keeps mcpStatus cancelled (id {id})"
            ),
            other => panic!("unexpected terminal status {other:?} (id {id})"),
        }
    }
    // `firer` kept registered so the `TaskCompleted` fire path is exercised
    // under the race (a spurious fire would surface any panic in that path).
    let _ = firer.recorded();
}

// F3-1 reverse race: a `kill` arriving AFTER a settle already won the terminal
// transition (and fired `TaskCompleted`) must NOT demote the task to `Killed`.
// Pre-fix, `kill` carried no terminal guard and unconditionally wrote
// `status=Killed` / `mcpStatus:"cancelled"`, clobbering the completed task —
// leaving it `Killed`+`cancelled` even though `TaskCompleted` had already fired
// for the same task, an inconsistency claude-code never produces.
//
// This test drives the interleave deterministically (settle fully, THEN kill)
// and asserts the completed state survives the kill and no spurious hook fires.
#[tokio::test]
async fn kill_after_settle_does_not_demote_completed_task() {
    let dir = tempdir().unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
    let runtime = Arc::new(MockRuntimeSpawner::default());
    let out_mgr = Arc::new(crate::output_manager::TaskOutputManager::new(
        PathBuf::from(dir.path()),
        fs.clone(),
    ));
    let firer = RecordingFirer::new();
    let registry =
        Arc::new(TaskRegistry::new(runtime, fs, out_mgr).with_task_completed_firer(firer.clone()));

    let cancel = tokio_util::sync::CancellationToken::new();
    let id = registry
        .register_mcp_task("git".into(), "log".into(), None, cancel.clone())
        .await
        .unwrap();

    // Settle wins the terminal transition first — fires `TaskCompleted` once.
    registry.settle_mcp_task(&id, "done", false).await.unwrap();
    assert_eq!(firer.recorded().len(), 1, "settle fires TaskCompleted once");

    // A late kill (e.g. a racing `TaskStop`) must NOT resurrect/demote the
    // already-completed task.
    registry.kill(&id).await.unwrap();

    let state = registry.get(&id).await.unwrap();
    let (status, mcp_status) = match &state {
        TaskState::McpTask(m) => (state.base().status, m.mcp_status.clone()),
        other => panic!("expected McpTask, got {other:?}"),
    };
    assert_eq!(
        status,
        TaskStatus::Completed,
        "a completed task is not demoted to Killed by a late kill"
    );
    assert_eq!(
        mcp_status, "completed",
        "the completed mcpStatus is not clobbered to cancelled by a late kill"
    );
    assert_eq!(
        firer.recorded().len(),
        1,
        "kill fires no additional TaskCompleted hook"
    );
}

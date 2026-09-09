//! Registry tests.
#![allow(clippy::unwrap_used)]

use super::*;
use crate::task_trait::{Task, TaskContext, TaskHandle};
use async_trait::async_trait;
use platform_api::filesystem::{FileContent, FileEvent, FileSystem, FlockGuard, FsError};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex as StdMutex;
use tempfile::tempdir;
use test_harness::mocks::MockRuntimeSpawner;

#[tokio::test]
async fn concurrent_in_process_stops_wait_for_backing_exit_before_departure() {
    struct BlockingStop {
        entered: tokio::sync::Notify,
        release: tokio::sync::Notify,
        calls: AtomicUsize,
        stopped: std::sync::atomic::AtomicBool,
        departures: AtomicUsize,
    }
    #[async_trait]
    impl Task for BlockingStop {
        fn name(&self) -> &str {
            "blocking-stop"
        }
        fn task_type(&self) -> TaskType {
            TaskType::InProcessTeammate
        }
        async fn spawn(&self, _: TaskSpawnInput, _: TaskContext) -> Result<TaskHandle, TaskError> {
            Ok(TaskHandle::new("istoplock", None))
        }
        async fn kill(&self, _: &str, _: TaskContext) -> Result<(), TaskError> {
            if self.calls.fetch_add(1, Ordering::SeqCst) != 0 {
                return Ok(());
            }
            self.entered.notify_one();
            self.release.notified().await;
            self.stopped.store(true, Ordering::SeqCst);
            Ok(())
        }
    }
    #[async_trait]
    impl platform_api::team_spawn::TeammateDepartureCleanup for BlockingStop {
        async fn has_pending_departure(&self, _: &str) -> bool {
            true
        }
        async fn complete_departure(&self, _: &str) -> Result<(), String> {
            assert!(
                self.stopped.load(Ordering::SeqCst),
                "departure must wait for actual backing exit"
            );
            self.departures.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }
    for cancel_first in [false, true] {
        let (_temp, mut registry) = make_registry();
        let handler = Arc::new(BlockingStop {
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
            calls: AtomicUsize::new(0),
            stopped: std::sync::atomic::AtomicBool::new(false),
            departures: AtomicUsize::new(0),
        });
        registry.register_handler(TaskType::InProcessTeammate, handler.clone());
        let task_id = registry
            .spawn(
                TaskType::InProcessTeammate,
                teammate_input(),
                "worker".into(),
            )
            .await
            .unwrap();
        let owner: Arc<dyn platform_api::team_spawn::TeammateDepartureCleanup> = handler.clone();
        registry
            .set_teammate_departure_cleanup(Arc::downgrade(&owner))
            .await;
        let mut first = Box::pin(registry.kill(&task_id));
        let mut second = Box::pin(registry.kill(&task_id));
        tokio::select! { _ = handler.entered.notified() => {}, result = &mut first => panic!("stop completed before exit: {result:?}") }
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut second)
                .await
                .is_err()
        );
        assert_eq!(handler.calls.load(Ordering::SeqCst), 1);
        assert_eq!(handler.departures.load(Ordering::SeqCst), 0);
        if cancel_first {
            drop(first);
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(20), &mut second)
                    .await
                    .is_err()
            );
            assert_eq!(handler.departures.load(Ordering::SeqCst), 0);
            handler.release.notify_one();
            second.await.unwrap();
            assert_eq!(handler.calls.load(Ordering::SeqCst), 2);
        } else {
            handler.release.notify_one();
            let (first, second) = tokio::join!(first, second);
            first.unwrap();
            second.unwrap();
            assert_eq!(handler.calls.load(Ordering::SeqCst), 1);
        }
        assert!(handler.departures.load(Ordering::SeqCst) > 0);
    }
}

#[tokio::test]
async fn approved_departure_io_failure_is_retryable_after_task_is_already_killed() {
    struct DepartureOwner {
        task_id: String,
        calls: AtomicUsize,
    }
    #[async_trait]
    impl platform_api::team_spawn::TeammateDepartureCleanup for DepartureOwner {
        async fn has_pending_departure(&self, task_id: &str) -> bool {
            task_id == self.task_id && self.calls.load(Ordering::SeqCst) < 2
        }
        async fn complete_departure(&self, task_id: &str) -> Result<(), String> {
            assert_eq!(task_id, self.task_id);
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                Err("departure storage unavailable".into())
            } else {
                Ok(())
            }
        }
    }
    let (_temp, registry) = make_registry();
    let task_id = registry
        .create(
            TaskType::InProcessTeammate,
            teammate_input(),
            "teammate".into(),
        )
        .await
        .unwrap();
    registry
        .set_status(&task_id, TaskStatus::Running)
        .await
        .unwrap();
    let owner = Arc::new(DepartureOwner {
        task_id: task_id.clone(),
        calls: AtomicUsize::new(0),
    });
    let cleanup: Arc<dyn platform_api::team_spawn::TeammateDepartureCleanup> = owner.clone();
    registry
        .set_teammate_departure_cleanup(Arc::downgrade(&cleanup))
        .await;
    let public: &dyn platform_api::TaskRegistryHandle = &registry;
    assert!(public.kill(&task_id).await.is_err());
    assert_eq!(
        registry.get(&task_id).await.unwrap().base().status,
        TaskStatus::Killed
    );
    assert!(public.has_pending_teammate_departure(&task_id).await);
    public.kill(&task_id).await.unwrap();
    assert_eq!(
        registry.get(&task_id).await.unwrap().base().status,
        TaskStatus::Killed
    );
    assert!(!public.has_pending_teammate_departure(&task_id).await);
    public.kill(&task_id).await.unwrap();
    assert_eq!(owner.calls.load(Ordering::SeqCst), 2);
    let ordinary = registry
        .create(
            TaskType::InProcessTeammate,
            teammate_input(),
            "finished".into(),
        )
        .await
        .unwrap();
    registry
        .set_status(&ordinary, TaskStatus::Completed)
        .await
        .unwrap();
    assert!(!public.has_pending_teammate_departure(&ordinary).await);
    public.kill(&ordinary).await.unwrap();
    assert_eq!(
        registry.get(&ordinary).await.unwrap().base().status,
        TaskStatus::Completed
    );
    assert_eq!(owner.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn external_teammate_stop_failure_is_retryable_through_public_task_registry() {
    struct StopOwner(AtomicUsize);
    #[async_trait]
    impl TeamSpawnSeam for StopOwner {
        async fn spawn_teammate(
            &self,
            _: protocol::AgentId,
            _: String,
            _: String,
            _: String,
        ) -> Result<String, TeamSpawnError> {
            unreachable!()
        }
        async fn kill(&self, _: &str) -> Result<(), TeamSpawnError> {
            if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
                Err(TeamSpawnError::Internal(
                    "backend could not confirm termination".into(),
                ))
            } else {
                Ok(())
            }
        }
    }
    let (_temp, registry) = make_registry();
    let task_id = registry
        .create(
            TaskType::InProcessTeammate,
            teammate_input(),
            "external pane".into(),
        )
        .await
        .unwrap();
    registry
        .set_status(&task_id, TaskStatus::Running)
        .await
        .unwrap();
    let owner = Arc::new(StopOwner(AtomicUsize::new(0)));
    let seam: Arc<dyn TeamSpawnSeam> = owner.clone();
    registry
        .set_external_teammate_controller(Arc::downgrade(&seam))
        .await;
    registry.register_external_teammate_task(&task_id).await;
    let public: &dyn platform_api::task_registry::TaskRegistryHandle = &registry;
    assert!(public.kill(&task_id).await.is_err());
    assert_eq!(
        public.get(&task_id).await.unwrap().unwrap().status,
        "running"
    );
    assert_eq!(public.kill(&task_id).await.unwrap().status, "killed");
    assert_eq!(owner.0.load(Ordering::SeqCst), 2);
}

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
    fusion_timeout_ms: Option<u64>,
    spawns: AtomicUsize,
    killed: StdMutex<Vec<String>>,
    cleanup_count: Option<Arc<AtomicUsize>>,
    kill_failures_remaining: AtomicUsize,
}
impl RecordingHandler {
    fn new(task_type: TaskType, task_id: &str) -> Arc<Self> {
        Arc::new(Self {
            task_type,
            task_id: task_id.to_string(),
            fusion_timeout_ms: None,
            spawns: AtomicUsize::new(0),
            killed: StdMutex::new(Vec::new()),
            cleanup_count: None,
            kill_failures_remaining: AtomicUsize::new(0),
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
            fusion_timeout_ms: None,
            spawns: AtomicUsize::new(0),
            killed: StdMutex::new(Vec::new()),
            cleanup_count: Some(cleanup_count),
            kill_failures_remaining: AtomicUsize::new(0),
        })
    }
    fn with_fusion_timeout(task_id: &str, timeout_ms: u64) -> Arc<Self> {
        Arc::new(Self {
            task_type: TaskType::LocalFusion,
            task_id: task_id.to_string(),
            fusion_timeout_ms: Some(timeout_ms),
            spawns: AtomicUsize::new(0),
            killed: StdMutex::new(Vec::new()),
            cleanup_count: None,
            kill_failures_remaining: AtomicUsize::new(0),
        })
    }
    fn fail_next_kill(&self) {
        self.kill_failures_remaining.store(1, Ordering::SeqCst);
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
        Ok(TaskHandle::new(self.task_id.clone(), cleanup)
            .with_fusion_timeout_ms(self.fusion_timeout_ms))
    }
    async fn kill(&self, task_id: &str, _ctx: TaskContext) -> Result<(), TaskError> {
        if self
            .kill_failures_remaining
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
        {
            return Err(TaskError::Internal("transient kill failure".into()));
        }
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

#[tokio::test]
async fn background_bash_identity_is_registered_settled_and_notified() {
    // The defect: a backgrounded Bash command lived in the process runner's own
    // id space, so TaskOutput/TaskStop could not resolve the id the model was
    // given and no completion notification could ever fire. claude-code mints
    // one identity per shell command and registers that same id (`Xne`), then
    // settles it from the child's result (`Ger`/`Bpt`).
    let (_dir, registry) = make_registry();

    // 1. Allocation mints a registry id and creates its output file, with no
    //    task record yet — a foreground command must not become a task.
    let (task_id, path) = registry.allocate_bash_output().await.unwrap();
    assert!(
        task_id.starts_with('b') && task_id.len() == 9,
        "background shell ids are the local_bash prefix plus 8 base-36 chars, got {task_id}",
    );
    assert_eq!(path, registry.output_manager.path_for(&task_id).unwrap());
    assert!(
        registry.get(&task_id).await.is_none(),
        "allocation alone registers nothing"
    );

    // 2. Registering makes the SAME id resolvable, which is what TaskOutput and
    //    TaskStop look up.
    registry
        .register_background_bash(
            task_id.clone(),
            "sleep 5".into(),
            "wait a bit".into(),
            Some("toolu_1".into()),
            Some("/work".into()),
            None,
        )
        .await
        .unwrap();
    let state = registry
        .get(&task_id)
        .await
        .expect("registered task resolves");
    assert_eq!(state.base().status, TaskStatus::Running);
    assert_eq!(state.base().tool_use_id.as_deref(), Some("toolu_1"));
    assert_eq!(state.base().output_file, path);
    // The record carries the fields claude-code stamps on `Xne`: the launch
    // directory, and the flag that says this shell is one the model can address.
    match &state {
        TaskState::LocalBash(bash) => {
            assert_eq!(bash.cwd.as_deref(), Some("/work"));
            assert_eq!(bash.is_backgrounded, Some(true));
        }
        other => panic!("expected a local_bash record, got {other:?}"),
    }

    // 3. Settling from the child's exit writes the status trailer claude-code
    //    appends and drives the record terminal.
    registry
        .settle_background_bash(&task_id, Some(0), false)
        .await
        .unwrap();
    let state = registry.get(&task_id).await.unwrap();
    assert_eq!(state.base().status, TaskStatus::Completed);
    assert!(state.base().end_time.is_some());
    let written = registry
        .output_manager
        .read(
            &path,
            crate::output_manager::OutputOptions {
                offset: None,
                limit: None,
            },
        )
        .await
        .unwrap();
    assert!(
        written.content.contains("[exited with code 0]"),
        "output file must carry the exit trailer, got: {:?}",
        written.content,
    );

    // 4. …and exactly one completion notification is queued for the model.
    let notifications = registry.take_pending_task_notifications().await;
    let mine: Vec<_> = notifications
        .iter()
        .filter(|n| n.task_id == task_id)
        .collect();
    assert_eq!(mine.len(), 1, "one completion notification for one command");
    assert_eq!(mine[0].task_type, "local_bash");
    assert_eq!(mine[0].status, "completed");
    assert_eq!(mine[0].description, "wait a bit");
}

#[tokio::test]
async fn settling_a_killed_background_bash_writes_the_killed_trailer() {
    let (_dir, registry) = make_registry();
    let (task_id, path) = registry.allocate_bash_output().await.unwrap();
    registry
        .register_background_bash(
            task_id.clone(),
            "sleep 5".into(),
            "wait".into(),
            None,
            None,
            None,
        )
        .await
        .unwrap();
    registry
        .settle_background_bash(&task_id, None, true)
        .await
        .unwrap();
    assert_eq!(
        registry.get(&task_id).await.unwrap().base().status,
        TaskStatus::Killed
    );
    let written = registry
        .output_manager
        .read(
            &path,
            crate::output_manager::OutputOptions {
                offset: None,
                limit: None,
            },
        )
        .await
        .unwrap();
    assert!(
        written.content.contains("[killed]"),
        "got: {:?}",
        written.content
    );
}

#[tokio::test]
async fn an_agents_background_shell_defers_that_agents_rest_notification() {
    // claude-code stamps the launching agent on the `local_bash` record
    // (`Xne`'s `agentId: _`). That is what lets the engine tell whether an agent
    // that came to rest still has live background work of its own: without the
    // stamp the shell looks like the main session's, the agent is reported as
    // fully at rest, and the model is told it finished while its own command is
    // still running.
    let (_dir, registry) = make_registry();
    let owner = protocol::AgentId::new();

    let agent_task = "a-owner-rest".to_string();
    registry
        .insert_state_for_test(TaskState::LocalAgent(crate::state::LocalAgentTaskState {
            base: crate::state::TaskStateBase {
                id: agent_task.clone(),
                task_type: TaskType::LocalAgent,
                status: TaskStatus::Running,
                description: "worker".into(),
                tool_use_id: None,
                start_time: SystemTime::now(),
                end_time: None,
                total_paused_ms: 0,
                output_file: std::path::PathBuf::from("/tmp/tasks/a-owner-rest.output"),
                evict_after: None,
                output_offset: 0,
                notified: false,
                creator_teammate_name: None,
                creator_team_name: None,
                creator_agent_id: None,
            },
            agent_id: owner,
            subagent_type: "general-purpose".into(),
            prompt: String::new(),
            error: None,
            messages: vec![],
            pending_messages: vec![],
            is_backgrounded: true,
            outcome: Default::default(),
            forked_skill_name: None,
        }))
        .await;

    let (shell_id, _path) = registry.allocate_bash_output().await.unwrap();
    registry
        .register_background_bash(
            shell_id.clone(),
            "sleep 60".into(),
            "long one".into(),
            None,
            None,
            Some(owner),
        )
        .await
        .unwrap();

    // The agent comes to rest while its shell is still running.
    registry
        .mark_task_rested(
            &agent_task,
            Some("done".into()),
            None,
            Some(owner),
            None,
            None,
        )
        .await;
    let held = registry.take_pending_task_notifications().await;
    assert!(
        !held.iter().any(|n| n.task_id == agent_task),
        "the rest notification must wait for the agent's own background shell",
    );

    // Once the shell settles, the deferred rest notification is released.
    registry
        .settle_background_bash(&shell_id, Some(0), false)
        .await
        .unwrap();
    let released = registry.take_pending_task_notifications().await;
    assert!(
        released.iter().any(|n| n.task_id == agent_task),
        "the rest notification must be released once no live child remains, got: {:?}",
        released.iter().map(|n| &n.task_id).collect::<Vec<_>>(),
    );
}

/// A stub for the live command behind an armed row — claude-code keeps the real
/// `shellCommand` there and calls `t.background(e)` on it.
#[derive(Default)]
struct RecordingBackgrounder {
    calls: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl platform_api::task_registry::TaskBackgrounder for RecordingBackgrounder {
    async fn background(&self) {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

async fn arm_foreground(
    registry: &TaskRegistry,
    label: &str,
) -> (String, std::sync::Arc<RecordingBackgrounder>) {
    let (id, _) = registry.allocate_bash_output().await.unwrap();
    registry
        .register_foreground_bash(
            &id,
            platform_api::task_registry::BackgroundBashRegistration {
                command: format!("sleep 60 # {label}"),
                description: label.into(),
                tool_use_id: Some(format!("toolu_{label}")),
                cwd: None,
                creator_agent_id: None,
            },
            true,
        )
        .await
        .unwrap();
    let requester = std::sync::Arc::new(RecordingBackgrounder::default());
    registry
        .bind_background_requester(&id, requester.clone())
        .await
        .unwrap();
    (id, requester)
}

/// claude-code `U6t` after `cnr` ms, then `W6t` when the command finishes in the
/// foreground: an armed row is visible while the command runs and is GONE once
/// it finishes there. A row left behind would sit `running` forever and
/// eventually narrate a completion for a command the model was never told had
/// started.
#[tokio::test]
async fn an_armed_foreground_row_is_visible_and_then_withdrawn() {
    let (_dir, registry) = make_registry();
    let (id, _) = arm_foreground(&registry, "slow").await;

    let state = registry.get(&id).await.expect("armed row is addressable");
    assert_eq!(state.base().status, TaskStatus::Running);
    assert!(
        matches!(&state, TaskState::LocalBash(bash) if bash.is_backgrounded == Some(false)),
        "an armed row is NOT a background task",
    );

    registry.unregister_foreground_bash(&id).await;
    assert!(
        registry.get(&id).await.is_none(),
        "the row must be withdrawn when the command finishes in the foreground",
    );
    assert!(
        registry.take_pending_task_notifications().await.is_empty(),
        "and withdrawing it must not narrate a completion",
    );
}

/// claude-code `Wer`/`I_t`: ask the live command to detach FIRST, then flip
/// `isBackgrounded`. A row that was already backgrounded is not asked twice.
#[tokio::test]
async fn backgrounding_an_armed_row_asks_the_command_then_flips_the_flag() {
    let (_dir, registry) = make_registry();
    let (id, requester) = arm_foreground(&registry, "slow").await;

    assert!(registry.background_task(&id).await);
    assert_eq!(
        requester.calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the live command must actually be asked to detach",
    );
    let state = registry.get(&id).await.unwrap();
    assert!(matches!(&state, TaskState::LocalBash(bash) if bash.is_backgrounded == Some(true)));

    // Idempotent: `Upt` excludes an already-backgrounded row.
    assert!(!registry.background_task(&id).await);
    assert_eq!(
        requester.calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "a second request must not reach the command again",
    );

    // And `W6t` leaves it alone now — it owns a live child.
    registry.unregister_foreground_bash(&id).await;
    assert!(
        registry.get(&id).await.is_some(),
        "a backgrounded row must survive the foreground withdrawal",
    );
}

/// claude-code `zM` / `H_t` / `Ode`.
#[tokio::test]
async fn background_all_moves_every_armed_row_and_tool_use_targets_one() {
    let (_dir, registry) = make_registry();
    let (first, _) = arm_foreground(&registry, "one").await;
    let (second, _) = arm_foreground(&registry, "two").await;

    assert!(registry.has_backgroundable_tasks().await);

    // `Ode` moves exactly the row owning that tool_use_id.
    assert!(registry.background_task_for_tool_use("toolu_one").await);
    assert!(!registry.background_task_for_tool_use("toolu_missing").await);
    let one = registry.get(&first).await.unwrap();
    assert!(matches!(&one, TaskState::LocalBash(b) if b.is_backgrounded == Some(true)));
    let two = registry.get(&second).await.unwrap();
    assert!(
        matches!(&two, TaskState::LocalBash(b) if b.is_backgrounded == Some(false)),
        "the other row must be untouched",
    );

    // `zM` then takes what is left, and nothing remains backgroundable.
    assert_eq!(registry.background_all_tasks().await, 1);
    assert!(!registry.has_backgroundable_tasks().await);
    assert_eq!(registry.background_all_tasks().await, 0);
}

/// `nHn`'s second half, `dG((r)=>r.agentId===e)`, is NOT scoped to the rows the
/// kill loop just touched: it drops every pending notification addressed to the
/// exiting agent. A shell the subagent ran to completion and never read is
/// exactly that case — the kill loop skips it (it is already terminal), so
/// before this the sweep left it to surface in the MAIN session.
///
/// This is the break the sibling test cannot see: `kill` itself stamps
/// `notified` through `mark_killed`, so neutering the sweep's own stamping
/// changes nothing for a row the sweep killed. Only a row the kill loop never
/// touches distinguishes the two mechanisms.
#[tokio::test]
async fn a_finishing_agent_also_silences_shells_it_already_finished() {
    let (_dir, registry) = make_registry();
    let mine = protocol::AgentId::new();
    let theirs = protocol::AgentId::new();

    let mut ids = Vec::new();
    for (label, owner) in [
        ("mine-finished", Some(mine)),
        ("theirs-finished", Some(theirs)),
        ("main-finished", None),
    ] {
        let (id, _) = registry.allocate_bash_output().await.unwrap();
        registry
            .register_background_bash(
                id.clone(),
                format!("echo hi # {label}"),
                label.into(),
                None,
                None,
                owner,
            )
            .await
            .unwrap();
        // Each one finishes on its OWN — no kill involved, so nothing has
        // stamped `notified` and each still owes the model a notification.
        registry
            .settle_background_bash(&id, Some(0), false)
            .await
            .unwrap();
        ids.push((label, id));
    }

    // The sweep kills nothing: every owned shell is already terminal.
    assert_eq!(
        registry.kill_background_shells_for_agent(mine).await,
        0,
        "nothing was still running to kill"
    );

    let drained = registry.take_pending_task_notifications().await;
    let surfaced: Vec<&str> = ids
        .iter()
        .filter(|(_, id)| drained.iter().any(|n| &n.task_id == id))
        .map(|(label, _)| *label)
        .collect();
    // The exiting agent's own finished shell is silenced; the other two are
    // untouched, which is what proves this is about ownership and not about the
    // drain being empty.
    assert_eq!(
        surfaced,
        vec!["theirs-finished", "main-finished"],
        "only the exiting agent's own finished shell may be silenced"
    );
}

#[tokio::test]
async fn a_finishing_agent_stops_only_its_own_background_shells() {
    // The Bash tool promises a synchronous subagent that a command it
    // backgrounds is terminated when the agent gives its final response.
    // claude-code sweeps exactly the finishing agent's own shells; a sweep that
    // took everything would kill the main session's commands every time any
    // subagent finished.
    let (_dir, registry) = make_registry();
    let mine = protocol::AgentId::new();
    let theirs = protocol::AgentId::new();

    let mut ids = Vec::new();
    for (label, owner) in [
        ("mine", Some(mine)),
        // A SECOND shell owned by the same agent, so the silence assertion
        // below is a COUNT and not a single-row `any`: a suppression that
        // stamped only the first row it killed would still pass the `any`.
        ("mine-2", Some(mine)),
        ("theirs", Some(theirs)),
        ("main-session", None),
    ] {
        let (id, _) = registry.allocate_bash_output().await.unwrap();
        registry
            .register_background_bash(
                id.clone(),
                format!("sleep 60 # {label}"),
                label.into(),
                None,
                None,
                owner,
            )
            .await
            .unwrap();
        ids.push((label, id));
    }

    let killed = registry.kill_background_shells_for_agent(mine).await;
    assert_eq!(killed, 2, "exactly the finishing agent's own shells");

    for (label, id) in &ids {
        let status = registry.get(id).await.unwrap().base().status;
        if label.starts_with("mine") {
            assert_eq!(status, TaskStatus::Killed, "{label} must be stopped");
        } else {
            assert_eq!(status, TaskStatus::Running, "{label} must be left alone");
        }
    }

    // A second sweep finds nothing left to kill.
    assert_eq!(registry.kill_background_shells_for_agent(mine).await, 0);

    // And the sweep is SILENT. `nHn` kills the rows and then immediately
    // dequeues the notifications that produced (`dG((r)=>r.agentId===e)`), so
    // tidying up after a subagent must not narrate itself into the main
    // session. The other two shells are untouched and still notify normally.
    let drained = registry.take_pending_task_notifications().await;
    let swept: Vec<String> = ids
        .iter()
        .filter(|(label, _)| label.starts_with("mine"))
        .map(|(_, id)| id.clone())
        .collect();
    assert_eq!(swept.len(), 2, "both of the agent's shells were swept");
    assert_eq!(
        drained
            .iter()
            .filter(|n| swept.contains(&n.task_id))
            .count(),
        0,
        "no swept shell may surface a <task-notification>, got: {:?}",
        drained.iter().map(|n| &n.task_id).collect::<Vec<_>>(),
    );

    // Control: an unswept shell reaching the same terminal state DOES notify,
    // which is what proves the assertion above is about the sweep and not about
    // the drain being empty.
    let untouched = ids
        .iter()
        .find(|(label, _)| *label == "main-session")
        .map(|(_, id)| id.clone())
        .unwrap();
    registry
        .settle_background_bash(&untouched, Some(0), false)
        .await
        .unwrap();
    let after = registry.take_pending_task_notifications().await;
    assert!(
        after.iter().any(|n| n.task_id == untouched),
        "an ordinary shell completion must still notify, got: {:?}",
        after.iter().map(|n| &n.task_id).collect::<Vec<_>>(),
    );
}

#[tokio::test]
async fn stopping_a_background_bash_closes_its_output_file_with_the_killed_trailer() {
    // claude-code's TaskStop path (`JF`) appends `\n[killed]\n` to the shell's
    // output file before it notifies, so a later Read of the file shows how the
    // command ended rather than just stopping mid-stream.
    let (_dir, registry) = make_registry();
    let (task_id, path) = registry.allocate_bash_output().await.unwrap();
    registry
        .register_background_bash(
            task_id.clone(),
            "sleep 60".into(),
            "long one".into(),
            None,
            None,
            None,
        )
        .await
        .unwrap();
    registry
        .output_manager
        .append(&path, "partial output\n")
        .await
        .unwrap();

    registry.kill(&task_id).await.unwrap();

    assert_eq!(
        registry.get(&task_id).await.unwrap().base().status,
        TaskStatus::Killed
    );
    let written = registry
        .output_manager
        .read(
            &path,
            crate::output_manager::OutputOptions {
                offset: None,
                limit: None,
            },
        )
        .await
        .unwrap();
    assert!(
        written.content.contains("partial output") && written.content.contains("[killed]"),
        "the killed trailer must follow the partial output, got: {:?}",
        written.content,
    );
    // A second kill must not append a second trailer: only the call that
    // performs the Running -> Killed transition writes one.
    registry.kill(&task_id).await.unwrap();
    let again = registry
        .output_manager
        .read(
            &path,
            crate::output_manager::OutputOptions {
                offset: None,
                limit: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        again.content.matches("[killed]").count(),
        1,
        "exactly one trailer per task, got: {:?}",
        again.content,
    );
}

#[tokio::test]
async fn discarding_an_unused_bash_identity_removes_its_output_file() {
    // A foreground command's allocated file is redundant once the command
    // returns inline; leaving it would drop one file per shell call into the
    // session's task directory (claude-code `deleteOutputFile`).
    let (_dir, registry) = make_registry();
    let (task_id, path) = registry.allocate_bash_output().await.unwrap();
    registry.discard_bash_output(&task_id).await;
    assert!(
        registry
            .output_manager
            .read(
                &path,
                crate::output_manager::OutputOptions {
                    offset: None,
                    limit: None
                }
            )
            .await
            .map(|out| out.content)
            .unwrap_or_default()
            .is_empty(),
        "the discarded spool must not survive",
    );
    // Re-allocating the same identity must be possible after a discard.
    registry
        .register_background_bash(task_id.clone(), "cmd".into(), "d".into(), None, None, None)
        .await
        .unwrap();
    registry.discard_bash_output(&task_id).await;
    assert!(
        registry.get(&task_id).await.is_some(),
        "discard must not remove a registered task's live output",
    );
}

fn teammate_input() -> TaskSpawnInput {
    TaskSpawnInput::InProcessTeammate {
        spawn_request: None,
        inheritance: None,
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
async fn spawn_publishes_the_handlers_captured_fusion_timeout_on_the_task_state() {
    const TIMEOUT_MS: u64 = 3_600_250;
    let (_d, mut registry) = make_registry();
    registry.register_handler(
        TaskType::LocalFusion,
        RecordingHandler::with_fusion_timeout("ftimeout1", TIMEOUT_MS),
    );
    let request = platform_api::FusionRequest {
        schema_version: 1,
        origin: platform_api::FusionOrigin::Slash,
        prompt: "review this".into(),
        preset: platform_api::FusionPreset::Quality,
        models: None,
        dimensions: vec!["coverage".into()],
        partial_ok: true,
        max_panel: None,
        cross_provider: false,
        parent_profile: "openai".into(),
        parent_model: "gpt-5.4".into(),
        conversation_id: Some("11111111-2222-4333-8444-555555555555".into()),
        workflow_run_id: None,
    };

    let id = registry
        .spawn(
            TaskType::LocalFusion,
            TaskSpawnInput::LocalFusion {
                request,
                conversation_id: "11111111-2222-4333-8444-555555555555".into(),
            },
            "Fusion quality same-provider: review this".into(),
        )
        .await
        .expect("spawn");

    let state = registry.get(&id).await.expect("fusion state published");
    let TaskState::LocalFusion(fusion) = state else {
        panic!("expected LocalFusion state");
    };
    assert_eq!(fusion.effective_timeout_ms, Some(TIMEOUT_MS));
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
        creator_teammate_name: None,
        creator_team_name: None,
        creator_agent_id: None,
        spawn_request: None,
        inheritance: None,
    }
}

fn local_agent_input_with_creator(name: &str, team: &str) -> TaskSpawnInput {
    TaskSpawnInput::LocalAgent {
        agent_id: protocol::AgentId::new(),
        subagent_type: "general-purpose".into(),
        prompt: "do the work".into(),
        is_backgrounded: true,
        tool_use_id: None,
        creator_teammate_name: Some(name.into()),
        creator_team_name: Some(team.into()),
        creator_agent_id: None,
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
async fn budget_stop_matches_claude_background_agent_filter() {
    use crate::state::LocalAgentTaskState;
    use platform_api::task_registry::TaskRegistryHandle;

    let (_d, mut registry) = make_registry();
    let agent_handler = RecordingHandler::new(TaskType::LocalAgent, "abudget01");
    let workflow_handler = RecordingHandler::new(TaskType::LocalWorkflow, "wbudget01");
    registry.register_handler(TaskType::LocalAgent, agent_handler.clone());
    registry.register_handler(TaskType::LocalWorkflow, workflow_handler.clone());

    registry
        .spawn(
            TaskType::LocalAgent,
            local_agent_input(),
            "background research".into(),
        )
        .await
        .unwrap();
    registry
        .spawn(
            TaskType::LocalWorkflow,
            TaskSpawnInput::LocalWorkflow {
                session_uuid: None,
                workflow_id: "budget-workflow".into(),
                script: "return true".into(),
                resume_from_run_id: None,
                args: None,
                run_id: Some("wf_budget".into()),
                parent_model: None,
                parent_model_profile: None,
                invocation_mode: Some("inline".into()),
                workflow_source: Some("inline".into()),
                script_is_verbatim_builtin: Some(false),
                transcript_subdir: None,
                launched_from_subagent: false,
                tool_use_id: None,
                creator_teammate_name: None,
                creator_team_name: None,
                creator_agent_id: None,
                scope: None,
            },
            "background workflow".into(),
        )
        .await
        .unwrap();

    // A running foreground agent is explicitly excluded by Claude's
    // `isBackgrounded === false` guard, even though it shares the same task
    // type and status as the background agent.
    registry
        .insert_state_for_test(TaskState::LocalAgent(LocalAgentTaskState {
            base: TaskStateBase {
                id: "aforegrnd".into(),
                task_type: TaskType::LocalAgent,
                status: TaskStatus::Running,
                description: "foreground".into(),
                tool_use_id: None,
                start_time: SystemTime::now(),
                end_time: None,
                total_paused_ms: 0,
                output_file: PathBuf::from("/tmp/tasks/aforegrnd.output"),
                evict_after: None,
                output_offset: 0,
                notified: false,
                creator_teammate_name: None,
                creator_team_name: None,
                creator_agent_id: None,
            },
            agent_id: protocol::AgentId::nil(),
            subagent_type: "general-purpose".into(),
            prompt: String::new(),
            error: None,
            messages: vec![],
            pending_messages: vec![],
            is_backgrounded: false,
            outcome: Default::default(),
            forked_skill_name: None,
        }))
        .await;

    let announced = std::sync::atomic::AtomicBool::new(false);
    let announce = || {
        assert!(agent_handler.killed_ids().is_empty());
        assert!(workflow_handler.killed_ids().is_empty());
        announced.store(true, Ordering::SeqCst);
    };
    let stopped = TaskRegistryHandle::stop_background_agents_for_budget(&registry, &announce)
        .await
        .unwrap();

    assert_eq!(stopped, 2);
    assert!(announced.load(Ordering::SeqCst));
    assert_eq!(agent_handler.killed_ids(), vec!["abudget01"]);
    assert_eq!(workflow_handler.killed_ids(), vec!["wbudget01"]);
    assert_eq!(
        registry
            .get("aforegrnd")
            .await
            .expect("foreground agent retained")
            .base()
            .status,
        TaskStatus::Running
    );

    // The recording workflow handler does not drive the production status
    // sink, so settle its synthetic state before exercising the no-match path.
    registry
        .set_status("wbudget01", TaskStatus::Killed)
        .await
        .unwrap();
    let unexpected_announcement = || panic!("no matching background tasks remain");
    let stopped_again =
        TaskRegistryHandle::stop_background_agents_for_budget(&registry, &unexpected_announcement)
            .await
            .unwrap();
    assert_eq!(stopped_again, 0);
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

#[tokio::test]
async fn failed_handler_kill_preserves_route_and_cleanup_for_retry() {
    let (_d, mut registry) = make_registry();
    let cleanup_count = Arc::new(AtomicUsize::new(0));
    let handler = RecordingHandler::with_cleanup_counter(
        TaskType::InProcessTeammate,
        "tretrykill",
        cleanup_count.clone(),
    );
    handler.fail_next_kill();
    registry.register_handler(TaskType::InProcessTeammate, handler.clone());

    let id = registry
        .spawn(
            TaskType::InProcessTeammate,
            teammate_input(),
            "retryable".into(),
        )
        .await
        .unwrap();

    assert!(registry.kill(&id).await.is_err());
    assert_eq!(cleanup_count.load(Ordering::SeqCst), 0);
    assert!(
        registry.is_alive(&id).await,
        "a failed owner kill must keep the task routable"
    );

    registry.kill(&id).await.unwrap();
    assert_eq!(handler.killed_ids(), vec![id]);
    assert_eq!(cleanup_count.load(Ordering::SeqCst), 1);
}

// ---- T04: TeamSpawnSeam impl on TaskRegistry ---------------------------

#[tokio::test]
async fn team_spawn_seam_spawns_real_teammate() {
    use platform_api::team_spawn::TeamSpawnSeam;

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
    use platform_api::team_spawn::{TeamSpawnError, TeamSpawnSeam};

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
        Ok(TaskHandle::new(self.task_id.clone(), None))
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
    use platform_api::team_spawn::TeamSpawnSeam;

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
async fn seam_send_message_accepts_the_same_aliases_as_stop_and_output() {
    // A spawned agent is addressable by its task id AND its aliases: the agent
    // id, its name, and `name@team`. `get`/`kill`/`output` all resolve those,
    // so the model can stop or read an agent by any of them — but SendMessage
    // looked the raw string up in the spawned-id index and answered
    // `Terminated`, which reads as "that agent is gone" for an agent that is
    // very much alive.
    use platform_api::team_spawn::TeamSpawnSeam;

    let (_d, mut registry) = make_registry();
    let handler = MsgRecordingHandler::new(TaskType::InProcessTeammate, "tmsgid2");
    registry.register_handler(TaskType::InProcessTeammate, handler.clone());

    let agent_id = protocol::AgentId::new();
    let seam: &dyn TeamSpawnSeam = &registry;
    let task_id = seam
        .spawn_teammate(
            agent_id,
            "buddy".into(),
            "alpha".into(),
            "a teammate".into(),
        )
        .await
        .unwrap();

    for alias in [
        task_id.clone(),
        agent_id.to_string(),
        "buddy".to_string(),
        "buddy@alpha".to_string(),
    ] {
        seam.send_message(&alias, format!("msg via {alias}"))
            .await
            .unwrap_or_else(|e| panic!("SendMessage must accept the alias {alias}: {e:?}"));
    }

    // Every delivery reached the handler under the CANONICAL task id.
    let received = handler.received();
    assert_eq!(
        received.len(),
        4,
        "one delivery per alias, got {received:?}"
    );
    assert!(
        received.iter().all(|(id, _)| id == &task_id),
        "the handler must always see the canonical id, got {received:?}",
    );

    // An id that is neither the task nor any alias still reports Terminated.
    assert!(
        seam.send_message("nope", "x".into()).await.is_err(),
        "an unknown id must still be Terminated",
    );
}

#[tokio::test]
async fn seam_send_message_unknown_task_is_terminated() {
    use platform_api::team_spawn::{TeamSpawnError, TeamSpawnSeam};

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
    use platform_api::team_spawn::{TeamSpawnError, TeamSpawnSeam};

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
    use platform_api::team_spawn::{TeamSpawnError, TeamSpawnSeam};

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
async fn completed_transition_carries_creator_identity() {
    let dir = tempdir().unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
    let runtime = Arc::new(MockRuntimeSpawner::default());
    let out_mgr = Arc::new(crate::output_manager::TaskOutputManager::new(
        PathBuf::from(dir.path()),
        fs.clone(),
    ));
    let firer = RecordingFirer::new();
    let mut registry =
        TaskRegistry::new(runtime, fs, out_mgr).with_task_completed_firer(firer.clone());
    let handler = RecordingHandler::new(TaskType::LocalAgent, "acompident");
    registry.register_handler(TaskType::LocalAgent, handler);

    let task_id = registry
        .spawn(
            TaskType::LocalAgent,
            local_agent_input_with_creator("researcher", "alpha"),
            "ship the parity port".into(),
        )
        .await
        .unwrap();
    registry
        .set_status(&task_id, TaskStatus::Completed)
        .await
        .unwrap();

    let recorded = firer.recorded();
    assert_eq!(
        recorded.len(),
        1,
        "exactly one TaskCompleted fire: {recorded:?}"
    );
    assert_eq!(recorded[0].teammate_name.as_deref(), Some("researcher"));
    assert_eq!(recorded[0].team_name.as_deref(), Some("alpha"));
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
async fn first_terminal_status_is_absorbing() {
    let (_d, registry, firer, task_id) = registry_with_firer("x").await;

    registry
        .set_status(&task_id, TaskStatus::Killed)
        .await
        .unwrap();
    let updated = registry
        .set_status(&task_id, TaskStatus::Failed)
        .await
        .unwrap();

    assert_eq!(updated.base().status, TaskStatus::Killed);
    assert!(
        firer.recorded().is_empty(),
        "a late failure must not overwrite Killed or fire TaskCompleted"
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

struct ActivationOrderingHandler {
    task_id: String,
    created_firer: Arc<RecordingCreatedFirer>,
    activated: Arc<std::sync::atomic::AtomicBool>,
    publication_check: Arc<dyn Fn() + Send + Sync>,
}

struct GatedCreatedFirer {
    started: Arc<tokio::sync::Semaphore>,
    release: Arc<tokio::sync::Semaphore>,
}

impl GatedCreatedFirer {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            started: Arc::new(tokio::sync::Semaphore::new(0)),
            release: Arc::new(tokio::sync::Semaphore::new(0)),
        })
    }
}

#[async_trait]
impl hooks::TaskCreatedFirer for GatedCreatedFirer {
    async fn fire(&self, _fire: hooks::TaskCreatedFire) {
        self.started.add_permits(1);
        self.release
            .acquire()
            .await
            .expect("release semaphore remains open")
            .forget();
    }
}

struct CancellationActivationHandler {
    task_id: String,
    activated: Arc<std::sync::atomic::AtomicBool>,
    cleanup_calls: Arc<AtomicUsize>,
}

#[async_trait]
impl Task for CancellationActivationHandler {
    fn name(&self) -> &str {
        "cancellation-activation"
    }

    fn task_type(&self) -> TaskType {
        TaskType::LocalAgent
    }

    async fn spawn(
        &self,
        _input: TaskSpawnInput,
        _ctx: TaskContext,
    ) -> Result<TaskHandle, TaskError> {
        let activated = self.activated.clone();
        let cleanup_calls = self.cleanup_calls.clone();
        let cleanup = Arc::new(move || {
            cleanup_calls.fetch_add(1, Ordering::SeqCst);
        }) as Arc<dyn Fn() + Send + Sync>;
        Ok(
            TaskHandle::new(self.task_id.clone(), Some(cleanup)).with_activation(move || {
                activated.store(true, Ordering::SeqCst);
            }),
        )
    }

    async fn kill(&self, _task_id: &str, _ctx: TaskContext) -> Result<(), TaskError> {
        Ok(())
    }
}

#[async_trait]
impl Task for ActivationOrderingHandler {
    fn name(&self) -> &str {
        "activation-ordering"
    }

    fn task_type(&self) -> TaskType {
        TaskType::LocalAgent
    }

    async fn spawn(
        &self,
        _input: TaskSpawnInput,
        _ctx: TaskContext,
    ) -> Result<TaskHandle, TaskError> {
        let task_id = self.task_id.clone();
        let created_firer = self.created_firer.clone();
        let activated = self.activated.clone();
        let publication_check = self.publication_check.clone();
        let cleanup = Arc::new(|| {}) as Arc<dyn Fn() + Send + Sync>;
        Ok(
            TaskHandle::new(task_id, Some(cleanup)).with_activation(move || {
                assert_eq!(
                    created_firer.recorded().len(),
                    1,
                    "worker activation must follow TaskCreated"
                );
                publication_check();
                activated.store(true, Ordering::SeqCst);
            }),
        )
    }

    async fn kill(&self, _task_id: &str, _ctx: TaskContext) -> Result<(), TaskError> {
        Ok(())
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
async fn spawn_activates_worker_only_after_full_registration_and_task_created() {
    let (_d, mut registry, firer) = registry_with_created_firer();
    let activated = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let input = local_agent_input_with_creator("researcher", "alpha");
    let expected_alias = match &input {
        TaskSpawnInput::LocalAgent { agent_id, .. } => agent_id.to_string(),
        _ => unreachable!(),
    };
    let tasks = registry.tasks.clone();
    let spawned = registry.spawned.clone();
    let cleanups = registry.cleanups.clone();
    let aliases = registry.aliases.clone();
    let publication_check = Arc::new(move || {
        assert!(tasks
            .try_read()
            .expect("task publication lock released before activation")
            .contains_key("aactivate"));
        assert_eq!(
            spawned
                .try_read()
                .expect("spawn-route lock released before activation")
                .get("aactivate"),
            Some(&TaskType::LocalAgent)
        );
        assert!(cleanups
            .try_lock()
            .expect("cleanup lock released before activation")
            .contains_key("aactivate"));
        assert_eq!(
            aliases
                .try_read()
                .expect("alias lock released before activation")
                .get(&expected_alias)
                .map(String::as_str),
            Some("aactivate")
        );
    });
    registry.register_handler(
        TaskType::LocalAgent,
        Arc::new(ActivationOrderingHandler {
            task_id: "aactivate".into(),
            created_firer: firer,
            activated: activated.clone(),
            publication_check,
        }),
    );

    registry
        .spawn(TaskType::LocalAgent, input, "activation ordering".into())
        .await
        .unwrap();

    assert!(
        activated.load(Ordering::SeqCst),
        "registry activates the prepared worker before returning"
    );
}

#[tokio::test]
async fn cancelling_spawn_during_task_created_rolls_back_every_publication() {
    let dir = tempdir().unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
    let runtime = Arc::new(MockRuntimeSpawner::default());
    let out_mgr = Arc::new(crate::output_manager::TaskOutputManager::new(
        PathBuf::from(dir.path()),
        fs.clone(),
    ));
    let firer = GatedCreatedFirer::new();
    let activated = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let cleanup_calls = Arc::new(AtomicUsize::new(0));
    let mut registry =
        TaskRegistry::new(runtime, fs, out_mgr).with_task_created_firer(firer.clone());
    registry.register_handler(
        TaskType::LocalAgent,
        Arc::new(CancellationActivationHandler {
            task_id: "acancel1".into(),
            activated: activated.clone(),
            cleanup_calls: cleanup_calls.clone(),
        }),
    );
    let registry = Arc::new(registry);
    let input = local_agent_input_with_creator("researcher", "alpha");
    let expected_alias = match &input {
        TaskSpawnInput::LocalAgent { agent_id, .. } => agent_id.to_string(),
        _ => unreachable!(),
    };

    let spawn = tokio::spawn({
        let registry = registry.clone();
        async move {
            registry
                .spawn(TaskType::LocalAgent, input, "cancel me".into())
                .await
        }
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), firer.started.acquire())
        .await
        .expect("TaskCreated hook starts")
        .unwrap()
        .forget();
    assert!(
        registry.get("acancel1").await.is_some(),
        "the hook runs only after the task row is published"
    );
    assert_eq!(
        registry.resolve_task_id(&expected_alias).await.as_deref(),
        Some("acancel1")
    );

    spawn.abort();
    assert!(spawn.await.unwrap_err().is_cancelled());

    for _ in 0..200 {
        if registry.get("acancel1").await.is_none() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(registry.get("acancel1").await.is_none());
    assert!(!registry.spawned.read().await.contains_key("acancel1"));
    assert!(!registry.cleanups.lock().await.contains_key("acancel1"));
    assert_eq!(registry.resolve_task_id(&expected_alias).await, None);
    assert!(!activated.load(Ordering::SeqCst));
    assert_eq!(cleanup_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn spawn_local_agent_task_created_carries_creator_identity() {
    let (_d, mut registry, firer) = registry_with_created_firer();
    let handler = RecordingHandler::new(TaskType::LocalAgent, "acreator1");
    registry.register_handler(TaskType::LocalAgent, handler);

    let task_id = registry
        .spawn(
            TaskType::LocalAgent,
            local_agent_input_with_creator("researcher", "alpha"),
            "background agent".into(),
        )
        .await
        .unwrap();

    let recorded = firer.recorded();
    assert_eq!(recorded.len(), 1, "spawn fires TaskCreated: {recorded:?}");
    assert_eq!(recorded[0].task_id, task_id);
    assert_eq!(recorded[0].teammate_name.as_deref(), Some("researcher"));
    assert_eq!(recorded[0].team_name.as_deref(), Some("alpha"));
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
    ) -> Result<platform_api::filesystem::FileContent, platform_api::filesystem::FsError> {
        let map = self.files.lock().await;
        let content = map.get(path).cloned().unwrap_or_default();
        let total_lines = content.lines().count() as u64;
        Ok(platform_api::filesystem::FileContent {
            content,
            truncated: false,
            total_lines,
        })
    }
    async fn write_file(
        &self,
        path: &str,
        body: &str,
    ) -> Result<(), platform_api::filesystem::FsError> {
        self.files
            .lock()
            .await
            .insert(path.to_string(), body.to_string());
        Ok(())
    }
    async fn create_new_file(&self, path: &str) -> Result<(), platform_api::filesystem::FsError> {
        self.creates.fetch_add(1, Ord2::SeqCst);
        let mut map = self.files.lock().await;
        if map.contains_key(path) {
            return Err(platform_api::filesystem::FsError::AlreadyExists(
                path.to_string(),
            ));
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
        std::pin::Pin<Box<dyn futures::Stream<Item = platform_api::filesystem::FileEvent> + Send>>,
        platform_api::filesystem::FsError,
    > {
        Err(platform_api::filesystem::FsError::Io("nope".into()))
    }
    async fn append_file(
        &self,
        path: &str,
        body: &str,
    ) -> Result<(), platform_api::filesystem::FsError> {
        self.files
            .lock()
            .await
            .entry(path.to_string())
            .or_default()
            .push_str(body);
        Ok(())
    }
    async fn truncate(&self, _: &str, _: u64) -> Result<(), platform_api::filesystem::FsError> {
        Ok(())
    }
    async fn file_mtime(
        &self,
        _: &str,
    ) -> Result<std::time::SystemTime, platform_api::filesystem::FsError> {
        Ok(std::time::SystemTime::UNIX_EPOCH)
    }
    async fn file_size(&self, path: &str) -> Result<u64, platform_api::filesystem::FsError> {
        let map = self.files.lock().await;
        Ok(map.get(path).map_or(0, |s| s.len() as u64))
    }
    async fn delete_file(&self, path: &str) -> Result<(), platform_api::filesystem::FsError> {
        self.files.lock().await.remove(path);
        Ok(())
    }
    async fn symlink(&self, _: &str, _: &str) -> Result<(), platform_api::filesystem::FsError> {
        Ok(())
    }
    async fn flock_exclusive(
        &self,
        _: &str,
    ) -> Result<Box<dyn platform_api::filesystem::FlockGuard>, platform_api::filesystem::FsError>
    {
        Err(platform_api::filesystem::FsError::Io("nope".into()))
    }
    async fn fsync(&self, _: &str) -> Result<(), platform_api::filesystem::FsError> {
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
        Ok(crate::task_trait::TaskHandle::new(
            self.task_id.clone(),
            None,
        ))
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
async fn ordinary_agent_raw_result_id_reads_output_and_stops_owning_handler() {
    use platform_api::task_registry::TaskRegistryHandle;

    let (_dir, mut registry) = make_registry();
    let handler = RecordingHandler::new(TaskType::LocalAgent, "aordinarytask");
    registry.register_handler(TaskType::LocalAgent, handler.clone());
    let input = local_agent_input();
    let TaskSpawnInput::LocalAgent { agent_id, .. } = &input else {
        unreachable!()
    };
    // Same canonical address Agent returns after this runtime spawn. This
    // unnamed launch cannot accidentally resolve through a display-name alias.
    let emitted_agent_id = agent_id.as_uuid().to_string();
    let task_id = registry
        .spawn(TaskType::LocalAgent, input, "ordinary agent".into())
        .await
        .unwrap();
    let spool = registry
        .get(&task_id)
        .await
        .unwrap()
        .base()
        .output_file
        .clone();
    registry
        .output_manager
        .append(&spool, "agent output\n")
        .await
        .unwrap();

    let handle: &dyn TaskRegistryHandle = &registry;
    let task = handle
        .get(&emitted_agent_id)
        .await
        .unwrap()
        .expect("Agent result ID must locate its task");
    assert_eq!(task.task_id, task_id);
    let output = handle.output(&emitted_agent_id, None).await.unwrap();
    assert_eq!(output.task_id, task_id);
    assert_eq!(output.content, "agent output\n");
    assert!(!output.done);

    let stopped = handle.kill(&emitted_agent_id).await.unwrap();
    assert_eq!(stopped.task_id, task_id);
    assert_eq!(stopped.status, "killed");
    assert_eq!(handler.killed_ids(), vec![task_id.clone()]);
    let retained = handle.output(&emitted_agent_id, None).await.unwrap();
    assert!(retained.done);
    assert_eq!(retained.status.as_deref(), Some("killed"));
    assert_eq!(retained.content, "agent output\n");
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
                spawn_request: None,
                inheritance: None,
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

// ---- WP6 fix round 1: `local_fusion` notification mapping crosses the sink
// seam. `RegistryStatusSink::set_fusion_error` /
// `set_fusion_egress_and_usage` / `finish_fusion_terminal` are the ONLY
// production callers that ever reach `TaskRegistry::set_fusion_error` etc.
// (a `local_fusion` task never calls the registry methods directly — see
// `LocalFusionHandler`), so a test that only calls the registry methods
// straight, or only asserts the handler CALLED the sink, never proves the
// sink's forwarding + the registry's `take_pending_task_notifications`
// mapping actually deliver `<error>` / `<result>` / `<usage>` /
// `<egress-profiles>` end to end. These two tests drive a `TaskState::LocalFusion`
// entry THROUGH `RegistryStatusSink` (never through `TaskRegistry` directly)
// and assert the exact drained `TaskNotification` fields.

fn local_fusion_state_for_test(id: &str, output_dir: &std::path::Path) -> TaskState {
    TaskState::LocalFusion(crate::state::LocalFusionTaskState {
        base: TaskStateBase {
            id: id.to_string(),
            task_type: TaskType::LocalFusion,
            status: TaskStatus::Running,
            description: "fusion run".into(),
            tool_use_id: None,
            start_time: SystemTime::now(),
            end_time: None,
            total_paused_ms: 0,
            output_file: output_dir.join(format!("{id}.output")),
            evict_after: None,
            output_offset: 0,
            notified: false,
            creator_teammate_name: None,
            creator_team_name: None,
            creator_agent_id: None,
        },
        conversation_id: "conv1".into(),
        prompt: "compare two approaches".into(),
        run_id: None,
        preset: "quality".into(),
        cross_provider: false,
        final_text: None,
        error: None,
        egress_profiles: Vec::new(),
        usage: None,
        stage: None,
        effective_timeout_ms: None,
        result_published: false,
    })
}

#[tokio::test]
async fn take_pending_drains_local_fusion_error_through_status_sink() {
    use crate::handlers::TaskStatusSink;
    use crate::registry_status_sink::RegistryStatusSink;

    let (dir, registry) = make_registry();
    let registry = Arc::new(registry);
    let id = "fu_err01";
    registry
        .insert_state_for_test(local_fusion_state_for_test(id, dir.path()))
        .await;

    // Bind a real `RegistryStatusSink` and drive the failure THROUGH it —
    // exactly the path `LocalFusionHandler` uses in production, and the seam
    // WP6 fix-round-1 proved was unpinned by mutation.
    let sink = RegistryStatusSink::new();
    sink.bind(registry.clone());
    sink.set_fusion_error(id, "too few fusion models".to_string())
        .await;
    sink.set_status(id, TaskStatus::Failed).await;

    let drained = registry.take_pending_task_notifications().await;
    assert_eq!(
        drained.len(),
        1,
        "one terminal fusion task ⇒ one notification"
    );
    let n = &drained[0];
    assert_eq!(n.task_id, id);
    assert_eq!(n.task_type, "local_fusion");
    assert_eq!(n.status, "failed");
    assert_eq!(
        n.error.as_deref(),
        Some("too few fusion models"),
        "a failed local_fusion task notifies with its real reason, not bare \"failed\""
    );
    assert!(n.result.is_none(), "no final_text was ever set on this run");
    assert!(n.egress_profiles.is_empty());
}

#[tokio::test]
async fn take_pending_drains_local_fusion_egress_and_usage_through_status_sink() {
    use crate::handlers::TaskStatusSink;
    use crate::registry_status_sink::RegistryStatusSink;
    use platform_api::task_registry::AgentRunUsage;

    let (dir, registry) = make_registry();
    let registry = Arc::new(registry);
    let id = "fu_ok01";
    registry
        .insert_state_for_test(local_fusion_state_for_test(id, dir.path()))
        .await;

    let sink = RegistryStatusSink::new();
    sink.bind(registry.clone());

    let usage = AgentRunUsage {
        subagent_tokens: 4200,
        tool_uses: 6,
        duration_ms: 8800,
    };
    sink.set_fusion_egress_and_usage(
        id,
        vec!["anthropic".to_string(), "openai".to_string()],
        Some(usage.clone()),
    )
    .await;
    sink.finish_fusion_terminal(
        id,
        "fu_run_1".to_string(),
        "final answer text".to_string(),
        TaskStatus::Completed,
    )
    .await;

    let drained = registry.take_pending_task_notifications().await;
    assert_eq!(
        drained.len(),
        1,
        "one terminal fusion task ⇒ one notification"
    );
    let n = &drained[0];
    assert_eq!(n.task_id, id);
    assert_eq!(n.task_type, "local_fusion");
    assert_eq!(n.status, "completed");
    assert!(n.error.is_none());
    assert_eq!(
        n.result.as_deref(),
        Some("final answer text"),
        "finish_fusion_terminal's final_text reaches <result>"
    );
    assert_eq!(
        n.egress_profiles,
        vec!["anthropic".to_string(), "openai".to_string()],
        "set_fusion_egress_and_usage's profiles reach <egress-profiles>"
    );
    assert_eq!(
        n.usage,
        Some(usage),
        "set_fusion_egress_and_usage's usage reaches <usage> when no local_agent outcome exists"
    );
}

/// F005: `LocalFusionTaskState.stage` updates in place, once per progress
/// event, through `RegistryStatusSink::set_fusion_stage` — the wire a
/// `/fusion` task's DTO/list entry uses to surface `FusionStage::label()`
/// text as the run progresses (mirroring `subagent_activity` on the
/// Agent-tool path).
#[tokio::test]
async fn set_fusion_stage_updates_the_local_fusion_task_state_in_place() {
    use crate::handlers::TaskStatusSink;
    use crate::registry_status_sink::RegistryStatusSink;

    let (dir, registry) = make_registry();
    let registry = Arc::new(registry);
    let id = "fu_stage01";
    registry
        .insert_state_for_test(local_fusion_state_for_test(id, dir.path()))
        .await;

    let stage_of = |state: &TaskState| match state {
        TaskState::LocalFusion(fusion) => fusion.stage.clone(),
        other => panic!("expected LocalFusion, got {other:?}"),
    };

    assert_eq!(
        stage_of(&registry.get(id).await.expect("task exists")),
        None,
        "no progress event has landed yet"
    );

    let sink = RegistryStatusSink::new();
    sink.bind(registry.clone());
    sink.set_fusion_stage(id, "Resolving models".to_string())
        .await;
    assert_eq!(
        stage_of(&registry.get(id).await.expect("task exists")),
        Some("Resolving models".to_string())
    );

    sink.set_fusion_stage(id, "Running panels 2/3".to_string())
        .await;
    assert_eq!(
        stage_of(&registry.get(id).await.expect("task exists")),
        Some("Running panels 2/3".to_string()),
        "a later progress event overwrites the previous stage in place"
    );
}

/// Review finding #17: a `Completed` `local_fusion` task must let a
/// downstream waiter (e.g. print mode's `await_local_fusion_result_bounded`)
/// distinguish "terminal status landed" from "the durable `<fusion-result>`
/// session append actually finished" — otherwise a one-shot host can return
/// (and exit the process) while the append is still in flight and silently
/// lose the row. `TaskRegistry::mark_fusion_result_published` is the seam
/// `local_fusion`'s worker calls, AFTER `FusionCompletionSink::publish`
/// resolves, to flip that flag.
#[tokio::test]
async fn mark_fusion_result_published_flips_the_flag_in_place() {
    use crate::handlers::TaskStatusSink;
    use crate::registry_status_sink::RegistryStatusSink;

    let (dir, registry) = make_registry();
    let registry = Arc::new(registry);
    let id = "fu_publish01";
    registry
        .insert_state_for_test(local_fusion_state_for_test(id, dir.path()))
        .await;

    let published_of = |state: &TaskState| match state {
        TaskState::LocalFusion(fusion) => fusion.result_published,
        other => panic!("expected LocalFusion, got {other:?}"),
    };

    assert!(
        !published_of(&registry.get(id).await.expect("task exists")),
        "a freshly spawned run has not published its result yet"
    );

    let sink = RegistryStatusSink::new();
    sink.bind(registry.clone());
    sink.mark_fusion_result_published(id).await;

    assert!(
        published_of(&registry.get(id).await.expect("task exists")),
        "mark_fusion_result_published must flip result_published to true"
    );

    // An unknown/evicted task id is a benign no-op, same as the other
    // best-effort fusion status-sink writes above (set_fusion_stage etc).
    sink.mark_fusion_result_published("fu_does_not_exist").await;
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
        evict_after: None,
        output_offset: 0,
        notified: false,
        creator_teammate_name: None,
        creator_team_name: None,
        creator_agent_id: None,
    };
    let creator_agent_id = protocol::AgentId::new();
    let input = TaskSpawnInput::LocalAgent {
        agent_id: protocol::AgentId::nil(),
        subagent_type: "general-purpose".into(),
        prompt: "go".into(),
        is_backgrounded: true,
        tool_use_id: Some("toolu_bg42".into()),
        creator_teammate_name: Some("researcher".into()),
        creator_team_name: Some("alpha".into()),
        creator_agent_id: Some(creator_agent_id),
        spawn_request: None,
        inheritance: None,
    };
    let state = state_for_spawn(base, &input);
    assert_eq!(state.base().tool_use_id.as_deref(), Some("toolu_bg42"));
    assert_eq!(
        state.base().creator_teammate_name.as_deref(),
        Some("researcher")
    );
    assert_eq!(state.base().creator_team_name.as_deref(), Some("alpha"));
    assert_eq!(state.base().creator_agent_id, Some(creator_agent_id));

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
        evict_after: None,
        output_offset: 0,
        notified: false,
        creator_teammate_name: None,
        creator_team_name: None,
        creator_agent_id: None,
    };
    let input2 = TaskSpawnInput::LocalAgent {
        agent_id: protocol::AgentId::nil(),
        subagent_type: "general-purpose".into(),
        prompt: "go".into(),
        is_backgrounded: true,
        tool_use_id: None,
        creator_teammate_name: None,
        creator_team_name: None,
        creator_agent_id: None,
        spawn_request: None,
        inheritance: None,
    };
    let state2 = state_for_spawn(base2, &input2);
    assert_eq!(state2.base().tool_use_id, None);
    assert_eq!(state2.base().creator_teammate_name, None);
    assert_eq!(state2.base().creator_team_name, None);
    assert_eq!(state2.base().creator_agent_id, None);
}

#[test]
fn state_for_spawn_stamps_local_workflow_tool_use_id() {
    let creator_agent_id = protocol::AgentId::new();
    let base = TaskStateBase {
        id: "wspawn001".into(),
        task_type: TaskType::LocalWorkflow,
        status: TaskStatus::Running,
        description: "Audit source".into(),
        tool_use_id: None,
        start_time: SystemTime::now(),
        end_time: None,
        total_paused_ms: 0,
        output_file: std::path::PathBuf::from("/tmp/tasks/wspawn001.output"),
        evict_after: None,
        output_offset: 0,
        notified: false,
        creator_teammate_name: None,
        creator_team_name: None,
        creator_agent_id: None,
    };
    let input = TaskSpawnInput::LocalWorkflow {
        session_uuid: Some("session-1".into()),
        workflow_id: "audit".into(),
        script: "return 'ok'".into(),
        resume_from_run_id: None,
        args: Some(r#"{"scope":"src"}"#.into()),
        run_id: Some("wf_abcdef".into()),
        parent_model: None,
        parent_model_profile: None,
        invocation_mode: Some("inline".into()),
        workflow_source: Some("inline".into()),
        script_is_verbatim_builtin: Some(false),
        transcript_subdir: Some("/tmp/transcripts/wf_abcdef".into()),
        launched_from_subagent: true,
        tool_use_id: Some("toolu_workflow42".into()),
        creator_teammate_name: Some("builder".into()),
        creator_team_name: Some("alpha".into()),
        creator_agent_id: Some(creator_agent_id),
        scope: None,
    };

    let state = state_for_spawn(base, &input);
    assert_eq!(
        state.base().tool_use_id.as_deref(),
        Some("toolu_workflow42")
    );
    assert_eq!(
        state.base().creator_teammate_name.as_deref(),
        Some("builder")
    );
    assert_eq!(state.base().creator_team_name.as_deref(), Some("alpha"));
    assert_eq!(state.base().creator_agent_id, Some(creator_agent_id));
    assert_eq!(state.base().description, "Audit source");
}

#[tokio::test]
async fn take_pending_carries_workflow_resume_and_terminal_metadata() {
    use crate::state::{LocalWorkflowTaskState, TaskState, TaskStateBase};

    let (_d, registry) = make_registry();
    registry
        .insert_state_for_test(TaskState::LocalWorkflow(LocalWorkflowTaskState {
            base: TaskStateBase {
                id: "wmeta0001".into(),
                task_type: TaskType::LocalWorkflow,
                status: TaskStatus::Completed,
                description: "Audit source".into(),
                tool_use_id: Some("toolu_workflow42".into()),
                start_time: SystemTime::now(),
                end_time: Some(SystemTime::now()),
                total_paused_ms: 0,
                output_file: std::path::PathBuf::from("/tmp/tasks/wmeta0001.output"),
                evict_after: None,
                output_offset: 0,
                notified: false,
                creator_teammate_name: None,
                creator_team_name: None,
                creator_agent_id: None,
            },
            session_uuid: Some("session-1".into()),
            workflow_id: "audit".into(),
            script: "return 'ok'".into(),
            resume_from_run_id: None,
            args: Some(r#"{"scope":"src"}"#.into()),
            run_id: Some("wf_abcdef".into()),
            script_path: Some("/tmp/session/workflows/wf_abcdef.js".into()),
            transcript_dir: Some("/tmp/session/subagents/workflows/wf_abcdef".into()),
            current_step: 2,
            outcome: platform_api::task_registry::WorkflowTerminalOutcome {
                result: Some("ok".into()),
                failures: vec!["one retry exhausted".into()],
                agent_count: 4,
                total_tokens: 120,
                total_tool_calls: 7,
                duration_ms: 900,
                agents_done: 2,
                agents_error: 1,
                agents_skipped: 1,
                agents_empty_result: 1,
                progress_counts_available: true,
                ..Default::default()
            },
            scope: None,
        }))
        .await;

    let drained = registry.take_pending_task_notifications().await;
    assert_eq!(drained.len(), 1);
    let notification = &drained[0];
    assert_eq!(notification.description, "Audit source");
    assert_eq!(
        notification.tool_use_id.as_deref(),
        Some("toolu_workflow42")
    );
    assert_eq!(notification.workflow_run_id.as_deref(), Some("wf_abcdef"));
    assert_eq!(
        notification.workflow_script_path.as_deref(),
        Some("/tmp/session/workflows/wf_abcdef.js")
    );
    assert_eq!(
        notification.workflow_transcript_dir.as_deref(),
        Some("/tmp/session/subagents/workflows/wf_abcdef")
    );
    assert_eq!(notification.workflow_agent_count, Some(4));
    assert_eq!(notification.workflow_agents_done, Some(2));
    assert_eq!(notification.workflow_agents_error, Some(1));
    assert_eq!(notification.workflow_agents_skipped, Some(1));
    assert_eq!(notification.workflow_agents_empty_result, Some(1));
}

#[tokio::test]
async fn workflow_notification_omits_default_progress_counts() {
    use crate::state::LocalWorkflowTaskState;

    let (_d, registry) = make_registry();
    registry
        .insert_state_for_test(TaskState::LocalWorkflow(LocalWorkflowTaskState {
            base: TaskStateBase {
                id: "wmeta0002".into(),
                task_type: TaskType::LocalWorkflow,
                status: TaskStatus::Killed,
                description: "Audit source".into(),
                tool_use_id: Some("toolu_workflow43".into()),
                start_time: SystemTime::now(),
                end_time: Some(SystemTime::now()),
                total_paused_ms: 0,
                output_file: std::path::PathBuf::from("/tmp/tasks/wmeta0002.output"),
                evict_after: None,
                output_offset: 0,
                notified: false,
                creator_teammate_name: None,
                creator_team_name: None,
                creator_agent_id: None,
            },
            session_uuid: Some("session-1".into()),
            workflow_id: "audit".into(),
            script: "return 'ok'".into(),
            resume_from_run_id: None,
            args: None,
            run_id: Some("wf_counts_hidden".into()),
            script_path: Some("/tmp/session/workflows/wf_counts_hidden.js".into()),
            transcript_dir: Some("/tmp/session/subagents/workflows/wf_counts_hidden".into()),
            current_step: 1,
            outcome: platform_api::task_registry::WorkflowTerminalOutcome {
                result: Some("stopped".into()),
                agent_count: 2,
                total_tokens: 40,
                total_tool_calls: 3,
                duration_ms: 120,
                agents_done: 0,
                agents_error: 0,
                agents_skipped: 0,
                agents_empty_result: 0,
                progress_counts_available: false,
                ..Default::default()
            },
            scope: None,
        }))
        .await;

    let drained = registry.take_pending_task_notifications().await;
    assert_eq!(drained.len(), 1);
    let notification = &drained[0];
    assert_eq!(notification.workflow_agent_count, Some(2));
    assert_eq!(notification.workflow_agents_done, None);
    assert_eq!(notification.workflow_agents_error, None);
    assert_eq!(notification.workflow_agents_skipped, None);
    assert_eq!(notification.workflow_agents_empty_result, None);
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
        evict_after: None,
        output_offset: 0,
        notified: false,
        creator_teammate_name: None,
        creator_team_name: None,
        creator_agent_id: None,
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
            outcome: Default::default(),
            forked_skill_name: None,
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

/// Build a `local_agent` state in `status`, ready for `insert_state_for_test`.
fn agent_state(id: &str, status: TaskStatus) -> crate::state::TaskState {
    use crate::state::{LocalAgentTaskState, TaskState, TaskStateBase};
    TaskState::LocalAgent(LocalAgentTaskState {
        base: TaskStateBase {
            id: id.into(),
            task_type: TaskType::LocalAgent,
            status,
            description: "research".into(),
            tool_use_id: None,
            start_time: SystemTime::now(),
            end_time: None,
            total_paused_ms: 0,
            output_file: std::path::PathBuf::from(format!("/tmp/tasks/{id}.output")),
            evict_after: None,
            output_offset: 0,
            notified: false,
            creator_teammate_name: None,
            creator_team_name: None,
            creator_agent_id: None,
        },
        agent_id: protocol::AgentId::nil(),
        subagent_type: String::new(),
        prompt: String::new(),
        error: None,
        messages: vec![],
        pending_messages: vec![],
        is_backgrounded: true,
        outcome: Default::default(),
        forked_skill_name: None,
    })
}

/// The terminating run's payload reaches the drained notification: `<result>`,
/// `<usage>` and the `<worktree>` coordinates. All three were hardcoded `None`
/// while nothing wrote them, so a background agent's completion reached the
/// model as a bare status.
#[tokio::test]
async fn take_pending_carries_agent_result_usage_and_worktree() {
    let (_d, registry) = make_registry();
    registry
        .insert_state_for_test(agent_state("adone0001", TaskStatus::Running))
        .await;
    registry
        .set_agent_outcome(
            "adone0001",
            platform_api::task_registry::AgentTerminalOutcome {
                result: Some("the answer".into()),
                usage: Some(platform_api::task_registry::AgentRunUsage {
                    subagent_tokens: 120,
                    tool_uses: 3,
                    duration_ms: 4_500,
                }),
                error: None,
                worktree_path: Some("/repo/.lingxi/worktrees/agent-1".into()),
                worktree_branch: Some("worktree-agent-1".into()),
                max_turns_reached: None,
            },
        )
        .await;
    registry
        .set_status("adone0001", TaskStatus::Completed)
        .await
        .unwrap();

    let drained = registry.take_pending_task_notifications().await;
    assert_eq!(drained.len(), 1);
    let n = &drained[0];
    assert_eq!(n.result.as_deref(), Some("the answer"));
    let usage = n.usage.clone().expect("usage section");
    assert_eq!(usage.subagent_tokens, 120);
    assert_eq!(usage.tool_uses, 3);
    assert_eq!(usage.duration_ms, 4_500);
    assert_eq!(
        n.worktree_path.as_deref(),
        Some("/repo/.lingxi/worktrees/agent-1")
    );
    assert_eq!(n.worktree_branch.as_deref(), Some("worktree-agent-1"));
    assert!(n.killed_by.is_none(), "a completion has no stop initiator");
}

// ---- TID-04 / TID-05 / AGT-11: the terminal clocks and the eviction sweep ----

/// TID-05: `end_time` was declared and never written outside the `mcp_task`
/// settle, so every other terminal row reported a `None` that could only be read
/// as "still running" — and the 30 s `mcp_task` eviction guard had nothing to
/// measure from.
#[tokio::test]
async fn a_terminal_transition_stamps_the_end_time() {
    let (_d, registry) = make_registry();
    registry
        .insert_state_for_test(agent_state("aclock001", TaskStatus::Running))
        .await;
    assert!(registry.get("aclock001").await.unwrap().base().end_time.is_none());

    registry
        .set_status("aclock001", TaskStatus::Completed)
        .await
        .unwrap();
    let base = registry.get("aclock001").await.unwrap();
    assert!(base.base().end_time.is_some(), "the terminal transition stamps it");
    // AGT-11: and the 30 s eviction deadline alongside it.
    assert!(base.base().evict_after.is_some(), "and the eviction deadline");
}

/// AGT-11: a resting agent that still owns live background children gets NO
/// deadline — the oracle's `if(t.park&&keepaliveReasons.size>0)return`. Without
/// this the parent is evicted out from under children that still report to it.
#[tokio::test]
async fn an_agent_holding_live_children_gets_no_eviction_deadline() {
    let (_d, registry) = make_registry();
    let parent = protocol::AgentId::new();
    let mut parent_state = agent_state("aheld0001", TaskStatus::Running);
    if let crate::state::TaskState::LocalAgent(agent) = &mut parent_state {
        agent.agent_id = parent;
    }
    registry.insert_state_for_test(parent_state).await;
    // A live child that names the parent as its creator.
    let mut child = agent_state("achild001", TaskStatus::Running);
    child.base_mut().creator_agent_id = Some(parent);
    registry.insert_state_for_test(child).await;

    registry
        .set_status("aheld0001", TaskStatus::Completed)
        .await
        .unwrap();
    let held = registry.get("aheld0001").await.unwrap();
    assert!(held.base().end_time.is_some(), "the clock still stamps");
    assert!(
        held.base().evict_after.is_none(),
        "but no deadline while children are live"
    );
}

/// TID-04's subtlest rule: the two `?? ` defaults point in OPPOSITE directions.
/// An agent with no deadline is kept forever (`evictAfter ?? 1/0`), a workflow
/// with no deadline is evicted at once (`evictAfter ?? 0`). Collapsing them to
/// one default silently changes which rows survive.
#[tokio::test]
async fn the_missing_deadline_defaults_point_opposite_ways() {
    let (_d, registry) = make_registry();
    // Both terminal + notified, neither carrying a deadline.
    let mut agent = agent_state("akeep0001", TaskStatus::Completed);
    agent.base_mut().notified = true;
    registry.insert_state_for_test(agent).await;
    let mut workflow = workflow_state_for_evict("wgone0001");
    workflow.base_mut().notified = true;
    registry.insert_state_for_test(workflow).await;

    let _ = registry.take_pending_task_notifications().await;

    assert!(
        registry.get("akeep0001").await.is_some(),
        "a deadline-less agent is KEPT (`?? 1/0`)"
    );
    assert!(
        registry.get("wgone0001").await.is_none(),
        "a deadline-less workflow is EVICTED (`?? 0`)"
    );
}

/// An `mcp_task` is the one type measured from `end_time`, not `evict_after`,
/// and it is held for the full 30 s.
#[tokio::test]
async fn an_mcp_task_is_held_for_thirty_seconds_after_it_ends() {
    let (_d, registry) = make_registry();
    let mut fresh = mcp_state_for_evict("kfresh001");
    fresh.base_mut().notified = true;
    fresh.base_mut().end_time = Some(SystemTime::now());
    registry.insert_state_for_test(fresh).await;
    let mut stale = mcp_state_for_evict("kstale001");
    stale.base_mut().notified = true;
    stale.base_mut().end_time = Some(SystemTime::now() - std::time::Duration::from_secs(31));
    registry.insert_state_for_test(stale).await;

    let _ = registry.take_pending_task_notifications().await;

    assert!(
        registry.get("kfresh001").await.is_some(),
        "still inside the 30 s window"
    );
    assert!(
        registry.get("kstale001").await.is_none(),
        "past it, so evicted"
    );
}

fn workflow_state_for_evict(id: &str) -> crate::state::TaskState {
    use crate::state::{LocalWorkflowTaskState, TaskState, TaskStateBase};
    TaskState::LocalWorkflow(LocalWorkflowTaskState {
        base: TaskStateBase {
            id: id.into(),
            task_type: TaskType::LocalWorkflow,
            status: TaskStatus::Completed,
            description: "wf".into(),
            tool_use_id: None,
            start_time: SystemTime::now(),
            end_time: None,
            total_paused_ms: 0,
            output_file: std::path::PathBuf::from(format!("/tmp/tasks/{id}.output")),
            evict_after: None,
            output_offset: 0,
            notified: false,
            creator_teammate_name: None,
            creator_team_name: None,
            creator_agent_id: None,
        },
        session_uuid: None,
        workflow_id: "evict".into(),
        script: String::new(),
        resume_from_run_id: None,
        args: None,
        run_id: None,
        script_path: None,
        transcript_dir: None,
        current_step: 0,
        outcome: platform_api::task_registry::WorkflowTerminalOutcome::default(),
        scope: None,
    })
}

fn mcp_state_for_evict(id: &str) -> crate::state::TaskState {
    use crate::state::{McpTaskState, TaskState, TaskStateBase};
    TaskState::McpTask(McpTaskState {
        base: TaskStateBase {
            id: id.into(),
            task_type: TaskType::McpTask,
            status: TaskStatus::Completed,
            description: "mcp".into(),
            tool_use_id: None,
            start_time: SystemTime::now(),
            end_time: None,
            total_paused_ms: 0,
            output_file: std::path::PathBuf::from(format!("/tmp/tasks/{id}.output")),
            evict_after: None,
            output_offset: 0,
            notified: false,
            creator_teammate_name: None,
            creator_team_name: None,
            creator_agent_id: None,
        },
        server_name: "acme".into(),
        tool_name: "deploy".into(),
        mcp_status: "completed".into(),
        status_message: None,
        result_text: None,
    })
}

/// AGT-08 / TN-06 middle link: `max_turns_reached` has to SURVIVE the drain.
/// The renderer's turn-limit variant and the handler that reads
/// `{"reason":"max_turns_exhausted"}` are both useless if the registry drops
/// the field between them.
#[tokio::test]
async fn take_pending_carries_the_exhausted_turn_budget() {
    let (_d, registry) = make_registry();
    registry
        .insert_state_for_test(agent_state("aturn0001", TaskStatus::Running))
        .await;
    registry
        .set_agent_outcome(
            "aturn0001",
            platform_api::task_registry::AgentTerminalOutcome {
                max_turns_reached: Some(12),
                ..Default::default()
            },
        )
        .await;
    registry
        .set_status("aturn0001", TaskStatus::Completed)
        .await
        .unwrap();

    let drained = registry.take_pending_task_notifications().await;
    assert_eq!(drained.len(), 1);
    assert_eq!(drained[0].max_turns_reached, Some(12));
}

/// The same drain leaves the field `None` for a run that stopped for any other
/// reason — otherwise every completion would render the turn-limit verb.
#[tokio::test]
async fn take_pending_leaves_the_turn_budget_unset_for_a_normal_completion() {
    let (_d, registry) = make_registry();
    registry
        .insert_state_for_test(agent_state("aturn0002", TaskStatus::Running))
        .await;
    registry
        .set_agent_outcome(
            "aturn0002",
            platform_api::task_registry::AgentTerminalOutcome {
                result: Some("done".into()),
                ..Default::default()
            },
        )
        .await;
    registry
        .set_status("aturn0002", TaskStatus::Completed)
        .await
        .unwrap();

    let drained = registry.take_pending_task_notifications().await;
    assert_eq!(drained.len(), 1);
    assert_eq!(drained[0].max_turns_reached, None);
}

/// `outcome.error` lands on the state's `error` field — the failed summary's
/// `{error}` reads from there, and nothing in production wrote it before, so
/// every failed background agent reported claude's `Unknown error` fallback.
#[tokio::test]
async fn set_agent_outcome_error_reaches_the_failed_summary() {
    let (_d, registry) = make_registry();
    registry
        .insert_state_for_test(agent_state("afail0001", TaskStatus::Running))
        .await;
    registry
        .set_agent_outcome(
            "afail0001",
            platform_api::task_registry::AgentTerminalOutcome {
                error: Some("model refused".into()),
                ..Default::default()
            },
        )
        .await;
    registry
        .set_status("afail0001", TaskStatus::Failed)
        .await
        .unwrap();

    let drained = registry.take_pending_task_notifications().await;
    assert_eq!(drained[0].error.as_deref(), Some("model refused"));
}

/// Merge semantics: a later partial report never erases an earlier one. A kill
/// that only carries a worktree must not blank out the result the run already
/// produced.
#[tokio::test]
async fn set_agent_outcome_merges_rather_than_replaces() {
    let (_d, registry) = make_registry();
    registry
        .insert_state_for_test(agent_state("amerge001", TaskStatus::Running))
        .await;
    registry
        .set_agent_outcome(
            "amerge001",
            platform_api::task_registry::AgentTerminalOutcome {
                result: Some("partial answer".into()),
                ..Default::default()
            },
        )
        .await;
    registry
        .set_agent_outcome(
            "amerge001",
            platform_api::task_registry::AgentTerminalOutcome {
                worktree_path: Some("/wt".into()),
                ..Default::default()
            },
        )
        .await;
    registry
        .set_status("amerge001", TaskStatus::Completed)
        .await
        .unwrap();

    let drained = registry.take_pending_task_notifications().await;
    assert_eq!(drained[0].result.as_deref(), Some("partial answer"));
    assert_eq!(drained[0].worktree_path.as_deref(), Some("/wt"));
}

/// `kill_with_reason` records WHO stopped the task, which selects the killed
/// summary's verb: `"parent"` → "was stopped by Claude" (the `TaskStop` TOOL),
/// `"user"` → "was stopped by user" (the UI / control-channel stop). Plain
/// `kill` records nothing — the binary's `undefined killedBy` → bare
/// "was stopped".
#[tokio::test]
async fn kill_with_reason_records_the_stop_initiator() {
    for (id, reason, expected) in [
        ("akillpar1", Some("parent"), Some("parent")),
        ("akilluse1", Some("user"), Some("user")),
        ("akillbare", None, None),
    ] {
        let (_d, registry) = make_registry();
        registry
            .insert_state_for_test(agent_state(id, TaskStatus::Running))
            .await;
        match reason {
            Some(r) => registry.kill_with_reason(id, r).await.unwrap(),
            None => registry.kill(id).await.unwrap(),
        }

        let drained = registry.take_pending_task_notifications().await;
        assert_eq!(drained.len(), 1, "{id}: killed task drains once");
        assert_eq!(drained[0].status, "killed");
        assert_eq!(drained[0].killed_by.as_deref(), expected, "{id}");
    }
}

/// AGT-07. Claude-code's resume queue refuses a stopped-by-user agent with
/// `uM` (`src_180597926.js` @407402) BEFORE it looks at anything else; the
/// gate's whole point is that the model must not silently resume work the user
/// cancelled.
///
/// The ordering is what this test really pins: `kill` REMOVES the spawned-id
/// entry, so a user-stopped agent has none. A gate placed after that lookup
/// would compile, pass a stubbed test, and be unreachable in production for
/// exactly the case it exists to catch — which is why the "parent" row here
/// asserts the ORDINARY `Terminated`, reached through the same missing entry.
#[tokio::test]
async fn seam_send_message_refuses_a_user_stopped_agent() {
    use platform_api::team_spawn::{TeamSpawnError, TeamSpawnSeam};

    let (_d, registry) = make_registry();
    registry
        .insert_state_for_test(agent_state("astopusr1", TaskStatus::Running))
        .await;
    registry
        .insert_state_for_test(agent_state("astoppar1", TaskStatus::Running))
        .await;
    registry.kill_with_reason("astopusr1", "user").await.unwrap();
    registry
        .kill_with_reason("astoppar1", "parent")
        .await
        .unwrap();

    let seam: &dyn TeamSpawnSeam = &registry;
    match seam.send_message("astopusr1", "keep going".into()).await {
        Err(TeamSpawnError::StoppedByUser(message)) => assert_eq!(
            message,
            "Agent astopusr1 was stopped by the user and won't be resumed."
        ),
        other => panic!("a user stop must be named, got {other:?}"),
    }

    // A model/parent stop carries no user intent, so it keeps the ordinary
    // "that agent is gone" answer.
    assert!(
        matches!(
            seam.send_message("astoppar1", "keep going".into()).await,
            Err(TeamSpawnError::Terminated)
        ),
        "a parent stop must not be reported as a user cancellation"
    );
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
                evict_after: None,
                output_offset: 0,
                notified: false,
                creator_teammate_name: None,
                creator_team_name: None,
                creator_agent_id: None,
            },
            session_uuid: None,
            workflow_id: String::new(),
            script: String::new(),
            resume_from_run_id: None,
            args: None,
            run_id: run_id.map(str::to_string),
            script_path: None,
            transcript_dir: None,
            current_step: 0,
            outcome: Default::default(),
            scope: None,
        })
    };

    registry
        .insert_state_for_test(mk("w-run", TaskStatus::Running, Some("wf_aaa")))
        .await;
    registry
        .insert_state_for_test(mk("w-done", TaskStatus::Completed, Some("wf_bbb")))
        .await;
    registry
        .insert_state_for_test(mk("w-paused", TaskStatus::Paused, Some("wf_ccc")))
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
    // A checkpoint adopted after restart is PAUSED, not still running. It must
    // remain visible while allowing an explicit Workflow(resumeFromRunId).
    assert_eq!(
        registry.find_running_workflow_by_run_id("wf_ccc").await,
        None
    );
    // Unknown id → None.
    assert_eq!(
        registry.find_running_workflow_by_run_id("wf_zzz").await,
        None
    );
}

#[tokio::test]
async fn find_nonterminal_local_app_workflows_matches_only_the_requested_app() {
    use crate::state::{LocalWorkflowTaskState, TaskState, TaskStateBase};

    let (_d, registry) = make_registry();
    let mk = |id: &str, app_id: &str, status: TaskStatus| {
        TaskState::LocalWorkflow(LocalWorkflowTaskState {
            base: TaskStateBase {
                id: id.into(),
                task_type: TaskType::LocalWorkflow,
                status,
                description: "local app build".into(),
                tool_use_id: None,
                start_time: SystemTime::now(),
                end_time: None,
                total_paused_ms: 0,
                output_file: std::path::PathBuf::from(format!("/tmp/tasks/{id}.output")),
                evict_after: None,
                output_offset: 0,
                notified: false,
                creator_teammate_name: None,
                creator_team_name: None,
                creator_agent_id: None,
            },
            session_uuid: None,
            workflow_id: "lingxi-local-app:local-app-build".into(),
            script: String::new(),
            resume_from_run_id: None,
            args: None,
            run_id: Some(format!("wf_{id}")),
            script_path: None,
            transcript_dir: None,
            current_step: 0,
            outcome: Default::default(),
            scope: Some(
                crate::scope::LocalAppWorkflowTaskScope::for_build(app_id).expect("valid app id"),
            ),
        })
    };

    registry
        .insert_state_for_test(mk("w-app-a1", "app-a", TaskStatus::Running))
        .await;
    registry
        .insert_state_for_test(mk("w-app-b1", "app-b", TaskStatus::Paused))
        .await;
    registry
        .insert_state_for_test(mk("w-app-a2", "app-a", TaskStatus::Completed))
        .await;

    assert_eq!(
        registry.find_nonterminal_local_app_workflows("app-a").await,
        vec!["w-app-a1"]
    );
    assert_eq!(
        registry.find_nonterminal_local_app_workflows("app-b").await,
        vec!["w-app-b1"]
    );
}

/// The delete guard now reads a task's typed `scope`, not its `workflow_id`
/// -- so `workflow_id` is irrelevant to it, in EITHER direction. Now two workflows
/// with completely different, made-up `workflow_id`s block the SAME app
/// equally, as long as they both carry a matching scope; and a scope-less
/// workflow does not block, regardless of how convincing its `workflow_id`
/// looks.
#[tokio::test]
async fn find_nonterminal_local_app_workflows_ignores_workflow_id() {
    use crate::state::{LocalWorkflowTaskState, TaskState, TaskStateBase};

    let (_d, registry) = make_registry();
    let mk = |id: &str, workflow_id: &str, scope| {
        TaskState::LocalWorkflow(LocalWorkflowTaskState {
            base: TaskStateBase {
                id: id.into(),
                task_type: TaskType::LocalWorkflow,
                status: TaskStatus::Running,
                description: "local app build".into(),
                tool_use_id: None,
                start_time: SystemTime::now(),
                end_time: None,
                total_paused_ms: 0,
                output_file: std::path::PathBuf::from(format!("/tmp/tasks/{id}.output")),
                evict_after: None,
                output_offset: 0,
                notified: false,
                creator_teammate_name: None,
                creator_team_name: None,
                creator_agent_id: None,
            },
            session_uuid: None,
            workflow_id: workflow_id.into(),
            script: String::new(),
            resume_from_run_id: None,
            args: None,
            run_id: Some(format!("wf_{id}")),
            script_path: None,
            transcript_dir: None,
            current_step: 0,
            outcome: Default::default(),
            scope,
        })
    };

    // Two DIFFERENT workflow_ids, one retired-looking and one made up, both
    // scoped to "canvas-app": both must block.
    let (_d, registry_two) = make_registry();
    registry_two
        .insert_state_for_test(mk(
            "w-canvas",
            "retired-local-app-alias",
            Some(
                crate::scope::LocalAppWorkflowTaskScope::for_build("canvas-app")
                    .expect("valid app id"),
            ),
        ))
        .await;
    registry_two
        .insert_state_for_test(mk(
            "w-canvas-2",
            "totally-unrelated-workflow-name",
            Some(
                crate::scope::LocalAppWorkflowTaskScope::for_use_test("canvas-app")
                    .expect("valid app id"),
            ),
        ))
        .await;
    let blockers = registry_two
        .find_nonterminal_local_app_workflows("canvas-app")
        .await;
    assert_eq!(blockers.len(), 2, "{blockers:?}");
    assert!(blockers.contains(&"w-canvas".to_string()));
    assert!(blockers.contains(&"w-canvas-2".to_string()));

    // The REAL namespaced build id with NO scope must not block -- the id carries
    // no authority any more.
    registry
        .insert_state_for_test(mk("w-other", "lingxi-local-app:local-app-build", None))
        .await;
    assert!(
        registry
            .find_nonterminal_local_app_workflows("canvas-app")
            .await
            .is_empty(),
        "workflow_id alone -- even the real namespaced build id -- must not block delete"
    );
}

/// §8.1: a custom workflow that reuses a REAL build workflow's exact name and
/// forges the victim's app id into its (caller-supplied) `args` must get
/// NOTHING -- not the workspace lease, not the App delete guard's
/// protection. Before this migration, `find_nonterminal_local_app_workflows`
/// matched on `workflow_id` membership in `LOCAL_APP_BUILD_WORKFLOWS` PLUS a
/// JSON-parsed `args.app_id` -- both caller-supplied -- so this EXACT shape
/// used to block the victim's delete. `requires_workspace_lease` matched the
/// same `workflow_id` alone. Both guards now read `scope`, which nothing but
/// a Host-side purpose constructor can populate ([`crate::scope`]'s module
/// docs); this task's `spawn()` never mints one from `args`, so a forged row
/// like this is `scope: None` -- indistinguishable, at this layer, from a
/// genuinely unscoped row, and denied by both guards.
#[tokio::test]
async fn a_custom_workflow_with_the_same_name_gets_no_lease_and_does_not_block_delete() {
    use crate::state::{LocalWorkflowTaskState, TaskState, TaskStateBase};

    assert!(
        !crate::handlers::local_workflow::requires_workspace_lease(None),
        "an unscoped row -- real or forged -- must never be granted the workspace lease"
    );

    let (_d, registry) = make_registry();
    let forged = TaskState::LocalWorkflow(LocalWorkflowTaskState {
        base: TaskStateBase {
            id: "wforged01".into(),
            task_type: TaskType::LocalWorkflow,
            status: TaskStatus::Running,
            description: "definitely not a build".into(),
            tool_use_id: None,
            start_time: SystemTime::now(),
            end_time: None,
            total_paused_ms: 0,
            output_file: std::path::PathBuf::from("/tmp/tasks/wforged01.output"),
            evict_after: None,
            output_offset: 0,
            notified: false,
            creator_teammate_name: None,
            creator_team_name: None,
            creator_agent_id: None,
        },
        session_uuid: None,
        // Reuses the REAL namespaced build workflow id...
        workflow_id: "lingxi-local-app:local-app-build".into(),
        script: String::new(),
        resume_from_run_id: None,
        // ...and forges the victim's app id into args, exactly as §8.1 and
        // this task's own hazard notes describe.
        args: Some(serde_json::json!({"app_id": "victim-app"}).to_string()),
        run_id: Some("wf_forged".into()),
        script_path: None,
        transcript_dir: None,
        current_step: 0,
        outcome: Default::default(),
        // No Host ever minted authority for this row -- exactly what a
        // custom workflow reusing the name gets today.
        scope: None,
    });
    registry.insert_state_for_test(forged).await;

    assert_eq!(
        registry
            .find_nonterminal_local_app_workflows("victim-app")
            .await,
        Vec::<String>::new(),
        "a scope-less row must not block deleting an app, even one named in \
         its own (caller-supplied, therefore untrusted) args"
    );
}

/// Design: `LocalAppWorkflowTaskScope::blocks_delete()` is `true` for ALL
/// THREE purposes, not just `Build` -- a `UseTest`/`McpAuthoring` scope never
/// takes the workspace lease (only `Build` does -- see `scope.rs`'s
/// `only_build_purpose_requires_a_workspace_lease`) but must still block the
/// app's delete while it runs.
///
/// The three tests below are GUARD-level, and that is the whole point:
/// `scope.rs`'s `every_purpose_blocks_delete` proves the PREDICATE answers
/// `true` for a purpose, which is a different claim from
/// `find_nonterminal_local_app_workflows` actually CONSULTING it for that
/// purpose. One test per purpose over this one shared body, so a guard that
/// silently narrowed back to `Build` fails once per purpose it dropped and
/// each failure names which purpose it was.
///
/// `Build` is the control: it stays green under exactly the narrowing that
/// reddens the other two, so their red measures the guard rather than a
/// shared body that never worked.
///
/// What is deliberately NOT covered here is `scope: None`. Per §8.1 an
/// unscoped row must NOT block -- the sibling
/// `a_custom_workflow_with_the_same_name_gets_no_lease_and_does_not_block_delete`
/// pins that -- because nothing in this crate can distinguish a "genuinely
/// mid-build but unscoped" row from a forged one: they are the same shape,
/// so treating `None` as blocking would either match on `workflow_id`/`args`
/// again or hand any caller a delete block on any app. See
/// `LocalWorkflowTaskState::scope`'s doc comment.
async fn assert_scope_blocks_delete_at_the_guard(
    scope: crate::scope::LocalAppWorkflowTaskScope,
    task_id: &str,
) {
    use crate::state::{LocalWorkflowTaskState, TaskState, TaskStateBase};

    let app_id = scope.app_id().to_string();
    let purpose = scope.purpose();

    let (_d, registry) = make_registry();
    let row = TaskState::LocalWorkflow(LocalWorkflowTaskState {
        base: TaskStateBase {
            id: task_id.into(),
            task_type: TaskType::LocalWorkflow,
            status: TaskStatus::Running,
            description: format!("{purpose:?} run"),
            tool_use_id: None,
            start_time: SystemTime::now(),
            end_time: None,
            total_paused_ms: 0,
            output_file: std::path::PathBuf::from(format!("/tmp/tasks/{task_id}.output")),
            evict_after: None,
            output_offset: 0,
            notified: false,
            creator_teammate_name: None,
            creator_team_name: None,
            creator_agent_id: None,
        },
        session_uuid: None,
        // The guard ignores `workflow_id` and `args` entirely, so a name that
        // matches nothing real is the honest input: the scope is the only
        // thing that may produce the block.
        workflow_id: "a-name-the-guard-must-not-read".into(),
        script: String::new(),
        resume_from_run_id: None,
        args: None,
        run_id: Some(format!("wf_{task_id}")),
        script_path: None,
        transcript_dir: None,
        current_step: 0,
        outcome: Default::default(),
        scope: Some(scope),
    });
    registry.insert_state_for_test(row).await;

    assert_eq!(
        registry.find_nonterminal_local_app_workflows(&app_id).await,
        vec![task_id],
        "find_nonterminal_local_app_workflows must report a non-terminal \
         {purpose:?}-purpose run as blocking {app_id}'s delete"
    );
}

/// Guard-level coverage for `Build`, and the control for the two tests below:
/// narrowing `blocks_delete()` to `Build` alone leaves THIS one green.
#[tokio::test]
async fn a_build_purpose_blocks_delete_at_the_guard() {
    let scope = crate::scope::LocalAppWorkflowTaskScope::for_build("app-1").expect("valid");
    assert!(
        scope.requires_workspace_lease(),
        "control: `Build` is the one purpose that also takes the workspace lease"
    );
    assert_scope_blocks_delete_at_the_guard(scope, "wbuildp01").await;
}

/// Guard-level coverage for `UseTest`: no workspace lease, still blocks.
#[tokio::test]
async fn a_use_test_purpose_still_blocks_delete_at_the_guard() {
    let scope = crate::scope::LocalAppWorkflowTaskScope::for_use_test("app-1").expect("valid");
    assert!(
        !scope.requires_workspace_lease(),
        "a use-test scope has no lease-granting -- i.e. no workspace-lease -- authority"
    );
    assert_scope_blocks_delete_at_the_guard(scope, "wusetest1").await;
}

/// Guard-level coverage for `McpAuthoring`: no workspace lease, still blocks.
/// Before this test the purpose was pinned only by `scope.rs`'s predicate
/// test, which cannot see whether the guard reads the predicate at all.
#[tokio::test]
async fn an_mcp_authoring_purpose_still_blocks_delete_at_the_guard() {
    let scope = crate::scope::LocalAppWorkflowTaskScope::for_mcp_authoring("app-1").expect("valid");
    assert!(
        !scope.requires_workspace_lease(),
        "an mcp-authoring scope has no lease-granting -- i.e. no workspace-lease -- authority"
    );
    assert_scope_blocks_delete_at_the_guard(scope, "wmcpauth1").await;
}

/// The `tasks` half of the P-1.7 R1 fix, pinned inside the crate that owns
/// the contract: a `LocalAppWorkflowTaskScope` handed to
/// [`crate::task_trait::TaskSpawnInput::LocalWorkflow`] is what reaches the
/// task row, so a Host that mints one gets a real delete block -- and a
/// `None` on the same input stays authority-free.
///
/// This goes through the production `spawn` path (`state_for_spawn` builds
/// the row from the REAL input, not `create`'s placeholders), so it also
/// pins that `state_for_spawn` copies the field instead of hard-coding
/// `None` there, which is exactly how the guard was inert before.
///
/// The two rows differ in NOTHING but the scope: same `workflow_id`, same
/// `args`, same status. So a guard that answered from either of those
/// caller-supplied fields could not produce this pair of answers.
#[tokio::test]
async fn a_spawned_workflows_scope_is_what_blocks_its_apps_delete() {
    let (_d, registry) = make_registry();
    let mut registry = registry;
    registry.register_handler(
        TaskType::LocalWorkflow,
        RecordingHandler::new(TaskType::LocalWorkflow, "wscoped01"),
    );

    let input =
        |scope: Option<crate::scope::LocalAppWorkflowTaskScope>| TaskSpawnInput::LocalWorkflow {
            session_uuid: None,
            workflow_id: "identical-workflow-id".into(),
            script: "return true".into(),
            resume_from_run_id: None,
            args: Some(serde_json::json!({"app_id": "scoped-app"}).to_string()),
            run_id: Some("wf_scoped".into()),
            parent_model: None,
            parent_model_profile: None,
            invocation_mode: Some("named".into()),
            workflow_source: Some("built-in".into()),
            script_is_verbatim_builtin: Some(true),
            transcript_subdir: None,
            launched_from_subagent: false,
            tool_use_id: None,
            creator_teammate_name: None,
            creator_team_name: None,
            creator_agent_id: None,
            scope,
        };

    let scoped = registry
        .spawn(
            TaskType::LocalWorkflow,
            input(Some(
                crate::scope::LocalAppWorkflowTaskScope::for_build("scoped-app")
                    .expect("valid app id"),
            )),
            "in-flight build".into(),
        )
        .await
        .expect("spawn");

    assert_eq!(
        registry
            .find_nonterminal_local_app_workflows("scoped-app")
            .await,
        vec![scoped.clone()],
        "the scope the Host put on the spawn input must reach the task row"
    );

    // Same handler id, so this REPLACES the row above with an otherwise
    // identical one whose only difference is the missing scope.
    let unscoped = registry
        .spawn(
            TaskType::LocalWorkflow,
            input(None),
            "unvouched-for run".into(),
        )
        .await
        .expect("spawn");
    assert_eq!(unscoped, scoped, "the stub handler reuses one task id");
    assert!(
        registry
            .find_nonterminal_local_app_workflows("scoped-app")
            .await
            .is_empty(),
        "an identical row WITHOUT a scope must not block: the authority is the \
         scope, not the workflow_id or the args"
    );
}

#[tokio::test]
async fn adopted_workflow_is_registered_as_paused_and_keeps_resume_metadata() {
    let (_d, registry) = make_registry();
    let started = SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(1_234);

    registry
        .register_adopted_workflow(crate::registry::AdoptedWorkflow {
            task_id: "wabc12345".into(),
            session_uuid: Some("session-1".into()),
            workflow_id: "lingxi-local-app:local-app-build".into(),
            run_id: "wf_abcdef".into(),
            script_path: "/workspace/.lingxi/workflows/build.js".into(),
            args: Some(r#"{"app_id":"demo"}"#.into()),
            transcript_dir: "/sessions/s1/subagents/workflows/wf_abcdef".into(),
            description: "Build local app".into(),
            start_time: started,
        })
        .await
        .expect("adopt workflow");

    let state = registry.get("wabc12345").await.expect("adopted state");
    let crate::state::TaskState::LocalWorkflow(workflow) = state else {
        panic!("expected local workflow")
    };
    assert_eq!(workflow.base.status, TaskStatus::Paused);
    assert!(
        workflow.base.notified,
        "adopted workflow must not emit stale completion"
    );
    assert_eq!(workflow.run_id.as_deref(), Some("wf_abcdef"));
    assert_eq!(
        workflow.script_path.as_deref(),
        Some("/workspace/.lingxi/workflows/build.js")
    );
    assert_eq!(
        workflow.transcript_dir.as_deref(),
        Some(std::path::Path::new(
            "/sessions/s1/subagents/workflows/wf_abcdef"
        ))
    );
    assert_eq!(workflow.session_uuid.as_deref(), Some("session-1"));
}

#[tokio::test]
async fn register_adopted_workflow_does_not_replace_existing_live_task_with_same_task_id() {
    use crate::state::{LocalWorkflowTaskState, TaskState, TaskStateBase};
    let (_d, registry) = make_registry();
    let live = TaskState::LocalWorkflow(LocalWorkflowTaskState {
        base: TaskStateBase {
            id: "wabc12345".into(),
            task_type: TaskType::LocalWorkflow,
            status: TaskStatus::Running,
            description: "live".into(),
            tool_use_id: None,
            start_time: SystemTime::now(),
            end_time: None,
            total_paused_ms: 0,
            output_file: std::path::PathBuf::from("/tmp/wabc12345.output"),
            evict_after: None,
            output_offset: 0,
            notified: false,
            creator_teammate_name: None,
            creator_team_name: None,
            creator_agent_id: None,
        },
        session_uuid: Some("session-live".into()),
        workflow_id: "live-workflow".into(),
        script: String::new(),
        resume_from_run_id: None,
        args: None,
        run_id: Some("wf_live".into()),
        script_path: None,
        transcript_dir: None,
        current_step: 0,
        outcome: Default::default(),
        scope: None,
    });
    registry.insert_state_for_test(live).await;

    let err = registry
        .register_adopted_workflow(crate::registry::AdoptedWorkflow {
            task_id: "wabc12345".into(),
            session_uuid: Some("session-restored".into()),
            workflow_id: "restored-workflow".into(),
            run_id: "wf_restored".into(),
            script_path: "/workspace/build.js".into(),
            args: None,
            transcript_dir: "/sessions/s1/subagents/workflows/wf_restored".into(),
            description: "restored".into(),
            start_time: SystemTime::now(),
        })
        .await
        .expect_err("live workflow must not be replaced by adoption");
    assert!(
        err.to_string()
            .contains("live task wabc12345 already exists"),
        "{err}"
    );

    let state = registry.get("wabc12345").await.expect("live state remains");
    let TaskState::LocalWorkflow(workflow) = state else {
        panic!("expected local workflow");
    };
    assert_eq!(workflow.run_id.as_deref(), Some("wf_live"));
    assert_eq!(workflow.session_uuid.as_deref(), Some("session-live"));
}

#[tokio::test]
async fn workflow_run_id_reservation_and_paused_cleanup_respect_liveness_and_session() {
    use crate::state::{LocalWorkflowTaskState, TaskState, TaskStateBase};
    let (_d, registry) = make_registry();
    let mk = |id: &str, session_uuid: &str, status: TaskStatus, run_id: &str| {
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
                evict_after: None,
                output_offset: 0,
                notified: false,
                creator_teammate_name: None,
                creator_team_name: None,
                creator_agent_id: None,
            },
            session_uuid: Some(session_uuid.into()),
            workflow_id: String::new(),
            script: String::new(),
            resume_from_run_id: None,
            args: None,
            run_id: Some(run_id.to_string()),
            script_path: None,
            transcript_dir: None,
            current_step: 0,
            outcome: Default::default(),
            scope: None,
        })
    };
    registry
        .insert_state_for_test(mk(
            "wpending1",
            "session-a",
            TaskStatus::Pending,
            "wf_pending",
        ))
        .await;
    registry
        .insert_state_for_test(mk(
            "wpaused01",
            "session-a",
            TaskStatus::Paused,
            "wf_paused",
        ))
        .await;
    registry
        .insert_state_for_test(mk(
            "wpaused02",
            "session-b",
            TaskStatus::Paused,
            "wf_paused",
        ))
        .await;

    let pending_err = registry
        .try_reserve_workflow_run_id("wf_pending")
        .await
        .expect_err("pending run id blocks duplicate launch");
    assert!(pending_err.to_string().contains("wf_pending"));

    let reservation = registry
        .try_reserve_workflow_run_id("wf_paused")
        .await
        .expect("paused run id stays resumable");
    let launching_err = registry
        .try_reserve_workflow_run_id("wf_paused")
        .await
        .expect_err("second launch must see the reservation");
    assert!(launching_err.to_string().contains("already launching"));
    drop(reservation);
    let reopened_reservation = registry
        .try_reserve_workflow_run_id("wf_paused")
        .await
        .expect("reservation release reopens the paused run id");
    drop(reopened_reservation);

    registry
        .remove_paused_workflow_by_run_id("session-a", "wf_paused")
        .await;
    assert!(registry.get("wpaused01").await.is_none());
    assert!(
        registry.get("wpaused02").await.is_some(),
        "resuming session-a must not delete session-b's paused checkpoint"
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
        evict_after: None,
        output_offset: 0,
        notified: false,
        creator_teammate_name: None,
        creator_team_name: None,
        creator_agent_id: None,
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
            outcome: Default::default(),
            forked_skill_name: None,
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
            Some(platform_api::task_registry::AgentRunUsage {
                subagent_tokens: 42,
                tool_uses: 3,
                duration_ms: 1500,
            }),
            None,
            None,
            None,
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
    registry
        .mark_task_rested("a-rest-1", None, None, None, None, None)
        .await;
    assert_eq!(
        registry.take_pending_task_notifications().await.len(),
        1,
        "each subsequent rest re-arms the notification"
    );
}

#[tokio::test]
async fn unnamed_rested_agent_waits_for_live_non_agent_children_before_notifying() {
    use crate::state::{LocalAgentTaskState, LocalWorkflowTaskState, TaskState, TaskStateBase};
    let (_d, registry) = make_registry();
    let parent_agent_id = protocol::AgentId::new();

    registry
        .insert_state_for_test(TaskState::LocalAgent(LocalAgentTaskState {
            base: TaskStateBase {
                id: "a-rest-parent".into(),
                task_type: TaskType::LocalAgent,
                status: TaskStatus::Running,
                description: "bg agent".into(),
                tool_use_id: Some("toolu_parent".into()),
                start_time: SystemTime::now(),
                end_time: None,
                total_paused_ms: 0,
                output_file: std::path::PathBuf::from("/tmp/tasks/a-rest-parent.output"),
                evict_after: None,
                output_offset: 0,
                notified: false,
                creator_teammate_name: None,
                creator_team_name: None,
                creator_agent_id: None,
            },
            agent_id: parent_agent_id,
            subagent_type: String::new(),
            prompt: String::new(),
            error: None,
            messages: vec![],
            pending_messages: vec![],
            is_backgrounded: true,
            outcome: Default::default(),
            forked_skill_name: None,
        }))
        .await;
    registry
        .insert_state_for_test(TaskState::LocalWorkflow(LocalWorkflowTaskState {
            base: TaskStateBase {
                id: "w-child-live".into(),
                task_type: TaskType::LocalWorkflow,
                status: TaskStatus::Running,
                description: "child workflow".into(),
                tool_use_id: None,
                start_time: SystemTime::now(),
                end_time: None,
                total_paused_ms: 0,
                output_file: std::path::PathBuf::from("/tmp/tasks/w-child-live.output"),
                evict_after: None,
                output_offset: 0,
                notified: false,
                creator_teammate_name: None,
                creator_team_name: None,
                creator_agent_id: Some(parent_agent_id),
            },
            session_uuid: None,
            workflow_id: "child".into(),
            script: "return true".into(),
            resume_from_run_id: None,
            args: None,
            run_id: Some("wf_child".into()),
            script_path: None,
            transcript_dir: None,
            current_step: 0,
            outcome: Default::default(),
            scope: None,
        }))
        .await;

    registry
        .mark_task_rested(
            "a-rest-parent",
            Some("rested".into()),
            Some(platform_api::task_registry::AgentRunUsage {
                subagent_tokens: 7,
                tool_uses: 1,
                duration_ms: 99,
            }),
            Some(parent_agent_id),
            None,
            None,
        )
        .await;

    assert!(
        registry.take_pending_task_notifications().await.is_empty(),
        "an unnamed rested agent stays quiet while its live workflow child is still running"
    );

    registry
        .set_status("w-child-live", TaskStatus::Completed)
        .await
        .expect("child terminal transition should succeed");

    let drained = registry.take_pending_task_notifications().await;
    assert_eq!(
        drained.len(),
        2,
        "child terminal + deferred rest notification"
    );
    let rest = drained
        .iter()
        .find(|notification| notification.task_id == "a-rest-parent")
        .expect("deferred rest notification should surface");
    assert_eq!(rest.status, "completed");
    assert_eq!(rest.result.as_deref(), Some("rested"));
    assert_eq!(rest.usage.as_ref().map(|u| u.subagent_tokens), Some(7));
}

/// Seed a `local_agent` row with an explicit identity and parent.
#[allow(clippy::too_many_arguments)]
async fn seed_agent(
    registry: &TaskRegistry,
    id: &str,
    agent_id: protocol::AgentId,
    parent: Option<protocol::AgentId>,
    status: TaskStatus,
) {
    seed_agent_described(registry, id, agent_id, parent, status, id).await;
}

#[allow(clippy::too_many_arguments)]
async fn seed_agent_described(
    registry: &TaskRegistry,
    id: &str,
    agent_id: protocol::AgentId,
    parent: Option<protocol::AgentId>,
    status: TaskStatus,
    description: &str,
) {
    use crate::state::{LocalAgentTaskState, TaskStateBase};
    registry
        .insert_state_for_test(TaskState::LocalAgent(LocalAgentTaskState {
            base: TaskStateBase {
                id: id.into(),
                task_type: TaskType::LocalAgent,
                status,
                description: description.into(),
                tool_use_id: None,
                start_time: SystemTime::now(),
                end_time: None,
                total_paused_ms: 0,
                output_file: std::path::PathBuf::from(format!("/tmp/tasks/{id}.output")),
                evict_after: None,
                output_offset: 0,
                notified: false,
                creator_teammate_name: None,
                creator_team_name: None,
                creator_agent_id: parent,
            },
            agent_id,
            subagent_type: String::new(),
            prompt: String::new(),
            error: None,
            messages: vec![],
            pending_messages: vec![],
            is_backgrounded: true,
            outcome: Default::default(),
            forked_skill_name: None,
        }))
        .await;
}

/// The link between the monitor worker that counts stdout bytes and the
/// renderer that reads them. Both ends have their own tests; without this one,
/// dropping the forward in the drain leaves every one of them green.
#[tokio::test]
async fn a_monitors_stdout_byte_count_reaches_the_notification() {
    use crate::state::{MonitorTaskState, TaskStateBase};
    let (_d, registry) = make_registry();

    let seed = |id: &str| MonitorTaskState {
        base: TaskStateBase {
            id: id.into(),
            task_type: TaskType::Monitor,
            status: TaskStatus::Running,
            description: "watch".into(),
            tool_use_id: None,
            start_time: SystemTime::now(),
            end_time: None,
            total_paused_ms: 0,
            output_file: std::path::PathBuf::from(format!("/tmp/tasks/{id}.output")),
            evict_after: None,
            output_offset: 0,
            notified: false,
            creator_teammate_name: None,
            creator_team_name: None,
            creator_agent_id: None,
        },
        command: "tail -f log".into(),
        exit_code: None,
        stdout_bytes: None,
    };
    registry
        .insert_state_for_test(TaskState::Monitor(seed("m-silent")))
        .await;
    registry
        .insert_state_for_test(TaskState::Monitor(seed("m-unmeasured")))
        .await;

    registry
        .set_monitor_stdout_bytes("m-silent", 0)
        .await
        .unwrap();
    for id in ["m-silent", "m-unmeasured"] {
        registry.set_status(id, TaskStatus::Completed).await.unwrap();
    }

    let drained = registry.take_pending_task_notifications().await;
    let silent = drained
        .iter()
        .find(|n| n.task_id == "m-silent")
        .expect("the silent monitor notifies");
    assert_eq!(
        silent.stdout_bytes,
        Some(0),
        "a measured zero must reach the renderer as Some(0)",
    );
    let unmeasured = drained
        .iter()
        .find(|n| n.task_id == "m-unmeasured")
        .expect("the unmeasured monitor notifies");
    assert_eq!(
        unmeasured.stdout_bytes, None,
        "never measured must stay None, not collapse to zero",
    );
}

/// claude-code stamps `notified` in every per-type kill handler EXCEPT
/// `local_agent`. The stamp suppresses a second telling the model does not need
/// — it already holds `Successfully stopped task: X` — while the agent
/// exception keeps the one stop it is not otherwise told about.
#[tokio::test]
async fn stopping_a_shell_is_silent_but_stopping_an_agent_is_not() {
    let (_d, registry) = make_registry();

    let (shell, _) = registry.allocate_bash_output().await.unwrap();
    registry
        .register_background_bash(
            shell.clone(),
            "sleep 60".into(),
            "long one".into(),
            None,
            None,
            None,
        )
        .await
        .unwrap();
    seed_agent(
        &registry,
        "a-victim",
        protocol::AgentId::new(),
        None,
        TaskStatus::Running,
    )
    .await;

    registry.kill_with_reason(&shell, "parent").await.unwrap();
    registry
        .kill_with_reason("a-victim", "parent")
        .await
        .unwrap();

    // Both really are terminal; the shell's silence is the stamp, not a
    // half-finished kill. Checked BEFORE the drain: the kill stamped the shell
    // `notified`, so the drain's TID-04 sweep now evicts that row and there
    // would be nothing left to read afterwards.
    assert_eq!(
        registry.get(&shell).await.unwrap().base().status,
        TaskStatus::Killed
    );

    let drained = registry.take_pending_task_notifications().await;
    let ids: Vec<&str> = drained.iter().map(|n| n.task_id.as_str()).collect();
    assert!(
        !ids.contains(&shell.as_str()),
        "a stopped shell must not narrate itself a second time, got: {ids:?}",
    );
    assert!(
        ids.contains(&"a-victim"),
        "but a stopped AGENT must still report — this is the exception a blanket \
         stamp would delete, got: {ids:?}",
    );
    // ...and having been stamped `notified` by the kill, the shell row is gone
    // after the sweep, while the agent — notified only by THIS drain — stays for
    // one more pass.
    assert!(
        registry.get(&shell).await.is_none(),
        "a kill-stamped shell is evicted by the next sweep"
    );
    assert!(
        registry.get("a-victim").await.is_some(),
        "the agent this drain just notified survives the same pass"
    );
}

/// A handler that flips its row to `Killed` through a bound `RegistryStatusSink`
/// from INSIDE `kill`, exactly as every production handler does
/// (`handlers/monitor.rs:594`, `handlers/local_bash.rs:586`,
/// `handlers/in_process_teammate.rs:1713`). `RecordingHandler` cannot stand in:
/// it has no registry back-reference, so its kill leaves the row non-terminal
/// and never reaches `mark_killed`'s already-terminal branch.
struct SinkKillingHandler {
    sink: Arc<crate::registry_status_sink::RegistryStatusSink>,
}

#[async_trait]
impl Task for SinkKillingHandler {
    fn name(&self) -> &str {
        "sink-killing"
    }
    fn task_type(&self) -> TaskType {
        TaskType::InProcessTeammate
    }
    async fn spawn(
        &self,
        _input: TaskSpawnInput,
        _ctx: TaskContext,
    ) -> Result<TaskHandle, TaskError> {
        Ok(TaskHandle::new("tsinkkill".to_string(), None))
    }
    async fn kill(&self, task_id: &str, _ctx: TaskContext) -> Result<(), TaskError> {
        use crate::handlers::TaskStatusSink;
        self.sink.set_status(task_id, TaskStatus::Killed).await;
        Ok(())
    }
}

/// The stamp has to survive a handler that flipped the row terminal FIRST.
///
/// `kill_backing_task` awaits `handler.kill(..)` before `mark_killed`, and every
/// handler with a bound `RegistryStatusSink` — Monitor and LocalBash
/// (`register_self_contained_handlers`), the desktop's InProcessTeammate, Dream,
/// LocalWorkflow, LocalFusion — sets `Killed` inside that call. With the stamp
/// only on `mark_killed`'s non-terminal path it was dead code for every one of
/// them and they kept double-telling.
#[tokio::test]
async fn a_kill_still_silences_a_row_its_handler_flipped_to_killed_first() {
    let (_d, mut registry) = make_registry();
    let sink = Arc::new(crate::registry_status_sink::RegistryStatusSink::new());
    registry.register_handler(
        TaskType::InProcessTeammate,
        Arc::new(SinkKillingHandler { sink: sink.clone() }),
    );
    let registry = Arc::new(registry);
    // Bound after the `Arc` exists — the production registration cycle.
    sink.bind(registry.clone());

    let id = registry
        .spawn(
            TaskType::InProcessTeammate,
            teammate_input(),
            "buddy".into(),
        )
        .await
        .unwrap();
    registry.kill_with_reason(&id, "parent").await.unwrap();

    assert_eq!(
        registry.get(&id).await.unwrap().base().status,
        TaskStatus::Killed,
        "premise: the handler's sink really did flip the row",
    );
    let drained = registry.take_pending_task_notifications().await;
    let ids: Vec<&str> = drained.iter().map(|n| n.task_id.as_str()).collect();
    assert!(
        !ids.contains(&id.as_str()),
        "a row its own handler killed must still be stamped, got: {ids:?}",
    );
}

/// claude-code `bjn` + `JFe` — the rosters a "no task found" message names.
#[tokio::test]
async fn not_found_rosters_list_running_teammates_and_unnamed_background_agents() {
    use crate::state::{InProcessTeammateTaskState, TaskStateBase};
    let (_d, registry) = make_registry();

    // A running teammate. `bjn` reports its addressable identity, which here is
    // the `name@team` alias the spawn path records.
    registry
        .insert_state_for_test(TaskState::InProcessTeammate(InProcessTeammateTaskState {
            is_idle: false,
            awaiting_plan_approval: false,
            base: TaskStateBase {
                id: "t-buddy".into(),
                task_type: TaskType::InProcessTeammate,
                status: TaskStatus::Running,
                description: "buddy".into(),
                tool_use_id: None,
                start_time: SystemTime::now(),
                end_time: None,
                total_paused_ms: 0,
                output_file: std::path::PathBuf::from("/tmp/tasks/t-buddy.output"),
                evict_after: None,
                output_offset: 0,
                notified: false,
                creator_teammate_name: None,
                creator_team_name: None,
                creator_agent_id: None,
            },
            agent_id: protocol::AgentId::new(),
            pending_messages: vec![],
        }))
        .await;
    registry
        .register_alias_for_test("buddy@alpha", "t-buddy")
        .await;

    // A backgrounded running agent with a description, and one WITHOUT — the
    // oracle renders the latter as a bare id, not `id ()`.
    seed_agent_described(
        &registry,
        "a-bg",
        protocol::AgentId::new(),
        None,
        TaskStatus::Running,
        "survey the crate",
    )
    .await;
    seed_agent_described(
        &registry,
        "a-bare",
        protocol::AgentId::new(),
        None,
        TaskStatus::Running,
        "",
    )
    .await;
    // A named agent is reported under its NAME elsewhere, so `JFe` excludes it.
    seed_agent_described(
        &registry,
        "a-named",
        protocol::AgentId::new(),
        None,
        TaskStatus::Running,
        "named one",
    )
    .await;

    let rosters = registry
        .not_found_rosters(None, &["a-named".to_string()])
        .await;
    assert_eq!(rosters.running_teammates, vec!["buddy@alpha".to_string()]);
    assert_eq!(
        rosters.background_agents,
        vec!["a-bare".to_string(), "a-bg (survey the crate)".to_string()],
        "a named agent is excluded, and a description-less agent is a bare id",
    );

    // The caller never suggests itself.
    let from_bg = registry
        .not_found_rosters(Some("a-bg"), &["a-named".to_string()])
        .await;
    assert_eq!(from_bg.background_agents, vec!["a-bare".to_string()]);
}

/// claude-code's cascade block in `rY`: stopping a RESTING agent stops every
/// live descendant with it, to any depth, and says nothing about them.
#[tokio::test]
async fn stopping_a_resting_parent_cascades_to_its_whole_subtree_silently() {
    let (_d, registry) = make_registry();
    let parent = protocol::AgentId::new();
    let child = protocol::AgentId::new();
    let grandchild = protocol::AgentId::new();
    let stranger = protocol::AgentId::new();

    seed_agent(&registry, "a-parent", parent, None, TaskStatus::Running).await;
    seed_agent(
        &registry,
        "a-child",
        child,
        Some(parent),
        TaskStatus::Running,
    )
    .await;
    // Depth 2 — the level a one-hop walk silently misses.
    seed_agent(
        &registry,
        "a-grandchild",
        grandchild,
        Some(child),
        TaskStatus::Running,
    )
    .await;
    // Someone else's agent, same registry.
    seed_agent(&registry, "a-stranger", stranger, None, TaskStatus::Running).await;

    // Arm the parent's rest: `GS` needs BOTH a rest and a live child.
    registry
        .mark_task_rested(
            &"a-parent".to_string(),
            None,
            None,
            Some(parent),
            None,
            None,
        )
        .await;

    registry
        .kill_with_reason("a-parent", "parent")
        .await
        .unwrap();

    for id in ["a-parent", "a-child", "a-grandchild"] {
        assert_eq!(
            registry.get(id).await.unwrap().base().status,
            TaskStatus::Killed,
            "{id} must be stopped",
        );
    }
    assert_eq!(
        registry.get("a-stranger").await.unwrap().base().status,
        TaskStatus::Running,
        "an unrelated agent must be left alone",
    );

    // The oracle stamps `notified` before each child kill, so a cascade is
    // silent. Only the directly stopped task may surface.
    let drained = registry.take_pending_task_notifications().await;
    let ids: Vec<&str> = drained.iter().map(|n| n.task_id.as_str()).collect();
    assert!(
        !ids.contains(&"a-child") && !ids.contains(&"a-grandchild"),
        "cascaded children must not narrate themselves, got: {ids:?}",
    );
}

/// The other half of `GS`: an agent that is actively RUNNING — not resting —
/// does not take its children with it. Cascading here would be stricter than
/// the oracle.
#[tokio::test]
async fn stopping_an_actively_running_parent_does_not_cascade() {
    let (_d, registry) = make_registry();
    let parent = protocol::AgentId::new();
    let child = protocol::AgentId::new();
    seed_agent(&registry, "a-busy", parent, None, TaskStatus::Running).await;
    seed_agent(&registry, "a-kid", child, Some(parent), TaskStatus::Running).await;

    // No `mark_task_rested` — the parent never came to rest.
    registry.kill_with_reason("a-busy", "parent").await.unwrap();

    assert_eq!(
        registry.get("a-busy").await.unwrap().base().status,
        TaskStatus::Killed,
    );
    assert_eq!(
        registry.get("a-kid").await.unwrap().base().status,
        TaskStatus::Running,
        "a busy parent's children survive it",
    );
}

/// `hVe` carries a visited-set cycle guard. A parent chain that loops must not
/// hang the stop path.
#[tokio::test]
async fn a_cyclic_parent_chain_terminates() {
    let (_d, registry) = make_registry();
    let a = protocol::AgentId::new();
    let b = protocol::AgentId::new();
    let target = protocol::AgentId::new();
    seed_agent(&registry, "a-target", target, None, TaskStatus::Running).await;
    seed_agent(&registry, "a-loop-a", a, Some(b), TaskStatus::Running).await;
    seed_agent(&registry, "a-loop-b", b, Some(a), TaskStatus::Running).await;
    registry
        .mark_task_rested(
            &"a-target".to_string(),
            None,
            None,
            Some(target),
            None,
            None,
        )
        .await;

    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        registry.kill_with_reason("a-target", "parent"),
    )
    .await
    .expect("the cycle guard must stop the walk")
    .unwrap();
}

#[tokio::test]
async fn named_rested_agent_waits_for_live_background_children_before_notifying() {
    use crate::state::{LocalAgentTaskState, LocalBashTaskState, TaskState, TaskStateBase};
    let (_d, registry) = make_registry();

    registry
        .insert_state_for_test(TaskState::LocalAgent(LocalAgentTaskState {
            base: TaskStateBase {
                id: "a-rest-parent".into(),
                task_type: TaskType::LocalAgent,
                status: TaskStatus::Running,
                description: "bg agent".into(),
                tool_use_id: Some("toolu_parent".into()),
                start_time: SystemTime::now(),
                end_time: None,
                total_paused_ms: 0,
                output_file: std::path::PathBuf::from("/tmp/tasks/a-rest-parent.output"),
                evict_after: None,
                output_offset: 0,
                notified: false,
                creator_teammate_name: None,
                creator_team_name: None,
                creator_agent_id: None,
            },
            agent_id: protocol::AgentId::nil(),
            subagent_type: String::new(),
            prompt: String::new(),
            error: None,
            messages: vec![],
            pending_messages: vec![],
            is_backgrounded: true,
            outcome: Default::default(),
            forked_skill_name: None,
        }))
        .await;
    registry
        .insert_state_for_test(TaskState::LocalBash(LocalBashTaskState {
            base: TaskStateBase {
                id: "b-child-live".into(),
                task_type: TaskType::LocalBash,
                status: TaskStatus::Running,
                description: "child".into(),
                tool_use_id: None,
                start_time: SystemTime::now(),
                end_time: None,
                total_paused_ms: 0,
                output_file: std::path::PathBuf::from("/tmp/tasks/b-child-live.output"),
                evict_after: None,
                output_offset: 0,
                notified: false,
                creator_teammate_name: Some("reviewer".into()),
                creator_team_name: Some("alpha".into()),
                creator_agent_id: None,
            },
            command: "sleep 1".into(),
            pid: None,
            exit_code: None,
            cwd: None,
            is_backgrounded: None,
        }))
        .await;

    registry
        .mark_task_rested(
            "a-rest-parent",
            Some("rested".into()),
            Some(platform_api::task_registry::AgentRunUsage {
                subagent_tokens: 7,
                tool_uses: 1,
                duration_ms: 99,
            }),
            None,
            Some("reviewer".into()),
            Some("alpha".into()),
        )
        .await;

    assert!(
        registry.take_pending_task_notifications().await.is_empty(),
        "a rested agent stays quiet while it still owns live background children"
    );

    registry
        .set_status("b-child-live", TaskStatus::Completed)
        .await
        .expect("child terminal transition should succeed");

    let drained = registry.take_pending_task_notifications().await;
    assert_eq!(
        drained.len(),
        2,
        "child terminal + deferred rest notification"
    );
    let rest = drained
        .iter()
        .find(|notification| notification.task_id == "a-rest-parent")
        .expect("deferred rest notification should surface");
    assert_eq!(rest.status, "completed");
    assert_eq!(rest.result.as_deref(), Some("rested"));
    assert_eq!(rest.usage.as_ref().map(|u| u.subagent_tokens), Some(7));
}

#[tokio::test]
async fn deferred_rest_requeue_preserves_newer_payload() {
    use crate::state::{LocalAgentTaskState, LocalBashTaskState, TaskState, TaskStateBase};
    let (_d, registry) = make_registry();

    registry
        .insert_state_for_test(TaskState::LocalAgent(LocalAgentTaskState {
            base: TaskStateBase {
                id: "a-rest-parent".into(),
                task_type: TaskType::LocalAgent,
                status: TaskStatus::Running,
                description: "bg agent".into(),
                tool_use_id: Some("toolu_parent".into()),
                start_time: SystemTime::now(),
                end_time: None,
                total_paused_ms: 0,
                output_file: std::path::PathBuf::from("/tmp/tasks/a-rest-parent.output"),
                evict_after: None,
                output_offset: 0,
                notified: false,
                creator_teammate_name: None,
                creator_team_name: None,
                creator_agent_id: None,
            },
            agent_id: protocol::AgentId::nil(),
            subagent_type: String::new(),
            prompt: String::new(),
            error: None,
            messages: vec![],
            pending_messages: vec![],
            is_backgrounded: true,
            outcome: Default::default(),
            forked_skill_name: None,
        }))
        .await;
    registry
        .insert_state_for_test(TaskState::LocalBash(LocalBashTaskState {
            base: TaskStateBase {
                id: "b-child-live".into(),
                task_type: TaskType::LocalBash,
                status: TaskStatus::Running,
                description: "child".into(),
                tool_use_id: None,
                start_time: SystemTime::now(),
                end_time: None,
                total_paused_ms: 0,
                output_file: std::path::PathBuf::from("/tmp/tasks/b-child-live.output"),
                evict_after: None,
                output_offset: 0,
                notified: false,
                creator_teammate_name: Some("reviewer".into()),
                creator_team_name: Some("alpha".into()),
                creator_agent_id: None,
            },
            command: "sleep 1".into(),
            pid: None,
            exit_code: None,
            cwd: None,
            is_backgrounded: None,
        }))
        .await;

    registry
        .mark_task_rested(
            "a-rest-parent",
            Some("stale".into()),
            Some(platform_api::task_registry::AgentRunUsage {
                subagent_tokens: 1,
                tool_uses: 1,
                duration_ms: 10,
            }),
            None,
            Some("reviewer".into()),
            Some("alpha".into()),
        )
        .await;
    let stale = registry
        .pending_rest
        .write()
        .await
        .remove("a-rest-parent")
        .expect("stale payload should be armed");

    registry
        .mark_task_rested(
            "a-rest-parent",
            Some("fresh".into()),
            Some(platform_api::task_registry::AgentRunUsage {
                subagent_tokens: 9,
                tool_uses: 2,
                duration_ms: 20,
            }),
            None,
            Some("reviewer".into()),
            Some("alpha".into()),
        )
        .await;
    registry
        .requeue_deferred_rest(vec![("a-rest-parent".into(), stale)])
        .await;

    registry
        .set_status("b-child-live", TaskStatus::Completed)
        .await
        .expect("child terminal transition should succeed");

    let drained = registry.take_pending_task_notifications().await;
    let rest = drained
        .iter()
        .find(|notification| notification.task_id == "a-rest-parent")
        .expect("fresh rest payload should survive stale requeue");
    assert_eq!(rest.result.as_deref(), Some("fresh"));
    assert_eq!(rest.usage.as_ref().map(|u| u.subagent_tokens), Some(9));
}

#[tokio::test]
async fn rested_agent_id_ignores_same_name_children_owned_by_someone_else() {
    use crate::state::{LocalAgentTaskState, LocalBashTaskState, TaskState, TaskStateBase};
    let (_d, registry) = make_registry();
    let owner_a = protocol::AgentId::new();
    let owner_b = protocol::AgentId::new();

    registry
        .insert_state_for_test(TaskState::LocalAgent(LocalAgentTaskState {
            base: TaskStateBase {
                id: "a-rest-a".into(),
                task_type: TaskType::LocalAgent,
                status: TaskStatus::Running,
                description: "bg agent a".into(),
                tool_use_id: Some("toolu_parent_a".into()),
                start_time: SystemTime::now(),
                end_time: None,
                total_paused_ms: 0,
                output_file: std::path::PathBuf::from("/tmp/tasks/a-rest-a.output"),
                evict_after: None,
                output_offset: 0,
                notified: false,
                creator_teammate_name: None,
                creator_team_name: None,
                creator_agent_id: None,
            },
            agent_id: owner_a,
            subagent_type: String::new(),
            prompt: String::new(),
            error: None,
            messages: vec![],
            pending_messages: vec![],
            is_backgrounded: true,
            outcome: Default::default(),
            forked_skill_name: None,
        }))
        .await;
    registry
        .insert_state_for_test(TaskState::LocalAgent(LocalAgentTaskState {
            base: TaskStateBase {
                id: "a-rest-b".into(),
                task_type: TaskType::LocalAgent,
                status: TaskStatus::Running,
                description: "bg agent b".into(),
                tool_use_id: Some("toolu_parent_b".into()),
                start_time: SystemTime::now(),
                end_time: None,
                total_paused_ms: 0,
                output_file: std::path::PathBuf::from("/tmp/tasks/a-rest-b.output"),
                evict_after: None,
                output_offset: 0,
                notified: false,
                creator_teammate_name: None,
                creator_team_name: None,
                creator_agent_id: None,
            },
            agent_id: owner_b,
            subagent_type: String::new(),
            prompt: String::new(),
            error: None,
            messages: vec![],
            pending_messages: vec![],
            is_backgrounded: true,
            outcome: Default::default(),
            forked_skill_name: None,
        }))
        .await;
    registry
        .insert_state_for_test(TaskState::LocalBash(LocalBashTaskState {
            base: TaskStateBase {
                id: "b-child-live".into(),
                task_type: TaskType::LocalBash,
                status: TaskStatus::Running,
                description: "child".into(),
                tool_use_id: None,
                start_time: SystemTime::now(),
                end_time: None,
                total_paused_ms: 0,
                output_file: std::path::PathBuf::from("/tmp/tasks/b-child-live.output"),
                evict_after: None,
                output_offset: 0,
                notified: false,
                creator_teammate_name: Some("reviewer".into()),
                creator_team_name: Some("alpha".into()),
                creator_agent_id: Some(owner_b),
            },
            command: "sleep 1".into(),
            pid: None,
            exit_code: None,
            cwd: None,
            is_backgrounded: None,
        }))
        .await;

    registry
        .mark_task_rested(
            "a-rest-a",
            Some("rested-a".into()),
            None,
            Some(owner_a),
            Some("reviewer".into()),
            Some("alpha".into()),
        )
        .await;
    registry
        .mark_task_rested(
            "a-rest-b",
            Some("rested-b".into()),
            None,
            Some(owner_b),
            Some("reviewer".into()),
            Some("alpha".into()),
        )
        .await;

    let drained = registry.take_pending_task_notifications().await;
    let rest_a = drained
        .iter()
        .find(|notification| notification.task_id == "a-rest-a")
        .expect("owner A should notify immediately");
    assert_eq!(rest_a.result.as_deref(), Some("rested-a"));
    assert!(
        drained
            .iter()
            .all(|notification| notification.task_id != "a-rest-b"),
        "owner B stays deferred while its own child is still live"
    );

    registry
        .set_status("b-child-live", TaskStatus::Completed)
        .await
        .expect("child terminal transition should succeed");
    let drained = registry.take_pending_task_notifications().await;
    let rest_b = drained
        .iter()
        .find(|notification| notification.task_id == "a-rest-b")
        .expect("owner B should notify after its child exits");
    assert_eq!(rest_b.result.as_deref(), Some("rested-b"));
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
            evict_after: None,
            output_offset: 0,
            notified: true, // already surfaced (e.g. via TaskOutput)
            creator_teammate_name: None,
            creator_team_name: None,
            creator_agent_id: None,
        };
        registry
            .insert_state_for_test(TaskState::LocalBash(LocalBashTaskState {
                base,
                command: String::new(),
                pid: None,
                exit_code: Some(0),
                cwd: None,
                is_backgrounded: None,
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

    // TID-04: the already-notified terminal row is EVICTED by the sweep this
    // drain runs first — it was notified by an earlier pass, which is exactly
    // the oracle's `Kan`/`Dlo` candidate. It used to survive forever, which is
    // what made the registry grow for the life of the session.
    assert!(
        registry.get("bnotified").await.is_none(),
        "an already-notified terminal row is evicted, not kept forever"
    );
    // The pending row is untouched: the sweep only ever considers terminal rows.
    assert!(registry.get(&pending).await.is_some(), "pending survives");
    // And the row this drain just notified survives THIS pass — eviction is
    // one-pass-delayed, so the model is never told about a task in the same
    // breath as the registry forgets it.
    assert!(
        registry.get(&fresh).await.is_some(),
        "a row notified by THIS drain is not evicted by THIS drain"
    );
}

// ---- M8 cc2.1.198: "Task panels: no stuck Running after finish" -----------

/// Minimal happy-path [`ProcessRunner`]: every `run()` succeeds with exit 0.
struct ExitZeroRunner;

#[async_trait]
impl platform_api::ProcessRunner for ExitZeroRunner {
    async fn run(
        &self,
        _cmd: &platform_api::SandboxedCommand,
    ) -> Result<platform_api::ProcessOutput, platform_api::ProcessError> {
        Ok(platform_api::ProcessOutput {
            stdout: "done\n".into(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        })
    }
    async fn spawn_background(
        &self,
        _cmd: &platform_api::SandboxedCommand,
    ) -> Result<platform_api::ProcessHandle, platform_api::ProcessError> {
        Err(platform_api::ProcessError::Unsupported)
    }
    async fn kill(
        &self,
        _handle: &platform_api::ProcessHandle,
    ) -> Result<(), platform_api::ProcessError> {
        Ok(())
    }
    fn is_available(&self) -> bool {
        true
    }
}

/// Pass-through [`platform_api::Sandbox`] stub (audited bypass tag, like the
/// local_bash unit tests').
struct PassSandbox;

#[async_trait]
impl platform_api::Sandbox for PassSandbox {
    fn is_available(&self) -> bool {
        true
    }
    fn backend(&self) -> platform_api::SandboxBackend {
        platform_api::SandboxBackend::None
    }
    fn prepare(
        &self,
        cmd: platform_api::ProcessCommand,
        _policy: &platform_api::SandboxPolicy,
    ) -> Result<platform_api::SandboxedCommand, platform_api::SandboxError> {
        Ok(platform_api::SandboxedCommand::__new_sandboxed(
            cmd,
            platform_api::SandboxedTag::BypassAuditedWithReason {
                reason: "test".into(),
            },
        ))
    }
    fn bypass_with_audit(
        &self,
        cmd: platform_api::ProcessCommand,
        reason: &str,
    ) -> platform_api::SandboxedCommand {
        platform_api::SandboxedCommand::__new_sandboxed(
            cmd,
            platform_api::SandboxedTag::BypassAuditedWithReason {
                reason: reason.into(),
            },
        )
    }
    async fn probe_capability(&self) -> platform_api::SandboxCapability {
        platform_api::SandboxCapability {
            available: true,
            reason: None,
            features: platform_api::SandboxFeatures::default(),
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
                as Arc<dyn platform_api::McpTransport>,
        )),
        bash_sink.clone() as Arc<dyn crate::handlers::TaskStatusSink>,
    );
    let registry = Arc::new(registry);
    bash_sink.bind(registry.clone());

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
    // MON-01: the notification carries the result INLINE and the fields `F`
    // needs. The spool copy above is for `TaskOutput`; a renderer cannot open a
    // file, so the text has to ride on the notification too.
    assert_eq!(
        n.result.as_deref(),
        Some("clean working tree"),
        "the settled text rides on the notification"
    );
    let meta = n.mcp.as_ref().expect("an mcp_task notification carries its meta");
    assert_eq!(meta.server_name, "git");
    assert_eq!(meta.tool_name, "status");
    assert_eq!(meta.mcp_status, "completed");
    assert_eq!(meta.status_message, None);
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
        TaskState::McpTask(m) => {
            assert_eq!(m.mcp_status, "failed");
            // A FAILED call renders from `statusMessage`, not from the text
            // (`F`'s `status==="completed" ? resultText : …`), so stashing it
            // would put it where nothing reads it and leave a failed row
            // looking like it had an answer.
            assert_eq!(
                m.result_text, None,
                "a failed settle must not stash a result body"
            );
        }
        other => panic!("expected McpTask, got {other:?}"),
    }
    let drained = registry.take_pending_task_notifications().await;
    assert_eq!(drained[0].result, None, "and it must not reach the notification");
    assert_eq!(
        drained[0].mcp.as_ref().map(|m| m.mcp_status.as_str()),
        Some("failed")
    );
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
    let registry =
        Arc::new(TaskRegistry::new(runtime, fs, out_mgr).with_task_completed_firer(firer.clone()));

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

#[tokio::test]
async fn plan_review_flag_preserves_task_lifecycle_and_clears_on_terminal() {
    let (_tmp, registry) = make_registry();
    let id = registry
        .create(
            TaskType::InProcessTeammate,
            teammate_input(),
            "plan work".into(),
        )
        .await
        .unwrap();
    registry.set_status(&id, TaskStatus::Paused).await.unwrap();
    registry
        .set_awaiting_plan_approval(&id, true)
        .await
        .unwrap();
    let state = registry.get(&id).await.unwrap();
    assert_eq!(state.base().status, TaskStatus::Paused);
    assert!(
        matches!(state,TaskState::InProcessTeammate(ref teammate) if teammate.awaiting_plan_approval)
    );
    registry
        .set_awaiting_plan_approval(&id, false)
        .await
        .unwrap();
    assert_eq!(
        registry.get(&id).await.unwrap().base().status,
        TaskStatus::Paused
    );
    registry
        .set_awaiting_plan_approval(&id, true)
        .await
        .unwrap();
    registry.set_status(&id, TaskStatus::Killed).await.unwrap();
    registry
        .set_awaiting_plan_approval(&id, true)
        .await
        .unwrap();
    assert!(
        matches!(registry.get(&id).await.unwrap(),TaskState::InProcessTeammate(teammate) if !teammate.awaiting_plan_approval)
    );
}

#[tokio::test]
async fn teammate_idle_sink_projects_idle_and_wake_without_ending_task() {
    use crate::handlers::TaskStatusSink;
    use crate::registry_status_sink::RegistryStatusSink;
    use platform_api::task_registry::TaskRegistryHandle;

    let (_tmp, registry) = make_registry();
    let registry = Arc::new(registry);
    let id = registry
        .create(TaskType::InProcessTeammate, teammate_input(), "work".into())
        .await
        .unwrap();
    let sink = RegistryStatusSink::new();
    sink.bind(registry.clone());
    sink.set_status(&id, TaskStatus::Running).await;
    assert!(
        !TaskRegistryHandle::get(registry.as_ref(), &id)
            .await
            .unwrap()
            .unwrap()
            .is_idle
    );
    sink.set_teammate_idle(&id).await;
    let idle = TaskRegistryHandle::get(registry.as_ref(), &id)
        .await
        .unwrap()
        .unwrap();
    assert!(idle.is_idle);
    assert_eq!(idle.status, "running");
    sink.set_status(&id, TaskStatus::Running).await;
    assert!(
        !TaskRegistryHandle::get(registry.as_ref(), &id)
            .await
            .unwrap()
            .unwrap()
            .is_idle
    );
    sink.set_status(&id, TaskStatus::Killed).await;
    sink.set_teammate_idle(&id).await;
    let killed = TaskRegistryHandle::get(registry.as_ref(), &id)
        .await
        .unwrap()
        .unwrap();
    assert!(!killed.is_idle);
    assert_eq!(killed.status, "killed");
}

//! `BackgroundAgentSpawner` — the composition-root decorator that makes the
//! `run_in_background` (async-agent) path LIVE end-to-end.
//!
//! `AgentTool::dispatch_async` already calls [`SubagentSpawner::spawn_async`]
//! and renders claude-code's byte-faithful `async_launched` payload; the
//! production [`agent::PoolSubagentSpawner`] leaves `spawn_async` unwired (a
//! clear error). This decorator wraps that spawner, delegates every sync method
//! verbatim, and overrides ONLY `spawn_async` to wire the full
//! `registerAsyncAgent` lifecycle by REUSING the machinery the coordinator
//! teammate already uses — no redesign:
//!
//! 1. `registry.spawn(LocalAgent{is_backgrounded:true})` → the persistent
//!    [`tasks::handlers::LocalAgentHandler`] worker (parks "comes to rest" after
//!    each turn-set, resumable via `send_message`). Returns the `task_id`.
//! 2. Register a [`TeammateMailbox`] for the advertised `AgentId` on the shared
//!    [`MailboxRouter`] (+ the optional `name`), so a `SendMessage({to})`
//!    resolves it.
//! 3. Start a [`coordinator::run_teammate_pump`] that drains that mailbox into
//!    `registry.send_message(task_id)` (the registry's [`TeamSpawnSeam`] impl →
//!    `LocalAgentHandler::send_message` → `pool.send_event`). This is the
//!    `injectUserMessageToTeammate` bridge.
//!
//! Routing chain: `SendMessage(agent_id|name)` → `MailboxRouter.route` →
//! mailbox → pump → `registry.send_message(task_id)` → handler resume →
//! `pool.send_event(agent_id)`. The pool routes by its OWN agent id; the
//! advertised `AgentId` is purely the mailbox key, consistent end-to-end.

use std::sync::Arc;

use async_trait::async_trait;
use coordinator::mailbox::{MailboxRouter, TeammateMailbox};
use coordinator::run_teammate_pump;
use protocol::AgentId;
use tasks::registry::TaskRegistry;
use tasks::task_trait::TaskSpawnInput;
use tasks::TaskType;
use traits::subagent_spawn::{
    AsyncLaunch, SelectedAgentMeta, SubagentInheritance, SubagentListingEntry, SubagentResult,
    SubagentSpawnError, SubagentSpawnRequest, SubagentSpawner,
};
use traits::team_spawn::TeamSpawnSeam;
use traits::RuntimeSpawner;

/// Decorator that wires `spawn_async` (the `run_in_background` path) while
/// delegating every synchronous `SubagentSpawner` method to `inner`.
pub struct BackgroundAgentSpawner {
    /// The wrapped production spawner — every sync method delegates here.
    pub inner: Arc<dyn SubagentSpawner>,
    /// The concrete registry: `spawn(LocalAgent)` dispatches to the persistent
    /// handler, and it doubles as the [`TeamSpawnSeam`] the pump calls back.
    pub registry: Arc<TaskRegistry>,
    /// The process-wide mailbox router shared with the `SendMessage` tool.
    pub mailbox_router: Arc<MailboxRouter>,
    /// D17-safe task spawner for the per-agent mailbox→runner pump.
    pub runtime: Arc<dyn RuntimeSpawner>,
    /// This session's `subagents/` directory — where every background agent's
    /// transcript lives, and therefore where a forked skill's scoping sidecars
    /// are written (beside `agent-<id>.jsonl`).
    /// `None` disables fork persistence (a host with no session storage); a
    /// forking skill then still launches, and a later resume refuses it because
    /// its task record names a skill with no scoping record on disk.
    pub subagents_dir: Option<std::path::PathBuf>,
}

impl BackgroundAgentSpawner {
    /// Persist a forked skill's scoping sidecars for `agent_id`, BEFORE the
    /// agent is spawned (claude `qdd(Gc(e), A)` precedes the task creation).
    ///
    /// Order matters: an agent that exists without its scoping on disk is one
    /// a later resume must refuse, so the record lands first. The write is
    /// keyed on the AGENT's own session path, not the parent's — each
    /// background agent gets its own transcript.
    ///
    /// Returns `Err` when the record cannot be persisted; the caller aborts the
    /// launch rather than producing an unresumable agent.
    async fn persist_fork_scoping(
        &self,
        agent_id: AgentId,
        request: &SubagentSpawnRequest,
    ) -> Result<(), SubagentSpawnError> {
        let Some(skill_name) = request.forked_skill_name.as_deref() else {
            return Ok(());
        };
        let Some(dir) = self.subagents_dir.as_ref() else {
            return Ok(());
        };
        let scoping = session::forked_skill::ForkedSkillScoping {
            skill_name: skill_name.to_string(),
            attribution_name: request
                .forked_skill_attribution
                .clone()
                .unwrap_or_else(|| skill_name.to_string()),
            effort: request
                .forked_skill_effort
                .clone()
                .map(session::forked_skill::Effort::Level),
            // An EMPTY list writes no key (claude gates the spread on
            // `f.length > 0`), so it must not become `"frozenCommandDenies":[]`.
            frozen_command_denies: (!request.frozen_command_denies.is_empty())
                .then(|| request.frozen_command_denies.clone()),
        };
        // Re-validate at the write boundary. The Skill tool already checked, but
        // this is the last point before bytes hit disk, and a record that fails
        // the schema reads back as `Malformed` — a resume REFUSAL, not "no
        // scoping" — so writing one would strand the agent.
        if !scoping.is_valid() {
            return Err(SubagentSpawnError::Runtime(
                "forked-skill scoping record is unpersistable".to_string(),
            ));
        }
        let jsonl = session::forked_skill::agent_transcript_path(dir, &agent_id.to_string());
        session::forked_skill::write_fork_records(&jsonl, &scoping)
            .await
            .map_err(|e| {
                SubagentSpawnError::Runtime(format!("failed to persist forked-skill scoping: {e}"))
            })
    }

    /// Shared async launch body. Fresh launches mint an id; cold restores pass
    /// the persisted one so transcript, mailbox, and parked-row identities stay
    /// stable across the process boundary.
    async fn spawn_async_with_id(
        &self,
        agent_id: AgentId,
        request: SubagentSpawnRequest,
        inherit: SubagentInheritance,
    ) -> Result<AsyncLaunch, SubagentSpawnError> {
        let description = request.description.clone().unwrap_or_default();

        // A forked skill's permission scoping lands on disk before the worker.
        // Rewriting the same validated sidecars during restore is idempotent and
        // keeps the crash-safety ordering identical to a fresh launch.
        self.persist_fork_scoping(agent_id, &request).await?;

        let task_id = self
            .registry
            .spawn(
                TaskType::LocalAgent,
                TaskSpawnInput::LocalAgent {
                    agent_id,
                    subagent_type: request.subagent_type.clone(),
                    prompt: request.prompt.clone(),
                    is_backgrounded: true,
                    tool_use_id: request.tool_use_id.clone(),
                    creator_teammate_name: request.creator_teammate_name.clone(),
                    creator_team_name: request.creator_team_name.clone(),
                    creator_agent_id: request.creator_agent_id,
                    spawn_request: Some(request.clone()),
                    inheritance: Some(inherit),
                },
                description,
            )
            .await
            .map_err(|e| SubagentSpawnError::Runtime(e.to_string()))?;

        let mailbox = Arc::new(TeammateMailbox::new(agent_id));
        self.mailbox_router
            .register(agent_id, mailbox.clone())
            .await;
        if let Some(name) = request.name.as_deref() {
            self.mailbox_router.register_name(name, agent_id).await;
        }

        let seam: Arc<dyn TeamSpawnSeam> = self.registry.clone();
        let router = self.mailbox_router.clone();
        let pump_task_id = task_id.clone();
        let pump = Box::pin(async move {
            run_teammate_pump(mailbox, pump_task_id, seam).await;
            router.unregister(&agent_id).await;
        });
        if let Err(e) = self.runtime.spawn("bg-agent-pump", pump).await {
            self.mailbox_router.unregister(&agent_id).await;
            let _ = self.registry.kill(&task_id).await;
            return Err(SubagentSpawnError::Runtime(format!(
                "failed to start background agent pump: {e}"
            )));
        }

        let output_file = self
            .registry
            .output_manager
            .path_for(&task_id)
            .map(|p| p.to_string_lossy().into_owned())
            .map_err(|e| SubagentSpawnError::Runtime(e.to_string()))?;

        Ok(AsyncLaunch {
            agent_id,
            output_file,
        })
    }
}

#[async_trait]
impl SubagentSpawner for BackgroundAgentSpawner {
    async fn spawn(
        &self,
        request: SubagentSpawnRequest,
        inherit: SubagentInheritance,
    ) -> Result<SubagentResult, SubagentSpawnError> {
        self.inner.spawn(request, inherit).await
    }

    async fn agent_listing(&self) -> Vec<SubagentListingEntry> {
        self.inner.agent_listing().await
    }

    async fn resolve_required_mcp_servers(&self, subagent_type: &str) -> Vec<String> {
        self.inner.resolve_required_mcp_servers(subagent_type).await
    }

    async fn resolve_selection(
        &self,
        subagent_type: &str,
        model: Option<&str>,
    ) -> SelectedAgentMeta {
        self.inner.resolve_selection(subagent_type, model).await
    }

    async fn register_name(&self, name: &str, agent_id: AgentId) {
        self.inner.register_name(name, agent_id).await
    }

    async fn resolve_name(&self, name: &str) -> Option<AgentId> {
        self.inner.resolve_name(name).await
    }

    async fn spawn_async(
        &self,
        request: SubagentSpawnRequest,
        inherit: SubagentInheritance,
    ) -> Result<AsyncLaunch, SubagentSpawnError> {
        self.spawn_async_with_id(AgentId::new(), request, inherit)
            .await
    }

    async fn restore_async(
        &self,
        agent_id: AgentId,
        request: SubagentSpawnRequest,
        inherit: SubagentInheritance,
    ) -> Result<AsyncLaunch, SubagentSpawnError> {
        self.spawn_async_with_id(agent_id, request, inherit).await
    }

    async fn concurrent_subagent_count(&self) -> usize {
        self.inner.concurrent_subagent_count().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::any::Any;
    use std::collections::HashMap;
    use std::future::Future;
    use std::path::PathBuf;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use std::sync::Mutex as StdMutex;

    use platform_posix::PosixFileSystem;
    use serde_json::json;
    use tasks::output_manager::TaskOutputManager;
    use tasks::task_trait::{Task, TaskContext, TaskError, TaskHandle};
    use tasks::TaskType;
    use test_harness::mocks::MockRuntimeSpawner;
    use tokio::task::JoinHandle;
    use traits::budget::{BudgetEnforcerHandle, BudgetError};
    use traits::tool_invoker::{SubagentInvocationContext, ToolInvoker, ToolInvokerError};
    use traits::{BackgroundTaskHandle, RuntimeError};

    /// Records the `is_backgrounded` flag the decorator spawned with, returning
    /// a fixed task id (the spool path is pure `path_for`, so no I/O needed).
    /// Reports `supports_messages` and answers `send_message` with a scripted
    /// [`TaskError`] so a test can drive the pump's terminal-stop path.
    struct RecordingHandler {
        seen_backgrounded: Arc<StdMutex<Option<bool>>>,
        killed_ids: Arc<StdMutex<Vec<String>>>,
        /// When `true`, `send_message` returns [`TaskError::TerminatedTask`]
        /// (the "runner is gone" signal) so a delivered message drives the pump
        /// to stop → the decorator unregisters the mailbox.
        terminate_on_message: bool,
    }
    #[async_trait]
    impl Task for RecordingHandler {
        fn name(&self) -> &str {
            "recording"
        }
        fn task_type(&self) -> TaskType {
            TaskType::LocalAgent
        }
        async fn spawn(
            &self,
            input: TaskSpawnInput,
            _ctx: TaskContext,
        ) -> Result<TaskHandle, TaskError> {
            if let TaskSpawnInput::LocalAgent {
                is_backgrounded, ..
            } = input
            {
                *self.seen_backgrounded.lock().unwrap() = Some(is_backgrounded);
            }
            Ok(TaskHandle::new("a-bg-test-1", None))
        }
        async fn kill(&self, task_id: &str, _ctx: TaskContext) -> Result<(), TaskError> {
            self.killed_ids.lock().unwrap().push(task_id.to_string());
            Ok(())
        }
        fn supports_messages(&self) -> bool {
            self.terminate_on_message
        }
        async fn send_message(
            &self,
            _task_id: &str,
            _message: String,
            _ctx: TaskContext,
        ) -> Result<(), TaskError> {
            if self.terminate_on_message {
                Err(TaskError::TerminatedTask)
            } else {
                Ok(())
            }
        }
    }

    /// Fails only the mailbox-pump spawn while otherwise behaving like the
    /// tokio-backed mock runtime.
    struct FailingPumpRuntime {
        next_id: AtomicU64,
        handles: StdMutex<HashMap<u64, JoinHandle<()>>>,
    }

    impl Default for FailingPumpRuntime {
        fn default() -> Self {
            Self {
                next_id: AtomicU64::new(1),
                handles: StdMutex::new(HashMap::new()),
            }
        }
    }

    #[async_trait]
    impl RuntimeSpawner for FailingPumpRuntime {
        async fn spawn(
            &self,
            name: &str,
            task: Pin<Box<dyn Future<Output = ()> + Send + 'static>>,
        ) -> Result<BackgroundTaskHandle, RuntimeError> {
            if name == "bg-agent-pump" {
                return Err(RuntimeError::Internal("pump spawn failed".into()));
            }
            let id = self.next_id.fetch_add(1, Ordering::SeqCst);
            let handle = tokio::spawn(task);
            self.handles.lock().unwrap().insert(id, handle);
            Ok(BackgroundTaskHandle {
                task_name: name.to_string(),
                task_id: id,
            })
        }

        async fn sleep(&self, duration: std::time::Duration) {
            tokio::time::sleep(duration).await;
        }

        async fn cancel(&self, handle: &BackgroundTaskHandle) -> Result<(), RuntimeError> {
            let task = self.handles.lock().unwrap().remove(&handle.task_id);
            if let Some(task) = task {
                task.abort();
                Ok(())
            } else {
                Err(RuntimeError::NotFound(handle.task_name.clone()))
            }
        }
    }

    /// The wrapped spawner is never called on the async path — `spawn` only has
    /// to type-check (the decorator delegates the SYNC path, untested here).
    struct InertSpawner;
    #[async_trait]
    impl SubagentSpawner for InertSpawner {
        async fn spawn(
            &self,
            _request: SubagentSpawnRequest,
            _inherit: SubagentInheritance,
        ) -> Result<SubagentResult, SubagentSpawnError> {
            Err(SubagentSpawnError::Internal("inert".into()))
        }
    }

    struct CountingSpawner {
        count: usize,
    }

    #[async_trait]
    impl SubagentSpawner for CountingSpawner {
        async fn spawn(
            &self,
            _request: SubagentSpawnRequest,
            _inherit: SubagentInheritance,
        ) -> Result<SubagentResult, SubagentSpawnError> {
            Err(SubagentSpawnError::Internal("counting".into()))
        }

        async fn concurrent_subagent_count(&self) -> usize {
            self.count
        }
    }

    struct MockInvoker;
    #[async_trait]
    impl ToolInvoker for MockInvoker {
        async fn invoke(
            &self,
            _name: &str,
            _input: serde_json::Value,
            _ctx: SubagentInvocationContext,
        ) -> Result<serde_json::Value, ToolInvokerError> {
            Ok(json!(null))
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    struct MockBudget;
    #[async_trait]
    impl BudgetEnforcerHandle for MockBudget {
        async fn check_and_charge(&self, _nano_usd: u64) -> Result<(), BudgetError> {
            Ok(())
        }
        async fn snapshot_total_nano_usd(&self) -> u64 {
            0
        }
    }

    fn request(name: Option<&str>) -> SubagentSpawnRequest {
        SubagentSpawnRequest {
            subagent_type: "general-purpose".into(),
            prompt: "go".into(),
            observer: None,
            context_paths: vec![],
            description: Some("a bg agent".into()),
            model: None,
            model_profile: None,
            run_in_background: true,
            name: name.map(str::to_string),
            team_name: None,
            creator_teammate_name: None,
            creator_team_name: None,
            creator_agent_id: None,
            mode: None,
            isolation: None,
            cwd: None,
            worktree: None,
            fork_context_messages: None,
            fork_parent_system_prompt: None,
            schema: None,
            effort: None,
            tool_use_id: None,
            system_prompt_override: None,
            system_prompt_addendum: None,
            additional_disallowed_tools: Vec::new(),
            depth: 0,
            parent_model_override: None,
            forked_skill_name: None,
            forked_skill_attribution: None,
            forked_skill_effort: None,
            frozen_command_denies: Vec::new(),
            resumed_history: None,
        }
    }

    /// `spawn_async` spawns a BACKGROUNDED LocalAgent through the registry,
    /// registers a mailbox (+ name) for the advertised AgentId, and returns a
    /// well-formed `AsyncLaunch` whose `output_file` is the task's spool path.
    #[tokio::test]
    async fn spawn_async_spawns_backgrounded_registers_mailbox_and_returns_launch() {
        let runtime: Arc<dyn RuntimeSpawner> = Arc::new(MockRuntimeSpawner::default());
        let dir = tempfile::tempdir().unwrap();
        let fs = Arc::new(PosixFileSystem::new(PathBuf::from(dir.path())));
        let output_manager = Arc::new(TaskOutputManager::new(
            PathBuf::from(dir.path()),
            fs.clone(),
        ));
        let mut reg = TaskRegistry::new(runtime.clone(), fs, output_manager);
        let seen = Arc::new(StdMutex::new(None));
        reg.register_handler(
            TaskType::LocalAgent,
            Arc::new(RecordingHandler {
                seen_backgrounded: seen.clone(),
                killed_ids: Arc::new(StdMutex::new(Vec::new())),
                terminate_on_message: false,
            }),
        );
        let registry = Arc::new(reg);
        let mailbox_router = Arc::new(MailboxRouter::new());

        let deco = BackgroundAgentSpawner {
            inner: Arc::new(InertSpawner),
            registry,
            mailbox_router: mailbox_router.clone(),
            runtime,
            subagents_dir: None,
        };
        let inherit = SubagentInheritance {
            tool_invoker: Arc::new(MockInvoker),
            budget: Arc::new(MockBudget),
        };

        let launch = deco
            .spawn_async(request(Some("bg1")), inherit)
            .await
            .expect("spawn_async should succeed");

        // 1. Spawned BACKGROUNDED (the persistent handler path).
        assert_eq!(
            *seen.lock().unwrap(),
            Some(true),
            "registry.spawn carried is_backgrounded=true"
        );
        // 2. A mailbox is registered for the advertised AgentId + the name,
        //    so a `SendMessage({to})` resolves it.
        assert!(
            mailbox_router.get(&launch.agent_id).await.is_some(),
            "mailbox registered for the launched AgentId"
        );
        assert_eq!(
            mailbox_router.resolve_name("bg1").await,
            Some(launch.agent_id),
            "name → AgentId registered for SendMessage(to: name)"
        );
        // 3. The launch advertises the task's spool path.
        assert!(
            launch.output_file.contains("a-bg-test-1"),
            "output_file is the spawned task's spool path: {}",
            launch.output_file
        );
    }

    #[tokio::test]
    async fn concurrent_subagent_count_delegates_to_inner_spawner() {
        let runtime: Arc<dyn RuntimeSpawner> = Arc::new(MockRuntimeSpawner::default());
        let dir = tempfile::tempdir().unwrap();
        let fs = Arc::new(PosixFileSystem::new(PathBuf::from(dir.path())));
        let output_manager = Arc::new(TaskOutputManager::new(
            PathBuf::from(dir.path()),
            fs.clone(),
        ));

        let deco = BackgroundAgentSpawner {
            inner: Arc::new(CountingSpawner { count: 7 }),
            registry: Arc::new(TaskRegistry::new(runtime.clone(), fs, output_manager)),
            mailbox_router: Arc::new(MailboxRouter::new()),
            runtime,
            subagents_dir: None,
        };

        assert_eq!(deco.concurrent_subagent_count().await, 7);
    }

    /// A `context: fork` skill's permission scoping is persisted beside the new
    /// agent's own transcript, and it lands BEFORE the agent exists — an agent
    /// running without a scoping record is one the resume gate must refuse.
    #[tokio::test]
    async fn spawn_async_persists_forked_skill_scoping_beside_the_agent_transcript() {
        let runtime: Arc<dyn RuntimeSpawner> = Arc::new(MockRuntimeSpawner::default());
        let dir = tempfile::tempdir().unwrap();
        let sessions = tempfile::tempdir().unwrap();
        let fs = Arc::new(PosixFileSystem::new(PathBuf::from(dir.path())));
        let output_manager = Arc::new(TaskOutputManager::new(
            PathBuf::from(dir.path()),
            fs.clone(),
        ));
        let mut reg = TaskRegistry::new(runtime.clone(), fs, output_manager);
        reg.register_handler(
            TaskType::LocalAgent,
            Arc::new(RecordingHandler {
                seen_backgrounded: Arc::new(StdMutex::new(None)),
                killed_ids: Arc::new(StdMutex::new(Vec::new())),
                terminate_on_message: false,
            }),
        );
        let deco = BackgroundAgentSpawner {
            inner: Arc::new(InertSpawner),
            registry: Arc::new(reg),
            mailbox_router: Arc::new(MailboxRouter::new()),
            runtime,
            subagents_dir: Some(sessions.path().to_path_buf()),
        };

        let mut req = request(None);
        req.forked_skill_name = Some("code-review".into());
        req.forked_skill_attribution = Some("reviewer".into());
        req.forked_skill_effort = Some("high".into());
        req.frozen_command_denies = vec!["Bash(rm:*)".into()];

        let launch = deco
            .spawn_async(
                req,
                SubagentInheritance {
                    tool_invoker: Arc::new(MockInvoker),
                    budget: Arc::new(MockBudget),
                },
            )
            .await
            .expect("spawn_async should succeed");

        let jsonl = session::forked_skill::agent_transcript_path(
            sessions.path(),
            &launch.agent_id.to_string(),
        );
        match session::forked_skill::read_scoping(&jsonl).await {
            session::forked_skill::ScopingStatus::Valid(s) => {
                assert_eq!(s.skill_name, "code-review");
                assert_eq!(s.attribution_name, "reviewer");
                assert_eq!(
                    s.effort,
                    Some(session::forked_skill::Effort::Level("high".into()))
                );
                assert_eq!(
                    s.frozen_command_denies.as_deref(),
                    Some(["Bash(rm:*)".to_string()].as_slice())
                );
            }
            other => panic!("expected a valid scoping record, got {other:?}"),
        }
        // The provenance marker witnesses the fork identity, so a later DELETION
        // of the scoping record is a refusal rather than an unscoped resume.
        assert_eq!(
            session::forked_skill::read_marker_skill_name(&jsonl).await,
            Some("code-review".to_string())
        );
    }

    /// A non-fork spawn writes nothing — the sidecars exist only for skills
    /// whose scoping cannot be recovered from the transcript.
    #[tokio::test]
    async fn spawn_async_writes_no_sidecars_for_an_ordinary_agent() {
        let runtime: Arc<dyn RuntimeSpawner> = Arc::new(MockRuntimeSpawner::default());
        let dir = tempfile::tempdir().unwrap();
        let sessions = tempfile::tempdir().unwrap();
        let fs = Arc::new(PosixFileSystem::new(PathBuf::from(dir.path())));
        let output_manager = Arc::new(TaskOutputManager::new(
            PathBuf::from(dir.path()),
            fs.clone(),
        ));
        let mut reg = TaskRegistry::new(runtime.clone(), fs, output_manager);
        reg.register_handler(
            TaskType::LocalAgent,
            Arc::new(RecordingHandler {
                seen_backgrounded: Arc::new(StdMutex::new(None)),
                killed_ids: Arc::new(StdMutex::new(Vec::new())),
                terminate_on_message: false,
            }),
        );
        let deco = BackgroundAgentSpawner {
            inner: Arc::new(InertSpawner),
            registry: Arc::new(reg),
            mailbox_router: Arc::new(MailboxRouter::new()),
            runtime,
            subagents_dir: Some(sessions.path().to_path_buf()),
        };

        let launch = deco
            .spawn_async(
                request(None),
                SubagentInheritance {
                    tool_invoker: Arc::new(MockInvoker),
                    budget: Arc::new(MockBudget),
                },
            )
            .await
            .unwrap();

        let jsonl = session::forked_skill::agent_transcript_path(
            sessions.path(),
            &launch.agent_id.to_string(),
        );
        assert_eq!(
            session::forked_skill::read_scoping(&jsonl).await,
            session::forked_skill::ScopingStatus::Absent
        );
    }

    /// When the backgrounded agent terminates, the pump stops and the decorator
    /// UNREGISTERS its mailbox + name index — so a later `SendMessage` resolves
    /// to "not found" instead of silently queueing into an undrained inbox
    /// (claude-code tears down async-agent state on termination). Here a
    /// delivered message maps to `Terminated`, driving the pump to stop.
    #[tokio::test]
    async fn spawn_async_unregisters_mailbox_when_agent_terminates() {
        use coordinator::mailbox::{MessageSender, TeammateMessage};

        let runtime: Arc<dyn RuntimeSpawner> = Arc::new(MockRuntimeSpawner::default());
        let dir = tempfile::tempdir().unwrap();
        let fs = Arc::new(PosixFileSystem::new(PathBuf::from(dir.path())));
        let output_manager = Arc::new(TaskOutputManager::new(
            PathBuf::from(dir.path()),
            fs.clone(),
        ));
        let mut reg = TaskRegistry::new(runtime.clone(), fs, output_manager);
        let seen = Arc::new(StdMutex::new(None));
        reg.register_handler(
            TaskType::LocalAgent,
            Arc::new(RecordingHandler {
                seen_backgrounded: seen.clone(),
                killed_ids: Arc::new(StdMutex::new(Vec::new())),
                // A delivered message ⇒ TerminatedTask ⇒ the pump stops.
                terminate_on_message: true,
            }),
        );
        let registry = Arc::new(reg);
        let mailbox_router = Arc::new(MailboxRouter::new());

        let deco = BackgroundAgentSpawner {
            inner: Arc::new(InertSpawner),
            registry,
            mailbox_router: mailbox_router.clone(),
            runtime,
            subagents_dir: None,
        };
        let inherit = SubagentInheritance {
            tool_invoker: Arc::new(MockInvoker),
            budget: Arc::new(MockBudget),
        };

        let launch = deco
            .spawn_async(request(Some("bg2")), inherit)
            .await
            .expect("spawn_async should succeed");

        // Precondition: the mailbox + name index are registered.
        assert!(mailbox_router.get(&launch.agent_id).await.is_some());
        assert_eq!(
            mailbox_router.resolve_name("bg2").await,
            Some(launch.agent_id)
        );

        // Deliver a message: the pump forwards it → TerminatedTask → the pump
        // stops → the wrapper unregisters the mailbox + name.
        mailbox_router
            .route(
                &launch.agent_id,
                TeammateMessage {
                    from: MessageSender::Coordinator,
                    from_name: "team-lead".to_string(),
                    content: "die".to_string(),
                    summary: None,
                    message_id: "m-1".to_string(),
                    timestamp: std::time::SystemTime::now(),
                    request_id: None,
                },
            )
            .await
            .expect("route delivers into the registered mailbox");

        // The pump runs on the mock runtime's tokio task; poll until the route
        // and name index are torn down.
        let mut gone = false;
        for _ in 0..400 {
            if mailbox_router.get(&launch.agent_id).await.is_none() {
                gone = true;
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(gone, "mailbox unregistered after the agent terminated");
        assert_eq!(
            mailbox_router.resolve_name("bg2").await,
            None,
            "name index cleared after the agent terminated"
        );
    }

    /// If the mailbox pump cannot be started, async launch must roll back: no
    /// success result, no lingering mailbox/name route, and the just-created
    /// task is killed immediately.
    #[tokio::test]
    async fn spawn_async_rolls_back_when_pump_spawn_fails() {
        let runtime: Arc<dyn RuntimeSpawner> = Arc::new(FailingPumpRuntime::default());
        let dir = tempfile::tempdir().unwrap();
        let fs = Arc::new(PosixFileSystem::new(PathBuf::from(dir.path())));
        let output_manager = Arc::new(TaskOutputManager::new(
            PathBuf::from(dir.path()),
            fs.clone(),
        ));
        let mut reg = TaskRegistry::new(runtime.clone(), fs, output_manager);
        let seen = Arc::new(StdMutex::new(None));
        let killed = Arc::new(StdMutex::new(Vec::new()));
        reg.register_handler(
            TaskType::LocalAgent,
            Arc::new(RecordingHandler {
                seen_backgrounded: seen.clone(),
                killed_ids: killed.clone(),
                terminate_on_message: false,
            }),
        );
        let registry = Arc::new(reg);
        let mailbox_router = Arc::new(MailboxRouter::new());

        let deco = BackgroundAgentSpawner {
            inner: Arc::new(InertSpawner),
            registry,
            mailbox_router: mailbox_router.clone(),
            runtime,
            subagents_dir: None,
        };
        let inherit = SubagentInheritance {
            tool_invoker: Arc::new(MockInvoker),
            budget: Arc::new(MockBudget),
        };

        let err = deco
            .spawn_async(request(Some("bg-fail")), inherit)
            .await
            .expect_err("pump spawn failure must roll back the async launch");

        assert!(
            err.to_string()
                .contains("failed to start background agent pump"),
            "error should explain the rollback cause: {err}"
        );
        assert_eq!(
            *seen.lock().unwrap(),
            Some(true),
            "the task was created before rollback"
        );
        assert!(
            mailbox_router.resolve_name("bg-fail").await.is_none(),
            "name route rolled back on failure"
        );
        assert!(
            killed.lock().unwrap().iter().any(|id| id == "a-bg-test-1"),
            "rollback must kill the just-created task"
        );
    }
}

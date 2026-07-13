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
        // The advertised id (the `SendMessage` routing key / mailbox key). The
        // handler's persistent runner uses its OWN pool agent id internally;
        // routing is mailbox→task_id→handler-map, so the two need not match.
        let agent_id = AgentId::new();
        let description = request.description.clone().unwrap_or_default();

        // 1. Spawn the BACKGROUNDED LocalAgent → the persistent handler worker.
        let task_id = self
            .registry
            .spawn(
                TaskType::LocalAgent,
                TaskSpawnInput::LocalAgent {
                    agent_id,
                    subagent_type: request.subagent_type.clone(),
                    prompt: request.prompt.clone(),
                    is_backgrounded: true,
                    // Stamp the originating tool_use_id so the task-notification
                    // carries `<tool-use-id>` (claude-code parity).
                    tool_use_id: request.tool_use_id.clone(),
                    // LocalAgent is a lifecycle wrapper, not a second spawn
                    // surface: retain every resolved Agent option verbatim.
                    spawn_request: Some(request.clone()),
                    // Preserve the immediate parent registry + budget Arcs so
                    // nested/background agents retain recursion and accounting
                    // semantics rather than falling back to root handles.
                    inheritance: Some(inherit),
                },
                description,
            )
            .await
            .map_err(|e| SubagentSpawnError::Runtime(e.to_string()))?;

        // 2. Register the mailbox (+ optional name) so SendMessage can reach it.
        let mailbox = Arc::new(TeammateMailbox::new(agent_id));
        self.mailbox_router
            .register(agent_id, mailbox.clone())
            .await;
        if let Some(name) = request.name.as_deref() {
            self.mailbox_router.register_name(name, agent_id).await;
        }

        // 3. Start the mailbox→runner pump (the injectUserMessageToTeammate
        //    bridge): drains the mailbox into registry.send_message(task_id).
        //    The pump now RETURNS once the backgrounded agent reaches a terminal
        //    state (via the seam's `is_alive` liveness re-check); wrap it so that
        //    on return the mailbox + name index are unregistered — otherwise a
        //    terminated agent leaks its route and a later `SendMessage` silently
        //    queues into an undrained inbox. This mirrors claude-code tearing
        //    down async-agent state on termination.
        let seam: Arc<dyn TeamSpawnSeam> = self.registry.clone();
        let router = self.mailbox_router.clone();
        let pump_task_id = task_id.clone();
        let pump = Box::pin(async move {
            run_teammate_pump(mailbox, pump_task_id, seam).await;
            router.unregister(&agent_id).await;
        });
        let _ = self.runtime.spawn("bg-agent-pump", pump).await;

        // The spool the handler already allocated (deterministic from task_id).
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::any::Any;
    use std::path::PathBuf;
    use std::sync::Mutex as StdMutex;

    use platform_posix::PosixFileSystem;
    use serde_json::json;
    use tasks::output_manager::TaskOutputManager;
    use tasks::task_trait::{Task, TaskContext, TaskError, TaskHandle};
    use tasks::TaskType;
    use test_harness::mocks::MockRuntimeSpawner;
    use traits::budget::{BudgetEnforcerHandle, BudgetError};
    use traits::tool_invoker::{SubagentInvocationContext, ToolInvoker, ToolInvokerError};

    /// Records the `is_backgrounded` flag the decorator spawned with, returning
    /// a fixed task id (the spool path is pure `path_for`, so no I/O needed).
    /// Reports `supports_messages` and answers `send_message` with a scripted
    /// [`TaskError`] so a test can drive the pump's terminal-stop path.
    struct RecordingHandler {
        seen_backgrounded: Arc<StdMutex<Option<bool>>>,
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
            Ok(TaskHandle {
                task_id: "a-bg-test-1".to_string(),
                cleanup: None,
            })
        }
        async fn kill(&self, _task_id: &str, _ctx: TaskContext) -> Result<(), TaskError> {
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
            context_paths: vec![],
            description: Some("a bg agent".into()),
            model: None,
            model_profile: None,
            run_in_background: true,
            name: name.map(str::to_string),
            team_name: None,
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
                    content: "die".to_string(),
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
}

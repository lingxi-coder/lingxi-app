//! Central registry tracking running tasks.
//!
//! The registry owns the in-memory map of task IDs to [`TaskState`] and to
//! the [`BackgroundTaskHandle`]s returned by the [`RuntimeSpawner`].

use crate::handlers::{
    DreamHandler, InProcessTeammateHandler, LocalAgentHandler, LocalBashHandler, MonitorMcpHandler,
};
use crate::id::{generate_task_id, TaskType};
use crate::output_manager::TaskOutputManager;
use crate::state::{TaskState, TaskStateBase, TaskStatus};
use crate::task_trait::{Task, TaskContext, TaskError, TaskSpawnInput};
use agent::{StateMachinePool, SubagentApiClient};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::RwLock;
use traits::team_spawn::{TeamSpawnError, TeamSpawnSeam};
use traits::{
    BackgroundTaskHandle, BudgetEnforcerHandle, FileSystem, ProcessRunner, RuntimeSpawner, Sandbox,
    SubagentSpawner, ToolInvoker,
};

/// Tracks running tasks and dispatches lifecycle operations to handlers.
pub struct TaskRegistry {
    tasks: Arc<RwLock<HashMap<String, TaskState>>>,
    handlers: HashMap<TaskType, Arc<dyn Task>>,
    handles: Arc<tokio::sync::Mutex<HashMap<String, BackgroundTaskHandle>>>,
    /// Handler-spawned task ids → their [`TaskType`], so [`Self::kill`] can
    /// dispatch teardown to the owning handler ([`Task::kill`]). Distinct from
    /// `handles`, which tracks the [`create`](Self::create) /
    /// [`RuntimeSpawner`]-cancel path; handler-spawned workers (e.g. the
    /// persistent `InProcessTeammate`) manage their own runtime task internally
    /// and must be torn down through the handler, not by cancelling a
    /// `BackgroundTaskHandle` the registry never holds.
    spawned: Arc<RwLock<HashMap<String, TaskType>>>,
    runtime: Arc<dyn RuntimeSpawner>,
    fs: Arc<dyn FileSystem>,
    /// Owner of task spool files.
    pub output_manager: Arc<TaskOutputManager>,
    /// Best-effort seam to fire the `TaskCompleted` hook when a task reaches a
    /// terminal status. `None` (the default) => strict no-op; the orchestrator
    /// injects a real firer via [`with_task_completed_firer`](Self::with_task_completed_firer).
    /// Mirrors the `TeamSpawnSeam` decoupling: the `tasks` leaf cannot reach a
    /// live hook executor, so it calls through this narrow trait instead.
    task_completed_firer: hooks::OptionalTaskCompletedFirer,
    /// Best-effort seam to fire the `TaskCreated` hook when a task is created.
    /// Counterpart to [`task_completed_firer`](Self::task_completed_firer):
    /// `None` (the default) => strict no-op; the orchestrator injects a real
    /// firer via [`with_task_created_firer`](Self::with_task_created_firer).
    task_created_firer: hooks::OptionalTaskCreatedFirer,
}

impl TaskRegistry {
    /// Construct an empty registry with no handlers yet registered.
    #[must_use]
    pub fn new(
        runtime: Arc<dyn RuntimeSpawner>,
        fs: Arc<dyn FileSystem>,
        output_manager: Arc<TaskOutputManager>,
    ) -> Self {
        Self {
            tasks: Arc::new(RwLock::new(HashMap::new())),
            handlers: HashMap::new(),
            handles: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            spawned: Arc::new(RwLock::new(HashMap::new())),
            runtime,
            fs,
            output_manager,
            task_completed_firer: None,
            task_created_firer: None,
        }
    }

    /// Inject the best-effort `TaskCompleted` hook firer. Default-`None`
    /// builder (the `RemoteTrigger`/seam pattern): existing `new()` callers and
    /// tests stay no-op; the composition root threads the orchestrator's firer
    /// here so a terminal status transition fires the `TaskCompleted` hook.
    #[must_use]
    pub fn with_task_completed_firer(
        mut self,
        firer: Arc<dyn hooks::TaskCompletedFirer>,
    ) -> Self {
        self.task_completed_firer = Some(firer);
        self
    }

    /// Inject the best-effort `TaskCreated` hook firer. Counterpart to
    /// [`with_task_completed_firer`](Self::with_task_completed_firer): existing
    /// `new()` callers and tests stay no-op; the composition root threads the
    /// orchestrator's firer here so creating a task fires the `TaskCreated`
    /// hook.
    #[must_use]
    pub fn with_task_created_firer(
        mut self,
        firer: Arc<dyn hooks::TaskCreatedFirer>,
    ) -> Self {
        self.task_created_firer = Some(firer);
        self
    }

    /// Register a per-type handler.
    pub fn register_handler(&mut self, task_type: TaskType, handler: Arc<dyn Task>) {
        self.handlers.insert(task_type, handler);
    }

    /// Create a new task entry. Allocates the output file and returns the
    /// generated task ID. Spawning the actual worker is done by handlers.
    pub async fn create(
        &self,
        task_type: TaskType,
        _input: TaskSpawnInput,
        description: String,
    ) -> Result<String, TaskError> {
        let id = generate_task_id(task_type);
        let path = self
            .output_manager
            .allocate(&id)
            .await
            .map_err(|e| TaskError::Io(e.to_string()))?;
        // Keep a copy of the description for the `TaskCreated` fire below — the
        // original is moved into `base` here.
        let description_for_hook = description.clone();
        let base = TaskStateBase {
            id: id.clone(),
            task_type,
            status: TaskStatus::Pending,
            description,
            tool_use_id: None,
            start_time: SystemTime::now(),
            end_time: None,
            total_paused_ms: 0,
            output_file: path,
            output_offset: 0,
            notified: false,
        };
        // Build a default state per type; production stores real fields.
        #[allow(clippy::match_same_arms)]
        let state = match task_type {
            TaskType::LocalBash => TaskState::LocalBash(crate::state::LocalBashTaskState {
                base,
                command: String::new(),
                pid: None,
                exit_code: None,
            }),
            TaskType::Dream => TaskState::Dream(crate::state::DreamTaskState {
                base,
                iteration_count: 0,
                max_iterations: None,
            }),
            _ => TaskState::LocalBash(crate::state::LocalBashTaskState {
                base,
                command: String::new(),
                pid: None,
                exit_code: None,
            }),
        };
        self.tasks.write().await.insert(id.clone(), state);
        // Best-effort `TaskCreated` fire (claude-code `executeTaskCreatedHooks`,
        // fired from `TaskCreateTool`). No-op when no firer is registered.
        self.fire_task_created(&id, task_type, &description_for_hook).await;
        Ok(id)
    }

    /// Best-effort `TaskCreated` hook fire for a newly-created task. Runs
    /// WITHOUT holding the registry lock (callers drop their write guard before
    /// invoking) so a slow/blocking hook never stalls other task operations.
    /// No-op when no firer is registered.
    ///
    /// Wire payload (`TaskCreatedHookInputSchema`): `task_subject` sources from
    /// the task's [`TaskType`] taxonomy bucket (the M-surface task state carries
    /// no distinct `subject` field), `task_description` from `description`.
    /// `teammate_name` / `team_name` are not stored on the task state, so they
    /// ride as `None` (the same documented gap as the `TaskCompleted` fire).
    async fn fire_task_created(&self, task_id: &str, task_type: TaskType, description: &str) {
        if let Some(firer) = &self.task_created_firer {
            firer
                .fire(hooks::TaskCreatedFire {
                    task_id: task_id.to_string(),
                    task_subject: format!("{task_type:?}"),
                    task_description: Some(description.to_string()),
                    teammate_name: None,
                    team_name: None,
                })
                .await;
        }
    }

    /// Spawn a task by dispatching to its registered per-type handler.
    ///
    /// Unlike [`create`](Self::create) — which only inserts a `Pending`
    /// placeholder row and never runs anything — `spawn` looks up the handler
    /// for `task_type`, runs [`Task::spawn`], and tracks the resulting task
    /// under the **handler-generated** `task_id` (e.g.
    /// `InProcessTeammateHandler` mints `t…` ids internally), which it returns.
    /// This is the load-bearing dispatch the coordinator's `TeamCreate` relies
    /// on to start a real worker.
    ///
    /// The typed [`TaskState`] is built from the real `input` fields (not the
    /// `AgentId::nil()` / empty-string placeholder `create` uses), and the
    /// task is recorded in the spawned-id index so [`kill`](Self::kill) can
    /// route teardown back to the owning handler.
    ///
    /// # Errors
    ///
    /// Returns [`TaskError::UnknownType`] if no handler is registered for
    /// `task_type`, [`TaskError::Io`] if the spool allocation fails, or
    /// whatever error the handler's [`Task::spawn`] surfaces.
    pub async fn spawn(
        &self,
        task_type: TaskType,
        input: TaskSpawnInput,
        description: String,
    ) -> Result<String, TaskError> {
        // 1. Look up the per-type handler first — a missing handler is a clean
        //    error before any side effects (spool allocation, state insert).
        let handler = self
            .handlers
            .get(&task_type)
            .ok_or(TaskError::UnknownType)?
            .clone();

        // 2. Dispatch to the handler. The handler allocates its OWN spool +
        //    `task_id` and starts the real worker; the returned id is the one
        //    callers (and `kill`) key on.
        let ctx = TaskContext {
            fs: self.fs.clone(),
            runtime: self.runtime.clone(),
        };
        let handle = handler.spawn(input.clone(), ctx).await?;
        let id = handle.task_id;

        // 3. Allocate a registry-visible spool path and insert the typed state
        //    built from the REAL input fields (not `create`'s placeholders), so
        //    `list()` / `get()` reflect a live spawned task.
        let path = self
            .output_manager
            .allocate(&id)
            .await
            .map_err(|e| TaskError::Io(e.to_string()))?;
        // Keep a copy of the description for the `TaskCreated` fire below — the
        // original is moved into `base` here.
        let description_for_hook = description.clone();
        let base = TaskStateBase {
            id: id.clone(),
            task_type,
            status: TaskStatus::Running,
            description,
            tool_use_id: None,
            start_time: SystemTime::now(),
            end_time: None,
            total_paused_ms: 0,
            output_file: path,
            output_offset: 0,
            notified: false,
        };
        let state = state_for_spawn(base, &input);
        self.tasks.write().await.insert(id.clone(), state);

        // 4. Record the handler-spawned id so `kill` dispatches teardown back
        //    to the owning handler (it manages its own runtime task; the
        //    registry holds no `BackgroundTaskHandle` for it).
        self.spawned.write().await.insert(id.clone(), task_type);

        // Best-effort `TaskCreated` fire — the production task-creation path
        // (alongside `create`'s placeholder path). Both insert a new task row,
        // so both fire. Runs after the write guards drop. No-op when no firer is
        // registered.
        self.fire_task_created(&id, task_type, &description_for_hook).await;

        Ok(id)
    }

    /// Look up a task by ID.
    pub async fn get(&self, task_id: &str) -> Option<TaskState> {
        self.tasks.read().await.get(task_id).cloned()
    }

    /// Return all known tasks.
    pub async fn list(&self) -> Vec<TaskState> {
        self.tasks.read().await.values().cloned().collect()
    }

    /// Test-only: force a `local_bash` task into a known status + exit code so
    /// `output()`'s `status`/`exit_code`/`done` projection can be exercised
    /// deterministically (`set_status` cannot set `exit_code`). Panics if the
    /// id is unknown or not a `local_bash` task.
    #[cfg(test)]
    pub(crate) async fn force_bash_terminal_for_test(
        &self,
        task_id: &str,
        status: TaskStatus,
        exit_code: Option<i32>,
    ) {
        let mut map = self.tasks.write().await;
        match map.get_mut(task_id) {
            Some(TaskState::LocalBash(b)) => {
                b.base.status = status;
                b.exit_code = exit_code;
            }
            _ => panic!("expected a local_bash task with id {task_id}"),
        }
    }

    /// Force `task_id`'s status to `status`. Returns
    /// [`TaskError::NotFound`] if the id is unknown. Only Bash and Agent
    /// variants currently carry a writable `status` field in the M1 surface;
    /// other variants are no-ops on the variant but still return the
    /// (possibly unchanged) state for the caller's consumption.
    pub async fn set_status(
        &self,
        task_id: &str,
        status: TaskStatus,
    ) -> Result<TaskState, TaskError> {
        let updated = {
            let mut map = self.tasks.write().await;
            let entry = map
                .get_mut(task_id)
                .ok_or_else(|| TaskError::NotFound(task_id.to_string()))?;
            match entry {
                TaskState::LocalBash(b) => b.base.status = status,
                TaskState::LocalAgent(a) => a.base.status = status,
                TaskState::RemoteAgent(r) => r.base.status = status,
                TaskState::InProcessTeammate(t) => t.base.status = status,
                TaskState::LocalWorkflow(w) => w.base.status = status,
                TaskState::MonitorMcp(m) => m.base.status = status,
                TaskState::Dream(d) => d.base.status = status,
            }
            entry.clone()
            // `map` write-guard drops here — the best-effort hook fire below
            // runs WITHOUT holding the registry lock so a slow/blocking hook
            // never stalls other task operations.
        };

        // Best-effort `TaskCompleted` fire on the terminal transition
        // (claude-code `executeTaskCompletedHooks`). Only `Completed` / `Failed`
        // mirror claude-code's fire points (`TaskUpdateTool` status →
        // `completed`; `stopHooks.ts` for a teammate's in-progress tasks). A
        // `Killed` transition is terminal but has no claude-code counterpart, so
        // it does NOT fire. No-op when no firer is registered.
        if let Some(firer) = &self.task_completed_firer {
            let status_str = match status {
                TaskStatus::Completed => Some("completed"),
                TaskStatus::Failed => Some("failed"),
                _ => None,
            };
            if let Some(status_str) = status_str {
                let base = updated.base();
                // Wire payload (`TaskCompletedHookInputSchema`): `task_subject`
                // and `task_description` both source from the task's
                // `description` — the M-surface task state carries no distinct
                // `subject` field. `teammate_name` / `team_name` are not stored
                // on the task state, so they ride as `None` (documented gap).
                firer
                    .fire(hooks::TaskCompletedFire {
                        task_id: task_id.to_string(),
                        status: status_str.to_string(),
                        task_subject: base.description.clone(),
                        task_description: Some(base.description.clone()),
                        teammate_name: None,
                        team_name: None,
                    })
                    .await;
            }
        }

        Ok(updated)
    }

    /// Kill a task, cancelling its background handle if any.
    ///
    /// Two paths, decided by how the task was started:
    /// - A task started via [`spawn`](Self::spawn) is torn down through its
    ///   owning handler ([`Task::kill`]) — the handler manages its own runtime
    ///   task internally, so there is no `BackgroundTaskHandle` for the registry
    ///   to cancel.
    /// - Otherwise the legacy [`create`](Self::create) /
    ///   [`RuntimeSpawner`]-cancel path runs: cancel any recorded
    ///   [`BackgroundTaskHandle`] and mark the Bash/Agent variant `Killed`.
    ///
    /// Note: only Bash and Agent states currently carry a writable `status`
    /// field in the M1 surface; other variants are no-ops on cancel.
    pub async fn kill(&self, task_id: &str) -> Result<(), TaskError> {
        // Handler-spawned path: dispatch teardown to the owning handler.
        let spawned_type = self.spawned.write().await.remove(task_id);
        if let Some(task_type) = spawned_type {
            if let Some(handler) = self.handlers.get(&task_type) {
                let ctx = TaskContext {
                    fs: self.fs.clone(),
                    runtime: self.runtime.clone(),
                };
                handler.kill(task_id, ctx).await?;
            }
            // Reflect the kill in the tracked state for any variant the M1
            // surface can write; the handler's status sink drives the rest.
            if let Some(s) = self.tasks.write().await.get_mut(task_id) {
                match s {
                    TaskState::LocalBash(b) => b.base.status = TaskStatus::Killed,
                    TaskState::LocalAgent(a) => a.base.status = TaskStatus::Killed,
                    _ => {}
                }
            }
            return Ok(());
        }

        let mut handles = self.handles.lock().await;
        if let Some(h) = handles.remove(task_id) {
            self.runtime
                .cancel(&h)
                .await
                .map_err(|e| TaskError::Internal(e.to_string()))?;
        }
        if let Some(s) = self.tasks.write().await.get_mut(task_id) {
            // Mark killed for the variants whose status is exposed here.
            match s {
                TaskState::LocalBash(b) => b.base.status = TaskStatus::Killed,
                TaskState::LocalAgent(a) => a.base.status = TaskStatus::Killed,
                // Other variants intentionally fall through in M1.
                _ => {}
            }
        }
        Ok(())
    }
}

/// `TeamSpawnSeam` impl — the typed spawn/kill seam the coordinator's
/// `TeamCreate` / `TeamDelete` tools use to start and stop a real
/// `InProcessTeammate` task WITHOUT a `coordinator` → `lingxi-tasks` dependency
/// cycle (the abstract trait lives in `traits`; this concrete impl lives here).
///
/// Unlike [`TaskRegistryHandle::create`](traits::task_registry::TaskRegistryHandle::create)
/// — which builds a nil-`AgentId` / empty-name placeholder — `spawn_teammate`
/// threads the worker `agent_id` + `name` straight through
/// [`TaskRegistry::spawn`], so the started teammate is keyed on the real worker
/// identity, and returns the **handler-generated** `task_id` the coordinator
/// reconciles back onto its `WorkerAgent`.
#[async_trait]
impl TeamSpawnSeam for TaskRegistry {
    async fn spawn_teammate(
        &self,
        agent_id: protocol::AgentId,
        name: String,
        description: String,
    ) -> Result<String, TeamSpawnError> {
        self.spawn(
            TaskType::InProcessTeammate,
            TaskSpawnInput::InProcessTeammate { agent_id, name },
            description,
        )
        .await
        .map_err(task_err_to_team_spawn_err)
    }

    async fn kill(&self, task_id: &str) -> Result<(), TeamSpawnError> {
        // Delegate to the inherent `TaskRegistry::kill`, which routes teardown
        // back to the owning handler for handler-spawned (teammate) tasks.
        TaskRegistry::kill(self, task_id)
            .await
            .map_err(task_err_to_team_spawn_err)
    }
}

/// Map a [`TaskError`] from the inherent spawn/kill path onto the narrow
/// [`TeamSpawnError`] seam vocabulary: a missing handler is `Unsupported`, an
/// unknown id is `NotFound`, everything else is `Internal`.
fn task_err_to_team_spawn_err(e: TaskError) -> TeamSpawnError {
    match e {
        TaskError::UnknownType => TeamSpawnError::Unsupported("in_process_teammate".to_string()),
        TaskError::NotFound(id) => TeamSpawnError::NotFound(id),
        other => TeamSpawnError::Internal(other.to_string()),
    }
}

/// Build the typed [`TaskState`] for a [`spawn`](TaskRegistry::spawn) using the
/// REAL `input` fields (the spawn path, unlike `create`'s placeholder path,
/// has the agent ids / commands the variant carries).
fn state_for_spawn(base: TaskStateBase, input: &TaskSpawnInput) -> TaskState {
    match input {
        TaskSpawnInput::LocalBash { command, .. } => {
            TaskState::LocalBash(crate::state::LocalBashTaskState {
                base,
                command: command.clone(),
                pid: None,
                exit_code: None,
            })
        }
        TaskSpawnInput::LocalAgent {
            agent_id,
            prompt,
            is_backgrounded,
            ..
        } => TaskState::LocalAgent(crate::state::LocalAgentTaskState {
            base,
            agent_id: *agent_id,
            prompt: prompt.clone(),
            error: None,
            messages: vec![],
            pending_messages: vec![],
            is_backgrounded: *is_backgrounded,
        }),
        TaskSpawnInput::RemoteAgent { endpoint, .. } => {
            TaskState::RemoteAgent(crate::state::RemoteAgentTaskState {
                base,
                remote_session_id: String::new(),
                remote_endpoint: endpoint.clone(),
            })
        }
        TaskSpawnInput::InProcessTeammate { agent_id, .. } => {
            TaskState::InProcessTeammate(crate::state::InProcessTeammateTaskState {
                base,
                agent_id: *agent_id,
                pending_messages: vec![],
            })
        }
        TaskSpawnInput::LocalWorkflow { workflow_id } => {
            TaskState::LocalWorkflow(crate::state::LocalWorkflowTaskState {
                base,
                workflow_id: workflow_id.clone(),
                current_step: 0,
            })
        }
        TaskSpawnInput::MonitorMcp { server_name, watch } => {
            TaskState::MonitorMcp(crate::state::MonitorMcpTaskState {
                base,
                server_name: server_name.clone(),
                watch_resources: watch.clone(),
            })
        }
        TaskSpawnInput::Dream { max_iterations, .. } => {
            TaskState::Dream(crate::state::DreamTaskState {
                base,
                iteration_count: 0,
                max_iterations: *max_iterations,
            })
        }
    }
}

/// Register the M2 *self-contained* per-type handlers — the ones whose only
/// dependencies are platform traits already available at boot (no agent /
/// subagent pool, mailbox, or budget enforcer). Today that is
/// [`TaskType::LocalBash`] and [`TaskType::MonitorMcp`].
///
/// `process` + `sandbox` are required because they are absent from
/// [`crate::task_trait::TaskContext`] yet [`LocalBashHandler`] cannot run a
/// command without them. The spool [`TaskOutputManager`] is shared with the
/// registry's own (`reg.output_manager`) so handler-allocated spool paths land
/// in the same sandboxed output dir the registry hands the TUI. `mcp` drives
/// [`MonitorMcpHandler`]'s catalog polls.
///
/// Call this *before* the registry is wrapped in an [`Arc`] — registration
/// takes `&mut self`. The remaining five task types (agent/teammate/workflow/
/// remote/dream) register once their production pools are wired (M9+).
pub fn register_self_contained_handlers(
    reg: &mut TaskRegistry,
    process: Arc<dyn ProcessRunner>,
    sandbox: Arc<dyn Sandbox>,
    mcp: Arc<mcp::McpRegistry>,
) {
    let output_manager = reg.output_manager.clone();
    reg.register_handler(
        TaskType::LocalBash,
        Arc::new(LocalBashHandler::new(
            process,
            sandbox,
            output_manager.clone(),
        )),
    );
    reg.register_handler(
        TaskType::MonitorMcp,
        Arc::new(MonitorMcpHandler::with_default_interval(mcp, output_manager)),
    );
}

/// Register the M2 *agent-backed* per-type handlers — the ones whose
/// dependencies are NOT available at platform boot but require the production
/// agent pipeline: [`TaskType::LocalAgent`] (a one-shot subagent driven through
/// a [`SubagentSpawner`]) and [`TaskType::InProcessTeammate`] (a persistent,
/// message-driven subagent driven through a [`StateMachinePool`]).
///
/// These are split out of [`register_self_contained_handlers`] because they need
/// seams the task-registry construction site does not have at boot today — a
/// concrete [`SubagentSpawner`] / [`StateMachinePool`], the parent's
/// [`ToolInvoker`] + [`BudgetEnforcerHandle`], and a [`SubagentApiClient`]. The
/// boot site (`apps/cli/src/init.rs`) constructs the task registry with
/// `subagent_spawner: None` / `budget_enforcer: None` and no teammate pool, so
/// wiring this helper there is a tracked follow-up (M9+). The helper exists now
/// so the wire step can call it once those pools land, and so the handlers are
/// reachable + tested in the interim.
///
/// `tool_invoker` + `budget` are passed through *unchanged* (cloning the `Arc`
/// preserves pointer identity, which the recursion-lock + budget-aggregation
/// invariants rely on). The spool [`TaskOutputManager`] is shared with the
/// registry's own (`reg.output_manager`). Both handlers default their narrow
/// status-sink seam; callers needing the registry-status adapter can build the
/// handlers directly and `register_handler` them instead. The subagent type
/// arrives already resolved on the [`crate::task_trait::TaskSpawnInput::LocalAgent`]
/// variant, so no resolver injection is needed here.
///
/// Call this *before* the registry is wrapped in an [`Arc`] — registration
/// takes `&mut self`.
pub fn register_agent_handlers(
    reg: &mut TaskRegistry,
    spawner: Arc<dyn SubagentSpawner>,
    pool: Arc<StateMachinePool>,
    tool_invoker: Arc<dyn ToolInvoker>,
    budget: Arc<dyn BudgetEnforcerHandle>,
    api_client: Arc<dyn SubagentApiClient>,
) {
    let output_manager = reg.output_manager.clone();
    reg.register_handler(
        TaskType::LocalAgent,
        Arc::new(LocalAgentHandler::new(
            spawner,
            tool_invoker.clone(),
            budget,
            output_manager.clone(),
        )),
    );
    reg.register_handler(
        TaskType::InProcessTeammate,
        Arc::new(
            InProcessTeammateHandler::new(pool, output_manager, api_client)
                .with_tool_invoker(tool_invoker),
        ),
    );
}

/// Register the M2 *agent-backed* dream handler — [`TaskType::Dream`] — a
/// one-shot forked subagent that runs the memory-consolidation prompt through a
/// [`SubagentSpawner`].
///
/// Split out for the same reason as [`register_agent_handlers`]: the dream
/// handler needs the production agent pipeline (a concrete [`SubagentSpawner`]
/// plus the parent's [`ToolInvoker`] + [`BudgetEnforcerHandle`]) that the
/// task-registry construction site does not have at boot today. The boot site
/// (`apps/cli/src/init.rs`) wires this once the subagent pool lands (M9+) — the
/// same deferred-wiring note the agent handlers carry. The helper exists now so
/// the wire step can call it once those pools land, and so the handler is
/// reachable + tested in the interim.
///
/// `tool_invoker` + `budget` are passed through *unchanged* (cloning the `Arc`
/// preserves pointer identity, which the recursion-lock + budget-aggregation
/// invariants rely on). The spool [`TaskOutputManager`] is shared with the
/// registry's own (`reg.output_manager`).
///
/// Call this *before* the registry is wrapped in an [`Arc`] — registration
/// takes `&mut self`.
pub fn register_dream_handler(
    reg: &mut TaskRegistry,
    spawner: Arc<dyn SubagentSpawner>,
    tool_invoker: Arc<dyn ToolInvoker>,
    budget: Arc<dyn BudgetEnforcerHandle>,
) {
    let output_manager = reg.output_manager.clone();
    reg.register_handler(
        TaskType::Dream,
        Arc::new(DreamHandler::new(spawner, tool_invoker, budget, output_manager)),
    );
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod spawn_tests {
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
        ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = FileEvent> + Send>>, FsError>
        {
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
    }
    impl RecordingHandler {
        fn new(task_type: TaskType, task_id: &str) -> Arc<Self> {
            Arc::new(Self {
                task_type,
                task_id: task_id.to_string(),
                spawns: AtomicUsize::new(0),
                killed: StdMutex::new(Vec::new()),
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
            Ok(TaskHandle {
                task_id: self.task_id.clone(),
                cleanup: None,
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
            .create(TaskType::InProcessTeammate, teammate_input(), "placeholder".into())
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
            .spawn_teammate(protocol::AgentId::new(), "buddy".into(), "a teammate".into())
            .await
            .unwrap();

        // Non-empty, handler-generated id (NOT the worker AgentId).
        assert!(!task_id.is_empty(), "seam returns a non-empty task_id");
        assert_eq!(task_id, "tseamid", "seam returns the handler-generated id");
        assert_eq!(handler.spawn_count(), 1, "the teammate handler ran exactly once");

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
            .spawn_teammate(protocol::AgentId::new(), "buddy".into(), "no handler".into())
            .await
            .unwrap_err();
        assert!(
            matches!(err, TeamSpawnError::Unsupported(_)),
            "missing teammate handler maps to TeamSpawnError::Unsupported; got {err:?}"
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
        let registry = TaskRegistry::new(runtime, fs, out_mgr)
            .with_task_completed_firer(firer.clone());
        let task_id = registry
            .create(TaskType::LocalBash, teammate_input(), description.to_string())
            .await
            .unwrap();
        (dir, registry, firer, task_id)
    }

    #[tokio::test]
    async fn completed_transition_fires_byte_faithful_payload() {
        let (_d, registry, firer, task_id) =
            registry_with_firer("ship the parity port").await;

        let updated = registry
            .set_status(&task_id, TaskStatus::Completed)
            .await
            .unwrap();
        assert_eq!(updated.base().status, TaskStatus::Completed);

        let recorded = firer.recorded();
        assert_eq!(recorded.len(), 1, "exactly one TaskCompleted fire: {recorded:?}");
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
        assert_eq!(recorded.len(), 1, "a Failed transition fires TaskCompleted: {recorded:?}");
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

    fn registry_with_created_firer() -> (tempfile::TempDir, TaskRegistry, Arc<RecordingCreatedFirer>)
    {
        let dir = tempdir().unwrap();
        let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let out_mgr = Arc::new(crate::output_manager::TaskOutputManager::new(
            PathBuf::from(dir.path()),
            fs.clone(),
        ));
        let firer = RecordingCreatedFirer::new();
        let registry =
            TaskRegistry::new(runtime, fs, out_mgr).with_task_created_firer(firer.clone());
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
        assert_eq!(recorded.len(), 1, "exactly one TaskCreated fire: {recorded:?}");
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
}

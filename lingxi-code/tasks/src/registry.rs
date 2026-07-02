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
    /// Task ids of PERSISTENT agents that came to rest since the last drain —
    /// armed by [`mark_task_rested`](Self::mark_task_rested) (via the status
    /// sink's `notify_rest`), surfaced ONCE per rest by
    /// [`take_pending_task_notifications`](Self::take_pending_task_notifications)
    /// WITHOUT eviction (the still-alive agent re-arms on its next rest). Kept
    /// out of [`TaskStateBase`] to avoid a workspace-wide exhaustive-initializer
    /// churn for a field only this path reads.
    pending_rest: Arc<RwLock<std::collections::HashMap<String, RestPayload>>>,
}

/// The optional `<result>` / `<usage>` payload an agent carries when it comes to
/// rest (binary `enqueueAgentNotification` always passes both when a result
/// exists). Stashed at arm time so [`TaskRegistry::take_pending_task_notifications`]
/// can populate the notification — the live task itself stays `Running`.
#[derive(Clone, Default)]
struct RestPayload {
    /// The agent's final-text response → the `<result>` section.
    result: Option<String>,
    /// Run usage → the `<usage>` section.
    usage: Option<traits::task_registry::AgentRunUsage>,
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
            pending_rest: Arc::new(RwLock::new(std::collections::HashMap::new())),
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

        // 3. Recover the spool path the handler ALREADY allocated (the path is a
        //    deterministic function of the id) and insert the typed state built
        //    from the REAL input fields (not `create`'s placeholders), so
        //    `list()` / `get()` reflect a live spawned task.
        //
        //    We deliberately do NOT call `allocate(&id)` a second time here: the
        //    handler created the spool (and its worker may already be appending
        //    to it), so a re-allocate would either error on the exclusive
        //    (`O_EXCL`) create or — worse, pre-fix — truncate output the worker
        //    just wrote (the T4 double-allocate truncate race). `path_for` only
        //    reconstructs the path; it touches no bytes.
        let path = self
            .output_manager
            .path_for(&id)
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

    /// Find a RUNNING `local_workflow` task whose effective run id equals
    /// `run_id`, returning its task id. Backs claude-code's resume gate
    /// (Workflow validateInput errorCode 3): a `resumeFromRunId` naming a
    /// still-running workflow must be rejected — two runs sharing a run id
    /// would race on the same journal.
    pub async fn find_running_workflow_by_run_id(&self, run_id: &str) -> Option<String> {
        let tasks = self.tasks.read().await;
        tasks.iter().find_map(|(task_id, state)| match state {
            TaskState::LocalWorkflow(w)
                if matches!(w.base.status, TaskStatus::Running)
                    && w.run_id.as_deref() == Some(run_id) =>
            {
                Some(task_id.clone())
            }
            _ => None,
        })
    }

    /// Record a `local_bash` task's child exit code (M8 cc2.1.198 "Task
    /// panels: no stuck Running"): the worker's status sink reports the exit
    /// code alongside the terminal status once the process ends; `output()`
    /// projects it as `exit_code`/`done`. Non-bash variants are a benign
    /// no-op (only bash children carry an OS exit code). `NotFound` for an
    /// unknown id — the sink swallows it (racing teardown tolerance).
    pub async fn set_bash_exit_code(
        &self,
        task_id: &str,
        exit_code: i32,
    ) -> Result<(), TaskError> {
        let mut map = self.tasks.write().await;
        let entry = map
            .get_mut(task_id)
            .ok_or_else(|| TaskError::NotFound(task_id.to_string()))?;
        if let TaskState::LocalBash(b) = entry {
            b.exit_code = Some(exit_code);
        }
        Ok(())
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

    /// Test-only: inject a fully-built [`TaskState`] into the task map under its
    /// own id. Lets tests exercise per-variant `output()` projections (e.g. the
    /// `local_agent` clean-result branch) without spinning up the variant's
    /// real handler.
    #[cfg(test)]
    pub(crate) async fn insert_state_for_test(&self, state: TaskState) {
        let id = state.base().id.clone();
        self.tasks.write().await.insert(id, state);
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

    /// Mark a task as having had its terminal output consumed by a reader
    /// (claude-code `TaskOutputTool` `updateTaskState(task_id, t => ({ ...t,
    /// notified: true }))`, fired from both the non-blocking terminal branch and
    /// the blocking terminal branch). Setting `notified` suppresses a later
    /// duplicate `<task-notification>` for a task the model has already seen.
    ///
    /// Eagerly evicts the task when it is BOTH terminal and now notified —
    /// claude-code's `evictTerminalTask` eager-GC path
    /// (`framework.ts:120-143`): a terminal + notified task has been consumed and
    /// is dropped from the map so memory is freed without waiting for the next
    /// poll-loop iteration. A non-terminal (still pending/running) task keeps the
    /// flag and stays in the map.
    ///
    /// Returns [`TaskError::NotFound`] if the id is unknown.
    pub async fn mark_notified(&self, task_id: &str) -> Result<(), TaskError> {
        let mut map = self.tasks.write().await;
        let entry = map
            .get_mut(task_id)
            .ok_or_else(|| TaskError::NotFound(task_id.to_string()))?;
        match entry {
            TaskState::LocalBash(b) => b.base.notified = true,
            TaskState::LocalAgent(a) => a.base.notified = true,
            TaskState::RemoteAgent(r) => r.base.notified = true,
            TaskState::InProcessTeammate(t) => t.base.notified = true,
            TaskState::LocalWorkflow(w) => w.base.notified = true,
            TaskState::MonitorMcp(m) => m.base.notified = true,
            TaskState::Dream(d) => d.base.notified = true,
        }
        // Eager eviction (claude-code `evictTerminalTask`): a terminal + notified
        // task is consumed and can be GC'd immediately.
        if entry.base().status.is_terminal() {
            map.remove(task_id);
        }
        Ok(())
    }

    /// Drop every task that is BOTH terminal (completed / failed / killed) AND
    /// `notified` — the lazy-GC safety net that mirrors claude-code's
    /// `generateTaskAttachments` eviction sweep (`framework.ts:172-180`,
    /// `applyTaskOffsetsAndEvictions:234-245`). The eager [`mark_notified`] path
    /// already drops a task the moment it becomes terminal+notified; this sweep
    /// catches any that became terminal AFTER they were notified (e.g. a notified
    /// `pending`/`running` task that later finished). Returns the evicted ids.
    pub async fn evict_terminal_tasks(&self) -> Vec<String> {
        let mut map = self.tasks.write().await;
        let evict: Vec<String> = map
            .iter()
            .filter(|(_, s)| {
                let b = s.base();
                b.notified && b.status.is_terminal()
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in &evict {
            map.remove(id);
        }
        evict
    }

    /// Arm a one-shot "came to rest" notification for a PERSISTENT, still-alive
    /// task (the `LocalAgentHandler` calls this via the status sink each time its
    /// backgrounded agent rests). The next [`take_pending_task_notifications`]
    /// surfaces it WITHOUT evicting; a fresh rest re-arms it. A no-op for an
    /// unknown or already-terminal task (the ordinary terminal-notification path
    /// owns those).
    pub async fn mark_task_rested(
        &self,
        task_id: &str,
        result: Option<String>,
        usage: Option<traits::task_registry::AgentRunUsage>,
    ) {
        {
            let map = self.tasks.read().await;
            match map.get(task_id) {
                Some(s) if !s.base().status.is_terminal() => {}
                _ => return,
            }
        }
        self.pending_rest
            .write()
            .await
            .insert(task_id.to_string(), RestPayload { result, usage });
    }

    /// Drain the terminal tasks not yet surfaced to the model, marking each
    /// `notified` (and evicting it, since terminal + notified is GC-able) so a
    /// completion is reported exactly once. Returns a [`TaskNotification`]
    /// snapshot per drained task, in registry-iteration order.
    ///
    /// This is the turn-boundary equivalent of claude-code's per-task-type
    /// completion callbacks (`enqueueShellNotification` / `enqueueAgentNotification`
    /// / …): a task that reaches a terminal status is reported once and then
    /// `notified`. A task ALREADY `notified` (its output consumed via
    /// `TaskOutput`/`TaskStop`) is skipped, so the model never sees a duplicate.
    /// Pending / running tasks are left untouched.
    ///
    /// Field mapping per type (mirroring the per-type `enqueue*Notification`):
    /// - `local_bash` carries `exit_code`;
    /// - `local_agent` carries `error` (its `failed` reason);
    /// - every type carries `tool_use_id` (when launched from a tool call) and
    ///   the spool `output_path`.
    pub async fn take_pending_task_notifications(
        &self,
    ) -> Vec<traits::task_registry::TaskNotification> {
        use crate::handle::{status_to_wire, task_type_to_wire};
        let mut map = self.tasks.write().await;
        // Collect ids first (terminal + not-notified) so the per-id remove below
        // doesn't fight the iteration borrow.
        let drain_ids: Vec<String> = map
            .iter()
            .filter(|(_, s)| {
                let b = s.base();
                b.status.is_terminal() && !b.notified
            })
            .map(|(id, _)| id.clone())
            .collect();
        let mut out = Vec::with_capacity(drain_ids.len());
        for id in drain_ids {
            let Some(state) = map.get(&id) else { continue };
            let b = state.base();
            // Per-type fields the renderer needs beyond the shared base.
            let exit_code = match state {
                TaskState::LocalBash(bash) => bash.exit_code,
                _ => None,
            };
            let error = match state {
                TaskState::LocalAgent(agent) => agent.error.clone(),
                _ => None,
            };
            out.push(traits::task_registry::TaskNotification {
                task_id: b.id.clone(),
                task_type: task_type_to_wire(b.task_type).to_string(),
                status: status_to_wire(b.status).to_string(),
                description: b.description.clone(),
                tool_use_id: b.tool_use_id.clone(),
                output_path: Some(b.output_file.to_string_lossy().into_owned()),
                exit_code,
                error,
                // `local_agent` `<result>` / `<usage>` (the optional sections):
                // `LocalAgentTaskState` carries neither the final-message text nor
                // the run usage today, so both stay `None` — the byte-faithful
                // "no result" case. They flow only once the BACKGROUNDED local_agent
                // path is wired (`AgentTool::call` dispatches synchronously today,
                // and the production `LocalAgentHandler` has no result-bearing sink;
                // the renderer already emits them when present — see
                // `prompt::task_notification`). Part of the deferred async-agent work.
                result: None,
                usage: None,
            });
            // Mark notified + evict (terminal + notified is GC-able) so the
            // completion surfaces exactly once. Mirrors `mark_notified`'s eager
            // eviction without re-acquiring the lock.
            map.remove(&id);
        }

        // Rest notifications: a PERSISTENT agent that came to rest (non-terminal,
        // still alive) surfaces ONCE per rest WITHOUT eviction — claude-code's
        // "fires each time this agent comes to rest … the same task-id may notify
        // more than once." Drain + clear the armed set; the agent re-arms on its
        // next rest. A task that raced to terminal is skipped here (the terminal
        // drain above already owns it).
        let rest_payloads: Vec<(String, RestPayload)> = {
            let mut armed = self.pending_rest.write().await;
            let drained = armed.drain().collect::<Vec<_>>();
            drained
        };
        for (id, payload) in rest_payloads {
            let Some(state) = map.get(&id) else { continue };
            let b = state.base();
            if b.status.is_terminal() {
                continue;
            }
            out.push(traits::task_registry::TaskNotification {
                task_id: b.id.clone(),
                task_type: task_type_to_wire(b.task_type).to_string(),
                // DISPLAY status "completed" — the notification renderer
                // (`prompt::task_notification`, the v2.1.185 `enqueueAgentNotification`)
                // maps `completed → "Agent … came to rest"` and any other status
                // to a wrong summary ("running" would hit the killed/`_` branch →
                // "(stopped by user)"). A normal rest IS "came to rest", so the
                // notification carries "completed". This is DISPLAY-ONLY and does
                // NOT touch the registry — the task itself stays `Running` (alive,
                // resumable); only this surfaced reminder reads `completed`.
                status: status_to_wire(TaskStatus::Completed).to_string(),
                description: b.description.clone(),
                tool_use_id: b.tool_use_id.clone(),
                // The spool carries the just-produced turn-set result; the model
                // reads it (it was told it can Read/Bash-tail the output file).
                output_path: Some(b.output_file.to_string_lossy().into_owned()),
                exit_code: None,
                error: None,
                // The agent's final-text response + run usage, captured at rest
                // time. The binary `enqueueAgentNotification` always emits these
                // when a result exists; the renderer omits each clause when `None`.
                result: payload.result,
                usage: payload.usage,
            });
        }
        out
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
        team_name: String,
        description: String,
    ) -> Result<String, TeamSpawnError> {
        self.spawn(
            TaskType::InProcessTeammate,
            TaskSpawnInput::InProcessTeammate {
                agent_id,
                name,
                team_name,
                // The TeamCreate description IS the teammate's initial task
                // (seeded as its first user message); also reused as the task
                // subject below.
                description: description.clone(),
            },
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

    /// Inject a message into a running teammate's turn loop. This is the
    /// production override of the defaulted seam method — the load-bearing
    /// bridge that lets a coordinator `SendMessage` reach a teammate's runner.
    ///
    /// Resolves the task's owning handler the SAME way [`kill`](Self::kill)
    /// does — via the handler-spawned-id index (`spawned`), so it routes to the
    /// exact handler that started the worker — then dispatches to
    /// [`Task::send_message`] when the handler
    /// [`supports_messages`](Task::supports_messages). For the
    /// `InProcessTeammate` handler that lands the text on the running agent's
    /// persist-mode `recv()` (the `injectUserMessageToTeammate` analogue).
    ///
    /// Error mapping (so the mailbox→runner pump can act): a task whose handler
    /// reports `TerminatedTask` (its runner dropped the receiver) OR an
    /// unknown/gone task (`NotFound`) maps to [`TeamSpawnError::Terminated`] —
    /// the pump's "stop" signal. A handler that does not support messages maps
    /// to [`TeamSpawnError::Unsupported`]; anything else is `Internal`.
    async fn send_message(&self, task_id: &str, message: String) -> Result<(), TeamSpawnError> {
        // Resolve the owning handler via the spawned-id index (mirrors `kill`'s
        // dispatch). An unknown id ⇒ the teammate is gone ⇒ `Terminated`.
        let task_type = self
            .spawned
            .read()
            .await
            .get(task_id)
            .copied()
            .ok_or(TeamSpawnError::Terminated)?;
        let handler = self
            .handlers
            .get(&task_type)
            .ok_or_else(|| TeamSpawnError::Unsupported(format!("{task_type:?}")))?
            .clone();
        if !handler.supports_messages() {
            return Err(TeamSpawnError::Unsupported(format!("{task_type:?}")));
        }
        let ctx = TaskContext {
            fs: self.fs.clone(),
            runtime: self.runtime.clone(),
        };
        handler
            .send_message(task_id, message, ctx)
            .await
            .map_err(|e| match e {
                // The runner dropped its receiver / the task is gone ⇒ the pump
                // must stop: both collapse onto the seam's `Terminated`.
                TaskError::TerminatedTask | TaskError::NotFound(_) => TeamSpawnError::Terminated,
                TaskError::Unsupported => TeamSpawnError::Unsupported(format!("{task_type:?}")),
                other => TeamSpawnError::Internal(other.to_string()),
            })
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
fn state_for_spawn(mut base: TaskStateBase, input: &TaskSpawnInput) -> TaskState {
    // Stamp the originating `tool_use_id` onto the task so a backgrounded agent's
    // `<task-notification>` carries the `<tool-use-id>` line (claude-code parity).
    // Only `LocalAgent` threads it today; other types keep the caller's `None`.
    if let TaskSpawnInput::LocalAgent { tool_use_id, .. } = input {
        if base.tool_use_id.is_none() {
            base.tool_use_id = tool_use_id.clone();
        }
    }
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
            subagent_type,
            prompt,
            is_backgrounded,
            ..
        } => TaskState::LocalAgent(crate::state::LocalAgentTaskState {
            base,
            agent_id: *agent_id,
            subagent_type: subagent_type.clone(),
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
        TaskSpawnInput::LocalWorkflow {
            workflow_id,
            script,
            resume_from_run_id,
            args,
            run_id,
            invocation_mode: _,
            workflow_source: _,
            launched_from_subagent: _,
        } => TaskState::LocalWorkflow(crate::state::LocalWorkflowTaskState {
            base,
            workflow_id: workflow_id.clone(),
            script: script.clone(),
            resume_from_run_id: resume_from_run_id.clone(),
            args: args.clone(),
            // Effective run id: the launcher-minted id for a fresh run, else the
            // resumed id (so the resume gate can find a still-running workflow).
            run_id: run_id.clone().or_else(|| resume_from_run_id.clone()),
            current_step: 0,
        }),
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
/// `bash_status_sink` (M8 cc2.1.198 "Task panels: no stuck Running") bridges
/// the bash worker's terminal status + exit code back into the registry.
/// Pre-fix the handler kept its default `NoopStatusSink`, so a finished
/// background bash task's stored `TaskStateBase.status` stayed `Running`
/// forever — the stuck panel. The sink is the DEFERRED
/// [`crate::registry_status_sink::RegistryStatusSink`] pattern: registered
/// here (before the registry `Arc` exists) and bound by the composition root
/// once it does — the same cycle-break the `LocalAgent` handler uses.
///
/// Call this *before* the registry is wrapped in an [`Arc`] — registration
/// takes `&mut self`. The remaining five task types (agent/teammate/workflow/
/// remote/dream) register once their production pools are wired (M9+).
pub fn register_self_contained_handlers(
    reg: &mut TaskRegistry,
    process: Arc<dyn ProcessRunner>,
    sandbox: Arc<dyn Sandbox>,
    mcp: Arc<mcp::McpRegistry>,
    bash_status_sink: Arc<dyn crate::handlers::TaskStatusSink>,
) {
    let output_manager = reg.output_manager.clone();
    reg.register_handler(
        TaskType::LocalBash,
        Arc::new(
            LocalBashHandler::new(process, sandbox, output_manager.clone())
                .with_status_sink(bash_status_sink),
        ),
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
#[path = "registry_test.rs"]
mod registry_test;

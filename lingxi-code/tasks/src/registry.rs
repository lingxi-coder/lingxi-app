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
use crate::task_trait::{Task, TaskContext, TaskError, TaskHandle, TaskSpawnInput};
use agent::{StateMachinePool, SubagentApiClient};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
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
    /// Alternate addresses accepted by task tools (`agent_id`, named async
    /// agents, `name@team`) mapped onto the canonical task id. Real task ids
    /// still win on lookup; aliases only bridge claude-code's Agent return
    /// surface to the task registry's `a…`/`t…` ids.
    aliases: Arc<RwLock<HashMap<String, String>>>,
    handlers: HashMap<TaskType, Arc<dyn Task>>,
    handles: Arc<tokio::sync::Mutex<HashMap<String, BackgroundTaskHandle>>>,
    /// Handler-returned cleanup hooks keyed by task id. Handler-spawned tasks
    /// often own runtime work internally; preserving this hook lets the registry
    /// participate in the same teardown path instead of dropping the only
    /// synchronous cleanup handle.
    cleanups: Arc<tokio::sync::Mutex<HashMap<String, TaskCleanup>>>,
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
    /// Per-session running total of subagents spawned through the `Agent` tool
    /// (claude 2.1.212 `taskRegistry` `getTotalAgentSpawns` /
    /// `incrementTotalAgentSpawns`). The tool reads this before every spawn and
    /// rejects the launch once it reaches `CLAUDE_CODE_MAX_SUBAGENTS_PER_SESSION`
    /// (default 200), then bumps it on a cleared spawn. Interior-mutable so the
    /// shared `Arc<TaskRegistry>` the tool holds can count without a write lock.
    total_agent_spawns: AtomicU64,
    /// Session-wide WebSearch call counter — the `taskRegistry` `n` behind
    /// `getWebSearchCalls`/`incrementWebSearchCalls`/`resetWebSearchCalls` (parity
    /// 2.1.212). Session-global (shared across the main loop and its subagents,
    /// which route through the same registry), so the `WebSearch` tool enforces
    /// ONE budget per session. An `Arc<AtomicU32>` — cheap atomic reads/writes off
    /// the tool's hot path, no lock.
    web_search_calls: Arc<std::sync::atomic::AtomicU32>,
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

type TaskCleanup = Arc<dyn Fn() + Send + Sync>;

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
            aliases: Arc::new(RwLock::new(HashMap::new())),
            handlers: HashMap::new(),
            handles: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            cleanups: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            spawned: Arc::new(RwLock::new(HashMap::new())),
            runtime,
            fs,
            output_manager,
            task_completed_firer: None,
            task_created_firer: None,
            pending_rest: Arc::new(RwLock::new(std::collections::HashMap::new())),
            total_agent_spawns: AtomicU64::new(0),
            web_search_calls: Arc::new(std::sync::atomic::AtomicU32::new(0)),
        }
    }

    /// Session running total of `Agent`-tool subagent spawns (claude 2.1.212
    /// `getTotalAgentSpawns`). Read before every spawn to enforce the
    /// `CLAUDE_CODE_MAX_SUBAGENTS_PER_SESSION` cap.
    #[must_use]
    pub fn total_agent_spawns(&self) -> u64 {
        self.total_agent_spawns.load(Ordering::SeqCst)
    }

    /// Bump the session subagent-spawn counter by one (claude 2.1.212
    /// `incrementTotalAgentSpawns`), called once a spawn clears the cap gate.
    pub fn increment_total_agent_spawns(&self) {
        self.total_agent_spawns.fetch_add(1, Ordering::SeqCst);
    }

    /// Current session-wide WebSearch call count — `getWebSearchCalls(){return n}`
    /// (parity 2.1.212).
    #[must_use]
    pub fn web_search_calls(&self) -> u32 {
        self.web_search_calls
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Increment the session WebSearch counter — `incrementWebSearchCalls(){n++}`.
    pub fn increment_web_search_calls(&self) {
        self.web_search_calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// Reset the session WebSearch counter — `resetWebSearchCalls(){n=0}`.
    pub fn reset_web_search_calls(&self) {
        self.web_search_calls
            .store(0, std::sync::atomic::Ordering::Relaxed);
    }

    /// Inject the best-effort `TaskCompleted` hook firer. Default-`None`
    /// builder (the `RemoteTrigger`/seam pattern): existing `new()` callers and
    /// tests stay no-op; the composition root threads the orchestrator's firer
    /// here so a terminal status transition fires the `TaskCompleted` hook.
    #[must_use]
    pub fn with_task_completed_firer(mut self, firer: Arc<dyn hooks::TaskCompletedFirer>) -> Self {
        self.task_completed_firer = Some(firer);
        self
    }

    /// Inject the best-effort `TaskCreated` hook firer. Counterpart to
    /// [`with_task_completed_firer`](Self::with_task_completed_firer): existing
    /// `new()` callers and tests stay no-op; the composition root threads the
    /// orchestrator's firer here so creating a task fires the `TaskCreated`
    /// hook.
    #[must_use]
    pub fn with_task_created_firer(mut self, firer: Arc<dyn hooks::TaskCreatedFirer>) -> Self {
        self.task_created_firer = Some(firer);
        self
    }

    /// Register a per-type handler.
    pub fn register_handler(&mut self, task_type: TaskType, handler: Arc<dyn Task>) {
        self.handlers.insert(task_type, handler);
    }

    /// Resolve `id_or_alias` to the canonical task id. A real task id wins over
    /// an alias collision; stale aliases are ignored and cleaned best-effort.
    pub async fn resolve_task_id(&self, id_or_alias: &str) -> Option<String> {
        if self.tasks.read().await.contains_key(id_or_alias) {
            return Some(id_or_alias.to_string());
        }
        let mapped = self.aliases.read().await.get(id_or_alias).cloned();
        if let Some(task_id) = mapped {
            if self.tasks.read().await.contains_key(&task_id) {
                return Some(task_id);
            }
            self.aliases.write().await.remove(id_or_alias);
        }
        None
    }

    async fn canonical_or_raw(&self, id_or_alias: &str) -> String {
        self.resolve_task_id(id_or_alias)
            .await
            .unwrap_or_else(|| id_or_alias.to_string())
    }

    /// Register an additional task address. Empty aliases and self aliases are
    /// ignored; later registrations intentionally win so a reused async-agent
    /// name points at the newest live task, matching claude-code's name map.
    pub async fn register_task_alias(
        &self,
        task_id: &str,
        alias: impl Into<String>,
    ) -> Result<(), TaskError> {
        if !self.tasks.read().await.contains_key(task_id) {
            return Err(TaskError::NotFound(task_id.to_string()));
        }
        let alias = alias.into();
        if alias.is_empty() || alias == task_id {
            return Ok(());
        }
        self.aliases
            .write()
            .await
            .insert(alias, task_id.to_string());
        Ok(())
    }

    async fn register_spawn_aliases(&self, task_id: &str, input: &TaskSpawnInput) {
        for alias in aliases_for_spawn(input) {
            let _ = self.register_task_alias(task_id, alias).await;
        }
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
        self.fire_task_created(&id, task_type, &description_for_hook)
            .await;
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

    /// Register a backgrounded MCP tool call (claude-code 2.1.212
    /// `callMcpToolWithAutoBackground`'s `i.register(NZu(...))`). Inserts a
    /// `running` [`crate::state::McpTaskState`] with `mcpStatus:"working"`,
    /// keyed on a freshly-minted `k…` id, and stores a cancel cleanup so a later
    /// [`kill`](Self::kill) aborts the still-running in-flight call (the port
    /// equivalent of the state's `abortController` + the poll loop's
    /// `cancelTask` on `status==="killed"`). Returns the minted id.
    pub async fn register_mcp_task(
        &self,
        server_name: String,
        tool_name: String,
        tool_use_id: Option<String>,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<String, TaskError> {
        let id = generate_task_id(TaskType::McpTask);
        let path = self
            .output_manager
            .allocate(&id)
            .await
            .map_err(|e| TaskError::Io(e.to_string()))?;
        // Description mirrors `NZu`'s `${serverName}/${toolName}` (also what
        // `background_message` interpolates as the tool label).
        let description = format!("{server_name}/{tool_name}");
        let description_for_hook = description.clone();
        let base = TaskStateBase {
            id: id.clone(),
            task_type: TaskType::McpTask,
            // `NZu` seeds `status:"running"` (distinct from the `create`
            // placeholder's `Pending`); the call is already in flight.
            status: TaskStatus::Running,
            description,
            tool_use_id,
            start_time: SystemTime::now(),
            end_time: None,
            total_paused_ms: 0,
            output_file: path,
            output_offset: 0,
            notified: false,
        };
        let state = TaskState::McpTask(crate::state::McpTaskState {
            base,
            server_name,
            tool_name,
            // `NZu` seeds `mcpStatus:"working"`.
            mcp_status: "working".to_string(),
            status_message: None,
        });
        self.tasks.write().await.insert(id.clone(), state);
        // Cancel-on-kill hook: `kill` runs this cleanup (no handler / no
        // `BackgroundTaskHandle` for an mcp_task), which fires the token the
        // caller races its in-flight call against.
        let cleanup: TaskCleanup = Arc::new(move || cancel.cancel());
        self.cleanups.lock().await.insert(id.clone(), cleanup);
        self.fire_task_created(&id, TaskType::McpTask, &description_for_hook)
            .await;
        Ok(id)
    }

    /// Settle a backgrounded MCP tool call once its detached `tools/call`
    /// resolves (claude-code `callMcpToolWithAutoBackground`'s `E` +
    /// `p.then(…)`). Writes `result_text` into the task spool so the drained
    /// `<task-notification>`'s `output-file` carries the real result, then marks
    /// the task terminal (`completed`/`failed`) and updates `mcpStatus`.
    ///
    /// A task already terminal (e.g. killed via `TaskStop`, or already settled)
    /// is left untouched — the binary's `if(O.notified) return O` guard — so a
    /// race between kill and settle never resurrects a killed task.
    ///
    /// Returns `Ok(true)` when THIS call won the terminal transition (the
    /// binary's `E` callback observing `k = true` after `i.update`), and
    /// `Ok(false)` when the task was already terminal so the update no-op'd
    /// (`if (O.notified) return O`). Callers use this to gate the
    /// `mcp_auto_background` outcome counter — a killed / already-settled task
    /// must never re-emit it.
    pub async fn settle_mcp_task(
        &self,
        task_id: &str,
        result_text: &str,
        failed: bool,
    ) -> Result<bool, TaskError> {
        let task_id = self.canonical_or_raw(task_id).await;
        // Guard + recover the spool path under a read lock.
        let output_file = {
            let map = self.tasks.read().await;
            match map.get(&task_id) {
                Some(s) if s.base().status.is_terminal() => return Ok(false),
                Some(s) => s.base().output_file.clone(),
                None => return Err(TaskError::NotFound(task_id)),
            }
        };
        // Persist the real result so the notification's `output-file` carries it
        // (the port surfaces the result via the spool, not an inline value).
        let _ = self.output_manager.append(&output_file, result_text).await;
        // The call has settled, so it is no longer cancellable — drop the
        // cancel hook before the atomic terminal transition below (this also
        // makes the task drain-eligible once it goes terminal). Removing it
        // first keeps the `.await` on `cleanups.lock()` OUT of the write guard.
        self.cleanups.lock().await.remove(&task_id);
        let status = if failed {
            TaskStatus::Failed
        } else {
            TaskStatus::Completed
        };
        // Terminal transition + `mcpStatus`, done ATOMICALLY under ONE write
        // guard — the binary's single-closure `i.update(_, O => { if (O.notified)
        // return O; … })`. `end_time`, `mcpStatus`, AND the terminal status flip
        // all land under the same held guard, so a concurrent kill either wins
        // fully (settle's re-check below sees `Killed`/terminal and no-ops) or
        // loses fully — never a torn half-settled state. This is why settle can
        // no longer overwrite a kill (nor fire a spurious `TaskCompleted`): the
        // status write is inside the guard, not a separate `set_status`
        // acquisition split off by the `cleanups.lock()` await above.
        let settled_base = {
            let mut map = self.tasks.write().await;
            let Some(state) = map.get_mut(&task_id) else {
                return Err(TaskError::NotFound(task_id));
            };
            // Re-check terminal under the write lock: a kill may have raced in
            // (set `Killed` / `mcpStatus:"cancelled"`) after the read guard
            // above. If so, leave it untouched — the `if (O.notified) return O`
            // no-op — so a killed task is never resurrected as `Completed`.
            if state.base().status.is_terminal() {
                return Ok(false);
            }
            state.base_mut().end_time = Some(SystemTime::now());
            if let TaskState::McpTask(m) = state {
                m.mcp_status = if failed {
                    "failed".to_string()
                } else {
                    "completed".to_string()
                };
            }
            state.base_mut().status = status;
            state.base().clone()
            // `map` write-guard drops here — the best-effort hook fire below
            // runs WITHOUT holding the registry lock (mirrors `set_status`).
        };
        // Best-effort `TaskCompleted` fire on the terminal transition THIS
        // settle just won (claude-code `executeTaskCompletedHooks`). Because the
        // status write above is atomic, a raced kill can never reach this fire:
        // it either wins the guard (settle no-ops before here) or loses it (and
        // its `Killed` transition, which has no claude-code counterpart, never
        // fires). Only `Completed` / `Failed` fire; no-op without a firer.
        if let Some(firer) = &self.task_completed_firer {
            let status_str = if failed { "failed" } else { "completed" };
            firer
                .fire(hooks::TaskCompletedFire {
                    task_id: task_id.to_string(),
                    status: status_str.to_string(),
                    task_subject: settled_base.description.clone(),
                    task_description: Some(settled_base.description.clone()),
                    teammate_name: None,
                    team_name: None,
                })
                .await;
        }
        Ok(true)
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
        let TaskHandle {
            task_id: id,
            cleanup,
        } = handler.spawn(input.clone(), ctx).await?;

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
        self.register_spawn_aliases(&id, &input).await;

        // 4. Record the handler-spawned id so `kill` dispatches teardown back
        //    to the owning handler (it manages its own runtime task; the
        //    registry holds no `BackgroundTaskHandle` for it).
        self.spawned.write().await.insert(id.clone(), task_type);
        if let Some(cleanup) = cleanup {
            self.cleanups.lock().await.insert(id.clone(), cleanup);
        }

        // Best-effort `TaskCreated` fire — the production task-creation path
        // (alongside `create`'s placeholder path). Both insert a new task row,
        // so both fire. Runs after the write guards drop. No-op when no firer is
        // registered.
        self.fire_task_created(&id, task_type, &description_for_hook)
            .await;

        Ok(id)
    }

    /// Look up a task by ID.
    pub async fn get(&self, task_id: &str) -> Option<TaskState> {
        let task_id = self.canonical_or_raw(task_id).await;
        self.tasks.read().await.get(&task_id).cloned()
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
    pub async fn set_bash_exit_code(&self, task_id: &str, exit_code: i32) -> Result<(), TaskError> {
        let task_id = self.canonical_or_raw(task_id).await;
        let mut map = self.tasks.write().await;
        let entry = map
            .get_mut(&task_id)
            .ok_or_else(|| TaskError::NotFound(task_id.clone()))?;
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
        let task_id = self.canonical_or_raw(task_id).await;
        let updated = {
            let mut map = self.tasks.write().await;
            let entry = map
                .get_mut(&task_id)
                .ok_or_else(|| TaskError::NotFound(task_id.clone()))?;
            match entry {
                TaskState::LocalBash(b) => b.base.status = status,
                TaskState::LocalAgent(a) => a.base.status = status,
                TaskState::RemoteAgent(r) => r.base.status = status,
                TaskState::InProcessTeammate(t) => t.base.status = status,
                TaskState::LocalWorkflow(w) => w.base.status = status,
                TaskState::MonitorMcp(m) => m.base.status = status,
                TaskState::McpTask(m) => m.base.status = status,
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
    /// Terminal tasks are retained after notification so `TaskList`,
    /// `TaskOutput`, and `TaskStop` can still address completed background work
    /// until an explicit cleanup/delete path removes it.
    ///
    /// Returns [`TaskError::NotFound`] if the id is unknown.
    pub async fn mark_notified(&self, task_id: &str) -> Result<(), TaskError> {
        let task_id = self.canonical_or_raw(task_id).await;
        let mut map = self.tasks.write().await;
        let entry = map
            .get_mut(&task_id)
            .ok_or_else(|| TaskError::NotFound(task_id.clone()))?;
        entry.base_mut().notified = true;
        Ok(())
    }

    /// Deprecated compatibility hook. Completed background tasks are retained
    /// after notification; there is no implicit terminal-task GC here.
    pub async fn evict_terminal_tasks(&self) -> Vec<String> {
        Vec::new()
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
        let Some(task_id) = self.resolve_task_id(task_id).await else {
            return;
        };
        {
            let map = self.tasks.read().await;
            match map.get(&task_id) {
                Some(s) if !s.base().status.is_terminal() => {}
                _ => return,
            }
        }
        self.pending_rest
            .write()
            .await
            .insert(task_id, RestPayload { result, usage });
    }

    /// Drain the terminal tasks not yet surfaced to the model, marking each
    /// `notified` while retaining it so a completion is reported exactly once.
    /// Returns a [`TaskNotification`]
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
            let Some(state) = map.get(&id) else {
                continue;
            };
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
                // `killed_by` (the by-Claude/by-user split) + the isolation
                // `<worktree>` section: `LocalAgentTaskState` tracks neither the
                // stop reason nor the carried worktree handle today, so both stay
                // `None` here — the byte-faithful "no reason / no worktree" case.
                // The renderer (`prompt::task_notification`) emits the correct
                // bytes the moment they are populated; wiring the stop reason
                // through the kill paths and the worktree metadata onto the state
                // is part of the same deferred backgrounded-local_agent work as
                // `result`/`usage` above.
                killed_by: None,
                worktree_path: None,
                worktree_branch: None,
            });
            // Mark notified so the completion surfaces exactly once. The task
            // itself stays addressable until an explicit cleanup/delete removes
            // it.
            if let Some(state) = map.get_mut(&id) {
                state.base_mut().notified = true;
            }
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
                // A rest notification is not a kill and carries no worktree
                // metadata on the rest payload, so both stay `None` (see the
                // terminal-drain note above; renderer-ready when populated).
                killed_by: None,
                worktree_path: None,
                worktree_branch: None,
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
        let task_id = self.canonical_or_raw(task_id).await;
        let task_id_ref = task_id.as_str();
        // Preserve the handler's explicit kill path as the primary cancellation
        // mechanism. The cleanup hook is a fallback/drop-owner hook; running it
        // first can remove the handler's worker record before `Task::kill` gets a
        // chance to cancel the live runtime work.
        let cleanup = self.cleanups.lock().await.remove(task_id_ref);
        // Handler-spawned path: dispatch teardown to the owning handler.
        let spawned_type = self.spawned.write().await.remove(task_id_ref);
        if let Some(task_type) = spawned_type {
            if let Some(handler) = self.handlers.get(&task_type) {
                let ctx = TaskContext {
                    fs: self.fs.clone(),
                    runtime: self.runtime.clone(),
                };
                handler.kill(task_id_ref, ctx).await?;
            }
            // Reflect the kill in the tracked state for any variant the M1
            // surface can write; the handler's status sink drives the rest.
            if let Some(s) = self.tasks.write().await.get_mut(task_id_ref) {
                match s {
                    TaskState::LocalBash(b) => b.base.status = TaskStatus::Killed,
                    TaskState::LocalAgent(a) => a.base.status = TaskStatus::Killed,
                    // A backgrounded MCP call: mark killed + `mcpStatus:"cancelled"`
                    // (the poll loop's `status==="killed"` → `cancelTask` branch).
                    TaskState::McpTask(m) => {
                        m.base.status = TaskStatus::Killed;
                        m.mcp_status = "cancelled".to_string();
                    }
                    _ => {}
                }
            }
            if let Some(cleanup) = cleanup {
                cleanup();
            }
            return Ok(());
        }

        let mut handles = self.handles.lock().await;
        if let Some(h) = handles.remove(task_id_ref) {
            self.runtime
                .cancel(&h)
                .await
                .map_err(|e| TaskError::Internal(e.to_string()))?;
        }
        if let Some(s) = self.tasks.write().await.get_mut(task_id_ref) {
            // Mark killed for the variants whose status is exposed here.
            match s {
                TaskState::LocalBash(b) => b.base.status = TaskStatus::Killed,
                TaskState::LocalAgent(a) => a.base.status = TaskStatus::Killed,
                // A backgrounded MCP call: the stored cancel cleanup aborts the
                // in-flight call; reflect the kill + `mcpStatus:"cancelled"`.
                TaskState::McpTask(m) => {
                    m.base.status = TaskStatus::Killed;
                    m.mcp_status = "cancelled".to_string();
                }
                // Other variants intentionally fall through in M1.
                _ => {}
            }
        }
        if let Some(cleanup) = cleanup {
            cleanup();
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

    /// A teammate task is ALIVE while its stored state is present and
    /// non-terminal. A terminal status (Completed / Failed / Killed) reads as
    /// "gone" even though the retained task remains inspectable — the
    /// mailbox→runner pump uses this on its park timeout to stop pumping a dead
    /// teammate (so its mailbox can be unregistered) even if no message ever
    /// arrived to surface `Terminated`. A resting PERSISTENT agent keeps a
    /// non-terminal (Running) status, so it correctly reads as alive.
    async fn is_alive(&self, task_id: &str) -> bool {
        match self.get(task_id).await {
            Some(state) => !state.base().status.is_terminal(),
            None => false,
        }
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
        TaskSpawnInput::McpTask {
            server_name,
            tool_name,
            tool_use_id,
        } => {
            if base.tool_use_id.is_none() {
                base.tool_use_id = tool_use_id.clone();
            }
            TaskState::McpTask(crate::state::McpTaskState {
                base,
                server_name: server_name.clone(),
                tool_name: tool_name.clone(),
                // `NZu` seeds `mcpStatus:"working"`.
                mcp_status: "working".to_string(),
                status_message: None,
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

fn aliases_for_spawn(input: &TaskSpawnInput) -> Vec<String> {
    let mut aliases = Vec::new();
    match input {
        TaskSpawnInput::LocalAgent {
            agent_id,
            spawn_request,
            ..
        } => {
            aliases.push(agent_id.to_string());
            if let Some(request) = spawn_request {
                if let Some(name) = request.name.as_deref().filter(|s| !s.is_empty()) {
                    aliases.push(name.to_string());
                    if let Some(team) = request.team_name.as_deref().filter(|s| !s.is_empty()) {
                        aliases.push(format!("{name}@{team}"));
                    }
                }
            }
        }
        TaskSpawnInput::InProcessTeammate {
            agent_id,
            name,
            team_name,
            ..
        } => {
            aliases.push(agent_id.to_string());
            if !name.is_empty() {
                aliases.push(name.clone());
                if !team_name.is_empty() {
                    aliases.push(format!("{name}@{team_name}"));
                }
            }
        }
        _ => {}
    }
    aliases
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
        Arc::new(MonitorMcpHandler::with_default_interval(
            mcp,
            output_manager,
        )),
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
        Arc::new(DreamHandler::new(
            spawner,
            tool_invoker,
            budget,
            output_manager,
        )),
    );
}

#[cfg(test)]
#[path = "registry_test.rs"]
mod registry_test;

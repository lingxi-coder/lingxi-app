//! Central registry tracking running tasks.
//!
//! The registry owns the in-memory map of task IDs to [`TaskState`] and to
//! the [`BackgroundTaskHandle`]s returned by the [`RuntimeSpawner`].

use crate::handlers::{
    DreamHandler, InProcessTeammateHandler, LocalAgentHandler, LocalBashHandler,
    LocalFusionHandler, MonitorHandler, MonitorMcpHandler,
};
use crate::id::{generate_task_id, TaskType};
use crate::output_manager::TaskOutputManager;
use crate::state::{TaskState, TaskStateBase, TaskStatus};
use crate::task_trait::{Task, TaskContext, TaskError, TaskSpawnInput};
use agent::{StateMachinePool, SubagentApiClient};
use async_trait::async_trait;
use platform_api::team_spawn::{TeamSpawnError, TeamSpawnSeam};
use platform_api::{
    BackgroundTaskHandle, BudgetEnforcerHandle, FileSystem, FusionCompletionSink, FusionExecutor,
    ProcessRunner, RuntimeSpawner, Sandbox, SubagentSpawner, ToolInvoker,
};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::RwLock;

/// Tracks running tasks and dispatches lifecycle operations to handlers.
pub struct TaskRegistry {
    tasks: Arc<RwLock<HashMap<String, TaskState>>>,
    workflow_launch_reservations: Arc<std::sync::Mutex<HashSet<String>>>,
    workflow_session_filter: Arc<std::sync::RwLock<Option<String>>>,
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
    /// Per-task handles that move a still-running FOREGROUND command to the
    /// background (claude-code `I_t`'s `t.background(e)`). Separate from
    /// `cleanups` because backgrounding is not teardown: the child keeps
    /// running, it just stops being awaited.
    backgrounders: Arc<
        tokio::sync::Mutex<HashMap<String, Arc<dyn platform_api::task_registry::TaskBackgrounder>>>,
    >,
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
    /// Bounded live `monitor_ws` stdout events waiting for the next turn.
    pending_monitor_events: Arc<
        tokio::sync::Mutex<
            std::collections::VecDeque<platform_api::task_registry::TaskNotification>,
        >,
    >,
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
    usage: Option<platform_api::task_registry::AgentRunUsage>,
    /// Persistent id of the resting agent. Used only for unnamed-owner
    /// fallback; never surfaced as a display name.
    agent_id: Option<protocol::AgentId>,
    /// The resting agent's DISPLAY identity. A "came to rest" notification is
    /// deferred while this agent still owns live background children, and the
    /// owner match is by the same creator fields child task creation stamps.
    agent_name: Option<String>,
    team_name: Option<String>,
}

type TaskCleanup = Arc<dyn Fn() + Send + Sync>;

/// Cancellation guard for the registry publication → TaskCreated → activation
/// handoff in [`TaskRegistry::spawn`].
///
/// Once the handler has prepared a worker, any cancellation before activation
/// must stop that worker and remove every registry artifact already published.
/// The cleanup hook is synchronous; Tokio-owned maps are removed on the current
/// runtime because `Drop` itself cannot await their locks.
struct SpawnPublicationGuard {
    task_id: String,
    tasks: Arc<RwLock<HashMap<String, TaskState>>>,
    spawned: Arc<RwLock<HashMap<String, TaskType>>>,
    cleanups: Arc<tokio::sync::Mutex<HashMap<String, TaskCleanup>>>,
    aliases: Arc<RwLock<HashMap<String, String>>>,
    cleanup: Option<TaskCleanup>,
    armed: bool,
}

impl SpawnPublicationGuard {
    fn new(registry: &TaskRegistry, task_id: String, cleanup: Option<TaskCleanup>) -> Self {
        Self {
            task_id,
            tasks: registry.tasks.clone(),
            spawned: registry.spawned.clone(),
            cleanups: registry.cleanups.clone(),
            aliases: registry.aliases.clone(),
            cleanup,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
        self.cleanup = None;
    }
}

impl Drop for SpawnPublicationGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }

        // Stop handler-owned work immediately. The hook is idempotent and may
        // itself schedule async pool teardown on the current runtime.
        if let Some(cleanup) = self.cleanup.take() {
            cleanup();
        }

        let task_id = self.task_id.clone();
        let tasks = self.tasks.clone();
        let spawned = self.spawned.clone();
        let cleanups = self.cleanups.clone();
        let aliases = self.aliases.clone();
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            tracing::error!(task_id, "task spawn cancelled without a Tokio runtime; registry rollback could not be scheduled");
            return;
        };
        drop(runtime.spawn(async move {
            // Match publication lock order. Holding the task write lock while
            // removing secondary indexes prevents readers from observing a row
            // whose route/cleanup is only partially rolled back.
            let mut task_rows = tasks.write().await;
            let mut routes = spawned.write().await;
            let mut cleanup_rows = cleanups.lock().await;
            let mut alias_rows = aliases.write().await;
            task_rows.remove(&task_id);
            routes.remove(&task_id);
            cleanup_rows.remove(&task_id);
            alias_rows.retain(|_, canonical| canonical != &task_id);
        }));
    }
}

/// Durable handoff fields used to rebuild a checkpointed workflow after the
/// host process restarts. The workflow is registered as `Paused`; no worker is
/// spawned until the user explicitly invokes `Workflow` with its run id.
///
/// Deliberately carries no `scope` field. Every field here is read straight
/// from the on-disk checkpoint (`adopt.json`), which records whatever the
/// ORIGINAL caller supplied -- so `workflow_id` and `args` are exactly as
/// untrustworthy for minting Local App authority as they are everywhere else
/// in this crate (see [`crate::scope::LocalAppWorkflowTaskScope`]'s module
/// docs). Putting a `scope` field on this same struct would invite exactly
/// the mistake this type exists to avoid: deriving authority from the
/// checkpoint's own `workflow_id`/`args` instead of from state the Host
/// re-resolves on load. [`TaskRegistry::register_adopted_workflow_with_scope`]
/// takes the re-derived scope as a separate argument instead.
#[derive(Debug, Clone)]
pub struct AdoptedWorkflow {
    /// Original workflow task id.
    pub task_id: String,
    /// Session that owned this workflow when it was checkpointed.
    pub session_uuid: Option<String>,
    /// Display identity derived from the workflow metadata.
    pub workflow_id: String,
    /// Stable `wf_…` journal run id.
    pub run_id: String,
    /// Persisted deterministic script to rerun.
    pub script_path: String,
    /// Serialized workflow args, when present.
    pub args: Option<String>,
    /// Directory containing the run's `journal.jsonl`.
    pub transcript_dir: String,
    /// Human-readable workflow description.
    pub description: String,
    /// Original run start time.
    pub start_time: SystemTime,
}

/// Cancellation-safe ownership of one workflow run id during launch/adoption.
/// Dropping the token releases the id even if the surrounding async future is
/// aborted before it reaches its normal return path.
#[must_use = "dropping the reservation immediately reopens the workflow run id"]
#[derive(Debug)]
pub struct WorkflowRunReservation {
    reservations: Arc<std::sync::Mutex<HashSet<String>>>,
    run_id: Option<String>,
}

impl Drop for WorkflowRunReservation {
    fn drop(&mut self) {
        let Some(run_id) = self.run_id.take() else {
            return;
        };
        self.reservations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&run_id);
    }
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
            workflow_launch_reservations: Arc::new(std::sync::Mutex::new(HashSet::new())),
            workflow_session_filter: Arc::new(std::sync::RwLock::new(None)),
            aliases: Arc::new(RwLock::new(HashMap::new())),
            handlers: HashMap::new(),
            handles: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            cleanups: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            backgrounders: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            spawned: Arc::new(RwLock::new(HashMap::new())),
            runtime,
            fs,
            output_manager,
            task_completed_firer: None,
            task_created_firer: None,
            pending_rest: Arc::new(RwLock::new(std::collections::HashMap::new())),
            pending_monitor_events: Arc::new(tokio::sync::Mutex::new(
                std::collections::VecDeque::new(),
            )),
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

    /// Reserve one lifetime spawn atomically, rejecting once `cap` is reached.
    pub fn try_reserve_total_agent_spawn(&self, cap: u64) -> Result<u64, u64> {
        self.total_agent_spawns
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
                (current < cap).then(|| current + 1)
            })
            .map(|previous| previous + 1)
    }

    /// Roll back a reservation for a launch rejected before pool allocation.
    pub fn release_total_agent_spawn_reservation(&self) {
        self.release_total_agent_spawn_reservations(1);
    }

    /// Reserve `n` lifetime spawn slots atomically.
    pub fn try_reserve_total_agent_spawns(&self, n: u64, cap: u64) -> Result<u64, u64> {
        if n == 0 {
            return Ok(self.total_agent_spawns());
        }
        self.total_agent_spawns
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
                current.checked_add(n).filter(|&next| next <= cap)
            })
            .map(|previous| previous + n)
    }

    /// Release `n` previously reserved lifetime spawn slots.
    pub fn release_total_agent_spawn_reservations(&self, n: u64) {
        if n == 0 {
            return;
        }
        let _ =
            self.total_agent_spawns
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
                    Some(current.saturating_sub(n))
                });
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
            creator_teammate_name: None,
            creator_team_name: None,
            creator_agent_id: None,
        };
        // Build a default state per type; production stores real fields.
        #[allow(clippy::match_same_arms)]
        let state = match task_type {
            TaskType::LocalBash => TaskState::LocalBash(crate::state::LocalBashTaskState {
                base,
                command: String::new(),
                pid: None,
                exit_code: None,
                cwd: None,
                is_backgrounded: None,
            }),
            TaskType::Dream => TaskState::Dream(crate::state::DreamTaskState {
                base,
                iteration_count: 0,
                max_iterations: None,
            }),
            TaskType::Monitor => TaskState::Monitor(crate::state::MonitorTaskState {
                base,
                command: String::new(),
                exit_code: None,
            }),
            _ => TaskState::LocalBash(crate::state::LocalBashTaskState {
                base,
                command: String::new(),
                pid: None,
                exit_code: None,
                cwd: None,
                is_backgrounded: None,
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
            let (teammate_name, team_name) = self
                .get(task_id)
                .await
                .map(|state| {
                    let base = state.base();
                    (
                        base.creator_teammate_name.clone(),
                        base.creator_team_name.clone(),
                    )
                })
                .unwrap_or((None, None));
            firer
                .fire(hooks::TaskCreatedFire {
                    task_id: task_id.to_string(),
                    task_subject: format!("{task_type:?}"),
                    task_description: Some(description.to_string()),
                    teammate_name,
                    team_name,
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
        self.register_mcp_task_owned(
            server_name,
            tool_name,
            tool_use_id,
            None,
            None,
            None,
            cancel,
        )
        .await
    }

    pub(crate) async fn register_mcp_task_owned(
        &self,
        server_name: String,
        tool_name: String,
        tool_use_id: Option<String>,
        creator_teammate_name: Option<String>,
        creator_team_name: Option<String>,
        creator_agent_id: Option<protocol::AgentId>,
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
            creator_teammate_name,
            creator_team_name,
            creator_agent_id,
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
    /// Register a shell command the caller is about to background, minting the
    /// id and creating the output file the process runner will append to.
    ///
    /// This is the port of claude-code `Xne` (2.1.263 `src_160988549.js`
    /// @4281167): the shell's own task identity becomes a `local_bash` record
    /// with `status: "running"`, so the `backgroundTaskId` handed to the model
    /// resolves in `TaskOutput` / `TaskStop` / `TaskList`, and the terminal
    /// transition produces the completion `<task-notification>`.
    ///
    /// The registry owns the id space and the output directory, so the caller
    /// must take both from here rather than minting its own — a second id space
    /// is exactly the defect this closes.
    ///
    /// # Errors
    /// Returns [`TaskError::Io`] when the output file cannot be allocated.
    pub async fn allocate_bash_output(&self) -> Result<(String, std::path::PathBuf), TaskError> {
        let id = generate_task_id(TaskType::LocalBash);
        let path = self
            .output_manager
            .allocate(&id)
            .await
            .map_err(|e| TaskError::Io(e.to_string()))?;
        Ok((id, path))
    }

    /// Delete an allocated shell output file that was never needed, because the
    /// command finished in the foreground and its output was returned inline
    /// (claude-code `deleteOutputFile` under `outputFileRedundant`).
    pub async fn discard_bash_output(&self, task_id: &str) {
        let Ok(path) = self.output_manager.path_for(task_id) else {
            return;
        };
        if self.tasks.read().await.contains_key(task_id) {
            // A record was registered against this identity after all; its file
            // is live output, not a redundant allocation.
            return;
        }
        let _ = self.output_manager.discard(&path).await;
    }

    /// Register a `local_bash` record for an identity already minted by
    /// [`Self::allocate_bash_output`].
    ///
    /// # Errors
    /// Returns [`TaskError::Io`] when the output path cannot be derived.
    pub async fn register_background_bash(
        &self,
        id: String,
        command: String,
        description: String,
        tool_use_id: Option<String>,
        cwd: Option<String>,
        creator_agent_id: Option<protocol::AgentId>,
    ) -> Result<(String, std::path::PathBuf), TaskError> {
        self.register_bash_row(
            id,
            command,
            description,
            tool_use_id,
            cwd,
            creator_agent_id,
            true,
        )
        .await
    }

    /// Shared row builder for the two `local_bash` registration entry points.
    /// `backgrounded` is claude-code's `isBackgrounded`: `true` for `Xne` (an
    /// explicit background spawn or a timed-out command), `false` for `U6t`
    /// (the 2 s foreground arming).
    #[allow(clippy::too_many_arguments)]
    async fn register_bash_row(
        &self,
        id: String,
        command: String,
        description: String,
        tool_use_id: Option<String>,
        cwd: Option<String>,
        creator_agent_id: Option<protocol::AgentId>,
        backgrounded: bool,
    ) -> Result<(String, std::path::PathBuf), TaskError> {
        let path = self
            .output_manager
            .path_for(&id)
            .map_err(|e| TaskError::Io(e.to_string()))?;
        let description_for_hook = description.clone();
        let base = TaskStateBase {
            id: id.clone(),
            task_type: TaskType::LocalBash,
            status: TaskStatus::Running,
            description,
            tool_use_id,
            start_time: SystemTime::now(),
            end_time: None,
            total_paused_ms: 0,
            output_file: path.clone(),
            output_offset: 0,
            notified: false,
            creator_teammate_name: None,
            creator_team_name: None,
            creator_agent_id,
        };
        let state = TaskState::LocalBash(crate::state::LocalBashTaskState {
            base,
            command,
            pid: None,
            exit_code: None,
            cwd,
            // This entry point only fires for a shell that is actually being
            // backgrounded (claude-code `Xne` registers `isBackgrounded: true`).
            // The 2 s foreground arming registers `false` through
            // [`Self::register_foreground_bash`].
            is_backgrounded: Some(backgrounded),
        });
        self.tasks.write().await.insert(id.clone(), state);
        self.fire_task_created(&id, TaskType::LocalBash, &description_for_hook)
            .await;
        Ok((id, path))
    }

    /// Record the OS pid of an already-registered background shell and install
    /// the cleanup that kills it, so `TaskStop` reaches a child this registry
    /// did not spawn itself.
    ///
    /// # Errors
    /// Returns [`TaskError::NotFound`] when the id is unknown.
    pub async fn bind_background_bash_process(
        &self,
        task_id: &str,
        pid: Option<u32>,
        killer: Arc<dyn platform_api::task_registry::TaskKiller>,
    ) -> Result<(), TaskError> {
        let task_id = self.canonical_or_raw(task_id).await;
        {
            let mut map = self.tasks.write().await;
            let entry = map
                .get_mut(&task_id)
                .ok_or_else(|| TaskError::NotFound(task_id.clone()))?;
            if let TaskState::LocalBash(bash) = entry {
                bash.pid = pid;
            }
        }
        // `TaskCleanup` is synchronous, so the kill is dispatched onto the
        // runtime rather than awaited in place (the same shape the handler
        // cleanups use for worker cancellation).
        let cleanup: TaskCleanup = Arc::new(move || {
            let killer = killer.clone();
            tokio::spawn(async move { killer.kill().await });
        });
        self.cleanups.lock().await.insert(task_id, cleanup);
        Ok(())
    }

    /// Register a still-running FOREGROUND shell (claude-code `U6t`, fired by
    /// the Bash poll loop once the command has run for `cnr` = 2000 ms).
    ///
    /// The row is `running` with `is_backgrounded = Some(false)`: visible to
    /// `/tasks` and addressable by Ctrl+B / background-all, but not a background
    /// task. [`Self::unregister_foreground_bash`] withdraws it again when the
    /// command finishes in the foreground.
    ///
    /// # Errors
    /// Returns [`TaskError`] if the record cannot be published.
    pub async fn register_foreground_bash(
        &self,
        task_id: &str,
        registration: platform_api::task_registry::BackgroundBashRegistration,
        auto_background_armed: bool,
    ) -> Result<(), TaskError> {
        // `autoBackgroundArmed` rides on the record in claude-code purely so the
        // UI can say whether the deadline will background or kill. Nothing in
        // this engine reads it yet, so it is not stored — recording a field no
        // consumer reads would be the "named, computed, never wired" shape.
        let _ = auto_background_armed;
        self.register_bash_row(
            task_id.to_string(),
            registration.command,
            registration.description,
            registration.tool_use_id,
            registration.cwd,
            registration.creator_agent_id,
            false,
        )
        .await
        .map(|_| ())
    }

    /// Withdraw an armed foreground row because the command finished in the
    /// foreground.
    ///
    /// claude-code `W6t`: `if(!bp(o)||o.isBackgrounded||o.notified) return;
    /// r.remove(e)`. A row that was backgrounded in the meantime is left alone —
    /// it owns a real child now and will produce its own completion
    /// notification.
    pub async fn unregister_foreground_bash(&self, task_id: &str) {
        let task_id = self.canonical_or_raw(task_id).await;
        let mut map = self.tasks.write().await;
        let Some(TaskState::LocalBash(bash)) = map.get(&task_id) else {
            return;
        };
        if bash.is_backgrounded == Some(true) || bash.base.notified {
            return;
        }
        map.remove(&task_id);
    }

    /// Attach the handle that moves an armed foreground shell to the background
    /// on demand (claude-code holds the live `shellCommand` on the record and
    /// calls `t.background(e)`).
    ///
    /// # Errors
    /// Returns [`TaskError::NotFound`] when the id is unknown.
    pub async fn bind_background_requester(
        &self,
        task_id: &str,
        requester: Arc<dyn platform_api::task_registry::TaskBackgrounder>,
    ) -> Result<(), TaskError> {
        let task_id = self.canonical_or_raw(task_id).await;
        if !self.tasks.read().await.contains_key(&task_id) {
            return Err(TaskError::NotFound(task_id));
        }
        self.backgrounders.lock().await.insert(task_id, requester);
        Ok(())
    }

    /// claude-code `Upt` — is this task backgroundable right now?
    ///
    /// For a shell: not already backgrounded, and there is a live command behind
    /// it (`Boolean(e.shellCommand)`; here, a bound requester). A terminal row
    /// has nothing to background.
    fn shell_is_backgroundable(state: &TaskState) -> bool {
        matches!(state, TaskState::LocalBash(bash)
            if bash.is_backgrounded != Some(true) && !bash.base.status.is_terminal())
    }

    /// claude-code `H_t` — whether anything can be backgrounded, i.e. whether
    /// the Ctrl+B affordance should be offered at all.
    pub async fn has_backgroundable_tasks(&self) -> bool {
        let map = self.tasks.read().await;
        let requesters = self.backgrounders.lock().await;
        map.iter()
            .any(|(id, state)| Self::shell_is_backgroundable(state) && requesters.contains_key(id))
    }

    /// claude-code `Wer` → `I_t` — move one still-running foreground shell to
    /// the background. Returns whether it moved.
    ///
    /// Order matters and is the oracle's: ask the command to detach FIRST
    /// (`if(!t.background(e)) return!1`), and only then flip `isBackgrounded`.
    /// Flipping first would leave a row claiming to be a background task if the
    /// request could not be delivered.
    pub async fn background_task(&self, task_id: &str) -> bool {
        let task_id = self.canonical_or_raw(task_id).await;
        {
            let map = self.tasks.read().await;
            match map.get(&task_id) {
                Some(state) if Self::shell_is_backgroundable(state) => {}
                _ => return false,
            }
        }
        let Some(requester) = self.backgrounders.lock().await.get(&task_id).cloned() else {
            return false;
        };
        requester.background().await;
        let mut map = self.tasks.write().await;
        if let Some(TaskState::LocalBash(bash)) = map.get_mut(&task_id) {
            bash.is_backgrounded = Some(true);
        }
        true
    }

    /// claude-code `bjn` + `JFe` — the two rosters a "no task found" message
    /// names.
    ///
    /// `bjn` lists a teammate by `identity.agentId`, which upstream is the
    /// `name@team` string. Here that form lives in the alias map (built at
    /// spawn by `aliases_for_spawn`), so the roster reverse-looks-it-up and
    /// falls back to the raw agent id when a teammate has no team.
    pub async fn not_found_rosters(
        &self,
        caller_agent_id: Option<&str>,
        named_agent_ids: &[String],
    ) -> platform_api::task_registry::TaskNotFoundRosters {
        let map = self.tasks.read().await;
        let aliases = self.aliases.read().await;
        // task id → its most addressable alias (`name@team` beats a bare name
        // beats the raw id), matching what the model is told to pass back.
        let mut addressable: HashMap<&str, &str> = HashMap::new();
        for (alias, task_id) in aliases.iter() {
            let better = addressable
                .get(task_id.as_str())
                .is_none_or(|current| !current.contains('@') && alias.contains('@'));
            if better {
                addressable.insert(task_id.as_str(), alias.as_str());
            }
        }

        let mut running_teammates = Vec::new();
        let mut background_agents = Vec::new();
        for (id, state) in map.iter() {
            match state {
                TaskState::InProcessTeammate(teammate)
                    if teammate.base.status == TaskStatus::Running =>
                {
                    running_teammates.push(
                        addressable
                            .get(id.as_str())
                            .map_or_else(|| teammate.agent_id.to_string(), ToString::to_string),
                    );
                }
                TaskState::LocalAgent(agent)
                    if agent.base.status == TaskStatus::Running
                        && agent.is_backgrounded
                        && Some(id.as_str()) != caller_agent_id
                        && agent.subagent_type != "main-session"
                        && !named_agent_ids.iter().any(|named| named == id) =>
                {
                    // `p.description ? `${p.id} (${Vn(p.description)})` : p.id`.
                    background_agents.push(if agent.base.description.is_empty() {
                        id.clone()
                    } else {
                        format!(
                            "{id} ({})",
                            platform_api::display::sanitize_display(&agent.base.description)
                        )
                    });
                }
                _ => {}
            }
        }
        running_teammates.sort();
        background_agents.sort();
        platform_api::task_registry::TaskNotFoundRosters {
            running_teammates,
            background_agents,
        }
    }

    /// claude-code `Ode` — move the task that owns `tool_use_id` to the
    /// background. Returns whether it moved.
    ///
    /// The oracle scans for the FIRST row with that `toolUseId` and returns
    /// early from the scan whether or not it could be backgrounded, so a
    /// terminal row does not fall through to some other task that happens to
    /// share the id.
    pub async fn background_task_for_tool_use(&self, tool_use_id: &str) -> bool {
        let target = {
            let map = self.tasks.read().await;
            map.iter()
                .find(|(_, state)| state.base().tool_use_id.as_deref() == Some(tool_use_id))
                .map(|(id, state)| (id.clone(), Self::shell_is_backgroundable(state)))
        };
        match target {
            Some((id, true)) => self.background_task(&id).await,
            _ => false,
        }
    }

    /// claude-code `zM` — move everything backgroundable to the background and
    /// report how many moved.
    ///
    /// The oracle runs two passes over the SAME snapshot, `local_bash` first and
    /// every other type second. Only the shell pass exists here; the agent pass
    /// (`s9`) needs foreground agents to be registered at all, which they are
    /// not yet — see `AGT-04` in `docs/task-parity-audit-2026-09-07.md`. The
    /// two-pass shape is kept so adding it is an insertion, not a rewrite.
    pub async fn background_all_tasks(&self) -> usize {
        let shells: Vec<String> = {
            let map = self.tasks.read().await;
            map.iter()
                .filter(|(_, state)| Self::shell_is_backgroundable(state))
                .map(|(id, _)| id.clone())
                .collect()
        };
        let mut moved = 0;
        for id in shells {
            if self.background_task(&id).await {
                moved += 1;
            }
        }
        // Pass 2 (`s9` over non-`local_bash` rows) lands with AGT-04.
        moved
    }

    /// Kill the background shells a finishing agent started, and report how
    /// many were still running.
    ///
    /// claude-code sweeps these in its agent-run cleanup (`nHn`), which is what
    /// makes the Bash tool's promise true: a synchronous subagent is told its
    /// backgrounded command "is terminated when you give your final response".
    /// Only shells owned by THIS agent are touched, so the main session's and
    /// other agents' commands are untouched.
    ///
    /// The sweep is silent. `nHn` is two halves — kill the rows, then
    /// `dG((r)=>r.agentId===e)`, where `dG` is
    /// `pendingNotificationQueue.dequeueAllMatching` — so the kill
    /// notifications it just produced are dequeued again and never reach the
    /// model. This engine has no per-agent notification queue (it derives
    /// notifications from state: terminal AND not `notified`), so the
    /// equivalent is to stamp `notified` on each row as it is killed.
    /// Without that, tidying up after a subagent spits a
    /// `<task-notification> … was stopped` into the MAIN session for every
    /// shell the subagent happened to leave running.
    ///
    /// Note this is the opposite of a user-initiated `TaskStop`, where the stop
    /// notification is the point; the oracle drops these because nothing is
    /// left to read them.
    pub async fn kill_background_shells_for_agent(&self, agent_id: protocol::AgentId) -> usize {
        let owned: Vec<String> = {
            let map = self.tasks.read().await;
            map.values()
                .filter_map(|state| {
                    let base = state.base();
                    let owned_shell = matches!(state, TaskState::LocalBash(_))
                        && base.creator_agent_id == Some(agent_id)
                        && !base.status.is_terminal();
                    owned_shell.then(|| base.id.clone())
                })
                .collect()
        };
        let mut killed = 0;
        for id in owned {
            if self.kill(&id).await.is_ok() {
                killed += 1;
                // The `dG` half: suppress the notification the kill just armed.
                let _ = self.mark_notified(&id).await;
            }
        }
        killed
    }

    /// Settle a background shell task once its child has been reaped.
    ///
    /// Terminal status follows claude-code `Fpt`: an interrupted/killed child is
    /// `killed`, exit code `0` is `completed`, anything else (including an
    /// unknown code) is `failed`. The exit code is written first so the drained
    /// `<task-notification>` can render `(exit code N)`.
    ///
    /// # Errors
    /// Returns [`TaskError::NotFound`] when the id is unknown.
    pub async fn settle_background_bash(
        &self,
        task_id: &str,
        exit_code: Option<i32>,
        killed: bool,
    ) -> Result<(), TaskError> {
        let task_id = self.canonical_or_raw(task_id).await;
        if let Some(code) = exit_code {
            let _ = self.set_bash_exit_code(&task_id, code).await;
        }
        let status = if killed {
            TaskStatus::Killed
        } else if exit_code == Some(0) {
            TaskStatus::Completed
        } else {
            TaskStatus::Failed
        };
        let output_file = {
            let mut map = self.tasks.write().await;
            match map.get_mut(&task_id) {
                Some(entry) if entry.base().status.is_terminal() => return Ok(()),
                Some(entry) => {
                    entry.base_mut().end_time = Some(SystemTime::now());
                    Some(entry.base().output_file.clone())
                }
                // No record: the identity was allocated for a command that was
                // never backgrounded, so there is nothing to settle.
                None => return Ok(()),
            }
        };
        // claude-code closes a background shell's output file with a status
        // trailer so a later `Read` shows how it ended (`Bpt`:
        // `s1e(e, "\n[" + (killed ? "killed" : "exited with code " + (code ?? "unknown")) + "]\n")`,
        // 2.1.263 `src_160988549.js` @4279785).
        if let Some(output_file) = output_file {
            let trailer = if killed {
                "\n[killed]\n".to_string()
            } else {
                let code = exit_code.map_or_else(|| "unknown".to_string(), |c| c.to_string());
                format!("\n[exited with code {code}]\n")
            };
            let _ = self.output_manager.append(&output_file, &trailer).await;
        }
        self.cleanups.lock().await.remove(&task_id);
        self.set_status(&task_id, status).await?;
        Ok(())
    }

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
                    teammate_name: settled_base.creator_teammate_name.clone(),
                    team_name: settled_base.creator_team_name.clone(),
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
        //    `task_id` and prepares the real worker; the returned id is the one
        //    callers (and `kill`) key on. Handlers with an activation barrier
        //    do not start external work until the commit point below.
        let ctx = TaskContext {
            fs: self.fs.clone(),
            runtime: self.runtime.clone(),
        };
        let mut handle = handler.spawn(input.clone(), ctx).await?;
        let id = handle.task_id.clone();
        let cleanup = handle.cleanup.clone();
        let mut publication_guard = SpawnPublicationGuard::new(self, id.clone(), cleanup.clone());

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
            creator_teammate_name: None,
            creator_team_name: None,
            creator_agent_id: None,
        };
        let mut state = state_for_spawn(base, &input);
        // Fusion handlers resolve their live settings before returning the
        // prepared handle. Publish that captured timeout with the state before
        // activation so print-mode waiters cannot observe a task without its
        // per-run deadline, and never need to reload mutable settings.
        if let (Some(timeout_ms), TaskState::LocalFusion(fusion)) =
            (handle.fusion_timeout_ms, &mut state)
        {
            fusion.effective_timeout_ms = Some(timeout_ms);
        }
        // 4. Publish every registry artifact as one transaction from the point
        //    of view of task readers. Holding the task write lock while the
        //    secondary guards are acquired prevents `list` / `get` / `kill`
        //    from observing a row before its handler route and cleanup exist.
        //    All guards are acquired before the first insert, so cancellation
        //    while waiting for a guard leaves no partial registry artifacts.
        let spawn_aliases = aliases_for_spawn(&input);
        {
            let mut tasks = self.tasks.write().await;
            let mut spawned = self.spawned.write().await;
            let mut cleanups = self.cleanups.lock().await;
            let mut aliases = self.aliases.write().await;

            tasks.insert(id.clone(), state);
            spawned.insert(id.clone(), task_type);
            if let Some(cleanup) = cleanup {
                cleanups.insert(id.clone(), cleanup);
            }
            for alias in spawn_aliases {
                if !alias.is_empty() && alias != id {
                    aliases.insert(alias, id.clone());
                }
            }
        }

        // Best-effort `TaskCreated` fire — the production task-creation path
        // (alongside `create`'s placeholder path). Both insert a new task row,
        // so both fire. Runs after the write guards drop. No-op when no firer is
        // registered.
        self.fire_task_created(&id, task_type, &description_for_hook)
            .await;

        // This is the commit point for handler-owned workers. Before this line,
        // dropping the registry future drops the unconsumed activation and the
        // prepared worker exits without starting. No await follows activation,
        // so callers cannot observe a partially committed successful spawn.
        handle.activate();
        publication_guard.disarm();

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

    /// Scope workflow rows in `TaskList` / `/workflows` to one session.
    /// `None` leaves all workflows visible (desktop / single-session behavior).
    pub fn set_workflow_session_filter(&self, session_uuid: Option<String>) {
        let mut guard = self
            .workflow_session_filter
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = session_uuid;
    }

    pub(crate) fn workflow_visible_in_current_session(&self, state: &TaskState) -> bool {
        let Some(current_session) = self
            .workflow_session_filter
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
        else {
            return true;
        };
        match state {
            TaskState::LocalWorkflow(workflow) => {
                workflow.session_uuid.as_deref() == Some(current_session.as_str())
            }
            _ => true,
        }
    }

    fn workflow_run_id_blocks_launch(status: TaskStatus) -> bool {
        matches!(status, TaskStatus::Pending | TaskStatus::Running)
    }

    /// Atomically reserve `run_id` for a new workflow launch so another launch
    /// cannot pass the live-run check before the task row exists.
    pub async fn try_reserve_workflow_run_id(
        &self,
        run_id: &str,
    ) -> Result<WorkflowRunReservation, TaskError> {
        {
            let mut reservations = self
                .workflow_launch_reservations
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !reservations.insert(run_id.to_string()) {
                return Err(TaskError::Internal(format!(
                    "workflow run id {run_id} is already launching"
                )));
            }
        }
        let reservation = WorkflowRunReservation {
            reservations: self.workflow_launch_reservations.clone(),
            run_id: Some(run_id.to_string()),
        };
        let tasks = self.tasks.read().await;
        if let Some((task_id, status)) = tasks.iter().find_map(|(task_id, state)| match state {
            TaskState::LocalWorkflow(workflow)
                if workflow.run_id.as_deref() == Some(run_id)
                    && Self::workflow_run_id_blocks_launch(workflow.base.status) =>
            {
                Some((task_id.clone(), workflow.base.status))
            }
            _ => None,
        }) {
            let state = match status {
                TaskStatus::Pending => "pending",
                TaskStatus::Running => "running",
                _ => "live",
            };
            return Err(TaskError::Internal(format!(
                "workflow run id {run_id} is already {state} on task {task_id}"
            )));
        }
        drop(tasks);
        Ok(reservation)
    }

    /// Find a live `local_workflow` task whose effective run id equals
    /// `run_id`, returning its task id. Backs claude-code's resume gate
    /// (Workflow validateInput errorCode 3): a `resumeFromRunId` naming a
    /// still-running workflow must be rejected — two runs sharing a run id
    /// would race on the same journal.
    pub async fn find_running_workflow_by_run_id(&self, run_id: &str) -> Option<String> {
        let tasks = self.tasks.read().await;
        tasks.iter().find_map(|(task_id, state)| match state {
            TaskState::LocalWorkflow(w)
                if Self::workflow_run_id_blocks_launch(w.base.status)
                    && w.run_id.as_deref() == Some(run_id) =>
            {
                Some(task_id.clone())
            }
            _ => None,
        })
    }

    /// Return non-terminal local-app workflow tasks that hold authority over
    /// `app_id`. Delete flows use this as a guard before removing the app
    /// directory.
    ///
    /// Reads each task's typed [`crate::scope::LocalAppWorkflowTaskScope`]
    /// (design §18 Phase -1 step 8 / §8.1) instead of matching `workflow_id`
    /// against this crate's (since-deleted) `LOCAL_APP_BUILD_WORKFLOWS` array
    /// and then parsing `app_id` out of caller-supplied `args` JSON. That
    /// used to be a genuine forgery
    /// vector: a custom workflow could declare a `workflow_id` naming one of
    /// this crate's two real build workflows and an `args.app_id` for
    /// whichever app the forger chose, and this guard would block that
    /// app's delete on the forger's say-so alone (§8.1: a custom workflow
    /// must get nothing "即使伪造 `meta.name` 或 `args.app_id`"). `workflow_id`
    /// and `args` are now IGNORED here entirely -- a task blocks `app_id`'s
    /// delete iff it carries `Some(scope)` whose `scope.app_id() == app_id`
    /// (`scope.blocks_delete()`, which every purpose satisfies -- see that
    /// method's doc comment).
    ///
    /// A task with `scope: None` never blocks any app's delete. That is a
    /// deliberate choice, not an oversight: an unscoped row is one the Host
    /// never vouched for, and treating it as blocking would either (a) match
    /// by `workflow_id`/`args` again -- reopening the exact forgery vector
    /// above -- or (b) block deleting every app while any unscoped workflow
    /// runs, which a forged workflow could trivially exploit as a
    /// denial-of-service against unrelated apps. Denying by default is the
    /// only option that does not reintroduce caller-controlled authority.
    ///
    /// A scope reaches a row through
    /// [`crate::task_trait::TaskSpawnInput::LocalWorkflow`]'s `scope` field,
    /// minted by the Host that resolved the app -- so a genuine in-flight
    /// build DOES block its app's delete. See that field's doc comment for
    /// what a `Some` proves.
    /// The active workspace lease (checked independently by callers, e.g.
    /// `engine-mobile`'s `handle_delete_app`) remains a second guard for a
    /// `Build`-purpose task that has actually acquired it; this guard's job
    /// is the wider one -- also covering `UseTest`/`McpAuthoring` scopes,
    /// which never take that lease at all. `registry_test.rs`'s
    /// `a_build_purpose_blocks_delete_at_the_guard`,
    /// `a_use_test_purpose_still_blocks_delete_at_the_guard` and
    /// `an_mcp_authoring_purpose_still_blocks_delete_at_the_guard` pin one
    /// purpose each HERE, at the guard, which is a stronger claim than
    /// `scope.rs`'s `every_purpose_blocks_delete` (that one only proves the
    /// predicate's answer, not that this function asks it).
    ///
    /// Two of the three arms are reachable in production: `for_build` (three
    /// mints in `engine-mobile/src/workflow_support.rs`) and
    /// `for_mcp_authoring` (one, in `launch`). `for_use_test` has no
    /// production mint -- but not because its workflow is unbuilt: the use-test
    /// workflow script ships, and nothing mints its scope. That arm is
    /// live contract and dead traffic, exercised only by the tests named
    /// above. See `scope.rs`'s `blocks_delete` for the per-constructor
    /// line numbers.
    pub async fn find_nonterminal_local_app_workflows(&self, app_id: &str) -> Vec<String> {
        let tasks = self.tasks.read().await;
        tasks
            .iter()
            .filter_map(|(task_id, state)| match state {
                TaskState::LocalWorkflow(workflow)
                    if !workflow.base.status.is_terminal()
                        && workflow.scope.as_ref().is_some_and(|scope| {
                            scope.blocks_delete() && scope.app_id() == app_id
                        }) =>
                {
                    Some(task_id.clone())
                }
                _ => None,
            })
            .collect()
    }

    /// Attach the on-disk resume identity to a newly launched workflow row.
    pub async fn set_workflow_resume_metadata(
        &self,
        task_id: &str,
        script_path: String,
        transcript_dir: std::path::PathBuf,
    ) -> Result<(), TaskError> {
        let task_id = self.canonical_or_raw(task_id).await;
        let mut tasks = self.tasks.write().await;
        let state = tasks
            .get_mut(&task_id)
            .ok_or_else(|| TaskError::NotFound(task_id.clone()))?;
        let TaskState::LocalWorkflow(workflow) = state else {
            return Err(TaskError::Internal(format!(
                "task {task_id} is not a local workflow"
            )));
        };
        workflow.script_path = Some(script_path);
        workflow.transcript_dir = Some(transcript_dir);
        Ok(())
    }

    /// Rebuild one workflow checkpoint without starting its script, with NO
    /// Local App authority (`scope: None`). This is the mobile equivalent of
    /// Claude Code's `registerAdoptedWorkflowTask`, and it is what a caller
    /// that has not independently re-validated the checkpoint against
    /// Host-owned state should call -- it reproduces the exact behavior this
    /// method has always had.
    ///
    /// See [`Self::register_adopted_workflow_with_scope`] for the sibling
    /// entry point a caller uses once it HAS re-resolved authority, and
    /// [`AdoptedWorkflow`]'s doc comment for why `scope` is never derived from
    /// the checkpoint's own fields here.
    pub async fn register_adopted_workflow(
        &self,
        adopted: AdoptedWorkflow,
    ) -> Result<(), TaskError> {
        self.register_adopted_workflow_with_scope(adopted, None)
            .await
    }

    /// Rebuild one workflow checkpoint, stamping `scope` on the resulting task
    /// row exactly as
    /// [`crate::task_trait::TaskSpawnInput::LocalWorkflow`]'s `scope` field
    /// does for a freshly spawned workflow.
    ///
    /// # The residual this closes
    ///
    /// [`Self::register_adopted_workflow`] always stamped `scope: None` on an
    /// adopted row, because [`AdoptedWorkflow`] is built straight from the
    /// on-disk checkpoint (`adopt.json`), and that checkpoint's fields are
    /// exactly as untrustworthy here as they are everywhere else in this
    /// crate: they record whatever the ORIGINAL caller supplied, so a custom
    /// workflow's checkpoint carries a forged `args.app_id` just as
    /// faithfully as a real build's. An adopted row is `Paused` --
    /// non-terminal -- so `scope: None` meant a Local App build in flight
    /// across an engine restart stopped blocking its app's delete, even
    /// though the pre-scope, name-matching guard this crate used to have DID
    /// block it. That is the gap this method closes.
    ///
    /// It closes the gap by taking `scope` as a SEPARATE argument rather than
    /// a field on [`AdoptedWorkflow`] itself, so the type that carries
    /// checkpoint-recorded (untrusted) fields can never be mistaken for
    /// Host-vouched authority. The caller (today, `engine-mobile`'s
    /// `MobileWorkflowCheckpointStore::adopt_session`) is responsible for
    /// RE-DERIVING `scope` from state it re-resolves on load -- the same
    /// app-must-exist / must-be-scaffolded / must-be-pinned checks
    /// `apply_materialized_local_app_collections_with_identity` applies to a
    /// live launch, run again here against the checkpoint's persisted script
    /// bytes, with the checkpoint's `app_id` used only as a lookup key, never
    /// as authority by itself (see that function's doc comment for the full
    /// reasoning, which applies unchanged). Passing `None` is always safe --
    /// it reproduces [`Self::register_adopted_workflow`]'s existing behavior
    /// -- so a caller that cannot or does not want to re-validate loses
    /// nothing by omitting scope.
    pub async fn register_adopted_workflow_with_scope(
        &self,
        adopted: AdoptedWorkflow,
        scope: Option<crate::scope::LocalAppWorkflowTaskScope>,
    ) -> Result<(), TaskError> {
        let valid_task_id = adopted.task_id.len() == 9
            && adopted
                .task_id
                .starts_with(TaskType::LocalWorkflow.id_prefix())
            && adopted
                .task_id
                .bytes()
                .skip(1)
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit());
        if !valid_task_id {
            return Err(TaskError::Internal(format!(
                "invalid adopted workflow task id {:?}",
                adopted.task_id
            )));
        }
        if !adopted.run_id.starts_with("wf_") {
            return Err(TaskError::Internal(format!(
                "invalid adopted workflow run id {:?}",
                adopted.run_id
            )));
        }

        let _reservation = self.try_reserve_workflow_run_id(&adopted.run_id).await?;
        let output_file = match self.output_manager.allocate(&adopted.task_id).await {
            Ok(path) => path,
            Err(crate::output_manager::OutputError::AlreadyExists(_)) => self
                .output_manager
                .path_for(&adopted.task_id)
                .map_err(|error| TaskError::Io(error.to_string()))?,
            Err(error) => return Err(TaskError::Io(error.to_string())),
        };
        let mut tasks = self.tasks.write().await;
        if let Some(existing) = tasks.get(&adopted.task_id) {
            if matches!(
                existing,
                TaskState::LocalWorkflow(workflow)
                    if Self::workflow_run_id_blocks_launch(workflow.base.status)
            ) {
                return Err(TaskError::Internal(format!(
                    "cannot adopt workflow {}: live task {} already exists",
                    adopted.run_id, adopted.task_id
                )));
            }
        }
        if let Some(existing_task_id) = tasks.iter().find_map(|(task_id, state)| match state {
            TaskState::LocalWorkflow(workflow)
                if workflow.run_id.as_deref() == Some(adopted.run_id.as_str())
                    && Self::workflow_run_id_blocks_launch(workflow.base.status) =>
            {
                Some(task_id.clone())
            }
            _ => None,
        }) {
            return Err(TaskError::Internal(format!(
                "cannot adopt workflow {}: live task {} already owns that run id",
                adopted.run_id, existing_task_id
            )));
        }
        let state = TaskState::LocalWorkflow(crate::state::LocalWorkflowTaskState {
            base: TaskStateBase {
                id: adopted.task_id.clone(),
                task_type: TaskType::LocalWorkflow,
                status: TaskStatus::Paused,
                description: adopted.description,
                tool_use_id: None,
                start_time: adopted.start_time,
                end_time: None,
                total_paused_ms: 0,
                output_file,
                output_offset: 0,
                notified: true,
                creator_teammate_name: None,
                creator_team_name: None,
                creator_agent_id: None,
            },
            session_uuid: adopted.session_uuid,
            workflow_id: adopted.workflow_id,
            script: String::new(),
            resume_from_run_id: Some(adopted.run_id.clone()),
            args: adopted.args,
            run_id: Some(adopted.run_id),
            script_path: Some(adopted.script_path),
            transcript_dir: Some(std::path::PathBuf::from(adopted.transcript_dir)),
            current_step: 0,
            outcome: Default::default(),
            // `scope` is exactly what the caller passed to
            // `register_adopted_workflow_with_scope` -- `None` when reached
            // via the plain `register_adopted_workflow` wrapper (preserving
            // that method's pre-existing behavior), or whatever
            // `adopt_session` re-derived from Host-owned state for this
            // specific checkpoint. See this method's doc comment for the
            // residual this closes and why `scope` travels as a separate
            // argument rather than a field on `AdoptedWorkflow`. Note it is
            // still never PERSISTED (see `LocalWorkflowTaskState::scope`'s
            // doc comment) -- it is re-derived fresh on every adoption, same
            // as a live spawn re-derives it fresh on every launch.
            scope,
        });
        tasks.insert(adopted.task_id, state);
        Ok(())
    }

    /// Remove the paused predecessor after an explicit resume has successfully
    /// spawned a replacement task for the same workflow run in the same session.
    pub async fn remove_paused_workflow_by_run_id(&self, session_uuid: &str, run_id: &str) {
        self.tasks.write().await.retain(|_, state| {
            !matches!(
                state,
                TaskState::LocalWorkflow(workflow)
                    if workflow.base.status == TaskStatus::Paused
                        && workflow.session_uuid.as_deref() == Some(session_uuid)
                        && workflow.run_id.as_deref() == Some(run_id)
            )
        });
    }

    /// Record a shell-backed task's child exit code (M8 cc2.1.198 "Task
    /// panels: no stuck Running"): the worker's status sink reports the exit
    /// code alongside the terminal status once the process ends; `output()`
    /// projects it as `exit_code`/`done`. Non-bash variants are a benign
    /// no-op (only bash and monitor children carry an OS exit code). `NotFound` for an
    /// unknown id — the sink swallows it (racing teardown tolerance).
    pub async fn set_bash_exit_code(&self, task_id: &str, exit_code: i32) -> Result<(), TaskError> {
        let task_id = self.canonical_or_raw(task_id).await;
        let mut map = self.tasks.write().await;
        let entry = map
            .get_mut(&task_id)
            .ok_or_else(|| TaskError::NotFound(task_id.clone()))?;
        match entry {
            TaskState::LocalBash(b) => b.exit_code = Some(exit_code),
            TaskState::Monitor(m) => m.exit_code = Some(exit_code),
            _ => {}
        }
        Ok(())
    }

    /// Queue one live `monitor_ws` stdout event for the next turn-boundary
    /// drain. The queue is deliberately bounded: a detached/high-volume
    /// producer must not grow session memory without limit while the model is
    /// busy. The monitor handler performs its own token-bucket suppression;
    /// this final cap protects the registry boundary as well.
    pub async fn enqueue_monitor_event(&self, task_id: &str, event: &str) -> Result<(), TaskError> {
        const MAX_PENDING_MONITOR_EVENTS: usize = 1_024;

        let task_id = self.canonical_or_raw(task_id).await;
        let notification = {
            let tasks = self.tasks.read().await;
            let state = tasks
                .get(&task_id)
                .ok_or_else(|| TaskError::NotFound(task_id.clone()))?;
            let TaskState::Monitor(monitor) = state else {
                return Err(TaskError::Unsupported);
            };
            if monitor.base.status.is_terminal() {
                return Ok(());
            }
            platform_api::task_registry::TaskNotification {
                task_id: monitor.base.id.clone(),
                task_type: "monitor_ws".to_string(),
                status: "running".to_string(),
                description: monitor.base.description.clone(),
                tool_use_id: monitor.base.tool_use_id.clone(),
                output_path: Some(monitor.base.output_file.to_string_lossy().into_owned()),
                result: Some(event.to_string()),
                ..Default::default()
            }
        };
        let mut queue = self.pending_monitor_events.lock().await;
        if queue.len() >= MAX_PENDING_MONITOR_EVENTS {
            queue.pop_front();
        }
        queue.push_back(notification);
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
    /// Seed an alias → task-id mapping the way `aliases_for_spawn` would.
    #[cfg(test)]
    pub(crate) async fn register_alias_for_test(&self, alias: &str, task_id: &str) {
        self.aliases
            .write()
            .await
            .insert(alias.to_string(), task_id.to_string());
    }

    pub(crate) async fn insert_state_for_test(&self, state: TaskState) {
        let id = state.base().id.clone();
        self.tasks.write().await.insert(id, state);
    }

    async fn fire_task_completed_hook(
        &self,
        task_id: &str,
        status: TaskStatus,
        updated: &TaskState,
    ) {
        if let Some(firer) = &self.task_completed_firer {
            let status_str = match status {
                TaskStatus::Completed => Some("completed"),
                TaskStatus::Failed => Some("failed"),
                _ => None,
            };
            if let Some(status_str) = status_str {
                let base = updated.base();
                firer
                    .fire(hooks::TaskCompletedFire {
                        task_id: task_id.to_string(),
                        status: status_str.to_string(),
                        task_subject: base.description.clone(),
                        task_description: Some(base.description.clone()),
                        teammate_name: base.creator_teammate_name.clone(),
                        team_name: base.creator_team_name.clone(),
                    })
                    .await;
            }
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
        let task_id = self.canonical_or_raw(task_id).await;
        let updated = {
            let mut map = self.tasks.write().await;
            let entry = map
                .get_mut(&task_id)
                .ok_or_else(|| TaskError::NotFound(task_id.clone()))?;
            // Task lifecycle is absorbing at the first terminal transition.
            // In particular, a late handler failure must not overwrite an
            // explicit kill (or fire TaskCompleted after Killed already won).
            if entry.base().status.is_terminal() {
                return Ok(entry.clone());
            }
            match entry {
                TaskState::LocalBash(b) => b.base.status = status,
                TaskState::LocalAgent(a) => a.base.status = status,
                TaskState::RemoteAgent(r) => r.base.status = status,
                TaskState::InProcessTeammate(t) => t.base.status = status,
                TaskState::LocalWorkflow(w) => w.base.status = status,
                TaskState::MonitorMcp(m) => m.base.status = status,
                TaskState::Monitor(m) => m.base.status = status,
                TaskState::McpTask(m) => m.base.status = status,
                TaskState::Dream(d) => d.base.status = status,
                TaskState::LocalFusion(f) => f.base.status = status,
            }
            entry.clone()
            // `map` write-guard drops here — the best-effort hook fire below
            // runs WITHOUT holding the registry lock so a slow/blocking hook
            // never stalls other task operations.
        };

        self.fire_task_completed_hook(&task_id, status, &updated)
            .await;

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
        usage: Option<platform_api::task_registry::AgentRunUsage>,
        agent_id: Option<protocol::AgentId>,
        agent_name: Option<String>,
        team_name: Option<String>,
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
        self.pending_rest.write().await.insert(
            task_id,
            RestPayload {
                result,
                usage,
                agent_id,
                agent_name,
                team_name,
            },
        );
    }

    fn has_live_background_children_locked(
        map: &HashMap<String, TaskState>,
        rested_agent_id: Option<protocol::AgentId>,
        agent_name: Option<&str>,
        team_name: Option<&str>,
    ) -> bool {
        if let Some(rested_agent_id) = rested_agent_id {
            return map.values().any(|state| {
                let base = state.base();
                !base.status.is_terminal() && base.creator_agent_id == Some(rested_agent_id)
            });
        }

        let Some(agent_name) = agent_name.filter(|name| !name.is_empty()) else {
            return false;
        };
        map.values().any(|state| {
            let base = state.base();
            !base.status.is_terminal()
                && base.creator_teammate_name.as_deref() == Some(agent_name)
                && base.creator_team_name.as_deref() == team_name
        })
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
    /// Record a terminating `local_agent`'s notification payload onto its state
    /// ([`crate::state::AgentOutcomeState::merge`] semantics: `Some`
    /// overwrites, `None` leaves the stored value).
    ///
    /// `outcome.error` lands on the state's own `error` field — the failed
    /// summary's `{error}` (claude `error || 'Unknown error'`) reads from there,
    /// and until this existed nothing in production ever wrote it, so EVERY
    /// failed background agent reported `Unknown error`.
    ///
    /// Callers must invoke this BEFORE the terminal `set_status`; see
    /// [`platform_api::task_registry::TaskRegistryHandle::set_agent_outcome`].
    pub async fn set_agent_outcome(
        &self,
        task_id: &str,
        outcome: platform_api::task_registry::AgentTerminalOutcome,
    ) {
        let task_id = self.canonical_or_raw(task_id).await;
        let mut map = self.tasks.write().await;
        if let Some(TaskState::LocalAgent(agent)) = map.get_mut(&task_id) {
            if let Some(error) = outcome.error.clone() {
                agent.error = Some(error);
            }
            agent.outcome.merge(outcome);
        }
    }

    /// Record a workflow's terminal payload before the status transition opens
    /// it to the notification drain.
    pub async fn set_workflow_outcome(
        &self,
        task_id: &str,
        outcome: platform_api::task_registry::WorkflowTerminalOutcome,
    ) {
        let task_id = self.canonical_or_raw(task_id).await;
        let mut map = self.tasks.write().await;
        if let Some(TaskState::LocalWorkflow(workflow)) = map.get_mut(&task_id) {
            workflow.outcome = outcome;
        }
    }

    /// Record a Fusion run's sanitized final text before the terminal status
    /// opens it to the notification drain.
    pub async fn set_fusion_outcome(&self, task_id: &str, run_id: String, final_text: String) {
        let task_id = self.canonical_or_raw(task_id).await;
        let mut map = self.tasks.write().await;
        if let Some(TaskState::LocalFusion(fusion)) = map.get_mut(&task_id) {
            fusion.run_id = Some(run_id);
            fusion.final_text = Some(final_text);
        }
    }

    /// Record a Fusion run's failure reason before the terminal `Failed`
    /// status opens it to the notification drain. Without this a failed
    /// `local_fusion` task notified as bare "failed" with no `<error>`
    /// section and no reason folded into the summary.
    pub async fn set_fusion_error(&self, task_id: &str, error: String) {
        let task_id = self.canonical_or_raw(task_id).await;
        let mut map = self.tasks.write().await;
        if let Some(TaskState::LocalFusion(fusion)) = map.get_mut(&task_id) {
            fusion.error = Some(error);
        }
    }

    /// Record a Fusion run's current progress-stage label (F005) — e.g.
    /// "Running panels 2/3" — surfaced on the `local_fusion` task DTO so a
    /// UI polling task state sees the same progress the Agent-tool path
    /// forwards as `subagent_activity`. Best-effort: a since-evicted task is
    /// a benign no-op.
    pub async fn set_fusion_stage(&self, task_id: &str, stage: String) {
        let task_id = self.canonical_or_raw(task_id).await;
        let mut map = self.tasks.write().await;
        if let Some(TaskState::LocalFusion(fusion)) = map.get_mut(&task_id) {
            fusion.stage = Some(stage);
        }
    }

    /// Record a Fusion run's egress profiles and usage summary alongside its
    /// terminal payload, before the terminal status opens it to the
    /// notification drain.
    pub async fn set_fusion_egress_and_usage(
        &self,
        task_id: &str,
        egress_profiles: Vec<String>,
        usage: Option<platform_api::task_registry::AgentRunUsage>,
    ) {
        let task_id = self.canonical_or_raw(task_id).await;
        let mut map = self.tasks.write().await;
        if let Some(TaskState::LocalFusion(fusion)) = map.get_mut(&task_id) {
            fusion.egress_profiles = egress_profiles;
            fusion.usage = usage;
        }
    }

    /// Record that [`platform_api::FusionCompletionSink::publish`]'s durable
    /// `<fusion-result>` session append has completed for a `Completed` run
    /// — called from `local_fusion`'s worker AFTER `sink.publish` resolves,
    /// necessarily after [`Self::finish_fusion_terminal`] already flipped the
    /// status (the notification drain's ordering requirement runs the other
    /// way and is unaffected). A one-shot host (print mode) that returns the
    /// instant it observes `Completed` can otherwise exit the process while
    /// the append is still in flight and lose the row entirely (review
    /// finding #17) — `apps/cli`'s `await_local_fusion_result_bounded` keeps
    /// polling a `Completed` run until this flips. Best-effort: a
    /// since-evicted task is a benign no-op.
    pub async fn mark_fusion_result_published(&self, task_id: &str) {
        let task_id = self.canonical_or_raw(task_id).await;
        let mut map = self.tasks.write().await;
        if let Some(TaskState::LocalFusion(fusion)) = map.get_mut(&task_id) {
            fusion.result_published = true;
        }
    }

    /// Atomically publish a Fusion run's terminal payload and terminal status.
    pub async fn finish_fusion_terminal(
        &self,
        task_id: &str,
        run_id: String,
        final_text: String,
        status: TaskStatus,
    ) -> Result<TaskState, TaskError> {
        let task_id = self.canonical_or_raw(task_id).await;
        let updated = {
            let mut map = self.tasks.write().await;
            let entry = map
                .get_mut(&task_id)
                .ok_or_else(|| TaskError::NotFound(task_id.clone()))?;
            let TaskState::LocalFusion(fusion) = entry else {
                return Ok(entry.clone());
            };
            if fusion.base.status.is_terminal() {
                return Ok(entry.clone());
            }
            fusion.run_id = Some(run_id);
            fusion.final_text = Some(final_text);
            fusion.base.status = status;
            entry.clone()
        };

        self.fire_task_completed_hook(&task_id, status, &updated)
            .await;
        Ok(updated)
    }

    /// Atomically publish a workflow's terminal payload and terminal status.
    ///
    /// This closes the outcome→status race: a concurrent kill/drain cannot
    /// observe a non-terminal workflow after its terminal payload already
    /// landed and then overwrite it as `Killed`.
    pub async fn finish_workflow_terminal(
        &self,
        task_id: &str,
        outcome: platform_api::task_registry::WorkflowTerminalOutcome,
        status: TaskStatus,
    ) -> Result<TaskState, TaskError> {
        let task_id = self.canonical_or_raw(task_id).await;
        let updated = {
            let mut map = self.tasks.write().await;
            let entry = map
                .get_mut(&task_id)
                .ok_or_else(|| TaskError::NotFound(task_id.clone()))?;
            let TaskState::LocalWorkflow(workflow) = entry else {
                return Ok(entry.clone());
            };
            if workflow.base.status.is_terminal() {
                return Ok(entry.clone());
            }
            workflow.outcome = outcome;
            workflow.base.status = status;
            entry.clone()
        };

        self.fire_task_completed_hook(&task_id, status, &updated)
            .await;
        Ok(updated)
    }

    /// Atomically commit a Host-validated Local App Build/UseTest success.
    ///
    /// This is intentionally not a generic workflow terminal primitive.  It
    /// accepts only a [`TaskState::LocalWorkflow`] row carrying an authenticated
    /// Build or UseTest scope, and it exists because those two workflows have
    /// two local publications that must linearize with the absorbing task
    /// state: the canonical result projection and the Host's already-prepared
    /// active QA receipt pointer.
    ///
    /// Expensive QA/evidence validation must happen before this method.  While
    /// the registry write lock is held, this method writes a bounded unverified
    /// spool placeholder, runs `publish_prepared`, makes canonical success
    /// authoritative only after publication, then assigns the exact resulting
    /// outcome/status. A competing kill therefore wins before these
    /// publications (and the closure is not called), or observes the committed
    /// terminal state afterward. `publish_prepared` must not call back into
    /// this registry, perform UI/network work, scan QA artifacts, or do large
    /// cleanup; those operations would introduce lock inversion or make the
    /// critical section unbounded. The returned boolean is true only when
    /// this call committed the prepared publication; an already-terminal row
    /// is returned with false so callers cannot emit a success summary for a
    /// publication closure that never ran.
    pub async fn commit_local_app_workflow_terminal<F, Fut>(
        &self,
        task_id: &str,
        outcome: platform_api::task_registry::WorkflowTerminalOutcome,
        publish_prepared: F,
    ) -> Result<(TaskState, bool), TaskError>
    where
        F: FnOnce() -> Fut + Send,
        Fut: std::future::Future<Output = Result<(), String>> + Send,
    {
        let canonical_result = outcome.result.clone().ok_or_else(|| {
            TaskError::Internal(
                "Local App terminal commit requires a canonical result payload".into(),
            )
        })?;
        let task_id = self.canonical_or_raw(task_id).await;
        let (updated, publication_committed) = {
            let mut map = self.tasks.write().await;
            let entry = map
                .get_mut(&task_id)
                .ok_or_else(|| TaskError::NotFound(task_id.clone()))?;
            let TaskState::LocalWorkflow(workflow) = entry else {
                return Err(TaskError::Unsupported);
            };
            let eligible = workflow.scope.as_ref().is_some_and(|scope| {
                matches!(
                    scope.purpose(),
                    crate::scope::LocalAppWorkflowPurpose::Build
                        | crate::scope::LocalAppWorkflowPurpose::UseTest
                )
            });
            if !eligible {
                return Err(TaskError::Unsupported);
            }
            if workflow.base.status.is_terminal() {
                return Ok((entry.clone(), false));
            }

            let output_file = workflow.base.output_file.clone();
            let canonical_result_error = self
                .output_manager
                .validate_terminal_result(&output_file, &canonical_result)
                .err()
                .map(|error| {
                    format!(
                        "local_app_completion_unverified: terminal spool replacement failed: {error}"
                    )
                });
            let publication_pending = serde_json::json!({
                "ok": false,
                "error": "local_app_completion_unverified: QA publication pending",
                "verified": false,
            })
            .to_string();
            let mut publication_committed = false;
            let commit_error = match canonical_result_error {
                Some(error) => Some(error),
                None => match self
                    .output_manager
                    .replace_terminal_result(&output_file, &publication_pending)
                    .await
                {
                    Ok(()) => match publish_prepared().await {
                        Ok(()) => {
                            publication_committed = true;
                            if let Err(error) = self
                                .output_manager
                                .replace_terminal_result_authoritative(
                                    &output_file,
                                    &canonical_result,
                                )
                                .await
                            {
                                tracing::warn!(
                                    task_id = %task_id,
                                    %error,
                                    "canonical Local App result is authoritative in memory; physical spool persistence failed after Host publication"
                                );
                            }
                            None
                        }
                        Err(error) => Some(format!(
                            "local_app_completion_unverified: QA publication failed: {error}"
                        )),
                    },
                    Err(error) => Some(format!(
                        "local_app_completion_unverified: terminal spool replacement failed: {error}"
                    )),
                },
            };

            if let Some(mut reason) = commit_error {
                let failure_payload = serde_json::json!({
                    "ok": false,
                    "error": reason,
                    "verified": false,
                })
                .to_string();
                // Make the failed projection authoritative before retrying
                // the filesystem rewrite. The physical spool still contains
                // only an unverified placeholder if this retry also fails.
                if let Err(error) = self
                    .output_manager
                    .replace_terminal_result_fail_closed(&output_file, &failure_payload)
                    .await
                {
                    reason.push_str(&format!(
                        "; terminal failure spool replacement failed: {error}"
                    ));
                }
                workflow.outcome = platform_api::task_registry::WorkflowTerminalOutcome {
                    result: None,
                    error: Some(reason),
                    ..outcome
                };
                workflow.base.status = TaskStatus::Failed;
            } else {
                workflow.outcome = outcome;
                workflow.base.status = TaskStatus::Completed;
            }
            (entry.clone(), publication_committed)
        };

        let status = updated.base().status;
        self.fire_task_completed_hook(&task_id, status, &updated)
            .await;
        Ok((updated, publication_committed))
    }

    /// [`Self::kill`] with the stop initiator recorded (`"parent"` / `"user"`).
    ///
    /// The reason is stamped BEFORE the kill dispatch so a handler that flips
    /// the task to `Killed` synchronously cannot let the drain observe a
    /// terminal task whose `killed_by` has not landed yet.
    pub async fn kill_with_reason(&self, task_id: &str, killed_by: &str) -> Result<(), TaskError> {
        let canonical = self.canonical_or_raw(task_id).await;
        if let Some(TaskState::LocalAgent(agent)) = self.tasks.write().await.get_mut(&canonical) {
            // Same reverse-race guard `kill` applies to the status: a task that
            // already finished on its own is not "stopped by" anyone, and
            // stamping one would leave a completed task carrying a stop
            // initiator.
            if !agent.base.status.is_terminal() {
                agent.outcome.killed_by = Some(killed_by.to_string());
            }
        }
        // claude-code captures `ue = GS(I)` BEFORE the kill, because the kill
        // clears the keepalive reasons the gate reads. Same here: resolve the
        // cascade target while the task is still resting.
        let cascade_from = self.resting_agent_holding_children(&canonical).await;
        let result = self.kill(task_id).await;
        if let Some(agent_id) = cascade_from {
            self.cascade_stop_descendants(agent_id, killed_by).await;
        }
        result
    }

    /// claude-code `GS(e)` — `type==="local_agent" && status==="completed" &&
    /// keepaliveReasons.size > 0`: an agent that already came to rest but is
    /// held open by live background children. Returns its agent id.
    ///
    /// The port spells "came to rest" as an armed `pending_rest` entry rather
    /// than a `completed` status (a resting agent stays `Running` here so it can
    /// be resumed), and `keepaliveReasons` — whose members are `agent:<childId>`
    /// entries for live children — as
    /// [`Self::has_live_background_children_locked`]. The entry survives across
    /// drains for exactly as long as the children do: the drain re-arms it
    /// through `requeue_deferred_rest`, which is what makes it readable as a
    /// gate here rather than a one-turn signal.
    ///
    /// Both halves are load-bearing. Cascading on live children ALONE would kill
    /// the children of an agent that is still actively working, which the oracle
    /// does not do; cascading on rest alone would fire for an agent with nothing
    /// left to hold open.
    async fn resting_agent_holding_children(&self, canonical: &str) -> Option<protocol::AgentId> {
        // Lock order is tasks → pending_rest everywhere else in this file; keep
        // it.
        let map = self.tasks.read().await;
        let TaskState::LocalAgent(agent) = map.get(canonical)? else {
            return None;
        };
        if agent.base.status.is_terminal() {
            return None;
        }
        let agent_id = agent.agent_id;
        if !self.pending_rest.read().await.contains_key(canonical) {
            return None;
        }
        Self::has_live_background_children_locked(&map, Some(agent_id), None, None)
            .then_some(agent_id)
    }

    /// claude-code's cascade block (`rY` @3598005): every still-live
    /// `local_agent` DESCENDANT of the stopped agent is stopped with it.
    ///
    /// Without this, stopping a resting parent leaves its children running with
    /// nothing left to report to — the parent that would have collected them is
    /// gone.
    ///
    /// Two things the oracle gets for free that this has to build:
    ///
    /// 1. **A parent index.** The oracle walks `r[p]` because a `local_agent`'s
    ///    task id IS its agent id. Here they are independent (`generate_task_id`
    ///    vs `LocalAgentTaskState::agent_id`), so the walk needs an explicit
    ///    `agent_id → task_id` map or it silently stops after one level and
    ///    grandchildren survive.
    /// 2. **Silence.** `CI(Me.id,r)` stamps `notified` before each child kill,
    ///    so the oracle emits nothing per cascaded child. This engine derives
    ///    notifications from `terminal && !notified`, so without the same stamp
    ///    a cascade would push one `Agent "…" was stopped by Claude` into the
    ///    parent's session per descendant — the hazard already documented on
    ///    [`Self::kill_background_shells_for_agent`].
    ///
    /// Children are stopped serially and their errors ignored, as the oracle
    /// does: one dead child must not abort the rest of the cascade.
    async fn cascade_stop_descendants(&self, stopped_agent_id: protocol::AgentId, killed_by: &str) {
        let victims: Vec<String> = {
            let map = self.tasks.read().await;
            let by_agent: HashMap<protocol::AgentId, &TaskState> = map
                .values()
                .filter_map(|state| match state {
                    TaskState::LocalAgent(agent) => Some((agent.agent_id, state)),
                    _ => None,
                })
                .collect();
            map.values()
                .filter_map(|state| {
                    let TaskState::LocalAgent(agent) = state else {
                        return None;
                    };
                    if agent.agent_id == stopped_agent_id || agent.base.status.is_terminal() {
                        return None;
                    }
                    // `hVe`: walk UP the parent chain, with the oracle's
                    // visited-set cycle guard.
                    let mut seen = std::collections::HashSet::new();
                    let mut parent = agent.base.creator_agent_id;
                    while let Some(pid) = parent {
                        if pid == stopped_agent_id {
                            return Some(agent.base.id.clone());
                        }
                        if !seen.insert(pid) {
                            break;
                        }
                        parent = by_agent.get(&pid).and_then(|s| s.base().creator_agent_id);
                    }
                    None
                })
                .collect()
        };
        for id in victims {
            // `CI` before the kill, exactly as the oracle orders it.
            let _ = self.mark_notified(&id).await;
            let _ = Box::pin(self.kill_with_reason(&id, killed_by)).await;
        }
    }

    pub async fn take_pending_task_notifications(
        &self,
    ) -> Vec<platform_api::task_registry::TaskNotification> {
        use crate::handle::{status_to_wire, task_type_to_wire};
        let mut out: Vec<_> = self.pending_monitor_events.lock().await.drain(..).collect();
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
        out.reserve(drain_ids.len());
        for id in drain_ids {
            let Some(state) = map.get(&id) else {
                continue;
            };
            let b = state.base();
            // Per-type fields the renderer needs beyond the shared base.
            let exit_code = match state {
                TaskState::LocalBash(bash) => bash.exit_code,
                TaskState::Monitor(monitor) => monitor.exit_code,
                _ => None,
            };
            let error = match state {
                TaskState::LocalAgent(agent) => agent.error.clone(),
                TaskState::LocalWorkflow(workflow) => workflow.outcome.error.clone(),
                TaskState::LocalFusion(fusion) => fusion.error.clone(),
                _ => None,
            };
            let workflow_outcome = match state {
                TaskState::LocalWorkflow(workflow) => Some(workflow.outcome.clone()),
                _ => None,
            };
            let workflow_metadata = match state {
                TaskState::LocalWorkflow(workflow) => Some((
                    workflow.script_path.clone(),
                    workflow.run_id.clone(),
                    workflow.args.clone(),
                    workflow
                        .transcript_dir
                        .as_ref()
                        .map(|path| path.to_string_lossy().into_owned()),
                )),
                _ => None,
            };
            let workflow_counts_available = workflow_outcome
                .as_ref()
                .is_some_and(|outcome| outcome.progress_counts_available);
            // `local_agent` optional sections — the payload the terminating run
            // reported via `set_agent_outcome` (and the stop initiator recorded
            // by `kill_with_reason`). Every other task type carries none of
            // them, which is the renderer's omit-the-clause case.
            let agent_outcome = match state {
                TaskState::LocalAgent(agent) => Some(agent.outcome.clone()),
                _ => None,
            };
            let agent_outcome = agent_outcome.unwrap_or_default();
            let fusion_state = match state {
                TaskState::LocalFusion(fusion) => Some(fusion.clone()),
                _ => None,
            };
            let fusion_final_text = fusion_state.as_ref().and_then(|f| f.final_text.clone());
            let output_path = self
                .output_manager
                .physical_output_is_authoritative(&b.output_file)
                .await
                .then(|| b.output_file.to_string_lossy().into_owned());
            out.push(platform_api::task_registry::TaskNotification {
                task_id: b.id.clone(),
                task_type: task_type_to_wire(b.task_type).to_string(),
                status: status_to_wire(b.status).to_string(),
                description: b.description.clone(),
                tool_use_id: b.tool_use_id.clone(),
                output_path,
                exit_code,
                error,
                // `local_agent` `<result>` / `<usage>`: the terminating run's
                // final text and usage rollup, reported through
                // `set_agent_outcome` before its terminal status. `None` stays
                // the byte-faithful "no result" case (a run that produced no
                // text, or a non-agent task). `local_fusion` reports its own
                // usage summary through `set_fusion_egress_and_usage`.
                usage: agent_outcome
                    .usage
                    .or_else(|| fusion_state.as_ref().and_then(|f| f.usage.clone())),
                // `killed_by` (the by-Claude/by-user split, from
                // `kill_with_reason`) + the isolation `<worktree>` section (the
                // KEPT worktree's path/branch, from the handler's terminal
                // `agent_worktree_result` judgment). `None` ⇒ the bare
                // `was stopped` verb / no worktree section.
                killed_by: agent_outcome.killed_by,
                worktree_path: agent_outcome.worktree_path,
                worktree_branch: agent_outcome.worktree_branch,
                result: workflow_outcome
                    .as_ref()
                    .and_then(|outcome| outcome.result.clone())
                    .or(agent_outcome.result)
                    .or(fusion_final_text),
                workflow_failures: workflow_outcome
                    .as_ref()
                    .map(|outcome| outcome.failures.clone())
                    .unwrap_or_default(),
                workflow_agent_count: workflow_outcome.as_ref().map(|outcome| outcome.agent_count),
                workflow_total_tokens: workflow_outcome
                    .as_ref()
                    .map(|outcome| outcome.total_tokens),
                workflow_total_tool_calls: workflow_outcome
                    .as_ref()
                    .map(|outcome| outcome.total_tool_calls),
                workflow_duration_ms: workflow_outcome.as_ref().map(|outcome| outcome.duration_ms),
                workflow_script_path: workflow_metadata
                    .as_ref()
                    .and_then(|metadata| metadata.0.clone()),
                workflow_run_id: workflow_metadata
                    .as_ref()
                    .and_then(|metadata| metadata.1.clone()),
                workflow_args: workflow_metadata
                    .as_ref()
                    .and_then(|metadata| metadata.2.clone()),
                workflow_transcript_dir: workflow_metadata
                    .as_ref()
                    .and_then(|metadata| metadata.3.clone()),
                workflow_agents_done: workflow_counts_available
                    .then(|| workflow_outcome.as_ref().map(|outcome| outcome.agents_done))
                    .flatten(),
                workflow_agents_error: workflow_counts_available
                    .then(|| {
                        workflow_outcome
                            .as_ref()
                            .map(|outcome| outcome.agents_error)
                    })
                    .flatten(),
                workflow_agents_skipped: workflow_counts_available
                    .then(|| {
                        workflow_outcome
                            .as_ref()
                            .map(|outcome| outcome.agents_skipped)
                    })
                    .flatten(),
                workflow_agents_empty_result: workflow_counts_available
                    .then(|| {
                        workflow_outcome
                            .as_ref()
                            .map(|outcome| outcome.agents_empty_result)
                    })
                    .flatten(),
                egress_profiles: fusion_state
                    .as_ref()
                    .map(|f| f.egress_profiles.clone())
                    .unwrap_or_default(),
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
            armed.drain().collect::<Vec<_>>()
        };
        let mut deferred_rest = Vec::new();
        for (id, payload) in rest_payloads {
            let Some(state) = map.get(&id) else { continue };
            let b = state.base();
            if b.status.is_terminal() {
                continue;
            }
            if Self::has_live_background_children_locked(
                &map,
                payload.agent_id,
                payload.agent_name.as_deref(),
                payload.team_name.as_deref(),
            ) {
                deferred_rest.push((id, payload));
                continue;
            }
            out.push(platform_api::task_registry::TaskNotification {
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
                workflow_failures: Vec::new(),
                workflow_agent_count: None,
                workflow_total_tokens: None,
                workflow_total_tool_calls: None,
                workflow_duration_ms: None,
                workflow_script_path: None,
                workflow_run_id: None,
                workflow_args: None,
                workflow_transcript_dir: None,
                workflow_agents_done: None,
                workflow_agents_error: None,
                workflow_agents_skipped: None,
                workflow_agents_empty_result: None,
                // A rest notification only ever fires for a `local_agent`,
                // which has no egress-profile concept.
                egress_profiles: Vec::new(),
            });
        }
        drop(map);
        self.requeue_deferred_rest(deferred_rest).await;
        out
    }

    async fn requeue_deferred_rest(&self, deferred_rest: Vec<(String, RestPayload)>) {
        if deferred_rest.is_empty() {
            return;
        }
        let mut armed = self.pending_rest.write().await;
        for (id, payload) in deferred_rest {
            armed.entry(id).or_insert(payload);
        }
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
        // Handler-spawned path: dispatch teardown to the owning handler.
        // Read the routing entry without consuming it. A failed handler kill is
        // retryable; removing the route/cleanup before the awaited call would
        // strand a still-running task with no way to stop it.
        let spawned_type = self.spawned.read().await.get(task_id_ref).copied();
        if let Some(task_type) = spawned_type {
            let handler = self
                .handlers
                .get(&task_type)
                .ok_or(TaskError::UnknownType)?;
            let ctx = TaskContext {
                fs: self.fs.clone(),
                runtime: self.runtime.clone(),
            };
            handler.kill(task_id_ref, ctx).await?;
            // Commit the routing teardown only after the owner confirms the
            // process is stopped. A concurrent terminal transition is harmless:
            // removals are idempotent and the status guard below preserves it.
            self.spawned.write().await.remove(task_id_ref);
            let cleanup = self.cleanups.lock().await.remove(task_id_ref);
            // Reflect the kill in the tracked state for any variant the M1
            // surface can write; the handler's status sink drives the rest.
            // Reverse-race guard: a task that already reached a terminal status
            // (e.g. an MCP settle that won the race and fired `TaskCompleted`)
            // must NOT be demoted to `Killed` — that would leave the task
            // `Killed` after `TaskCompleted` already fired. Skip the status
            // flip once terminal (mirrors `settle_mcp_task`'s re-check).
            let killed_bash_output = self.mark_killed(task_id_ref).await;
            self.append_killed_trailer(killed_bash_output).await;
            if let Some(cleanup) = cleanup {
                cleanup();
            }
            return Ok(());
        }

        let cleanup = self.cleanups.lock().await.remove(task_id_ref);
        let mut handles = self.handles.lock().await;
        if let Some(h) = handles.remove(task_id_ref) {
            self.runtime
                .cancel(&h)
                .await
                .map_err(|e| TaskError::Internal(e.to_string()))?;
        }
        let killed_bash_output = self.mark_killed(task_id_ref).await;
        self.append_killed_trailer(killed_bash_output).await;
        if let Some(cleanup) = cleanup {
            cleanup();
        }
        Ok(())
    }

    /// Flip a task to `Killed`, but only when THIS call performs the
    /// non-terminal transition. Returns the output file of a `local_bash` task
    /// that was actually killed here, so the caller can close its file.
    ///
    /// Reverse-race guard: a task that already reached a terminal status (e.g.
    /// an MCP settle that won the race and fired `TaskCompleted`) must NOT be
    /// demoted to `Killed`.
    async fn mark_killed(&self, task_id: &str) -> Option<std::path::PathBuf> {
        let mut map = self.tasks.write().await;
        let Some(state) = map.get_mut(task_id) else {
            return None;
        };
        if state.base().status.is_terminal() {
            return None;
        }
        let mut killed_bash_output = None;
        match state {
            TaskState::LocalBash(bash) => {
                bash.base.status = TaskStatus::Killed;
                killed_bash_output = Some(bash.base.output_file.clone());
            }
            TaskState::LocalAgent(agent) => agent.base.status = TaskStatus::Killed,
            TaskState::Monitor(monitor) => monitor.base.status = TaskStatus::Killed,
            // A backgrounded MCP call: mark killed + `mcpStatus:"cancelled"`
            // (the poll loop's `status==="killed"` → `cancelTask` branch).
            TaskState::McpTask(mcp) => {
                mcp.base.status = TaskStatus::Killed;
                mcp.mcp_status = "cancelled".to_string();
            }
            _ => {}
        }
        killed_bash_output
    }

    /// Close a killed shell's output file with the trailer claude-code appends
    /// (`JF`: `s1e(e, "\n[killed]\n")`, 2.1.263 `src_160988549.js` @2039181),
    /// so a later `Read` of that file shows how the command ended.
    async fn append_killed_trailer(&self, output_file: Option<std::path::PathBuf>) {
        if let Some(output_file) = output_file {
            let _ = self
                .output_manager
                .append(&output_file, "\n[killed]\n")
                .await;
        }
    }
}

/// `TeamSpawnSeam` impl — the typed spawn/kill seam the coordinator's
/// `TeamCreate` / `TeamDelete` tools use to start and stop a real
/// `InProcessTeammate` task WITHOUT a `coordinator` → `lingxi-tasks` dependency
/// cycle (the abstract trait lives in `traits`; this concrete impl lives here).
///
/// Unlike [`TaskRegistryHandle::create`](platform_api::task_registry::TaskRegistryHandle::create)
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
        // Addressing parity with the other task surfaces: an agent is
        // registered under its task id AND its aliases (the agent id, its name,
        // and `name@team`), and `get` / `kill` / `output` all resolve those
        // aliases first. This one did not, so an id the model was handed — the
        // `<task-id>` of a completion notification, or a name from `TaskList` —
        // resolved for `TaskStop` and `TaskOutput` but came back `Terminated`
        // here, which reads to the model as "that agent is gone".
        let task_id = &self.canonical_or_raw(task_id).await;
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
    // Stamp the originating `tool_use_id` onto the task so a background task's
    // `<task-notification>` carries the `<tool-use-id>` line (claude-code parity).
    match input {
        TaskSpawnInput::LocalAgent {
            tool_use_id,
            creator_teammate_name,
            creator_team_name,
            creator_agent_id,
            ..
        } => {
            if base.tool_use_id.is_none() {
                base.tool_use_id.clone_from(tool_use_id);
            }
            if base.creator_teammate_name.is_none() {
                base.creator_teammate_name.clone_from(creator_teammate_name);
            }
            if base.creator_team_name.is_none() {
                base.creator_team_name.clone_from(creator_team_name);
            }
            if base.creator_agent_id.is_none() {
                base.creator_agent_id = *creator_agent_id;
            }
        }
        TaskSpawnInput::LocalWorkflow {
            tool_use_id,
            creator_teammate_name,
            creator_team_name,
            creator_agent_id,
            ..
        }
        | TaskSpawnInput::Monitor {
            tool_use_id,
            creator_teammate_name,
            creator_team_name,
            creator_agent_id,
            ..
        }
        | TaskSpawnInput::McpTask {
            tool_use_id,
            creator_teammate_name,
            creator_team_name,
            creator_agent_id,
            ..
        } => {
            if base.tool_use_id.is_none() {
                base.tool_use_id.clone_from(tool_use_id);
            }
            if base.creator_teammate_name.is_none() {
                base.creator_teammate_name.clone_from(creator_teammate_name);
            }
            if base.creator_team_name.is_none() {
                base.creator_team_name.clone_from(creator_team_name);
            }
            if base.creator_agent_id.is_none() {
                base.creator_agent_id = *creator_agent_id;
            }
        }
        _ => {}
    }
    match input {
        TaskSpawnInput::LocalBash { command, .. } => {
            TaskState::LocalBash(crate::state::LocalBashTaskState {
                base,
                command: command.clone(),
                pid: None,
                exit_code: None,
                cwd: None,
                is_backgrounded: None,
            })
        }
        TaskSpawnInput::LocalAgent {
            agent_id,
            subagent_type,
            prompt,
            is_backgrounded,
            spawn_request,
            ..
        } => TaskState::LocalAgent(crate::state::LocalAgentTaskState {
            // The fork identity rides the full spawn request, not the compact
            // task-index fields — a forked skill is dispatched through the same
            // `SubagentSpawnRequest` as any background agent.
            forked_skill_name: spawn_request
                .as_ref()
                .and_then(|r| r.forked_skill_name.clone()),
            base,
            agent_id: *agent_id,
            subagent_type: subagent_type.clone(),
            prompt: prompt.clone(),
            error: None,
            messages: vec![],
            pending_messages: vec![],
            is_backgrounded: *is_backgrounded,
            outcome: Default::default(),
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
            session_uuid,
            workflow_id,
            script,
            resume_from_run_id,
            args,
            run_id,
            parent_model: _,
            parent_model_profile: _,
            invocation_mode: _,
            workflow_source: _,
            script_is_verbatim_builtin: _,
            transcript_subdir,
            launched_from_subagent: _,
            tool_use_id: _,
            creator_teammate_name: _,
            creator_team_name: _,
            creator_agent_id: _,
            scope,
        } => TaskState::LocalWorkflow(crate::state::LocalWorkflowTaskState {
            base,
            session_uuid: session_uuid.clone(),
            workflow_id: workflow_id.clone(),
            script: script.clone(),
            resume_from_run_id: resume_from_run_id.clone(),
            args: args.clone(),
            // Effective run id: the launcher-minted id for a fresh run, else the
            // resumed id (so the resume gate can find a still-running workflow).
            run_id: run_id.clone().or_else(|| resume_from_run_id.clone()),
            script_path: None,
            transcript_dir: transcript_subdir.clone(),
            current_step: 0,
            outcome: Default::default(),
            // The Host's minted authority, carried through verbatim. There is
            // deliberately no derivation here from `workflow_id` or `args`:
            // both are caller-supplied, and reading either would re-open the
            // forgery vector `find_nonterminal_local_app_workflows`' doc
            // comment describes. See `LocalWorkflowTaskState::scope`.
            scope: scope.clone(),
        }),
        TaskSpawnInput::MonitorMcp { server_name, watch } => {
            TaskState::MonitorMcp(crate::state::MonitorMcpTaskState {
                base,
                server_name: server_name.clone(),
                watch_resources: watch.clone(),
            })
        }
        TaskSpawnInput::Monitor {
            command,
            timeout: _,
            cwd: _,
            tool_use_id,
            creator_teammate_name: _,
            creator_team_name: _,
            creator_agent_id: _,
        } => {
            if base.tool_use_id.is_none() {
                base.tool_use_id = tool_use_id.clone();
            }
            TaskState::Monitor(crate::state::MonitorTaskState {
                base,
                command: command.clone(),
                exit_code: None,
            })
        }
        TaskSpawnInput::McpTask {
            server_name,
            tool_name,
            tool_use_id,
            creator_teammate_name: _,
            creator_team_name: _,
            creator_agent_id: _,
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
        TaskSpawnInput::LocalFusion {
            request,
            conversation_id,
        } => TaskState::LocalFusion(crate::state::LocalFusionTaskState {
            base,
            conversation_id: conversation_id.clone(),
            prompt: request.prompt.clone(),
            run_id: None,
            preset: match request.preset {
                platform_api::FusionPreset::Quality => "quality".to_string(),
                platform_api::FusionPreset::Fast => "fast".to_string(),
            },
            cross_provider: request.cross_provider,
            final_text: None,
            error: None,
            egress_profiles: Vec::new(),
            usage: None,
            stage: None,
            effective_timeout_ms: None,
            result_published: false,
        }),
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
/// [`TaskType::LocalBash`], [`TaskType::Monitor`], and
/// [`TaskType::MonitorMcp`].
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
            LocalBashHandler::new(process.clone(), sandbox.clone(), output_manager.clone())
                .with_status_sink(bash_status_sink.clone()),
        ),
    );
    reg.register_handler(
        TaskType::Monitor,
        Arc::new(
            MonitorHandler::new(process, sandbox, output_manager.clone())
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
/// registry's own (`reg.output_manager`). `status_sink` is the deferred
/// registry adapter used both for lifecycle writeback and for the LocalAgent
/// activation barrier. The subagent type arrives already resolved on the
/// [`crate::task_trait::TaskSpawnInput::LocalAgent`] variant, so no resolver
/// injection is needed here.
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
    status_sink: Arc<dyn crate::handlers::TaskStatusSink>,
) {
    let output_manager = reg.output_manager.clone();
    reg.register_handler(
        TaskType::LocalAgent,
        Arc::new(
            LocalAgentHandler::new(
                spawner,
                tool_invoker.clone(),
                budget,
                output_manager.clone(),
            )
            .with_status_sink(status_sink),
        ),
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
    status_sink: Arc<dyn crate::handlers::TaskStatusSink>,
) {
    let output_manager = reg.output_manager.clone();
    reg.register_handler(
        TaskType::Dream,
        Arc::new(
            DreamHandler::new(spawner, tool_invoker, budget, output_manager)
                .with_status_sink(status_sink),
        ),
    );
}

/// Register [`TaskType::LocalFusion`]. Call before wrapping the registry in an
/// [`Arc`].
pub fn register_fusion_handler(
    reg: &mut TaskRegistry,
    executor: Arc<dyn FusionExecutor>,
    sink: Arc<dyn FusionCompletionSink>,
    tool_invoker: Arc<dyn ToolInvoker>,
    budget: Arc<dyn BudgetEnforcerHandle>,
    status_sink: Arc<dyn crate::handlers::TaskStatusSink>,
) {
    let output_manager = reg.output_manager.clone();
    reg.register_handler(
        TaskType::LocalFusion,
        Arc::new(
            LocalFusionHandler::new(executor, sink, tool_invoker, budget, output_manager)
                .with_status_sink(status_sink),
        ),
    );
}

/// Registry-API-level tests for the restart-adoption scope seam (P-1.12).
/// Kept as its own inline module rather than added to `registry_test.rs` so
/// this task's edits stay confined to files it owns; `registry_test.rs`
/// already exercises the unscoped `register_adopted_workflow` path
/// (`adopted_workflow_is_registered_as_paused_and_keeps_resume_metadata`,
/// `register_adopted_workflow_does_not_replace_existing_live_task_with_same_task_id`)
/// and is left untouched.
///
/// These two tests pin the registry API/plumbing in isolation (a caller
/// handing `register_adopted_workflow_with_scope` an already-built scope, or
/// going through the plain unscoped entry point). The two tests named
/// exactly `an_adopted_in_flight_build_still_blocks_its_apps_delete` and
/// `an_adopted_forged_workflow_still_gets_no_scope` live in
/// `engine-mobile`'s `workflow_support::run_id_tests` instead, where they
/// exercise the real `resolve_adopted_local_app_build_scope` re-derivation
/// end to end against real checkpoint/manifest fixtures -- that is where the
/// residual actually lived, so that is where the requested test names carry
/// the load-bearing coverage.
#[cfg(test)]
mod adopted_workflow_scope_test {
    use super::*;
    use platform_api::filesystem::{FileContent, FileEvent, FileSystem, FlockGuard, FsError};

    // ---- Minimal in-memory FileSystem, just enough for `output_manager` ----
    struct InMemoryFs {
        files: tokio::sync::Mutex<HashMap<String, String>>,
        fail_writes: std::sync::atomic::AtomicBool,
        write_count: std::sync::atomic::AtomicUsize,
        fail_write_at: std::sync::atomic::AtomicUsize,
    }
    impl InMemoryFs {
        fn new() -> Self {
            Self {
                files: tokio::sync::Mutex::new(HashMap::new()),
                fail_writes: std::sync::atomic::AtomicBool::new(false),
                write_count: std::sync::atomic::AtomicUsize::new(0),
                fail_write_at: std::sync::atomic::AtomicUsize::new(0),
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
            let write_number = self
                .write_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                + 1;
            if self.fail_writes.load(std::sync::atomic::Ordering::SeqCst)
                || self.fail_write_at.load(std::sync::atomic::Ordering::SeqCst) == write_number
            {
                return Err(FsError::Io("injected write failure".into()));
            }
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

    fn make_registry() -> (tempfile::TempDir, Arc<InMemoryFs>, TaskRegistry) {
        let dir = tempfile::tempdir().unwrap();
        let in_memory_fs = Arc::new(InMemoryFs::new());
        let fs: Arc<dyn FileSystem> = in_memory_fs.clone();
        let runtime = Arc::new(test_harness::mocks::MockRuntimeSpawner::default());
        let out_mgr = Arc::new(crate::output_manager::TaskOutputManager::new(
            std::path::PathBuf::from(dir.path()),
            fs.clone(),
        ));
        (dir, in_memory_fs, TaskRegistry::new(runtime, fs, out_mgr))
    }

    fn adopted(task_id: &str, run_id: &str, args_app_id: &str) -> AdoptedWorkflow {
        AdoptedWorkflow {
            task_id: task_id.to_string(),
            session_uuid: Some("session-1".to_string()),
            // A real build workflow's name -- exactly what a forged
            // checkpoint would also carry (see `AdoptedWorkflow`'s doc
            // comment). Both tests below prove the guard no longer cares.
            workflow_id: "lingxi-local-app:local-app-build".to_string(),
            run_id: run_id.to_string(),
            script_path: "/workspace/.lingxi/workflows/build.js".to_string(),
            args: Some(format!(r#"{{"app_id":"{args_app_id}"}}"#)),
            transcript_dir: format!("/sessions/s1/subagents/workflows/{run_id}"),
            description: "Build local app".to_string(),
            start_time: SystemTime::now(),
        }
    }

    /// (1) Registry-level half of the P-1.12 fix: once a caller HAS
    /// re-derived a scope for an in-flight build (the job
    /// `engine-mobile`'s `adopt_session` does end to end --
    /// see `workflow_support::run_id_tests::an_adopted_in_flight_build_still_blocks_its_apps_delete`,
    /// which is the test carrying this exact name against the real
    /// resolution logic), `register_adopted_workflow_with_scope` stamps it on
    /// the adopted (`Paused`, non-terminal) row and the delete guard honors
    /// it -- independent of how that scope was derived.
    #[tokio::test]
    async fn an_adopted_in_flight_build_still_blocks_its_apps_delete_at_the_registry_api() {
        let (_dir, _fs, registry) = make_registry();
        let scope = crate::scope::LocalAppWorkflowTaskScope::for_build("resumed-app")
            .expect("well-formed app id");

        registry
            .register_adopted_workflow_with_scope(
                adopted("wadopted1", "wf_adopted1", "resumed-app"),
                Some(scope),
            )
            .await
            .expect("adopt with re-derived scope");

        let state = registry.get("wadopted1").await.expect("adopted state");
        assert!(
            !state.base().status.is_terminal(),
            "an adopted row must be non-terminal (Paused)"
        );
        assert_eq!(
            registry
                .find_nonterminal_local_app_workflows("resumed-app")
                .await,
            vec!["wadopted1".to_string()],
            "a build re-validated on adoption must still block its app's delete"
        );
    }

    /// (2) Registry-level half: a checkpoint adopted through the plain
    /// (unscoped) `register_adopted_workflow` entry point -- what any caller
    /// gets by default, and exactly what a forged checkpoint (`workflow_id`
    /// naming a real build workflow, `args.app_id` naming a victim app) must
    /// still resolve to once re-validated -- must never block another app's
    /// delete. See
    /// `workflow_support::run_id_tests::an_adopted_forged_workflow_still_gets_no_scope`
    /// for the test carrying this exact name that exercises the real
    /// re-validation logic end to end.
    #[tokio::test]
    async fn an_adopted_forged_workflow_still_gets_no_scope_at_the_registry_api() {
        let (_dir, _fs, registry) = make_registry();

        registry
            .register_adopted_workflow(adopted("wforged12", "wf_forged1", "victim-app"))
            .await
            .expect("adopt without scope");

        let state = registry.get("wforged12").await.expect("adopted state");
        assert!(
            !state.base().status.is_terminal(),
            "an adopted row must be non-terminal (Paused)"
        );
        assert!(
            registry
                .find_nonterminal_local_app_workflows("victim-app")
                .await
                .is_empty(),
            "a forged workflow_id/args.app_id pair must never block another \
            app's delete just because it was adopted"
        );
    }

    async fn running_scoped_workflow(
        registry: &TaskRegistry,
        task_id: &str,
        purpose: crate::scope::LocalAppWorkflowPurpose,
    ) {
        let scope = match purpose {
            crate::scope::LocalAppWorkflowPurpose::Build => {
                crate::scope::LocalAppWorkflowTaskScope::for_build("qa-app")
            }
            crate::scope::LocalAppWorkflowPurpose::UseTest => {
                crate::scope::LocalAppWorkflowTaskScope::for_use_test("qa-app")
            }
            crate::scope::LocalAppWorkflowPurpose::McpAuthoring => {
                crate::scope::LocalAppWorkflowTaskScope::for_mcp_authoring("qa-app")
            }
        }
        .expect("well-formed app id");
        registry
            .register_adopted_workflow_with_scope(
                adopted(task_id, "wf_terminal1", "qa-app"),
                Some(scope),
            )
            .await
            .expect("register workflow");
        registry
            .set_status(task_id, TaskStatus::Running)
            .await
            .expect("activate workflow");
    }

    fn checked_outcome() -> platform_api::task_registry::WorkflowTerminalOutcome {
        platform_api::task_registry::WorkflowTerminalOutcome {
            result: Some(r#"{"ok":true,"host_checked":true}"#.into()),
            agent_count: 3,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn local_app_terminal_commit_publishes_spool_metadata_and_state_together() {
        let (_dir, _fs, registry) = make_registry();
        running_scoped_workflow(
            &registry,
            "wcommit01",
            crate::scope::LocalAppWorkflowPurpose::UseTest,
        )
        .await;
        let published = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = published.clone();

        let (actual, committed) = registry
            .commit_local_app_workflow_terminal(
                "wcommit01",
                checked_outcome(),
                move || async move {
                    flag.store(true, std::sync::atomic::Ordering::SeqCst);
                    Ok(())
                },
            )
            .await
            .expect("commit success");

        assert_eq!(actual.base().status, TaskStatus::Completed);
        assert!(committed);
        assert!(published.load(std::sync::atomic::Ordering::SeqCst));
        let output = registry
            .output_manager
            .read(
                &registry.output_manager.path_for("wcommit01").unwrap(),
                crate::output_manager::OutputOptions::default(),
            )
            .await
            .expect("canonical spool");
        assert_eq!(output.content, r#"{"ok":true,"host_checked":true}"#);
    }

    #[tokio::test]
    async fn published_local_app_stays_completed_when_final_spool_write_fails() {
        let (_dir, fs, registry) = make_registry();
        running_scoped_workflow(
            &registry,
            "wcommit08",
            crate::scope::LocalAppWorkflowPurpose::UseTest,
        )
        .await;
        registry
            .tasks
            .write()
            .await
            .get_mut("wcommit08")
            .expect("registered workflow")
            .base_mut()
            .notified = false;

        // The pending placeholder succeeds, Host publication succeeds, and
        // only the subsequent canonical spool write fails.
        let placeholder_write = fs.write_count.load(std::sync::atomic::Ordering::SeqCst) + 1;
        fs.fail_write_at
            .store(placeholder_write + 1, std::sync::atomic::Ordering::SeqCst);
        let published = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = published.clone();

        let (actual, committed) = registry
            .commit_local_app_workflow_terminal(
                "wcommit08",
                checked_outcome(),
                move || async move {
                    flag.store(true, std::sync::atomic::Ordering::SeqCst);
                    Ok(())
                },
            )
            .await
            .expect("Host publication is the commit point");

        assert!(published.load(std::sync::atomic::Ordering::SeqCst));
        assert!(committed);
        assert_eq!(actual.base().status, TaskStatus::Completed);
        let TaskState::LocalWorkflow(workflow) = actual else {
            panic!("expected workflow")
        };
        assert_eq!(workflow.outcome, checked_outcome());

        let handle: &dyn platform_api::task_registry::TaskRegistryHandle = &registry;
        let chunk = handle
            .output("wcommit08", None)
            .await
            .expect("public TaskOutput projection");
        assert_eq!(chunk.status.as_deref(), Some("completed"));
        assert!(chunk.done);
        assert_eq!(chunk.content, r#"{"ok":true,"host_checked":true}"#);
        assert_eq!(
            chunk.output_path, None,
            "the stale pending spool is not an authoritative output path"
        );

        let notifications = registry.take_pending_task_notifications().await;
        let notification = notifications
            .iter()
            .find(|notification| notification.task_id == "wcommit08")
            .expect("completed terminal notification");
        assert_eq!(notification.status, "completed");
        assert_eq!(
            notification.result.as_deref(),
            Some(r#"{"ok":true,"host_checked":true}"#)
        );
        assert!(notification.error.is_none());
        assert_eq!(
            notification.output_path, None,
            "the notification must not advertise stale physical bytes"
        );

        let physical_path = registry.output_manager.path_for("wcommit08").unwrap();
        let physical = fs
            .files
            .lock()
            .await
            .get(physical_path.to_str().unwrap())
            .cloned()
            .expect("pending spool remains on disk");
        let physical: serde_json::Value = serde_json::from_str(&physical).unwrap();
        assert_eq!(physical["verified"], false);
        assert!(physical["error"]
            .as_str()
            .is_some_and(|error| error.contains("publication pending")));
    }

    #[tokio::test]
    async fn killed_local_app_never_runs_the_prepared_publication_commit() {
        let (_dir, _fs, registry) = make_registry();
        running_scoped_workflow(
            &registry,
            "wcommit02",
            crate::scope::LocalAppWorkflowPurpose::Build,
        )
        .await;
        registry
            .set_status("wcommit02", TaskStatus::Killed)
            .await
            .expect("kill wins");
        let published = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = published.clone();

        let (actual, committed) = registry
            .commit_local_app_workflow_terminal(
                "wcommit02",
                checked_outcome(),
                move || async move {
                    flag.store(true, std::sync::atomic::Ordering::SeqCst);
                    Ok(())
                },
            )
            .await
            .expect("return authoritative terminal row");

        assert_eq!(actual.base().status, TaskStatus::Killed);
        assert!(!committed);
        assert!(!published.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn prior_completed_local_app_does_not_claim_a_skipped_publication_commit() {
        let (_dir, _fs, registry) = make_registry();
        running_scoped_workflow(
            &registry,
            "wcommit06",
            crate::scope::LocalAppWorkflowPurpose::Build,
        )
        .await;
        registry
            .finish_workflow_terminal("wcommit06", checked_outcome(), TaskStatus::Completed)
            .await
            .expect("competing completion wins");
        let published = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = published.clone();

        let (actual, committed) = registry
            .commit_local_app_workflow_terminal(
                "wcommit06",
                checked_outcome(),
                move || async move {
                    flag.store(true, std::sync::atomic::Ordering::SeqCst);
                    Ok(())
                },
            )
            .await
            .expect("return authoritative completed row");

        assert_eq!(actual.base().status, TaskStatus::Completed);
        assert!(!committed);
        assert!(!published.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn local_app_publication_failure_is_one_canonical_failed_terminal() {
        let (_dir, _fs, registry) = make_registry();
        running_scoped_workflow(
            &registry,
            "wcommit03",
            crate::scope::LocalAppWorkflowPurpose::Build,
        )
        .await;

        let (actual, committed) = registry
            .commit_local_app_workflow_terminal("wcommit03", checked_outcome(), || async {
                Err("injected pointer commit failure".into())
            })
            .await
            .expect("commit returns failed row");

        assert_eq!(actual.base().status, TaskStatus::Failed);
        assert!(!committed);
        let TaskState::LocalWorkflow(workflow) = actual else {
            panic!("expected workflow")
        };
        assert!(workflow.outcome.result.is_none());
        assert!(workflow
            .outcome
            .error
            .as_deref()
            .is_some_and(|error| error.contains("pointer commit failure")));
        let output = registry
            .output_manager
            .read(
                &registry.output_manager.path_for("wcommit03").unwrap(),
                crate::output_manager::OutputOptions::default(),
            )
            .await
            .expect("failure spool");
        let payload: serde_json::Value = serde_json::from_str(&output.content).unwrap();
        assert_eq!(payload["ok"], false);
        assert_eq!(payload["verified"], false);
        assert!(payload["error"]
            .as_str()
            .is_some_and(|error| error.contains("pointer commit failure")));
    }

    #[tokio::test]
    async fn local_app_publication_failure_fails_closed_when_failure_spool_rewrite_fails() {
        let (_dir, fs, registry) = make_registry();
        running_scoped_workflow(
            &registry,
            "wcommit07",
            crate::scope::LocalAppWorkflowPurpose::Build,
        )
        .await;
        registry
            .tasks
            .write()
            .await
            .get_mut("wcommit07")
            .expect("registered workflow")
            .base_mut()
            .notified = false;

        // The unverified publication placeholder is the next write. The
        // following failure-payload rewrite fails, leaving the placeholder on
        // disk while the drained notification suppresses that stale path.
        let placeholder_write = fs.write_count.load(std::sync::atomic::Ordering::SeqCst) + 1;
        fs.fail_write_at
            .store(placeholder_write + 1, std::sync::atomic::Ordering::SeqCst);

        let (actual, committed) = registry
            .commit_local_app_workflow_terminal("wcommit07", checked_outcome(), || async {
                Err("injected pointer commit failure".into())
            })
            .await
            .expect("commit returns failed row");

        assert_eq!(actual.base().status, TaskStatus::Failed);
        assert!(!committed);
        let TaskState::LocalWorkflow(workflow) = actual else {
            panic!("expected workflow")
        };
        assert!(workflow.outcome.result.is_none());
        assert!(workflow
            .outcome
            .error
            .as_deref()
            .is_some_and(|error| error.contains("pointer commit failure")));

        let notifications = registry.take_pending_task_notifications().await;
        let notification = notifications
            .iter()
            .find(|notification| notification.task_id == "wcommit07")
            .expect("failed terminal notification");
        assert_eq!(notification.status, "failed");
        assert!(notification
            .error
            .as_deref()
            .is_some_and(|error| error.contains("pointer commit failure")));
        assert!(notification.result.is_none());
        assert_eq!(
            notification.output_path, None,
            "the failed rewrite leaves no authoritative physical spool"
        );
        let physical_path = registry.output_manager.path_for("wcommit07").unwrap();
        let physical = fs
            .files
            .lock()
            .await
            .get(physical_path.to_str().unwrap())
            .cloned()
            .expect("pending physical spool must still exist");
        let physical_payload: serde_json::Value = serde_json::from_str(&physical).unwrap();
        assert_eq!(physical_payload["ok"], false);
        assert_eq!(physical_payload["verified"], false);
        assert!(physical_payload["error"]
            .as_str()
            .is_some_and(|error| error.contains("publication")));

        let output = registry
            .output_manager
            .read(
                &registry.output_manager.path_for("wcommit07").unwrap(),
                crate::output_manager::OutputOptions::default(),
            )
            .await
            .expect("fail-closed failure spool");
        let payload: serde_json::Value = serde_json::from_str(&output.content).unwrap();
        assert_eq!(payload["ok"], false);
        assert_eq!(payload["verified"], false);
        assert!(payload["error"]
            .as_str()
            .is_some_and(|error| error.contains("pointer commit failure")));

        let path = registry.output_manager.path_for("wcommit07").unwrap();
        registry
            .output_manager
            .append(&path, "late raw handler output")
            .await
            .expect("late terminal appends are absorbed");
        let offset = registry
            .output_manager
            .read(
                &path,
                crate::output_manager::OutputOptions {
                    offset: Some(1),
                    limit: None,
                },
            )
            .await
            .expect("post-failure offset read");
        assert!(offset.content.is_empty());
        assert_eq!(offset.total_lines, 1);

        // Exercise the public TaskOutput projection as well as the backing
        // manager: the failed registry row and the fail-closed payload must
        // agree even when the rewrite itself was rejected.
        let handle: &dyn platform_api::task_registry::TaskRegistryHandle = &registry;
        let chunk = handle
            .output("wcommit07", None)
            .await
            .expect("TaskOutput projection");
        assert_eq!(chunk.status.as_deref(), Some("failed"));
        assert!(chunk.done);
        assert_eq!(chunk.output_path, None);
        let chunk_payload: serde_json::Value = serde_json::from_str(&chunk.content).unwrap();
        assert_eq!(chunk_payload["ok"], false);
        assert_eq!(chunk_payload["verified"], false);
    }

    #[tokio::test]
    async fn local_app_spool_failure_never_publishes_or_completes() {
        let (_dir, fs, registry) = make_registry();
        running_scoped_workflow(
            &registry,
            "wcommit04",
            crate::scope::LocalAppWorkflowPurpose::UseTest,
        )
        .await;
        fs.fail_writes
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let published = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = published.clone();

        let (actual, committed) = registry
            .commit_local_app_workflow_terminal(
                "wcommit04",
                checked_outcome(),
                move || async move {
                    flag.store(true, std::sync::atomic::Ordering::SeqCst);
                    Ok(())
                },
            )
            .await
            .expect("spool failure becomes task failure");

        assert_eq!(actual.base().status, TaskStatus::Failed);
        assert!(!committed);
        assert!(!published.load(std::sync::atomic::Ordering::SeqCst));
        let TaskState::LocalWorkflow(workflow) = actual else {
            panic!("expected workflow")
        };
        assert!(workflow
            .outcome
            .error
            .as_deref()
            .is_some_and(|error| error.contains("terminal spool replacement failed")));
        let output = registry
            .output_manager
            .read(
                &registry.output_manager.path_for("wcommit04").unwrap(),
                crate::output_manager::OutputOptions::default(),
            )
            .await
            .expect("fail-closed initial spool failure");
        let payload: serde_json::Value = serde_json::from_str(&output.content).unwrap();
        assert_eq!(payload["ok"], false);
        assert_eq!(payload["verified"], false);
    }

    #[tokio::test]
    async fn mcp_authoring_scope_cannot_use_the_qa_terminal_commit() {
        let (_dir, _fs, registry) = make_registry();
        running_scoped_workflow(
            &registry,
            "wcommit05",
            crate::scope::LocalAppWorkflowPurpose::McpAuthoring,
        )
        .await;
        let published = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = published.clone();

        let error = registry
            .commit_local_app_workflow_terminal(
                "wcommit05",
                checked_outcome(),
                move || async move {
                    flag.store(true, std::sync::atomic::Ordering::SeqCst);
                    Ok(())
                },
            )
            .await
            .expect_err("only Build/UseTest may commit QA publication");

        assert!(matches!(error, TaskError::Unsupported));
        assert!(!published.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(
            registry.get("wcommit05").await.unwrap().base().status,
            TaskStatus::Running
        );
    }
}

#[cfg(test)]
#[path = "registry_test.rs"]
mod registry_test;

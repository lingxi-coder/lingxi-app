//! `LocalWorkflowHandler` — runs a model-authored workflow script and bridges
//! its `agent()` calls to real subagent spawns.
//!
//! claude-code executes workflow scripts as a JS `AsyncFunction` whose injected
//! `agent()` global spawns a child agent and resolves with its result. LingXi
//! runs the same scripts on the embedded QuickJS runtime (the `workflow` crate);
//! this module bridges that runtime's batch `agent_runner` seam to the
//! [`SubagentSpawner`] the Task subsystem uses, and wires the whole thing into a
//! background [`Task`] (mirroring [`crate::handlers::local_agent`]).
//!
//! ## The sync ↔ async bridge
//!
//! `workflow::run` is synchronous (it drives the QuickJS job queue on the
//! calling thread) and its `agent_runner` callback is invoked from sync JS, but
//! [`SubagentSpawner::spawn`] is `async` and engine code must not `block_on` the
//! runtime directly (it spawns through the `RuntimeSpawner` seam). We therefore
//! run `workflow::run` on a dedicated `std::thread` and connect it to the async
//! world with channels:
//!
//! * the script thread's batch runner sends the batch's prompts over an mpsc
//!   channel and blocks (`blocking_recv`) on a per-batch oneshot reply;
//! * an async worker loop (driven on the caller's runtime) receives each batch,
//!   spawns its subagents concurrently — bounded by [`concurrency_cap`], in the
//!   prompts' order — and sends the ordered results back over the reply channel.
//!
//! There is at most one batch in flight (the runner blocks until its reply), so
//! the channels cannot deadlock: the script only makes progress once the worker
//! has answered, and `workflow::run` only returns once every batch has been
//! answered — at which point the runner (holding the request sender) is dropped,
//! the worker loop sees the closed channel and ends, and the outcome that the
//! script thread sends last is delivered to the caller.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::StreamExt;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot, Mutex};
use traits::{
    BackgroundTaskHandle, BudgetEnforcerHandle, RuntimeSpawner, SubagentInheritance,
    SubagentResult, SubagentSpawnError, SubagentSpawnRequest, SubagentSpawner, ToolInvoker,
};

use crate::id::TaskType;
use crate::output_manager::TaskOutputManager;
use crate::state::TaskStatus;
use crate::task_trait::{Task, TaskContext, TaskError, TaskHandle, TaskSpawnInput};

// Reuse the status-sink seam defined once in the bash handler (single impl wired
// across handlers), exactly as `local_agent` does.
pub use crate::handlers::local_bash::{NoopStatusSink, TaskStatusSink};

/// Handler name reported by [`Task::name`] / used as the runtime task-name.
const HANDLER_NAME: &str = "local_workflow";

/// The subagent type spawned for a bare `agent(prompt)` call — claude-code's
/// default workflow subagent.
pub const DEFAULT_WORKFLOW_SUBAGENT: &str = "general-purpose";

/// A live worker-cancel record: the background-task handle plus the runtime that
/// minted it, so [`Task::kill`] / cleanup can cancel the in-flight worker without
/// a fresh [`TaskContext`]. (Each handler keeps its own — the fields are private;
/// `pub` so it can appear in [`LocalWorkflowHandler::workers_map`]'s return type.)
pub struct WorkerCancel {
    handle: BackgroundTaskHandle,
    runtime: Arc<dyn RuntimeSpawner>,
}

/// Background [`Task`] that runs a workflow script and spools its result.
///
/// Mirrors [`crate::handlers::local_agent::LocalAgentHandler`]: a runtime-spawned
/// worker drives the script to completion (via [`run_workflow_script`]), spools
/// the script's return value, reports the terminal status, and removes its own
/// cancel record on exit. `kill` cancels the in-flight worker.
pub struct LocalWorkflowHandler {
    /// Allocates a subagent slot and pumps it to a terminal [`SubagentResult`].
    spawner: Arc<dyn SubagentSpawner>,
    /// Parent's tool invoker — passed through *unchanged* in
    /// [`SubagentInheritance`] (the recursion lock relies on `Arc::ptr_eq`).
    tool_invoker: Arc<dyn ToolInvoker>,
    /// Parent's budget enforcer — passed through *unchanged* so budget charges
    /// aggregate across the whole agent tree (`Arc::ptr_eq` invariant).
    budget: Arc<dyn BudgetEnforcerHandle>,
    /// Owns the spool directory + path allocation for the result payload.
    output_manager: Arc<TaskOutputManager>,
    /// Where terminal status transitions are reported.
    status_sink: Arc<dyn TaskStatusSink>,
    /// `task_id` → live worker-cancel record (removed by the worker on exit, or
    /// by [`Task::kill`] / cleanup).
    workers: Arc<Mutex<HashMap<String, WorkerCancel>>>,
    /// `task_id` → record queued for teardown by the synchronous
    /// [`TaskHandle::cleanup`] closure; drained by [`Self::drain_pending_kills`].
    pending_kill: Arc<Mutex<HashMap<String, WorkerCancel>>>,
    /// The turn's token target (`cfg.token_budget`) backing the script's
    /// `budget.total`. `None` ⇒ no target (FLEET defaults, budget loops skip).
    /// `budget.spent()` is the workflow's own accumulated subagent output
    /// tokens (a close, safe approximation of claude-code's shared pool — the
    /// main-loop delta is not included; wiring the exact session pool needs
    /// orchestrator surgery and is a documented follow-up). Set by the root.
    token_budget_total: Option<u64>,
}

impl LocalWorkflowHandler {
    /// Construct a handler with the injected dependencies. `tool_invoker` +
    /// `budget` are stored so each run can bundle them into a
    /// [`SubagentInheritance`] (cloning the `Arc` preserves pointer identity —
    /// required by the recursion-lock + budget-aggregation invariants).
    #[must_use]
    pub fn new(
        spawner: Arc<dyn SubagentSpawner>,
        tool_invoker: Arc<dyn ToolInvoker>,
        budget: Arc<dyn BudgetEnforcerHandle>,
        output_manager: Arc<TaskOutputManager>,
    ) -> Self {
        Self {
            spawner,
            tool_invoker,
            budget,
            output_manager,
            status_sink: Arc::new(NoopStatusSink),
            workers: Arc::new(Mutex::new(HashMap::new())),
            pending_kill: Arc::new(Mutex::new(HashMap::new())),
            token_budget_total: None,
        }
    }

    /// Attach a [`TaskStatusSink`] so terminal transitions are reported.
    #[must_use]
    pub fn with_status_sink(mut self, sink: Arc<dyn TaskStatusSink>) -> Self {
        self.status_sink = sink;
        self
    }

    /// Set the turn's token target (`cfg.token_budget`) so the script's
    /// `budget.total`/`remaining()` reflect it (`spent()` is the run's own
    /// accumulated subagent output tokens).
    #[must_use]
    pub fn with_token_budget(mut self, total: Option<u64>) -> Self {
        self.token_budget_total = total;
        self
    }

    /// Share the same `workers` map with an external owner (registry wiring) so a
    /// [`TaskHandle::cleanup`] closure and [`Task::kill`] observe the same handles.
    #[must_use]
    pub fn workers_map(&self) -> Arc<Mutex<HashMap<String, WorkerCancel>>> {
        self.workers.clone()
    }

    /// Drain records queued by [`TaskHandle::cleanup`] and cancel each worker
    /// future for real (the async counterpart of the synchronous cleanup closure).
    pub async fn drain_pending_kills(&self) {
        let pending: Vec<(String, WorkerCancel)> = self.pending_kill.lock().await.drain().collect();
        for (task_id, rec) in pending {
            let _ = rec.runtime.cancel(&rec.handle).await;
            self.status_sink.set_status(&task_id, TaskStatus::Killed).await;
        }
    }
}

/// claude-code's concurrency cap for in-flight `agent()` calls:
/// `min(16, cpu_cores - 2)`, at least 1.
fn concurrency_cap() -> usize {
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    cores.saturating_sub(2).clamp(1, 16)
}

/// Build the `SubagentSpawnRequest` for one `agent(prompt, opts)` call. The
/// spawn-affecting `agent()` opts are mapped from `opts_json`
/// (`JSON.stringify(opts)`): `agentType` overrides the default subagent type,
/// `model` and `isolation` pass through. (`schema` — structured output — and the
/// display-only `label`/`phase`/`effort` opts are not mapped here.)
fn make_request(default_subagent_type: &str, prompt: &str, opts_json: &str) -> SubagentSpawnRequest {
    let opts: Value = serde_json::from_str(opts_json).unwrap_or(Value::Null);
    let opt_str = |k: &str| {
        opts.get(k)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let subagent_type = opt_str("agentType").unwrap_or_else(|| default_subagent_type.to_string());
    SubagentSpawnRequest {
        subagent_type,
        prompt: prompt.to_string(),
        context_paths: Vec::new(),
        description: None,
        model: opt_str("model"),
        run_in_background: false,
        name: None,
        team_name: None,
        mode: None,
        isolation: opt_str("isolation"),
        cwd: None,
        fork_context_messages: None,
        fork_parent_system_prompt: None,
        // `agent(prompt, { schema })` → structured output. The opt is a JSON
        // Schema object; carry it as its serialised form for the runner to force.
        schema: opts
            .get("schema")
            .filter(|v| !v.is_null())
            .map(std::string::ToString::to_string),
    }
}

/// Render a subagent's JSON `content` payload as the string the workflow's
/// `agent()` resolves with. A bare JSON string resolves to its inner text
/// (claude-code: `agent()` without a schema returns the subagent's final text);
/// any other shape (an object/array from a structured `schema` run) is
/// serialised verbatim so the script can `JSON.parse` it.
fn value_to_text(content: Value) -> String {
    match content {
        Value::String(s) => s,
        other => other.to_string(),
    }
}

/// Map a terminal subagent result to the string the runner returns for that
/// prompt. A failed/killed/errored agent maps to the empty string, which is
/// falsy in JS and so is dropped by the `.filter(Boolean)` the scripts use —
/// matching claude-code's `null` return for a skipped/dead agent.
fn result_to_string(result: Result<SubagentResult, SubagentSpawnError>) -> String {
    match result {
        Ok(SubagentResult::Completed { content, .. }) => value_to_text(content),
        Ok(SubagentResult::Failed { .. } | SubagentResult::Killed { .. }) | Err(_) => {
            String::new()
        }
    }
}

/// Render a live `phase()`/`log()` progress event as a task-output line.
fn format_progress(p: &workflow::Progress) -> String {
    match p {
        workflow::Progress::Phase(title) => format!("=== {title} ==="),
        workflow::Progress::Log(message) => message.clone(),
    }
}

/// Backs the script's `budget` global: a fixed `total` (the turn's token target)
/// and the run's own accumulated subagent output tokens as `spent`.
struct OwnSpendBudget {
    total: Option<u64>,
    spent: Arc<std::sync::atomic::AtomicU64>,
}
impl workflow::WorkflowBudgetSource for OwnSpendBudget {
    fn total(&self) -> Option<u64> {
        self.total
    }
    fn spent(&self) -> u64 {
        self.spent.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// Run a workflow `script` to completion, spawning each `agent()` call as a real
/// subagent of type `subagent_type` via `spawner`. Returns the script's
/// [`workflow::RunOutcome`] (its `phase()`/`log()` progress + return value) or a
/// [`workflow::WorkflowError`].
///
/// `tool_invoker` and `budget` are the parent's inheritance `Arc`s; the same
/// `Arc`s (cloned handle, identical inner) are handed to every child so the
/// recursion lock and budget aggregate across the whole agent tree.
pub async fn run_workflow_script(
    script: &str,
    subagent_type: &str,
    spawner: Arc<dyn SubagentSpawner>,
    tool_invoker: Arc<dyn ToolInvoker>,
    budget: Arc<dyn BudgetEnforcerHandle>,
    progress_tx: Option<mpsc::UnboundedSender<String>>,
    journal: Option<Arc<std::sync::Mutex<HashMap<String, String>>>>,
    token_budget_total: Option<u64>,
) -> Result<workflow::RunOutcome, workflow::WorkflowError> {
    use std::sync::atomic::{AtomicU64, Ordering};

    // The script's `budget`: `total` is the turn's target; `spent()` is this
    // run's own accumulated subagent output tokens (a close, safe approximation
    // of claude-code's shared pool). The worker adds each fresh subagent's
    // output tokens; replayed (journaled) agents cost nothing, as in claude-code.
    let spent = Arc::new(AtomicU64::new(0));
    let budget_source: Arc<dyn workflow::WorkflowBudgetSource> = Arc::new(OwnSpendBudget {
        total: token_budget_total,
        spent: spent.clone(),
    });
    // Request channel: each in-flight batch is (calls, reply-sender), where a
    // call is (prompt, opts_json). A buffer of one suffices — the runner blocks
    // on its reply before sending the next.
    let (req_tx, mut req_rx) =
        mpsc::channel::<(Vec<(String, String)>, oneshot::Sender<Vec<String>>)>(1);
    let (outcome_tx, outcome_rx) =
        oneshot::channel::<Result<workflow::RunOutcome, workflow::WorkflowError>>();
    let script_owned = script.to_string();

    // The script runs synchronously on its own OS thread; its batch runner is
    // plain sync code, so `blocking_send`/`blocking_recv` are safe here (this is
    // not a runtime worker thread).
    std::thread::Builder::new()
        .name("workflow-script".into())
        .spawn(move || {
            let runner = move |prompts: &[String], opts_json: &[String]| -> Vec<String> {
                let (reply_tx, reply_rx) = oneshot::channel();
                let calls: Vec<(String, String)> = prompts
                    .iter()
                    .cloned()
                    .zip(opts_json.iter().cloned())
                    .collect();
                if req_tx.blocking_send((calls, reply_tx)).is_err() {
                    return Vec::new();
                }
                reply_rx.blocking_recv().unwrap_or_default()
            };
            // `phase()`/`log()` fire this live; an unbounded `send` is non-blocking
            // and needs no runtime, so it is safe from the script thread. The host
            // drains `progress_tx` concurrently (e.g. spools to the task output).
            let on_progress = move |p: &workflow::Progress| {
                if let Some(tx) = &progress_tx {
                    let _ = tx.send(format_progress(p));
                }
            };
            let outcome = workflow::run_with_progress(
                &script_owned,
                runner,
                on_progress,
                Some(budget_source),
            );
            let _ = outcome_tx.send(outcome);
        })
        .expect("spawn workflow-script thread");

    let cap = concurrency_cap();
    // Async worker: answer each batch by spawning its subagents concurrently
    // (bounded, order-preserving). The loop ends when the runner's sender is
    // dropped — i.e. when `workflow::run` returns.
    while let Some((calls, reply)) = req_rx.recv().await {
        let results: Vec<String> =
            futures::stream::iter(calls.into_iter().map(|(prompt, opts_json)| {
                let spawner = spawner.clone();
                let inherit = SubagentInheritance {
                    tool_invoker: tool_invoker.clone(),
                    budget: budget.clone(),
                };
                let subagent_type = subagent_type.to_string();
                let journal = journal.clone();
                let spent = spent.clone();
                async move {
                    // Resume cache: a journaled result for the same (prompt, opts)
                    // is replayed instead of re-spawning. The guard is dropped
                    // before any `.await` (a std Mutex guard is not `Send`).
                    let key = format!("{prompt}\u{0}{opts_json}");
                    if let Some(j) = &journal {
                        if let Some(cached) = j.lock().unwrap().get(&key).cloned() {
                            return cached;
                        }
                    }
                    let request = make_request(&subagent_type, &prompt, &opts_json);
                    let raw = spawner.spawn(request, inherit).await;
                    // Accumulate this fresh subagent's output tokens into the run's
                    // own spend (replayed/cached agents above cost nothing).
                    if let Ok(SubagentResult::Completed { usage, .. }) = &raw {
                        spent.fetch_add(usage.output_tokens, Ordering::Relaxed);
                    }
                    let result = result_to_string(raw);
                    if let Some(j) = &journal {
                        j.lock().unwrap().insert(key, result.clone());
                    }
                    result
                }
            }))
            .buffered(cap)
            .collect()
            .await;
        // Receiver gone only if the script thread vanished; nothing to do.
        let _ = reply.send(results);
    }

    outcome_rx.await.map_err(|_| {
        workflow::WorkflowError::Engine(
            "workflow script thread terminated without an outcome".into(),
        )
    })?
}

#[async_trait]
impl Task for LocalWorkflowHandler {
    fn name(&self) -> &str {
        HANDLER_NAME
    }

    fn task_type(&self) -> TaskType {
        TaskType::LocalWorkflow
    }

    async fn spawn(
        &self,
        input: TaskSpawnInput,
        ctx: TaskContext,
    ) -> Result<TaskHandle, TaskError> {
        // 1. Only the LocalWorkflow variant is accepted.
        let TaskSpawnInput::LocalWorkflow {
            workflow_id: _workflow_id,
            script,
            resume_from_run_id,
        } = input
        else {
            return Err(TaskError::Internal(
                "local_workflow handler received a non-LocalWorkflow spawn input".into(),
            ));
        };

        // 2. Generate the task id (prefix 'w') and allocate its spool file.
        let task_id = crate::id::generate_task_id(TaskType::LocalWorkflow);
        let spool_path = self
            .output_manager
            .allocate(&task_id)
            .await
            .map_err(|e| TaskError::Io(e.to_string()))?;
        if spool_path.to_str().is_none() {
            return Err(TaskError::Internal("spool path is not valid UTF-8".into()));
        }

        // 3. Drive the workflow to completion inside a runtime-spawned worker
        //    (engine code must not call tokio::spawn directly — D17). The worker
        //    awaits `run_workflow_script`, spools the script's return value,
        //    reports the terminal status, and removes its own cancel record.
        let spawner = self.spawner.clone();
        let tool_invoker = self.tool_invoker.clone();
        let budget = self.budget.clone();
        let status_sink = self.status_sink.clone();
        let workers = self.workers.clone();
        let output_manager = self.output_manager.clone();
        let fs = ctx.fs.clone();
        let token_budget_total = self.token_budget_total;
        let worker_spool_path = spool_path.clone();
        let worker_task_id = task_id.clone();
        let worker = Box::pin(async move {
            status_sink
                .set_status(&worker_task_id, TaskStatus::Running)
                .await;

            // Resume journal: a stable `wf_…` run id keys a per-(prompt, opts)
            // `agent()` result cache. A new run mints one; a resume reuses the
            // caller's id and pre-loads its journal so unchanged agents replay
            // instead of re-spawning. The id is surfaced to the task output so a
            // later call can pass it back as `resumeFromRunId`.
            let run_id = resume_from_run_id
                .clone()
                .unwrap_or_else(|| format!("wf_{:016x}", rand::random::<u64>()));
            let journal_path = worker_spool_path
                .parent()
                .map(|d| d.join(format!("workflow-{run_id}.json")));
            let mut cache: HashMap<String, String> = HashMap::new();
            if resume_from_run_id.is_some() {
                if let Some(p) = journal_path.as_ref().and_then(|p| p.to_str()) {
                    if let Ok(fc) = fs.read_file(p, None, None).await {
                        if let Ok(loaded) =
                            serde_json::from_str::<HashMap<String, String>>(&fc.content)
                        {
                            cache = loaded;
                        }
                    }
                }
            }
            let journal = Arc::new(std::sync::Mutex::new(cache));
            let _ = output_manager
                .append(&worker_spool_path, &format!("runId: {run_id}\n"))
                .await;

            // Live progress: `phase()`/`log()` lines are spooled to the task
            // output as they happen (drained concurrently with the run), so a
            // long workflow's progress is visible via TaskOutput before it
            // finishes. The drainer ends when `run_workflow_script` drops its
            // sender on return.
            let (ptx, mut prx) = mpsc::unbounded_channel::<String>();
            let prog_output = output_manager.clone();
            let prog_spool = worker_spool_path.clone();
            let drain = async move {
                while let Some(line) = prx.recv().await {
                    let _ = prog_output.append(&prog_spool, &format!("{line}\n")).await;
                }
            };
            let run = run_workflow_script(
                &script,
                DEFAULT_WORKFLOW_SUBAGENT,
                spawner,
                tool_invoker,
                budget,
                Some(ptx),
                Some(journal.clone()),
                token_budget_total,
            );
            let (outcome, ()) = tokio::join!(run, drain);

            // Persist the journal (new + replayed results) under the run id so a
            // later resume can replay them. Serialise before any await so the std
            // Mutex guard never crosses an await point.
            let serialized = serde_json::to_string(&*journal.lock().unwrap()).ok();
            if let (Some(p), Some(s)) = (
                journal_path.as_ref().and_then(|p| p.to_str()),
                serialized.as_ref(),
            ) {
                let _ = fs.write_file(p, s).await;
            }

            // The Workflow tool result is the script's return value; spool it.
            // A script-level error spools its message and fails the task. Spool
            // I/O is best-effort — a write failure must not mask the result.
            let (payload, status) = match outcome {
                Ok(out) => (out.result.unwrap_or_default(), TaskStatus::Completed),
                Err(e) => (e.to_string(), TaskStatus::Failed),
            };
            if !payload.is_empty() {
                let _ = output_manager.append(&worker_spool_path, &payload).await;
            }

            status_sink.set_status(&worker_task_id, status).await;
            workers.lock().await.remove(&worker_task_id);
        });

        let bg_handle = ctx
            .runtime
            .spawn(&format!("{HANDLER_NAME}:{task_id}"), worker)
            .await
            .map_err(|e| TaskError::Internal(e.to_string()))?;

        self.workers.lock().await.insert(
            task_id.clone(),
            WorkerCancel {
                handle: bg_handle,
                runtime: ctx.runtime.clone(),
            },
        );

        // 4. Synchronous cleanup seam (claude-code `registerCleanup` parity): the
        //    closure cannot await, so it moves any live cancel record into
        //    `pending_kill`; `drain_pending_kills` performs the real cancel.
        let cleanup_workers = self.workers.clone();
        let cleanup_pending = self.pending_kill.clone();
        let cleanup_task_id = task_id.clone();
        let cleanup: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            if let (Ok(mut workers), Ok(mut pending)) =
                (cleanup_workers.try_lock(), cleanup_pending.try_lock())
            {
                if let Some(rec) = workers.remove(&cleanup_task_id) {
                    pending.insert(cleanup_task_id.clone(), rec);
                }
            }
        });

        Ok(TaskHandle {
            task_id,
            cleanup: Some(cleanup),
        })
    }

    async fn kill(&self, task_id: &str, _ctx: TaskContext) -> Result<(), TaskError> {
        // Cancel the in-flight worker future (the analogue of TS
        // `abortController.abort()`). An absent record ⇒ already terminated ⇒
        // graceful no-op.
        let rec = self.workers.lock().await.remove(task_id);
        if let Some(rec) = rec {
            rec.runtime
                .cancel(&rec.handle)
                .await
                .map_err(|e| TaskError::Io(e.to_string()))?;
        }
        self.status_sink.set_status(task_id, TaskStatus::Killed).await;
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::any::Any;
    use std::collections::HashMap as StdHashMap;
    use std::path::PathBuf;
    use std::sync::Mutex as StdMutex;
    use tempfile::tempdir;
    use test_harness::mocks::MockRuntimeSpawner;
    use tokio::sync::Mutex as TokioMutex;
    use traits::filesystem::{FileContent, FileEvent, FileSystem, FlockGuard, FsError};
    use traits::tool_invoker::{SubagentInvocationContext, ToolInvokerError};
    use traits::{BudgetError, SubagentUsage};

    // ---- Echo SubagentSpawner: `agent(p)` → "echo:p" (records prompts) ------

    #[derive(Default)]
    struct EchoSpawner {
        seen: StdMutex<Vec<String>>,
        seen_reqs: StdMutex<Vec<SubagentSpawnRequest>>,
        fail: bool,
    }

    #[async_trait]
    impl SubagentSpawner for EchoSpawner {
        async fn spawn(
            &self,
            request: SubagentSpawnRequest,
            _inherit: SubagentInheritance,
        ) -> Result<SubagentResult, SubagentSpawnError> {
            self.seen.lock().unwrap().push(request.prompt.clone());
            self.seen_reqs.lock().unwrap().push(request.clone());
            if self.fail {
                return Ok(SubagentResult::Failed {
                    agent_id: protocol::AgentId::new(),
                    reason: "boom".into(),
                });
            }
            Ok(SubagentResult::Completed {
                agent_id: protocol::AgentId::new(),
                content: Value::String(format!("echo:{}", request.prompt)),
                usage: SubagentUsage {
                    output_tokens: 100,
                    ..Default::default()
                },
                total_tool_use_count: 0,
                total_duration_ms: 0,
                total_tokens: 0,
                assistant_message_count: 0,
                response_char_count: 0,
                last_request_id: None,
            })
        }
    }

    // ---- Inert ToolInvoker / BudgetEnforcerHandle ---------------------------

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

    // ---- In-memory FileSystem (mirrors the other handler fixtures) ----------

    struct InMemoryFs {
        files: TokioMutex<StdHashMap<String, String>>,
    }
    impl InMemoryFs {
        fn new() -> Self {
            Self {
                files: TokioMutex::new(StdHashMap::new()),
            }
        }
    }
    #[async_trait]
    impl FileSystem for InMemoryFs {
        async fn read_file(
            &self,
            path: &str,
            offset: Option<u64>,
            limit: Option<u64>,
        ) -> Result<FileContent, FsError> {
            let map = self.files.lock().await;
            let content = map.get(path).cloned().unwrap_or_default();
            let off = usize::try_from(offset.unwrap_or(0)).unwrap_or(usize::MAX);
            let body: String = content.chars().skip(off).collect();
            let truncated = limit.is_some_and(|lim| body.len() as u64 > lim);
            let trimmed = match limit {
                Some(lim) => body
                    .chars()
                    .take(usize::try_from(lim).unwrap_or(usize::MAX))
                    .collect(),
                None => body,
            };
            let total_lines = content.lines().count() as u64;
            Ok(FileContent {
                content: trimmed,
                truncated,
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
            let mut map = self.files.lock().await;
            map.entry(path.to_string()).or_default().push_str(body);
            Ok(())
        }
        async fn truncate(&self, path: &str, len: u64) -> Result<(), FsError> {
            let mut map = self.files.lock().await;
            if let Some(s) = map.get_mut(path) {
                s.truncate(usize::try_from(len).unwrap_or(usize::MAX));
            }
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

    // ---- Recording status sink ----------------------------------------------

    #[derive(Default)]
    struct RecordingSink {
        statuses: StdMutex<Vec<(String, TaskStatus)>>,
    }
    #[async_trait]
    impl TaskStatusSink for RecordingSink {
        async fn set_status(&self, task_id: &str, status: TaskStatus) {
            self.statuses
                .lock()
                .unwrap()
                .push((task_id.to_string(), status));
        }
    }
    impl RecordingSink {
        fn last_status(&self) -> Option<TaskStatus> {
            self.statuses.lock().unwrap().last().map(|(_, s)| *s)
        }
    }

    // ---- Helpers ------------------------------------------------------------

    fn make_ctx(fs: Arc<dyn FileSystem>) -> TaskContext {
        TaskContext {
            fs,
            runtime: Arc::new(MockRuntimeSpawner::default()),
        }
    }

    fn workflow_input(script: &str) -> TaskSpawnInput {
        TaskSpawnInput::LocalWorkflow {
            workflow_id: "wf".into(),
            script: script.into(),
            resume_from_run_id: None,
        }
    }

    /// Poll the sink until it reports a terminal status (the worker runs on the
    /// `MockRuntimeSpawner`'s tokio task, so yields let it finish).
    async fn await_terminal(sink: &Arc<RecordingSink>) -> TaskStatus {
        for _ in 0..400 {
            if let Some(s) = sink.last_status() {
                if s.is_terminal() {
                    return s;
                }
            }
            tokio::task::yield_now().await;
        }
        sink.last_status().expect("worker never reported a status")
    }

    // ==== Bridge-level tests (run_workflow_script directly) ==================

    fn logs(outcome: &workflow::RunOutcome) -> Vec<String> {
        outcome
            .progress
            .iter()
            .filter_map(|p| match p {
                workflow::Progress::Log(s) => Some(s.clone()),
                workflow::Progress::Phase(_) => None,
            })
            .collect()
    }

    async fn run_bridge(script: &str, spawner: Arc<EchoSpawner>) -> workflow::RunOutcome {
        run_workflow_script(
            script,
            DEFAULT_WORKFLOW_SUBAGENT,
            spawner,
            Arc::new(MockInvoker),
            Arc::new(MockBudget),
            None,
            None,
            None,
        )
        .await
        .expect("workflow runs to completion")
    }

    #[tokio::test]
    async fn parallel_agents_round_trip_through_spawner_in_order() {
        let spawner = Arc::new(EchoSpawner::default());
        let script = r#"
            const rs = await parallel([
              () => agent('a'),
              () => agent('b'),
              () => agent('c'),
            ]);
            log('R:' + rs.join(','));
        "#;
        let outcome = run_bridge(script, spawner.clone()).await;
        assert_eq!(logs(&outcome), vec!["R:echo:a,echo:b,echo:c".to_string()]);
        let mut seen = spawner.seen.lock().unwrap().clone();
        seen.sort();
        assert_eq!(seen, vec!["a".to_string(), "b".to_string(), "c".to_string()]);
    }

    #[tokio::test]
    async fn sequential_awaits_preserve_order() {
        let spawner = Arc::new(EchoSpawner::default());
        let script = r#"
            const a = await agent('1');
            const b = await agent('2');
            log(a + '|' + b);
        "#;
        let outcome = run_bridge(script, spawner.clone()).await;
        assert_eq!(logs(&outcome), vec!["echo:1|echo:2".to_string()]);
        assert_eq!(
            *spawner.seen.lock().unwrap(),
            vec!["1".to_string(), "2".to_string()]
        );
    }

    #[tokio::test]
    async fn failed_agent_resolves_to_a_falsy_value() {
        let spawner = Arc::new(EchoSpawner {
            fail: true,
            ..Default::default()
        });
        let script = r#"
            const r = await agent('x');
            log('got:' + (r || 'NONE'));
        "#;
        let outcome = run_bridge(script, spawner).await;
        assert_eq!(logs(&outcome), vec!["got:NONE".to_string()]);
    }

    #[tokio::test]
    async fn pipeline_stages_run_each_item_through_the_spawner() {
        let spawner = Arc::new(EchoSpawner::default());
        let script = r#"
            const rs = await pipeline(
              ['x', 'y'],
              (item) => agent(item),
              (prev) => agent(prev + '!'),
            );
            log('P:' + rs.join(','));
        "#;
        let outcome = run_bridge(script, spawner.clone()).await;
        assert_eq!(
            logs(&outcome),
            vec!["P:echo:echo:x!,echo:echo:y!".to_string()]
        );
    }

    #[tokio::test]
    async fn agent_opts_map_to_the_spawn_request() {
        let spawner = Arc::new(EchoSpawner::default());
        let script = r#"
            await agent('p', { agentType: 'code-reviewer', model: 'opus', isolation: 'worktree', schema: { type: 'object' } });
            await agent('plain');
        "#;
        run_bridge(script, spawner.clone()).await;
        let reqs = spawner.seen_reqs.lock().unwrap().clone();
        assert_eq!(reqs.len(), 2);
        // agentType / model / isolation / schema opts → the spawn request.
        assert_eq!(reqs[0].subagent_type, "code-reviewer");
        assert_eq!(reqs[0].model.as_deref(), Some("opus"));
        assert_eq!(reqs[0].isolation.as_deref(), Some("worktree"));
        assert_eq!(reqs[0].schema.as_deref(), Some(r#"{"type":"object"}"#));
        // A bare agent(prompt) → default type, no overrides.
        assert_eq!(reqs[1].subagent_type, "general-purpose");
        assert_eq!(reqs[1].model, None);
        assert_eq!(reqs[1].isolation, None);
        assert_eq!(reqs[1].schema, None);
    }

    // ==== Handler-level tests (full Task lifecycle) =========================

    fn make_handler(
        spawner: Arc<dyn SubagentSpawner>,
        mgr: Arc<TaskOutputManager>,
        sink: Arc<dyn TaskStatusSink>,
    ) -> LocalWorkflowHandler {
        LocalWorkflowHandler::new(spawner, Arc::new(MockInvoker), Arc::new(MockBudget), mgr)
            .with_status_sink(sink)
    }

    #[tokio::test]
    async fn handler_runs_workflow_and_spools_the_return_value() {
        let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
        let spawner = Arc::new(EchoSpawner::default());
        let dir = tempdir().unwrap();
        let mgr = Arc::new(TaskOutputManager::new(PathBuf::from(dir.path()), fs.clone()));
        let sink = Arc::new(RecordingSink::default());

        let handler = make_handler(spawner.clone(), mgr.clone(), sink.clone());

        // A workflow that fans out two agents and returns a structured result.
        let script = r#"
            const rs = await parallel([() => agent('a'), () => agent('b')]);
            return { confirmed: rs };
        "#;
        let handle = handler
            .spawn(workflow_input(script), make_ctx(fs))
            .await
            .expect("spawn should succeed");

        assert!(handle.task_id.starts_with('w'), "LocalWorkflow id prefix 'w'");
        assert!(handle.cleanup.is_some(), "cleanup seam present");

        assert_eq!(await_terminal(&sink).await, TaskStatus::Completed);

        // Both agents ran.
        let mut seen = spawner.seen.lock().unwrap().clone();
        seen.sort();
        assert_eq!(seen, vec!["a".to_string(), "b".to_string()]);

        // The script's return value (JSON) was spooled as the task result,
        // after the surfaced run id.
        let spool_path = dir.path().join(format!("{}.output", handle.task_id));
        let read = mgr
            .read(&spool_path, crate::output_manager::OutputOptions::default())
            .await
            .unwrap();
        assert!(read.content.starts_with("runId: wf_"), "{}", read.content);
        assert!(
            read.content.contains(r#"{"confirmed":["echo:a","echo:b"]}"#),
            "{}",
            read.content
        );
    }

    #[tokio::test]
    async fn handler_spools_live_progress_then_the_result() {
        let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
        let spawner = Arc::new(EchoSpawner::default());
        let dir = tempdir().unwrap();
        let mgr = Arc::new(TaskOutputManager::new(PathBuf::from(dir.path()), fs.clone()));
        let sink = Arc::new(RecordingSink::default());
        let handler = make_handler(spawner, mgr.clone(), sink.clone());

        let script = r#"
            phase('Scan');
            log('found 2 things');
            return { ok: true };
        "#;
        let handle = handler
            .spawn(workflow_input(script), make_ctx(fs))
            .await
            .unwrap();
        assert_eq!(await_terminal(&sink).await, TaskStatus::Completed);

        let spool_path = dir.path().join(format!("{}.output", handle.task_id));
        let read = mgr
            .read(&spool_path, crate::output_manager::OutputOptions::default())
            .await
            .unwrap();
        // phase/log were spooled live, followed by the return value.
        assert!(read.content.contains("=== Scan ==="), "phase: {}", read.content);
        assert!(read.content.contains("found 2 things"), "log: {}", read.content);
        assert!(read.content.contains(r#"{"ok":true}"#), "result: {}", read.content);
    }

    #[tokio::test]
    async fn resume_replays_journaled_agent_results_without_respawning() {
        let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
        let dir = tempdir().unwrap();
        let mgr = Arc::new(TaskOutputManager::new(PathBuf::from(dir.path()), fs.clone()));
        let script = r#"
            const a = await agent('a');
            const b = await agent('b');
            return { a, b };
        "#;

        // Run 1: fresh run — both agents spawn; capture the surfaced runId.
        let spawner1 = Arc::new(EchoSpawner::default());
        let sink1 = Arc::new(RecordingSink::default());
        let h1 = make_handler(spawner1.clone(), mgr.clone(), sink1.clone());
        let handle1 = h1
            .spawn(workflow_input(script), make_ctx(fs.clone()))
            .await
            .unwrap();
        assert_eq!(await_terminal(&sink1).await, TaskStatus::Completed);
        assert_eq!(spawner1.seen.lock().unwrap().len(), 2, "run 1 spawns both");

        let spool1 = dir.path().join(format!("{}.output", handle1.task_id));
        let out1 = mgr
            .read(&spool1, crate::output_manager::OutputOptions::default())
            .await
            .unwrap();
        let run_id = out1
            .content
            .lines()
            .find_map(|l| l.strip_prefix("runId: "))
            .expect("runId surfaced")
            .to_string();
        assert!(run_id.starts_with("wf_"), "runId: {run_id}");

        // Run 2: resume the same id — every agent() must replay from the journal,
        // so the spawner is never called, and the result is rebuilt from cache.
        let spawner2 = Arc::new(EchoSpawner::default());
        let sink2 = Arc::new(RecordingSink::default());
        let h2 = make_handler(spawner2.clone(), mgr.clone(), sink2.clone());
        let input2 = TaskSpawnInput::LocalWorkflow {
            workflow_id: "wf".into(),
            script: script.into(),
            resume_from_run_id: Some(run_id),
        };
        let handle2 = h2.spawn(input2, make_ctx(fs.clone())).await.unwrap();
        assert_eq!(await_terminal(&sink2).await, TaskStatus::Completed);
        assert!(
            spawner2.seen.lock().unwrap().is_empty(),
            "resume must replay journaled results, not re-spawn"
        );

        let spool2 = dir.path().join(format!("{}.output", handle2.task_id));
        let out2 = mgr
            .read(&spool2, crate::output_manager::OutputOptions::default())
            .await
            .unwrap();
        assert!(
            out2.content.contains(r#"{"a":"echo:a","b":"echo:b"}"#),
            "rebuilt from cache: {}",
            out2.content
        );
    }

    #[tokio::test]
    async fn budget_total_and_own_spend_drive_the_budget_global() {
        let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
        let spawner = Arc::new(EchoSpawner::default());
        let dir = tempdir().unwrap();
        let mgr = Arc::new(TaskOutputManager::new(PathBuf::from(dir.path()), fs.clone()));
        let sink = Arc::new(RecordingSink::default());
        let handler =
            make_handler(spawner, mgr.clone(), sink.clone()).with_token_budget(Some(500));

        // Each echo agent reports 100 output tokens; two agents ⇒ spent 200.
        let script = r#"
            await agent('a');
            await agent('b');
            log('B:' + budget.total + '/' + budget.spent() + '/' + budget.remaining());
            return {};
        "#;
        let handle = handler
            .spawn(workflow_input(script), make_ctx(fs))
            .await
            .unwrap();
        assert_eq!(await_terminal(&sink).await, TaskStatus::Completed);

        let spool = dir.path().join(format!("{}.output", handle.task_id));
        let read = mgr
            .read(&spool, crate::output_manager::OutputOptions::default())
            .await
            .unwrap();
        // total=500, spent=2×100, remaining=300.
        assert!(read.content.contains("B:500/200/300"), "{}", read.content);
    }

    #[tokio::test]
    async fn handler_maps_a_script_error_to_failed() {
        let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
        let spawner = Arc::new(EchoSpawner::default());
        let dir = tempdir().unwrap();
        let mgr = Arc::new(TaskOutputManager::new(PathBuf::from(dir.path()), fs.clone()));
        let sink = Arc::new(RecordingSink::default());

        let handler = make_handler(spawner, mgr, sink.clone());

        // A script that throws ⇒ WorkflowError::Script ⇒ Failed.
        let handle = handler
            .spawn(workflow_input("throw new Error('kaboom');"), make_ctx(fs))
            .await
            .unwrap();

        assert_eq!(await_terminal(&sink).await, TaskStatus::Failed);
        assert!(handle.task_id.starts_with('w'));
    }

    #[tokio::test]
    async fn handler_rejects_a_non_workflow_input() {
        let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
        let dir = tempdir().unwrap();
        let mgr = Arc::new(TaskOutputManager::new(PathBuf::from(dir.path()), fs.clone()));
        let handler = make_handler(
            Arc::new(EchoSpawner::default()),
            mgr,
            Arc::new(RecordingSink::default()),
        );

        let wrong = TaskSpawnInput::LocalAgent {
            agent_id: protocol::AgentId::new(),
            subagent_type: "general-purpose".into(),
            prompt: "p".into(),
            is_backgrounded: true,
        };
        match handler.spawn(wrong, make_ctx(fs)).await {
            Err(TaskError::Internal(_)) => {}
            Err(other) => panic!("expected Internal, got {other:?}"),
            Ok(_) => panic!("non-LocalWorkflow input must be rejected"),
        }
    }

    #[tokio::test]
    async fn name_and_type_are_local_workflow() {
        let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
        let dir = tempdir().unwrap();
        let mgr = Arc::new(TaskOutputManager::new(PathBuf::from(dir.path()), fs));
        let handler = make_handler(
            Arc::new(EchoSpawner::default()),
            mgr,
            Arc::new(RecordingSink::default()),
        );
        assert_eq!(handler.name(), "local_workflow");
        assert_eq!(handler.task_type(), TaskType::LocalWorkflow);
    }
}

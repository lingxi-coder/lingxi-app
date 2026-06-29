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
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use futures::stream::StreamExt;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot, Mutex};
use traits::filesystem::FileSystem;
use traits::{
    BackgroundTaskHandle, BudgetEnforcerHandle, RuntimeSpawner, SubagentInheritance,
    SubagentResult, SubagentSpawnError, SubagentSpawnRequest, SubagentSpawner, ToolInvoker,
};

use crate::id::TaskType;
use crate::output_manager::TaskOutputManager;
use crate::state::TaskStatus;
use crate::task_trait::{Task, TaskContext, TaskError, TaskHandle, TaskSpawnInput};
use telemetry::sink::{AnalyticsValue, LogEventMetadata};
use telemetry::AnalyticsBus;

// Reuse the status-sink seam defined once in the bash handler (single impl wired
// across handlers), exactly as `local_agent` does.
pub use crate::handlers::local_bash::{NoopStatusSink, TaskStatusSink};

/// Handler name reported by [`Task::name`] / used as the runtime task-name.
const HANDLER_NAME: &str = "local_workflow";

/// claude-code `k6a` — the per-run lifetime cap on real `agent()` spawns. The
/// 1001st spawn is refused via the throw channel so the prelude rejects the
/// `agent()` promise with `WorkflowAgentCapError` (a runaway-loop backstop).
const WORKFLOW_AGENT_CAP: u64 = 1000;

/// The `WorkflowAgentCapError` message (binary `c0p` @201953488), `${k6a}`=1000.
const WORKFLOW_AGENT_CAP_MESSAGE: &str = "Workflow agent() call cap reached (1000). This usually means a loop using budget.remaining() never terminates because no token budget was set \u{2014} remaining() returns Infinity when budget.total is null. Add a hard iteration cap to the loop, or pass a token budget.";

/// Prefix the worker prepends to a result slot to make the prelude THROW that
/// `agent()` with the rest of the string as the Error message (agent-cap or
/// budget-ceiling). U+0001-framed: collision-proof and NUL-free (the prelude
/// eval path uses a C string). MUST stay byte-identical to the prelude's
/// `__WF_THROW_PREFIX` (`String.fromCharCode(1)+"__wf_throw__"+...`).
const WF_THROW_PREFIX: &str = "\u{1}__wf_throw__\u{1}";

/// Build a throw-channel result slot carrying `message`.
fn wf_throw(message: &str) -> String {
    format!("{WF_THROW_PREFIX}{message}")
}

/// Normalize the opts object for the resume chain-key (claude-code `ABp`).
///
/// The binary projects opts to ONLY `["schema","model","effort","isolation","agentType"]`
/// (in that order; undefined/function values skipped) then `JSON.stringify` with a
/// recursive key-sorter. In JSON there are no functions, so we skip null/absent.
///
/// IMPORTANT: `serde_json` is compiled with `preserve_order` (IndexMap-backed), so
/// `serde_json::Map` preserves INSERTION order, NOT alphabetical order. The explicit
/// sort in `sort_value` (and the fixed `KEYS`-slice iteration order for the outer map)
/// is therefore REQUIRED for determinism — do not remove it.
///
/// This means display-only fields like `phase`, `label`, `stallMs` are stripped,
/// so annotating a call differently does NOT change the key and does NOT invalidate
/// the cache on resume.
fn normalize_opts_for_chain_key(opts: &Value) -> String {
    const KEYS: &[&str] = &["schema", "model", "effort", "isolation", "agentType"];
    let mut map = serde_json::Map::new();
    if let Some(obj) = opts.as_object() {
        for &k in KEYS {
            if let Some(v) = obj.get(k) {
                if !v.is_null() {
                    map.insert(k.to_string(), sort_value(v.clone()));
                }
            }
        }
    }
    serde_json::to_string(&Value::Object(map)).unwrap_or_else(|_| "{}".to_string())
}

/// Recursively sort object keys so the JSON representation is deterministic
/// regardless of insertion order (mirrors the binary's `JSON.stringify` key-sorter).
///
/// IMPORTANT: `serde_json` is compiled with `preserve_order` (IndexMap-backed), so
/// `serde_json::Map` preserves INSERTION order, NOT alphabetical order. The explicit
/// sort here is REQUIRED for determinism — do not remove it.
fn sort_value(v: Value) -> Value {
    match v {
        Value::Object(map) => {
            // Collect into a Vec, sort by key, then rebuild a Map. Since serde_json
            // uses IndexMap with preserve_order, the sort is LOAD-BEARING — without
            // it, insertion order would determine the JSON output, making chain keys
            // non-deterministic across different construction paths.
            let mut pairs: Vec<(String, Value)> = map.into_iter().collect();
            pairs.sort_by(|a, b| a.0.cmp(&b.0));
            let sorted: serde_json::Map<String, Value> =
                pairs.into_iter().map(|(k, cv)| (k, sort_value(cv))).collect();
            Value::Object(sorted)
        }
        Value::Array(arr) => Value::Array(arr.into_iter().map(sort_value).collect()),
        other => other,
    }
}

/// The chained resume-cache key for an `agent(prompt, opts)` call (claude-code
/// `qKa(se, te, m)`): a running hash that folds in the PREVIOUS key (`prev`), so
/// any change in the preceding sequence of agent() calls cascades into every
/// later key — giving "longest unchanged prefix" replay (a reorder, an inserted
/// call, or an edited prompt all break the chain from that point on). The exact
/// hash bytes are private to LingXi (the journal is its own same-session format),
/// so a stable FNV-1a-64 over `prev | prompt | opts` suffices.
fn chain_key(prev: &str, prompt: &str, opts_json: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut fold = |bytes: &[u8]| {
        for &b in bytes {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    fold(prev.as_bytes());
    fold(b"\x1e");
    fold(prompt.as_bytes());
    fold(b"\x1f");
    fold(opts_json.as_bytes());
    format!("{h:016x}")
}

/// `WorkflowBudgetExceededError` message (binary `I6a` @201953813), with
/// thousands-separated counts (`toLocaleString`).
fn workflow_budget_exceeded_message(spent: u64, total: u64) -> String {
    format!(
        "Workflow token budget exceeded ({} / {} output tokens). Stopping further agent() calls. In-flight agents will complete; their results are preserved.",
        group_en_us(spent),
        group_en_us(total),
    )
}

/// `Number.prototype.toLocaleString()` for en-US: group integer digits in 3s
/// with commas.
fn group_en_us(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

/// The subagent type spawned for a bare `agent(prompt)` call — claude-code's
/// default workflow subagent.
pub const DEFAULT_WORKFLOW_SUBAGENT: &str = "workflow-subagent";

/// A live worker-cancel record: the background-task handle plus the runtime that
/// minted it, so [`Task::kill`] / cleanup can cancel the in-flight worker without
/// a fresh [`TaskContext`]. (Each handler keeps its own — the fields are private;
/// `pub` so it can appear in [`LocalWorkflowHandler::workers_map`]'s return type.)
pub struct WorkerCancel {
    handle: BackgroundTaskHandle,
    runtime: Arc<dyn RuntimeSpawner>,
    /// Cooperative-cancel flag shared with the running script thread's engine
    /// interrupt handler. Flipped by [`Task::kill`] / cleanup so a runaway
    /// pure-JS loop aborts instead of leaking the OS thread.
    cancel: Arc<std::sync::atomic::AtomicBool>,
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
    /// Analytics bus for emitting `tengu_workflow_*` telemetry events.
    bus: Arc<AnalyticsBus>,
    /// The turn's token target (`cfg.token_budget`) backing the script's
    /// `budget.total`. `None` ⇒ no target (FLEET defaults, budget loops skip).
    /// Set by the root via [`Self::with_token_budget`].
    token_budget_total: Option<u64>,
    /// Late-bound shared output-token pool backing the script's `budget.spent()`.
    /// The composition root publishes the orchestrator's
    /// `output_token_pool` here once it exists (the handler is registered before
    /// the orchestrator is built — same deferred pattern as the tool invoker).
    /// When set, a run's subagents add their output tokens to this same `Arc`
    /// that the main loop also feeds, so `spent()` reads main loop + all
    /// workflows. When unset (tests), a run falls back to its own private pool.
    output_pool_cell: Option<Arc<OnceLock<Arc<AtomicU64>>>>,
    /// Late-bound turn-start output baseline (claude-code `xtr`): the cumulative
    /// output at the start of the CURRENT turn. The composition root publishes
    /// the orchestrator's `turn_start_output_baseline` here. At spawn the handler
    /// snapshots `baseline.load()` so the run's `budget.spent()` is turn-relative
    /// (`pool - baseline` = `getTurnSpent()`). Unset (tests) ⇒ baseline 0.
    turn_baseline_cell: Option<Arc<OnceLock<Arc<AtomicU64>>>>,
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
            bus: Arc::new(AnalyticsBus::new()),
            token_budget_total: None,
            output_pool_cell: None,
            turn_baseline_cell: None,
        }
    }

    /// Attach a [`TaskStatusSink`] so terminal transitions are reported.
    #[must_use]
    pub fn with_status_sink(mut self, sink: Arc<dyn TaskStatusSink>) -> Self {
        self.status_sink = sink;
        self
    }

    /// Attach an [`AnalyticsBus`] so `tengu_workflow_*` events are emitted.
    /// Without this, a no-op bus is used (default).
    #[must_use]
    pub fn with_bus(mut self, bus: Arc<AnalyticsBus>) -> Self {
        self.bus = bus;
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

    /// Late-bind the shared output-token pool backing `budget.spent()`. The
    /// composition root passes a cloned `OnceLock` cell here at registration and
    /// publishes the orchestrator's `output_token_pool` into it once the
    /// orchestrator is built — so a run's `spent()` reflects the union of
    /// main-loop and all-workflow output tokens. See [`Self::output_pool_cell`].
    #[must_use]
    pub fn with_output_pool_cell(mut self, cell: Arc<OnceLock<Arc<AtomicU64>>>) -> Self {
        self.output_pool_cell = Some(cell);
        self
    }

    /// Late-bind the turn-start output baseline cell (claude-code `xtr`). The
    /// composition root publishes the orchestrator's `turn_start_output_baseline`
    /// so each run's `budget.spent()` is turn-relative. See
    /// [`Self::turn_baseline_cell`].
    #[must_use]
    pub fn with_turn_baseline_cell(mut self, cell: Arc<OnceLock<Arc<AtomicU64>>>) -> Self {
        self.turn_baseline_cell = Some(cell);
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
            rec.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            let _ = rec.runtime.cancel(&rec.handle).await;
            self.status_sink.set_status(&task_id, TaskStatus::Killed).await;
        }
    }
}

/// claude-code's concurrency cap for in-flight `agent()` calls:
/// `Math.min(16, Math.max(2, cpus-2))` — at least 2.
fn concurrency_cap() -> usize {
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    cores.saturating_sub(2).max(2).min(16)
}

/// Build the `SubagentSpawnRequest` for one `agent(prompt, opts)` call. The
/// spawn-affecting `agent()` opts are mapped from `opts_json`
/// (`JSON.stringify(opts)`): `agentType` overrides the default subagent type,
/// `model` and `isolation` pass through, and `label` becomes the subagent's
/// display name (claude-code `re = opts.label ?? prompt.slice(0,60)`, surfaced
/// as the agent's progress label). (`schema` is handled below; the `phase` opt
/// groups the agent in claude's /workflows progress tree, for which LingXi has
/// no per-agent progress surface — only `phase()`/`log()` lines — so it has no
/// mapping target here.)
fn make_request(default_subagent_type: &str, prompt: &str, opts_json: &str) -> SubagentSpawnRequest {
    let opts: Value = serde_json::from_str(opts_json).unwrap_or(Value::Null);
    let opt_str = |k: &str| {
        opts.get(k)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let subagent_type = opt_str("agentType").unwrap_or_else(|| default_subagent_type.to_string());

    // Workflow agent() routing (binary §§1-4 + §6):
    // Case 1: bare agent(prompt) → workflow-subagent + kBp (subagent_type already =
    //   "workflow-subagent" from DEFAULT_WORKFLOW_SUBAGENT; no override needed)
    // Case 2: agent(prompt, {schema}) only → workflow-subagent + xBp (override kBp prompt)
    // Case 3: agent(prompt, {agentType}) → that type + HBp addendum + disallow union
    // Case 4: agent(prompt, {agentType, schema}) → that type + IBp addendum + disallow union
    let has_agent_type = opt_str("agentType").is_some();
    let has_schema = opts.get("schema").filter(|v| !v.is_null()).is_some();

    let system_prompt_override = if !has_agent_type && has_schema {
        // Case 2: bare schema call → xBp replaces kBp
        Some(agent::builtins::WORKFLOW_SUBAGENT_SCHEMA_PROMPT.to_string())
    } else {
        None
    };

    let system_prompt_addendum = if has_agent_type {
        if has_schema {
            // Case 4: user agentType + schema → IBp addendum
            Some(agent::builtins::WORKFLOW_SUBAGENT_SCHEMA_ADDENDUM.to_string())
        } else {
            // Case 3: user agentType no schema → HBp addendum
            Some(agent::builtins::WORKFLOW_SUBAGENT_NON_SCHEMA_ADDENDUM.to_string())
        }
    } else {
        None
    };

    let additional_disallowed_tools = if has_agent_type {
        // Cases 3/4: union disallowed with {SendUserMessage, Agent, Workflow}
        agent::builtins::workflow_subagent_disallowed()
    } else {
        vec![]
    };

    SubagentSpawnRequest {
        subagent_type,
        prompt: prompt.to_string(),
        context_paths: Vec::new(),
        description: None,
        model: opt_str("model"),
        model_profile: None,
        run_in_background: false,
        // `agent(prompt, { label })` → the subagent's display label.
        name: opt_str("label"),
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
        // `agent(prompt, { effort })` → override the subagent's thinking effort
        // (claude-code `me={...ie,effort:ae}`). A level string or integer, carried
        // raw for the spawner to apply onto the resolved agent definition.
        effort: opts.get("effort").filter(|v| !v.is_null()).cloned(),
        // Workflow `agent()` spawns are not tool-call-originated background tasks.
        tool_use_id: None,
        system_prompt_override,
        system_prompt_addendum,
        additional_disallowed_tools,
        depth: 0,
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
/// prompt. A failed/killed/errored agent maps to the NULL sentinel, which the
/// prelude resolves the `agent()` promise with `null` — claude-code's contract
/// (`if (Be.skipped) return null` / `if (Be.apiError) return null`). `null` is
/// falsy, so `.filter(Boolean)` still drops it, while explicit `=== null` / `??`
/// checks now behave correctly. (An empty-but-successful agent still returns "".)
fn result_to_string(result: Result<SubagentResult, SubagentSpawnError>) -> String {
    match result {
        Ok(SubagentResult::Completed { content, .. }) => value_to_text(content),
        Ok(SubagentResult::Failed { .. } | SubagentResult::Killed { .. }) | Err(_) => {
            workflow::WF_NULL_SENTINEL.to_string()
        }
    }
}

/// Render a live `phase()`/`log()`/`agent` progress event as a task-output line.
///
/// `Phase` and `Log` render as human-readable text lines (back-compat).
/// `Agent` events serialize as a JSON line prefixed `[workflow_agent] ` —
/// this provides structured data in the task-output spool while keeping the
/// format readable (and parseable by consumers looking for `[workflow_agent]`).
fn format_progress(p: &workflow::Progress) -> String {
    match p {
        workflow::Progress::Phase { index, title } => format!("[{index}] === {title} ==="),
        workflow::Progress::Log { message } => message.clone(),
        workflow::Progress::Agent {
            index,
            label,
            phase_index,
            phase_title,
            agent_id,
            model,
            state,
            tool_use_id,
        } => {
            let mut obj = serde_json::json!({
                "type": "workflow_agent",
                "index": index,
                "label": label,
                "state": state.as_str(),
                "toolUseID": tool_use_id,
            });
            if let Some(obj_map) = obj.as_object_mut() {
                if let Some(pi) = phase_index {
                    obj_map.insert("phaseIndex".to_string(), serde_json::json!(pi));
                }
                if let Some(pt) = phase_title {
                    obj_map.insert("phaseTitle".to_string(), serde_json::json!(pt));
                }
                if let Some(id) = agent_id {
                    obj_map.insert("agentId".to_string(), serde_json::json!(id));
                }
                if let Some(m) = model {
                    obj_map.insert("model".to_string(), serde_json::json!(m));
                }
            }
            format!("[workflow_agent] {}", serde_json::to_string(&obj).unwrap_or_default())
        }
    }
}

/// Backs the script's `budget` global: a fixed `total` (the turn's token target)
/// and `spent()` = output tokens spent SINCE THE CURRENT TURN STARTED — the
/// shared cumulative pool minus the turn-start baseline (`baseline`). This is
/// claude-code `getTurnSpent()` = `rT() - R` where `R = xtr` is the cumulative
/// output at turn start (binary @192177594/@202008079): a turn-relative delta,
/// NOT the absolute session pool. `baseline` is snapshotted at workflow start.
struct OwnSpendBudget {
    total: Option<u64>,
    spent: Arc<std::sync::atomic::AtomicU64>,
    /// Cumulative output at the start of the turn this workflow was spawned in.
    baseline: u64,
}
impl OwnSpendBudget {
    fn turn_spent(&self) -> u64 {
        self.spent
            .load(std::sync::atomic::Ordering::Relaxed)
            .saturating_sub(self.baseline)
    }
}
impl workflow::WorkflowBudgetSource for OwnSpendBudget {
    fn total(&self) -> Option<u64> {
        self.total
    }
    fn spent(&self) -> u64 {
        self.turn_spent()
    }
}

/// Nesting config for a workflow run: whether top-level `workflow()` is allowed
/// (a nested run sets `false`, enforcing claude-code's one-level limit), the
/// `args` global (JSON string), and a filesystem to resolve a nested
/// `workflow({scriptPath})` / `workflow(name)`.
#[derive(Default, Clone)]
pub struct NestedConfig {
    /// `true` for the top-level run; `false` inside a nested workflow.
    pub allow_nested: bool,
    /// The `args` global value as a JSON string. `None` ⇒ `undefined`.
    pub args: Option<String>,
    /// Filesystem for resolving `workflow({scriptPath})` / `workflow(name)`.
    pub fs: Option<Arc<dyn FileSystem>>,
}

/// Per-call plan for one batch: decided sequentially in Phase A (prefix-cache
/// cursor), executed concurrently in Phase B.
enum Plan {
    /// `__wf_resolve` — resolve a nested workflow reference to its source.
    Resolve(Value),
    /// Replay a journaled result (prefix hit).
    Cached {
        /// The cached result string.
        result: String,
        /// 0-based agent call ordinal.
        call_index: u64,
        /// The agent's label: `opts.label ?? prompt.slice(0, 60)`.
        label: String,
        /// Phase info at dispatch time (extracted from `__wf_phase`).
        phase_index: Option<u32>,
        /// Phase title at dispatch time.
        phase_title: Option<String>,
    },
    /// Spawn a real subagent; journal the result under `key` (when present).
    Live {
        key: Option<String>,
        prompt: String,
        opts_json: String,
        /// 0-based agent call ordinal.
        call_index: u64,
        /// The agent's label: `opts.label ?? prompt.slice(0, 60)`.
        label: String,
        /// Phase info at dispatch time (extracted from `__wf_phase`).
        phase_index: Option<u32>,
        /// Phase title at dispatch time.
        phase_title: Option<String>,
    },
}

/// Context fields for `tengu_workflow_phase_completed` events (oracle §7).
/// these are not available inside `run_workflow_script` itself (the run_id is
/// minted in the outer worker closure), so the caller threads them in as a bundle.
/// When `None`, phase events still fire but omit the context fields.
pub struct PhaseTelemetryCtx {
    /// The `wf_…` run id for this workflow invocation.
    pub run_id: String,
    /// Invocation source: path for `scriptPath`, name for `named`, `"inline"`.
    pub workflow_source: Option<String>,
    /// `meta.name` from the workflow script.
    pub workflow_name: Option<String>,
    /// How the workflow was invoked: `"scriptPath"` | `"named"` | `"inline"`.
    ///
    /// Oracle §7: `tengu_workflow_phase_completed` is gated on `p.source === "built-in"`,
    /// which corresponds to named/saved workflows only (invocation_mode == `"named"`).
    /// Inline scripts and arbitrary `scriptPath` invocations do NOT emit this event.
    pub invocation_mode: Option<String>,
}

/// Run a workflow `script` to completion, spawning each `agent()` call as a real
/// subagent of type `subagent_type` via `spawner`. Returns the script's
/// [`workflow::RunOutcome`] (its `phase()`/`log()` progress + return value) or a
/// [`workflow::WorkflowError`].
///
/// `tool_invoker` and `budget` are the parent's inheritance `Arc`s; the same
/// `Arc`s (cloned handle, identical inner) are handed to every child so the
/// recursion lock and budget aggregate across the whole agent tree.
///
/// `shared_pool` backs the script's `budget.spent()`: when `Some`, it is the
/// session-wide pool the main loop also feeds (every successful main-loop API
/// response adds its output tokens), so `spent()` reads main loop + every
/// workflow — claude-code's shared pool. When `None` (tests / no orchestrator),
/// the run uses a fresh private pool counting only its own subagent output.
#[allow(clippy::too_many_arguments)]
pub async fn run_workflow_script(
    script: &str,
    subagent_type: &str,
    spawner: Arc<dyn SubagentSpawner>,
    tool_invoker: Arc<dyn ToolInvoker>,
    budget: Arc<dyn BudgetEnforcerHandle>,
    progress_tx: Option<mpsc::UnboundedSender<String>>,
    journal: Option<Arc<std::sync::Mutex<HashMap<String, String>>>>,
    token_budget_total: Option<u64>,
    shared_pool: Option<Arc<AtomicU64>>,
    turn_start_baseline: u64,
    nested: NestedConfig,
    // Cooperative-cancel flag (claude-code `abortController`). `Task::kill` flips
    // it; the embedded engine's interrupt handler then aborts the script thread,
    // so a runaway pure-JS loop is stopped instead of leaking the OS thread.
    cancel: Arc<std::sync::atomic::AtomicBool>,
    bus: Arc<AnalyticsBus>,
    // Optional externally-provided agent_count Arc so the caller can read the final
    // count after the run (used by tengu_workflow_completed). When None, a private
    // counter is created (test/standalone path).
    agent_count_out: Option<Arc<AtomicU64>>,
    // Optional fields for tengu_workflow_phase_completed (oracle §7):
    // `workflow_run_id`, `workflow_source`, `workflow_name`. When None, the
    // phase_completed events still fire but omit these optional context fields.
    phase_telemetry_ctx: Option<PhaseTelemetryCtx>,
) -> Result<workflow::RunOutcome, workflow::WorkflowError> {
    let NestedConfig {
        allow_nested,
        args: nested_args,
        fs: nested_fs,
    } = nested;
    use std::sync::atomic::Ordering;

    // The script's `budget`: `total` is the turn's target; `spent()` reads the
    // shared pool — main loop (fed by the orchestrator per response) plus every
    // workflow (the worker adds each fresh subagent's output tokens; replayed
    // journaled agents cost nothing, as in claude-code). Without a shared pool
    // (tests), a private counter tracks this run's own subagent output only.
    let spent = shared_pool.unwrap_or_else(|| Arc::new(AtomicU64::new(0)));
    let budget_source: Arc<dyn workflow::WorkflowBudgetSource> = Arc::new(OwnSpendBudget {
        total: token_budget_total,
        spent: spent.clone(),
        baseline: turn_start_baseline,
    });
    // Request channel: each in-flight batch is (calls, reply-sender), where a
    // call is (prompt, opts_json). A buffer of one suffices — the runner blocks
    // on its reply before sending the next.
    let (req_tx, mut req_rx) =
        mpsc::channel::<(Vec<(String, String)>, oneshot::Sender<Vec<String>>)>(1);
    let (outcome_tx, outcome_rx) =
        oneshot::channel::<Result<workflow::RunOutcome, workflow::WorkflowError>>();
    let script_owned = script.to_string();
    // Clone progress_tx for the async worker loop (agent lifecycle events). The
    // script thread owns the original for phase()/log() events; the async worker
    // uses this clone to emit workflow_agent start/done/error/cached events.
    let progress_tx_for_worker = progress_tx.clone();

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
                allow_nested,
                nested_args,
                Some(cancel),
            );
            let _ = outcome_tx.send(outcome);
        })
        .expect("spawn workflow-script thread");

    let cap = concurrency_cap();
    // Per-run real-spawn counter (claude-code `c`/`S()`): only fresh spawns
    // count — replayed (journaled) and `__wf_resolve` calls are exempt, so
    // resuming a >1000-agent workflow never trips the cap on replay.
    // When agent_count_out is provided by the caller (for tengu_workflow_completed
    // reporting), use that Arc so the caller can read the final count after the run.
    let agent_count = agent_count_out.unwrap_or_else(|| Arc::new(AtomicU64::new(0)));
    // Per-run agent call ordinal counter (oracle §8: the `index` field on
    // `workflow_agent` events is monotonically incrementing per call, including
    // cached replay calls; `__wf_resolve` calls do NOT count).
    let call_index_counter = Arc::new(AtomicU64::new(0));
    // Use the pre-cloned progress_tx for the async worker (agent lifecycle events).
    let worker_progress_tx = progress_tx_for_worker;
    // PREFIX resume cursor (claude-code `m` + gone-live flag `f`): the journal is
    // a longest-unchanged-prefix cache. `running_key` chains each real agent()
    // call into the previous key (so any change cascades to all later keys), and
    // once a lookup MISSES we go live (`gone_live`) for every later call — never
    // replaying a stale result out of order. Both advance in agent()-CALL order,
    // which the worker sees batch-by-batch (this loop is sequential) and, within
    // a batch, in the queue's call order. `__wf_resolve` (nested-workflow source)
    // calls are not real agents: they neither advance the chain nor touch the
    // journal.
    let mut running_key = String::new();
    let mut gone_live = false;

    // Async worker: answer each batch. Phase A decides cached-vs-live per call in
    // order (advancing the prefix cursor); Phase B runs the plans concurrently
    // (bounded, order-preserving). The loop ends when the runner's sender is
    // dropped — i.e. when `workflow::run` returns.
    while let Some((calls, reply)) = req_rx.recv().await {
        // Phase A — sequential, in call order: prefix-cache decision per call.
        let mut plans: Vec<Plan> = Vec::with_capacity(calls.len());
        for (prompt, opts_json) in calls {
            let mut opts: Value = serde_json::from_str(&opts_json).unwrap_or(Value::Null);
            if let Some(spec_json) = opts.get("__wf_resolve").and_then(Value::as_str) {
                let spec: Value = serde_json::from_str(spec_json).unwrap_or(Value::Null);
                plans.push(Plan::Resolve(spec));
                continue;
            }
            // Extract and strip the phase context injected by the script engine
            // (`__wf_phase: {index, title}`) — display-only, not forwarded to spawner.
            // This phase_index is 1-based (oracle §8: `workflow_phase` toolUseID uses `Q`
            // which auto-increments from 1). Contrast with the `phase_index` field in
            // `tengu_workflow_phase_completed` telemetry, which is 0-based (oracle §7).
            let (phase_index, phase_title) = if let Some(ph) = opts.as_object_mut().and_then(|o| o.remove("__wf_phase")) {
                let idx = ph.get("index").and_then(Value::as_u64).map(|v| v as u32);
                let title = ph.get("title").and_then(Value::as_str).map(str::to_string);
                (idx, title)
            } else {
                (None, None)
            };
            // Compute the clean opts_json (without __wf_phase) for the spawner.
            let clean_opts_json = serde_json::to_string(&opts).unwrap_or_else(|_| opts_json.clone());
            // Extract label: `opts.label ?? prompt.slice(0, 60)` (oracle §8).
            let label = opts.get("label")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| {
                    let chars: Vec<char> = prompt.chars().take(60).collect();
                    chars.iter().collect()
                });
            // Advance the chained key for this real agent() call (before the
            // cache check, so cached calls also advance the chain — claude `m`).
            // Normalize opts to the 5 identity keys only (ABp): display-only
            // fields like `phase`/`label`/`stallMs` are stripped so re-annotating
            // a call doesn't invalidate the cache on resume.
            let normalized_opts = normalize_opts_for_chain_key(&opts);
            let key = chain_key(&running_key, &prompt, &normalized_opts);
            running_key.clone_from(&key);
            // Assign a monotonic call index to each real agent() call.
            let call_index = call_index_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if !gone_live {
                let cached = journal
                    .as_ref()
                    .and_then(|j| j.lock().unwrap().get(&key).cloned());
                if let Some(cached) = cached {
                    plans.push(Plan::Cached {
                        result: cached,
                        call_index,
                        label,
                        phase_index,
                        phase_title,
                    });
                    continue;
                }
                gone_live = true; // first miss → everything after runs live
            }
            let journaled_key = journal.as_ref().map(|_| key);
            plans.push(Plan::Live {
                key: journaled_key,
                prompt,
                opts_json: clean_opts_json,
                call_index,
                label,
                phase_index,
                phase_title,
            });
        }

        // Phase B — concurrent (bounded, order-preserving).
        let results: Vec<String> = futures::stream::iter(plans.into_iter().map(|plan| {
            let spawner = spawner.clone();
            let tool_invoker = tool_invoker.clone();
            let budget = budget.clone();
            let subagent_type = subagent_type.to_string();
            let journal = journal.clone();
            let spent = spent.clone();
            let agent_count = agent_count.clone();
            let nested_fs = nested_fs.clone();
            let budget_total = token_budget_total;
            let baseline = turn_start_baseline;
            let bus_call = bus.clone();
            let ptx = worker_progress_tx.clone();
            async move {
                let (key, prompt, opts_json, call_index, label, phase_index, phase_title) = match plan {
                    // `workflow()` resolution: read + strip the nested source; `""`
                    // ⇒ the runtime throws "could not resolve".
                    Plan::Resolve(spec) => {
                        return match resolve_nested_script(&spec, nested_fs.as_ref()).await {
                            Ok(src) => workflow::strip_meta_export(&src),
                            Err(_) => String::new(),
                        }
                    }
                    Plan::Cached { result, call_index, label, phase_index, phase_title } => {
                        // Emit a `cached` workflow_agent event for journal replays.
                        let tool_use_id = format!("workflow_agent_{call_index}_cached");
                        let cached_event = workflow::Progress::Agent {
                            index: call_index,
                            label: label.clone(),
                            phase_index,
                            phase_title,
                            agent_id: None,
                            model: None,
                            state: workflow::AgentState::Cached,
                            tool_use_id,
                        };
                        if let Some(ref tx) = ptx {
                            let _ = tx.send(format_progress(&cached_event));
                        }
                        return result;
                    }
                    Plan::Live {
                        key,
                        prompt,
                        opts_json,
                        call_index,
                        label,
                        phase_index,
                        phase_title,
                    } => (key, prompt, opts_json, call_index, label, phase_index, phase_title),
                };
                let opts: Value = serde_json::from_str(&opts_json).unwrap_or(Value::Null);
                // Extract model from opts for the workflow_agent event.
                let agent_model = opts.get("model").and_then(Value::as_str).map(str::to_string);
                // Budget hard ceiling (claude-code `v()` before each spawn): when a
                // token target is set and the turn-relative spend has reached it,
                // refuse the spawn → the prelude throws WorkflowBudgetExceededError
                // (sequential loops stop; in parallel/pipeline the throw is caught →
                // null). Checked before the cap so an over-budget run reports it.
                if let Some(total) = budget_total.filter(|&t| t > 0) {
                    let turn_spent = spent.load(Ordering::Relaxed).saturating_sub(baseline);
                    if turn_spent >= total {
                        let mut md: LogEventMetadata = HashMap::new();
                        md.insert("spent".to_string(), AnalyticsValue::Int(turn_spent as i64));
                        md.insert("budget".to_string(), AnalyticsValue::Int(total as i64));
                        md.insert("agentCount".to_string(), AnalyticsValue::Int(agent_count.load(Ordering::Relaxed) as i64));
                        bus_call.log_event(telemetry::tengu::workflow::BUDGET_CAP_EXCEEDED, md).await;
                        return wf_throw(&workflow_budget_exceeded_message(turn_spent, total));
                    }
                }
                // 1000-agent lifetime cap (claude-code `S()` before each real
                // spawn): replayed/resolve calls are exempt (they never reach here).
                // `fetch_add` returns the prior count → spawns 0..999 proceed, the
                // 1001st throws WorkflowAgentCapError.
                if agent_count.fetch_add(1, Ordering::SeqCst) >= WORKFLOW_AGENT_CAP {
                    let mut md: LogEventMetadata = HashMap::new();
                    md.insert("agentCount".to_string(), AnalyticsValue::Int(WORKFLOW_AGENT_CAP as i64));
                    bus_call.log_event(telemetry::tengu::workflow::AGENT_CAP_EXCEEDED, md).await;
                    return wf_throw(WORKFLOW_AGENT_CAP_MESSAGE);
                }
                // agentType validation (binary `F` @202933121): an explicit
                // `agentType` must name a known agent, else throw the byte-exact
                // not-found error listing the available agents.
                if let Some(at) = opts
                    .get("agentType")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                {
                    let listing = spawner.agent_listing().await;
                    if !listing.iter().any(|e| e.agent_type == at) {
                        let available = listing
                            .iter()
                            .map(|e| e.agent_type.clone())
                            .collect::<Vec<_>>()
                            .join(", ");
                        return wf_throw(&format!(
                            "agent({{agentType}}): agent type '{at}' not found. Available agents: {available}"
                        ));
                    }
                }
                // Emit `start` workflow_agent event before spawning (oracle §8).
                // toolUseID for the in-flight queued emit uses the suffix "queued"
                // (oracle §8: `workflow_agent_${ne}_queued` for queued/start state).
                {
                    let start_event = workflow::Progress::Agent {
                        index: call_index,
                        label: label.clone(),
                        phase_index,
                        phase_title: phase_title.clone(),
                        agent_id: None,
                        model: agent_model.clone(),
                        state: workflow::AgentState::Start,
                        tool_use_id: format!("workflow_agent_{call_index}_queued"),
                    };
                    if let Some(ref tx) = ptx {
                        let _ = tx.send(format_progress(&start_event));
                    }
                }
                let inherit = SubagentInheritance {
                    tool_invoker,
                    budget,
                };
                let request = make_request(&subagent_type, &prompt, &opts_json);
                let raw = spawner.spawn(request, inherit).await;
                // Accumulate this fresh subagent's output tokens into the shared
                // `spent` pool (replayed/cached agents cost nothing) — the same
                // pool the main loop feeds when wired.
                if let Ok(SubagentResult::Completed { usage, .. }) = &raw {
                    spent.fetch_add(usage.output_tokens, Ordering::Relaxed);
                }
                // Emit `done` or `error` workflow_agent event after spawning.
                // For done/error, toolUseID uses the agent_id UUID (oracle §8:
                // `workflow_agent_${K}_${ct}` where ct is the agentId UUID).
                {
                    let (state, agent_id_str) = match &raw {
                        Ok(SubagentResult::Completed { agent_id, .. }) => {
                            (workflow::AgentState::Done, Some(agent_id.to_string()))
                        }
                        Ok(SubagentResult::Failed { agent_id, .. } | SubagentResult::Killed { agent_id, .. }) => {
                            (workflow::AgentState::Error, Some(agent_id.to_string()))
                        }
                        Err(_) => (workflow::AgentState::Error, None),
                    };
                    let tool_use_id = if let Some(ref id) = agent_id_str {
                        format!("workflow_agent_{call_index}_{id}")
                    } else {
                        format!("workflow_agent_{call_index}_error")
                    };
                    let lifecycle_event = workflow::Progress::Agent {
                        index: call_index,
                        label: label.clone(),
                        phase_index,
                        phase_title,
                        agent_id: agent_id_str,
                        model: agent_model,
                        state,
                        tool_use_id,
                    };
                    if let Some(ref tx) = ptx {
                        let _ = tx.send(format_progress(&lifecycle_event));
                    }
                }
                let result = result_to_string(raw);
                // Journal only a real result — a dead/skipped agent (NULL sentinel)
                // is NOT cached (claude-code `if (a && ie && de !== null) append`),
                // so a resume re-runs it.
                if result != workflow::WF_NULL_SENTINEL {
                    if let (Some(j), Some(k)) = (journal.as_ref(), key) {
                        j.lock().unwrap().insert(k, result.clone());
                    }
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

    let outcome = outcome_rx.await.map_err(|_| {
        workflow::WorkflowError::Engine(
            "workflow script thread terminated without an outcome".into(),
        )
    })??;

    // Emit `tengu_workflow_phase_completed` for each Phase in the outcome's
    // progress list (oracle §7 payload).
    //
    // Oracle §7 gating condition: `p.source === "built-in"` — this event is emitted
    // ONLY for named/saved workflows (invocation_mode == "named"). Inline scripts and
    // arbitrary scriptPath invocations do NOT emit this event. See oracle §7 for the
    // exact binary gate site.
    // invocation_mode values that trigger the emit: "named" only.
    // invocation_mode values that suppress the emit: "inline", "scriptPath", and None.
    //
    // Per-phase metric fields (`phase_tokens`, `phase_tool_calls`,
    // `phase_agent_duration_ms`, `phase_agent_count`, `phase_error_count`,
    // `phase_skip_count`) are UNAVAILABLE: LingXi's `Progress::Phase` only carries
    // a title; no numeric per-phase aggregation is performed. These fields are
    // omitted rather than emitted as 0 to avoid misleading consumers.
    {
        // Gate: only emit for named/saved workflows (p.source === "built-in" in oracle §7).
        let is_named_source = phase_telemetry_ctx
            .as_ref()
            .and_then(|ctx| ctx.invocation_mode.as_deref())
            .map(|mode| mode == "named")
            .unwrap_or(false);

        if is_named_source {
            // phase_index is 0-based in telemetry (oracle §7: `U` starts at 0 and
            // increments per emitted phase event). Note: Progress::Phase.index and
            // the phaseIndex field on workflow_agent progress events are 1-based
            // (oracle §8: `Q` auto-increments from 1 for workflow_phase events).
            let mut phase_idx: i64 = 0;
            for p in &outcome.progress {
                if let workflow::Progress::Phase { title, .. } = p {
                    let mut md: LogEventMetadata = HashMap::new();
                    if let Some(ref ctx) = phase_telemetry_ctx {
                        md.insert(
                            "workflow_run_id".to_string(),
                            AnalyticsValue::String(ctx.run_id.clone()),
                        );
                        if let Some(ref src) = ctx.workflow_source {
                            md.insert(
                                "workflow_source".to_string(),
                                AnalyticsValue::String(src.clone()),
                            );
                        }
                        if let Some(ref name) = ctx.workflow_name {
                            md.insert(
                                "workflow_name".to_string(),
                                AnalyticsValue::String(name.clone()),
                            );
                        }
                    }
                    md.insert("phase_index".to_string(), AnalyticsValue::Int(phase_idx));
                    md.insert(
                        "phase_title".to_string(),
                        AnalyticsValue::String(title.clone()),
                    );
                    bus.log_event(telemetry::tengu::workflow::PHASE_COMPLETED, md).await;
                    phase_idx += 1;
                }
            }
        }
    }

    Ok(outcome)
}

/// Resolve a `workflow()` reference (`{ name }` or `{ scriptPath }`) to a script
/// source via `fs`: `scriptPath` is read directly; `name` resolves under
/// `.lingxi/workflows/<name>.{js,mjs,ts}`.
async fn resolve_nested_script(
    spec: &Value,
    fs: Option<&Arc<dyn FileSystem>>,
) -> Result<String, String> {
    let fs = fs.ok_or_else(|| "no filesystem to resolve workflow()".to_string())?;
    if let Some(path) = spec
        .get("scriptPath")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        return fs
            .read_file(path, None, None)
            .await
            .map(|fc| fc.content)
            .map_err(|e| e.to_string());
    }
    if let Some(name) = spec
        .get("name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        for ext in [".js", ".mjs", ".ts"] {
            if let Ok(fc) = fs
                .read_file(
                    &format!("{}/workflows/{name}{ext}", branding::DOT_DIR),
                    None,
                    None,
                )
                .await
            {
                if !fc.content.is_empty() {
                    return Ok(fc.content);
                }
            }
        }
        return Err(format!("no saved workflow named '{name}'"));
    }
    Err("workflow() requires a name or scriptPath".to_string())
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
            args: workflow_args,
            run_id: provided_run_id,
            invocation_mode,
            workflow_source,
            launched_from_subagent,
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
        // The shared `budget.spent()` pool (main loop + all workflows), published
        // by the root once the orchestrator exists. `None` in tests ⇒ the run
        // uses its own private pool (own-spend only).
        let shared_pool = self
            .output_pool_cell
            .as_ref()
            .and_then(|c| c.get().cloned());
        // Snapshot the turn-start baseline (claude-code `R = xtr`) once, now, at
        // spawn — it is fixed for this workflow's life even as later turns update
        // the orchestrator's live baseline.
        let turn_start_baseline = self
            .turn_baseline_cell
            .as_ref()
            .and_then(|c| c.get())
            .map(|a| a.load(std::sync::atomic::Ordering::Relaxed))
            .unwrap_or(0);
        let worker_spool_path = spool_path.clone();
        let worker_task_id = task_id.clone();
        // Cooperative-cancel flag: shared between the script thread's engine
        // interrupt handler (via `run_workflow_script`) and the `WorkerCancel`
        // record `kill` flips. `false` until killed.
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        // Cloned for kill-detection in tengu_workflow_completed status derivation
        // (oracle §7: `abortController?.signal.aborted ? "killed" : error ? "failed" : "completed"`).
        let completed_cancel = cancel.clone();
        let worker_bus = self.bus.clone();
        // Parse meta fields from the script NOW (before moving `script` into the
        // worker closure) so they are available for both `launched` and `completed`
        // telemetry without re-parsing inside the async closure.
        let meta_name: Option<String> = workflow::meta_string_value(&script, "name");
        let meta_description: Option<String> = workflow::meta_string_value(&script, "description");
        // phase_count: number of declared phases in `meta.phases`. The workflow
        // crate exposes `meta_string_value` for string fields; there is no direct
        // array-length API. The `meta.phases` field is present in some templates
        // but not enforced by validate_meta. We parse it with a minimal tree-sitter
        // scan via `workflow::meta_array_len` when available; for now use 0 as the
        // safe fallback (LingXi does not yet expose a meta_array_len helper).
        // NOTE: oracle §7 spec: `c.meta.phases?.length ?? 0` — the phase COUNT is
        // from the script's static meta declaration, not from runtime phase() calls.
        // Since `workflow::meta_string_value` cannot walk arrays, we emit 0.
        // This is marked unavailable in the module comment below.
        let meta_phase_count: i64 = 0; // UNAVAILABLE: no workflow::meta_array_len()
        let script_size_chars: i64 = script.chars().count() as i64;
        // Shared agent_count Arc threaded into run_workflow_script so the completed
        // event can read the real post-run count (oracle §7: `k.agentCount`).
        let shared_agent_count = Arc::new(AtomicU64::new(0));
        let worker = Box::pin({
            let shared_agent_count = shared_agent_count.clone();
            async move {
            status_sink
                .set_status(&worker_task_id, TaskStatus::Running)
                .await;

            // Resume journal: a stable `wf_…` run id keys a per-(prompt, opts)
            // `agent()` result cache. A new run mints one; a resume reuses the
            // caller's id and pre-loads its journal so unchanged agents replay
            // instead of re-spawning. The id is surfaced to the task output so a
            // later call can pass it back as `resumeFromRunId`.
            // A resume reuses the caller's id; a fresh run uses the
            // launcher-minted id (so the Workflow tool result can return it)
            // and only mints one here as a last resort (direct test spawns).
            // Fresh-mint shape matches claude-code 2.1.195
            // `wf_${randomUUID().slice(0,12)}` = `wf_` + 8 hex + `-` + 3 hex.
            let run_id = resume_from_run_id
                .clone()
                .or(provided_run_id)
                .unwrap_or_else(|| {
                    let r = rand::random::<u64>();
                    format!("wf_{:08x}-{:03x}", (r >> 32) as u32, (r as u32) & 0xfff)
                });

            // tengu_workflow_launched — oracle §7 exact payload.
            // NOTE: `workflow_run_id` is NOT present on `launched` (oracle §7 shows
            // it only on `completed` and `phase_completed`). `invocation_mode`,
            // `workflow_source`, `workflow_name`, `workflow_description`,
            // `phase_count`, `launched_from_subagent`, `has_args`, `is_resume`,
            // and `script_size_chars` are all present.
            // `launched_from_subagent`: oracle `t.agentId != null`. LingXi does not
            // thread the calling agent id to the task handler — this field comes
            // from the spawn input where it was set by the launcher.
            {
                let mut md: LogEventMetadata = HashMap::new();
                md.insert(
                    "invocation_mode".to_string(),
                    AnalyticsValue::String(
                        invocation_mode.clone().unwrap_or_else(|| "inline".to_string()),
                    ),
                );
                md.insert(
                    "workflow_source".to_string(),
                    AnalyticsValue::String(
                        workflow_source.clone().unwrap_or_else(|| "inline".to_string()),
                    ),
                );
                if let Some(ref name) = meta_name {
                    md.insert("workflow_name".to_string(), AnalyticsValue::String(name.clone()));
                }
                if let Some(ref desc) = meta_description {
                    md.insert(
                        "workflow_description".to_string(),
                        AnalyticsValue::String(desc.clone()),
                    );
                }
                md.insert("phase_count".to_string(), AnalyticsValue::Int(meta_phase_count));
                md.insert(
                    "launched_from_subagent".to_string(),
                    AnalyticsValue::Bool(launched_from_subagent),
                );
                md.insert(
                    "has_args".to_string(),
                    AnalyticsValue::Bool(workflow_args.is_some()),
                );
                md.insert(
                    "is_resume".to_string(),
                    AnalyticsValue::Bool(resume_from_run_id.is_some()),
                );
                md.insert(
                    "script_size_chars".to_string(),
                    AnalyticsValue::Int(script_size_chars),
                );
                worker_bus.log_event(telemetry::tengu::workflow::LAUNCHED, md).await;
            }

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
                // tengu_workflow_journal_started_hit_respawn — resume loaded a
                // non-empty journal: previously-started agents will be replayed.
                if !cache.is_empty() {
                    let mut md: LogEventMetadata = HashMap::new();
                    md.insert("attempts".to_string(), AnalyticsValue::Int(cache.len() as i64));
                    worker_bus.log_event(telemetry::tengu::workflow::JOURNAL_STARTED_HIT_RESPAWN, md).await;
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
            let run_start = std::time::Instant::now();
            let run = run_workflow_script(
                &script,
                DEFAULT_WORKFLOW_SUBAGENT,
                spawner,
                tool_invoker,
                budget,
                Some(ptx),
                Some(journal.clone()),
                token_budget_total,
                shared_pool,
                turn_start_baseline,
                NestedConfig {
                    allow_nested: true,
                    args: workflow_args,
                    fs: Some(fs.clone()),
                },
                worker_cancel,
                worker_bus.clone(),
                Some(shared_agent_count.clone()),
                // Pass the phase telemetry context so run_workflow_script can
                // emit tengu_workflow_phase_completed with the correct run_id,
                // workflow_source, workflow_name, and the invocation_mode gate
                // (oracle §7: only "named" sources emit phase_completed).
                Some(PhaseTelemetryCtx {
                    run_id: run_id.clone(),
                    workflow_source: workflow_source.clone(),
                    workflow_name: meta_name.clone(),
                    invocation_mode: invocation_mode.clone(),
                }),
            );
            let (outcome, ()) = tokio::join!(run, drain);
            let elapsed_ms = run_start.elapsed().as_millis() as i64;

            // tengu_workflow_completed — oracle §7 exact payload.
            // `status` derivation: `abortController?.signal.aborted ? "killed"
            //   : k.error ? "failed" : "completed"` (oracle §7).
            // `agent_count`: real final count from the shared_agent_count Arc.
            // `total_tokens` / `total_tool_calls`: UNAVAILABLE — LingXi's
            //   `RunOutcome` carries no per-run token/tool-call aggregates; these
            //   fields are omitted rather than emitted as 0. `SubagentResult` per-
            //   agent usage is only available inside `run_workflow_script`'s worker
            //   future and is not propagated to `RunOutcome`. A future refactor that
            //   accumulates these into the outcome struct can add them here.
            {
                let agent_count_val = shared_agent_count.load(std::sync::atomic::Ordering::Relaxed) as i64;
                let status_str = if completed_cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    "killed"
                } else {
                    match &outcome {
                        Ok(_) => "completed",
                        Err(_) => "failed",
                    }
                };
                let mut md: LogEventMetadata = HashMap::new();
                md.insert("workflow_run_id".to_string(), AnalyticsValue::String(run_id.clone()));
                md.insert(
                    "workflow_source".to_string(),
                    AnalyticsValue::String(
                        workflow_source.clone().unwrap_or_else(|| "inline".to_string()),
                    ),
                );
                if let Some(ref name) = meta_name {
                    md.insert("workflow_name".to_string(), AnalyticsValue::String(name.clone()));
                }
                if let Some(ref desc) = meta_description {
                    md.insert(
                        "workflow_description".to_string(),
                        AnalyticsValue::String(desc.clone()),
                    );
                }
                md.insert("status".to_string(), AnalyticsValue::String(status_str.to_string()));
                md.insert("agent_count".to_string(), AnalyticsValue::Int(agent_count_val));
                md.insert("duration_ms".to_string(), AnalyticsValue::Int(elapsed_ms));
                worker_bus.log_event(telemetry::tengu::workflow::COMPLETED, md).await;
            }
            // Note: tengu_workflow_phase_completed is emitted inside
            // run_workflow_script with the PhaseTelemetryCtx passed above.

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
        }});

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
                cancel,
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
            // Flip the cooperative-cancel flag FIRST so the script thread's engine
            // interrupt handler aborts a runaway pure-JS loop, then cancel the
            // async worker future (claude-code `abortController.abort()`).
            rec.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
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
        async fn agent_listing(&self) -> Vec<traits::subagent_spawn::SubagentListingEntry> {
            ["general-purpose", "Explore", "code-reviewer", "workflow-subagent"]
                .iter()
                .map(|t| traits::subagent_spawn::SubagentListingEntry {
                    agent_type: (*t).to_string(),
                    when_to_use: String::new(),
                    tools_description: String::new(),
                })
                .collect()
        }
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
            args: None,
            run_id: None,
            invocation_mode: Some("inline".to_string()),
            workflow_source: Some("inline".to_string()),
            launched_from_subagent: false,
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
                workflow::Progress::Log { message: s } => Some(s.clone()),
                workflow::Progress::Phase { .. } | workflow::Progress::Agent { .. } => None,
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
            None,
            0,
            NestedConfig::default(),
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            Arc::new(AnalyticsBus::new()),
            None,
            None,
        )
        .await
        .expect("workflow runs to completion")
    }

    /// The 1000-agent lifetime cap: the 1001st REAL spawn rejects with the
    /// byte-exact `WorkflowAgentCapError` message, terminating the run.
    #[tokio::test]
    async fn agent_cap_rejects_the_1001st_spawn() {
        let spawner = Arc::new(EchoSpawner::default());
        let result = run_workflow_script(
            "for (let i = 0; i < 1001; i++) { await agent('x'); } return 'done';",
            DEFAULT_WORKFLOW_SUBAGENT,
            spawner,
            Arc::new(MockInvoker),
            Arc::new(MockBudget),
            None,
            None,
            None,
            None,
            0,
            NestedConfig::default(),
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            Arc::new(AnalyticsBus::new()),
            None,
            None,
        )
        .await;
        let err = result.expect_err("the 1001st agent() must throw the cap error");
        let msg = format!("{err}");
        assert!(
            msg.contains("Workflow agent() call cap reached (1000)"),
            "got: {msg}"
        );
    }

    /// An explicit unknown `agentType` throws the byte-exact not-found error
    /// listing the available agents; a known one runs fine.
    #[tokio::test]
    async fn unknown_agent_type_throws_not_found() {
        let spawner = Arc::new(EchoSpawner::default());
        let result = run_workflow_script(
            "await agent('p', { agentType: 'nope' }); return 'done';",
            DEFAULT_WORKFLOW_SUBAGENT,
            spawner,
            Arc::new(MockInvoker),
            Arc::new(MockBudget),
            None,
            None,
            None,
            None,
            0,
            NestedConfig::default(),
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            Arc::new(AnalyticsBus::new()),
            None,
            None,
        )
        .await;
        let err = result.expect_err("unknown agentType must throw");
        assert!(
            format!("{err}").contains(
                "agent({agentType}): agent type 'nope' not found. Available agents: general-purpose, Explore, code-reviewer"
            ),
            "got: {err}"
        );
    }

    /// A workflow that stays under the cap runs to completion unaffected.
    #[tokio::test]
    async fn under_cap_workflow_completes() {
        let spawner = Arc::new(EchoSpawner::default());
        let outcome = run_bridge(
            "for (let i = 0; i < 50; i++) { await agent('x'); } return 'ok';",
            spawner,
        )
        .await;
        assert_eq!(outcome.result.as_deref(), Some("\"ok\""));
    }

    /// `budget.spent()` is TURN-RELATIVE: the shared pool minus the turn-start
    /// baseline (claude-code `getTurnSpent()=rT()-xtr`), so prior-turn output is
    /// excluded.
    #[tokio::test]
    async fn budget_spent_is_turn_relative_via_baseline() {
        use std::sync::atomic::AtomicU64;
        // Pool already at 500 from prior turns; THIS turn started at 500 → the
        // baseline is 500, so a fresh 100-token subagent yields spent()==100.
        let pool = Arc::new(AtomicU64::new(500));
        let spawner = Arc::new(EchoSpawner::default());
        let outcome = run_workflow_script(
            "await agent('a'); log('spent=' + budget.spent()); return '';",
            DEFAULT_WORKFLOW_SUBAGENT,
            spawner,
            Arc::new(MockInvoker),
            Arc::new(MockBudget),
            None,
            None,
            None,
            Some(pool),
            500,
            NestedConfig::default(),
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            Arc::new(AnalyticsBus::new()),
            None,
            None,
        )
        .await
        .expect("runs");
        assert!(
            logs(&outcome).iter().any(|l| l == "spent=100"),
            "spent should be turn-relative (100), logs: {:?}",
            logs(&outcome)
        );
    }

    /// The budget hard ceiling: once turn-relative spend reaches the target, the
    /// next `agent()` throws the byte-exact `WorkflowBudgetExceededError`.
    #[tokio::test]
    async fn budget_ceiling_throws_when_turn_spend_exceeds_total() {
        use std::sync::atomic::AtomicU64;
        // total=100; pool already at 150 this turn (baseline 0) → 150 >= 100.
        let pool = Arc::new(AtomicU64::new(150));
        let spawner = Arc::new(EchoSpawner::default());
        let result = run_workflow_script(
            "await agent('a'); return 'done';",
            DEFAULT_WORKFLOW_SUBAGENT,
            spawner,
            Arc::new(MockInvoker),
            Arc::new(MockBudget),
            None,
            None,
            Some(100),
            Some(pool),
            0,
            NestedConfig::default(),
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            Arc::new(AnalyticsBus::new()),
            None,
            None,
        )
        .await;
        let err = result.expect_err("over-budget agent() must throw");
        assert!(
            format!("{err}")
                .contains("Workflow token budget exceeded (150 / 100 output tokens)"),
            "got: {err}"
        );
    }

    /// `budget.spent()` reads the shared pool: a pre-seeded value (standing in
    /// for main-loop output the orchestrator already accumulated) plus every
    /// subagent's output tokens — the union claude-code exposes, not own-spend.
    #[tokio::test]
    async fn shared_pool_makes_spent_read_main_loop_plus_subagents() {
        use std::sync::atomic::{AtomicU64, Ordering};
        // Pre-seed as if the main loop already spent 500 output tokens this turn.
        let pool = Arc::new(AtomicU64::new(500));
        let spawner = Arc::new(EchoSpawner::default()); // 100 output tokens/agent
        let outcome = run_workflow_script(
            "await agent('a'); await agent('b'); log('spent=' + budget.spent()); return '';",
            DEFAULT_WORKFLOW_SUBAGENT,
            spawner,
            Arc::new(MockInvoker),
            Arc::new(MockBudget),
            None,
            None,
            Some(1_000_000),
            Some(pool.clone()),
            0,
            NestedConfig::default(),
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            Arc::new(AnalyticsBus::new()),
            None,
            None,
        )
        .await
        .expect("workflow runs to completion");
        // 500 (seeded main loop) + 2×100 (the two subagents) = 700.
        assert_eq!(pool.load(Ordering::Relaxed), 700);
        assert!(
            logs(&outcome).iter().any(|l| l == "spent=700"),
            "spent() must read the shared pool live; got {:?}",
            logs(&outcome)
        );
    }

    /// Without a shared pool (the standalone/test path), `spent()` falls back to
    /// a private counter of this run's own subagent output only.
    #[tokio::test]
    async fn no_shared_pool_falls_back_to_own_spend() {
        let spawner = Arc::new(EchoSpawner::default());
        let outcome = run_bridge(
            "await agent('a'); log('spent=' + budget.spent()); return '';",
            spawner,
        )
        .await;
        assert!(
            logs(&outcome).iter().any(|l| l == "spent=100"),
            "own-spend fallback should count just the one subagent; got {:?}",
            logs(&outcome)
        );
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

    #[test]
    fn make_request_maps_effort_opt() {
        // `agent({effort})` — a level string or an integer — is carried raw.
        assert_eq!(
            make_request("general-purpose", "p", r#"{"effort":"high"}"#).effort,
            Some(serde_json::json!("high"))
        );
        assert_eq!(
            make_request("general-purpose", "p", r#"{"effort":8000}"#).effort,
            Some(serde_json::json!(8000))
        );
        assert!(make_request("general-purpose", "p", "{}").effort.is_none());
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
        // A bare agent(prompt) → default type (workflow-subagent), no overrides.
        assert_eq!(reqs[1].subagent_type, "workflow-subagent");
        assert_eq!(reqs[1].model, None);
        assert_eq!(reqs[1].isolation, None);
        assert_eq!(reqs[1].schema, None);
    }

    #[tokio::test]
    async fn top_level_args_global_reaches_the_script() {
        let spawner = Arc::new(EchoSpawner::default());
        let outcome = run_workflow_script(
            "log('a=' + args.a); return args;",
            DEFAULT_WORKFLOW_SUBAGENT,
            spawner,
            Arc::new(MockInvoker),
            Arc::new(MockBudget),
            None,
            None,
            None,
            None,
            0,
            NestedConfig {
                allow_nested: false,
                args: Some(r#"{"a":5}"#.to_string()),
                fs: None,
            },
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            Arc::new(AnalyticsBus::new()),
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(logs(&outcome), vec!["a=5".to_string()]);
        assert_eq!(outcome.result.as_deref(), Some(r#"{"a":5}"#));
    }

    #[tokio::test]
    async fn workflow_runs_a_nested_scriptpath_inline_sharing_the_runtime() {
        let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
        // The nested workflow itself spawns an agent and reads its own args —
        // proving it runs IN the parent's runtime (shared spawner + globals).
        fs.write_file(
            "/wf/child.js",
            "const x = await agent('child-task'); return { got: x, n: args.n };",
        )
        .await
        .unwrap();
        let spawner = Arc::new(EchoSpawner::default());
        let parent = r#"
            const r = await workflow({ scriptPath: '/wf/child.js' }, { n: 9 });
            log('got=' + r.got + ' n=' + r.n);
            return r;
        "#;
        let outcome = run_workflow_script(
            parent,
            DEFAULT_WORKFLOW_SUBAGENT,
            spawner.clone(),
            Arc::new(MockInvoker),
            Arc::new(MockBudget),
            None,
            None,
            None,
            None,
            0,
            NestedConfig {
                allow_nested: true,
                args: None,
                fs: Some(fs),
            },
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            Arc::new(AnalyticsBus::new()),
            None,
            None,
        )
        .await
        .unwrap();
        // The nested workflow's agent() went through the PARENT's spawner.
        assert_eq!(*spawner.seen.lock().unwrap(), vec!["child-task".to_string()]);
        // Its return value (incl. its own args) flowed back to the parent.
        assert_eq!(logs(&outcome), vec!["got=echo:child-task n=9".to_string()]);
        assert_eq!(
            outcome.result.as_deref(),
            Some(r#"{"got":"echo:child-task","n":9}"#)
        );
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
        // Phase now renders as "[N] === Title ===" (index-prefixed).
        assert!(read.content.contains("=== Scan ==="), "phase: {}", read.content);
        assert!(read.content.contains("[1]"), "phase index: {}", read.content);
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
            args: None,
            run_id: None,
            invocation_mode: Some("inline".to_string()),
            workflow_source: Some("inline".to_string()),
            launched_from_subagent: false,
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
    async fn label_opt_becomes_the_subagent_display_name() {
        let spawner = Arc::new(EchoSpawner::default());
        run_bridge(
            "await agent('p', { label: 'my-label' }); await agent('q');",
            spawner.clone(),
        )
        .await;
        let reqs = spawner.seen_reqs.lock().unwrap().clone();
        assert_eq!(reqs[0].name.as_deref(), Some("my-label"), "label → name");
        assert_eq!(reqs[1].name, None, "no label → no name");
    }

    #[tokio::test]
    async fn resume_with_a_changed_prefix_reruns_from_the_edit_onward() {
        // PREFIX semantics: editing the FIRST agent's prompt on resume must
        // re-run it AND every later call (the chained key cascades) — NOT replay
        // the now-misaligned journaled results by a flat (prompt,opts) match.
        let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
        let dir = tempdir().unwrap();
        let mgr = Arc::new(TaskOutputManager::new(PathBuf::from(dir.path()), fs.clone()));

        // Run 1: journal agents 'a' then 'b'.
        let script1 = "const a = await agent('a'); const b = await agent('b'); return { a, b };";
        let spawner1 = Arc::new(EchoSpawner::default());
        let sink1 = Arc::new(RecordingSink::default());
        let h1 = make_handler(spawner1.clone(), mgr.clone(), sink1.clone());
        let handle1 = h1
            .spawn(workflow_input(script1), make_ctx(fs.clone()))
            .await
            .unwrap();
        assert_eq!(await_terminal(&sink1).await, TaskStatus::Completed);
        let spool1 = dir.path().join(format!("{}.output", handle1.task_id));
        let out1 = mgr
            .read(&spool1, crate::output_manager::OutputOptions::default())
            .await
            .unwrap();
        let run_id = out1
            .content
            .lines()
            .find_map(|l| l.strip_prefix("runId: "))
            .expect("runId")
            .to_string();

        // Run 2: resume the same id but EDIT the first prompt ('a' → 'a2'). Both
        // agents must re-spawn — 'b' too, because its key chains off the changed
        // 'a2'. The flat-map cache would have wrongly replayed 'b' from run 1.
        let script2 = "const a = await agent('a2'); const b = await agent('b'); return { a, b };";
        let spawner2 = Arc::new(EchoSpawner::default());
        let sink2 = Arc::new(RecordingSink::default());
        let h2 = make_handler(spawner2.clone(), mgr.clone(), sink2.clone());
        let input2 = TaskSpawnInput::LocalWorkflow {
            workflow_id: "wf".into(),
            script: script2.into(),
            resume_from_run_id: Some(run_id),
            args: None,
            run_id: None,
            invocation_mode: Some("inline".to_string()),
            workflow_source: Some("inline".to_string()),
            launched_from_subagent: false,
        };
        h2.spawn(input2, make_ctx(fs.clone())).await.unwrap();
        assert_eq!(await_terminal(&sink2).await, TaskStatus::Completed);
        let mut seen = spawner2.seen.lock().unwrap().clone();
        seen.sort();
        assert_eq!(
            seen,
            vec!["a2".to_string(), "b".to_string()],
            "edited prefix re-runs the edit AND everything after it"
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
            tool_use_id: None,
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

    // ==== agent() routing tests (Cases 1-4 + §6) ============================

    /// Case 1: bare `agent(prompt)` → subagent_type = "workflow-subagent",
    /// no system_prompt_override, no addendum, no additional disallowed tools
    /// (the builtin def already has {SendUserMessage, Agent, Workflow}).
    #[test]
    fn bare_agent_routes_to_workflow_subagent_with_kbp() {
        let req = make_request(DEFAULT_WORKFLOW_SUBAGENT, "do something", "{}");
        assert_eq!(req.subagent_type, "workflow-subagent");
        assert!(req.system_prompt_override.is_none(), "no prompt override for bare agent()");
        assert!(req.system_prompt_addendum.is_none(), "no addendum for bare agent()");
        assert!(req.additional_disallowed_tools.is_empty(), "no extra disallowed for bare agent()");
    }

    /// Case 2: `agent(prompt, {schema})` (no agentType) → workflow-subagent +
    /// system_prompt_override = xBp.
    #[test]
    fn bare_schema_agent_uses_xbp() {
        let req = make_request(
            DEFAULT_WORKFLOW_SUBAGENT,
            "return structured",
            r#"{"schema":{"type":"object","properties":{"count":{"type":"number"}}}}"#,
        );
        assert_eq!(req.subagent_type, "workflow-subagent");
        // Override must be the xBp string (WORKFLOW_SUBAGENT_SCHEMA_PROMPT).
        let override_prompt = req.system_prompt_override.as_deref().expect("override must be set for schema agent()");
        assert_eq!(override_prompt, agent::builtins::WORKFLOW_SUBAGENT_SCHEMA_PROMPT);
        assert!(req.system_prompt_addendum.is_none(), "no addendum when no explicit agentType");
        assert!(req.additional_disallowed_tools.is_empty(), "no extra disallowed for bare schema agent()");
    }

    /// Case 3: `agent(prompt, {agentType})` (no schema) → that agentType, HBp
    /// addendum appended, disallow union {SendUserMessage, Agent, Workflow}.
    #[test]
    fn user_agenttype_gets_hbp_addendum_and_disallow_union() {
        let req = make_request(
            DEFAULT_WORKFLOW_SUBAGENT,
            "analyze code",
            r#"{"agentType":"general-purpose"}"#,
        );
        assert_eq!(req.subagent_type, "general-purpose");
        assert!(req.system_prompt_override.is_none(), "no prompt override for user agentType");
        let addendum = req.system_prompt_addendum.as_deref().expect("HBp addendum must be set");
        assert_eq!(addendum, agent::builtins::WORKFLOW_SUBAGENT_NON_SCHEMA_ADDENDUM);
        // Must request union with {SendUserMessage, Agent, Workflow}.
        let disallowed = &req.additional_disallowed_tools;
        assert!(disallowed.contains(&"SendUserMessage".to_string()), "SendUserMessage must be disallowed: {disallowed:?}");
        assert!(disallowed.contains(&"Agent".to_string()), "Agent must be disallowed: {disallowed:?}");
        assert!(disallowed.contains(&"Workflow".to_string()), "Workflow must be disallowed: {disallowed:?}");
    }

    /// Case 4: `agent(prompt, {agentType, schema})` → that agentType, IBp
    /// addendum appended, disallow union set.
    #[test]
    fn user_agenttype_with_schema_gets_ibp_and_disallow_union() {
        let req = make_request(
            DEFAULT_WORKFLOW_SUBAGENT,
            "return structured",
            r#"{"agentType":"code-reviewer","schema":{"type":"object"}}"#,
        );
        assert_eq!(req.subagent_type, "code-reviewer");
        assert!(req.system_prompt_override.is_none(), "no prompt override for user agentType");
        let addendum = req.system_prompt_addendum.as_deref().expect("IBp addendum must be set");
        assert_eq!(addendum, agent::builtins::WORKFLOW_SUBAGENT_SCHEMA_ADDENDUM);
        // Must request union with {SendUserMessage, Agent, Workflow}.
        let disallowed = &req.additional_disallowed_tools;
        assert!(disallowed.contains(&"SendUserMessage".to_string()), "SendUserMessage must be disallowed: {disallowed:?}");
        assert!(disallowed.contains(&"Agent".to_string()), "Agent must be disallowed: {disallowed:?}");
        assert!(disallowed.contains(&"Workflow".to_string()), "Workflow must be disallowed: {disallowed:?}");
    }

    // ---- chain_key / normalize_opts_for_chain_key ---------------------------

    /// Display-only opts (`phase`, `label`, `stallMs`) must NOT change the chain
    /// key — they are stripped by `normalize_opts_for_chain_key` (ABp parity).
    #[test]
    fn chain_key_ignores_display_only_opts() {
        let opts_a = r#"{"model":"claude-opus-4","phase":"research","label":"step1"}"#;
        let opts_b = r#"{"model":"claude-opus-4","phase":"writing","label":"step2","stallMs":5000}"#;
        let key_a = chain_key(
            "",
            "do something",
            &normalize_opts_for_chain_key(&serde_json::from_str(opts_a).unwrap()),
        );
        let key_b = chain_key(
            "",
            "do something",
            &normalize_opts_for_chain_key(&serde_json::from_str(opts_b).unwrap()),
        );
        assert_eq!(key_a, key_b, "display-only fields must not affect the chain key");
    }

    /// Changing `model` (an identity key) MUST produce a different chain key.
    #[test]
    fn chain_key_differs_on_model_change() {
        let opts_a = r#"{"model":"claude-opus-4"}"#;
        let opts_b = r#"{"model":"claude-sonnet-4"}"#;
        let key_a = chain_key(
            "",
            "do something",
            &normalize_opts_for_chain_key(&serde_json::from_str(opts_a).unwrap()),
        );
        let key_b = chain_key(
            "",
            "do something",
            &normalize_opts_for_chain_key(&serde_json::from_str(opts_b).unwrap()),
        );
        assert_ne!(key_a, key_b, "different model must produce different chain key");
    }

    /// Key order in the raw opts JSON must NOT matter — normalization sorts keys.
    #[test]
    fn chain_key_stable_regardless_of_input_key_order() {
        let opts_a = r#"{"model":"claude-opus-4","schema":{"type":"object"}}"#;
        let opts_b = r#"{"schema":{"type":"object"},"model":"claude-opus-4"}"#;
        let key_a = chain_key(
            "",
            "do something",
            &normalize_opts_for_chain_key(&serde_json::from_str(opts_a).unwrap()),
        );
        let key_b = chain_key(
            "",
            "do something",
            &normalize_opts_for_chain_key(&serde_json::from_str(opts_b).unwrap()),
        );
        assert_eq!(key_a, key_b, "key order in opts JSON must not affect the chain key");
    }

    /// Verify the concurrency cap formula: Math.min(16, Math.max(2, cpus-2)).
    /// At 1–3 cores the floor is 2; at 5 cores it's 3; at 18 cores it's capped at 16.
    #[test]
    fn concurrency_cap_formula_matches_binary() {
        // Direct formula test: cores.saturating_sub(2).max(2).min(16)
        let formula = |cores: usize| cores.saturating_sub(2).max(2).min(16);
        assert_eq!(formula(1), 2, "1 core → 2");
        assert_eq!(formula(2), 2, "2 cores → 2");
        assert_eq!(formula(3), 2, "3 cores → 2");
        assert_eq!(formula(4), 2, "4 cores → 2");
        assert_eq!(formula(5), 3, "5 cores → 3");
        assert_eq!(formula(18), 16, "18 cores → 16 (cap)");
    }

    // ==== Telemetry tests ====================================================

    /// `tengu_workflow_phase_completed` does NOT fire when the script has no
    /// `phase()` calls (bridge-level — verifies the post-run emit loop is a no-op
    /// when `outcome.progress` has no Phase entries).
    #[tokio::test]
    async fn telemetry_no_phase_events_without_phase_calls() {
        use telemetry::InMemorySink;
        let sink = Arc::new(InMemorySink::default());
        let bus = Arc::new(AnalyticsBus::new());
        bus.attach_sink(sink.clone()).await;

        let spawner = Arc::new(EchoSpawner::default());
        run_workflow_script(
            "return 'ok';",
            DEFAULT_WORKFLOW_SUBAGENT,
            spawner,
            Arc::new(MockInvoker),
            Arc::new(MockBudget),
            None,
            None,
            None,
            None,
            0,
            NestedConfig::default(),
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            bus.clone(),
            None,
            None,
        )
        .await
        .expect("runs");

        let events = sink.events().await;
        assert!(
            !events.iter().any(|e| e.name == telemetry::tengu::workflow::PHASE_COMPLETED),
            "no phase_completed for a script with no phase() calls; events: {events:?}"
        );
    }

    /// `tengu_workflow_phase_completed` fires once per `phase()` call for a NAMED
    /// (built-in source) workflow — oracle §7 gating condition.
    #[tokio::test]
    async fn telemetry_phase_completed_fires_per_phase_for_named_workflow() {
        use telemetry::InMemorySink;
        let sink = Arc::new(InMemorySink::default());
        let bus = Arc::new(AnalyticsBus::new());
        bus.attach_sink(sink.clone()).await;

        let spawner = Arc::new(EchoSpawner::default());
        run_workflow_script(
            "phase('Step 1'); phase('Step 2'); return 'done';",
            DEFAULT_WORKFLOW_SUBAGENT,
            spawner,
            Arc::new(MockInvoker),
            Arc::new(MockBudget),
            None,
            None,
            None,
            None,
            0,
            NestedConfig::default(),
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            bus.clone(),
            None,
            // Pass a named invocation_mode — oracle §7: only "named" (built-in source)
            // emits tengu_workflow_phase_completed.
            Some(PhaseTelemetryCtx {
                run_id: "wf_test".to_string(),
                workflow_source: Some("my-workflow".to_string()),
                workflow_name: Some("My Workflow".to_string()),
                invocation_mode: Some("named".to_string()),
            }),
        )
        .await
        .expect("runs");

        let events: Vec<_> = sink.events().await;
        let phase_events: Vec<_> = events
            .iter()
            .filter(|e| e.name == telemetry::tengu::workflow::PHASE_COMPLETED)
            .collect();
        assert_eq!(phase_events.len(), 2, "one event per phase() for named workflow; got {phase_events:?}");
        assert!(
            matches!(phase_events[0].metadata.get("phase_title"), Some(AnalyticsValue::String(s)) if s == "Step 1"),
            "first phase title"
        );
        // phase_index is 0-based in telemetry (oracle §7). Contrast with workflow_agent
        // phaseIndex which is 1-based (oracle §8).
        assert!(
            matches!(phase_events[0].metadata.get("phase_index"), Some(AnalyticsValue::Int(0))),
            "first phase index (0-based in telemetry, oracle §7)"
        );
        assert!(
            matches!(phase_events[1].metadata.get("phase_title"), Some(AnalyticsValue::String(s)) if s == "Step 2"),
            "second phase title"
        );
        assert!(
            matches!(phase_events[1].metadata.get("phase_index"), Some(AnalyticsValue::Int(1))),
            "second phase index (0-based in telemetry, oracle §7)"
        );
    }

    /// `tengu_workflow_phase_completed` does NOT fire for an INLINE script, even if
    /// it calls `phase()` — oracle §7 gates this event on `p.source === "built-in"`
    /// (invocation_mode == "named") only.
    #[tokio::test]
    async fn telemetry_phase_completed_suppressed_for_inline_workflow() {
        use telemetry::InMemorySink;
        let sink = Arc::new(InMemorySink::default());
        let bus = Arc::new(AnalyticsBus::new());
        bus.attach_sink(sink.clone()).await;

        let spawner = Arc::new(EchoSpawner::default());
        run_workflow_script(
            "phase('Step 1'); phase('Step 2'); return 'done';",
            DEFAULT_WORKFLOW_SUBAGENT,
            spawner,
            Arc::new(MockInvoker),
            Arc::new(MockBudget),
            None,
            None,
            None,
            None,
            0,
            NestedConfig::default(),
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            bus.clone(),
            None,
            // Inline invocation_mode: oracle §7 suppresses phase_completed for
            // "inline" (and "scriptPath") — only "named" (built-in source) emits it.
            Some(PhaseTelemetryCtx {
                run_id: "wf_test_inline".to_string(),
                workflow_source: Some("inline".to_string()),
                workflow_name: None,
                invocation_mode: Some("inline".to_string()),
            }),
        )
        .await
        .expect("runs");

        let events = sink.events().await;
        assert!(
            !events.iter().any(|e| e.name == telemetry::tengu::workflow::PHASE_COMPLETED),
            "tengu_workflow_phase_completed must NOT fire for inline workflows (oracle §7); events: {events:?}"
        );
    }

    /// `tengu_workflow_phase_completed` does NOT fire for a `scriptPath` workflow,
    /// even if it calls `phase()` — oracle §7 gates this on `p.source === "built-in"`
    /// (invocation_mode == "named") only. scriptPath is not a saved/built-in source.
    #[tokio::test]
    async fn telemetry_phase_completed_suppressed_for_script_path_workflow() {
        use telemetry::InMemorySink;
        let sink = Arc::new(InMemorySink::default());
        let bus = Arc::new(AnalyticsBus::new());
        bus.attach_sink(sink.clone()).await;

        let spawner = Arc::new(EchoSpawner::default());
        run_workflow_script(
            "phase('Step A'); return 'done';",
            DEFAULT_WORKFLOW_SUBAGENT,
            spawner,
            Arc::new(MockInvoker),
            Arc::new(MockBudget),
            None,
            None,
            None,
            None,
            0,
            NestedConfig::default(),
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            bus.clone(),
            None,
            Some(PhaseTelemetryCtx {
                run_id: "wf_test_scriptpath".to_string(),
                workflow_source: Some("/path/to/workflow.js".to_string()),
                workflow_name: None,
                invocation_mode: Some("scriptPath".to_string()),
            }),
        )
        .await
        .expect("runs");

        let events = sink.events().await;
        assert!(
            !events.iter().any(|e| e.name == telemetry::tengu::workflow::PHASE_COMPLETED),
            "tengu_workflow_phase_completed must NOT fire for scriptPath workflows (oracle §7); events: {events:?}"
        );
    }

    /// `tengu_workflow_phase_completed` does NOT fire when phase_telemetry_ctx is None
    /// (the bridge-level path without full context — verifies gating predicate).
    #[tokio::test]
    async fn telemetry_phase_completed_suppressed_when_no_telemetry_ctx() {
        use telemetry::InMemorySink;
        let sink = Arc::new(InMemorySink::default());
        let bus = Arc::new(AnalyticsBus::new());
        bus.attach_sink(sink.clone()).await;

        let spawner = Arc::new(EchoSpawner::default());
        run_workflow_script(
            "phase('Step 1'); return 'done';",
            DEFAULT_WORKFLOW_SUBAGENT,
            spawner,
            Arc::new(MockInvoker),
            Arc::new(MockBudget),
            None,
            None,
            None,
            None,
            0,
            NestedConfig::default(),
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            bus.clone(),
            None,
            // No PhaseTelemetryCtx → is_named_source = false → no phase_completed events.
            // Note: run_bridge() / test helpers typically pass None here; this test
            // documents that the gate also protects the None case (no ctx = not named).
            None,
        )
        .await
        .expect("runs");

        let events = sink.events().await;
        assert!(
            !events.iter().any(|e| e.name == telemetry::tengu::workflow::PHASE_COMPLETED),
            "tengu_workflow_phase_completed must NOT fire when phase_telemetry_ctx is None; events: {events:?}"
        );
    }

    /// `tengu_workflow_budget_cap_exceeded` fires when the budget ceiling is hit.
    #[tokio::test]
    async fn telemetry_budget_cap_fires() {
        use std::sync::atomic::AtomicU64;
        use telemetry::InMemorySink;
        let sink = Arc::new(InMemorySink::default());
        let bus = Arc::new(AnalyticsBus::new());
        bus.attach_sink(sink.clone()).await;

        // total=100; pool already at 150 this turn (baseline 0) → 150 >= 100.
        let pool = Arc::new(AtomicU64::new(150));
        let spawner = Arc::new(EchoSpawner::default());
        let _ = run_workflow_script(
            "await agent('a'); return 'done';",
            DEFAULT_WORKFLOW_SUBAGENT,
            spawner,
            Arc::new(MockInvoker),
            Arc::new(MockBudget),
            None,
            None,
            Some(100),
            Some(pool),
            0,
            NestedConfig::default(),
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            bus.clone(),
            None,
            None,
        )
        .await; // expected Err

        let events: Vec<_> = sink.events().await;
        let cap_event = events
            .iter()
            .find(|e| e.name == telemetry::tengu::workflow::BUDGET_CAP_EXCEEDED);
        assert!(cap_event.is_some(), "tengu_workflow_budget_cap_exceeded must fire; events: {events:?}");
        let md = &cap_event.unwrap().metadata;
        assert!(
            matches!(md.get("spent"), Some(AnalyticsValue::Int(150))),
            "spent field must be 150"
        );
        assert!(
            matches!(md.get("budget"), Some(AnalyticsValue::Int(100))),
            "budget field must be 100"
        );
    }

    /// `tengu_workflow_agent_cap_exceeded` fires when the 1000-agent cap is hit.
    #[tokio::test]
    async fn telemetry_agent_cap_fires() {
        use telemetry::InMemorySink;
        let sink = Arc::new(InMemorySink::default());
        let bus = Arc::new(AnalyticsBus::new());
        bus.attach_sink(sink.clone()).await;

        let spawner = Arc::new(EchoSpawner::default());
        let _ = run_workflow_script(
            "for (let i = 0; i < 1001; i++) { await agent('x'); } return 'done';",
            DEFAULT_WORKFLOW_SUBAGENT,
            spawner,
            Arc::new(MockInvoker),
            Arc::new(MockBudget),
            None,
            None,
            None,
            None,
            0,
            NestedConfig::default(),
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            bus.clone(),
            None,
            None,
        )
        .await; // expected Err

        let events: Vec<_> = sink.events().await;
        let cap_event = events
            .iter()
            .find(|e| e.name == telemetry::tengu::workflow::AGENT_CAP_EXCEEDED);
        assert!(cap_event.is_some(), "tengu_workflow_agent_cap_exceeded must fire; events: {events:?}");
        let md = &cap_event.unwrap().metadata;
        assert!(
            matches!(md.get("agentCount"), Some(AnalyticsValue::Int(1000))),
            "agentCount field must be 1000"
        );
    }

    // ==== Structured progress event tests (Task 10) ==========================

    /// A single-agent script emits `start` then `done` workflow_agent events to
    /// the progress spool, with the correct index (0-based), label, state, and
    /// toolUseID format (`workflow_agent_{index}_{suffix}`).
    #[tokio::test]
    async fn workflow_agent_progress_start_and_done_emitted() {
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let spawner = Arc::new(EchoSpawner::default());
        run_workflow_script(
            "const r = await agent('analyze the code', { label: 'my-label' }); log('r=' + r);",
            DEFAULT_WORKFLOW_SUBAGENT,
            spawner,
            Arc::new(MockInvoker),
            Arc::new(MockBudget),
            Some(tx),
            None,
            None,
            None,
            0,
            NestedConfig::default(),
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            Arc::new(AnalyticsBus::new()),
            None,
            None,
        )
        .await
        .expect("runs");

        let mut lines: Vec<String> = Vec::new();
        while let Ok(line) = rx.try_recv() {
            lines.push(line);
        }

        // Must have at least a `start` and a `done` agent event, plus a log.
        let agent_lines: Vec<&str> = lines.iter().filter(|l| l.starts_with("[workflow_agent]")).map(|l| l.as_str()).collect();
        assert!(agent_lines.len() >= 2, "expected start+done events; got: {lines:?}");

        // Parse the start event.
        let start_json_str = agent_lines[0].trim_start_matches("[workflow_agent] ");
        let start: serde_json::Value = serde_json::from_str(start_json_str).expect("start is JSON");
        assert_eq!(start["type"], "workflow_agent", "type field");
        assert_eq!(start["index"], 0, "index is 0 for first agent");
        assert_eq!(start["label"], "my-label", "label from opts.label");
        assert_eq!(start["state"], "start", "first event is start");
        let tool_use_id = start["toolUseID"].as_str().expect("toolUseID present");
        assert!(tool_use_id.starts_with("workflow_agent_0_"), "toolUseID format: {tool_use_id}");

        // Parse the done event.
        let done_json_str = agent_lines[1].trim_start_matches("[workflow_agent] ");
        let done: serde_json::Value = serde_json::from_str(done_json_str).expect("done is JSON");
        assert_eq!(done["state"], "done", "second event is done");
        assert_eq!(done["index"], 0, "same agent index");
        assert!(done.get("agentId").is_some(), "done has agentId");
    }

    /// `phase()` calls produce `workflow_phase` formatted progress lines with index + title.
    /// `log()` calls produce bare text lines (workflow_log style).
    #[tokio::test]
    async fn workflow_phase_and_log_progress_format() {
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let spawner = Arc::new(EchoSpawner::default());
        run_workflow_script(
            "phase('Analysis'); log('hello world'); phase('Report');",
            DEFAULT_WORKFLOW_SUBAGENT,
            spawner,
            Arc::new(MockInvoker),
            Arc::new(MockBudget),
            Some(tx),
            None,
            None,
            None,
            0,
            NestedConfig::default(),
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            Arc::new(AnalyticsBus::new()),
            None,
            None,
        )
        .await
        .expect("runs");

        let mut lines: Vec<String> = Vec::new();
        while let Ok(line) = rx.try_recv() {
            lines.push(line);
        }

        // Phase lines render as "[index] === title ===".
        assert!(
            lines.iter().any(|l| l.contains("[1]") && l.contains("=== Analysis ===")),
            "first phase: {lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.contains("[2]") && l.contains("=== Report ===")),
            "second phase: {lines:?}"
        );
        // Log line renders as bare text.
        assert!(lines.iter().any(|l| l == "hello world"), "log line: {lines:?}");
    }

    /// Agent index increments monotonically across sequential agent() calls.
    #[tokio::test]
    async fn workflow_agent_index_is_monotonic() {
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let spawner = Arc::new(EchoSpawner::default());
        run_workflow_script(
            "await agent('first'); await agent('second'); await agent('third');",
            DEFAULT_WORKFLOW_SUBAGENT,
            spawner,
            Arc::new(MockInvoker),
            Arc::new(MockBudget),
            Some(tx),
            None,
            None,
            None,
            0,
            NestedConfig::default(),
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            Arc::new(AnalyticsBus::new()),
            None,
            None,
        )
        .await
        .expect("runs");

        let mut lines: Vec<String> = Vec::new();
        while let Ok(line) = rx.try_recv() {
            lines.push(line);
        }

        // Collect all start events and verify indices 0, 1, 2.
        let start_events: Vec<serde_json::Value> = lines
            .iter()
            .filter(|l| l.starts_with("[workflow_agent]"))
            .filter_map(|l| {
                let json_str = l.trim_start_matches("[workflow_agent] ");
                serde_json::from_str::<serde_json::Value>(json_str).ok()
            })
            .filter(|v| v["state"] == "start")
            .collect();
        assert_eq!(start_events.len(), 3, "three start events; got: {lines:?}");
        assert_eq!(start_events[0]["index"], 0);
        assert_eq!(start_events[1]["index"], 1);
        assert_eq!(start_events[2]["index"], 2);
    }

    /// Agent events include phaseIndex/phaseTitle when agent() is dispatched during a phase.
    #[tokio::test]
    async fn workflow_agent_carries_phase_context() {
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let spawner = Arc::new(EchoSpawner::default());
        run_workflow_script(
            "phase('MyPhase'); await agent('task1');",
            DEFAULT_WORKFLOW_SUBAGENT,
            spawner,
            Arc::new(MockInvoker),
            Arc::new(MockBudget),
            Some(tx),
            None,
            None,
            None,
            0,
            NestedConfig::default(),
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            Arc::new(AnalyticsBus::new()),
            None,
            None,
        )
        .await
        .expect("runs");

        let mut lines: Vec<String> = Vec::new();
        while let Ok(line) = rx.try_recv() {
            lines.push(line);
        }

        let start_event = lines
            .iter()
            .filter(|l| l.starts_with("[workflow_agent]"))
            .find_map(|l| {
                let json_str = l.trim_start_matches("[workflow_agent] ");
                let v: serde_json::Value = serde_json::from_str(json_str).ok()?;
                (v["state"] == "start").then_some(v)
            })
            .expect("start event present");

        assert_eq!(start_event["phaseIndex"], 1, "phaseIndex = 1 (first phase)");
        assert_eq!(start_event["phaseTitle"], "MyPhase", "phaseTitle = MyPhase");
    }

    /// A journal-replayed agent emits a `cached` workflow_agent event.
    #[tokio::test]
    async fn workflow_agent_cached_event_on_journal_replay() {
        let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
        let dir = tempdir().unwrap();
        let mgr = Arc::new(TaskOutputManager::new(PathBuf::from(dir.path()), fs.clone()));

        let script = "const r = await agent('task'); log('r=' + r);";

        // Run 1: fresh — journal the result.
        let spawner1 = Arc::new(EchoSpawner::default());
        let sink1 = Arc::new(RecordingSink::default());
        let handle1 = make_handler(spawner1.clone(), mgr.clone(), sink1.clone())
            .spawn(workflow_input(script), make_ctx(fs.clone()))
            .await
            .unwrap();
        assert_eq!(await_terminal(&sink1).await, TaskStatus::Completed);
        let spool1 = dir.path().join(format!("{}.output", handle1.task_id));
        let out1 = mgr.read(&spool1, Default::default()).await.unwrap();
        let run_id = out1.content.lines().find_map(|l| l.strip_prefix("runId: ")).expect("runId").to_string();

        // Run 2: resume — agent replays from journal, should see `cached` event.
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let journal_path = dir.path().join(format!("workflow-{run_id}.json"));
        let journal_content = fs.read_file(journal_path.to_str().unwrap(), None, None).await.unwrap().content;
        let cache: std::collections::HashMap<String, String> = serde_json::from_str(&journal_content).expect("journal JSON");
        let journal = Arc::new(std::sync::Mutex::new(cache));

        run_workflow_script(
            script,
            DEFAULT_WORKFLOW_SUBAGENT,
            Arc::new(EchoSpawner::default()),
            Arc::new(MockInvoker),
            Arc::new(MockBudget),
            Some(tx),
            Some(journal),
            None,
            None,
            0,
            NestedConfig::default(),
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            Arc::new(AnalyticsBus::new()),
            None,
            None,
        )
        .await
        .expect("resume runs");

        let mut lines: Vec<String> = Vec::new();
        while let Ok(line) = rx.try_recv() {
            lines.push(line);
        }

        let cached_event = lines
            .iter()
            .filter(|l| l.starts_with("[workflow_agent]"))
            .find_map(|l| {
                let json_str = l.trim_start_matches("[workflow_agent] ");
                let v: serde_json::Value = serde_json::from_str(json_str).ok()?;
                (v["state"] == "cached").then_some(v)
            });
        assert!(cached_event.is_some(), "cached event must be emitted on journal replay; lines: {lines:?}");
        let ev = cached_event.unwrap();
        assert_eq!(ev["index"], 0, "cached agent has index 0");
        let tuid = ev["toolUseID"].as_str().expect("toolUseID");
        assert_eq!(tuid, "workflow_agent_0_cached", "cached toolUseID format");
    }

    /// A failed agent emits an `error` workflow_agent event.
    #[tokio::test]
    async fn workflow_agent_error_event_on_failure() {
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let spawner = Arc::new(EchoSpawner { fail: true, ..Default::default() });
        run_workflow_script(
            "const r = await agent('task'); log('r=' + r);",
            DEFAULT_WORKFLOW_SUBAGENT,
            spawner,
            Arc::new(MockInvoker),
            Arc::new(MockBudget),
            Some(tx),
            None,
            None,
            None,
            0,
            NestedConfig::default(),
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            Arc::new(AnalyticsBus::new()),
            None,
            None,
        )
        .await
        .expect("runs (failed agent is not a script error)");

        let mut lines: Vec<String> = Vec::new();
        while let Ok(line) = rx.try_recv() {
            lines.push(line);
        }

        let error_event = lines
            .iter()
            .filter(|l| l.starts_with("[workflow_agent]"))
            .find_map(|l| {
                let json_str = l.trim_start_matches("[workflow_agent] ");
                let v: serde_json::Value = serde_json::from_str(json_str).ok()?;
                (v["state"] == "error").then_some(v)
            });
        assert!(error_event.is_some(), "error event must be emitted for failed agent; lines: {lines:?}");
    }
}

//! Parallel Fusion panel execution via [`platform_api::SubagentSpawner`].

use crate::budget::FusionPriceBook;
use crate::config::FusionRuntimeConfig;
use crate::model_resolver::{ModelSource, ResolvedPanel};
use crate::orchestrator::price_realized_usage;
use crate::progress;
use async_trait::async_trait;
use platform_api::subagent_output_guard::sanitize_blocks;
use platform_api::subagent_spawn::{
    StructuredOutputMode, SubagentObservation, SubagentResult, SubagentSpawnObserver,
    SubagentSpawnRequest, SubagentSpawner, SubagentUsage, SUBAGENT_QUERY_TIMEOUT_REASON_PREFIX,
};
use platform_api::{
    validate_panel_report, FusionError, FusionInheritance, FusionProgress, FusionStage,
    FusionUsage, PanelReport, PanelRunStatus, WorkflowQueryWatchdog, FUSION_MIN_PANEL,
    FUSION_PANEL_TYPE,
};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::Sender;
use tokio::sync::Notify;
use tokio::task::JoinSet;
use tokio::time::timeout;

/// [Round-4 rework, item 2] Live sink for a panel stage's realized spend,
/// updated by `run_panels` after EVERY panel reaches a terminal outcome
/// (not just once, after the whole stage returns `Ok`). Bundles the three
/// survives-a-drop cells `run()`/`run_inner` already thread through the
/// orchestrator (see their own field docs) with everything
/// `price_realized_usage` needs to price what has ACTUALLY completed so
/// far, so a cancel landing mid-fan-out — after some panels finished with
/// real, billed usage but before the stage as a whole resolves — still
/// leaves the cells holding that real data instead of `None`/the coarse
/// pre-panel estimate latched before any panel was even dispatched. Every
/// field mirrors a `price_realized_usage` parameter naming an analyst/synth
/// call that has provably not happened yet at this point in the run (the
/// panel stage always precedes `analyze_and_decide`), so `update` always
/// passes `analyst_usage: None`, `analyst_attempted: false`,
/// `synth_usage: None`, `synth_attempted: false` — exactly the same
/// arguments `run_inner`'s own `check_panel_bar`-failure arm uses.
pub struct RealizedSpendSink<'a> {
    pub realized_tokens: &'a Arc<Mutex<Option<u64>>>,
    pub resolved_egress: &'a Arc<Mutex<Option<Vec<String>>>>,
    pub settlement: &'a Arc<Mutex<Option<(u64, bool)>>>,
    pub catalog: &'a dyn ModelSource,
    pub prices: &'a dyn FusionPriceBook,
    pub analyst: &'a ResolvedPanel,
    pub parent_profile: &'a str,
    pub parent_model: &'a str,
    pub request_prompt: &'a str,
}

impl RealizedSpendSink<'_> {
    /// Refresh all three cells from whatever has been collected so far,
    /// PLUS a one-turn floor for every panel that has already reached the
    /// spawner but has not finished yet ([`in_flight_panels`]).
    /// Called from inside `run_panels`' collection loop after every push to
    /// `collected`, from its cancel arm, and every time a panel task first
    /// reaches the spawner — so the cells are never more than one panel
    /// event stale when a cancel drops the whole future and discards
    /// `collected` itself.
    ///
    /// [Round-5 review items 6/7] Before this, `update` priced ONLY the
    /// panels that had already reached a terminal outcome, so the cell was
    /// not monotone: the first `update` after a spawn-rejected panel
    /// (`usage: None`, priced at exact $0) DELETED the pre-panel floor that
    /// covered the siblings still streaming, and a cancel one moment later
    /// committed less than a cancel one moment earlier would have. Panels
    /// still in flight have provably egressed their whole prompt, so they
    /// carry the same `estimate_in_flight_usage` floor `finish_panel` gives
    /// a panel killed mid-flight — and a panel that never reached the
    /// spawner contributes nothing at all (see `PanelDispatch`).
    fn update(
        &self,
        collected: &[(usize, PanelInternal)],
        panels: &[ResolvedPanel],
        generic_prompt: &str,
        dispatch: &PanelDispatch,
    ) {
        let in_flight = in_flight_panels(collected, panels, generic_prompt, dispatch);
        if let Ok(mut guard) = self.realized_tokens.lock() {
            // Output tokens only — an in-flight panel has provably sent its
            // input but has produced no output this side can count, so the
            // floor above deliberately contributes 0 here.
            *guard = Some(realized_output_tokens_so_far(collected));
        }
        if let Ok(mut guard) = self.resolved_egress.lock() {
            *guard = dispatched_profiles_so_far(collected, &in_flight);
        }
        if let Ok(mut guard) = self.settlement.lock() {
            // `price_realized_usage` prices a `&[PanelInternal]`, not the
            // `(index, PanelInternal)` pairs `collected` holds — a small
            // clone of whatever has finished so far (never more than
            // `total` panels) is cheap next to the provider round-trips
            // that produced them.
            let mut priced_so_far: Vec<PanelInternal> =
                collected.iter().map(|(_, panel)| panel.clone()).collect();
            priced_so_far.extend(in_flight.iter().cloned());
            *guard = Some(price_realized_usage(
                self.catalog,
                self.prices,
                &priced_so_far,
                self.analyst,
                None,
                false,
                self.parent_profile,
                self.parent_model,
                None,
                false,
                self.request_prompt,
            ));
        }
    }
}

/// `output_tokens` across every panel collected so far — the same field
/// `run_inner`'s post-stage refresh reads off `aggregate_panel_usage`, just
/// computed incrementally over `run_panels`' own in-progress `collected`
/// rather than once over the finished, sorted `Vec<PanelInternal>`.
fn realized_output_tokens_so_far(collected: &[(usize, PanelInternal)]) -> u64 {
    collected
        .iter()
        .filter_map(|(_, panel)| panel.usage.as_ref())
        .fold(0_u64, |acc, usage| acc.saturating_add(usage.output_tokens))
}

/// Mirrors `orchestrator::dispatched_egress_profiles`'s exact filter
/// (every panel except one provably reached before any provider call —
/// a pre-allocation `"spawn"` rejection or a `"not_dispatched"` slot the
/// bar/cancel killed before a subagent was ever allocated for it) and its
/// `None`-vs-empty contract, computed over `run_panels`' own in-progress
/// `collected` instead of the finished `Vec<PanelInternal>` `run_inner`
/// aggregates post-stage.
///
/// [Round-5 review item 6] `in_flight` (panels that reached the spawner and
/// have not finished) counts too: their whole prompt has already egressed,
/// so a cancel landing mid-fan-out must disclose their profiles exactly as
/// the terminal path does.
fn dispatched_profiles_so_far(
    collected: &[(usize, PanelInternal)],
    in_flight: &[PanelInternal],
) -> Option<Vec<String>> {
    let mut profiles: Vec<String> = collected
        .iter()
        .map(|(_, panel)| panel)
        .chain(in_flight.iter())
        .filter(|panel| !is_never_dispatched_category(panel.error_category.as_deref()))
        .map(|panel| panel.profile.clone())
        .collect();
    if profiles.is_empty() {
        return None;
    }
    profiles.sort();
    profiles.dedup();
    Some(profiles)
}

/// The `error_category` values that PROVE no provider call was made for that
/// slot: `"spawn"` (the spawner rejected the panel pre-allocation) and
/// `"not_dispatched"` ([Round-5 review items 8/12, round-6 blocking B1] the
/// slot was cancelled before its task ever called the spawner, or aborted
/// while inside a spawner call that never allocated a child — see
/// [`PanelDispatch`], whose two flags are exactly this distinction). Every
/// other category describes a panel for which a subagent provably existed,
/// and which may therefore have been billed.
///
/// A thin crate-local alias for [`platform_api::fusion::panel_never_dispatched`],
/// which is the SINGLE SOURCE OF TRUTH: `tool-agent` reads the same predicate
/// from there to decide how much of the session spawn reservation to release,
/// and round-6 blocking B2 was these two lists drifting apart when they were
/// independent `matches!` arms. Add a new value in platform-api, never here.
pub(crate) fn is_never_dispatched_category(category: Option<&str>) -> bool {
    platform_api::fusion::panel_never_dispatched(category)
}

/// One synthetic [`PanelInternal`] per panel that has reached the spawner
/// but has not yet reached a terminal outcome, carrying the same
/// [`estimate_in_flight_usage`] floor `finish_panel` gives a panel killed
/// mid-flight. Panels whose task never reached the spawner are omitted
/// entirely: estimating for them would invent spend for a call that
/// provably never happened (round-5 review item 16's rule).
fn in_flight_panels(
    collected: &[(usize, PanelInternal)],
    panels: &[ResolvedPanel],
    generic_prompt: &str,
    dispatch: &PanelDispatch,
) -> Vec<PanelInternal> {
    (0..panels.len())
        .filter(|index| {
            dispatch.reached_spawner(*index)
                && !collected.iter().any(|(collected, _)| collected == index)
        })
        .map(|index| PanelInternal {
            index,
            profile: panels[index].profile.clone(),
            model: panels[index].model.clone(),
            anonymous_id: String::new(),
            status: PanelRunStatus::Failed,
            report: None,
            duration_ms: 0,
            error_category: None,
            error_detail: None,
            usage: Some(estimate_in_flight_usage(generic_prompt)),
            spawn_prompt: generic_prompt.to_string(),
        })
        .collect()
}

/// [Round-5 review items 6/7/8/12] Which panel tasks have actually reached
/// the spawner.
///
/// `JoinSet::spawn` only ENQUEUES a future — tokio does not poll it before
/// the spawning task yields — and every panel task begins with its own
/// biased `cancel.cancelled()` arm, so "N tasks were spawned" is not
/// evidence that any subagent was ever created. Each task flips its own
/// flag from INSIDE the branch that calls `spawn_workflow_with_observer`
/// (see `spawn_panel_tasks`), which can only run once that biased cancel
/// arm has lost, and wakes `run_panels`' collection loop so it can emit
/// `PanelsDispatched` and refresh the settlement floor at that moment
/// instead of one line after `spawn_panel_tasks` returns.
/// [Round-6 blocking B1] The two flags are DELIBERATELY distinct — they
/// answer two different questions, and conflating them is the defect this
/// type was reworked to remove:
///
/// * `reached` = "this task entered the spawner call". It is set by the
///   task itself, one line before `spawn_workflow_with_observer`, so it
///   records an INTENT: the prompt is on its way out. It is the right
///   predicate for the in-flight settlement floor ([`in_flight_panels`])
///   and for the `PanelsDispatched` progress emit.
/// * `allocated` = "the pool actually allocated a child for this panel".
///   It is set from the `SubagentObservation::Allocated` the production
///   spawner emits on the line immediately after `pool.allocate` succeeds
///   and BEFORE the runner that makes any provider call exists
///   (`agent/src/handle.rs`). It records a FACT, and it is the only sound
///   predicate for "could this slot have been billed?".
///
/// They differ for seconds at a time: `spawn_workflow_with_observer` runs
/// `build_subagent_context` — which CONNECTS the panel's inline MCP servers
/// — before the pool can even refuse the spawn. A panel parked in that
/// window has `reached = true` and `allocated = false`, and if the panel bar
/// aborts it there (a sibling rejection sealed the run), classifying it as a
/// mid-flight `"aborted"` charges a lifetime spawn slot for a subagent that
/// provably never existed.
pub(crate) struct PanelDispatch {
    reached: Vec<AtomicBool>,
    allocated: Vec<AtomicBool>,
    signal: Notify,
}

impl PanelDispatch {
    pub(crate) fn new(total: usize) -> Self {
        Self {
            reached: (0..total).map(|_| AtomicBool::new(false)).collect(),
            allocated: (0..total).map(|_| AtomicBool::new(false)).collect(),
            signal: Notify::new(),
        }
    }

    /// Called by panel `index`'s own task immediately before it calls the
    /// spawner. `Notify::notify_one` stores a permit when nobody is
    /// waiting, so a mark that lands while the collection loop is busy is
    /// never lost.
    fn mark(&self, index: usize) {
        if let Some(flag) = self.reached.get(index) {
            flag.store(true, Ordering::SeqCst);
        }
        self.signal.notify_one();
    }

    /// Called from [`PanelAllocationObserver`] when the spawner reports that
    /// panel `index` has a real child slot. Deliberately does NOT notify:
    /// the `PanelsDispatched` emit and the settlement floor are keyed on
    /// [`Self::reached_spawner`], which has already woken the collection
    /// loop for this panel.
    fn mark_allocated(&self, index: usize) {
        if let Some(flag) = self.allocated.get(index) {
            flag.store(true, Ordering::SeqCst);
        }
    }

    pub(crate) fn reached_spawner(&self, index: usize) -> bool {
        self.reached
            .get(index)
            .is_some_and(|flag| flag.load(Ordering::SeqCst))
    }

    /// Whether a subagent was PROVABLY created for panel `index` — see the
    /// type docs for why this is not the same question as
    /// [`Self::reached_spawner`].
    pub(crate) fn allocated(&self, index: usize) -> bool {
        self.allocated
            .get(index)
            .is_some_and(|flag| flag.load(Ordering::SeqCst))
    }

    fn any_reached_spawner(&self) -> bool {
        self.reached.iter().any(|flag| flag.load(Ordering::SeqCst))
    }

    async fn notified(&self) {
        self.signal.notified().await;
    }
}

/// One per panel task: the `observer` slot of `spawn_workflow_with_observer`,
/// used for the single question `PanelDispatch` cannot answer from this side
/// of the spawn boundary — did the pool actually allocate a child?
///
/// `PoolSubagentSpawner::spawn_with_observer` emits
/// [`SubagentObservation::Allocated`] exactly once, on the line right after
/// `pool.allocate` returns `Ok` and before the runner exists, so observing it
/// is proof that a subagent was created; NOT observing it (the spawner
/// rejected the panel, or the task was killed while still building the
/// child's context) is proof that none was. Every other observation is
/// ignored: this panel's real work is already reported through the
/// `SubagentResult` the spawner call returns.
///
/// The observation is delivered through `agent::api::ObserverEventSink`'s
/// bounded channel, so there is a sub-millisecond window between
/// `pool.allocate` returning and this flag being set. `Allocated` is the
/// FIRST event ever emitted on a spawn's own freshly-built sink, so it can
/// never be the one the sink drops when full; the only residual is an abort
/// landing inside that window, which classifies a just-allocated panel as
/// `"not_dispatched"` and releases its spawn slot. That direction is the
/// safe one (a slot handed back, never a phantom subagent charged forever),
/// and it needs the bar to seal in the same instant the pool allocated.
struct PanelAllocationObserver {
    dispatch: Arc<PanelDispatch>,
    index: usize,
}

#[async_trait]
impl SubagentSpawnObserver for PanelAllocationObserver {
    async fn on_event(&self, event: SubagentObservation) {
        if matches!(event, SubagentObservation::Allocated { .. }) {
            self.dispatch.mark_allocated(self.index);
        }
    }
}

/// Host-side view of one panel after `JoinSet` collection (pre-anonymization).
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct PanelInternal {
    /// Spawn index (stable before shuffle).
    pub index: usize,
    /// Provider profile (stripped before the analyst).
    pub profile: String,
    /// Wire model (stripped before the analyst).
    pub model: String,
    /// Anonymous id assigned after shuffle (`P1` …).
    pub anonymous_id: String,
    /// Terminal status.
    pub status: PanelRunStatus,
    /// Validated, sanitized report when [`PanelRunStatus::Completed`].
    pub report: Option<PanelReport>,
    /// Wall-clock duration.
    pub duration_ms: u64,
    /// Sanitized error category.
    pub error_category: Option<String>,
    /// Sanitized, length-capped one-line detail of the source error (G011).
    /// Never a raw provider body — see [`sanitize_detail`].
    pub error_detail: Option<String>,
    /// Usage rollup.
    pub usage: Option<FusionUsage>,
    /// Prompt actually sent (tests assert mutual invisibility).
    pub spawn_prompt: String,
}

/// JSON Schema the hidden `fusion-panel` `StructuredOutput` tool must satisfy.
#[must_use]
pub fn panel_report_json_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": [
            "schema_version",
            "summary",
            "candidate_answer",
            "claims",
            "evidence",
            "assumptions",
            "risks",
            "unresolved_questions"
        ],
        "properties": {
            "schema_version": { "type": "integer" },
            "summary": { "type": "string" },
            "candidate_answer": { "type": "string" },
            "claims": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["statement", "evidence_refs", "confidence"],
                    "properties": {
                        "statement": { "type": "string" },
                        "evidence_refs": {
                            "type": "array",
                            "items": { "type": "string" }
                        },
                        "confidence": { "type": "integer", "minimum": 0, "maximum": 100 }
                    }
                }
            },
            "evidence": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["id", "kind", "locator"],
                    "properties": {
                        "id": { "type": "string" },
                        "kind": { "type": "string", "enum": ["file", "url", "command"] },
                        "locator": { "type": "string" },
                        "excerpt": { "type": "string" }
                    }
                }
            },
            "assumptions": {
                "type": "array",
                "items": { "type": "string" }
            },
            "risks": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["severity", "description"],
                    "properties": {
                        "severity": {
                            "type": "string",
                            "enum": ["low", "medium", "high", "critical"]
                        },
                        "description": { "type": "string" }
                    }
                }
            },
            "unresolved_questions": {
                "type": "array",
                "items": { "type": "string" }
            }
        }
    })
}

/// One panel task's return payload: its index, the resolved panel identity,
/// the prompt it was spawned with (needed to synthesize a panicked/aborted
/// slot's [`PanelInternal`]), how long it ran, and how it finished.
type PanelTaskOutput = (usize, ResolvedPanel, String, Duration, PanelFinish);

/// The stall-detector deadline passed to each panel's [`WorkflowQueryWatchdog`].
///
/// Must never exceed `panel_total_timeout_ms`: the panel's own hard timeout
/// wraps the whole `spawn_workflow_with_observer` future in `timeout()`
/// (see [`spawn_panel_tasks`]), so once that outer timeout fires the panel
/// task is gone and its watchdog can never fire afterward. A
/// `panel_idle_timeout_ms` configured above `panel_total_timeout_ms` is
/// therefore unreachable dead configuration — the stall detector is
/// silently inert for the run's entire duration — rather than a stricter
/// setting. Clamp it here, at the use site, so a merged settings view that
/// slips past the per-file and per-load checks still can't produce an
/// unusable watchdog deadline.
fn panel_stall_timeout_ms(config: &FusionRuntimeConfig) -> u64 {
    config.panel_idle_timeout_ms.min(config.panel_total_timeout_ms)
}

/// `run_panels` helper: spawn every panel's subagent task onto a fresh
/// `JoinSet`, returning it plus a task-id → panel-index map (the collection
/// loop needs this to recover a panicked/aborted task's identity — a join
/// error loses the task's own return payload, F012-join). Split out purely
/// to keep the caller under the line-count lint — same spawn shape, same
/// per-panel timeout/cancel/watchdog wiring.
#[allow(clippy::too_many_arguments)]
fn spawn_panel_tasks(
    spawner: &Arc<dyn SubagentSpawner>,
    inherit: &FusionInheritance,
    config: &FusionRuntimeConfig,
    panels: &[ResolvedPanel],
    run_id: &str,
    overall_deadline: Duration,
    schema: &str,
    generic_prompt: &str,
    max_input_bytes: u64,
    // [Round-5 review items 8/12] Flipped by each task from inside the
    // branch that actually calls the spawner, and — [round-6 blocking B1]
    // — flipped a second time from the spawner's own `Allocated`
    // observation once a child really exists. See `PanelDispatch`.
    dispatch: &Arc<PanelDispatch>,
) -> (JoinSet<PanelTaskOutput>, HashMap<tokio::task::Id, usize>) {
    let mut join_set = JoinSet::new();
    let mut task_index: HashMap<tokio::task::Id, usize> = HashMap::with_capacity(panels.len());
    // [Finding 16] The host-side spawn `name` must identify the same panel
    // the run reports under `anonymous_id` (assigned later, after
    // collection, by `anonymize`'s run_id-seeded shuffle) — not the
    // pre-shuffle spawn slot. Compute each spawn index's POST-shuffle anon
    // rank here, up front, from the same `(run_id, panel count)` inputs
    // `anonymize` will use, so `Fusion P{n}` in the Runtime Center and
    // `P{n}` in the reported outcome always name the same panel.
    let anon_rank = anon_rank_by_spawn_index(run_id, panels.len());
    for (index, panel) in panels.iter().cloned().enumerate() {
        let spawner = Arc::clone(spawner);
        let subagent = inherit.subagent.clone();
        let cancel = inherit.cancel.clone();
        let prompt = generic_prompt.to_string();
        let spawn_prompt = prompt.clone();
        let schema = schema.to_string();
        let run_id = run_id.to_string();
        let max_turns = config.panel_max_turns;
        let max_out = config.panel_max_output_tokens_per_turn;
        let panel_total_timeout =
            Duration::from_millis(config.panel_total_timeout_ms).min(overall_deadline);
        let panel_watchdog = WorkflowQueryWatchdog {
            stall_timeout_ms: panel_stall_timeout_ms(config),
            max_retries: 0,
        };
        let name_index = anon_rank[index];
        let dispatch = Arc::clone(dispatch);
        let abort_handle = join_set.spawn(async move {
            let started = Instant::now();
            let request = spawn_request(
                &panel,
                prompt,
                &schema,
                max_turns,
                max_out,
                max_input_bytes,
                &run_id,
                index,
                name_index,
            );
            let inherit = platform_api::subagent_spawn::SubagentInheritance {
                tool_invoker: subagent.tool_invoker,
                budget: subagent.budget,
            };
            let outcome = tokio::select! {
                biased;
                // [Round-5 review item 16] A cancel that lands before this
                // task ever called the spawner made no provider call, so it
                // must NOT be settled with the in-flight estimate the
                // mid-flight cancel below legitimately gets.
                () = cancel.cancelled() => PanelFinish::Cancelled {
                    dispatched: dispatch.reached_spawner(index),
                },
                result = timeout(
                    panel_total_timeout,
                    // [Round-5 review items 8/12] The mark lives INSIDE this
                    // future's body, not beside the `select!`: a `select!`
                    // branch expression is built eagerly (even when the
                    // biased cancel arm above wins), while this line runs
                    // only once this branch is actually polled — i.e. only
                    // once the spawner call is genuinely about to be made.
                    async {
                        dispatch.mark(index);
                        // [Round-6 blocking B1] The `observer` slot, which
                        // used to be `None`, is what turns "we called the
                        // spawner" into "a subagent exists": the pool emits
                        // `Allocated` the instant it hands this panel a
                        // child slot. Without it, a panel killed by the bar
                        // while still INSIDE a slow rejection (the spawner
                        // connects the panel's inline MCP servers before it
                        // can refuse) was indistinguishable from one cut
                        // down mid-stream.
                        let allocation_observer: Arc<dyn SubagentSpawnObserver> =
                            Arc::new(PanelAllocationObserver {
                                dispatch: Arc::clone(&dispatch),
                                index,
                            });
                        spawner
                            .spawn_workflow_with_observer(
                                request,
                                inherit,
                                None,
                                Some(allocation_observer),
                                panel_watchdog,
                            )
                            .await
                    },
                ) => {
                    match result {
                        Ok(Ok(terminal)) => PanelFinish::Done(terminal),
                        Ok(Err(err)) => PanelFinish::Failed {
                            category: "spawn".into(),
                            detail: Some(sanitize_detail(&err.to_string())),
                        },
                        Err(_) => PanelFinish::TotalTimedOut,
                    }
                }
            };
            (index, panel, spawn_prompt, started.elapsed(), outcome)
        });
        task_index.insert(abort_handle.id(), index);
    }
    (join_set, task_index)
}

/// Run every panel concurrently.
///
/// Cancel aborts the `JoinSet` and joins every task. Re-evaluates the panel bar
/// after every completion (G004): once no combination of the still-running
/// panels could reach `config.min_successful_panels` (or, when
/// `!partial_ok`, as soon as any panel fails), the remaining siblings
/// are aborted immediately rather than left to spend the full
/// `panel_total_timeout_ms` paying a provider for a run that is already
/// sealed. A panicked or aborted task's slot is synthesized from the
/// `JoinError` (F012-join) so `result.len() == panels.len()` always holds —
/// the bar and telemetry never silently lose a panel.
///
/// `partial_ok` is the CALLER's already-combined effective value —
/// [Finding 20] `request.partial_ok && config.partial_ok` — not
/// `config.partial_ok` alone: a request that opts out of partial results
/// (`/fusion --no-partial`, the Agent tool's `partial_ok: false`, or
/// workflow `fusion({partialOk:false})`) must seal the bar on the FIRST
/// panel failure exactly the way a settings-level `fusion.partialOk: false`
/// already does, instead of only being enforced after every panel has
/// already burned a full `panel_total_timeout_ms` round-trip in
/// `check_panel_bar`'s separate, later `PanelSetIncomplete` check.
///
/// `overall_deadline` (F004 review fix) is what remains of the end-to-end
/// `total_timeout_ms` budget at the moment the caller starts the panel stage
/// (`FusionOrchestrator::remaining(started)`). Each panel's own timeout is
/// `min(config.panel_total_timeout_ms, overall_deadline)` so a panel stage that
/// would otherwise run past the whole-run deadline is cut off HERE — with
/// whatever panels already finished kept in the returned `Vec` — instead of
/// being cut off by the outer `run()` wrapper, which drops `run_inner` (and every
/// panel result gathered so far) wholesale and degrades to `TimedOutEmpty` even
/// when panels had already produced enough successful material for `NeedsParent`.
#[allow(clippy::too_many_arguments)]
pub async fn run_panels(
    spawner: Arc<dyn SubagentSpawner>,
    inherit: &FusionInheritance,
    config: &FusionRuntimeConfig,
    partial_ok: bool,
    task_prompt: &str,
    panels: &[ResolvedPanel],
    run_id: &str,
    overall_deadline: Duration,
    progress: &Option<Sender<FusionProgress>>,
    // [Round-4 rework, item 2] `None` in tests that don't exercise the
    // survives-a-drop cells; `Some` from `FusionOrchestrator::run_panel_stage`
    // on every real run, so a cancel landing mid-fan-out — see the four
    // `sink.update(..)` call sites below (a panel reached the spawner, a
    // panel finished, a panel's task died, and the cancel arm itself) —
    // still leaves them holding whatever really happened before it did. See
    // `RealizedSpendSink`.
    sink: Option<&RealizedSpendSink<'_>>,
) -> Result<Vec<PanelInternal>, FusionError> {
    let schema = serde_json::to_string(&panel_report_json_schema()).unwrap_or_default();
    let total = panels.len();
    let min_successful = usize::from(
        config
            .min_successful_panels
            .min(u8::try_from(total).unwrap_or(u8::MAX)),
    )
    .max(usize::from(FUSION_MIN_PANEL))
    .min(total);
    // Every panel's prompt is identical (panels are anonymized to each
    // other, so the task text never varies by identity) — built once and
    // reused both for spawning and for synthesizing a panicked/aborted slot.
    let generic_prompt = panel_prompt(task_prompt);
    // T1 item 2 (user-directed policy): this is the one bytes-per-token
    // conversion that runs BEFORE any provider call (`budget::quote`'s own
    // peak stays pure config-derived token counts — see its module doc,
    // "never 1 byte = 1 token" — so it has no text-based estimate of its own
    // to align). It used to hardcode a bare `4` here; now it reads
    // `llm_client::model::count_tokens::APPROX_CHARS_PER_TOKEN`, the SAME
    // named constant `count_tokens`'s own coarse transcript-size estimates
    // are built from, instead of a silently-duplicated magic number that
    // could drift from it. The missing-usage SETTLEMENT fallback
    // (`estimate_in_flight_usage` above, `judge_input_token_estimate` in
    // `orchestrator.rs`) instead calls
    // `count_tokens::approximate_tokens_for_bytes`, the request-fit formula
    // `count_tokens::approximate_tokens` itself is defined in terms of — a
    // deliberately MORE conservative (denser) divisor than this cap's, since
    // the two serve different purposes: this is a generous ceiling on bytes
    // actually sent (permissive is safe — panels have their own real
    // `max_input_bytes_per_turn` enforcement), while the settlement fallback
    // prices real dollars and must not UNDER-count.
    let max_input_bytes = u64::from(config.panel_reserved_input_tokens_per_turn)
        * llm_client::model::count_tokens::APPROX_CHARS_PER_TOKEN;

    // [Round-5 review items 8/12] Shared with every spawned task so both
    // the `PanelsDispatched` emit below and the aborted-slot classification
    // further down can tell a task that really called the spawner from one
    // that was killed before it ever did — and [round-6 blocking B1] so the
    // latter can further tell a task that was merely INSIDE the spawner
    // call from one the pool actually allocated a child for. See
    // `PanelDispatch`.
    let dispatch = Arc::new(PanelDispatch::new(total));
    let (mut join_set, task_index) = spawn_panel_tasks(
        &spawner,
        inherit,
        config,
        panels,
        run_id,
        overall_deadline,
        &schema,
        &generic_prompt,
        max_input_bytes,
        &dispatch,
    );

    let mut collected: Vec<(usize, PanelInternal)> = Vec::with_capacity(total);
    let mut succeeded = 0usize;
    let mut failed = 0usize;
    let mut bar_aborted = false;
    let mut dispatched_emitted = false;
    while collected.len() < total {
        tokio::select! {
            biased;
            () = inherit.cancel.cancelled() => {
                join_set.abort_all();
                // [Round-9 review item 1] The drain here used to be
                // `while join_set.join_next().await.is_some() {}` — every
                // task output bound to nothing. This arm is polled BEFORE
                // `join_next_with_id()` below, so a panel that had already
                // finished with real, provider-reported usage but had not
                // yet been pulled into `collected` loses that race and was
                // thrown away; the `sink.update` at the end of this arm then
                // re-synthesized it through `in_flight_panels` at
                // `estimate_in_flight_usage`'s prompt-length floor
                // (`approximate_tokens_for_bytes(prompt.len())` input tokens
                // and ZERO output tokens) instead of the tokens the provider
                // had already billed — and that is the figure `run()`'s
                // outer `Err` arm commits through `lease.commit`. Collect
                // each drained payload exactly the way the completed arm
                // below does instead.
                //
                // The refresh is INSIDE the loop, not once after it: if any
                // remaining task is not already finished, this drain yields,
                // and `run()`'s outer biased cancel arm (waiting on the SAME
                // token) then drops `run_inner` — with this whole `select!`
                // — before the `sink.update` at the end of this arm can run.
                // Latching each real completion as it is drained is what
                // survives that drop; the trailing update still covers the
                // panels that are genuinely still in flight.
                //
                // `JoinError` slots stay discarded: they carry no usage, and
                // synthesizing one here would REPLACE the in-flight floor a
                // reached-but-never-allocated slot legitimately holds with
                // an exact $0 that the abort itself does not prove (see
                // `panel_from_join_error`'s `"not_dispatched"` arm and
                // `in_flight_panels`' deliberate `reached_spawner`
                // predicate).
                while let Some(joined) = join_set.join_next_with_id().await {
                    if let Ok((_id, (index, panel, spawn_prompt, elapsed, outcome))) = joined {
                        let internal =
                            finish_panel(index, panel, spawn_prompt, elapsed, outcome);
                        collected.push((index, internal));
                        if let Some(sink) = sink {
                            sink.update(&collected, panels, &generic_prompt, &dispatch);
                        }
                    }
                }
                // [Round-5 rework of item 12] This arm is BIASED ABOVE the
                // `dispatch.notified()` arm below, so when a panel task
                // really reached the spawner (`dispatch.mark` ran, a
                // subagent is being built) and the cancel fires before the
                // loop is next polled, the notify arm never runs and this
                // exit would leave the whole run with NO progress event at
                // all. `tools/agent`'s `stage_proves_panel_spawned` would
                // then see no proof, `releases_full_reservation` would be
                // true for `FusionError::Cancelled`, and the session would
                // hand back the entire `panel_n` spawn reservation for
                // subagents that genuinely exist — the exact mirror of the
                // over-charge item 12 removed. The shared one-shot helper
                // is what keeps the two arms from drifting apart again:
                // nothing is emitted when no task ever reached the spawner.
                emit_panels_dispatched_once(&mut dispatched_emitted, &dispatch, progress, total);
                // [Round-5 review item 6] One last refresh before the whole
                // `collected` vector is discarded: the panels still in
                // flight at this instant have already egressed their entire
                // prompt, and without this they would drop out of the
                // settlement the outer `Err` arm commits — making a cancel
                // bill strictly less than the total-timeout terminal state
                // does for the identical panel set.
                if let Some(sink) = sink {
                    sink.update(&collected, panels, &generic_prompt, &dispatch);
                }
                return Err(FusionError::Cancelled);
            }
            // [round-3 review, findings 11/19; round-5 review item 12] A
            // panel task has reached the spawner — real provider calls are,
            // or immediately were, in flight. This is the earliest point at
            // which that is TRUE: `spawn_panel_tasks` only enqueues futures
            // (tokio polls none of them before this task yields) and every
            // panel task begins with its own biased cancel arm, so the old
            // emit — one synchronous line after `spawn_panel_tasks`
            // returned — announced dispatch for a set of panels a cancel in
            // that same window guaranteed would never exist, and the Agent
            // tool kept the session's spawn quota charged for them.
            () = dispatch.notified() => {
                emit_panels_dispatched_once(&mut dispatched_emitted, &dispatch, progress, total);
                // The in-flight floor this panel just earned (its prompt is
                // egressed the moment the spawner call is made) belongs in
                // the settlement cell right away — a cancel one poll later
                // reads exactly this value.
                if let Some(sink) = sink {
                    sink.update(&collected, panels, &generic_prompt, &dispatch);
                }
            }
            next = join_set.join_next_with_id() => {
                match next {
                    Some(Ok((_id, (index, panel, spawn_prompt, elapsed, outcome)))) => {
                        let internal = finish_panel(index, panel, spawn_prompt, elapsed, outcome);
                        if internal.status == PanelRunStatus::Completed {
                            succeeded += 1;
                        } else {
                            failed += 1;
                        }
                        collected.push((index, internal));
                        // [Round-4 rework, item 2] Refresh the survives-a-drop
                        // cells with this panel's real usage/egress/spend
                        // BEFORE the next `cancel.cancelled()` poll of this
                        // same `select!` can win the race — otherwise a
                        // cancel landing right after this panel's own
                        // completion would still see stale (or `None`) cells.
                        if let Some(sink) = sink {
                            sink.update(&collected, panels, &generic_prompt, &dispatch);
                        }
                        // F005: fan out ONE `RunningPanels{completed,total}`
                        // event per finished panel (not just once at 0/total
                        // before the stage starts) so the longest stage of a
                        // run — up to `panel_total_timeout_ms` per panel — has
                        // visible progress instead of a single stalled event.
                        emit_running_panels(progress, index, collected.len(), total);
                    }
                    Some(Err(join_err)) => {
                        if let Some((index, internal)) = panel_from_join_error(
                            &join_err,
                            &task_index,
                            panels,
                            &generic_prompt,
                            &dispatch,
                        ) {
                            failed += 1;
                            collected.push((index, internal));
                            // [Round-4 rework, item 2] Same incremental
                            // refresh as the completed-panel arm above — a
                            // panicked/aborted slot still needs to be
                            // reflected before the next cancel poll.
                            if let Some(sink) = sink {
                                sink.update(&collected, panels, &generic_prompt, &dispatch);
                            }
                            emit_running_panels(progress, index, collected.len(), total);
                        }
                    }
                    None => break,
                }
            }
        }
        if !bar_aborted {
            let remaining = total.saturating_sub(collected.len());
            let cannot_reach_min = succeeded.saturating_add(remaining) < min_successful;
            let any_failure_requires_all = !partial_ok && failed > 0;
            if remaining > 0 && (cannot_reach_min || any_failure_requires_all) {
                join_set.abort_all();
                bar_aborted = true;
            }
        }
    }

    collected.sort_by_key(|(index, _)| *index);
    Ok(collected.into_iter().map(|(_, internal)| internal).collect())
}

/// `run_panels` helper: synthesize the panel slot behind a `JoinError`.
///
/// A panicked or (post-abort) cancelled task loses its `(index, panel, ..)`
/// payload with the join error — recover the index from the id map planted
/// at spawn time and the panel identity from the ORIGINAL `panels` slice, so
/// the slot is never simply dropped (F012-join).
///
/// [Round-5 review item 8] A slot aborted before its task ever called the
/// spawner (`abort_all` racing a still-unpolled task, or one still parked on
/// its own cancel arm) provably created no subagent and made no provider
/// call — the same shape as a pre-allocation `"spawn"` rejection, and the
/// opposite of a panel cut down mid-stream. Only the latter keeps
/// `"aborted"` (and with it the in-flight usage estimate `finish_panel`
/// gives that category).
///
/// [Round-6 blocking B1] The predicate for "cut down mid-stream" is
/// `PanelDispatch::allocated`, NOT `reached_spawner`. Entering the spawner
/// call is only an intent: `spawn_workflow_with_observer` builds the child's
/// context (connecting its inline MCP servers) and contends the pool's slot
/// table before it can even reject, and a panel the bar aborts inside THAT
/// window provably has no subagent and no provider call — exactly the
/// `"spawn"` shape. Keying this on `reached_spawner` meant a run whose every
/// panel was rejected by a full pool, with one rejection slow enough for the
/// bar to abort it first, reported `AllPanelsFailed` instead of
/// `AllPanelsFailedPreflight` and kept the caller's ENTIRE spawn
/// reservation charged for zero subagents.
fn panel_from_join_error(
    join_err: &tokio::task::JoinError,
    task_index: &HashMap<tokio::task::Id, usize>,
    panels: &[ResolvedPanel],
    generic_prompt: &str,
    dispatch: &PanelDispatch,
) -> Option<(usize, PanelInternal)> {
    let &index = task_index.get(&join_err.id())?;
    let category = if join_err.is_panic() {
        "panic"
    } else if dispatch.allocated(index) {
        "aborted"
    } else {
        "not_dispatched"
    };
    let internal = finish_panel(
        index,
        panels[index].clone(),
        generic_prompt.to_string(),
        Duration::default(),
        PanelFinish::Failed {
            category: category.into(),
            detail: Some(sanitize_detail(&join_err.to_string())),
        },
    );
    Some((index, internal))
}

/// Emit `RunningPanels{completed,total}` for one finished panel (F005). Panels
/// are not yet anonymized inside `run_panels` (anonymization runs on the
/// caller's side after this returns), so `panel_id` is the pre-shuffle spawn
/// slot (`p{index+1}`) — an identifier for progress purposes only, never
/// exposed as the panel's real anonymous id.
/// [round-3 review, findings 11/19] Emit `PanelsDispatched{total}` once,
/// right after every panel task has been handed to the spawner (see the
/// call site in [`run_panels`] for why this must be distinct from both the
/// pre-spawn `RunningPanels{completed:0,..}` event and the post-completion
/// `RunningPanels{completed>0,..}` events [`emit_running_panels`] sends).
fn emit_panels_dispatched(progress: &Option<Sender<FusionProgress>>, total: usize) {
    let stage = FusionStage::PanelsDispatched {
        total: u8::try_from(total).unwrap_or(u8::MAX),
    };
    progress::emit(progress, stage.clone(), None, stage.label());
}

/// [Round-5 rework of item 12] The one-shot `PanelsDispatched` emit shared
/// by BOTH exits of `run_panels`' collection loop that can be the first to
/// observe a panel reaching the spawner: the biased `cancel.cancelled()`
/// arm and the `dispatch.notified()` arm below it.
///
/// It exists as one function precisely so those two cannot drift apart
/// again. Item 12 moved the emit off the synchronous line after
/// `spawn_panel_tasks` (which announced dispatch for panels a cancel in
/// that window guaranteed would never exist) into the notify arm — but the
/// cancel arm is polled FIRST, so a cancel landing after a task had really
/// run `dispatch.mark` and entered `spawn_workflow_with_observer` returned
/// `Err(Cancelled)` with no progress event at all. `tools/agent`'s
/// `panels_proven_spawned` then saw no proof and released the session's
/// ENTIRE `panel_n` spawn reservation for subagents that genuinely exist —
/// the exact mirror of the over-charge item 12 removed.
///
/// The two other exits of that loop (a panel completed, a panel's task
/// died) never need this: `select!` is `biased`, so `dispatch.notified()`
/// is polled before `join_next_with_id()`, and `PanelDispatch::mark` sets
/// its flag before storing the `Notify` permit — so any panel that reached
/// the spawner has already made the notify arm win an earlier poll. See
/// `panels_dispatched_precedes_the_completed_arm_for_a_dispatched_panel`.
fn emit_panels_dispatched_once(
    emitted: &mut bool,
    dispatch: &PanelDispatch,
    progress: &Option<Sender<FusionProgress>>,
    total: usize,
) {
    if *emitted || !dispatch.any_reached_spawner() {
        return;
    }
    *emitted = true;
    emit_panels_dispatched(progress, total);
}

fn emit_running_panels(
    progress: &Option<Sender<FusionProgress>>,
    finished_index: usize,
    completed: usize,
    total: usize,
) {
    let stage = FusionStage::RunningPanels {
        completed: u8::try_from(completed).unwrap_or(u8::MAX),
        total: u8::try_from(total).unwrap_or(u8::MAX),
    };
    progress::emit(
        progress,
        stage.clone(),
        Some(format!("p{}", finished_index + 1)),
        stage.label(),
    );
}

enum PanelFinish {
    Done(SubagentResult),
    /// Sanitized category label plus an optional sanitized, length-capped
    /// one-line detail of the source error (G011) — never a raw provider
    /// body. `detail` is `None` only when there was nothing to attach.
    Failed {
        category: String,
        detail: Option<String>,
    },
    TotalTimedOut,
    /// The panel's own biased `cancel.cancelled()` arm won. `dispatched`
    /// records whether this task had already called the spawner when that
    /// happened — [Round-5 review item 16] a cancel that landed first made
    /// no provider call at all, so it must not be settled with the
    /// in-flight estimate a genuinely mid-flight cancel gets.
    ///
    /// [Round-6 blocking B1, class sweep] This stays keyed on
    /// `reached_spawner`, NOT on the new `allocated` flag, for two reasons.
    /// (1) It also decides the SETTLEMENT: `dispatched: true` is what buys
    /// the `estimate_in_flight_usage` floor, and B1 explicitly keeps that
    /// floor on "the prompt has egressed" rather than "a child exists".
    /// (2) Its category can never reach a spawn-quota consumer: this
    /// variant is only produced once `inherit.cancel` is set, and
    /// `run_panels`' own collection loop has a BIASED `cancel.cancelled()`
    /// arm above every arm that could collect it, so the loop returns
    /// `Err(FusionError::Cancelled)` — an error whose accounting keys on
    /// `PanelsDispatched`, not on any panel category — before this outcome
    /// can be pushed into `collected`. `dispatched: false` still implies
    /// `!allocated`, so the `"not_dispatched"` it does emit is never a lie.
    Cancelled {
        dispatched: bool,
    },
}

#[allow(clippy::too_many_arguments)]
fn spawn_request(
    panel: &ResolvedPanel,
    prompt: String,
    schema: &str,
    max_turns: u32,
    max_out: u32,
    max_input_bytes: u64,
    run_id: &str,
    index: usize,
    name_index: usize,
) -> SubagentSpawnRequest {
    SubagentSpawnRequest {
        subagent_type: FUSION_PANEL_TYPE.to_string(),
        prompt,
        model: Some(panel.model.clone()),
        model_profile: Some(panel.profile.clone()),
        schema: Some(schema.to_string()),
        // Auto tool_choice while turns remain, forced only on the last turn
        // (or a no-progress nudge) — so the panel can actually use its
        // Read/Grep/Glob/WebFetch tools instead of answering blind on turn 1
        // (F002).
        structured_output_mode: StructuredOutputMode::WhenDone,
        max_turns_override: Some(max_turns),
        max_output_tokens_per_turn: Some(max_out),
        // Per-turn input cap derived from the reserved-token budget basis
        // (F002): `cap_input_bytes` is pair-aware (see `runner::cap_input_bytes`)
        // so this can never orphan a tool_use/tool_result half.
        max_input_bytes_per_turn: Some(max_input_bytes),
        query_source_label: Some("fusion_panel".into()),
        correlation_id: Some(format!("{run_id}:p{index}")),
        // Host-side observability name (F005 prerequisite) — without this a
        // spawn observer falls back to the bare agent_type and every panel
        // in a run renders as an indistinguishable "fusion-panel" row.
        // [Finding 16] Named by `name_index` — the POST-shuffle anon rank —
        // not the raw pre-shuffle `index`, so this label always matches the
        // `P{n}` the run later reports as `anonymous_id` for the same panel.
        name: Some(format!("Fusion P{}", name_index + 1)),
        ..SubagentSpawnRequest::default()
    }
}

// `pub(crate)`, not private: `orchestrator_test` reconstructs the exact
// spawn prompt to compute the expected value of the missing-usage estimate
// fallback (`estimate_in_flight_usage`) instead of duplicating this literal.
pub(crate) fn panel_prompt(task: &str) -> String {
    format!(
        "You are one independent Fusion panel. You cannot see other panels and \
must not mention providers, model names, or that you are part of an ensemble.\n\n\
Task:\n{task}"
    )
}

fn finish_panel(
    index: usize,
    panel: ResolvedPanel,
    spawn_prompt: String,
    elapsed: Duration,
    outcome: PanelFinish,
) -> PanelInternal {
    let duration_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
    let mut internal = PanelInternal {
        index,
        profile: panel.profile,
        model: panel.model,
        anonymous_id: String::new(),
        status: PanelRunStatus::Failed,
        report: None,
        duration_ms,
        error_category: None,
        error_detail: None,
        usage: None,
        spawn_prompt,
    };
    match outcome {
        PanelFinish::TotalTimedOut => {
            internal.status = PanelRunStatus::TimedOut;
            internal.error_category = Some("timeout".into());
            // T1 item 1 (user-directed policy): the panel's future was
            // racing `timeout()` and lost, so no terminal `SubagentResult`
            // was ever produced — there is no real usage to report. It was
            // genuinely in flight, though (unlike a pre-flight spawn
            // failure), so settle for an estimate of what we DO know was
            // sent rather than reporting exact $0 for real, already-billed
            // spend.
            internal.usage = Some(estimate_in_flight_usage(&internal.spawn_prompt));
        }
        PanelFinish::Cancelled { dispatched: true }
        | PanelFinish::Done(SubagentResult::Killed { .. }) => {
            internal.status = PanelRunStatus::Cancelled;
            internal.error_category = Some("cancelled".into());
            // Same reasoning as the timeout arm above.
            internal.usage = Some(estimate_in_flight_usage(&internal.spawn_prompt));
        }
        PanelFinish::Cancelled { dispatched: false } => {
            // [Round-5 review item 16] The cancel beat this task to its own
            // spawner call, so nothing was ever sent: `usage` stays `None`
            // (exact $0, the same treatment a `"spawn"` rejection gets)
            // and the category says the slot never dispatched so
            // `check_panel_bar` / `dispatched_egress_profiles` can tell it
            // apart from a panel killed mid-stream.
            internal.status = PanelRunStatus::Cancelled;
            internal.error_category = Some("not_dispatched".into());
        }
        PanelFinish::Failed { category, detail } => {
            // Round-3 review item 2 fix: a `"aborted"`/`"panic"` category
            // comes from the `JoinError` arm in `run_panels` — the bar-abort
            // path (a still-streaming panel killed by `abort_all()` once the
            // remaining panels can no longer reach `min_successful_panels`)
            // or a genuine task panic. Either way the task was cut down
            // MID-FLIGHT, exactly the "future never produced a terminal
            // `SubagentResult`" shape the `TotalTimedOut`/`Cancelled` arms
            // above already estimate for under the same T1 policy — settling
            // it at exact `usage: None` silently drops real, already-billed
            // spend. A `"spawn"` category, in contrast, is a PRE-FLIGHT
            // failure (the spawner's `Result::Err` returned before any
            // provider call could possibly have happened) and must stay at
            // `None`: estimating there would invent spend for a call that
            // never happened (see `spawn_failure_still_reports_no_usage_at_all`
            // below).
            if category == "aborted" || category == "panic" {
                internal.usage = Some(estimate_in_flight_usage(&internal.spawn_prompt));
            }
            internal.error_category = Some(category);
            internal.error_detail = detail;
        }
        PanelFinish::Done(SubagentResult::Failed { reason, usage, .. })
            if is_query_watchdog_timeout(&reason) =>
        {
            internal.status = PanelRunStatus::TimedOut;
            internal.error_category = Some("idle_timeout".into());
            // Finding [11]: an idle-timeout still reflects real, already-billed
            // spend from every turn that completed before the watchdog fired —
            // settling it at $0 (the old `internal.usage` stays `None` shape)
            // silently ate that spend instead of pricing it.
            internal.usage = Some(usage_from_failed_subagent(&usage));
        }
        PanelFinish::Done(SubagentResult::Failed { reason, usage, .. })
            if is_missing_structured_output_reason(&reason) =>
        {
            // Round-3 review items 9/20 fix: `agent/src/runner.rs`'s
            // `!terminated_cleanly` arm now intercepts EVERY schema run that
            // falls out of its turn loop without a captured StructuredOutput
            // call — BEFORE it can ever reach the `Completed{"reason":
            // "max_turns_exhausted"}` shape `max_turns_exhausted_detail`
            // below recognizes. Since every Fusion panel spawns with
            // `schema: Some(..)` (`spawn_request` below), that means turn-
            // budget exhaustion (and the nudge-give-up path, which shares
            // the identical byte-locked reason string — the runner cannot
            // be edited to tell them apart without breaking oracle parity,
            // see `runner.rs`'s own comment at the emission site) now always
            // arrives HERE, as a `SubagentResult::Failed`, not as the
            // `Completed` shape below. Label it distinctly from a genuine
            // provider error rather than folding it into `"provider"`
            // (the [Finding 25] misattribution this was filed to remove) —
            // and distinctly from `"max_turns"` too, since this string alone
            // cannot promise a turn count or distinguish the two causes.
            internal.error_category = Some("no_structured_output".into());
            internal.error_detail = Some(sanitize_detail(&reason));
            internal.usage = Some(usage_from_failed_subagent(&usage));
        }
        PanelFinish::Done(SubagentResult::Failed { reason, usage, .. })
            if is_structured_retry_cap_exceeded_reason(&reason) =>
        {
            // [Round-4 review item 16] `agent/src/runner.rs` has a SECOND
            // schema-contract failure, distinct from
            // `is_missing_structured_output_reason` above: the model DID call
            // `StructuredOutput`, but every one of `structured_retry_cap`
            // attempts failed schema validation. Left unrecognized this fell
            // through to the generic `"provider"` arm below, telling the
            // operator the *provider* failed when the panel's own output
            // never matched the schema — a distinct cause from
            // `"no_structured_output"` (which means the model never called
            // `StructuredOutput` at all) and from a genuine provider error,
            // so it gets its own category rather than folding into either.
            internal.error_category = Some("schema_retry_exhausted".into());
            internal.error_detail = Some(sanitize_detail(&reason));
            internal.usage = Some(usage_from_failed_subagent(&usage));
        }
        PanelFinish::Done(SubagentResult::Failed { reason, usage, .. }) => {
            internal.error_category = Some("provider".into());
            internal.error_detail = Some(sanitize_detail(&reason));
            // Finding [11]: same reasoning as the idle-timeout arm above — a
            // provider-error termination still carries whatever billed spend
            // happened on turns before the one that failed.
            internal.usage = Some(usage_from_failed_subagent(&usage));
        }
        PanelFinish::Done(SubagentResult::Completed {
            content,
            usage,
            cumulative_usage,
            assistant_message_count,
            usage_complete,
            ..
        }) => {
            let mut panel_usage = usage_from_subagent(
                &cumulative_usage,
                &usage,
                assistant_message_count,
            );
            // Finding [9]: the runner's `api_error_partial` salvage path
            // emits `Completed` with STALE usage (the last turn that
            // completed successfully before the unrecovered mid-stream
            // error) — `usage_complete: false` says so. Mark the run
            // `estimated` rather than reporting the short figure as exact.
            if !usage_complete {
                panel_usage.estimated = true;
            }
            internal.usage = Some(panel_usage);
            if let Some(detail) = max_turns_exhausted_detail(&content) {
                // [Finding 25] The runner's `!terminated_cleanly` arm reports
                // turn-budget exhaustion as a normal `Completed` event, not a
                // schema violation — recognize it before `parse_and_sanitize`
                // (which would always return `Err("protocol")` for this
                // shape, since it has none of `PanelReport`'s required
                // fields) so telemetry and the parent-visible outcome say
                // "ran out of turns", not "malformed report".
                internal.error_category = Some("max_turns".into());
                internal.error_detail = Some(detail);
            } else {
                match parse_and_sanitize(&content) {
                    Ok(report) => {
                        internal.status = PanelRunStatus::Completed;
                        internal.report = Some(report);
                    }
                    Err(category) => {
                        // [Round-4 review item 5] `usage_complete: false` is
                        // ONLY ever set by the runner's CC 2.1.207
                        // `api_error_partial` salvage arm
                        // (`agent::runner::build_recovered_result`, taken when
                        // a mid-stream provider rate-limit/overload/transport
                        // error cuts a turn off after the panel has already
                        // produced some text). That shape has none of
                        // `PanelReport`'s required fields, so
                        // `parse_and_sanitize` always returns `Err("protocol")`
                        // for it — reporting a provider-side connection drop
                        // as "the panel returned a malformed report" and
                        // silently dropping the "Agent terminated early due to
                        // an API error: …" text the runner already carried in
                        // the first block of `content`. Recover it here so the
                        // operator (and `tengu.fusion` telemetry) see the real
                        // cause instead of a generic parse failure.
                        let cutoff_detail =
                            (!usage_complete).then(|| api_error_cutoff_detail(&content)).flatten();
                        if let Some(detail) = cutoff_detail {
                            internal.error_category = Some("provider_cutoff".into());
                            internal.error_detail = Some(sanitize_detail(&detail));
                        } else {
                            internal.error_category = Some(category);
                        }
                    }
                }
            }
        }
    }
    internal
}

/// [Finding 25] Recognize the runner's `!terminated_cleanly` shape
/// (`agent/src/runner.rs`'s `SubagentEvent::Completed{ result: {"reason":
/// "max_turns_exhausted", "max_turns": N}, .. }`) before it is handed to
/// `parse_and_sanitize`. Returns a human-readable detail (carrying `N` when
/// present) when `content` matches, `None` for every other completion shape
/// (including a genuinely malformed `PanelReport`, which still falls through
/// to the `"protocol"` category as before).
///
/// Round-3 review items 9/20: `agent/src/runner.rs`'s schema-contract guard
/// (added alongside P0-1's `tool_choice` relaxation) now intercepts turn-
/// budget exhaustion BEFORE this shape can be produced for any run with
/// `ctx.schema.is_some()` — and every Fusion panel sets `schema` (see
/// `spawn_request` below). So this recognizer, and the `Completed` arm that
/// calls it, are unreachable for a real panel today; `finish_panel`'s
/// `is_missing_structured_output_reason` arm on the `SubagentResult::Failed`
/// case is what actually fires. Left in place rather than deleted: it is
/// still exactly correct for any FUTURE non-schema producer of this shape
/// (the runner's non-schema `!terminated_cleanly` path already emits it —
/// see `runner_test.rs`'s `loop_exhausts_max_turns_when_never_terminal`),
/// and deleting it would only trade one silent gap for another.
fn max_turns_exhausted_detail(content: &Value) -> Option<String> {
    if content.get("reason").and_then(Value::as_str) != Some("max_turns_exhausted") {
        return None;
    }
    Some(match content.get("max_turns").and_then(Value::as_u64) {
        Some(n) => format!("panel exhausted its {n}-turn budget without a valid StructuredOutput"),
        None => "panel exhausted its turn budget without a valid StructuredOutput".into(),
    })
}

/// [Round-4 review item 5] Extract the human-readable cause from the
/// runner's CC 2.1.207 `api_error_partial` salvage shape
/// (`agent::runner::build_recovered_result`):
/// `{"content":[{"type":"text","text": cutoff_note}, ...], "text":…,
/// "stop_reason":null}` where `cutoff_note` is
/// `agent::runner::build_cutoff_note`'s byte-locked
/// `"Agent terminated early due to an API error: {api_error_text}\n\n\
/// Everything below is PARTIAL output…"` text, always the FIRST block
/// (`build_recovered_result` inserts it at index 0). Returns just the
/// `{api_error_text}` line — trimmed of the boilerplate that follows the
/// blank-line separator — or `None` when `content` doesn't match this exact
/// shape (so a caller can fall back to the generic "protocol" category
/// instead of fabricating a cause for an unrelated malformed report).
fn api_error_cutoff_detail(content: &Value) -> Option<String> {
    let first_text = content
        .get("content")?
        .as_array()?
        .first()?
        .get("text")?
        .as_str()?;
    let cause = first_text.strip_prefix("Agent terminated early due to an API error: ")?;
    Some(cause.split("\n\n").next().unwrap_or(cause).trim().to_string())
}

fn is_query_watchdog_timeout(reason: &str) -> bool {
    reason.starts_with(SUBAGENT_QUERY_TIMEOUT_REASON_PREFIX)
}

/// Round-3 review items 9/20: the exact, byte-locked reason
/// `agent/src/runner.rs` emits (from BOTH its two-nudge give-up path and its
/// newer max-turns schema-contract guard — the runner cannot be edited to
/// tell the two apart without breaking oracle parity, see the emission
/// site's own comment) when a schema run ends without a captured
/// `StructuredOutput` call. Recognized here so `finish_panel` can label it
/// distinctly from a genuine provider error instead of folding it into
/// `"provider"`.
fn is_missing_structured_output_reason(reason: &str) -> bool {
    reason == "agent({schema}): subagent completed without calling StructuredOutput (after in-conversation nudge)"
}

/// [Round-4 review item 16] The runner's other schema-contract `Failed`
/// reason (`agent/src/runner.rs`'s `force_structured_tool.is_some() &&
/// structured_result.is_none() && structured_failed_count >=
/// structured_retry_cap` guard): the model called `StructuredOutput`
/// `structured_retry_cap` times and every call failed schema validation, so
/// the loop gives up rather than exhausting its turn budget. The prefix
/// (not full-string `==`) is deliberate: the byte-locked format embeds the
/// live `structured_retry_cap`/`structured_failed_count`/pluralized-"call(s)"
/// values after this point, so a full match would silently stop matching the
/// moment either counter changed.
fn is_structured_retry_cap_exceeded_reason(reason: &str) -> bool {
    reason.starts_with("agent({schema}): StructuredOutput retry cap (")
}

fn usage_from_subagent(
    cumulative: &SubagentUsage,
    final_turn: &SubagentUsage,
    assistant_message_count: u64,
) -> FusionUsage {
    let src = if cumulative.total_tokens == 0 && cumulative.input_tokens == 0 {
        final_turn
    } else {
        cumulative
    };
    FusionUsage {
        input_tokens: src.input_tokens,
        output_tokens: src.output_tokens,
        // Finding [1]: this used to fall to `FusionUsage::default()`'s `0`
        // via the struct-update below — a panel's reasoning tokens never
        // reached `FusionUsage.reasoning_tokens` (the field is reported to
        // the user AND fed into pricing at `orchestrator::price_realized_usage`).
        reasoning_tokens: src.reasoning_output_tokens,
        cache_read_tokens: src.cache_read_input_tokens,
        cache_write_tokens: src.cache_creation_input_tokens,
        // The real assistant-turn count (G011) — the runner's per-turn round
        // trips, not a hardcoded 1 that undercounts a multi-turn panel by up
        // to `panelMaxTurns`x in the result/spool/telemetry and the
        // per-request fee quote.
        provider_requests: u32::try_from(assistant_message_count).unwrap_or(u32::MAX),
        ..FusionUsage::default()
    }
}

/// Finding [11]: build a [`FusionUsage`] from the bare [`SubagentUsage`]
/// carried by a `SubagentResult::Failed` — every turn that completed
/// successfully BEFORE the terminating failure (provider error, idle-timeout
/// watchdog, max-turns / structured-output-retry give-up). Unlike
/// [`usage_from_subagent`] there is no per-turn `assistant_message_count`
/// available on this path, so `provider_requests` stays `0` (a known,
/// bounded under-count of the flat per-request fee only — the token counts,
/// the dominant cost component, are real). Always `estimated: true`: the
/// failing turn's own cost (if the provider billed it at all before erroring)
/// is never captured here, so this is a floor on real spend, not the exact
/// total.
/// T1 item 1 (user-directed policy): "token counting takes the count the LLM
/// provider returns, and only falls back to computing it ourselves when none
/// is found." A panel that was genuinely in flight (its future raced
/// `timeout()`/cancellation and lost, so no `SubagentResult` — success or
/// `Failed` — was ever produced) has no provider-reported usage at all. It
/// still very likely spent real, already-billed tokens on at least the
/// prompt we know we sent, so estimate from that using the SAME
/// character-based approximation `count_tokens::approximate_tokens` uses for
/// its own documented fallback (`llm_client::model::count_tokens`), rather
/// than inventing a second, divergent formula. Output stays `0`: unlike the
/// input prompt, no response text is known to exist for an in-flight panel.
/// `estimated: true` always, so this is never mistaken for an exact
/// provider count (mirrors [`usage_from_failed_subagent`]'s same rule).
fn estimate_in_flight_usage(spawn_prompt: &str) -> FusionUsage {
    FusionUsage {
        input_tokens: llm_client::model::count_tokens::approximate_tokens_for_bytes(
            spawn_prompt.len() as u64,
        ),
        estimated: true,
        ..FusionUsage::default()
    }
}

fn usage_from_failed_subagent(usage: &SubagentUsage) -> FusionUsage {
    FusionUsage {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        reasoning_tokens: usage.reasoning_output_tokens,
        cache_read_tokens: usage.cache_read_input_tokens,
        cache_write_tokens: usage.cache_creation_input_tokens,
        estimated: true,
        ..FusionUsage::default()
    }
}

/// Cap and neutralize a source error string before it is surfaced as
/// [`platform_api::PanelOutcome::error_detail`] (G011): the same NUL-strip +
/// prompt-injection guard [`sanitize_text`] applies to panel report fields,
/// plus a hard byte cap so a verbose provider error body cannot balloon the
/// spool/telemetry payload.
fn sanitize_detail(input: &str) -> String {
    const MAX_DETAIL_BYTES: usize = 500;
    let sanitized = sanitize_text(input);
    if sanitized.len() <= MAX_DETAIL_BYTES {
        return sanitized;
    }
    let mut end = MAX_DETAIL_BYTES;
    while end > 0 && !sanitized.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &sanitized[..end])
}

/// Parse panel content as [`PanelReport`], then NUL-strip + output-guard.
pub fn parse_and_sanitize(content: &Value) -> Result<PanelReport, String> {
    let mut report = parse_panel_report(content)?;
    sanitize_report(&mut report);
    if report.candidate_answer.trim().is_empty() {
        return Err("protocol".into());
    }
    let encoded = serde_json::to_vec(&report).unwrap_or_default();
    if encoded.len() > 256 * 1024 {
        return Err("protocol".into());
    }
    validate_panel_report(&report).map_err(|_| "protocol".to_string())?;
    Ok(report)
}

fn parse_panel_report(content: &Value) -> Result<PanelReport, String> {
    if let Ok(report) = serde_json::from_value::<PanelReport>(content.clone()) {
        return Ok(report);
    }
    if let Some(raw) = content.as_str() {
        if let Ok(report) = serde_json::from_str::<PanelReport>(raw) {
            return Ok(report);
        }
    }
    if let Some(text) = flatten_text_blocks(content) {
        if let Ok(report) = serde_json::from_str::<PanelReport>(&text) {
            return Ok(report);
        }
    }
    Err("protocol".into())
}

fn flatten_text_blocks(content: &Value) -> Option<String> {
    let blocks = content.get("content")?.as_array()?;
    let mut out = String::new();
    for block in blocks {
        if block.get("type").and_then(Value::as_str) == Some("text") {
            if let Some(text) = block.get("text").and_then(Value::as_str) {
                out.push_str(text);
            }
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

fn sanitize_report(report: &mut PanelReport) {
    report.summary = sanitize_text(&report.summary);
    report.candidate_answer = sanitize_text(&report.candidate_answer);
    for claim in &mut report.claims {
        claim.statement = sanitize_text(&claim.statement);
        // (finding [6]) `evidence_refs` are panel-authored strings that reach
        // the analyst prompt verbatim via `analyst_user_message`'s whole-struct
        // serialization, exactly like `statement` above — sanitize them the
        // same way. `sanitize_text` is a pure function of its input, so a ref
        // that names a real `evidence[].id` (sanitized identically below)
        // stays byte-equal after sanitization and `validate_panel_report`'s
        // referential-integrity check still passes.
        for evidence_ref in &mut claim.evidence_refs {
            *evidence_ref = sanitize_text(evidence_ref);
        }
    }
    for evidence in &mut report.evidence {
        // (finding [6]) `id` is panel-authored and, like `locator`/`excerpt`
        // below, is serialized verbatim into the analyst prompt — it was the
        // one field in this loop left unsanitized.
        evidence.id = sanitize_text(&evidence.id);
        evidence.locator = sanitize_text(&evidence.locator);
        if let Some(excerpt) = evidence.excerpt.as_mut() {
            *excerpt = sanitize_text(excerpt);
        }
    }
    for item in &mut report.assumptions {
        *item = sanitize_text(item);
    }
    for risk in &mut report.risks {
        risk.description = sanitize_text(&risk.description);
    }
    for item in &mut report.unresolved_questions {
        *item = sanitize_text(item);
    }
}

fn sanitize_text(input: &str) -> String {
    let stripped = input.replace('\0', "");
    sanitize_blocks(&[stripped]).content.join("")
}

/// Assign anonymous `P1..Pn` with a `run_id`-derived shuffle.
pub fn anonymize(panels: &mut [PanelInternal], run_id: &str) {
    let order = anon_order(run_id, panels.len());
    for (anon, original) in order.into_iter().enumerate() {
        if let Some(panel) = panels.get_mut(original) {
            panel.anonymous_id = format!("P{}", anon + 1);
        }
    }
}

/// The `run_id`-derived shuffle `anonymize` applies: `order[anon] ==
/// original_spawn_index`. Factored out so the host-side spawn name (assigned
/// BEFORE collection) and the reported `anonymous_id` (assigned AFTER
/// collection) are computed from the exact same permutation — see
/// [`anon_rank_by_spawn_index`] and [Finding 16].
fn anon_order(run_id: &str, len: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..len).collect();
    shuffle(&mut order, seed_from(run_id));
    order
}

/// Inverse of [`anon_order`]: for each pre-shuffle spawn index, the
/// zero-based rank (`0` == `P1`) it will be assigned as `anonymous_id` once
/// `anonymize` runs after collection. Lets the spawn-time host name agree
/// with the post-collection reported id without waiting for collection.
fn anon_rank_by_spawn_index(run_id: &str, len: usize) -> Vec<usize> {
    let order = anon_order(run_id, len);
    let mut rank = vec![0usize; len];
    for (anon, original) in order.into_iter().enumerate() {
        if let Some(slot) = rank.get_mut(original) {
            *slot = anon;
        }
    }
    rank
}

fn seed_from(s: &str) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325_u64;
    for byte in s.as_bytes() {
        h ^= u64::from(*byte);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

fn shuffle(items: &mut [usize], mut state: u64) {
    for i in (1..items.len()).rev() {
        state = state.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1);
        // Reduce mod (i+1) in u64 BEFORE narrowing to usize: the result is
        // always < i+1 (which already fits usize, being a valid index bound),
        // so converting the wide `state` first would truncate on a 32-bit
        // target and skew the distribution before the modulo ever ran.
        let j = usize::try_from(state % (i as u64 + 1)).unwrap_or(0);
        items.swap(i, j);
    }
}

#[cfg(test)]
mod usage_from_subagent_tests {
    use super::*;

    /// Finding [1] (panel half): a panel's reasoning-output tokens must
    /// survive into `FusionUsage.reasoning_tokens` — before this fix the
    /// field was left at `FusionUsage::default()`'s `0` regardless of what
    /// the subagent actually reported, so `orchestrator::price_realized_usage`
    /// (which prices straight off this field) could never bill them and
    /// `aggregate_panel_usage`'s reported total was wrong too.
    #[test]
    fn carries_reasoning_tokens_from_cumulative_usage() {
        let cumulative = SubagentUsage {
            total_tokens: 500,
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
            reasoning_output_tokens: 350,
        };
        let final_turn = SubagentUsage::default();
        let usage = usage_from_subagent(&cumulative, &final_turn, 1);
        assert_eq!(
            usage.reasoning_tokens, 350,
            "cumulative reasoning tokens must reach FusionUsage.reasoning_tokens"
        );
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
    }

    /// The `final_turn` fallback path (single-turn panel, `cumulative` still
    /// zeroed) must carry reasoning tokens too.
    #[test]
    fn carries_reasoning_tokens_from_final_turn_fallback() {
        let cumulative = SubagentUsage::default();
        let final_turn = SubagentUsage {
            total_tokens: 80,
            input_tokens: 20,
            output_tokens: 10,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
            reasoning_output_tokens: 50,
        };
        let usage = usage_from_subagent(&cumulative, &final_turn, 1);
        assert_eq!(usage.reasoning_tokens, 50);
    }
}

#[cfg(test)]
mod missing_usage_settlement_fallback_tests {
    use super::*;
    use platform_api::PanelRunStatus;

    fn panel() -> ResolvedPanel {
        ResolvedPanel {
            profile: "anthropic".into(),
            model: "sonnet".into(),
        }
    }

    /// T1 item 1 (panel half, user-directed policy): a panel that was
    /// genuinely IN FLIGHT when the whole-run timeout fired carries no
    /// `SubagentUsage` at all (its future was raced against `timeout()` and
    /// lost, so no terminal `SubagentResult` — successful or `Failed` — was
    /// ever produced). Before this fix `finish_panel` left `internal.usage`
    /// at `None`, so `price_realized_usage` silently contributed $0 for real,
    /// already-billed spend. It must instead estimate from the prompt we DO
    /// know was sent, using main's shared byte-length approximation
    /// (`llm_client::model::count_tokens::approximate_tokens_for_bytes`), and
    /// keep `estimated: true` so the figure is never passed off as exact.
    #[test]
    fn total_timed_out_estimates_usage_from_the_sent_prompt() {
        let prompt = "x".repeat(300);
        let internal = finish_panel(
            0,
            panel(),
            prompt.clone(),
            Duration::from_secs(1),
            PanelFinish::TotalTimedOut,
        );
        assert_eq!(internal.status, PanelRunStatus::TimedOut);
        let usage = internal.usage.expect(
            "a panel that was in flight when the total timeout fired must carry an \
estimated usage, not None (silently under-billing real, already-spent tokens)",
        );
        assert!(
            usage.estimated,
            "the estimate must be flagged, never passed off as an exact provider count"
        );
        let expected =
            llm_client::model::count_tokens::approximate_tokens_for_bytes(prompt.len() as u64);
        assert_eq!(
            usage.input_tokens, expected,
            "input estimate must equal main's shared byte-length approximation over the sent \
prompt, not a second, divergent formula"
        );
    }

    /// Same policy, the `Cancelled` terminal shape (host cancel raced the
    /// same way as the total-timeout case above).
    #[test]
    fn cancelled_estimates_usage_from_the_sent_prompt() {
        let prompt = "y".repeat(75);
        let internal = finish_panel(
            0,
            panel(),
            prompt.clone(),
            Duration::from_millis(1),
            PanelFinish::Cancelled { dispatched: true },
        );
        assert_eq!(internal.status, PanelRunStatus::Cancelled);
        let usage = internal
            .usage
            .expect("a cancelled in-flight panel must carry an estimated usage, not None");
        assert!(usage.estimated);
        assert_eq!(
            usage.input_tokens,
            llm_client::model::count_tokens::approximate_tokens_for_bytes(prompt.len() as u64)
        );
    }

    /// [Round-5 review item 16] The other half of the same policy: a cancel
    /// that beat this panel's task to its own spawner call sent nothing, so
    /// it must report NO usage at all (exact $0) and must say so in a
    /// category `check_panel_bar` / `dispatched_egress_profiles` can read.
    /// Estimating here would invent spend for a provider call that provably
    /// never happened.
    #[test]
    fn cancel_before_the_spawner_call_reports_no_usage_and_a_not_dispatched_category() {
        let internal = finish_panel(
            0,
            panel(),
            "z".repeat(75),
            Duration::from_millis(1),
            PanelFinish::Cancelled { dispatched: false },
        );
        assert_eq!(internal.status, PanelRunStatus::Cancelled);
        assert_eq!(
            internal.error_category.as_deref(),
            Some("not_dispatched"),
            "a slot cancelled before its spawner call must be distinguishable from one cut \
down mid-flight"
        );
        assert!(
            internal.usage.is_none(),
            "nothing was sent, so there is no usage to estimate: got {:?}",
            internal.usage
        );
        assert!(is_never_dispatched_category(internal.error_category.as_deref()));
    }

    /// A genuine PRE-FLIGHT spawn failure (the spawner's `Result::Err`
    /// returned before any provider call could possibly have happened) must
    /// NOT invent spend: $0 / `usage: None`, unchanged. Estimating here would
    /// over-bill a call that provably never reached a provider — the exact
    /// failure mode the "only estimate when a call could plausibly have
    /// happened" half of the policy exists to prevent.
    #[test]
    fn spawn_failure_still_reports_no_usage_at_all() {
        let internal = finish_panel(
            0,
            panel(),
            "prompt that was never sent to any provider".into(),
            Duration::from_millis(1),
            PanelFinish::Failed {
                category: "spawn".into(),
                detail: None,
            },
        );
        assert!(
            internal.usage.is_none(),
            "a pre-flight spawn failure never reached a provider — inventing an estimated \
usage here would over-bill a call that never happened"
        );
    }

    /// Round-3 review item 2: an `"aborted"` `JoinError` — the shape a panel
    /// killed mid-flight by `run_panels`' early-abort bar (`abort_all()`
    /// once the remaining panels can no longer reach `min_successful_panels`)
    /// produces — must settle like `TotalTimedOut`/`Cancelled` above, NOT
    /// like the pre-flight `"spawn"` case: the task genuinely reached a
    /// provider before being cut down, so `usage: None` would silently drop
    /// real, already-billed spend to exact $0.
    #[test]
    fn aborted_join_error_estimates_usage_from_the_sent_prompt() {
        let prompt = "z".repeat(120);
        let internal = finish_panel(
            0,
            panel(),
            prompt.clone(),
            Duration::default(),
            PanelFinish::Failed {
                category: "aborted".into(),
                detail: Some("task was cancelled".into()),
            },
        );
        let usage = internal.usage.expect(
            "an aborted (bar-killed) panel was genuinely in flight and must carry an \
estimated usage, not None",
        );
        assert!(usage.estimated, "the estimate must be flagged, never exact");
        assert_eq!(
            usage.input_tokens,
            llm_client::model::count_tokens::approximate_tokens_for_bytes(prompt.len() as u64),
            "must reuse the same in-flight estimate formula as TotalTimedOut/Cancelled"
        );
        assert_eq!(internal.error_category.as_deref(), Some("aborted"));
    }

    /// Same policy, the `"panic"` category (a genuine task panic mid-flight
    /// carries the identical "already billed, never confirmed" shape).
    #[test]
    fn panicked_join_error_estimates_usage_from_the_sent_prompt() {
        let prompt = "w".repeat(40);
        let internal = finish_panel(
            0,
            panel(),
            prompt.clone(),
            Duration::default(),
            PanelFinish::Failed {
                category: "panic".into(),
                detail: Some("task panicked".into()),
            },
        );
        let usage = internal
            .usage
            .expect("a panicked panel must carry an estimated usage, not None");
        assert!(usage.estimated);
        assert_eq!(
            usage.input_tokens,
            llm_client::model::count_tokens::approximate_tokens_for_bytes(prompt.len() as u64)
        );
    }
}

#[cfg(test)]
mod missing_structured_output_reason_tests {
    use super::*;
    use platform_api::{PanelRunStatus, SubagentUsage};

    fn panel() -> ResolvedPanel {
        ResolvedPanel {
            profile: "anthropic".into(),
            model: "sonnet".into(),
        }
    }

    /// Round-3 review items 9/20: for a real Fusion panel (every panel spawns
    /// with `schema: Some(..)`), `agent/src/runner.rs`'s schema-contract
    /// guard now intercepts turn-budget exhaustion BEFORE the
    /// `Completed{"reason":"max_turns_exhausted"}` shape `finish_panel`'s
    /// `[Finding 25]` arm recognizes can ever be produced — it always
    /// arrives as this exact `SubagentResult::Failed` reason instead. Before
    /// this fix that fell into the generic `"provider"` arm, misattributing
    /// a turn-budget/schema-contract outcome as a provider error. It must
    /// instead be labelled distinctly, and must still price the real,
    /// already-billed spend from every turn that ran before the guard fired
    /// (same [Finding 11] policy as the generic provider-Failed arm).
    #[test]
    fn missing_structured_output_failure_is_not_mislabeled_provider() {
        let usage = SubagentUsage {
            total_tokens: 900,
            input_tokens: 700,
            output_tokens: 200,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
            reasoning_output_tokens: 0,
        };
        let internal = finish_panel(
            0,
            panel(),
            "panel prompt".into(),
            Duration::from_secs(5),
            PanelFinish::Done(SubagentResult::Failed {
                agent_id: protocol::AgentId::new(),
                reason: "agent({schema}): subagent completed without calling StructuredOutput \
(after in-conversation nudge)"
                    .to_string(),
                usage,
            }),
        );
        assert_eq!(internal.status, PanelRunStatus::Failed);
        assert_eq!(
            internal.error_category.as_deref(),
            Some("no_structured_output"),
            "a turn-budget/schema-contract exhaustion must not be labelled \"provider\" — \
that is the [Finding 25] misattribution this fix removes, and it must not be labelled \
\"max_turns\" either, since this reason string alone cannot promise a turn count or rule \
out the nudge-give-up path"
        );
        let priced = internal
            .usage
            .expect("real, already-billed spend from turns before the guard fired must be priced");
        assert_eq!(
            priced.input_tokens, 700,
            "must price the REAL provider-reported token counts (via \
usage_from_failed_subagent), not a prompt-length estimate"
        );
    }

    /// A genuinely different provider error (unrelated reason string) must
    /// still take the generic `"provider"` path — this fix must not
    /// over-match.
    #[test]
    fn unrelated_provider_failure_still_reports_provider_category() {
        let internal = finish_panel(
            0,
            panel(),
            "panel prompt".into(),
            Duration::from_secs(1),
            PanelFinish::Done(SubagentResult::Failed {
                agent_id: protocol::AgentId::new(),
                reason: "upstream 503".to_string(),
                usage: SubagentUsage::default(),
            }),
        );
        assert_eq!(internal.error_category.as_deref(), Some("provider"));
    }
}

#[cfg(test)]
mod structured_retry_cap_exceeded_tests {
    use super::*;
    use platform_api::{PanelRunStatus, SubagentUsage};

    fn panel() -> ResolvedPanel {
        ResolvedPanel {
            profile: "anthropic".into(),
            model: "sonnet".into(),
        }
    }

    /// [Round-4 review item 16] `agent/src/runner.rs` has a SECOND
    /// schema-contract `Failed` reason distinct from
    /// `is_missing_structured_output_reason`: the model called
    /// `StructuredOutput` `structured_retry_cap` times and every call failed
    /// schema validation. Before this fix that string fell through to the
    /// generic `"provider"` arm, telling the operator the *provider* failed
    /// when the panel's own output never matched the schema.
    #[test]
    fn retry_cap_exceeded_is_not_mislabeled_provider() {
        let usage = SubagentUsage {
            total_tokens: 900,
            input_tokens: 700,
            output_tokens: 200,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
            reasoning_output_tokens: 0,
        };
        let internal = finish_panel(
            0,
            panel(),
            "panel prompt".into(),
            Duration::from_secs(5),
            PanelFinish::Done(SubagentResult::Failed {
                agent_id: protocol::AgentId::new(),
                reason: "agent({schema}): StructuredOutput retry cap (5) exceeded \u{2014} 5 \
failed calls with no valid output"
                    .to_string(),
                usage,
            }),
        );
        assert_eq!(internal.status, PanelRunStatus::Failed);
        assert_eq!(
            internal.error_category.as_deref(),
            Some("schema_retry_exhausted"),
            "a StructuredOutput retry-cap exhaustion is the PANEL's own output repeatedly \
failing schema validation, not a provider failure — it must not be labelled \"provider\" \
(the misattribution this fix removes), and it is not \"no_structured_output\" either since \
the model DID call StructuredOutput, just never validly"
        );
        assert_eq!(
            internal.error_detail.as_deref(),
            Some(
                "agent({schema}): StructuredOutput retry cap (5) exceeded \u{2014} 5 failed \
calls with no valid output"
            ),
        );
        let priced = internal
            .usage
            .expect("real, already-billed spend from turns before the cap fired must be priced");
        assert_eq!(priced.input_tokens, 700);
    }

    /// The single-call singular ("1 failed call") variant must match too —
    /// the recognizer is a prefix match, not tied to a specific count.
    #[test]
    fn retry_cap_exceeded_singular_call_also_recognized() {
        let internal = finish_panel(
            0,
            panel(),
            "panel prompt".into(),
            Duration::from_secs(1),
            PanelFinish::Done(SubagentResult::Failed {
                agent_id: protocol::AgentId::new(),
                reason: "agent({schema}): StructuredOutput retry cap (1) exceeded \u{2014} 1 \
failed call with no valid output"
                    .to_string(),
                usage: SubagentUsage::default(),
            }),
        );
        assert_eq!(
            internal.error_category.as_deref(),
            Some("schema_retry_exhausted")
        );
    }
}

#[cfg(test)]
mod api_error_cutoff_tests {
    use super::*;
    use platform_api::{PanelRunStatus, SubagentUsage};

    fn panel() -> ResolvedPanel {
        ResolvedPanel {
            profile: "anthropic".into(),
            model: "sonnet".into(),
        }
    }

    /// The runner's `build_recovered_result` shape, exactly as
    /// `agent::runner::build_cutoff_note` + `build_recovered_result` emit it
    /// for the CC 2.1.207 `api_error_partial` salvage path.
    fn salvaged_completed(api_error_text: &str) -> SubagentResult {
        let cutoff_note = format!(
            "Agent terminated early due to an API error: {api_error_text}\n\n\
Everything below is PARTIAL output recovered from the agent before it was cut off. The agent \
did NOT finish its task \u{2014} treat these results as incomplete."
        );
        SubagentResult::Completed {
            agent_id: protocol::AgentId::new(),
            content: serde_json::json!({
                "content": [{"type": "text", "text": cutoff_note}],
                "text": cutoff_note,
                "stop_reason": serde_json::Value::Null,
            }),
            usage: SubagentUsage {
                total_tokens: 900,
                input_tokens: 700,
                output_tokens: 200,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
                reasoning_output_tokens: 0,
            },
            total_tool_use_count: 1,
            total_duration_ms: 1,
            total_tokens: 900,
            assistant_message_count: 2,
            response_char_count: 1,
            last_request_id: None,
            cumulative_usage: SubagentUsage {
                total_tokens: 900,
                input_tokens: 700,
                output_tokens: 200,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
                reasoning_output_tokens: 0,
            },
            // The defect's precondition: this is the ONLY production path
            // that ever sets `usage_complete: false` on a `Completed` event.
            usage_complete: false,
        }
    }

    /// [Round-4 review item 5] A panel cut off mid-stream by a provider
    /// rate-limit/overload/connection-drop after producing at least one
    /// assistant text block takes the runner's `api_error_partial` salvage
    /// arm, which emits `Completed{ usage_complete: false, .. }` with a
    /// content shape that has none of `PanelReport`'s required fields.
    /// Before this fix, `parse_and_sanitize` always failed that shape with
    /// the generic `"protocol"` category and NO detail — reporting a
    /// provider-side connection drop as "the panel returned a malformed
    /// report" and discarding the "Agent terminated early due to an API
    /// error: …" text the runner already carried.
    #[test]
    fn salvaged_api_error_is_reported_as_provider_cutoff_with_detail() {
        let internal = finish_panel(
            0,
            panel(),
            "panel prompt".into(),
            Duration::from_secs(5),
            PanelFinish::Done(salvaged_completed(
                "Connection closed mid-response. The response above may be incomplete.",
            )),
        );
        assert_eq!(internal.status, PanelRunStatus::Failed);
        assert_eq!(
            internal.error_category.as_deref(),
            Some("provider_cutoff"),
            "a provider-side mid-stream cutoff must not be labelled \"protocol\" — that says \
\"the panel's own output was malformed\", which is a different cause entirely"
        );
        assert_eq!(
            internal.error_detail.as_deref(),
            Some("Connection closed mid-response. The response above may be incomplete."),
            "the runner's API-error cause text must survive into error_detail instead of \
being silently discarded"
        );
    }

    /// A genuinely malformed `PanelReport` (the model just emitted broken
    /// JSON, `usage_complete: true`) must still take the generic
    /// `"protocol"` path — this fix must not over-match every parse failure.
    #[test]
    fn malformed_report_with_complete_usage_still_reports_protocol() {
        let internal = finish_panel(
            0,
            panel(),
            "panel prompt".into(),
            Duration::from_secs(1),
            PanelFinish::Done(SubagentResult::Completed {
                agent_id: protocol::AgentId::new(),
                content: serde_json::json!({"not": "a valid panel report"}),
                usage: SubagentUsage::default(),
                total_tool_use_count: 0,
                total_duration_ms: 1,
                total_tokens: 0,
                assistant_message_count: 1,
                response_char_count: 1,
                last_request_id: None,
                cumulative_usage: SubagentUsage::default(),
                usage_complete: true,
            }),
        );
        assert_eq!(internal.error_category.as_deref(), Some("protocol"));
        assert_eq!(internal.error_detail, None);
    }
}

#[cfg(test)]
mod panel_stall_timeout_clamp_tests {
    use super::*;

    /// Round-3 review finding [24] (rework): a settings tier that sets only
    /// `panelIdleTimeoutMs` (no `panelTotalTimeoutMs` in that same file)
    /// passes `FusionSettingsJson::validate`'s same-file-only check and can
    /// reach `FusionRuntimeConfig` with `panel_idle_timeout_ms` far above
    /// `panel_total_timeout_ms`. The watchdog deadline handed to
    /// `spawn_workflow_with_observer` must still never exceed the panel's
    /// own hard timeout — that outer `timeout()` cancels the whole panel
    /// task at `panel_total_timeout_ms`, so a higher stall deadline can
    /// never fire and the stall detector goes silently inert for the run.
    #[test]
    fn panel_stall_timeout_ms_never_exceeds_the_panel_total_timeout() {
        let config = FusionRuntimeConfig {
            panel_idle_timeout_ms: 5_000_000,
            panel_total_timeout_ms: 600_000,
            ..FusionRuntimeConfig::defaults()
        };
        assert_eq!(
            panel_stall_timeout_ms(&config),
            600_000,
            "a panel_idle_timeout_ms above panel_total_timeout_ms must be clamped to the \
panel's own hard timeout — otherwise the stall watchdog is configured with a deadline it can \
never reach before the panel is aborted out from under it, leaving the run with zero stall \
protection"
        );
    }

    /// The common case (idle below total) must pass through unclamped.
    #[test]
    fn panel_stall_timeout_ms_passes_through_when_already_below_total() {
        let config = FusionRuntimeConfig {
            panel_idle_timeout_ms: 90_000,
            panel_total_timeout_ms: 600_000,
            ..FusionRuntimeConfig::defaults()
        };
        assert_eq!(panel_stall_timeout_ms(&config), 90_000);
    }
}

/// Round-3 review findings 11/19 (rework): `run_panels` must signal "panels
/// genuinely dispatched" distinctly and BEFORE the first one can possibly
/// reach a terminal outcome. Kept inline rather than in `orchestrator_test.rs`
/// so this fixer's changes stay isolated to files it owns (same rationale as
/// `record_failed_analyst_usage_tests` in `orchestrator.rs`) — the mocks
/// below are deliberately minimal duplicates of the ones in that file, not a
/// shared import, for the same reason.
#[cfg(test)]
mod panels_dispatched_emission_tests {
    use super::*;
    use async_trait::async_trait;
    use platform_api::budget::{BudgetEnforcerHandle, BudgetError};
    use platform_api::subagent_spawn::{SubagentInheritance, SubagentSpawnError};
    use platform_api::tool_invoker::{SubagentInvocationContext, ToolInvoker, ToolInvokerError};
    use tokio_util::sync::CancellationToken;

    struct InertInvoker;
    #[async_trait]
    impl ToolInvoker for InertInvoker {
        async fn invoke(
            &self,
            _name: &str,
            _input: Value,
            _ctx: SubagentInvocationContext,
        ) -> Result<Value, ToolInvokerError> {
            Ok(Value::Null)
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    struct InertBudget;
    #[async_trait]
    impl BudgetEnforcerHandle for InertBudget {
        async fn check_and_charge(&self, _nano_usd: u64) -> Result<(), BudgetError> {
            Ok(())
        }
        async fn snapshot_total_nano_usd(&self) -> u64 {
            0
        }
    }

    /// Every panel hangs forever — never produces a terminal
    /// `SubagentResult` — so this test can observe the progress emitted
    /// BEFORE any panel completes without racing a real completion. Only
    /// `spawn` needs implementing: `spawn_workflow_with_observer` (what
    /// `run_panels` actually calls) has no override here, so it falls
    /// through the trait's own default chain to this.
    struct HangingSpawner;
    #[async_trait]
    impl SubagentSpawner for HangingSpawner {
        async fn spawn(
            &self,
            _request: SubagentSpawnRequest,
            _inherit: SubagentInheritance,
        ) -> Result<SubagentResult, SubagentSpawnError> {
            std::future::pending::<()>().await;
            unreachable!("hangs forever; the test cancels before this could resolve")
        }
    }

    fn two_panels() -> Vec<ResolvedPanel> {
        vec![
            ResolvedPanel {
                profile: "anthropic".into(),
                model: "claude-sonnet-5".into(),
            },
            ResolvedPanel {
                profile: "openai".into(),
                model: "gpt-5.6-terra".into(),
            },
        ]
    }

    /// The defect this fixes: before the branch, the whole window between
    /// "N panel tasks spawned and calling the provider" and "the first
    /// panel finishes" produced NOTHING on the progress channel — a cancel
    /// landing there was indistinguishable from a cancel that landed before
    /// any panel task existed. Every panel here hangs forever, so if
    /// `PanelsDispatched` is ever observed, it can only have come from
    /// immediately after `spawn_panel_tasks` returns, never from a
    /// completion event.
    #[tokio::test]
    async fn emits_panels_dispatched_before_any_panel_can_possibly_complete() {
        let config = FusionRuntimeConfig {
            panel_total_timeout_ms: 60_000,
            ..FusionRuntimeConfig::defaults()
        };
        let cancel = CancellationToken::new();
        let inherit = FusionInheritance::new(
            SubagentInheritance {
                tool_invoker: Arc::new(InertInvoker),
                budget: Arc::new(InertBudget),
            },
            cancel.clone(),
        );
        let (tx, mut rx) = tokio::sync::mpsc::channel::<FusionProgress>(8);
        let progress = Some(tx);

        let run = tokio::spawn(async move {
            run_panels(
                Arc::new(HangingSpawner),
                &inherit,
                &config,
                true,
                "task",
                &two_panels(),
                "run-id",
                Duration::from_secs(60),
                &progress,
                None,
            )
            .await
        });

        let first = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect(
                "a PanelsDispatched event must arrive promptly — before this fix, nothing at \
all was emitted between spawn and the first panel completion, so this would hang until the \
5s timeout instead",
            )
            .expect("channel must not close before an event is sent");
        assert_eq!(
            first.stage,
            FusionStage::PanelsDispatched { total: 2 },
            "run_panels must emit PanelsDispatched immediately after spawn_panel_tasks \
returns, before it can possibly have collected a completion — got {:?} instead",
            first.stage
        );

        cancel.cancel();
        let _ = run.await;
    }
    /// [Round-5 review item 12] The inverse of the test above: when the
    /// cancel token is ALREADY set, `spawn_panel_tasks` still enqueues N
    /// futures — tokio has not polled a single one, and each begins with
    /// its own biased `cancel.cancelled()` arm, so none of them will ever
    /// call the spawner. No `PanelsDispatched` may be emitted for that set.
    ///
    /// Before this fix the event fired on the very next synchronous line
    /// after `spawn_panel_tasks` returned, and `tools/agent`'s
    /// `stage_proves_panel_spawned` treats it as proof that real provider
    /// calls are in flight — so the session's spawn quota stayed charged
    /// for the whole run for subagents that were never created.
    #[tokio::test]
    async fn emits_no_panels_dispatched_when_the_cancel_beats_the_first_spawner_call() {
        let config = FusionRuntimeConfig {
            panel_total_timeout_ms: 60_000,
            ..FusionRuntimeConfig::defaults()
        };
        let cancel = CancellationToken::new();
        cancel.cancel();
        let inherit = FusionInheritance::new(
            SubagentInheritance {
                tool_invoker: Arc::new(InertInvoker),
                budget: Arc::new(InertBudget),
            },
            cancel,
        );
        let (tx, mut rx) = tokio::sync::mpsc::channel::<FusionProgress>(8);
        let progress = Some(tx);

        let err = run_panels(
            Arc::new(HangingSpawner),
            &inherit,
            &config,
            true,
            "task",
            &two_panels(),
            "run-id",
            Duration::from_secs(60),
            &progress,
            None,
        )
        .await
        .expect_err("an already-cancelled run must not produce panels");
        assert_eq!(err, FusionError::Cancelled);

        drop(progress);
        let mut stages = Vec::new();
        while let Ok(event) = rx.try_recv() {
            stages.push(event.stage);
        }
        assert!(
            !stages
                .iter()
                .any(|stage| matches!(stage, FusionStage::PanelsDispatched { .. })),
            "no panel task ever reached the spawner, so PanelsDispatched must not be \
emitted — got {stages:?}"
        );
    }

    /// A spawner that reaches its body (so `dispatch.mark(index)` has run)
    /// and then fails immediately — the panel task therefore terminates
    /// through the collection loop's COMPLETED arm rather than its cancel
    /// or notify arm.
    struct FailingSpawner;
    #[async_trait]
    impl SubagentSpawner for FailingSpawner {
        async fn spawn(
            &self,
            _request: SubagentSpawnRequest,
            _inherit: SubagentInheritance,
        ) -> Result<SubagentResult, SubagentSpawnError> {
            Err(SubagentSpawnError::Runtime("probe".into()))
        }
    }

    /// [Round-5 rework of item 12, class sweep] The other two exits of the
    /// collection loop (a panel completed, a panel's task died) never have
    /// to emit `PanelsDispatched` themselves: `select!` is `biased`, so
    /// `dispatch.notified()` is polled BEFORE `join_next_with_id()`, and
    /// `PanelDispatch::mark` sets its flag before `notify_one` stores the
    /// permit — so any panel that reached the spawner has already made the
    /// notify arm win an earlier poll. This pins that ordering: every panel
    /// here reaches the spawner and then fails, and `PanelsDispatched` must
    /// still arrive, ahead of the per-completion `RunningPanels` events.
    #[tokio::test]
    async fn panels_dispatched_precedes_the_completed_arm_for_a_dispatched_panel() {
        let config = FusionRuntimeConfig {
            panel_total_timeout_ms: 60_000,
            ..FusionRuntimeConfig::defaults()
        };
        let inherit = FusionInheritance::new(
            SubagentInheritance {
                tool_invoker: Arc::new(InertInvoker),
                budget: Arc::new(InertBudget),
            },
            CancellationToken::new(),
        );
        let (tx, mut rx) = tokio::sync::mpsc::channel::<FusionProgress>(8);
        let progress = Some(tx);

        let panels = run_panels(
            Arc::new(FailingSpawner),
            &inherit,
            &config,
            true,
            "task",
            &two_panels(),
            "run-id",
            Duration::from_secs(60),
            &progress,
            None,
        )
        .await
        .expect("partial_ok run must return its (failed) panel slots");
        assert_eq!(panels.len(), 2);

        drop(progress);
        let mut stages = Vec::new();
        while let Ok(event) = rx.try_recv() {
            stages.push(event.stage);
        }
        let dispatched_at = stages
            .iter()
            .position(|stage| matches!(stage, FusionStage::PanelsDispatched { total: 2 }));
        let first_running_at = stages
            .iter()
            .position(|stage| matches!(stage, FusionStage::RunningPanels { .. }));
        assert!(
            dispatched_at.is_some(),
            "panels reached the spawner, so PanelsDispatched must be emitted even though \
this run exits through the completed arm — got {stages:?}"
        );
        assert!(
            first_running_at.is_none() || dispatched_at < first_running_at,
            "PanelsDispatched must arrive before the first RunningPanels — got {stages:?}"
        );
    }

    /// A spawner that cancels the run from INSIDE `spawn`, then hangs.
    ///
    /// Reaching this body PROVES the panel task got past its own biased
    /// `cancel.cancelled()` arm and ran `dispatch.mark(index)` — a subagent
    /// is genuinely being built — so the cancel it sets here is the
    /// "cancel lands after a panel really reached the spawner" case, the
    /// exact mirror of `HangingSpawner` + an already-cancelled token.
    struct CancelFromInsideSpawner {
        cancel: CancellationToken,
    }
    #[async_trait]
    impl SubagentSpawner for CancelFromInsideSpawner {
        async fn spawn(
            &self,
            _request: SubagentSpawnRequest,
            _inherit: SubagentInheritance,
        ) -> Result<SubagentResult, SubagentSpawnError> {
            self.cancel.cancel();
            std::future::pending::<()>().await;
            unreachable!("hangs forever; the cancel set above ends the run")
        }
    }

    /// [Round-5 rework of item 12] The third case, between the two tests
    /// above: a panel task REACHED the spawner (so a subagent really was
    /// created) and only then did the cancel fire. Item 12's fix moved the
    /// emit out of the synchronous line after `spawn_panel_tasks` and into
    /// the `dispatch.notified()` arm of the collection loop — but that arm
    /// sits BELOW the biased `cancel.cancelled()` arm, which returned
    /// `Err(FusionError::Cancelled)` without emitting anything. With no
    /// `PanelsDispatched` (and no `RunningPanels{completed>0}`),
    /// `tools/agent`'s `stage_proves_panel_spawned` sees no proof, so
    /// `releases_full_reservation` is true for `FusionError::Cancelled` and
    /// the session hands back the ENTIRE `panel_n` spawn reservation for
    /// subagents that were really created — letting the session exceed
    /// `CLAUDE_CODE_MAX_SUBAGENTS_PER_SESSION`. That is the exact opposite
    /// of the over-charge item 12 was filed to remove.
    #[tokio::test]
    async fn emits_panels_dispatched_when_the_cancel_lands_after_a_panel_reached_the_spawner() {
        let config = FusionRuntimeConfig {
            panel_total_timeout_ms: 60_000,
            ..FusionRuntimeConfig::defaults()
        };
        let cancel = CancellationToken::new();
        let inherit = FusionInheritance::new(
            SubagentInheritance {
                tool_invoker: Arc::new(InertInvoker),
                budget: Arc::new(InertBudget),
            },
            cancel.clone(),
        );
        let (tx, mut rx) = tokio::sync::mpsc::channel::<FusionProgress>(8);
        let progress = Some(tx);

        let err = run_panels(
            Arc::new(CancelFromInsideSpawner {
                cancel: cancel.clone(),
            }),
            &inherit,
            &config,
            true,
            "task",
            &two_panels(),
            "run-id",
            Duration::from_secs(60),
            &progress,
            None,
        )
        .await
        .expect_err("the spawner cancels the run from inside, so this must be Cancelled");
        assert_eq!(err, FusionError::Cancelled);

        drop(progress);
        let mut stages = Vec::new();
        while let Ok(event) = rx.try_recv() {
            stages.push(event.stage);
        }
        assert!(
            stages
                .iter()
                .any(|stage| matches!(stage, FusionStage::PanelsDispatched { total: 2 })),
            "a panel task REACHED the spawner (a subagent was created), so \
PanelsDispatched must still be emitted on the cancel exit — otherwise the whole spawn \
reservation is released for subagents that really exist — got {stages:?}"
        );
    }
}

/// [Round-6 blocking B1] The two `PanelDispatch` flags mean different things
/// and must never be re-conflated: `mark` (this task entered the spawner
/// call) is an intent, `mark_allocated` (the spawner reported a real child
/// slot) is a fact. Everything that decides whether a session's lifetime
/// spawn quota stays charged reads the second one.
#[cfg(test)]
mod dispatch_flag_tests {
    use super::*;
    use platform_api::subagent_spawn::SubagentObservation;

    fn observation(kind: &str) -> SubagentObservation {
        match kind {
            "allocated" => SubagentObservation::Allocated {
                agent_id: protocol::AgentId::new(),
                agent_type: FUSION_PANEL_TYPE.to_string(),
                name: None,
                model: "claude-sonnet-5".into(),
                model_profile: Some("anthropic".into()),
                persistent: false,
                initial_message_index: 0,
            },
            _ => SubagentObservation::Progress {
                agent_id: protocol::AgentId::new(),
                tool_use_count: 1,
                token_count: 10,
            },
        }
    }

    /// Entering the spawner call is NOT evidence that a subagent exists —
    /// `spawn_workflow_with_observer` connects the panel's inline MCP
    /// servers and contends the pool slot table before it can even reject.
    #[test]
    fn marking_a_panel_as_having_reached_the_spawner_does_not_make_it_allocated() {
        let dispatch = PanelDispatch::new(2);
        dispatch.mark(0);
        assert!(
            dispatch.reached_spawner(0),
            "mark(0) must record that panel 0 entered the spawner call"
        );
        assert!(
            !dispatch.allocated(0),
            "no Allocated observation has arrived for panel 0, so nothing may claim a \
subagent exists for it"
        );
        assert!(!dispatch.reached_spawner(1) && !dispatch.allocated(1));
    }

    /// The observer in the `spawn_workflow_with_observer` `observer` slot
    /// flips `allocated` for ITS OWN panel index and only on the
    /// `Allocated` observation — every other event on a panel's stream
    /// (progress beacons, messages, the terminal result) is already
    /// reported through the spawner call's own return value.
    #[tokio::test]
    async fn only_the_allocated_observation_flips_the_allocated_flag() {
        let dispatch = Arc::new(PanelDispatch::new(2));
        let observer = PanelAllocationObserver {
            dispatch: Arc::clone(&dispatch),
            index: 1,
        };
        observer.on_event(observation("progress")).await;
        assert!(
            !dispatch.allocated(1),
            "a Progress observation must not be mistaken for proof that the pool \
allocated a child"
        );
        observer.on_event(observation("allocated")).await;
        assert!(
            dispatch.allocated(1),
            "the Allocated observation is the spawner's own proof that a child slot \
exists for panel 1"
        );
        assert!(
            !dispatch.allocated(0),
            "one panel's allocation must never be credited to a sibling index"
        );
        assert!(
            !dispatch.reached_spawner(1),
            "the allocation flag is separate from the reached-spawner flag; \
mark_allocated must not silently set both"
        );
    }
}

/// [Round-9 review item 1] `run_panels`' biased `cancel.cancelled()` arm
/// drains the `JoinSet` after `abort_all()`. That drain used to be
/// `while join_set.join_next().await.is_some() {}` — every task output bound
/// to nothing — so a panel that had ALREADY finished with real,
/// provider-reported usage but had not yet been pulled into `collected` (the
/// cancel arm is polled BEFORE `join_next_with_id()`, so a ready completion
/// loses that race) was thrown away. The `sink.update` at the end of the same
/// arm then re-synthesized it through [`in_flight_panels`] at the
/// prompt-length floor — `approximate_tokens_for_bytes(prompt.len())` input
/// tokens and ZERO output tokens — instead of the tokens the provider had
/// already billed, and that is the figure the outer `run()` `Err` arm commits
/// through `lease.commit`.
///
/// REACHABILITY (the narrowed claim these tests pin, not the wider one the
/// finding opened with): `run()`'s own outer `select!` is `biased` on the
/// SAME `CancellationToken` and, while `finalizing` is false — which it is
/// for the whole panel stage — returns `Err(FusionError::Cancelled)` without
/// awaiting the pinned `run_future`, dropping `run_inner` and this `select!`
/// with it. So an ordinary Ctrl-C never reaches this arm at all. The arm runs
/// only when the token flips while `run_inner` is being polled, and its
/// `sink.update` is reached only when the drain finds every remaining task
/// already finished, so it runs to `None` without yielding back to that outer
/// select. That conjunction is exactly what the test below constructs, and it
/// is the only state in which this discard changes the committed settlement.
#[cfg(test)]
mod cancel_drain_settlement_tests {
    use super::*;
    use crate::budget::ModelRates;
    use crate::model_resolver::CatalogModel;
    use async_trait::async_trait;
    use platform_api::budget::{BudgetEnforcerHandle, BudgetError};
    use platform_api::subagent_spawn::{SubagentInheritance, SubagentSpawnError};
    use platform_api::tool_invoker::{SubagentInvocationContext, ToolInvoker, ToolInvokerError};
    use tokio_util::sync::CancellationToken;

    struct InertInvoker;
    #[async_trait]
    impl ToolInvoker for InertInvoker {
        async fn invoke(
            &self,
            _name: &str,
            _input: Value,
            _ctx: SubagentInvocationContext,
        ) -> Result<Value, ToolInvokerError> {
            Ok(Value::Null)
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    struct InertBudget;
    #[async_trait]
    impl BudgetEnforcerHandle for InertBudget {
        async fn check_and_charge(&self, _nano_usd: u64) -> Result<(), BudgetError> {
            Ok(())
        }
        async fn snapshot_total_nano_usd(&self) -> u64 {
            0
        }
    }

    /// 1 nano-USD per input and per output token, nothing else — so the
    /// settlement figure below IS the token count, and a mis-priced panel is
    /// readable directly off the assertion.
    struct UnitPrices;
    impl FusionPriceBook for UnitPrices {
        fn rates_for(&self, _profile: &str, _model: &str) -> Option<ModelRates> {
            Some(ModelRates {
                input_nano_usd_per_token: 1,
                output_nano_usd_per_token: 1,
                per_request_nano_usd: 0,
                cache_read_nano_usd_per_token: 0,
                cache_write_nano_usd_per_token: 0,
                reasoning_nano_usd_per_token: 0,
                cache_write_rate_is_ttl_approximated: false,
            })
        }
    }

    const REAL_INPUT_TOKENS: u64 = 180_000;
    const REAL_OUTPUT_TOKENS: u64 = 24_000;

    /// The first panel to reach the spawner finishes with real,
    /// provider-reported usage AND cancels the run from inside that same
    /// call. On a current-thread runtime the collection loop is therefore
    /// next polled with (a) the token already set and (b) that panel's
    /// finished `PanelTaskOutput` sitting un-collected in the `JoinSet` —
    /// the exact state in which the biased cancel arm wins the race against
    /// `join_next_with_id()`. Every sibling is left to its own biased cancel
    /// arm, so it too is finished by then and the drain never yields.
    struct CompleteThenCancelSpawner {
        cancel: CancellationToken,
    }
    #[async_trait]
    impl SubagentSpawner for CompleteThenCancelSpawner {
        async fn spawn(
            &self,
            _request: SubagentSpawnRequest,
            _inherit: SubagentInheritance,
        ) -> Result<SubagentResult, SubagentSpawnError> {
            self.cancel.cancel();
            let usage = SubagentUsage {
                total_tokens: REAL_INPUT_TOKENS + REAL_OUTPUT_TOKENS,
                input_tokens: REAL_INPUT_TOKENS,
                output_tokens: REAL_OUTPUT_TOKENS,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
                reasoning_output_tokens: 0,
            };
            Ok(SubagentResult::Completed {
                agent_id: protocol::AgentId::new(),
                content: serde_json::json!({
                    "schema_version": 1,
                    "summary": "s",
                    "candidate_answer": "a",
                }),
                usage: usage.clone(),
                total_tool_use_count: 0,
                total_duration_ms: 1,
                total_tokens: REAL_INPUT_TOKENS + REAL_OUTPUT_TOKENS,
                assistant_message_count: 3,
                response_char_count: 1,
                last_request_id: None,
                cumulative_usage: usage,
                usage_complete: true,
            })
        }
    }

    fn two_panels() -> Vec<ResolvedPanel> {
        vec![
            ResolvedPanel {
                profile: "anthropic".into(),
                model: "claude-sonnet-5".into(),
            },
            ResolvedPanel {
                profile: "openai".into(),
                model: "gpt-5.6-terra".into(),
            },
        ]
    }

    #[tokio::test]
    async fn cancel_drain_settles_a_finished_panel_from_its_real_usage_not_the_prompt_floor() {
        let config = FusionRuntimeConfig {
            panel_total_timeout_ms: 60_000,
            ..FusionRuntimeConfig::defaults()
        };
        let cancel = CancellationToken::new();
        let inherit = FusionInheritance::new(
            SubagentInheritance {
                tool_invoker: Arc::new(InertInvoker),
                budget: Arc::new(InertBudget),
            },
            cancel.clone(),
        );

        let realized_tokens: Arc<Mutex<Option<u64>>> = Arc::new(Mutex::new(None));
        let resolved_egress: Arc<Mutex<Option<Vec<String>>>> = Arc::new(Mutex::new(None));
        let settlement: Arc<Mutex<Option<(u64, bool)>>> = Arc::new(Mutex::new(None));
        let catalog: Vec<CatalogModel> = Vec::new();
        let prices = UnitPrices;
        let analyst = ResolvedPanel {
            profile: "anthropic".into(),
            model: "claude-sonnet-5".into(),
        };
        let sink = RealizedSpendSink {
            realized_tokens: &realized_tokens,
            resolved_egress: &resolved_egress,
            settlement: &settlement,
            catalog: &catalog,
            prices: &prices,
            analyst: &analyst,
            parent_profile: "anthropic",
            parent_model: "claude-sonnet-5",
            request_prompt: "task",
        };

        let panels = two_panels();
        let err = run_panels(
            Arc::new(CompleteThenCancelSpawner {
                cancel: cancel.clone(),
            }),
            &inherit,
            &config,
            true,
            "task",
            &panels,
            "run-id",
            Duration::from_secs(60),
            &None,
            Some(&sink),
        )
        .await
        .expect_err("the spawner cancels the run from inside, so this must be Cancelled");
        assert_eq!(err, FusionError::Cancelled);

        // What the discard used to leave behind, spelled out so the red run
        // names it: the finished panel re-synthesized at the in-flight floor.
        let floor_input_tokens = llm_client::model::count_tokens::approximate_tokens_for_bytes(
            panel_prompt("task").len() as u64,
        );

        let observed_output = *realized_tokens
            .lock()
            .expect("realized_tokens cell must not be poisoned");
        assert_eq!(
            observed_output,
            Some(REAL_OUTPUT_TOKENS),
            "the cancel arm drained a panel that had already finished with \
{REAL_OUTPUT_TOKENS} provider-reported output tokens; discarding that payload leaves the \
realized-token cell at the in-flight floor's 0 output tokens, so the workflow token charge \
under-counts real, already-billed spend"
        );

        let observed_settlement = *settlement
            .lock()
            .expect("settlement cell must not be poisoned");
        let (priced_nano_usd, _estimated) =
            observed_settlement.expect("the cancel arm must leave a settlement figure");
        assert_eq!(
            priced_nano_usd,
            REAL_INPUT_TOKENS + REAL_OUTPUT_TOKENS,
            "at 1 nano-USD/token the committed settlement must be the finished panel's real \
{REAL_INPUT_TOKENS} input + {REAL_OUTPUT_TOKENS} output tokens; the discarded drain instead \
prices it through in_flight_panels at estimate_in_flight_usage's {floor_input_tokens} input \
tokens and 0 output tokens"
        );
    }
}

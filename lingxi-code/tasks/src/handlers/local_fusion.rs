//! Fusion deliberation task handler.
//!
//! `/fusion` spawns a [`TaskType::LocalFusion`] row, runs
//! [`platform_api::FusionExecutor`] on the engine runtime (never `tokio::spawn`
//! — D17), spools a sanitized [`platform_api::FusionResult`], and publishes one
//! `user_meta` fusion-result envelope through [`platform_api::FusionCompletionSink`].
//! Sink failures must not rewrite the task's terminal status.

use crate::id::TaskType;
use crate::output_manager::TaskOutputManager;
use crate::state::TaskStatus;
use crate::task_trait::{Task, TaskContext, TaskError, TaskHandle, TaskSpawnInput};
use async_trait::async_trait;
use platform_api::{
    BackgroundTaskHandle, BudgetEnforcerHandle, FusionCompletionSink, FusionError, FusionExecutor,
    FusionInheritance, FusionPublicationReceipt, FusionResult, FusionStatus, RuntimeSpawner,
    SubagentInheritance, ToolInvoker,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use tokio::sync::{oneshot, Mutex};
use tokio_util::sync::CancellationToken;

pub use crate::handlers::local_bash::{NoopStatusSink, TaskStatusSink};

const HANDLER_NAME: &str = "local_fusion";

/// Bounded wait `kill`/`drain_pending_kills` give the worker to unwind
/// through its own finalize path (`FusionOrchestrator::run`'s cooperative
/// `cancel.cancelled()` branch — CANCELLED telemetry, `Cancelled` progress,
/// `run_panels`' abort+join, explicit lease release) after firing `cancel`,
/// before falling back to `runtime.cancel` (a hard `JoinHandle::abort()` that
/// would skip all of that — F012). Mirrors `local_workflow`'s
/// `cancel_workflow_worker`.
const FUSION_KILL_COMPLETION_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

struct WorkerCancel {
    handle: BackgroundTaskHandle,
    runtime: Arc<dyn RuntimeSpawner>,
    cancel: CancellationToken,
    /// Worker completion signal, fired (via [`WorkerCompletionSignal`]'s
    /// `Drop`) once the spawned worker future is fully done — naturally, or
    /// forcibly on `runtime.cancel`'s own abort. `kill`/`drain_pending_kills`
    /// wait on this for up to [`FUSION_KILL_COMPLETION_GRACE`] before falling
    /// back to the hard abort, so a cooperative `cancel()` gets a real chance
    /// to run `FusionOrchestrator::run`'s own finalize path (F012).
    completion_rx: StdMutex<Option<oneshot::Receiver<()>>>,
    finalizing: bool,
}

/// Fires its held oneshot on drop, unconditionally — whichever path the
/// worker future exits through (normal completion, the early
/// `may_finalize == false` return, or a forced `abort()` dropping the task
/// mid-poll). Held as the worker future's own first local so its lifetime
/// exactly brackets "the worker is done, one way or another".
struct WorkerCompletionSignal(Option<oneshot::Sender<()>>);

impl WorkerCompletionSignal {
    fn new(tx: oneshot::Sender<()>) -> Self {
        Self(Some(tx))
    }
}

impl Drop for WorkerCompletionSignal {
    fn drop(&mut self) {
        if let Some(tx) = self.0.take() {
            let _ = tx.send(());
        }
    }
}

/// `cancel()` the worker, then wait up to [`FUSION_KILL_COMPLETION_GRACE`] for
/// its own completion signal before falling back to `runtime.cancel` (a hard
/// abort that bypasses `FusionOrchestrator::run`'s cooperative cancel branch —
/// F012). Mirrors `local_workflow::cancel_workflow_worker`.
async fn cancel_fusion_worker(rec: WorkerCancel) -> Result<(), TaskError> {
    rec.cancel.cancel();
    let completion_rx = rec
        .completion_rx
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take();
    let completed = if let Some(completion_rx) = completion_rx {
        tokio::time::timeout(FUSION_KILL_COMPLETION_GRACE, completion_rx)
            .await
            .is_ok()
    } else {
        false
    };
    if !completed {
        rec.runtime
            .cancel(&rec.handle)
            .await
            .map_err(|error| TaskError::Io(error.to_string()))?;
    }
    Ok(())
}

/// Terminal-outcome half of `spawn`'s worker future: write the spool body /
/// error, push the egress+usage summary, flip the task to its terminal
/// status, and (on success) publish the sanitized result envelope. Split out
/// of `spawn` purely to keep that function under the line-count lint — same
/// ordering, same branches, same side effects.
async fn finalize_fusion_outcome(
    outcome: &Result<FusionResult, FusionError>,
    output_manager: &TaskOutputManager,
    worker_spool_path: &std::path::Path,
    worker_task_id: &str,
    status_sink: &Arc<dyn TaskStatusSink>,
    sink: &Arc<dyn FusionCompletionSink>,
    conversation_id: &str,
    // [Finding 14] The last `realized_output_tokens` seen on the progress
    // channel before the run ended in `Err` — `None` when nothing egressed
    // yet (a preflight refusal) or the orchestrator never reported it for
    // this failure shape. Used to give the Err arms a best-effort `<usage>`
    // disclosure instead of the structural silence a bare `None` produces
    // downstream.
    last_realized_output_tokens: Option<u64>,
    // [Round-3 review B2] Same latching as `last_realized_output_tokens`,
    // for the resolved egress profile list — see
    // `platform_api::FusionProgress::egress_profiles`.
    last_egress_profiles: Option<Vec<String>>,
) {
    match outcome {
        Ok(result) => {
            let body =
                serde_json::to_string_pretty(result).unwrap_or_else(|_| result.final_text.clone());
            let _ = output_manager.append(worker_spool_path, &body).await;
            // Write the egress/usage summary BEFORE the terminal status
            // transition (same ordering rule as `set_agent_outcome`): the
            // registry's notification drain is terminal-status-gated, so the
            // reverse order could let a drain observe a completed task whose
            // usage summary hasn't landed yet.
            let usage_summary = platform_api::task_registry::AgentRunUsage {
                // [Round-7 items 5+6] `AgentRunUsage.subagent_tokens` is
                // main-owned and documented as claude-code's `totalTokens`
                // (`platform_api::task_registry::AgentRunUsage`); main's own
                // producer sums the four BILLABLE buckets —
                // `local_agent.rs`'s `bt.input + bt.cache_write +
                // bt.cache_read + bt.output`, mirrored in
                // `agent::handle::subagent_usage_from_llm_usage`. Summing
                // only `input + output` here (as this did) dropped the two
                // cache buckets `FusionUsage` already carries, so a cached
                // multi-panel run reported a small fraction of the tokens an
                // equivalent background-agent run reports under the very same
                // `<subagent_tokens>` tag and the model could not compare the
                // two. `reasoning_tokens` stays OUT on purpose: `local_agent`
                // excludes `bt.reasoning_output` too, and the contract here is
                // to match main's definition of the tag, not to invent a third
                // one.
                subagent_tokens: result
                    .usage
                    .input_tokens
                    .saturating_add(result.usage.cache_write_tokens)
                    .saturating_add(result.usage.cache_read_tokens)
                    .saturating_add(result.usage.output_tokens),
                tool_uses: u64::from(result.usage.provider_requests),
                duration_ms: result.timing.total_ms,
            };
            status_sink
                .set_fusion_egress_and_usage(
                    worker_task_id,
                    result.egress_profiles.clone(),
                    Some(usage_summary),
                )
                .await;
            status_sink
                .set_fusion_publication(worker_task_id, FusionPublicationReceipt::pending())
                .await;
            status_sink
                .finish_fusion_terminal(
                    worker_task_id,
                    result.run_id.clone(),
                    result.final_text.clone(),
                    TaskStatus::Completed,
                )
                .await;
            // Publication is independent from computation. The terminal
            // answer is retained even when the append fails; only the typed
            // receipt decides readiness and the legacy `result_published`
            // compatibility flag.
            let receipt = sink.publish(conversation_id, result).await;
            status_sink
                .set_fusion_publication(worker_task_id, receipt.clone())
                .await;
            if receipt.is_published() {
                // Keep the old narrow hook for standalone sinks and older
                // status adapters; its registry implementation now writes a
                // typed `Published` receipt as well.
                status_sink
                    .mark_fusion_result_published(worker_task_id)
                    .await;
            }
        }
        Err(FusionError::Cancelled) => {
            // [Finding 14] Same ordering rule as the `Ok` arm above: write
            // whatever usage disclosure we have BEFORE the terminal status
            // transition, so the notification drain (terminal-status-gated)
            // never observes a `Killed` task whose partial usage hasn't
            // landed yet.
            disclose_partial_usage(
                status_sink,
                worker_task_id,
                last_realized_output_tokens,
                last_egress_profiles,
            )
            .await;
            status_sink
                .set_status(worker_task_id, TaskStatus::Killed)
                .await;
        }
        Err(err) => {
            let _ = output_manager
                .append(worker_spool_path, &err.to_string())
                .await;
            disclose_partial_usage(
                status_sink,
                worker_task_id,
                last_realized_output_tokens,
                last_egress_profiles,
            )
            .await;
            status_sink
                .set_fusion_error(worker_task_id, err.to_string())
                .await;
            status_sink
                .set_status(worker_task_id, TaskStatus::Failed)
                .await;
        }
    }
}

/// [Finding 14; round-3 review B2 follow-up] Best-effort `<usage>` AND
/// `<egress-profiles>` disclosure for a fusion run that ends in `Err` after
/// real panel spend: `FusionError` carries neither payload, so the progress
/// channel's last-seen `realized_output_tokens` / `egress_profiles`
/// (latched by `run_fusion_worker`'s forwarder — see
/// `platform_api::FusionProgress::egress_profiles`'s doc for where the
/// orchestrator populates it) are the only signals available at this seam.
/// `tool_uses`/`duration_ms` are still unknown too and stay at `0` rather
/// than fabricated. A no-op when nothing ever egressed (e.g. a preflight
/// refusal — both `last_*` arguments are `None`), matching the render arm's
/// existing `usage: None` omission.
///
/// [Round-7 items 5+6] **The `subagent_tokens` written here is deliberately
/// NOT the same quantity the `Ok` arm writes.** The `Ok` arm has the whole
/// `FusionUsage` and sums main's four billable buckets (`input +
/// cache_write + cache_read + output`, matching `local_agent.rs`); this arm
/// has only `FusionProgress::realized_output_tokens` — OUTPUT tokens alone,
/// because that is the single figure the progress channel carries (it exists
/// for the workflow budget bridge, which charges from it). So the value
/// below is a true LOWER BOUND on the run's total, not the total, and is
/// reported that way rather than reporting nothing at all. Widening it would
/// mean widening `fusion::progress::emit_with_realized_tokens` to carry a
/// full realized total alongside the output-only figure — a producer-side
/// change, not one this seam can make.
async fn disclose_partial_usage(
    status_sink: &Arc<dyn TaskStatusSink>,
    worker_task_id: &str,
    last_realized_output_tokens: Option<u64>,
    last_egress_profiles: Option<Vec<String>>,
) {
    if let Some(tokens) = last_realized_output_tokens {
        status_sink
            .set_fusion_egress_and_usage(
                worker_task_id,
                last_egress_profiles.unwrap_or_default(),
                Some(platform_api::task_registry::AgentRunUsage {
                    subagent_tokens: tokens,
                    tool_uses: 0,
                    duration_ms: 0,
                }),
            )
            .await;
    }
}

/// Owned inputs the fusion background worker future needs, collected into a
/// struct so `spawn` can build it in one call instead of moving a dozen
/// separate captures into the closure — the struct itself is what keeps
/// `spawn` under the argument-count and line-count lints.
struct FusionWorkerArgs {
    executor: Arc<dyn FusionExecutor>,
    inherit: FusionInheritance,
    request: platform_api::FusionRequest,
    conversation_id: String,
    sink: Arc<dyn FusionCompletionSink>,
    status_sink: Arc<dyn TaskStatusSink>,
    workers: Arc<Mutex<HashMap<String, WorkerCancel>>>,
    output_manager: Arc<TaskOutputManager>,
    worker_spool_path: std::path::PathBuf,
    worker_task_id: String,
    activation_rx: Option<oneshot::Receiver<()>>,
    /// Held for its `Drop` side effect only — fires the completion signal
    /// unconditionally once this future is done, whichever path it exits
    /// through. Never read.
    #[allow(dead_code)]
    completion_signal: WorkerCompletionSignal,
}

/// `spawn`'s background worker body: run the deliberation, forward its
/// progress into the task DTO's `stage`, and finalize the terminal outcome.
/// Split out of `spawn` (as a plain fn taking owned args, not an inline
/// `async move` closure) purely to keep that function under the line-count
/// lint — same ordering, same finalize race handling, same side effects.
async fn run_fusion_worker(args: FusionWorkerArgs) {
    let FusionWorkerArgs {
        executor,
        inherit,
        request,
        conversation_id,
        sink,
        status_sink,
        workers,
        output_manager,
        worker_spool_path,
        worker_task_id,
        activation_rx,
        completion_signal: _completion_signal,
    } = args;
    if let Some(activation_rx) = activation_rx {
        if activation_rx.await.is_err() {
            workers.lock().await.remove(&worker_task_id);
            return;
        }
    }
    status_sink
        .set_status(&worker_task_id, TaskStatus::Running)
        .await;

    // F005: forward progress into `LocalFusionTaskState.stage` so a client
    // polling the task DTO sees the same `FusionStage::label()` text the
    // Agent-tool path forwards as `subagent_activity` — before this the
    // task carried NO progress at all between `Running` and its terminal
    // status.
    let (prog_tx, mut prog_rx) = tokio::sync::mpsc::channel::<platform_api::FusionProgress>(32);
    let forward_status_sink = status_sink.clone();
    let forward_task_id = worker_task_id.clone();
    // [Finding 14; round-3 review B2 follow-up] Track the LAST
    // `realized_output_tokens` AND `egress_profiles` the orchestrator emits
    // (it does so when a post-fan-out failure follows real panel spend —
    // `fusion::orchestrator::run_inner`'s `check_panel_bar` arm, and on the
    // outer cancel/timeout choke point in `FusionOrchestrator::run`) so a
    // terminal `Err` below can disclose what already egressed instead of
    // the task notification silently omitting `<usage>`/`<egress-profiles>`
    // for a run that really burned tokens and really reached those
    // providers. `FusionError` itself carries neither payload, so these
    // progress-channel values are the only carrier available at this seam.
    // Only a `Some` overwrites the running latch — a later event with
    // `None` (the ordinary case for most progress stages) must not erase
    // an earlier `Some`.
    let forwarder = tokio::spawn(async move {
        let mut last_realized_output_tokens: Option<u64> = None;
        let mut last_egress_profiles: Option<Vec<String>> = None;
        while let Some(event) = prog_rx.recv().await {
            if let Some(tokens) = event.realized_output_tokens {
                last_realized_output_tokens = Some(tokens);
            }
            if let Some(profiles) = event.egress_profiles {
                last_egress_profiles = Some(profiles);
            }
            forward_status_sink
                .set_fusion_stage(&forward_task_id, event.stage.label())
                .await;
        }
        (last_realized_output_tokens, last_egress_profiles)
    });

    let outcome = executor.run(request, inherit, Some(prog_tx)).await;
    // Natural completion and TaskStop race on this same worker-map lock.
    // Whichever removes/marks the record first owns the terminal
    // transition. Once finalizing wins, kill must not abort the
    // commit→completion-sink window.
    //
    // [Finding 18] Claim `finalizing` BEFORE `forwarder.await`, not after:
    // the forwarder task must be rescheduled and can itself take
    // contended registry locks per buffered stage event, so awaiting it
    // first widens the race window in which a concurrent `kill` (which
    // short-circuits only on `finalizing`) can see the record still
    // un-claimed and discard an already-successful `Ok(FusionResult)` as
    // `Killed` with none of its terminal side effects run. Claiming first
    // makes `kill` land on the same "already finalizing" short-circuit
    // whether it arrives before or during the forwarder drain; the
    // forwarder's own stage writes are harmless either way since `kill`
    // no longer touches the record once `finalizing` is set.
    let may_finalize = {
        let mut workers = workers.lock().await;
        match workers.get_mut(&worker_task_id) {
            Some(rec) => {
                rec.finalizing = true;
                true
            }
            None => false,
        }
    };
    let (last_realized_output_tokens, last_egress_profiles) =
        forwarder.await.unwrap_or((None, None));
    if !may_finalize {
        // [Round-4 review finding 10] `kill`/`drain_pending_kills` both
        // remove this worker's record from `workers` BEFORE firing
        // `cancel` (see their own comments), so `finalize_fusion_outcome`'s
        // `Err(FusionError::Cancelled)` arm — and the `disclose_partial_usage`
        // it calls — can never run for a user-stopped `/fusion`: by the
        // time `executor.run` returns `Err(Cancelled)` here, the record is
        // already gone and `may_finalize` is `false`. `kill`/
        // `drain_pending_kills` still write the terminal `Killed` status
        // themselves; this call recovers only the disclosure that arm
        // would otherwise have written — the egress profiles / realized
        // token usage the forwarder above already latched from the
        // progress channel before the record was removed. A no-op when
        // nothing ever egressed (both `last_*` are `None`), matching
        // `disclose_partial_usage`'s own guard.
        disclose_partial_usage(
            &status_sink,
            &worker_task_id,
            last_realized_output_tokens,
            last_egress_profiles,
        )
        .await;
        return;
    }
    finalize_fusion_outcome(
        &outcome,
        &output_manager,
        &worker_spool_path,
        &worker_task_id,
        &status_sink,
        &sink,
        &conversation_id,
        last_realized_output_tokens,
        last_egress_profiles,
    )
    .await;
    workers.lock().await.remove(&worker_task_id);
}

/// Handler for [`TaskType::LocalFusion`].
pub struct LocalFusionHandler {
    executor: Arc<dyn FusionExecutor>,
    sink: Arc<dyn FusionCompletionSink>,
    tool_invoker: Arc<dyn ToolInvoker>,
    budget: Arc<dyn BudgetEnforcerHandle>,
    output_manager: Arc<TaskOutputManager>,
    status_sink: Arc<dyn TaskStatusSink>,
    workers: Arc<Mutex<HashMap<String, WorkerCancel>>>,
    pending_kill: Arc<StdMutex<Vec<String>>>,
}

impl LocalFusionHandler {
    /// Construct with composition-root handles.
    #[must_use]
    pub fn new(
        executor: Arc<dyn FusionExecutor>,
        sink: Arc<dyn FusionCompletionSink>,
        tool_invoker: Arc<dyn ToolInvoker>,
        budget: Arc<dyn BudgetEnforcerHandle>,
        output_manager: Arc<TaskOutputManager>,
    ) -> Self {
        Self {
            executor,
            sink,
            tool_invoker,
            budget,
            output_manager,
            status_sink: Arc::new(NoopStatusSink),
            workers: Arc::new(Mutex::new(HashMap::new())),
            pending_kill: Arc::new(StdMutex::new(Vec::new())),
        }
    }

    /// Attach a registry-backed status sink.
    #[must_use]
    pub fn with_status_sink(mut self, sink: Arc<dyn TaskStatusSink>) -> Self {
        self.status_sink = sink;
        self
    }

    /// Drain kill requests queued by the synchronous cleanup closure.
    pub async fn drain_pending_kills(&self) {
        let pending = {
            let mut pending = self
                .pending_kill
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            std::mem::take(&mut *pending)
        };
        for task_id in pending {
            if self.status_sink.is_terminal(&task_id).await {
                continue;
            }
            let rec = {
                let mut workers = self.workers.lock().await;
                if workers.get(&task_id).is_some_and(|rec| rec.finalizing) {
                    None
                } else {
                    workers.remove(&task_id)
                }
            };
            let Some(rec) = rec else { continue };
            let _ = cancel_fusion_worker(rec).await;
            if !self.status_sink.is_terminal(&task_id).await {
                self.status_sink
                    .set_status(&task_id, TaskStatus::Killed)
                    .await;
            }
        }
    }
}

#[async_trait]
impl Task for LocalFusionHandler {
    fn name(&self) -> &str {
        HANDLER_NAME
    }

    fn task_type(&self) -> TaskType {
        TaskType::LocalFusion
    }

    async fn spawn(
        &self,
        input: TaskSpawnInput,
        ctx: TaskContext,
    ) -> Result<TaskHandle, TaskError> {
        let TaskSpawnInput::LocalFusion {
            request,
            conversation_id,
        } = input
        else {
            return Err(TaskError::Internal(
                "fusion handler received a non-LocalFusion spawn input".into(),
            ));
        };

        let task_id = crate::id::generate_task_id(TaskType::LocalFusion);
        let spool_path = self
            .output_manager
            .allocate(&task_id)
            .await
            .map_err(|e| TaskError::Io(e.to_string()))?;

        let cancel = CancellationToken::new();
        let executor = self.executor.clone();
        // Capture the executor's effective per-run timeout before the worker
        // is handed to the registry. The same snapshot is carried through
        // FusionInheritance so the orchestrator's actual deadline cannot
        // diverge if settings change before activation.
        let effective_timeout_ms = executor.effective_timeout_ms();
        let inherit = FusionInheritance::new(
            SubagentInheritance {
                tool_invoker: self.tool_invoker.clone(),
                budget: self.budget.clone(),
            },
            cancel.clone(),
        )
        .with_effective_timeout_ms(effective_timeout_ms);
        let sink = self.sink.clone();
        let status_sink = self.status_sink.clone();
        let workers = self.workers.clone();
        let output_manager = self.output_manager.clone();
        let worker_spool_path = spool_path;
        let worker_task_id = task_id.clone();
        let (activation_tx, activation_rx) = if status_sink.requires_explicit_activation() {
            let (tx, rx) = tokio::sync::oneshot::channel();
            (Some(tx), Some(rx))
        } else {
            (None, None)
        };
        let (completion_tx, completion_rx) = oneshot::channel();

        let worker = Box::pin(run_fusion_worker(FusionWorkerArgs {
            executor,
            inherit,
            request,
            conversation_id,
            sink,
            status_sink,
            workers,
            output_manager,
            worker_spool_path,
            worker_task_id,
            activation_rx,
            // Fires unconditionally when this future is done — naturally, or
            // forced by `runtime.cancel`'s hard abort — so `kill`'s bounded
            // wait can tell the two apart (F012).
            completion_signal: WorkerCompletionSignal::new(completion_tx),
        }));

        let mut workers = self.workers.lock().await;
        let bg_handle = ctx
            .runtime
            .spawn(&format!("{HANDLER_NAME}:{task_id}"), worker)
            .await
            .map_err(|e| TaskError::Internal(e.to_string()))?;
        workers.insert(
            task_id.clone(),
            WorkerCancel {
                handle: bg_handle,
                runtime: ctx.runtime.clone(),
                cancel,
                completion_rx: StdMutex::new(Some(completion_rx)),
                finalizing: false,
            },
        );
        drop(workers);

        let cleanup_pending = self.pending_kill.clone();
        let cleanup_task_id = task_id.clone();
        let cleanup: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            cleanup_pending
                .lock()
                .unwrap()
                .push(cleanup_task_id.clone());
        });

        let handle =
            TaskHandle::new(task_id, Some(cleanup)).with_fusion_timeout_ms(effective_timeout_ms);
        Ok(match activation_tx {
            Some(activation_tx) => handle.with_activation(move || {
                let _ = activation_tx.send(());
            }),
            None => handle,
        })
    }

    async fn kill(&self, task_id: &str, _ctx: TaskContext) -> Result<(), TaskError> {
        if self.status_sink.is_terminal(task_id).await {
            return Ok(());
        }
        let rec = {
            let mut workers = self.workers.lock().await;
            if workers.get(task_id).is_some_and(|rec| rec.finalizing) {
                return Ok(());
            }
            workers.remove(task_id)
        };
        if let Some(rec) = rec {
            cancel_fusion_worker(rec).await?;
        }
        if !self.status_sink.is_terminal(task_id).await {
            self.status_sink
                .set_status(task_id, TaskStatus::Killed)
                .await;
        }
        Ok(())
    }
}

/// XML-escape `&`, `<`, `>`, `"`, `'`.
#[must_use]
pub fn escape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
    out
}

/// Short label for a [`platform_api::FusionNeedsParentReason`]. Mirrors
/// `fusion::orchestrator::reason_line` (not reused — this crate does not
/// depend on the `fusion` crate, see §0) but goes through [`escape_xml`]
/// before embedding, since `AnalystRequested`'s `reason` is untrusted text
/// from a panel/analyst model (F010 is the deeper sanitize-at-the-source
/// fix; this is the minimum needed so it cannot break out of its element).
fn needs_parent_reason_label(reason: &platform_api::FusionNeedsParentReason) -> String {
    use platform_api::FusionNeedsParentReason as Reason;
    match reason {
        Reason::AnalystRequested { reason } => reason.clone(),
        Reason::AnalysisParseFailed => "analyst output could not be parsed".to_string(),
        Reason::AnalysisFailed { category } => format!("analyst call failed: {category}"),
        Reason::CriticalContradiction => "unresolved critical contradiction".to_string(),
        Reason::LowConfidence => "confidence below merge threshold".to_string(),
        Reason::SynthesisFailed => "synthesizer failed".to_string(),
        Reason::SynthesisTimedOut => "synthesizer timed out".to_string(),
    }
}

/// Sanitized fusion-result envelope. No `PanelReport`, no raw provider errors,
/// no analyst payload, no model names (profile ids only, via
/// `<egress-profiles>`).
#[must_use]
pub fn fusion_result_xml(result: &FusionResult) -> String {
    let status = match result.status {
        FusionStatus::Completed => "completed",
        FusionStatus::NeedsParent => "needs_parent",
    };
    let decision = match &result.decision {
        platform_api::FusionDecision::Picked { panel_id } => format!("picked:{panel_id}"),
        platform_api::FusionDecision::Merged => "merged".to_string(),
        platform_api::FusionDecision::NeedsParent { .. } => "needs_parent".to_string(),
    };
    let egress_section = if result.egress_profiles.is_empty() {
        String::new()
    } else {
        format!(
            "\n  <egress-profiles>{}</egress-profiles>",
            escape_xml(&result.egress_profiles.join(", "))
        )
    };
    let needs_parent_reason_section = match &result.decision {
        platform_api::FusionDecision::NeedsParent { reason } => format!(
            "\n  <needs-parent-reason>{}</needs-parent-reason>",
            escape_xml(&needs_parent_reason_label(reason))
        ),
        platform_api::FusionDecision::Picked { .. } | platform_api::FusionDecision::Merged => {
            String::new()
        }
    };
    let usage_section = format!(
        "\n  <usage><input-tokens>{}</input-tokens><output-tokens>{}</output-tokens><provider-requests>{}</provider-requests><estimated>{}</estimated></usage>",
        result.usage.input_tokens,
        result.usage.output_tokens,
        result.usage.provider_requests,
        result.usage.estimated,
    );
    format!(
        "<fusion-result>\n  <run-id>{}</run-id>\n  <status>{}</status>\n  <decision>{}</decision>\n  <final-text>{}</final-text>{egress_section}{needs_parent_reason_section}{usage_section}\n</fusion-result>",
        escape_xml(&result.run_id),
        status,
        escape_xml(&decision),
        escape_xml(&result.final_text),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use platform_api::{
        FusionAnalysis, FusionDecision, FusionNeedsParentReason, FusionRecommendation,
        FusionTiming, FusionUsage,
    };

    // A sentinel `FusionAnalysis` (not `None`) so `!xml.contains("analysis")`
    // and friends actually prove the analyst payload is omitted, rather than
    // trivially passing because there was never anything to omit.
    fn sentinel_analysis() -> FusionAnalysis {
        FusionAnalysis {
            schema_version: 1,
            consensus: vec!["ANALYSIS_SENTINEL_consensus".into()],
            contradictions: vec![],
            unique_insights: vec![],
            coverage_gaps: vec!["ANALYSIS_SENTINEL_gap".into()],
            scores: std::collections::BTreeMap::new(),
            confidence: 42,
            recommendation: FusionRecommendation::NeedsParent {
                reason: "ANALYSIS_SENTINEL_reason".into(),
            },
        }
    }

    #[test]
    fn xml_escapes_and_omits_raw_analysis() {
        let xml = fusion_result_xml(&FusionResult {
            schema_version: 1,
            run_id: "fu_<x>".into(),
            status: FusionStatus::NeedsParent,
            decision: FusionDecision::NeedsParent {
                reason: FusionNeedsParentReason::LowConfidence,
            },
            final_text: "a & b < c".into(),
            analysis: Some(sentinel_analysis()),
            panels: vec![],
            usage: FusionUsage::default(),
            timing: FusionTiming::default(),
            egress_profiles: vec!["openai".into()],
        });
        assert!(xml.contains("<run-id>fu_&lt;x&gt;</run-id>"));
        assert!(xml.contains("<status>needs_parent</status>"));
        assert!(xml.contains("<final-text>a &amp; b &lt; c</final-text>"));
        // The sentinel's own text never appears anywhere in the envelope —
        // proves the analyst payload really is omitted, not merely absent
        // because the fixture had nothing in it.
        assert!(
            !xml.contains("ANALYSIS_SENTINEL"),
            "raw analysis leaked: {xml}"
        );
        assert!(!xml.contains("<analysis"), "got: {xml}");
        // F006 item 3: the egress profile is now surfaced — inside its own
        // element, never inside <final-text>.
        assert!(
            xml.contains("<egress-profiles>openai</egress-profiles>"),
            "got: {xml}"
        );
        let final_text_section = xml
            .split("<final-text>")
            .nth(1)
            .and_then(|s| s.split("</final-text>").next())
            .unwrap_or_default();
        assert!(
            !final_text_section.contains("openai"),
            "profile leaked into <final-text>: {xml}"
        );
    }

    #[test]
    fn xml_carries_needs_parent_reason_only_for_needs_parent_decisions() {
        let needs_parent = fusion_result_xml(&FusionResult {
            schema_version: 1,
            run_id: "fu_np".into(),
            status: FusionStatus::NeedsParent,
            decision: FusionDecision::NeedsParent {
                reason: FusionNeedsParentReason::CriticalContradiction,
            },
            final_text: "needs parent".into(),
            analysis: None,
            panels: vec![],
            usage: FusionUsage::default(),
            timing: FusionTiming::default(),
            egress_profiles: vec![],
        });
        assert!(
            needs_parent.contains(
                "<needs-parent-reason>unresolved critical contradiction</needs-parent-reason>"
            ),
            "got: {needs_parent}"
        );
        assert!(
            !needs_parent.contains("<egress-profiles>"),
            "no egress clause when empty: {needs_parent}"
        );

        let picked = fusion_result_xml(&FusionResult {
            schema_version: 1,
            run_id: "fu_pick".into(),
            status: FusionStatus::Completed,
            decision: FusionDecision::Picked {
                panel_id: "P1".into(),
            },
            final_text: "picked answer".into(),
            analysis: None,
            panels: vec![],
            usage: FusionUsage::default(),
            timing: FusionTiming::default(),
            egress_profiles: vec![],
        });
        assert!(
            !picked.contains("<needs-parent-reason>"),
            "a Picked decision must not render a needs-parent-reason: {picked}"
        );
    }

    #[test]
    fn xml_usage_section_carries_tokens_and_estimated_flag_but_no_model_names() {
        let xml = fusion_result_xml(&FusionResult {
            schema_version: 1,
            run_id: "fu_usage".into(),
            status: FusionStatus::Completed,
            decision: FusionDecision::Merged,
            final_text: "merged".into(),
            analysis: None,
            panels: vec![],
            usage: FusionUsage {
                input_tokens: 111,
                output_tokens: 222,
                provider_requests: 3,
                estimated: true,
                ..FusionUsage::default()
            },
            timing: FusionTiming::default(),
            egress_profiles: vec![],
        });
        assert!(
            xml.contains(
                "<usage><input-tokens>111</input-tokens><output-tokens>222</output-tokens><provider-requests>3</provider-requests><estimated>true</estimated></usage>"
            ),
            "got: {xml}"
        );
    }
}

#[cfg(test)]
#[path = "local_fusion_test.rs"]
mod local_fusion_test;

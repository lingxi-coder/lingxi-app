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
    FusionInheritance, FusionResult, FusionStatus, RuntimeSpawner, SubagentInheritance,
    ToolInvoker,
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
        let inherit = FusionInheritance::new(
            SubagentInheritance {
                tool_invoker: self.tool_invoker.clone(),
                budget: self.budget.clone(),
            },
            cancel.clone(),
        );

        let executor = self.executor.clone();
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

        let worker = Box::pin(async move {
            // Fires unconditionally when this future is done — naturally, or
            // forced by `runtime.cancel`'s hard abort — so `kill`'s bounded
            // wait can tell the two apart (F012).
            let _completion_signal = WorkerCompletionSignal::new(completion_tx);
            if let Some(activation_rx) = activation_rx {
                if activation_rx.await.is_err() {
                    workers.lock().await.remove(&worker_task_id);
                    return;
                }
            }
            status_sink
                .set_status(&worker_task_id, TaskStatus::Running)
                .await;

            // F005: forward progress into `LocalFusionTaskState.stage` so a
            // client polling the task DTO sees the same `FusionStage::label()`
            // text the Agent-tool path forwards as `subagent_activity` —
            // before this the task carried NO progress at all between
            // `Running` and its terminal status.
            let (prog_tx, mut prog_rx) =
                tokio::sync::mpsc::channel::<platform_api::FusionProgress>(32);
            let forward_status_sink = status_sink.clone();
            let forward_task_id = worker_task_id.clone();
            let forwarder = tokio::spawn(async move {
                while let Some(event) = prog_rx.recv().await {
                    forward_status_sink
                        .set_fusion_stage(&forward_task_id, event.stage.label())
                        .await;
                }
            });

            let outcome = executor.run(request, inherit, Some(prog_tx)).await;
            let _ = forwarder.await;
            // Natural completion and TaskStop race on this same worker-map
            // lock. Whichever removes/marks the record first owns the terminal
            // transition. Once finalizing wins, kill must not abort the
            // commit→completion-sink window.
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
            if !may_finalize {
                return;
            }
            match &outcome {
                Ok(result) => {
                    let body = serde_json::to_string_pretty(result)
                        .unwrap_or_else(|_| result.final_text.clone());
                    let _ = output_manager.append(&worker_spool_path, &body).await;
                    // Write the egress/usage summary BEFORE the terminal
                    // status transition (same ordering rule as
                    // `set_agent_outcome`): the registry's notification drain
                    // is terminal-status-gated, so the reverse order could
                    // let a drain observe a completed task whose usage
                    // summary hasn't landed yet.
                    let usage_summary = platform_api::task_registry::AgentRunUsage {
                        subagent_tokens: result
                            .usage
                            .input_tokens
                            .saturating_add(result.usage.output_tokens),
                        tool_uses: u64::from(result.usage.provider_requests),
                        duration_ms: result.timing.total_ms,
                    };
                    status_sink
                        .set_fusion_egress_and_usage(
                            &worker_task_id,
                            result.egress_profiles.clone(),
                            Some(usage_summary),
                        )
                        .await;
                    status_sink
                        .finish_fusion_terminal(
                            &worker_task_id,
                            result.run_id.clone(),
                            result.final_text.clone(),
                            TaskStatus::Completed,
                        )
                        .await;
                    sink.publish(&conversation_id, result).await;
                }
                Err(FusionError::Cancelled) => {
                    status_sink
                        .set_status(&worker_task_id, TaskStatus::Killed)
                        .await;
                }
                Err(err) => {
                    let _ = output_manager
                        .append(&worker_spool_path, &err.to_string())
                        .await;
                    status_sink
                        .set_fusion_error(&worker_task_id, err.to_string())
                        .await;
                    status_sink
                        .set_status(&worker_task_id, TaskStatus::Failed)
                        .await;
                }
            }
            workers.lock().await.remove(&worker_task_id);
        });

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

        let handle = TaskHandle::new(task_id, Some(cleanup));
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

/// Sanitized fusion-result envelope. No PanelReport, no raw provider errors,
/// no analyst payload, no model names (profile ids only, via
/// `<egress-profiles>`).
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

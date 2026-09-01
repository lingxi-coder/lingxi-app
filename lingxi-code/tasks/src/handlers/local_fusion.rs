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
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

pub use crate::handlers::local_bash::{NoopStatusSink, TaskStatusSink};

const HANDLER_NAME: &str = "local_fusion";

struct WorkerCancel {
    handle: BackgroundTaskHandle,
    runtime: Arc<dyn RuntimeSpawner>,
    cancel: CancellationToken,
    finalizing: bool,
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
            rec.cancel.cancel();
            let _ = rec.runtime.cancel(&rec.handle).await;
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

        let worker = Box::pin(async move {
            if let Some(activation_rx) = activation_rx {
                if activation_rx.await.is_err() {
                    workers.lock().await.remove(&worker_task_id);
                    return;
                }
            }
            status_sink
                .set_status(&worker_task_id, TaskStatus::Running)
                .await;

            let outcome = executor.run(request, inherit, None).await;
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
            rec.cancel.cancel();
            let _ = rec.runtime.cancel(&rec.handle).await;
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

/// Sanitized fusion-result envelope. No PanelReport, no raw provider errors.
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
    format!(
        "<fusion-result>\n  <run-id>{}</run-id>\n  <status>{}</status>\n  <decision>{}</decision>\n  <final-text>{}</final-text>\n</fusion-result>",
        escape_xml(&result.run_id),
        status,
        escape_xml(&decision),
        escape_xml(&result.final_text),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use platform_api::{FusionDecision, FusionNeedsParentReason, FusionTiming, FusionUsage};

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
            analysis: None,
            panels: vec![],
            usage: FusionUsage::default(),
            timing: FusionTiming::default(),
            egress_profiles: vec!["openai".into()],
        });
        assert!(xml.contains("<run-id>fu_&lt;x&gt;</run-id>"));
        assert!(xml.contains("<status>needs_parent</status>"));
        assert!(xml.contains("<final-text>a &amp; b &lt; c</final-text>"));
        assert!(!xml.contains("analysis"));
        assert!(!xml.contains("openai"));
    }
}

#[cfg(test)]
#[path = "local_fusion_test.rs"]
mod local_fusion_test;

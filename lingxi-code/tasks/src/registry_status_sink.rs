//! [`RegistryStatusSink`] — a [`TaskStatusSink`] that writes a handler worker's
//! status (and rest signals) THROUGH to the [`TaskRegistry`].
//!
//! THE GAP this closes: handler-spawned tasks (`LocalAgent`) default to the
//! `NoopStatusSink`, so a worker's `set_status` never reached the registry —
//! the stored `TaskStateBase.status` stayed `Running` forever, so
//! `take_pending_task_notifications` (terminal-gated) never fired the
//! "you will be notified when complete" promise, and `notify_rest` (the
//! backgrounded agent's "comes to rest") had nowhere to land.
//!
//! DEFERRED by construction: the handler is registered INTO the registry before
//! the registry `Arc` exists (a registration cycle), so the sink holds a
//! set-once cell bound at the composition root once `Arc<TaskRegistry>` is
//! built — the same pattern as the deferred tool invoker.

use std::sync::Arc;
use std::sync::OnceLock;

use crate::handlers::TaskStatusSink;
use crate::registry::TaskRegistry;
use crate::TaskStatus;
use async_trait::async_trait;

/// Bridges a handler's status-sink calls onto the registry handle. Build it,
/// hand a clone to the handler via `with_status_sink`, and [`bind`](Self::bind)
/// it once the registry `Arc` exists.
#[derive(Default)]
pub struct RegistryStatusSink {
    registry: OnceLock<std::sync::Weak<TaskRegistry>>,
}

impl RegistryStatusSink {
    /// A new, UNBOUND sink (every call is a no-op until [`bind`](Self::bind)).
    #[must_use]
    pub fn new() -> Self {
        Self {
            registry: OnceLock::new(),
        }
    }

    /// Bind the registry handle. Idempotent — a second bind is ignored.
    pub fn bind(&self, registry: Arc<TaskRegistry>) {
        let _ = self.registry.set(Arc::downgrade(&registry));
    }

    fn registry(&self) -> Option<Arc<TaskRegistry>> {
        self.registry.get().and_then(std::sync::Weak::upgrade)
    }
}

#[async_trait]
impl TaskStatusSink for RegistryStatusSink {
    fn requires_explicit_activation(&self) -> bool {
        true
    }

    async fn set_status(&self, task_id: &str, status: TaskStatus) {
        if let Some(reg) = self.registry() {
            // Best-effort: a status write for a since-evicted task is a benign
            // `NotFound` we deliberately swallow (mirrors the bash sink's
            // tolerance for a racing teardown).
            let _ = reg.set_status(task_id, status).await;
        }
    }

    async fn set_exit_code(&self, task_id: &str, exit_code: i32) {
        // (M8 cc2.1.198) `local_bash` write-through: the worker reports the
        // child's exit code just before its terminal `set_status`; without
        // this the stored state kept `exit_code: None` and the panel/output
        // projection could never show the real completion. Best-effort like
        // `set_status`.
        if let Some(reg) = self.registry() {
            let _ = reg.set_bash_exit_code(task_id, exit_code).await;
        }
    }

    async fn notify_rest(
        &self,
        task_id: &str,
        result: Option<String>,
        usage: Option<platform_api::task_registry::AgentRunUsage>,
        agent_id: Option<protocol::AgentId>,
        agent_name: Option<String>,
        team_name: Option<String>,
    ) {
        if let Some(reg) = self.registry() {
            reg.mark_task_rested(task_id, result, usage, agent_id, agent_name, team_name)
                .await;
        }
    }

    async fn set_agent_outcome(
        &self,
        task_id: &str,
        outcome: platform_api::task_registry::AgentTerminalOutcome,
    ) {
        if let Some(reg) = self.registry() {
            reg.set_agent_outcome(task_id, outcome).await;
        }
    }

    async fn set_workflow_outcome(
        &self,
        task_id: &str,
        outcome: platform_api::task_registry::WorkflowTerminalOutcome,
    ) {
        if let Some(reg) = self.registry() {
            reg.set_workflow_outcome(task_id, outcome).await;
        }
    }

    async fn set_fusion_outcome(&self, task_id: &str, run_id: String, final_text: String) {
        if let Some(reg) = self.registry() {
            reg.set_fusion_outcome(task_id, run_id, final_text).await;
        }
    }

    async fn set_fusion_error(&self, task_id: &str, error: String) {
        if let Some(reg) = self.registry() {
            reg.set_fusion_error(task_id, error).await;
        }
    }

    async fn set_fusion_egress_and_usage(
        &self,
        task_id: &str,
        egress_profiles: Vec<String>,
        usage: Option<platform_api::task_registry::AgentRunUsage>,
    ) {
        if let Some(reg) = self.registry() {
            reg.set_fusion_egress_and_usage(task_id, egress_profiles, usage)
                .await;
        }
    }

    async fn set_fusion_stage(&self, task_id: &str, stage: String) {
        if let Some(reg) = self.registry() {
            reg.set_fusion_stage(task_id, stage).await;
        }
    }

    async fn set_fusion_publication(
        &self,
        task_id: &str,
        receipt: platform_api::FusionPublicationReceipt,
    ) {
        if let Some(reg) = self.registry() {
            reg.set_fusion_publication(task_id, receipt).await;
        }
    }

    async fn finish_fusion_terminal(
        &self,
        task_id: &str,
        run_id: String,
        final_text: String,
        status: TaskStatus,
    ) {
        if let Some(reg) = self.registry() {
            let _ = reg
                .finish_fusion_terminal(task_id, run_id, final_text, status)
                .await;
        } else {
            self.set_fusion_outcome(task_id, run_id, final_text).await;
            self.set_status(task_id, status).await;
        }
    }

    async fn finish_workflow_terminal(
        &self,
        task_id: &str,
        outcome: platform_api::task_registry::WorkflowTerminalOutcome,
        status: TaskStatus,
    ) {
        if let Some(reg) = self.registry() {
            let _ = reg.finish_workflow_terminal(task_id, outcome, status).await;
        } else {
            self.set_workflow_outcome(task_id, outcome).await;
            self.set_status(task_id, status).await;
        }
    }

    async fn notify_monitor_event(&self, task_id: &str, event: &str) {
        if let Some(reg) = self.registry() {
            let _ = reg.enqueue_monitor_event(task_id, event).await;
        }
    }

    async fn is_registered(&self, task_id: &str) -> bool {
        match self.registry() {
            Some(reg) => reg.get(task_id).await.is_some(),
            // An unbound sink is inert and should not stall a standalone
            // handler forever.
            None => true,
        }
    }

    /// Consult the stored task status so `drain_pending_kills` can skip flipping
    /// an already-terminal task to `Killed`. An unbound sink, an unknown/evicted
    /// task, or a lookup error all read as "not terminal" (`false`) — a
    /// since-evicted terminal task's `set_status` is already a benign `NotFound`
    /// no-op, so nothing is clobbered either way.
    async fn is_terminal(&self, task_id: &str) -> bool {
        if let Some(reg) = self.registry() {
            if let Some(rec) = reg.get(task_id).await {
                return rec.base().status.is_terminal();
            }
        }
        false
    }
}

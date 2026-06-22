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

use async_trait::async_trait;
use traits::task_registry::TaskRegistryHandle;

use crate::handle::status_to_wire;
use crate::handlers::TaskStatusSink;
use crate::TaskStatus;

/// Bridges a handler's status-sink calls onto the registry handle. Build it,
/// hand a clone to the handler via `with_status_sink`, and [`bind`](Self::bind)
/// it once the registry `Arc` exists.
#[derive(Default)]
pub struct RegistryStatusSink {
    registry: OnceLock<Arc<dyn TaskRegistryHandle>>,
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
    pub fn bind(&self, registry: Arc<dyn TaskRegistryHandle>) {
        let _ = self.registry.set(registry);
    }
}

#[async_trait]
impl TaskStatusSink for RegistryStatusSink {
    async fn set_status(&self, task_id: &str, status: TaskStatus) {
        if let Some(reg) = self.registry.get() {
            // Best-effort: a status write for a since-evicted task is a benign
            // `NotFound` we deliberately swallow (mirrors the bash sink's
            // tolerance for a racing teardown).
            let _ = reg.set_status(task_id, status_to_wire(status)).await;
        }
    }

    async fn notify_rest(
        &self,
        task_id: &str,
        result: Option<String>,
        usage: Option<traits::task_registry::AgentRunUsage>,
    ) {
        if let Some(reg) = self.registry.get() {
            reg.mark_rested(task_id, result, usage).await;
        }
    }
}

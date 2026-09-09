//! Orchestrator-side [`TaskNotificationProvider`] backed by the live
//! [`TaskRegistryHandle`].
//!
//! T35: the orchestrator folds a `<task-notification>` reminder into each turn
//! (see [`ConversationOrchestrator::task_notification_reminder_messages`]) by
//! draining the registry's terminal-not-notified tasks. The orchestrator names
//! the registry only through the narrow `platform_api::task_registry::TaskRegistryHandle`
//! seam (it has no dependency on the `tasks` crate), so this adapter closes the
//! seam from the orchestrator side: it owns an `Arc<dyn TaskRegistryHandle>` —
//! the SAME registry handle the composition root hands the tool context — and
//! forwards [`TaskNotificationProvider::take_pending_task_notifications`] to the
//! registry's `take_pending_task_notifications` (which snapshots + marks-notified
//! + evicts, so each completion surfaces exactly once).
//!
//! Best-effort: a registry error degrades to "nothing to surface" (empty) so a
//! transient registry failure never breaks the turn — matching the fold's
//! `None`-on-empty contract.
//!
//! Wiring: the composition root (`engine-desktop`) builds this over the SAME
//! `Arc` it registers as the `task_registry`, then injects it via
//! [`ConversationOrchestrator::with_task_notifications`].
//!
//! [`ConversationOrchestrator::task_notification_reminder_messages`]:
//!     crate::ConversationOrchestrator
//! [`ConversationOrchestrator::with_task_notifications`]:
//!     crate::ConversationOrchestrator

use std::sync::Arc;

use async_trait::async_trait;
use platform_api::task_registry::{TaskNotification, TaskRegistryHandle};

use crate::prompt::task_notification::TaskNotificationProvider;

/// [`TaskNotificationProvider`] that drains the live registry each turn.
pub struct RegistryTaskNotifications {
    registry: Arc<dyn TaskRegistryHandle>,
}

impl RegistryTaskNotifications {
    /// Wrap the registry handle the composition root shares with the tool
    /// context.
    #[must_use]
    pub fn new(registry: Arc<dyn TaskRegistryHandle>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl TaskNotificationProvider for RegistryTaskNotifications {
    async fn take_pending_task_notifications(&self) -> Vec<TaskNotification> {
        // Best-effort: an error degrades to "nothing to surface" so a transient
        // registry failure never breaks the turn.
        self.registry
            .take_pending_task_notifications()
            .await
            .unwrap_or_default()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use platform_api::task_registry::{
        TaskCreateInput, TaskListFilter, TaskOutputChunk, TaskRecord, TaskRegistryError,
        TaskUpdatePatch,
    };
    use std::sync::Mutex;

    /// A `TaskRegistryHandle` whose only meaningful method is the
    /// notification drain — every other method is unreachable in these tests.
    struct FakeRegistry(Mutex<Vec<TaskNotification>>);

    #[async_trait]
    impl TaskRegistryHandle for FakeRegistry {
        async fn create(&self, _i: TaskCreateInput) -> Result<TaskRecord, TaskRegistryError> {
            unreachable!()
        }
        async fn get(&self, _id: &str) -> Result<Option<TaskRecord>, TaskRegistryError> {
            unreachable!()
        }
        async fn list(&self, _f: TaskListFilter) -> Result<Vec<TaskRecord>, TaskRegistryError> {
            unreachable!()
        }
        async fn update(
            &self,
            _id: &str,
            _p: TaskUpdatePatch,
        ) -> Result<TaskRecord, TaskRegistryError> {
            unreachable!()
        }
        async fn set_status(&self, _id: &str, _s: &str) -> Result<TaskRecord, TaskRegistryError> {
            unreachable!()
        }
        async fn kill(&self, _id: &str) -> Result<TaskRecord, TaskRegistryError> {
            unreachable!()
        }
        async fn output(
            &self,
            _id: &str,
            _o: Option<u64>,
        ) -> Result<TaskOutputChunk, TaskRegistryError> {
            unreachable!()
        }
        async fn take_pending_task_notifications(
            &self,
        ) -> Result<Vec<TaskNotification>, TaskRegistryError> {
            // Drain once (consume-once), mirroring the real registry.
            Ok(std::mem::take(&mut *self.0.lock().unwrap()))
        }
    }

    #[tokio::test]
    async fn forwards_drain_to_registry_then_empty() {
        let n = TaskNotification {
            task_id: "b1".into(),
            task_type: "local_bash".into(),
            status: "completed".into(),
            description: "x".into(),
            tool_use_id: None,
            output_path: None,
            exit_code: Some(0),
            error: None,
            result: None,
            usage: None,
            killed_by: None,
            worktree_path: None,
            worktree_branch: None,
            workflow_failures: Vec::new(),
            workflow_agent_count: None,
            workflow_total_tokens: None,
            workflow_total_tool_calls: None,
            workflow_duration_ms: None,
            ..Default::default()
        };
        let reg = Arc::new(FakeRegistry(Mutex::new(vec![n.clone()])));
        let provider = RegistryTaskNotifications::new(reg);
        assert_eq!(provider.take_pending_task_notifications().await, vec![n]);
        // Consume-once: the registry drained, so the second call is empty.
        assert!(provider.take_pending_task_notifications().await.is_empty());
    }
}

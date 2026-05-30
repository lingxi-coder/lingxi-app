//! Task completion notifications.
//!
//! Notifications are XML-tagged strings injected into the agent's next
//! user message so they survive token-budget compaction.

use crate::state::TaskStatus;
use std::path::Path;

/// Builder for task completion XML notifications.
pub struct TaskNotificationBuilder;

impl TaskNotificationBuilder {
    /// Build the XML notification string.
    #[must_use]
    pub fn build(
        task_id: &str,
        tool_use_id: Option<&str>,
        output_path: &Path,
        status: TaskStatus,
        summary: &str,
    ) -> String {
        let tool_use_line = tool_use_id
            .map(|s| format!("  <tool-use-id>{s}</tool-use-id>\n"))
            .unwrap_or_default();
        format!(
            "<task-notification>\n  <task-id>{task_id}</task-id>\n{tool_use_line}  <output-file>{}</output-file>\n  <status>{:?}</status>\n  <summary>{summary}</summary>\n</task-notification>",
            output_path.display(),
            status,
        )
    }
}

/// A queued notification ready to inject into the next user turn.
#[derive(Debug, Clone)]
pub struct PendingNotification {
    /// Pre-rendered notification string.
    pub value: String,
    /// Delivery mode.
    pub mode: NotificationMode,
}

/// Delivery mode for a pending notification.
#[derive(Debug, Clone, Copy)]
pub enum NotificationMode {
    /// Normal user-channel notification.
    Normal,
    /// Task notification channel (separate XML envelope).
    TaskNotification,
}

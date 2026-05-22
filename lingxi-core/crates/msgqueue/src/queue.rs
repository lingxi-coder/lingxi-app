//! Unified message queue for user / notification / orphan / hook inputs
//! (spec §27).
//!
//! Priorities are encoded as a Rust `Ord` enum so dequeue can rely on
//! ordered insertion (B3 — Ord on priority; B4 — single shared queue).

use lingxi_protocol::{AgentId, HookId, ToolUseId};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::{Notify, RwLock};

/// One queued unit of work waiting to be consumed by an Agent's run loop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuedCommand {
    /// Unique identifier for trace correlation.
    pub uuid: String,
    /// What the command represents (user text, notification, hook, etc).
    pub content: QueuedCommandContent,
    /// Dequeue priority.
    pub priority: QueuePriority,
    /// When the command entered the queue.
    pub queued_at: SystemTime,
    /// What subsystem produced the command.
    pub source: QueueSource,
}

/// The payload of a [`QueuedCommand`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QueuedCommandContent {
    /// Raw text from the human user.
    UserInput {
        /// User-supplied text.
        text: String,
    },
    /// A parsed slash-command invocation.
    SlashCommand {
        /// Slash-command payload, already validated.
        parsed_json: serde_json::Value,
    },
    /// Notification from a background subagent or scheduled task.
    TaskNotification {
        /// Notification payload (serialized).
        value: String,
        /// How the notification should be surfaced.
        mode: NotificationMode,
    },
    /// Inter-agent message routed via `send_message`.
    TeammateMessage {
        /// Originating agent.
        from: AgentId,
        /// Body of the message.
        content: String,
    },
    /// Permission decision orphaned because the requesting tool use vanished.
    OrphanedPermission {
        /// Tool use that was awaiting the decision.
        tool_use_id: ToolUseId,
        /// Human-readable explanation.
        reason: String,
    },
    /// Engine-injected content from a hook.
    HookInjected {
        /// Content to inject into the agent's next prompt.
        content: String,
        /// Hook that produced the injection.
        hook_id: HookId,
    },
}

/// How a [`QueuedCommandContent::TaskNotification`] should be surfaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NotificationMode {
    /// Normal notification.
    Normal,
    /// Notification associated with a task completion.
    TaskNotification,
}

/// Dequeue priority. `Ord` is derived so `Later < Next < Now` and the engine
/// can always pop the maximum priority off the front by keeping the queue
/// sorted at insert time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum QueuePriority {
    /// Defer until after all higher-priority work.
    Later,
    /// Next available slot.
    Next,
    /// Insert at the front; processed before any non-`Now` items.
    Now,
}

/// Where a queued command came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueueSource {
    /// Direct prompt input from the user.
    PromptInput,
    /// Subagent task completion notification.
    TaskCompletion,
    /// Inter-agent `send_message`.
    AgentSendMessage,
    /// Engine hook injection.
    Hook,
    /// Orphan permission cleanup.
    Orphan,
    /// Scheduled cron task.
    Cron,
}

/// The runtime queue itself. Cheap to clone (`Arc` inside).
pub struct MessageQueueManager {
    queue: Arc<RwLock<VecDeque<QueuedCommand>>>,
    notify: Arc<Notify>,
}

impl MessageQueueManager {
    /// Construct an empty queue.
    #[must_use]
    pub fn new() -> Self {
        Self {
            queue: Arc::new(RwLock::new(VecDeque::new())),
            notify: Arc::new(Notify::new()),
        }
    }

    /// Insert `cmd` in priority order; wakes one waiter.
    pub async fn enqueue(&self, cmd: QueuedCommand) {
        let mut q = self.queue.write().await;
        // Insert in priority order — find first item with strictly lower priority.
        let pos = q
            .iter()
            .position(|c| c.priority < cmd.priority)
            .unwrap_or(q.len());
        q.insert(pos, cmd);
        self.notify.notify_one();
    }

    /// Pop the highest-priority queued command, if any.
    pub async fn dequeue(&self) -> Option<QueuedCommand> {
        self.queue.write().await.pop_front()
    }

    /// Pop every `Now`-priority item currently at the front of the queue.
    pub async fn drain_now_priority(&self) -> Vec<QueuedCommand> {
        let mut q = self.queue.write().await;
        let mut out = Vec::new();
        while let Some(front) = q.front() {
            if front.priority == QueuePriority::Now {
                out.push(q.pop_front().unwrap());
            } else {
                break;
            }
        }
        out
    }

    /// Snapshot the queue without consuming it. Useful for diagnostics.
    pub async fn snapshot(&self) -> Vec<QueuedCommand> {
        self.queue.read().await.iter().cloned().collect()
    }

    /// Wait up to `timeout` for an item, returning it if one arrives.
    pub async fn wait_for_message(
        &self,
        timeout: std::time::Duration,
    ) -> Option<QueuedCommand> {
        tokio::select! {
            () = self.notify.notified() => self.queue.write().await.pop_front(),
            () = tokio::time::sleep(timeout) => None,
        }
    }
}

impl Default for MessageQueueManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(priority: QueuePriority, text: &str) -> QueuedCommand {
        QueuedCommand {
            uuid: text.into(),
            content: QueuedCommandContent::UserInput { text: text.into() },
            priority,
            queued_at: SystemTime::now(),
            source: QueueSource::PromptInput,
        }
    }

    #[tokio::test]
    async fn now_priority_dequeues_first() {
        let q = MessageQueueManager::new();
        q.enqueue(mk(QueuePriority::Later, "later")).await;
        q.enqueue(mk(QueuePriority::Now, "now")).await;
        q.enqueue(mk(QueuePriority::Next, "next")).await;
        assert_eq!(q.dequeue().await.unwrap().uuid, "now");
        assert_eq!(q.dequeue().await.unwrap().uuid, "next");
        assert_eq!(q.dequeue().await.unwrap().uuid, "later");
    }

    #[tokio::test]
    async fn drain_now_returns_only_now_items() {
        let q = MessageQueueManager::new();
        q.enqueue(mk(QueuePriority::Now, "n1")).await;
        q.enqueue(mk(QueuePriority::Now, "n2")).await;
        q.enqueue(mk(QueuePriority::Next, "x")).await;
        let drained = q.drain_now_priority().await;
        assert_eq!(drained.len(), 2);
    }
}

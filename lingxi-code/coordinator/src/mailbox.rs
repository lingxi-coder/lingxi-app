//! Teammate mailbox + router for coordinator-mode messaging.
//!
//! Each registered worker has a `TeammateMailbox`; the `MailboxRouter`
//! resolves an [`AgentId`] to its mailbox and delivers messages. Tokio
//! `Notify` is used for the async wake-up path; the inbox itself sits
//! behind a `std::sync::Mutex` because we only ever hold it across
//! synchronous boundaries.

#![allow(clippy::unwrap_used)] // std mutex poisoning is fatal for us anyway

use protocol::AgentId;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use thiserror::Error;
use tokio::sync::Notify;

/// One message routed between a coordinator and a teammate worker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeammateMessage {
    /// Who sent the message.
    pub from: MessageSender,
    /// Message content.
    pub content: String,
    /// Caller-assigned message ID for idempotency.
    pub message_id: String,
    /// Wall-clock send time.
    pub timestamp: std::time::SystemTime,
    /// Optional swarm-protocol request id for messages that participate in a
    /// request/response handshake (shutdown / plan-approval). Minted by
    /// `SendMessage` for a `shutdown_request` (`generateRequestId`,
    /// `SendMessageTool.ts:276`) and echoed by the corresponding response.
    /// `None` for ordinary teammate messages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

/// Originator of a [`TeammateMessage`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MessageSender {
    /// The coordinator agent.
    Coordinator,
    /// A specific teammate worker.
    Teammate(AgentId),
    /// A user-initiated message.
    User,
    /// A system-generated message (e.g. injected notifications).
    System,
}

/// Errors produced by mailbox operations.
#[derive(Debug, Clone, Error)]
pub enum MailboxError {
    /// The mailbox has been closed.
    #[error("mailbox closed")]
    Closed,
    /// The mailbox is at capacity.
    #[error("mailbox full")]
    Full,
    /// No mailbox is registered for the recipient ID.
    #[error("recipient not found: {0}")]
    NotFound(AgentId),
}

/// Per-teammate inbox with bounded capacity and async wake-up.
pub struct TeammateMailbox {
    /// The owning teammate's agent ID.
    pub agent_id: AgentId,
    inbox: Mutex<VecDeque<TeammateMessage>>,
    max_size: usize,
    waker: Arc<Notify>,
    closed: AtomicBool,
}

impl TeammateMailbox {
    /// Construct a new mailbox owned by `agent_id`.
    #[must_use]
    pub fn new(agent_id: AgentId) -> Self {
        Self {
            agent_id,
            inbox: Mutex::new(VecDeque::new()),
            max_size: 100,
            waker: Arc::new(Notify::new()),
            closed: AtomicBool::new(false),
        }
    }

    /// Deliver `msg` to the inbox. Returns an error if the mailbox is full
    /// or closed.
    pub fn deliver(&self, msg: TeammateMessage) -> Result<(), MailboxError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(MailboxError::Closed);
        }
        let mut inbox = self.inbox.lock().unwrap();
        if inbox.len() >= self.max_size {
            return Err(MailboxError::Full);
        }
        inbox.push_back(msg);
        self.waker.notify_one();
        Ok(())
    }

    /// Drain all queued messages.
    pub fn drain(&self) -> Vec<TeammateMessage> {
        self.inbox.lock().unwrap().drain(..).collect()
    }

    /// Wait up to `timeout` for the next message.
    pub async fn wait_for_message(&self, timeout: Duration) -> Option<TeammateMessage> {
        tokio::select! {
            () = self.waker.notified() => self.inbox.lock().unwrap().pop_front(),
            () = tokio::time::sleep(timeout) => None,
        }
    }
}

/// Routes a [`TeammateMessage`] to the mailbox registered for an [`AgentId`].
pub struct MailboxRouter {
    mailboxes: tokio::sync::RwLock<std::collections::HashMap<AgentId, Arc<TeammateMailbox>>>,
    /// Display-name → [`AgentId`] index (keys lower-cased) so a message can be
    /// addressed by a teammate's NAME, not just its id. claude-code's mailbox is
    /// keyed by agent name (`teammateMailbox.ts`); the `TaskUpdate` owner-change
    /// notification and `getAgentStatuses` both address recipients by NAME. The
    /// registry populates this on `spawn_worker` and clears it on
    /// `delete_worker`. Separate from `mailboxes` so the id-keyed delivery path
    /// stays unchanged.
    names: tokio::sync::RwLock<std::collections::HashMap<String, AgentId>>,
}

impl MailboxRouter {
    /// Construct an empty router.
    #[must_use]
    pub fn new() -> Self {
        Self {
            mailboxes: tokio::sync::RwLock::new(std::collections::HashMap::new()),
            names: tokio::sync::RwLock::new(std::collections::HashMap::new()),
        }
    }

    /// Register `mailbox` for `agent_id`. Existing entries are overwritten.
    pub async fn register(&self, agent_id: AgentId, mailbox: Arc<TeammateMailbox>) {
        self.mailboxes.write().await.insert(agent_id, mailbox);
    }

    /// Index `name` → `agent_id` so the worker can later be addressed by its
    /// display name (case-insensitively). Empty names are ignored.
    pub async fn register_name(&self, name: &str, agent_id: AgentId) {
        if name.is_empty() {
            return;
        }
        self.names
            .write()
            .await
            .insert(name.to_ascii_lowercase(), agent_id);
    }

    /// Resolve a teammate display `name` to its [`AgentId`] (case-insensitive),
    /// if one is registered.
    pub async fn resolve_name(&self, name: &str) -> Option<AgentId> {
        self.names
            .read()
            .await
            .get(&name.to_ascii_lowercase())
            .copied()
    }

    /// Look up the mailbox registered for `agent_id`, if any.
    ///
    /// Returns a clone of the `Arc<TeammateMailbox>` so the caller (the
    /// mailbox→runner pump) can park on it independently of the router lock.
    /// `None` when no mailbox is registered (the worker was never spawned or was
    /// already unregistered).
    pub async fn get(&self, agent_id: &AgentId) -> Option<Arc<TeammateMailbox>> {
        self.mailboxes.read().await.get(agent_id).cloned()
    }

    /// Route `msg` to `to`.
    pub async fn route(&self, to: &AgentId, msg: TeammateMessage) -> Result<(), MailboxError> {
        let mailboxes = self.mailboxes.read().await;
        let mb = mailboxes.get(to).ok_or(MailboxError::NotFound(*to))?;
        mb.deliver(msg)
    }

    /// Drop the mailbox registration for `agent` (and any name index pointing at
    /// it).
    pub async fn unregister(&self, agent: &AgentId) {
        self.mailboxes.write().await.remove(agent);
        self.names.write().await.retain(|_, id| id != agent);
    }
}

impl Default for MailboxRouter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn get_returns_registered_mailbox() {
        let router = MailboxRouter::new();
        let agent = AgentId::new();
        // Unknown id before registration.
        assert!(router.get(&agent).await.is_none());

        let mailbox = Arc::new(TeammateMailbox::new(agent));
        router.register(agent, mailbox.clone()).await;

        let got = router.get(&agent).await.expect("registered mailbox resolves");
        assert!(Arc::ptr_eq(&got, &mailbox), "get returns the same Arc");

        // After unregister it is gone again.
        router.unregister(&agent).await;
        assert!(router.get(&agent).await.is_none());
    }
}

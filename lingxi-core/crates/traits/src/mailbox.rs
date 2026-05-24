//! `MailboxRouterHandle` — narrow trait abstracting
//! `coordinator::MailboxRouter::route` so `SendMessageTool` in `lingxi-tools`
//! can dispatch without taking a cyclic dep on `lingxi-coordinator`.
//!
//! Concrete impl lives in `lingxi-coordinator`. Tests inject a recording mock.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::time::SystemTime;
use thiserror::Error;

/// One message routed across teammates.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MailboxMessage {
    /// Caller-assigned message id for idempotency.
    pub message_id: String,
    /// Plain-text message body.
    pub content: String,
    /// Wall-clock send time.
    pub timestamp: SystemTime,
}

/// Ack returned by [`MailboxRouterHandle::route`].
///
/// `claim_window_secs` is byte-locked to `30` — the spec §7 line 498
/// teammate-claim window.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RouteAck {
    /// Wall-clock time the recipient mailbox accepted the message.
    pub claimed_at: SystemTime,
    /// Window during which the recipient must acknowledge. Locked to 30s.
    pub claim_window_secs: u64,
}

/// Failure modes for [`MailboxRouterHandle::route`].
#[derive(Debug, Error)]
pub enum MailboxError {
    /// The recipient is not registered with the router.
    #[error("Mailbox: recipient not found: {0}")]
    NotFound(String),
    /// The recipient's inbox is full.
    #[error("Mailbox: inbox full")]
    Full,
    /// The recipient's inbox is closed.
    #[error("Mailbox: closed")]
    Closed,
    /// Any other internal failure.
    #[error("Mailbox: internal error: {0}")]
    Internal(String),
}

/// Route-a-message seam used by `SendMessageTool`.
#[async_trait]
pub trait MailboxRouterHandle: Send + Sync {
    /// Route `message` from `from_agent` to `to_agent`.
    async fn route(
        &self,
        from_agent: &str,
        to_agent: &str,
        message: MailboxMessage,
    ) -> Result<RouteAck, MailboxError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn trait_is_object_safe() {
        let _: Option<Arc<dyn MailboxRouterHandle>> = None;
    }
}

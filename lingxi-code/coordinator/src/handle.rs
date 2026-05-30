//! `MailboxRouterHandle` trait impl for `MailboxRouter`.
//!
//! Bridges the `lingxi-traits` seam (string-based agent ids + claim-window
//! ack) to the concrete `MailboxRouter::route` (UUID-based `AgentId`) so
//! `SendMessageTool` in `lingxi-tools` can dispatch without taking a cyclic
//! dep on `lingxi-coordinator`.
//!
//! The 30-second `claim_window_secs` is byte-locked to
//! `SEND_MESSAGE_CLAIM_WINDOW` in `lingxi-tools::builtin::send_message`.

use crate::mailbox::{MailboxRouter, MessageSender, TeammateMessage};
use async_trait::async_trait;
use protocol::AgentId;
use std::time::SystemTime;
use traits::mailbox::{MailboxError, MailboxMessage, MailboxRouterHandle, RouteAck};
use uuid::Uuid;

/// Byte-locked teammate claim window (spec §7 line 498).
pub const CLAIM_WINDOW_SECS: u64 = 30;

fn parse_agent_id(s: &str) -> Result<AgentId, MailboxError> {
    // Accept either bare UUIDs or the display form `agent:<uuid>`.
    let raw = s.strip_prefix("agent:").unwrap_or(s);
    Uuid::parse_str(raw)
        .map(AgentId::from_uuid)
        .map_err(|e| MailboxError::Internal(format!("invalid agent id '{s}': {e}")))
}

#[async_trait]
impl MailboxRouterHandle for MailboxRouter {
    async fn route(
        &self,
        from_agent: &str,
        to_agent: &str,
        message: MailboxMessage,
    ) -> Result<RouteAck, MailboxError> {
        let from_id = parse_agent_id(from_agent)?;
        let to_id = parse_agent_id(to_agent)?;

        let teammate_msg = TeammateMessage {
            from: MessageSender::Teammate(from_id),
            content: message.content,
            message_id: message.message_id,
            timestamp: message.timestamp,
        };

        // Adapt the concrete error space onto the trait's error type.
        match self.route(&to_id, teammate_msg).await {
            Ok(()) => Ok(RouteAck {
                claimed_at: SystemTime::now(),
                claim_window_secs: CLAIM_WINDOW_SECS,
            }),
            Err(crate::mailbox::MailboxError::NotFound(id)) => {
                Err(MailboxError::NotFound(id.to_string()))
            }
            Err(crate::mailbox::MailboxError::Full) => Err(MailboxError::Full),
            Err(crate::mailbox::MailboxError::Closed) => Err(MailboxError::Closed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mailbox::TeammateMailbox;
    use std::sync::Arc;

    #[test]
    fn claim_window_locked_30s() {
        assert_eq!(CLAIM_WINDOW_SECS, 30);
    }

    #[tokio::test]
    async fn route_via_handle_returns_30s_claim_window() {
        let router = MailboxRouter::new();
        let to = AgentId::new();
        let from = AgentId::new();
        router
            .register(to, Arc::new(TeammateMailbox::new(to)))
            .await;

        let h: &dyn MailboxRouterHandle = &router;
        let ack = h
            .route(
                &from.as_uuid().to_string(),
                &to.as_uuid().to_string(),
                MailboxMessage {
                    message_id: "m1".into(),
                    content: "hi".into(),
                    timestamp: SystemTime::now(),
                },
            )
            .await
            .unwrap();
        assert_eq!(ack.claim_window_secs, 30);
    }

    #[tokio::test]
    async fn route_via_handle_unknown_recipient_is_not_found() {
        let router = MailboxRouter::new();
        let h: &dyn MailboxRouterHandle = &router;
        let from = AgentId::new();
        let to = AgentId::new();
        let err = h
            .route(
                &from.as_uuid().to_string(),
                &to.as_uuid().to_string(),
                MailboxMessage {
                    message_id: "m1".into(),
                    content: "hi".into(),
                    timestamp: SystemTime::now(),
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(err, MailboxError::NotFound(_)));
    }
}

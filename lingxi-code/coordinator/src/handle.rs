//! `MailboxRouterHandle` trait impl for `MailboxRouter`.
//!
//! Bridges the `lingxi-traits` seam (string-based agent ids + claim-window
//! ack) to the concrete `MailboxRouter::route` (UUID-based `AgentId`) so
//! `SendMessageTool` in `lingxi-tools` can dispatch without taking a cyclic
//! dep on `lingxi-coordinator`.
//!
//! The 30-second `claim_window_secs` is byte-locked to
//! `SEND_MESSAGE_CLAIM_WINDOW` in `lingxi-tool_api::builtin::send_message`.

use crate::mailbox::{MailboxRouter, MessageSender, TeammateMessage};
use crate::team_registry::{TeamRegistry, WorkerStatus};
use async_trait::async_trait;
use protocol::AgentId;
use std::time::SystemTime;
use traits::mailbox::{MailboxError, MailboxMessage, MailboxRouterHandle, RouteAck};
use traits::team_registry::{TeamRegistryHandle, WorkerInfo};
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

/// Canonical lowering of [`WorkerStatus`] to the simplified `WorkerInfo.status`
/// label string consumed by the bridge roster DTO (T18) and the TUI
/// `WorkerRow` (T20). Defined here so the read seam and the (future) lowering
/// share one source of truth.
#[must_use]
pub fn worker_status_label(status: &WorkerStatus) -> String {
    match status {
        WorkerStatus::Idle => "idle",
        WorkerStatus::Working { .. } => "working",
        WorkerStatus::AwaitingMessage => "awaiting_message",
        WorkerStatus::Completed => "completed",
        WorkerStatus::Failed { .. } => "failed",
        WorkerStatus::Killed => "killed",
    }
    .to_string()
}

#[async_trait]
impl TeamRegistryHandle for TeamRegistry {
    async fn list_workers(&self) -> Vec<WorkerInfo> {
        self.list()
            .await
            .into_iter()
            .map(|w| WorkerInfo {
                agent_id: w.agent_id.to_string(),
                agent_type: w.agent_type,
                name: w.name,
                status: worker_status_label(&w.status),
            })
            .collect()
    }

    async fn team_name(&self) -> Option<String> {
        TeamRegistry::team_name(self).await
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

    #[tokio::test]
    async fn team_registry_handle_lists_workers() {
        let reg = TeamRegistry::new(AgentId::new());
        let alpha = reg
            .spawn_worker("explorer".into(), "alpha".into(), String::new())
            .await
            .unwrap();
        let _beta = reg
            .spawn_worker("writer".into(), "beta".into(), String::new())
            .await
            .unwrap();

        // Drive one worker to a non-default status so the label mapping is exercised.
        reg.update_status(
            &alpha,
            WorkerStatus::Working {
                activity: "running".into(),
            },
        )
        .await;
        reg.set_team_name(Some("squad".into())).await;

        let handle: &dyn TeamRegistryHandle = &reg;
        let mut workers = handle.list_workers().await;
        assert_eq!(workers.len(), 2);

        // Order is map-iteration-dependent; sort by name for deterministic asserts.
        workers.sort_by(|a, b| a.name.cmp(&b.name));

        assert_eq!(workers[0].name, "alpha");
        assert_eq!(workers[0].agent_type, "explorer");
        assert_eq!(workers[0].agent_id, alpha.to_string());
        assert_eq!(workers[0].status, "working");

        assert_eq!(workers[1].name, "beta");
        assert_eq!(workers[1].agent_type, "writer");
        // Freshly spawned, untouched → Idle.
        assert_eq!(workers[1].status, "idle");

        assert_eq!(handle.team_name().await, Some("squad".to_string()));
    }

    #[test]
    fn worker_status_label_covers_all_variants() {
        assert_eq!(worker_status_label(&WorkerStatus::Idle), "idle");
        assert_eq!(
            worker_status_label(&WorkerStatus::Working {
                activity: "x".into()
            }),
            "working"
        );
        assert_eq!(
            worker_status_label(&WorkerStatus::AwaitingMessage),
            "awaiting_message"
        );
        assert_eq!(worker_status_label(&WorkerStatus::Completed), "completed");
        assert_eq!(
            worker_status_label(&WorkerStatus::Failed {
                error: "e".into()
            }),
            "failed"
        );
        assert_eq!(worker_status_label(&WorkerStatus::Killed), "killed");
    }
}

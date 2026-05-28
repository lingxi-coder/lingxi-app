//! Orchestrator → TUI permission bridge (M6-05).
//!
//! Provides:
//! - [`PermissionExchange`] — one in-flight permission round-trip
//!   (request + a oneshot back-channel for the response).
//! - [`TuiPermissionGate`] — `PermissionGate` impl that sends the request
//!   onto a `tokio::sync::mpsc` and awaits the oneshot reply.
//!
//! The TUI app side drains the mpsc, opens the matching dialog, and
//! ultimately fills the `resp_tx` when the user resolves. The gate
//! consults a session-rule list first to short-circuit when the user
//! previously picked `AllowAlways` for the same tool. `AllowAlways`
//! resolutions are appended back into that rule list so subsequent
//! same-tool calls skip the dialog.
#![forbid(unsafe_code)]

use std::sync::Arc;

use async_trait::async_trait;
use lingxi_permission::gate::{
    PermissionDecision, PermissionGate, PermissionRequest, PermissionResponse,
};
use lingxi_permission::PermissionRule;
use tokio::sync::{mpsc, oneshot, Mutex};

/// One in-flight permission round-trip between the orchestrator and TUI.
///
/// Constructed by [`TuiPermissionGate::check`] and sent over the mpsc to
/// the TUI app. The TUI fills `resp_tx` when the user resolves the
/// dialog. Dropping `resp_tx` without sending counts as cancellation
/// (the gate maps it to `Deny { reason: "TUI permission response dropped" }`).
#[derive(Debug)]
pub struct PermissionExchange {
    /// What we're asking permission for.
    pub request: PermissionRequest,
    /// One-shot reply channel — TUI sends back when the user resolves.
    pub resp_tx: oneshot::Sender<PermissionResponse>,
}

/// Orchestrator-side permission gate that forwards each `check` call to
/// the TUI over an mpsc channel and awaits a oneshot reply.
pub struct TuiPermissionGate {
    /// Channel into the TUI app. The receiver is owned by the TUI event
    /// loop; the orchestrator only holds the sender.
    pub event_tx: mpsc::Sender<PermissionExchange>,
    /// Session-scoped allow rules. Consulted before the dialog opens;
    /// appended to when the user picks `AllowAlways`.
    pub session_allow_rules: Arc<Mutex<Vec<PermissionRule>>>,
}

impl TuiPermissionGate {
    /// Construct a fresh gate with the given sender and rule list.
    #[must_use]
    pub fn new(
        event_tx: mpsc::Sender<PermissionExchange>,
        session_allow_rules: Arc<Mutex<Vec<PermissionRule>>>,
    ) -> Self {
        Self {
            event_tx,
            session_allow_rules,
        }
    }
}

#[async_trait]
impl PermissionGate for TuiPermissionGate {
    async fn check(&self, name: &str, input: &serde_json::Value) -> PermissionDecision {
        // Step 1: consult session rules.
        {
            let rules = self.session_allow_rules.lock().await;
            if rules.iter().any(|r| r.matches_tool(name)) {
                return PermissionDecision::Allow;
            }
        }

        // Step 2: build request.
        let default_decision = lingxi_permission::tool_default(name);
        let request = PermissionRequest::ToolUseConfirm {
            tool_name: name.to_string(),
            tool_input: input.clone(),
            default_decision,
        };

        // Step 3: send + await.
        let (tx, rx) = oneshot::channel();
        let exchange = PermissionExchange {
            request,
            resp_tx: tx,
        };
        if self.event_tx.send(exchange).await.is_err() {
            // TUI is gone — fail closed.
            return PermissionDecision::Deny {
                reason: "TUI permission bridge closed".to_string(),
            };
        }
        let response = match rx.await {
            Ok(r) => r,
            Err(_) => {
                return PermissionDecision::Deny {
                    reason: "TUI permission response dropped".to_string(),
                };
            }
        };

        // Step 4: persist if AllowAlways.
        if matches!(response, PermissionResponse::AllowAlways) {
            self.session_allow_rules
                .lock()
                .await
                .push(PermissionRule::allow_tool_session(name));
        }

        // Step 5: map to decision.
        match response {
            PermissionResponse::AllowOnce | PermissionResponse::AllowAlways => {
                PermissionDecision::Allow
            }
            PermissionResponse::Deny => PermissionDecision::Deny {
                reason: "user denied via dialog".to_string(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn tui_gate_sends_request_and_receives_response() {
        let (event_tx, mut event_rx) = mpsc::channel::<PermissionExchange>(4);
        let rules = Arc::new(Mutex::new(Vec::new()));
        let gate = TuiPermissionGate::new(event_tx, rules.clone());

        // Spawn a "TUI" that responds AllowOnce to whatever comes in.
        let tui_task = tokio::spawn(async move {
            let ex = event_rx.recv().await.unwrap();
            // Verify the request matches the call.
            match &ex.request {
                PermissionRequest::ToolUseConfirm { tool_name, .. } => {
                    assert_eq!(tool_name, "Bash");
                }
                _ => panic!("unexpected variant"),
            }
            let _ = ex.resp_tx.send(PermissionResponse::AllowOnce);
        });

        let decision = gate.check("Bash", &json!({"command": "ls"})).await;
        assert_eq!(decision, PermissionDecision::Allow);
        tui_task.await.unwrap();

        // No rule should be persisted on AllowOnce.
        assert!(rules.lock().await.is_empty());
    }

    #[tokio::test]
    async fn tui_gate_skips_dialog_when_session_rule_matches() {
        let (event_tx, mut event_rx) = mpsc::channel::<PermissionExchange>(4);
        let rules = Arc::new(Mutex::new(vec![PermissionRule::allow_tool_session("Bash")]));
        let gate = TuiPermissionGate::new(event_tx, rules.clone());

        let decision = gate.check("Bash", &json!({"command": "ls"})).await;
        assert_eq!(decision, PermissionDecision::Allow);
        // No event should have been sent to the TUI.
        assert!(event_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn tui_gate_persists_allow_always_into_session_rules() {
        let (event_tx, mut event_rx) = mpsc::channel::<PermissionExchange>(4);
        let rules = Arc::new(Mutex::new(Vec::new()));
        let gate = TuiPermissionGate::new(event_tx, rules.clone());

        let tui_task = tokio::spawn(async move {
            let ex = event_rx.recv().await.unwrap();
            let _ = ex.resp_tx.send(PermissionResponse::AllowAlways);
        });
        let decision = gate.check("Bash", &json!({})).await;
        assert_eq!(decision, PermissionDecision::Allow);
        tui_task.await.unwrap();

        let stored = rules.lock().await;
        assert_eq!(stored.len(), 1);
        assert!(stored[0].matches_tool("Bash"));
    }

    #[tokio::test]
    async fn tui_gate_returns_deny_when_user_denies() {
        let (event_tx, mut event_rx) = mpsc::channel::<PermissionExchange>(4);
        let rules = Arc::new(Mutex::new(Vec::new()));
        let gate = TuiPermissionGate::new(event_tx, rules);

        let tui_task = tokio::spawn(async move {
            let ex = event_rx.recv().await.unwrap();
            let _ = ex.resp_tx.send(PermissionResponse::Deny);
        });
        let decision = gate.check("Bash", &json!({})).await;
        match decision {
            PermissionDecision::Deny { reason } => {
                assert!(reason.contains("user denied"), "got: {reason}");
            }
            _ => panic!("expected Deny"),
        }
        tui_task.await.unwrap();
    }

    #[tokio::test]
    async fn tui_gate_returns_deny_when_response_dropped() {
        let (event_tx, mut event_rx) = mpsc::channel::<PermissionExchange>(4);
        let rules = Arc::new(Mutex::new(Vec::new()));
        let gate = TuiPermissionGate::new(event_tx, rules);

        let tui_task = tokio::spawn(async move {
            let ex = event_rx.recv().await.unwrap();
            // Drop resp_tx without sending anything.
            drop(ex.resp_tx);
        });
        let decision = gate.check("Bash", &json!({})).await;
        match decision {
            PermissionDecision::Deny { reason } => {
                assert!(reason.contains("dropped"), "got: {reason}");
            }
            _ => panic!("expected Deny"),
        }
        tui_task.await.unwrap();
    }
}

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
use permission::gate::{
    PermissionDecision, PermissionGate, PermissionRequest, PermissionResponse, PromptWorker,
};
use permission::{
    persist_permission_update, PermissionPaths, PermissionRule, PermissionUpdate,
    PermissionUpdateDestination,
};
use tokio::sync::{mpsc, oneshot, Mutex};

/// One in-flight permission round-trip between the orchestrator and TUI.
///
/// Moved to `tui-core` (`tui_core::permission_bridge::PermissionExchange`)
/// during the iocraft → ratatui migration; re-exported so `crate::permission_bridge::PermissionExchange`
/// keeps resolving. `TuiPermissionGate` (below) still constructs it.
pub use tui_core::permission_bridge::PermissionExchange;

/// Orchestrator-side permission gate that forwards each `check` call to
/// the TUI over an mpsc channel and awaits a oneshot reply.
pub struct TuiPermissionGate {
    /// Channel into the TUI app. The receiver is owned by the TUI event
    /// loop; the orchestrator only holds the sender.
    pub event_tx: mpsc::Sender<PermissionExchange>,
    /// Session-scoped allow rules. Consulted before the dialog opens;
    /// appended to when the user picks `AllowAlways`.
    pub session_allow_rules: Arc<Mutex<Vec<PermissionRule>>>,
    /// (3c) Filesystem roots for persisting an `AllowAlways` to
    /// `settings.local.json`. `None` → session-only. NOTE: this gate is not yet
    /// wired into the production TUI runtime (the engine selects the
    /// `NoOp`/`Adapter` gate at boot), so the persist path is exercised only by
    /// tests until the TUI gate itself is wired; the capability mirrors
    /// `AdapterPermissionGate` so it is ready when that happens.
    persist_paths: Option<PermissionPaths>,
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
            persist_paths: None,
        }
    }

    /// (3c) Enable persisting an `AllowAlways` choice to `settings.local.json`.
    #[must_use]
    pub fn with_persist(mut self, paths: PermissionPaths) -> Self {
        self.persist_paths = Some(paths);
        self
    }
}

#[async_trait]
impl PermissionGate for TuiPermissionGate {
    async fn check(&self, name: &str, input: &serde_json::Value) -> PermissionDecision {
        // Main-thread call — no worker attribution.
        self.check_with_worker(name, input, None).await
    }

    async fn check_with_worker(
        &self,
        name: &str,
        input: &serde_json::Value,
        worker: Option<PromptWorker>,
    ) -> PermissionDecision {
        // Step 1: consult session rules (content-aware: a narrowed AllowAlways
        // rule only short-circuits a matching command/path/domain).
        {
            let rules = self.session_allow_rules.lock().await;
            if rules
                .iter()
                .any(|r| permission::call_matches_rule(r, name, input))
            {
                return PermissionDecision::Allow;
            }
        }

        // Step 2: build request.
        let default_decision = permission::tool_default(name);
        let request = PermissionRequest::ToolUseConfirm {
            tool_name: name.to_string(),
            tool_input: input.clone(),
            default_decision,
        };

        // Step 3: send + await. A worker-originated request carries the worker
        // identity so the dialog attributes it (claude-code's `● @name` badge).
        // `color` seeds the multiagent color (`agent_color_from_name`); the
        // wired ToolUseConfirm badge renders only the name.
        let worker_info =
            worker.map(
                |w| crate::components::permissions::worker::WorkerPermissionInfo {
                    color: w.name.clone(),
                    name: w.name,
                    team: w.team,
                },
            );
        let (tx, rx) = oneshot::channel();
        let exchange = PermissionExchange {
            request,
            resp_tx: tx,
            worker: worker_info,
        };
        if self.event_tx.send(exchange).await.is_err() {
            // TUI is gone — fail closed.
            return PermissionDecision::Deny {
                reason: "TUI permission bridge closed".to_string(),
            };
        }
        let Ok(response) = rx.await else {
            return PermissionDecision::Deny {
                reason: "TUI permission response dropped".to_string(),
            };
        };

        // Step 4: persist if AllowAlways. The rule is NARROWED to the specific
        // command / path / domain the call used (claude-code `ruleSuggestions`),
        // not a bare tool-wide allow — so "always allow" scopes the grant.
        if matches!(response, PermissionResponse::AllowAlways) {
            let rule = permission::allow_suggestion(name, input);
            self.session_allow_rules.lock().await.push(rule.clone());
            // (3c) Durably record the choice when a persist target is wired.
            // Best-effort: a write failure must not fail the check. Skip a
            // degenerate empty tool name so we never persist `allow: [""]`.
            if let Some(paths) = self.persist_paths.as_ref().filter(|_| !name.is_empty()) {
                let update = PermissionUpdate {
                    rule,
                    destination: PermissionUpdateDestination::LocalSettings,
                };
                if let Err(e) = persist_permission_update(&update, paths).await {
                    tracing::warn!(error = %e, tool = name, "failed to persist AllowAlways permission rule");
                }
            }
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
    async fn tui_gate_attributes_worker_on_the_exchange() {
        // A worker-originated check carries the worker identity onto the
        // PermissionExchange, so the dialog renders the `● @name` badge.
        let (event_tx, mut event_rx) = mpsc::channel::<PermissionExchange>(4);
        let rules = Arc::new(Mutex::new(Vec::new()));
        let gate = TuiPermissionGate::new(event_tx, rules);
        let tui_task = tokio::spawn(async move {
            let ex = event_rx.recv().await.unwrap();
            let w = ex.worker.clone().expect("worker attributed");
            assert_eq!(w.name, "researcher");
            assert_eq!(w.team.as_deref(), Some("alpha"));
            let _ = ex.resp_tx.send(PermissionResponse::AllowOnce);
        });
        let decision = gate
            .check_with_worker(
                "Bash",
                &json!({"command": "ls"}),
                Some(PromptWorker {
                    name: "researcher".to_string(),
                    team: Some("alpha".to_string()),
                    is_async: true,
                }),
            )
            .await;
        assert_eq!(decision, PermissionDecision::Allow);
        tui_task.await.unwrap();
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
            PermissionDecision::Allow => panic!("expected Deny"),
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
            PermissionDecision::Allow => panic!("expected Deny"),
        }
        tui_task.await.unwrap();
    }

    /// [P2] The real seam: PolicyPermissionGate (default mode, no rules) wraps
    /// the injected TuiPermissionGate. A mutating tool with no rule is an
    /// unresolved `Ask` → delegated to the inner gate → one exchange on the
    /// channel → the reply resolves the blocked check().
    #[tokio::test]
    async fn policy_gate_delegates_unresolved_ask_to_tui_gate() {
        use permission::gate::{PermissionDecision, PermissionGate, PermissionResponse};
        use permission::{PermissionMode, PermissionPolicy};
        use std::sync::Arc;

        let (event_tx, mut event_rx) = mpsc::channel::<PermissionExchange>(4);
        let rules = Arc::new(Mutex::new(Vec::new()));
        let inner: Arc<dyn PermissionGate> = Arc::new(TuiPermissionGate::new(event_tx, rules));
        let policy = Arc::new(PermissionPolicy::new(PermissionMode::Default));
        let gate = permission::PolicyPermissionGate::new(policy, inner);

        // A "TUI" that answers AllowOnce to the first exchange.
        let responder = tokio::spawn(async move {
            let ex = event_rx.recv().await.expect("one exchange expected");
            // It must be the Write tool we asked for.
            if let permission::gate::PermissionRequest::ToolUseConfirm { tool_name, .. } =
                &ex.request
            {
                assert_eq!(tool_name, "Write");
            } else {
                panic!("expected ToolUseConfirm");
            }
            ex.resp_tx.send(PermissionResponse::AllowOnce).unwrap();
        });

        let decision = gate.check("Write", &json!({"file_path": "a.txt"})).await;
        assert!(matches!(decision, PermissionDecision::Allow));
        responder.await.unwrap();
    }

    /// [P2] A Deny reply resolves the blocked check() to Deny.
    #[tokio::test]
    async fn policy_gate_relays_deny_from_tui_gate() {
        use permission::gate::{PermissionDecision, PermissionGate, PermissionResponse};
        use permission::{PermissionMode, PermissionPolicy};
        use std::sync::Arc;

        let (event_tx, mut event_rx) = mpsc::channel::<PermissionExchange>(4);
        let rules = Arc::new(Mutex::new(Vec::new()));
        let inner: Arc<dyn PermissionGate> = Arc::new(TuiPermissionGate::new(event_tx, rules));
        let policy = Arc::new(PermissionPolicy::new(PermissionMode::Default));
        let gate = permission::PolicyPermissionGate::new(policy, inner);

        let responder = tokio::spawn(async move {
            let ex = event_rx.recv().await.expect("one exchange");
            ex.resp_tx.send(PermissionResponse::Deny).unwrap();
        });

        let decision = gate.check("Write", &json!({})).await;
        assert!(matches!(decision, PermissionDecision::Deny { .. }));
        responder.await.unwrap();
    }

    /// [P2] A read-only tool (AllowByDefault) is resolved by the policy itself —
    /// the inner TuiPermissionGate is NEVER consulted (no exchange emitted).
    #[tokio::test]
    async fn policy_gate_auto_allows_readonly_without_dialog() {
        use permission::gate::{PermissionDecision, PermissionGate};
        use permission::{PermissionMode, PermissionPolicy};
        use std::sync::Arc;

        let (event_tx, mut event_rx) = mpsc::channel::<PermissionExchange>(4);
        let rules = Arc::new(Mutex::new(Vec::new()));
        let inner: Arc<dyn PermissionGate> = Arc::new(TuiPermissionGate::new(event_tx, rules));
        let policy = Arc::new(PermissionPolicy::new(PermissionMode::Default));
        let gate = permission::PolicyPermissionGate::new(policy, inner);

        let decision = gate.check("Read", &json!({"file_path": "a.txt"})).await;
        assert!(matches!(decision, PermissionDecision::Allow));
        // No exchange should have been emitted.
        assert!(
            matches!(event_rx.try_recv(), Err(mpsc::error::TryRecvError::Empty)),
            "read-only tool must not consult the interactive gate"
        );
    }
}

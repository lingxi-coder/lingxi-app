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
    PermissionCheckContext, PermissionDecision, PermissionGate, PermissionOutcome,
    PermissionRequest, PermissionResponse, PromptWorker,
};
use permission::{persist_permission_update, PermissionPaths, PermissionRule, PermissionUpdate};
use tokio::sync::{mpsc, oneshot, Mutex};

/// One in-flight permission round-trip between the orchestrator and TUI.
///
/// Moved to `tui-core` (`tui_core::permission_bridge::PermissionExchange`)
/// during the iocraft → ratatui migration; re-exported so `crate::permission_bridge::PermissionExchange`
/// keeps resolving. `TuiPermissionGate` (below) still constructs it.
use tui_core::permission_bridge::PermissionExchange;

type PolicyRuleAuthority = dyn Fn(&str, &serde_json::Value) -> bool + Send + Sync;

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
    /// `settings.local.json`. `None` → session-only. The CLI TUI composition
    /// root wires these paths before the engine wraps this transport in its
    /// policy gate.
    persist_paths: Option<PermissionPaths>,
    persist_cwd: std::sync::OnceLock<Arc<dyn Fn() -> std::path::PathBuf + Send + Sync>>,
    policy_rule_authority: std::sync::OnceLock<Arc<PolicyRuleAuthority>>,
    persistence_enabled: std::sync::atomic::AtomicBool,
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
            persist_cwd: std::sync::OnceLock::new(),
            policy_rule_authority: std::sync::OnceLock::new(),
            persistence_enabled: std::sync::atomic::AtomicBool::new(true),
        }
    }

    /// (3c) Enable persisting an `AllowAlways` choice to `settings.local.json`.
    #[must_use]
    pub fn with_persist(mut self, paths: PermissionPaths) -> Self {
        self.persist_paths = Some(paths);
        self
    }

    /// Resolve project/local persistence from the same live cwd as file tools.
    pub fn set_persistence_cwd_provider(
        &self,
        read: Arc<dyn Fn() -> std::path::PathBuf + Send + Sync>,
    ) {
        let _ = self.persist_cwd.set(read);
    }

    /// When an outer policy owns rules, a delegated Ask must reach the human.
    /// Its source/layer-aware decision takes precedence over this legacy cache.
    pub fn set_policy_rule_authority(&self, read: Arc<PolicyRuleAuthority>) {
        let _ = self.policy_rule_authority.set(read);
    }

    fn persistence_paths(&self) -> Option<PermissionPaths> {
        let mut paths = self.persist_paths.clone()?;
        if let Some(read) = self.persist_cwd.get() {
            paths.cwd = read();
        }
        Some(paths)
    }
}

#[async_trait]
impl PermissionGate for TuiPermissionGate {
    fn set_permission_persistence_enabled(&self, enabled: bool) {
        self.persistence_enabled
            .store(enabled, std::sync::atomic::Ordering::Release);
    }

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
        match self
            .check_with_context_impl(name, input, worker, false, None, None, false)
            .await
        {
            PermissionOutcome::Allow { .. } | PermissionOutcome::AllowAuto { .. } => {
                PermissionDecision::Allow
            }
            PermissionOutcome::Deny { reason } => PermissionDecision::Deny { reason },
        }
    }

    /// §27b — carries [`PermissionCheckContext::requires_user_interaction`]
    /// through to the dialog so it can suppress "Yes, allow always"
    /// (oracle `suppressesAlwaysAllowRule`, @182520462) for a tool that needs
    /// fresh interaction on every call. The default trait impl would drop
    /// `ctx` entirely (forwarding only `ctx.worker` into
    /// `check_with_worker`), so this override is required to reach the bit.
    async fn check_with_context(
        &self,
        name: &str,
        input: &serde_json::Value,
        ctx: &PermissionCheckContext,
    ) -> PermissionOutcome {
        match self
            .check_with_context_impl(
                name,
                input,
                ctx.worker.clone(),
                ctx.suppress_always_allow_rule || ctx.requires_user_interaction,
                ctx.permission_suggestions.as_ref().and_then(|suggestions| {
                    permission::allow_suggestion::permission_persistence_suggestion(
                        name,
                        suggestions,
                    )
                }),
                ctx.auto_mode_prompt,
                ctx.background_owned,
            )
            .await
        {
            outcome => outcome,
        }
    }
}

impl TuiPermissionGate {
    /// Shared body behind [`PermissionGate::check_with_worker`] and
    /// [`PermissionGate::check_with_context`]. Auto eligibility is supplied by
    /// the engine; this transport validates that the prompt kind agrees with
    /// the tool name before exposing the row.
    async fn check_with_context_impl(
        &self,
        name: &str,
        input: &serde_json::Value,
        worker: Option<PromptWorker>,
        suppress_always_allow_rule: bool,
        permission_persistence: Option<
            permission::allow_suggestion::PermissionPersistenceSuggestion,
        >,
        auto_mode_prompt: Option<permission::gate::AutoModePrompt>,
        // Finding 22: `true` only when `ctx.background_owned` was set by an
        // invoker the composition root built exclusively for a background
        // task (see `PermissionCheckContext::background_owned`). `check`/
        // `check_with_worker` (no `ctx`) always pass `false`, matching prior
        // behavior for every main-thread / worker-only call.
        background_owned: bool,
    ) -> PermissionOutcome {
        let auto_mode_prompt = match auto_mode_prompt {
            Some(permission::gate::AutoModePrompt::ExitPlanMode) if name == "ExitPlanMode" => {
                Some(permission::gate::AutoModePrompt::ExitPlanMode)
            }
            Some(permission::gate::AutoModePrompt::WorkflowBash) if name != "ExitPlanMode" => {
                Some(permission::gate::AutoModePrompt::WorkflowBash)
            }
            _ => None,
        };
        let permission_persistence = self
            .persistence_enabled
            .load(std::sync::atomic::Ordering::Acquire)
            .then_some(permission_persistence)
            .flatten()
            .filter(|_| !suppress_always_allow_rule && auto_mode_prompt.is_none());
        let allow_always_offered = if name == "ExitPlanMode" {
            !suppress_always_allow_rule
        } else {
            !suppress_always_allow_rule
                && auto_mode_prompt.is_none()
                && permission_persistence.is_some()
        };
        // Step 1: consult session rules (content-aware: a narrowed AllowAlways
        // rule only short-circuits a matching command/path/domain). A tool that
        // requires a human decision on every invocation must not be bypassed by
        // an older session rule.
        {
            let rules = self.session_allow_rules.lock().await;
            let policy_owns_rules = self
                .policy_rule_authority
                .get()
                .is_some_and(|read| read(name, input));
            if !policy_owns_rules
                && !suppress_always_allow_rule
                && rules
                    .iter()
                    .any(|r| permission::call_matches_rule(r, name, input))
            {
                return PermissionOutcome::Allow {
                    updated_input: None,
                    permission_updates: Vec::new(),
                    decision_classification: None,
                };
            }
        }

        // Step 2: build request.
        let default_decision = permission::tool_default(name);
        let request = if name == "ExitPlanMode" {
            PermissionRequest::ExitPlanMode {
                plan: input
                    .get("plan")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            }
        } else {
            PermissionRequest::ToolUseConfirm {
                tool_name: name.to_string(),
                tool_input: input.clone(),
                default_decision,
                suppress_always_allow_rule,
            }
        };

        // Step 3: send + await. A worker-originated request carries the worker
        // identity so the dialog attributes it (claude-code's `● @name` badge).
        // `color` seeds the multiagent color (`agent_color_from_name`); the
        // wired ToolUseConfirm badge renders only the name.
        let worker_info = worker.map(|w| tui_core::permission_bridge::WorkerPermissionInfo {
            color: w.name.clone(),
            name: w.name,
            team: w.team,
        });
        let (tx, rx) = oneshot::channel();
        let exchange = PermissionExchange {
            request,
            resp_tx: tx,
            worker: worker_info,
            suppress_always_allow_rule,
            permission_persistence: permission_persistence.clone(),
            auto_mode_prompt,
            background_owned,
        };
        // G006: this deny reason can reach the calling MODEL as a tool-error
        // string (a background `/fusion` panel's Bash/WebFetch call denied
        // here feeds straight into that panel's next turn) — model-neutral
        // copy only, no transport-internal words ("TUI"/"dropped") a
        // provider model has no context for.
        if self.event_tx.send(exchange).await.is_err() {
            // TUI is gone — fail closed.
            return PermissionOutcome::Deny {
                reason: "permission request could not be delivered".to_string(),
            };
        }
        let Ok(response) = rx.await else {
            return PermissionOutcome::Deny {
                reason: "permission request was not resolved".to_string(),
            };
        };

        // Step 4: persist if AllowAlways. The rule is NARROWED to the specific
        // command / path / domain the call used (claude-code `ruleSuggestions`),
        // not a bare tool-wide allow — so "always allow" scopes the grant.
        //
        // `allow_always_offered` encodes the only cases where the transport
        // rendered a second persistent-approval row. Any other AllowAlways
        // response is stale/malicious input and must not record a rule.
        let selected_persistence = matches!(response, PermissionResponse::AllowAlways)
            .then_some(permission_persistence.as_ref())
            .flatten()
            .filter(|_| {
                allow_always_offered
                    && self
                        .persistence_enabled
                        .load(std::sync::atomic::Ordering::Acquire)
            });
        if let Some(suggestion) = selected_persistence {
            let rule = suggestion.rule.clone();
            let mut session_rules = self.session_allow_rules.lock().await;
            if !session_rules.iter().any(|existing| existing == &rule) {
                session_rules.push(rule.clone());
            }
            drop(session_rules);
            // (3c) Durably record the choice when a persist target is wired.
            // Best-effort: a write failure must not fail the check. Skip a
            // degenerate empty tool name so we never persist `allow: [""]`.
            if let Some(paths) = self.persistence_paths().filter(|_| {
                !name.is_empty()
                    && self
                        .persistence_enabled
                        .load(std::sync::atomic::Ordering::Acquire)
            }) {
                let update = PermissionUpdate {
                    rule,
                    destination: suggestion.destination,
                };
                if let Err(e) = persist_permission_update(&update, &paths).await {
                    tracing::warn!(error = %e, tool = name, "failed to persist AllowAlways permission rule");
                }
            }
        }

        // Step 5: map to decision.
        match response {
            PermissionResponse::AllowOnce => PermissionOutcome::Allow {
                updated_input: None,
                permission_updates: Vec::new(),
                decision_classification: None,
            },
            PermissionResponse::AllowAlways if allow_always_offered => PermissionOutcome::Allow {
                updated_input: None,
                permission_updates: selected_persistence
                    .map(|suggestion| vec![suggestion.update.clone()])
                    .unwrap_or_default(),
                decision_classification: None,
            },
            // A stale or malicious responder cannot select "don't ask again"
            // unless the engine supplied a real persistence suggestion. Preserve
            // the approval as a one-shot grant without recording a rule.
            PermissionResponse::AllowAlways => PermissionOutcome::Allow {
                updated_input: None,
                permission_updates: Vec::new(),
                decision_classification: None,
            },
            PermissionResponse::AllowAuto
                if auto_mode_prompt.is_some() && !suppress_always_allow_rule =>
            {
                PermissionOutcome::AllowAuto {
                    updated_input: None,
                }
            }
            // A stale or malicious responder cannot select Auto unless the
            // engine marked this request eligible. Preserve the approval as a
            // one-shot grant without a mode transition.
            PermissionResponse::AllowAuto => PermissionOutcome::Allow {
                updated_input: None,
                permission_updates: Vec::new(),
                decision_classification: None,
            },
            PermissionResponse::Deny => PermissionOutcome::Deny {
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
    async fn allow_always_after_cd_writes_current_project() {
        let home = tempfile::tempdir().unwrap();
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let (event_tx, mut event_rx) = mpsc::channel::<PermissionExchange>(4);
        let gate = TuiPermissionGate::new(event_tx, Arc::new(Mutex::new(Vec::new()))).with_persist(
            PermissionPaths {
                lingxi_home: home.path().to_owned(),
                cwd: first.path().to_owned(),
            },
        );
        let cwd = tool_api::SessionCwd::new(first.path().to_owned(), vec![first.path().to_owned()]);
        gate.set_persistence_cwd_provider({
            let cwd = cwd.clone();
            Arc::new(move || cwd.cwd())
        });
        cwd.change_cwd(second.path().to_owned());
        let ctx = PermissionCheckContext {
            permission_suggestions: Some(json!([{
                "type": "addRules", "rules": [{"toolName": "Bash", "ruleContent": "git diff *"}],
                "behavior": "allow", "destination": "localSettings"
            }])),
            ..Default::default()
        };
        let responder = tokio::spawn(async move {
            let exchange = event_rx.recv().await.unwrap();
            assert!(exchange.permission_persistence.is_some());
            exchange
                .resp_tx
                .send(PermissionResponse::AllowAlways)
                .unwrap();
        });
        let outcome = gate
            .check_with_context("Bash", &json!({"command": "git diff --stat"}), &ctx)
            .await;
        assert!(matches!(outcome, PermissionOutcome::Allow { .. }));
        responder.await.unwrap();
        assert!(!first
            .path()
            .join(branding::DOT_DIR)
            .join("settings.local.json")
            .exists());
        let body = std::fs::read_to_string(
            second
                .path()
                .join(branding::DOT_DIR)
                .join("settings.local.json"),
        )
        .unwrap();
        assert!(body.contains("Bash(git diff *)"), "{body}");
        assert!(!home.path().join("settings.json").exists());
    }

    #[tokio::test]
    async fn tui_gate_persists_allow_always_into_session_rules() {
        let (event_tx, mut event_rx) = mpsc::channel::<PermissionExchange>(4);
        let rules = Arc::new(Mutex::new(Vec::new()));
        let gate = TuiPermissionGate::new(event_tx, rules.clone());
        let ctx = PermissionCheckContext {
            permission_suggestions: Some(json!([
                {
                    "type": "addRules",
                    "rules": [{"toolName": "Bash", "ruleContent": "git diff *"}],
                    "behavior": "allow",
                    "destination": "session"
                }
            ])),
            ..Default::default()
        };

        let tui_task = tokio::spawn(async move {
            let ex = event_rx.recv().await.unwrap();
            assert_eq!(
                ex.permission_persistence
                    .as_ref()
                    .map(|suggestion| suggestion.label.as_str()),
                Some("Yes, and don't ask again for git diff *")
            );
            let _ = ex.resp_tx.send(PermissionResponse::AllowAlways);
        });
        let outcome = gate
            .check_with_context("Bash", &json!({"command": "git status"}), &ctx)
            .await;
        let PermissionOutcome::Allow {
            permission_updates, ..
        } = outcome
        else {
            panic!("expected allow outcome");
        };
        assert_eq!(
            permission_updates,
            vec![ctx.permission_suggestions.unwrap()[0].clone()]
        );
        tui_task.await.unwrap();

        let stored = rules.lock().await;
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].value.rule_content.as_deref(), Some("git diff *"));
    }

    // ===== §27b: `check_with_context` / `suppress_always_allow_rule` =====

    #[tokio::test]
    async fn check_with_context_ignores_ctx_by_default_on_check_with_worker() {
        // `check`/`check_with_worker` never see `permission_suggestions`, so
        // the transport must fail closed and omit the persistence row.
        let (event_tx, mut event_rx) = mpsc::channel::<PermissionExchange>(4);
        let rules = Arc::new(Mutex::new(Vec::new()));
        let gate = TuiPermissionGate::new(event_tx, rules);
        let tui_task = tokio::spawn(async move {
            let ex = event_rx.recv().await.unwrap();
            match &ex.request {
                PermissionRequest::ToolUseConfirm {
                    suppress_always_allow_rule,
                    ..
                } => assert!(!suppress_always_allow_rule),
                _ => panic!("unexpected variant"),
            }
            assert_eq!(ex.permission_persistence, None);
            let _ = ex.resp_tx.send(PermissionResponse::AllowOnce);
        });
        let _ = gate.check_with_worker("Bash", &json!({}), None).await;
        tui_task.await.unwrap();
    }

    #[tokio::test]
    async fn check_with_context_forwards_requires_user_interaction_as_suppress_bit() {
        // `PermissionCheckContext::requires_user_interaction: true` (e.g. an
        // MCP tool whose `_meta.anthropic/requiresUserInteraction === true`)
        // must reach the dialog as `suppress_always_allow_rule: true` — the
        // oracle's `suppressesAlwaysAllowRule` (@182520462).
        let (event_tx, mut event_rx) = mpsc::channel::<PermissionExchange>(4);
        let rules = Arc::new(Mutex::new(Vec::new()));
        let gate = TuiPermissionGate::new(event_tx, rules);
        let tui_task = tokio::spawn(async move {
            let ex = event_rx.recv().await.unwrap();
            match &ex.request {
                PermissionRequest::ToolUseConfirm {
                    suppress_always_allow_rule,
                    ..
                } => assert!(suppress_always_allow_rule),
                _ => panic!("unexpected variant"),
            }
            let _ = ex.resp_tx.send(PermissionResponse::AllowOnce);
        });
        let ctx = permission::gate::PermissionCheckContext {
            requires_user_interaction: true,
            ..Default::default()
        };
        let outcome = gate
            .check_with_context("mcp__server__tool", &json!({}), &ctx)
            .await;
        assert_eq!(
            outcome,
            PermissionOutcome::Allow {
                updated_input: None,
                permission_updates: Vec::new(),
                decision_classification: None,
            }
        );
        tui_task.await.unwrap();
    }

    #[tokio::test]
    async fn check_with_context_never_persists_allow_always_when_suppressed() {
        // Defense in depth: even if a view somehow answered `AllowAlways` for
        // a suppressed request, `TuiPermissionGate` must not record a
        // persistent rule for it.
        let (event_tx, mut event_rx) = mpsc::channel::<PermissionExchange>(4);
        let rules = Arc::new(Mutex::new(Vec::new()));
        let gate = TuiPermissionGate::new(event_tx, rules.clone());
        let tui_task = tokio::spawn(async move {
            let ex = event_rx.recv().await.unwrap();
            assert_eq!(ex.permission_persistence, None);
            let _ = ex.resp_tx.send(PermissionResponse::AllowAlways);
        });
        let ctx = permission::gate::PermissionCheckContext {
            requires_user_interaction: true,
            permission_suggestions: Some(json!([{
                "type": "addRules",
                "rules": [{"toolName": "mcp__server__tool"}],
                "behavior": "allow",
                "destination": "session"
            }])),
            ..Default::default()
        };
        let outcome = gate
            .check_with_context("mcp__server__tool", &json!({}), &ctx)
            .await;
        assert!(matches!(outcome, PermissionOutcome::Allow { .. }));
        tui_task.await.unwrap();
        assert!(
            rules.lock().await.is_empty(),
            "a suppressed AllowAlways must never be persisted"
        );
    }

    #[tokio::test]
    async fn managed_only_mode_suppresses_permission_persistence() {
        let (event_tx, mut event_rx) = mpsc::channel::<PermissionExchange>(4);
        let rules = Arc::new(Mutex::new(Vec::new()));
        let gate = TuiPermissionGate::new(event_tx, rules.clone());
        gate.set_permission_persistence_enabled(false);
        let task = tokio::spawn(async move {
            let exchange = event_rx.recv().await.unwrap();
            assert_eq!(exchange.permission_persistence, None);
            exchange
                .resp_tx
                .send(PermissionResponse::AllowAlways)
                .unwrap();
        });
        let ctx = PermissionCheckContext {
            permission_suggestions: Some(json!([{
                "type": "addRules",
                "rules": [{"toolName": "Bash", "ruleContent": "git diff *"}],
                "behavior": "allow",
                "destination": "localSettings"
            }])),
            ..Default::default()
        };
        let outcome = gate
            .check_with_context("Bash", &json!({"command": "git diff"}), &ctx)
            .await;
        task.await.unwrap();
        assert!(rules.lock().await.is_empty());
        assert!(matches!(
            outcome,
            PermissionOutcome::Allow {
                permission_updates,
                ..
            } if permission_updates.is_empty()
        ));
    }

    #[tokio::test]
    async fn check_with_context_omits_allow_always_when_no_suggestion_exists() {
        let (event_tx, mut event_rx) = mpsc::channel::<PermissionExchange>(4);
        let rules = Arc::new(Mutex::new(Vec::new()));
        let gate = TuiPermissionGate::new(event_tx, rules.clone());
        let tui_task = tokio::spawn(async move {
            let ex = event_rx.recv().await.unwrap();
            assert_eq!(ex.permission_persistence, None);
            let _ = ex.resp_tx.send(PermissionResponse::AllowAlways);
        });
        let outcome = gate
            .check_with_context(
                "Bash",
                &json!({"command": "git status"}),
                &Default::default(),
            )
            .await;
        assert!(matches!(outcome, PermissionOutcome::Allow { .. }));
        tui_task.await.unwrap();
        assert!(
            rules.lock().await.is_empty(),
            "AllowAlways without a suggestion must not persist a rule"
        );
    }

    #[tokio::test]
    async fn eligible_auto_response_is_rich_outcome_without_session_rule() {
        let (event_tx, mut event_rx) = mpsc::channel::<PermissionExchange>(4);
        let rules = Arc::new(Mutex::new(Vec::new()));
        let gate = TuiPermissionGate::new(event_tx, rules.clone());
        let ctx = PermissionCheckContext {
            auto_mode_prompt: Some(permission::gate::AutoModePrompt::WorkflowBash),
            ..Default::default()
        };
        let task = tokio::spawn(async move {
            gate.check_with_context("Bash", &json!({"command": "echo hi"}), &ctx)
                .await
        });
        let exchange = event_rx.recv().await.unwrap();
        assert_eq!(
            exchange.auto_mode_prompt,
            Some(permission::gate::AutoModePrompt::WorkflowBash)
        );
        exchange
            .resp_tx
            .send(PermissionResponse::AllowAuto)
            .unwrap();
        assert!(matches!(
            task.await.unwrap(),
            PermissionOutcome::AllowAuto { .. }
        ));
        assert!(rules.lock().await.is_empty());
    }

    #[tokio::test]
    async fn eligible_exit_plan_uses_plan_request_and_auto_response() {
        let (event_tx, mut event_rx) = mpsc::channel::<PermissionExchange>(4);
        let rules = Arc::new(Mutex::new(Vec::new()));
        let gate = TuiPermissionGate::new(event_tx, rules);
        let ctx = PermissionCheckContext {
            auto_mode_prompt: Some(permission::gate::AutoModePrompt::ExitPlanMode),
            ..Default::default()
        };
        let task = tokio::spawn(async move {
            gate.check_with_context("ExitPlanMode", &json!({"plan": "1. Ship it"}), &ctx)
                .await
        });
        let exchange = event_rx.recv().await.unwrap();
        match exchange.request {
            PermissionRequest::ExitPlanMode { plan } => assert_eq!(plan, "1. Ship it"),
            other => panic!("unexpected request: {other:?}"),
        }
        assert_eq!(
            exchange.auto_mode_prompt,
            Some(permission::gate::AutoModePrompt::ExitPlanMode)
        );
        exchange
            .resp_tx
            .send(PermissionResponse::AllowAuto)
            .unwrap();
        assert!(matches!(
            task.await.unwrap(),
            PermissionOutcome::AllowAuto { .. }
        ));
    }

    #[tokio::test]
    async fn tui_gate_maps_eligible_auto_to_rich_outcome_without_persistence() {
        let (event_tx, mut event_rx) = mpsc::channel::<PermissionExchange>(4);
        let rules = Arc::new(Mutex::new(Vec::new()));
        let gate = TuiPermissionGate::new(event_tx, rules.clone());
        let ctx = permission::gate::PermissionCheckContext {
            auto_mode_prompt: Some(permission::gate::AutoModePrompt::WorkflowBash),
            ..Default::default()
        };
        let task = tokio::spawn(async move {
            gate.check_with_context("Bash", &json!({"command": "echo hi"}), &ctx)
                .await
        });
        let exchange = event_rx.recv().await.unwrap();
        assert_eq!(
            exchange.auto_mode_prompt,
            Some(permission::gate::AutoModePrompt::WorkflowBash)
        );
        exchange
            .resp_tx
            .send(PermissionResponse::AllowAuto)
            .unwrap();
        assert!(matches!(
            task.await.unwrap(),
            permission::gate::PermissionOutcome::AllowAuto { .. }
        ));
        assert!(rules.lock().await.is_empty());
    }

    #[tokio::test]
    async fn tui_gate_maps_exit_plan_to_plan_request_with_payload() {
        let (event_tx, mut event_rx) = mpsc::channel::<PermissionExchange>(4);
        let rules = Arc::new(Mutex::new(Vec::new()));
        let gate = TuiPermissionGate::new(event_tx, rules);
        let ctx = permission::gate::PermissionCheckContext {
            auto_mode_prompt: Some(permission::gate::AutoModePrompt::ExitPlanMode),
            ..Default::default()
        };
        let task = tokio::spawn(async move { gate.check_exit_plan_mode("1. Ship it", &ctx).await });
        let exchange = event_rx.recv().await.unwrap();
        match exchange.request {
            PermissionRequest::ExitPlanMode { plan } => assert_eq!(plan, "1. Ship it"),
            other => panic!("unexpected request: {other:?}"),
        }
        assert_eq!(
            exchange.auto_mode_prompt,
            Some(permission::gate::AutoModePrompt::ExitPlanMode)
        );
        exchange
            .resp_tx
            .send(PermissionResponse::AllowAuto)
            .unwrap();
        assert!(matches!(
            task.await.unwrap(),
            permission::gate::PermissionOutcome::AllowAuto { .. }
        ));
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
                // G006: this reason can reach a (possibly foreign) model as
                // raw tool-error text — model-neutral copy, no
                // transport-internal jargon.
                assert_eq!(reason, "permission request was not resolved");
                assert!(
                    !reason.contains("TUI") && !reason.contains("dropped"),
                    "got: {reason}"
                );
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

    // ---- G006: deny reasons the runner feeds back to a (possibly foreign)
    // model as raw tool-error text must be model-neutral copy, not
    // transport-internal jargon ("TUI"/"dropped"). The dropped-response-
    // channel case is covered above by
    // `tui_gate_returns_deny_when_response_dropped`; this covers the other
    // deny path — the event channel itself already closed. ---------------

    #[tokio::test]
    async fn closed_event_channel_denies_with_model_neutral_copy() {
        let (event_tx, event_rx) = mpsc::channel::<PermissionExchange>(4);
        // Drop the receiver up front so `event_tx.send(..)` fails immediately
        // (the TUI event loop has already gone away).
        drop(event_rx);
        let rules = Arc::new(Mutex::new(Vec::new()));
        let gate = TuiPermissionGate::new(event_tx, rules);

        let decision = gate.check("Bash", &json!({"command": "ls"})).await;
        let PermissionDecision::Deny { reason } = decision else {
            panic!("expected deny, got {decision:?}");
        };
        assert!(
            !reason.contains("TUI"),
            "deny reason must not leak transport-internal jargon to the model: {reason:?}"
        );
        assert_eq!(reason, "permission request could not be delivered");
    }
}

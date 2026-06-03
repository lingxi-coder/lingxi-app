//! `PolicyPermissionGate` — the enforcing gate (permission enforcement phase 2).
//!
//! Bridges the rule engine ([`PermissionPolicy::authorize`], 3-valued
//! Allow/Deny/Ask) onto the 2-valued [`PermissionGate::check`] contract the
//! orchestrator consults at its single dispatch chokepoint. It wraps a loaded
//! [`PermissionPolicy`] plus an INNER [`PermissionGate`] (the existing prompting
//! transport — `InteractivePromptingGate` / `TuiPermissionGate` /
//! `AdapterPermissionGate` / a no-op), and maps:
//!
//! - `authorize` → `Allow`  ⇒ [`PermissionDecision::Allow`].
//! - `authorize` → `Deny`   ⇒ [`PermissionDecision::Deny`] (reason rendered).
//! - `authorize` → `Ask`    ⇒ either AUTO-ALLOW when the tool is read-only /
//!   agent-local ([`tool_default`] is [`PromptDefault::AllowByDefault`]) — a
//!   phase-2 stand-in for claude-code's per-tool default-allow that avoids an
//!   ask-storm on every `Read`/`Glob`/etc. — or DELEGATE to `inner.check(...)`
//!   so the prompt surfaces through whatever transport the host wired.
//!
//! ## Scope
//! `authorize` matches TOOL-WIDE rules + mode for all tools, and per-tool FILE
//! PATH content matching for file tools (phase 3a — `Edit(src/**)` /
//! `Read(./secrets/**)`). `Bash`/`WebFetch` content matching (e.g.
//! `Bash(npm run *)`) is still tool-wide (the 3a-bash deferral). The subagent
//! path ([`RegistryToolInvoker`]) is now ALSO gated (phase 3b — closed the
//! bypass; subagent + teammate tool calls consult this same gate). `Plan` /
//! `AcceptEdits` modes still COLLAPSE to plain `Ask` (the mode fallback only
//! special-cases `BypassPermissions` + `DontAsk`), so they do NOT yet impose
//! `Plan`'s mutation block or `AcceptEdits`' edit auto-allow. This gate is built
//! at boot only behind an OPT-IN toggle; the default remains the always-allow
//! `NoOpPermissionGate`.

use crate::defaults_per_tool::tool_default;
use crate::gate::{PermissionDecision, PermissionGate, PromptDefault};
use crate::policy::PermissionPolicy;
use crate::result::{PermissionDecisionReason, PermissionResult};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

/// Enforcing gate over a [`PermissionPolicy`] (see module docs).
pub struct PolicyPermissionGate {
    /// The loaded rule policy. `authorize` reads its allow/deny/ask buckets.
    policy: Arc<PermissionPolicy>,
    /// Prompt transport consulted only when `authorize` returns `Ask` for a
    /// non-read-only tool. Reusing an existing `PermissionGate` keeps all the
    /// parking/oneshot machinery in one place.
    inner: Arc<dyn PermissionGate>,
}

impl PolicyPermissionGate {
    /// Wrap `policy` with `inner` as the `Ask`-delegation prompt transport.
    #[must_use]
    pub fn new(policy: Arc<PermissionPolicy>, inner: Arc<dyn PermissionGate>) -> Self {
        Self { policy, inner }
    }
}

#[async_trait]
impl PermissionGate for PolicyPermissionGate {
    async fn check(&self, name: &str, input: &Value) -> PermissionDecision {
        match self.policy.authorize(name, input) {
            PermissionResult::Allow { .. } => PermissionDecision::Allow,
            PermissionResult::Deny {
                reason,
                explanation,
                ..
            } => PermissionDecision::Deny {
                reason: explanation.unwrap_or_else(|| deny_reason_string(&reason)),
            },
            PermissionResult::Ask { .. } => {
                if matches!(tool_default(name), PromptDefault::AllowByDefault) {
                    // Read-only / agent-local tool — auto-allow rather than
                    // ask-storm. (phase-2 stand-in for the per-tool default.)
                    PermissionDecision::Allow
                } else {
                    // Surface the prompt through the host's transport.
                    self.inner.check(name, input).await
                }
            }
        }
    }
}

/// Render a [`PermissionDecisionReason`] to the human/model-facing deny string
/// (used when the `Deny` carries no explicit `explanation`).
fn deny_reason_string(reason: &PermissionDecisionReason) -> String {
    match reason {
        PermissionDecisionReason::MatchedRule { rule } => {
            format!("denied by permission rule {}", rule.value.to_rule_string())
        }
        PermissionDecisionReason::PermissionMode { mode } => {
            format!("denied by permission mode {mode:?}")
        }
        _ => "permission denied".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mode::PermissionMode;
    use crate::rule::PermissionRuleSource;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Records how many times its `check` is called, returning a fixed decision.
    struct RecordingInner {
        calls: AtomicUsize,
        decision: PermissionDecision,
    }
    impl RecordingInner {
        fn new(decision: PermissionDecision) -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicUsize::new(0),
                decision,
            })
        }
        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }
    #[async_trait]
    impl PermissionGate for RecordingInner {
        async fn check(&self, _name: &str, _input: &Value) -> PermissionDecision {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.decision.clone()
        }
    }

    fn policy_with(raw: &str, mode: PermissionMode) -> Arc<PermissionPolicy> {
        let rules = crate::loader::permission_rules_from_settings_json(
            raw,
            PermissionRuleSource::UserSettings,
        )
        .unwrap();
        Arc::new(PermissionPolicy::from_rules(mode, rules))
    }

    #[tokio::test]
    async fn allow_rule_short_circuits_without_prompting() {
        let policy = policy_with(r#"{ "permissions": { "allow": ["Bash"] } }"#, PermissionMode::Default);
        let inner = RecordingInner::new(PermissionDecision::Deny {
            reason: "should not be reached".into(),
        });
        let gate = PolicyPermissionGate::new(policy, inner.clone());
        assert_eq!(
            gate.check("Bash", &serde_json::json!({})).await,
            PermissionDecision::Allow
        );
        assert_eq!(inner.calls(), 0, "allow rule must not prompt");
    }

    #[tokio::test]
    async fn deny_rule_denies_with_rendered_reason_without_prompting() {
        let policy = policy_with(r#"{ "permissions": { "deny": ["Bash"] } }"#, PermissionMode::Default);
        let inner = RecordingInner::new(PermissionDecision::Allow);
        let gate = PolicyPermissionGate::new(policy, inner.clone());
        match gate.check("Bash", &serde_json::json!({})).await {
            PermissionDecision::Deny { reason } => {
                assert!(reason.contains("Bash"), "reason names the rule: {reason}");
            }
            PermissionDecision::Allow => panic!("expected Deny, got Allow"),
        }
        assert_eq!(inner.calls(), 0, "deny rule must not prompt");
    }

    #[tokio::test]
    async fn ask_fallback_auto_allows_read_only_tool() {
        // No rule for Read → Default mode asks; Read is AllowByDefault → auto-allow.
        let policy = policy_with(r#"{ "permissions": {} }"#, PermissionMode::Default);
        let inner = RecordingInner::new(PermissionDecision::Deny {
            reason: "should not prompt".into(),
        });
        let gate = PolicyPermissionGate::new(policy, inner.clone());
        assert_eq!(
            gate.check("Read", &serde_json::json!({})).await,
            PermissionDecision::Allow
        );
        assert_eq!(inner.calls(), 0, "read-only tool auto-allows, no prompt");
    }

    #[tokio::test]
    async fn ask_fallback_delegates_to_inner_for_non_read_only_tool() {
        // No rule for Bash → Default mode asks; Bash is DenyByDefault → delegate.
        let policy = policy_with(r#"{ "permissions": {} }"#, PermissionMode::Default);
        let inner = RecordingInner::new(PermissionDecision::Allow);
        let gate = PolicyPermissionGate::new(policy, inner.clone());
        assert_eq!(
            gate.check("Bash", &serde_json::json!({})).await,
            PermissionDecision::Allow // whatever the inner prompt returned
        );
        assert_eq!(inner.calls(), 1, "non-read-only ask delegates to the inner gate");
    }

    #[tokio::test]
    async fn dontask_mode_denies_unmatched_without_prompting() {
        let policy = policy_with(r#"{ "permissions": {} }"#, PermissionMode::DontAsk);
        let inner = RecordingInner::new(PermissionDecision::Allow);
        let gate = PolicyPermissionGate::new(policy, inner.clone());
        // Even a DenyByDefault tool is denied by mode, never prompted.
        assert!(matches!(
            gate.check("Bash", &serde_json::json!({})).await,
            PermissionDecision::Deny { .. }
        ));
        assert_eq!(inner.calls(), 0);
    }
}

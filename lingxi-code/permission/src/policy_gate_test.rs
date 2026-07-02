//! Extracted tests from policy_gate.rs.

use super::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mode::PermissionMode;
    use crate::rule::PermissionRuleSource;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn deny_reason_string_is_byte_faithful() {
        // Generic deny (rule/mode/other) ⇒ claude-code `permissions.ts:1087,1179`.
        let generic = PermissionDecisionReason::Other {
            reason: "anything".into(),
        };
        assert_eq!(
            deny_reason_string(&generic, "Bash"),
            "Permission to use Bash has been denied."
        );
        // dontAsk mode ⇒ `DONT_ASK_REJECT_MESSAGE` (`messages.ts:237`) with guidance.
        let dont_ask = PermissionDecisionReason::PermissionMode {
            mode: PermissionMode::DontAsk,
        };
        assert_eq!(
            deny_reason_string(&dont_ask, "Edit"),
            format!("Permission to use Edit has been denied because LingXi is running in don't ask mode. {DENIAL_WORKAROUND_GUIDANCE}")
        );
        // The guidance text itself is byte-locked.
        assert!(DENIAL_WORKAROUND_GUIDANCE.starts_with(
            "IMPORTANT: You *may* attempt to accomplish this action using other tools"
        ));
        assert!(DENIAL_WORKAROUND_GUIDANCE.ends_with("Let the user decide how to proceed."));
    }

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
        let policy = policy_with(
            r#"{ "permissions": { "allow": ["Bash"] } }"#,
            PermissionMode::Default,
        );
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
        let policy = policy_with(
            r#"{ "permissions": { "deny": ["Bash"] } }"#,
            PermissionMode::Default,
        );
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
    async fn explicit_ask_rule_prompts_even_for_read_only_tool() {
        // An explicit `ask:["Read"]` rule tags the ask `MatchedRule`, which must
        // PRE-EMPT the read-only auto-allow stand-in and surface the prompt —
        // claude-code reaches the read-only default-allow only after the ask walk.
        let policy = policy_with(
            r#"{ "permissions": { "ask": ["Read"] } }"#,
            PermissionMode::Default,
        );
        let inner = RecordingInner::new(PermissionDecision::Allow);
        let gate = PolicyPermissionGate::new(policy, inner.clone());
        assert_eq!(
            gate.check("Read", &serde_json::json!({ "file_path": "/x.rs" }))
                .await,
            PermissionDecision::Allow // whatever the prompt returned
        );
        assert_eq!(
            inner.calls(),
            1,
            "an explicit ask rule on a read-only tool delegates to the prompt"
        );
    }

    #[tokio::test]
    async fn explicit_content_ask_rule_surfaces_as_ask_for_read_only_tool() {
        // A CONTENT ask rule `ask:["Read(./secrets/**)"]` likewise tags the ask
        // `MatchedRule`, so resolve_detailed surfaces `Ask` (firing
        // PermissionRequest) rather than auto-allowing the read. (Path-content
        // discrimination is roots-gated and not exercised here — this policy is
        // built without roots, so the content rule matches the tool; the point is
        // that a MatchedRule ask is never short-circuited to auto-allow.)
        let policy = policy_with(
            r#"{ "permissions": { "ask": ["Read(./secrets/**)"] } }"#,
            PermissionMode::Default,
        );
        let inner = RecordingInner::new(PermissionDecision::Allow);
        let gate = PolicyPermissionGate::new(policy, inner.clone());
        assert_eq!(
            gate.resolve_detailed(
                "Read",
                &serde_json::json!({ "file_path": "./secrets/key.pem" })
            )
            .await,
            PermissionResolution::Ask,
            "a content ask rule surfaces as Ask, not an auto-allow"
        );
    }

    /// Inner gate that records the worker handed to `check_with_worker`.
    struct WorkerRecordingInner {
        worker: std::sync::Mutex<Option<Option<crate::gate::PromptWorker>>>,
    }
    #[async_trait]
    impl PermissionGate for WorkerRecordingInner {
        async fn check(&self, _name: &str, _input: &Value) -> PermissionDecision {
            *self.worker.lock().unwrap() = Some(None);
            PermissionDecision::Allow
        }
        async fn check_with_worker(
            &self,
            _name: &str,
            _input: &Value,
            worker: Option<crate::gate::PromptWorker>,
        ) -> PermissionDecision {
            *self.worker.lock().unwrap() = Some(worker);
            PermissionDecision::Allow
        }
    }

    #[tokio::test]
    async fn check_with_worker_forwards_attribution_to_inner_on_ask() {
        // No rule for Bash → Default mode asks; Bash is DenyByDefault → delegate.
        // The worker identity must reach the inner prompt transport so the dialog
        // is attributed (claude-code worker permission badge).
        let policy = policy_with(r#"{ "permissions": {} }"#, PermissionMode::Default);
        let inner = Arc::new(WorkerRecordingInner {
            worker: std::sync::Mutex::new(None),
        });
        let gate = PolicyPermissionGate::new(policy, inner.clone());
        let decision = gate
            .check_with_worker(
                "Bash",
                &serde_json::json!({}),
                Some(crate::gate::PromptWorker {
                    name: "researcher".into(),
                    team: Some("alpha".into()),
                    is_async: true,
                }),
            )
            .await;
        assert_eq!(decision, PermissionDecision::Allow);
        let seen = inner
            .worker
            .lock()
            .unwrap()
            .clone()
            .expect("inner consulted");
        let w = seen.expect("worker forwarded to inner transport");
        assert_eq!(w.name, "researcher");
        assert_eq!(w.team.as_deref(), Some("alpha"));
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
        assert_eq!(
            inner.calls(),
            1,
            "non-read-only ask delegates to the inner gate"
        );
    }

    #[tokio::test]
    async fn plan_mode_read_only_tool_auto_allows() {
        // Plan + Read: the backstop is NOT taken (Read is plan-safe), it falls to
        // the mode-fallback ask, and Read is AllowByDefault → auto-allow, no prompt.
        let policy = policy_with(r#"{ "permissions": {} }"#, PermissionMode::Plan);
        let inner = RecordingInner::new(PermissionDecision::Deny {
            reason: "should not prompt for read-only in plan mode".into(),
        });
        let gate = PolicyPermissionGate::new(policy, inner.clone());
        assert_eq!(
            gate.check("Read", &serde_json::json!({})).await,
            PermissionDecision::Allow
        );
        assert_eq!(inner.calls(), 0, "plan-safe read-only tool auto-allows");
    }

    #[tokio::test]
    async fn plan_mode_mutating_tool_delegates_to_inner() {
        // Plan + Edit: the backstop fires (Edit is not plan-safe) → Ask(Plan);
        // Edit is DenyByDefault → the ask is delegated to the inner transport,
        // NOT short-circuited to auto-allow.
        let policy = policy_with(r#"{ "permissions": {} }"#, PermissionMode::Plan);
        let inner = RecordingInner::new(PermissionDecision::Allow);
        let gate = PolicyPermissionGate::new(policy, inner.clone());
        assert_eq!(
            gate.check("Edit", &serde_json::json!({ "file_path": "/x.rs" }))
                .await,
            PermissionDecision::Allow // whatever the prompt returned
        );
        assert_eq!(
            inner.calls(),
            1,
            "plan-mode mutating tool delegates to the inner gate"
        );
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

    #[tokio::test]
    async fn headless_deny_gate_denies_unresolved_ask_but_spares_rules_and_read_only() {
        // The HEADLESS composition: PolicyPermissionGate wrapping DenyOnAskGate
        // (claude-code `--print` parity). The deny inner must ONLY turn an
        // otherwise-unresolved ask into a denial — never override an allow rule
        // or a read-only auto-allow.
        use crate::headless_gate::DenyOnAskGate;

        // No rules → Default mode asks. Bash (DenyByDefault) delegates to the
        // inner DenyOnAskGate → Deny.
        let policy = policy_with(r#"{ "permissions": {} }"#, PermissionMode::Default);
        let gate = PolicyPermissionGate::new(policy, Arc::new(DenyOnAskGate));
        assert!(
            matches!(
                gate.check("Bash", &serde_json::json!({})).await,
                PermissionDecision::Deny { .. }
            ),
            "headless: a mutating tool with no rule must be denied, not allowed"
        );
        // Read (AllowByDefault) is auto-allowed BEFORE delegation → never denied.
        assert_eq!(
            gate.check("Read", &serde_json::json!({})).await,
            PermissionDecision::Allow,
            "headless: read-only tools stay frictionless"
        );

        // An explicit allow rule still short-circuits to Allow even with the
        // deny inner (rules are resolved before delegation).
        let policy2 = policy_with(
            r#"{ "permissions": { "allow": ["Bash"] } }"#,
            PermissionMode::Default,
        );
        let gate2 = PolicyPermissionGate::new(policy2, Arc::new(DenyOnAskGate));
        assert_eq!(
            gate2.check("Bash", &serde_json::json!({})).await,
            PermissionDecision::Allow,
            "headless: an explicit allow rule still wins"
        );
    }

    #[tokio::test]
    async fn check_after_hook_allow_enforces_deny_rules_but_skips_prompt() {
        // (Hooks unit 3, issue 1) A PreToolUse/PermissionRequest hook 'allow'
        // skips the PROMPT but must NOT override rule-based deny/ask
        // (claude-code `resolveHookPermissionDecision` +
        // `checkRuleBasedPermissions`).

        // A deny RULE must still deny even after a hook approved the call.
        let policy = policy_with(
            r#"{ "permissions": { "deny": ["Bash"] } }"#,
            PermissionMode::Default,
        );
        let gate =
            PolicyPermissionGate::new(policy, RecordingInner::new(PermissionDecision::Allow));
        assert!(
            matches!(
                gate.check_after_hook_allow("Bash", &serde_json::json!({}))
                    .await,
                PermissionDecision::Deny { .. }
            ),
            "a deny rule must override a hook 'allow'"
        );

        // A mutating tool with NO rule would normally Ask→prompt; under a hook
        // 'allow' the prompt is SKIPPED → Allow, and the inner transport is
        // NEVER consulted.
        let inner = RecordingInner::new(PermissionDecision::Deny {
            reason: "must not prompt under hook-allow".into(),
        });
        let policy2 = policy_with(r#"{ "permissions": {} }"#, PermissionMode::Default);
        let gate2 = PolicyPermissionGate::new(policy2, inner.clone());
        assert_eq!(
            gate2
                .check_after_hook_allow("Bash", &serde_json::json!({}))
                .await,
            PermissionDecision::Allow,
            "hook 'allow' skips the prompt for an un-ruled mutating tool"
        );
        assert_eq!(
            inner.calls(),
            0,
            "hook 'allow' must NOT delegate to the prompt"
        );

        // An explicit allow rule → Allow.
        let policy3 = policy_with(
            r#"{ "permissions": { "allow": ["Bash"] } }"#,
            PermissionMode::Default,
        );
        let gate3 =
            PolicyPermissionGate::new(policy3, RecordingInner::new(PermissionDecision::Allow));
        assert_eq!(
            gate3
                .check_after_hook_allow("Bash", &serde_json::json!({}))
                .await,
            PermissionDecision::Allow
        );
    }

    /// Default-impl gates (no rule layer) keep the prior wholesale-bypass: a hook
    /// 'allow' → Allow.
    #[tokio::test]
    async fn default_check_after_hook_allow_is_wholesale_allow() {
        let gate = RecordingInner::new(PermissionDecision::Deny {
            reason: "default must not be consulted".into(),
        });
        assert_eq!(
            gate.check_after_hook_allow("Bash", &serde_json::json!({}))
                .await,
            PermissionDecision::Allow,
            "a rule-less gate treats a hook 'allow' as a wholesale allow"
        );
        assert_eq!(gate.calls(), 0, "default impl must not call check()");
    }

    // ── Plan-mode dynamic gate: check_in_plan_mode ───────────────────────────

    #[tokio::test]
    async fn plan_mode_gate_auto_allows_plan_safe_read_without_prompting() {
        // Live plan mode (boot mode Default): Read is plan-safe AND AllowByDefault
        // → auto-allowed, the inner prompt is never consulted.
        let policy = policy_with(r#"{ "permissions": {} }"#, PermissionMode::Default);
        let inner = RecordingInner::new(PermissionDecision::Deny {
            reason: "should not prompt for read in plan mode".into(),
        });
        let gate = PolicyPermissionGate::new(policy, inner.clone());
        assert_eq!(
            gate.check_in_plan_mode("Read", &serde_json::json!({}))
                .await,
            PermissionDecision::Allow
        );
        assert_eq!(inner.calls(), 0, "plan-safe read auto-allows, no prompt");
    }

    #[tokio::test]
    async fn plan_mode_gate_delegates_mutating_tool_to_inner() {
        // Live plan mode: Edit is NOT plan-safe → the backstop fires → Ask →
        // Edit is DenyByDefault → delegate to the inner prompt transport.
        let policy = policy_with(r#"{ "permissions": {} }"#, PermissionMode::Default);
        let inner = RecordingInner::new(PermissionDecision::Allow);
        let gate = PolicyPermissionGate::new(policy, inner.clone());
        assert_eq!(
            gate.check_in_plan_mode("Edit", &serde_json::json!({ "file_path": "/x.rs" }))
                .await,
            PermissionDecision::Allow // whatever the prompt returned
        );
        assert_eq!(
            inner.calls(),
            1,
            "plan-mode mutating tool delegates to the inner prompt"
        );
    }

    #[tokio::test]
    async fn plan_mode_gate_keeps_deny_rule_without_prompting() {
        // A deny rule binds even under live plan mode (rules resolve before mode).
        let policy = policy_with(
            r#"{ "permissions": { "deny": ["Edit"] } }"#,
            PermissionMode::Default,
        );
        let inner = RecordingInner::new(PermissionDecision::Allow);
        let gate = PolicyPermissionGate::new(policy, inner.clone());
        assert!(matches!(
            gate.check_in_plan_mode("Edit", &serde_json::json!({}))
                .await,
            PermissionDecision::Deny { .. }
        ));
        assert_eq!(
            inner.calls(),
            0,
            "deny rule binds under plan mode, no prompt"
        );
    }

    #[tokio::test]
    async fn plan_mode_gate_keeps_allow_rule_without_prompting() {
        // An explicit allow rule wins over the plan backstop, no prompt.
        let policy = policy_with(
            r#"{ "permissions": { "allow": ["Edit"] } }"#,
            PermissionMode::Default,
        );
        let inner = RecordingInner::new(PermissionDecision::Deny {
            reason: "should not prompt under an allow rule".into(),
        });
        let gate = PolicyPermissionGate::new(policy, inner.clone());
        assert_eq!(
            gate.check_in_plan_mode("Edit", &serde_json::json!({}))
                .await,
            PermissionDecision::Allow
        );
        assert_eq!(
            inner.calls(),
            0,
            "allow rule wins under plan mode, no prompt"
        );
    }

    #[tokio::test]
    async fn default_check_in_plan_mode_delegates_to_check() {
        // A rule-less gate (no mode layer) has nothing extra to enforce under plan
        // mode, so the default impl just delegates to check().
        let gate = RecordingInner::new(PermissionDecision::Allow);
        assert_eq!(
            gate.check_in_plan_mode("Edit", &serde_json::json!({}))
                .await,
            PermissionDecision::Allow
        );
        assert_eq!(
            gate.calls(),
            1,
            "default check_in_plan_mode delegates to check()"
        );
    }

    // ── Deny-source substrate: resolve_detailed ──────────────────────────────

    #[tokio::test]
    async fn resolve_detailed_allow_rule_is_allow_without_inner() {
        let policy = policy_with(
            r#"{ "permissions": { "allow": ["Bash"] } }"#,
            PermissionMode::Default,
        );
        let inner = RecordingInner::new(PermissionDecision::Deny {
            reason: "should not prompt".into(),
        });
        let gate = PolicyPermissionGate::new(policy, inner.clone());
        assert_eq!(
            gate.resolve_detailed("Bash", &serde_json::json!({})).await,
            PermissionResolution::Allow
        );
        assert_eq!(
            inner.calls(),
            0,
            "resolve_detailed never consults the inner"
        );
    }

    #[tokio::test]
    async fn resolve_detailed_deny_rule_carries_rule_source() {
        let policy = policy_with(
            r#"{ "permissions": { "deny": ["Bash"] } }"#,
            PermissionMode::Default,
        );
        let gate =
            PolicyPermissionGate::new(policy, RecordingInner::new(PermissionDecision::Allow));
        match gate.resolve_detailed("Bash", &serde_json::json!({})).await {
            PermissionResolution::Deny { source, reason, .. } => {
                assert_eq!(source, PermissionDecisionSource::Rule);
                assert!(reason.contains("Bash"), "reason names the rule: {reason}");
            }
            other => panic!("expected Deny{{Rule}}, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn resolve_detailed_mode_deny_carries_mode_source() {
        // DontAsk mode denies an unmatched mutating tool by MODE (not a rule).
        let policy = policy_with(r#"{ "permissions": {} }"#, PermissionMode::DontAsk);
        let gate =
            PolicyPermissionGate::new(policy, RecordingInner::new(PermissionDecision::Allow));
        match gate.resolve_detailed("Bash", &serde_json::json!({})).await {
            PermissionResolution::Deny { source, .. } => {
                assert_eq!(source, PermissionDecisionSource::Mode);
            }
            other => panic!("expected Deny{{Mode}}, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn resolve_detailed_read_only_ask_is_allow_without_inner() {
        let policy = policy_with(r#"{ "permissions": {} }"#, PermissionMode::Default);
        let inner = RecordingInner::new(PermissionDecision::Deny {
            reason: "should not prompt".into(),
        });
        let gate = PolicyPermissionGate::new(policy, inner.clone());
        assert_eq!(
            gate.resolve_detailed("Read", &serde_json::json!({})).await,
            PermissionResolution::Allow
        );
        assert_eq!(inner.calls(), 0, "read-only auto-allow never prompts");
    }

    #[tokio::test]
    async fn resolve_detailed_mutating_ask_is_ask_without_inner() {
        let policy = policy_with(r#"{ "permissions": {} }"#, PermissionMode::Default);
        let inner = RecordingInner::new(PermissionDecision::Allow);
        let gate = PolicyPermissionGate::new(policy, inner.clone());
        assert_eq!(
            gate.resolve_detailed("Bash", &serde_json::json!({})).await,
            PermissionResolution::Ask,
            "a would-be prompt surfaces as Ask, not a delegated decision"
        );
        assert_eq!(
            inner.calls(),
            0,
            "resolve_detailed returns Ask, it does not delegate"
        );
    }

    #[tokio::test]
    async fn default_resolve_detailed_maps_check() {
        // A rule-less gate: Allow → Allow, Deny → Deny{Unspecified}, never Ask.
        let allow = RecordingInner::new(PermissionDecision::Allow);
        assert_eq!(
            allow.resolve_detailed("Bash", &serde_json::json!({})).await,
            PermissionResolution::Allow
        );
        let deny = RecordingInner::new(PermissionDecision::Deny {
            reason: "nope".into(),
        });
        match deny.resolve_detailed("Bash", &serde_json::json!({})).await {
            PermissionResolution::Deny { source, reason, .. } => {
                assert_eq!(source, PermissionDecisionSource::Unspecified);
                assert_eq!(reason, "nope");
            }
            other => panic!("expected Deny{{Unspecified}}, got {other:?}"),
        }
    }

    #[test]
    fn serialize_decision_reason_matches_oracle_switch() {
        use crate::result::ClassifierKind;
        use crate::rule::{PermissionRule, PermissionRuleSource, PermissionRuleValue};
        // rule/mode/subcommandResults/permissionPromptTool ⇒ None (oracle returns
        // undefined; the SDK host parses decision_reason_type for those).
        let rule = PermissionRule {
            value: PermissionRuleValue::from_rule_string("Bash"),
            behavior: crate::rule::PermissionBehavior::Ask,
            source: PermissionRuleSource::UserSettings,
        };
        assert_eq!(
            serialize_decision_reason(&PermissionDecisionReason::MatchedRule { rule }),
            None
        );
        assert_eq!(
            serialize_decision_reason(&PermissionDecisionReason::PermissionMode {
                mode: PermissionMode::Default
            }),
            None
        );
        assert_eq!(
            serialize_decision_reason(&PermissionDecisionReason::PermissionPromptTool {
                tool_name: "mcp__x".into()
            }),
            None
        );
        // hook/asyncAgent/workingDir/safetyCheck/other ⇒ the reason string.
        assert_eq!(
            serialize_decision_reason(&PermissionDecisionReason::HookOverride {
                hook_id: "h1".into(),
                source: None,
                reason: Some("blocked by hook".into()),
            }),
            Some("blocked by hook".to_string())
        );
        // A hook with no reason has nothing to serialize.
        assert_eq!(
            serialize_decision_reason(&PermissionDecisionReason::HookOverride {
                hook_id: "h1".into(),
                source: None,
                reason: None,
            }),
            None
        );
        assert_eq!(
            serialize_decision_reason(&PermissionDecisionReason::AsyncAgent {
                reason: "async said no".into()
            }),
            Some("async said no".to_string())
        );
        assert_eq!(
            serialize_decision_reason(&PermissionDecisionReason::WorkingDirectory {
                reason: "escapes root".into()
            }),
            Some("escapes root".to_string())
        );
        assert_eq!(
            serialize_decision_reason(&PermissionDecisionReason::SafetyCheck {
                reason: "dangerous rm".into(),
                classifier_approvable: false,
            }),
            Some("dangerous rm".to_string())
        );
        assert_eq!(
            serialize_decision_reason(&PermissionDecisionReason::Other {
                reason: "misc".into()
            }),
            Some("misc".to_string())
        );
        // classifier is feature-gated OFF in the external build → None.
        assert_eq!(
            serialize_decision_reason(&PermissionDecisionReason::ClassifierRejected {
                classifier: ClassifierKind::Transcript,
                score: 0.9,
            }),
            None
        );
        // sandboxOverride: enum reason, no faithful string → None.
        assert_eq!(
            serialize_decision_reason(&PermissionDecisionReason::SandboxOverride {
                reason: crate::result::SandboxOverrideReason::ExcludedCommand,
            }),
            None
        );
    }

    /// Inner gate that records the [`PermissionCheckContext`] handed to
    /// `check_with_context` (so a test can assert the gate enriched it).
    struct ContextRecordingInner {
        ctx: std::sync::Mutex<Option<PermissionCheckContext>>,
    }
    #[async_trait]
    impl PermissionGate for ContextRecordingInner {
        async fn check(&self, _name: &str, _input: &Value) -> PermissionDecision {
            PermissionDecision::Allow
        }
        async fn check_with_context(
            &self,
            _name: &str,
            _input: &Value,
            ctx: &PermissionCheckContext,
        ) -> PermissionOutcome {
            *self.ctx.lock().unwrap() = Some(ctx.clone());
            PermissionOutcome::Allow {
                updated_input: None,
                permission_updates: Vec::new(),
            }
        }
    }

    #[tokio::test]
    async fn check_with_context_enriches_decision_reason_on_delegated_ask() {
        // A mutating tool with no rule → Default mode Ask (reason `PermissionMode`)
        // → delegated to the inner transport. The gate must enrich the ctx with the
        // serialized decision_reason and preserve the turn loop's tool_use_id.
        // PermissionMode serializes to None (oracle omits rule/mode), so the
        // forwarded decision_reason is None for this common case — byte-faithful.
        let policy = policy_with(r#"{ "permissions": {} }"#, PermissionMode::Default);
        let inner = Arc::new(ContextRecordingInner {
            ctx: std::sync::Mutex::new(None),
        });
        let gate = PolicyPermissionGate::new(policy, inner.clone());
        let ctx = PermissionCheckContext {
            tool_use_id: Some("toolu_42".into()),
            ..Default::default()
        };
        let outcome = gate
            .check_with_context("Bash", &serde_json::json!({}), &ctx)
            .await;
        assert_eq!(
            outcome,
            PermissionOutcome::Allow {
                updated_input: None,
                permission_updates: Vec::new()
            }
        );
        let seen = inner.ctx.lock().unwrap().clone().expect("inner consulted");
        assert_eq!(
            seen.tool_use_id.as_deref(),
            Some("toolu_42"),
            "the dispatcher tool_use_id is preserved through enrichment"
        );
        assert_eq!(
            seen.decision_reason, None,
            "a PermissionMode ask omits decision_reason (oracle returns undefined)"
        );
    }

    #[test]
    fn map_decision_source_maps_classifier_rejected_to_classifier() {
        // The classifier source is what unblocks the PermissionDenied hook; the
        // auto-mode classifier path is unwired in the public build, so this is the
        // only direct coverage of the mapping (it would otherwise be dormant).
        use crate::result::ClassifierKind;
        assert_eq!(
            map_decision_source(&PermissionDecisionReason::ClassifierRejected {
                classifier: ClassifierKind::Transcript,
                score: 0.9,
            }),
            PermissionDecisionSource::Classifier
        );
    }

    #[tokio::test]
    async fn auto_mode_classifier_allows_safe_local_shell_without_prompt() {
        let policy = Arc::new(PermissionPolicy::from_rules(
            PermissionMode::Auto,
            std::iter::empty(),
        ));
        let inner = RecordingInner::new(PermissionDecision::Deny {
            reason: "prompt should not run".into(),
        });
        let gate = PolicyPermissionGate::new(policy, inner.clone());

        assert_eq!(
            gate.check(
                "Bash",
                &serde_json::json!({ "command": "cargo test -p permission" })
            )
            .await,
            PermissionDecision::Allow
        );
        assert_eq!(inner.calls(), 0, "classifier allow must skip prompt");
    }

    #[tokio::test]
    async fn auto_mode_classifier_deny_has_classifier_source() {
        let policy = Arc::new(PermissionPolicy::from_rules(
            PermissionMode::Auto,
            std::iter::empty(),
        ));
        let gate =
            PolicyPermissionGate::new(policy, RecordingInner::new(PermissionDecision::Allow));

        match gate
            .resolve_detailed(
                "Bash",
                &serde_json::json!({ "command": "git reset --hard" }),
            )
            .await
        {
            PermissionResolution::Deny { source, reason, .. } => {
                assert_eq!(source, PermissionDecisionSource::Classifier);
                assert!(
                    reason.contains("Auto mode classifier blocked action"),
                    "reason carries classifier block text: {reason}"
                );
            }
            other => panic!("expected classifier deny, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn auto_mode_explicit_ask_rule_still_prompts() {
        let policy = policy_with(
            r#"{ "permissions": { "ask": ["Bash"] } }"#,
            PermissionMode::Auto,
        );
        let inner = RecordingInner::new(PermissionDecision::Allow);
        let gate = PolicyPermissionGate::new(policy, inner.clone());

        assert_eq!(
            gate.check("Bash", &serde_json::json!({ "command": "cargo test" }))
                .await,
            PermissionDecision::Allow
        );
        assert_eq!(
            inner.calls(),
            1,
            "explicit ask rule must delegate to prompt instead of classifier"
        );
    }

    #[tokio::test]
    async fn auto_mode_skips_dangerous_allow_rules_before_classifier() {
        let rules = crate::loader::permission_rules_from_settings_json(
            r#"{ "permissions": { "allow": ["Bash(python:*)"] } }"#,
            PermissionRuleSource::UserSettings,
        )
        .unwrap();
        let policy = Arc::new(PermissionPolicy::from_rules(PermissionMode::Auto, rules));
        let gate = PolicyPermissionGate::new(
            policy,
            RecordingInner::new(PermissionDecision::Deny {
                reason: "prompt-denied".into(),
            }),
        );

        assert!(matches!(
            gate.check("Bash", &serde_json::json!({ "command": "python evil.py" }))
                .await,
            PermissionDecision::Deny { .. }
        ));
    }

    // ── set_permission_mode (live mode override) ─────────────────────────────

    #[tokio::test]
    async fn set_permission_mode_override_changes_authorize_outcome() {
        // No rules, boot mode Default: a mutating tool with no allow rule is an
        // Ask → delegates to the inner prompt transport (here: Deny).
        // bypass_permissions_available=true: the session was launched with
        // --dangerously-skip-permissions, so a live switch into bypass is allowed.
        let policy = Arc::new(
            PermissionPolicy::from_rules(PermissionMode::Default, Vec::new())
                .with_bypass_available(true),
        );
        let inner = RecordingInner::new(PermissionDecision::Deny {
            reason: "prompt-denied".into(),
        });
        let gate = PolicyPermissionGate::new(policy, inner.clone());
        assert!(matches!(
            gate.check("Write", &serde_json::json!({})).await,
            PermissionDecision::Deny { .. }
        ));

        // Switch to bypassPermissions LIVE: everything now allows, the inner
        // prompt is never consulted again.
        gate.set_permission_mode("bypassPermissions").await.unwrap();
        let calls_before = inner.calls();
        assert_eq!(
            gate.check("Write", &serde_json::json!({})).await,
            PermissionDecision::Allow
        );
        assert_eq!(
            inner.calls(),
            calls_before,
            "bypass mode must not delegate to the prompt transport"
        );
    }

    #[tokio::test]
    async fn set_permission_mode_accepts_unknown_mode_as_noop() {
        // The binary accepts any mode string and no-ops an unknown one (no error
        // frame). A mutating tool with no rule is an Ask → delegates to the inner
        // transport both before and after the unknown set (mode unchanged).
        let policy = policy_with(r#"{ "permissions": {} }"#, PermissionMode::Default);
        let inner = RecordingInner::new(PermissionDecision::Deny {
            reason: "prompt-denied".into(),
        });
        let gate = PolicyPermissionGate::new(policy, inner.clone());
        gate.set_permission_mode("nonsense")
            .await
            .expect("unknown mode acked, not errored");
        // Mode unchanged → still delegates to the inner prompt transport.
        assert!(matches!(
            gate.check("Write", &serde_json::json!({})).await,
            PermissionDecision::Deny { .. }
        ));
    }

    #[tokio::test]
    async fn set_permission_mode_rejects_bypass_when_killswitch_active() {
        let mut policy = PermissionPolicy::from_rules(PermissionMode::Default, Vec::new());
        policy.bypass_killswitch_active = true;
        let gate = PolicyPermissionGate::new(
            Arc::new(policy),
            RecordingInner::new(PermissionDecision::Allow),
        );
        assert_eq!(
            gate.set_permission_mode("bypassPermissions").await.unwrap_err(),
            "Cannot set permission mode to bypassPermissions because it is disabled by settings or configuration"
        );
    }

    #[tokio::test]
    async fn set_permission_mode_rejects_bypass_when_not_launched_with_flag() {
        // Killswitch inactive but the session was NOT launched with
        // --dangerously-skip-permissions (bypass_permissions_available=false,
        // the default) → the second ordered check rejects, byte-exact.
        let policy = PermissionPolicy::from_rules(PermissionMode::Default, Vec::new());
        assert!(!policy.bypass_permissions_available);
        let gate = PolicyPermissionGate::new(
            Arc::new(policy),
            RecordingInner::new(PermissionDecision::Allow),
        );
        assert_eq!(
            gate.set_permission_mode("bypassPermissions").await.unwrap_err(),
            "Cannot set permission mode to bypassPermissions because the session was not launched with --dangerously-skip-permissions"
        );
    }

    #[test]
    fn parse_settable_mode_accepts_six_modes_noops_unknown() {
        assert_eq!(
            parse_settable_mode("default"),
            Some(PermissionMode::Default)
        );
        assert_eq!(parse_settable_mode("plan"), Some(PermissionMode::Plan));
        assert_eq!(
            parse_settable_mode("acceptEdits"),
            Some(PermissionMode::AcceptEdits)
        );
        assert_eq!(
            parse_settable_mode("bypassPermissions"),
            Some(PermissionMode::BypassPermissions)
        );
        assert_eq!(
            parse_settable_mode("dontAsk"),
            Some(PermissionMode::DontAsk)
        );
        assert_eq!(parse_settable_mode("auto"), Some(PermissionMode::Auto));
        // Unknown → None (the caller acks + no-ops, matching the binary).
        assert_eq!(parse_settable_mode("bubble"), None);
    }
}

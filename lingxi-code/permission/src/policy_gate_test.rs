//! Extracted tests from policy_gate.rs.

use super::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filesystem::FsRoots;
    use crate::mode::PermissionMode;
    use crate::rule::PermissionRuleSource;
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

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

    struct PersistenceRecordingInner {
        enabled: AtomicBool,
        persisted: AtomicUsize,
    }

    #[async_trait]
    impl PermissionGate for PersistenceRecordingInner {
        async fn check(&self, _name: &str, _input: &Value) -> PermissionDecision {
            PermissionDecision::Allow
        }

        fn set_permission_persistence_enabled(&self, enabled: bool) {
            self.enabled.store(enabled, Ordering::SeqCst);
        }

        async fn persist_permission_updates(&self, updates: &[Value]) {
            self.persisted.fetch_add(updates.len(), Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn managed_rules_only_disables_inner_permission_persistence() {
        let policy = Arc::new(
            PermissionPolicy::from_rules(PermissionMode::Default, Vec::new())
                .with_managed_permission_rules_only(true),
        );
        let inner = Arc::new(PersistenceRecordingInner {
            enabled: AtomicBool::new(true),
            persisted: AtomicUsize::new(0),
        });
        let gate = PolicyPermissionGate::new(policy, inner.clone());
        assert!(!inner.enabled.load(Ordering::SeqCst));
        gate.persist_permission_updates(&[json!({"type": "addRules"})])
            .await;
        assert_eq!(inner.persisted.load(Ordering::SeqCst), 0);
    }

    fn policy_with(raw: &str, mode: PermissionMode) -> Arc<PermissionPolicy> {
        let rules = crate::loader::permission_rules_from_settings_json(
            raw,
            PermissionRuleSource::UserSettings,
        )
        .unwrap();
        Arc::new(PermissionPolicy::from_rules(mode, rules))
    }

    fn policy_with_roots(raw: &str, mode: PermissionMode) -> Arc<PermissionPolicy> {
        let rules = crate::loader::permission_rules_from_settings_json(
            raw,
            PermissionRuleSource::UserSettings,
        )
        .unwrap();
        Arc::new(
            PermissionPolicy::from_rules(mode, rules).with_roots(FsRoots {
                cwd: PathBuf::from("/proj"),
                home: Some(PathBuf::from("/home/u")),
                lingxi_home: PathBuf::from("/home/u/.lingxi"),
            }),
        )
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
    /// HOOKALLOW-01 / cc 2.1.218 `lin` — a PreToolUse hook `allow` is re-checked
    /// against the rules UNCONDITIONALLY, and an ASK RULE sends the call to the
    /// full permission pipeline (a PROMPT), NOT a silent allow. This is the
    /// laundering hole: previously any hook allow mapped Ask→Allow.
    #[tokio::test]
    async fn hook_allow_is_overridden_by_an_ask_rule_and_prompts() {
        let inner = RecordingInner::new(PermissionDecision::Deny {
            reason: "prompt said no".into(),
        });
        let policy = policy_with(
            r#"{ "permissions": { "ask": ["Bash"] } }"#,
            PermissionMode::Default,
        );
        let gate = PolicyPermissionGate::new(policy, inner.clone());

        // NO `updatedInput` needed — `lin` re-checks unconditionally.
        let decision = gate
            .check_after_hook_allow("Bash", &serde_json::json!({}))
            .await;
        assert!(
            matches!(decision, PermissionDecision::Deny { .. }),
            "an ask RULE must reach the prompt (here the inner denies), not be auto-allowed"
        );
        assert_eq!(
            inner.calls(),
            1,
            "the ask rule must DELEGATE to the full pipeline"
        );
    }

    /// The mode BACKSTOP is not a rule verdict: `_pt` has no mode layer, so an
    /// ordinary Default-mode mutating call yields NO verdict and the hook's allow
    /// stands. Guards against the over-block that a naive `authorize` re-check
    /// would cause (it would deny nearly every hook-rescued call).
    #[tokio::test]
    async fn hook_allow_is_not_blocked_by_the_mode_backstop() {
        let inner = RecordingInner::new(PermissionDecision::Deny {
            reason: "must not prompt".into(),
        });
        let policy = policy_with(r#"{ "permissions": {} }"#, PermissionMode::Default);
        let gate = PolicyPermissionGate::new(policy, inner.clone());

        assert_eq!(
            gate.check_after_hook_allow("Bash", &serde_json::json!({}))
                .await,
            PermissionDecision::Allow,
            "a mode-sourced ask is NOT a rule verdict — the hook allow stands"
        );
        assert_eq!(inner.calls(), 0, "and it must not prompt");

        // Same for the rewritten (PermissionRequest) variant.
        assert_eq!(
            gate.check_after_hook_allow_rewritten("Bash", &serde_json::json!({}))
                .await,
            PermissionDecision::Allow,
            "a rewritten hook allow is likewise not blocked by the mode backstop"
        );
        assert_eq!(inner.calls(), 0);
    }

    /// cc 2.1.218 `Fxy`/`epr` — on the headless PermissionRequest surface an ask
    /// rule becomes a HARD DENY (no prompt available), carrying the ASK's message.
    #[tokio::test]
    async fn rewritten_hook_allow_turns_an_ask_rule_into_a_hard_deny() {
        let inner = RecordingInner::new(PermissionDecision::Allow);
        let policy = policy_with(
            r#"{ "permissions": { "ask": ["Bash"] } }"#,
            PermissionMode::Default,
        );
        let gate = PolicyPermissionGate::new(policy, inner.clone());

        let decision = gate
            .check_after_hook_allow_rewritten("Bash", &serde_json::json!({}))
            .await;
        match decision {
            PermissionDecision::Deny { reason } => {
                assert!(!reason.is_empty(), "the deny carries the ask's own message")
            }
            other => panic!("expected a hard Deny, got {other:?}"),
        }
        assert_eq!(
            inner.calls(),
            0,
            "the headless rescue must NOT re-prompt — the hook consumed the prompt"
        );
    }

    /// cc 2.1.218 `Fxy` STANDING allow (HOOKALLOW-01, gap218 #11) — no
    /// `updatedInput` and the tool does not `requiresUserInteraction`, so the
    /// oracle's re-check gate `if(a.updatedInput||e.requiresUserInteraction?.())`
    /// is FALSE and the allow returns unchecked. An ask rule — the reason the
    /// gate resolved `Ask` and fired the PermissionRequest hook — must NOT be
    /// re-evaluated here, or the rescue is defeated in its primary use case.
    /// Contrast [`rewritten_hook_allow_turns_an_ask_rule_into_a_hard_deny`] above,
    /// which turns the SAME ask rule into a hard deny.
    #[tokio::test]
    async fn honour_hook_allow_lets_an_ask_rule_stand_without_a_re_check() {
        let inner = RecordingInner::new(PermissionDecision::Deny {
            reason: "must not prompt".into(),
        });
        let policy = policy_with(
            r#"{ "permissions": { "ask": ["Bash"] } }"#,
            PermissionMode::Default,
        );
        let gate = PolicyPermissionGate::new(policy, inner.clone());

        assert_eq!(
            gate.honour_hook_allow("Bash", &serde_json::json!({})).await,
            PermissionDecision::Allow,
            "the standing PermissionRequest-hook allow stands over an ask rule"
        );
        assert_eq!(
            inner.calls(),
            0,
            "the standing allow must NOT delegate to the prompt transport"
        );

        // The REWRITTEN twin, on the SAME ask rule, is a hard deny — the two arms
        // deliberately diverge (Fxy's `updatedInput || requiresUserInteraction`
        // re-check gate).
        assert!(matches!(
            gate.check_after_hook_allow_rewritten("Bash", &serde_json::json!({}))
                .await,
            PermissionDecision::Deny { .. }
        ));
    }

    /// Neither resolver may over-block a call the rules genuinely permit.
    #[tokio::test]
    async fn hook_allow_paths_keep_an_explicit_allow_rule() {
        let policy = policy_with(
            r#"{ "permissions": { "allow": ["Bash"] } }"#,
            PermissionMode::Default,
        );
        let gate = PolicyPermissionGate::new(
            policy,
            RecordingInner::new(PermissionDecision::Deny {
                reason: "must not prompt".into(),
            }),
        );
        assert_eq!(
            gate.check_after_hook_allow("Bash", &serde_json::json!({}))
                .await,
            PermissionDecision::Allow
        );
        assert_eq!(
            gate.check_after_hook_allow_rewritten("Bash", &serde_json::json!({}))
                .await,
            PermissionDecision::Allow
        );
    }

    /// A DENY rule overrides a hook allow on BOTH surfaces.
    #[tokio::test]
    async fn deny_rule_overrides_hook_allow_on_both_surfaces() {
        let policy = policy_with(
            r#"{ "permissions": { "deny": ["Bash"] } }"#,
            PermissionMode::Default,
        );
        let gate =
            PolicyPermissionGate::new(policy, RecordingInner::new(PermissionDecision::Allow));
        assert!(matches!(
            gate.check_after_hook_allow("Bash", &serde_json::json!({}))
                .await,
            PermissionDecision::Deny { .. }
        ));
        assert!(matches!(
            gate.check_after_hook_allow_rewritten("Bash", &serde_json::json!({}))
                .await,
            PermissionDecision::Deny { .. }
        ));
    }

    /// REGRESSION (adversarial round 2, over-block #1): under `DontAsk` the port
    /// rewrites a surviving ask into a MODE-tagged deny. That is a mode-tail
    /// artefact, not a rule verdict — `_pt` never sees `dontAsk` (it lives in the
    /// outer `$xy` wrapper), so a hook allow must still stand.
    #[tokio::test]
    async fn dont_ask_mode_does_not_hard_deny_a_hook_allow() {
        let inner = RecordingInner::new(PermissionDecision::Deny {
            reason: "must not prompt".into(),
        });
        let policy = policy_with(r#"{ "permissions": {} }"#, PermissionMode::DontAsk);
        let gate = PolicyPermissionGate::new(policy, inner.clone());

        assert_eq!(
            gate.check_after_hook_allow("Bash", &serde_json::json!({"command": "npm install"}))
                .await,
            PermissionDecision::Allow,
            "DontAsk's ask→deny transform must not masquerade as a rule verdict"
        );
        assert_eq!(
            gate.check_after_hook_allow_rewritten(
                "Bash",
                &serde_json::json!({"command": "npm install"})
            )
            .await,
            PermissionDecision::Allow
        );
        assert_eq!(inner.calls(), 0);
    }

    /// REGRESSION (adversarial round 2, over-block #2): `_pt` reports an ask only
    /// for an ask RULE / safetyCheck / sandboxOverride. Guard asks tagged `Other`
    /// (path constraints, `$IFS` bash-safety, sed, PowerShell containment) are
    /// `type:"other"` in the oracle too and `_pt` SKIPS them — so they must not
    /// block a hook allow. Treating every non-mode ask as a verdict regressed
    /// this (an allow-all CI hook would suddenly prompt / hard-deny).
    #[tokio::test]
    async fn other_tagged_guard_asks_do_not_block_a_hook_allow() {
        let inner = RecordingInner::new(PermissionDecision::Deny {
            reason: "must not prompt".into(),
        });
        let policy = policy_with(r#"{ "permissions": {} }"#, PermissionMode::Default);
        let gate = PolicyPermissionGate::new(policy, inner.clone());

        // `$IFS` trips the bash-safety guard ask (`Other`-tagged) on the NORMAL
        // path; under a hook allow it must be ignored.
        for cmd in ["cat${IFS}/etc/passwd", "echo x > /etc/foo"] {
            let input = serde_json::json!({ "command": cmd });
            assert_eq!(
                gate.check_after_hook_allow("Bash", &input).await,
                PermissionDecision::Allow,
                "`{cmd}` is an Other-tagged guard ask — _pt skips it"
            );
            assert_eq!(
                gate.check_after_hook_allow_rewritten("Bash", &input).await,
                PermissionDecision::Allow,
                "`{cmd}` must not hard-deny the headless rescue either"
            );
        }
        assert_eq!(inner.calls(), 0, "no guard ask may reach the prompt here");
    }

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
            // `policy_with` loads the rule from `userSettings` — the raw
            // `SettingSource` token claude-code's `ZX_` reads off the matched
            // rule to label the OTEL decision source.
            PermissionResolution::Allow {
                rule_source: Some("userSettings".into())
            }
        );
        assert_eq!(
            inner.calls(),
            0,
            "resolve_detailed never consults the inner"
        );
    }

    #[tokio::test]
    async fn resolve_detailed_session_allow_rule_carries_session_source() {
        // A TUI "always allow" grant is a `session` rule — the one scope `ZX_`
        // renders as `user_temporary` rather than `user_permanent`.
        let policy = Arc::new(PermissionPolicy::from_rules(
            PermissionMode::Default,
            vec![crate::rule::PermissionRule::allow_tool_session("Bash")],
        ));
        let gate =
            PolicyPermissionGate::new(policy, RecordingInner::new(PermissionDecision::Allow));
        assert_eq!(
            gate.resolve_detailed("Bash", &serde_json::json!({})).await,
            PermissionResolution::Allow {
                rule_source: Some("session".into())
            }
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
            PermissionResolution::Deny {
                source,
                reason,
                rule_source,
                ..
            } => {
                assert_eq!(source, PermissionDecisionSource::Rule);
                assert_eq!(rule_source.as_deref(), Some("userSettings"));
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
            PermissionResolution::Deny {
                source,
                rule_source,
                ..
            } => {
                assert_eq!(source, PermissionDecisionSource::Mode);
                assert_eq!(rule_source, None, "a mode deny matched no rule");
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
            // No rule matched — the auto-allow carries no settings scope, so
            // `ZX_`'s default arm labels it "config".
            PermissionResolution::Allow { rule_source: None }
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
            PermissionResolution::Allow { rule_source: None }
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
        // classifier preserves the classifier's own free-text reason.
        assert_eq!(
            serialize_decision_reason(&PermissionDecisionReason::ClassifierRejected {
                classifier: ClassifierKind::Transcript,
                score: 0.9,
                reason: "blocked by transcript classifier".into(),
            }),
            Some("blocked by transcript classifier".to_string())
        );
        // sandboxOverride: enum reason, no faithful string → None.
        assert_eq!(
            serialize_decision_reason(&PermissionDecisionReason::SandboxOverride {
                reason: crate::result::SandboxOverrideReason::ExcludedCommand,
            }),
            None
        );
    }

    #[test]
    fn decision_reason_type_matches_binary_discriminants() {
        use crate::result::{ClassifierKind, SandboxOverrideReason};
        use crate::rule::{PermissionRule, PermissionRuleSource, PermissionRuleValue};
        let rule = PermissionRule {
            value: PermissionRuleValue::from_rule_string("Bash"),
            behavior: crate::rule::PermissionBehavior::Ask,
            source: PermissionRuleSource::UserSettings,
        };
        // The four where decision_reason TEXT is None → the type carries the info.
        assert_eq!(
            decision_reason_type(&PermissionDecisionReason::MatchedRule { rule }),
            Some("rule")
        );
        assert_eq!(
            decision_reason_type(&PermissionDecisionReason::PermissionMode {
                mode: PermissionMode::Default
            }),
            Some("mode")
        );
        assert_eq!(
            decision_reason_type(&PermissionDecisionReason::PermissionPromptTool {
                tool_name: "mcp__x".into()
            }),
            Some("permissionPromptTool")
        );
        assert_eq!(
            decision_reason_type(&PermissionDecisionReason::HookOverride {
                hook_id: "h1".into(),
                source: None,
                reason: None,
            }),
            Some("hook")
        );
        assert_eq!(
            decision_reason_type(&PermissionDecisionReason::ClassifierRejected {
                classifier: ClassifierKind::Transcript,
                score: 0.9,
                reason: "blocked".into(),
            }),
            Some("classifier")
        );
        assert_eq!(
            decision_reason_type(&PermissionDecisionReason::SandboxOverride {
                reason: SandboxOverrideReason::ExcludedCommand,
            }),
            Some("sandboxOverride")
        );
        assert_eq!(
            decision_reason_type(&PermissionDecisionReason::Other { reason: "m".into() }),
            Some("other")
        );
        // LingXi-internal reasons carry no CC `.type`.
        assert_eq!(
            decision_reason_type(&PermissionDecisionReason::BypassPermissions),
            None
        );
        assert_eq!(
            decision_reason_type(&PermissionDecisionReason::AutoModeFallback),
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
                decision_classification: None,
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
                permission_updates: Vec::new(),
                decision_classification: None,
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
        assert_eq!(seen.classifier_approvable, None);
        assert_eq!(seen.matched_ask_rule, None);
    }

    #[tokio::test]
    async fn interactive_denial_breaker_forwards_rewritten_classifier_reason() {
        let policy = Arc::new(PermissionPolicy::from_rules(
            PermissionMode::Auto,
            std::iter::empty(),
        ));
        let inner = Arc::new(ContextRecordingInner {
            ctx: std::sync::Mutex::new(None),
        });
        let gate = PolicyPermissionGate::new(policy, inner.clone());
        let ctx = PermissionCheckContext {
            is_non_interactive_session: false,
            tool_use_id: Some("toolu_breaker".into()),
            ..Default::default()
        };
        let input = json!({ "command": "git reset --hard" });

        for _ in 0..2 {
            assert!(matches!(
                gate.check_with_context("Bash", &input, &ctx).await,
                PermissionOutcome::Deny { .. }
            ));
        }
        assert_eq!(
            gate.check_with_context("Bash", &input, &ctx).await,
            PermissionOutcome::Allow {
                updated_input: None,
                permission_updates: Vec::new(),
                decision_classification: None,
            }
        );
        let seen = inner.ctx.lock().unwrap().clone().expect("inner consulted");
        assert_eq!(seen.tool_use_id.as_deref(), Some("toolu_breaker"));
        assert_eq!(seen.decision_reason_type.as_deref(), Some("classifier"));
        assert_eq!(
            seen.decision_reason.as_deref(),
            Some("3 consecutive actions were blocked. Please review the transcript before continuing.\n\nLatest blocked action: Auto-mode BLOCK policy matched shell command")
        );
    }

    /// Review MED: a hook-allow overridden by an ASK RULE re-checks via
    /// `check_after_hook_allow_ctx`, which must delegate to the inner transport
    /// carrying the REAL tool_use_id (was a fresh UUID) + the serialized ask
    /// reason — so the stdio `can_use_tool` is byte-faithful, matching `lin`.
    #[tokio::test]
    async fn hook_allow_ask_delegation_carries_tool_use_id_and_reason() {
        let policy = policy_with(
            r#"{ "permissions": { "ask": ["Bash"] } }"#,
            PermissionMode::Default,
        );
        let inner = Arc::new(ContextRecordingInner {
            ctx: std::sync::Mutex::new(None),
        });
        let gate = PolicyPermissionGate::new(policy, inner.clone());
        let ctx = PermissionCheckContext {
            tool_use_id: Some("toolu_hook_99".into()),
            ..Default::default()
        };
        let outcome = gate
            .check_after_hook_allow_outcome_ctx(
                "Bash",
                &serde_json::json!({ "command": "ls" }),
                &ctx,
            )
            .await;
        assert!(matches!(
            outcome,
            PermissionOutcome::Allow {
                decision_classification: Some(
                    traits::permission_gate::ToolDecisionClassification::UserTemporary
                ),
                ..
            }
        ));
        let seen = inner
            .ctx
            .lock()
            .unwrap()
            .clone()
            .expect("ask rule must delegate to the inner transport");
        assert_eq!(
            seen.tool_use_id.as_deref(),
            Some("toolu_hook_99"),
            "the real tool_use_id must reach the inner (not a fresh UUID)"
        );
        // An ask RULE serializes to a reason (MatchedRule → decision_reason_type "rule").
        assert_eq!(seen.decision_reason_type.as_deref(), Some("rule"));
    }

    #[tokio::test]
    async fn interaction_metadata_does_not_create_a_duplicate_permission_prompt() {
        let policy = policy_with(r#"{ "permissions": {} }"#, PermissionMode::Default);
        let inner = Arc::new(ContextRecordingInner {
            ctx: std::sync::Mutex::new(None),
        });
        let gate = PolicyPermissionGate::new(policy, inner.clone());
        let ctx = PermissionCheckContext {
            requires_user_interaction: true,
            ..Default::default()
        };

        let outcome = gate
            .check_with_context("AskUserQuestion", &json!({}), &ctx)
            .await;

        assert_eq!(
            outcome,
            PermissionOutcome::Allow {
                updated_input: None,
                permission_updates: Vec::new(),
                decision_classification: None,
            }
        );
        assert!(
            inner.ctx.lock().unwrap().is_none(),
            "requires_user_interaction is metadata only; the tool's dedicated UI must not be preceded by a generic permission prompt"
        );
    }

    #[tokio::test]
    async fn delegated_ask_carries_exact_rule_but_no_invented_suggestion() {
        let policy = policy_with(
            r#"{ "permissions": { "ask": ["Bash"] } }"#,
            PermissionMode::Default,
        );
        let inner = Arc::new(ContextRecordingInner {
            ctx: std::sync::Mutex::new(None),
        });
        let gate = PolicyPermissionGate::new(policy, inner.clone());
        let original = PermissionCheckContext {
            tool_use_id: Some("toolu_rule".into()),
            requires_user_interaction: true,
            ..Default::default()
        };
        let _ = gate
            .check_with_context("Bash", &json!({ "command": "echo ok" }), &original)
            .await;

        let seen = inner.ctx.lock().unwrap().clone().expect("inner consulted");
        assert_eq!(seen.decision_reason_type.as_deref(), Some("rule"));
        assert_eq!(seen.requires_user_interaction, true);
        // A PLAIN ask-rule ask conveys the rule via `decision_reason_type: "rule"`
        // and carries NO `matched_ask_rule`: the 2.1.218 schema sets that field
        // ONLY in the substitution case (a rule forces the prompt but the ask
        // keeps the tool's own decision_reason, so the rule rides in
        // matched_ask_rule *instead of* the "rule" type). The two are mutually
        // exclusive; emitting both (the prior behavior) is not something the
        // oracle does.
        assert_eq!(seen.matched_ask_rule, None);
        // The oracle's `_pt` ask-rule arm carries NO `permission_suggestions`
        // (the previously-emitted addRules/allow/session payload was invented +
        // inert — review finding).
        assert_eq!(seen.permission_suggestions, None);
    }

    #[tokio::test]
    async fn delegated_safety_ask_carries_classifier_approvable_false() {
        let policy = policy_with_roots(r#"{ "permissions": {} }"#, PermissionMode::Default);
        let inner = Arc::new(ContextRecordingInner {
            ctx: std::sync::Mutex::new(None),
        });
        let gate = PolicyPermissionGate::new(policy, inner.clone());
        let _ = gate
            .check_with_context(
                "Bash",
                &json!({ "command": "rm -rf /" }),
                &PermissionCheckContext::default(),
            )
            .await;

        let seen = inner.ctx.lock().unwrap().clone().expect("inner consulted");
        assert_eq!(seen.decision_reason_type.as_deref(), Some("safetyCheck"));
        assert_eq!(seen.classifier_approvable, Some(false));
    }

    #[test]
    fn classifier_approvable_folds_nested_safety_checks() {
        let safety = |approvable| PermissionResult::Ask {
            reason: PermissionDecisionReason::SafetyCheck {
                reason: "test safety check".into(),
                classifier_approvable: approvable,
            },
            prompt: crate::result::PermissionPrompt {
                title: "Allow?".into(),
                message: "test".into(),
                options: Vec::new(),
            },
            pending_classifier_check: None,
            metadata: PermissionMetadata::default(),
        };
        let reason = PermissionDecisionReason::SubcommandResults {
            reasons: HashMap::from([
                ("safe".into(), Box::new(safety(true))),
                ("unsafe".into(), Box::new(safety(false))),
            ]),
        };

        assert_eq!(classifier_approvable(&reason), Some(false));
        assert_eq!(
            classifier_approvable(&PermissionDecisionReason::PermissionMode {
                mode: PermissionMode::Default,
            }),
            None,
            "the field is omitted when no safety check exists"
        );
    }

    #[tokio::test]
    async fn delegated_path_ask_carries_structured_blocked_path() {
        let policy = policy_with_roots(r#"{ "permissions": {} }"#, PermissionMode::Default);
        let inner = Arc::new(ContextRecordingInner {
            ctx: std::sync::Mutex::new(None),
        });
        let gate = PolicyPermissionGate::new(policy, inner.clone());
        let _ = gate
            .check_with_context(
                "Bash",
                &json!({ "command": "cat /etc/passwd" }),
                &PermissionCheckContext::default(),
            )
            .await;

        let seen = inner.ctx.lock().unwrap().clone().expect("inner consulted");
        assert_eq!(seen.blocked_path.as_deref(), Some("/etc/passwd"));
    }

    #[tokio::test]
    async fn check_with_context_mode_override_gates_mutation_under_plan() {
        // A per-call plan override (a spawned `mode:"plan"` child, claude-code
        // 2.1.207 `ve`) re-authorizes THIS dispatch under Plan even though the
        // gate's boot mode (bypassPermissions) would allow the mutation outright:
        // the mutation trips the plan backstop → delegated to the inner transport,
        // while a read-only tool stays frictionless. The gate's own mode is never
        // mutated (the parent's checks are unaffected).
        let policy = policy_with(
            r#"{ "permissions": {} }"#,
            PermissionMode::BypassPermissions,
        );
        let inner = RecordingInner::new(PermissionDecision::Deny {
            reason: "plan blocks writes".into(),
        });
        let gate = PolicyPermissionGate::new(policy, inner.clone());

        // No override: boot bypass allows the write, inner never consulted.
        let baseline = gate
            .check_with_context(
                "Edit",
                &serde_json::json!({ "file_path": "/x.rs" }),
                &PermissionCheckContext::default(),
            )
            .await;
        assert!(
            matches!(baseline, PermissionOutcome::Allow { .. }),
            "boot bypassPermissions allows the mutation with no override"
        );
        assert_eq!(inner.calls(), 0, "bypass does not delegate to the prompt");

        // Override plan: Edit trips the plan backstop → delegated to inner (Deny).
        let plan_ctx = PermissionCheckContext {
            mode_override: Some("plan".into()),
            ..Default::default()
        };
        let edit = gate
            .check_with_context(
                "Edit",
                &serde_json::json!({ "file_path": "/x.rs" }),
                &plan_ctx,
            )
            .await;
        match edit {
            PermissionOutcome::Deny { reason } => {
                assert!(reason.contains("plan blocks writes"), "got {reason}");
            }
            other => panic!("expected Deny under plan override, got {other:?}"),
        }
        assert_eq!(
            inner.calls(),
            1,
            "the plan override delegates the mutation to the inner transport"
        );

        // Override plan: Read is plan-safe / AllowByDefault → auto-allow, no prompt.
        let read = gate
            .check_with_context("Read", &serde_json::json!({}), &plan_ctx)
            .await;
        assert!(
            matches!(read, PermissionOutcome::Allow { .. }),
            "a plan child's reads stay frictionless"
        );
        assert_eq!(
            inner.calls(),
            1,
            "plan-safe read auto-allows without delegating"
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
                reason: "blocked".into(),
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
    async fn hook_ask_floor_prevents_classifier_from_defeating_hook_ask() {
        // HOOK-ASKFLOOR-03: Auto mode + a safe local shell the classifier WOULD
        // allow. Without the floor the classifier auto-allows (inner untouched);
        // WITH the floor (a PreToolUse hook returned `ask`) the classifier is
        // SKIPPED, so the ask delegates to the inner transport (prompt / headless
        // deny) — the hook's ask is honored, not silently re-allowed.
        let input = serde_json::json!({ "command": "cargo test -p permission" });

        // No floor → classifier allows, inner not consulted.
        let policy = Arc::new(PermissionPolicy::from_rules(
            PermissionMode::Auto,
            std::iter::empty(),
        ));
        let inner = RecordingInner::new(PermissionDecision::Deny {
            reason: "asked".into(),
        });
        let gate = PolicyPermissionGate::new(policy, inner.clone());
        let no_floor = gate
            .check_with_context("Bash", &input, &PermissionCheckContext::default())
            .await;
        assert!(
            matches!(no_floor, PermissionOutcome::Allow { .. }),
            "no floor: classifier allows"
        );
        assert_eq!(inner.calls(), 0, "no floor: classifier skips the prompt");

        // Floor set → classifier skipped, ask delegated to inner (→ Deny here).
        let policy2 = Arc::new(PermissionPolicy::from_rules(
            PermissionMode::Auto,
            std::iter::empty(),
        ));
        let inner2 = RecordingInner::new(PermissionDecision::Deny {
            reason: "asked".into(),
        });
        let gate2 = PolicyPermissionGate::new(policy2, inner2.clone());
        let ctx = PermissionCheckContext {
            hook_ask_floor: true,
            ..Default::default()
        };
        let floored = gate2.check_with_context("Bash", &input, &ctx).await;
        assert!(
            matches!(floored, PermissionOutcome::Deny { .. }),
            "floor: classifier must NOT re-allow the hook's ask"
        );
        assert!(
            inner2.calls() >= 1,
            "floor: the ask must reach the prompt/inner, not classifier-allow"
        );
    }

    /// PERM-07 (claude-code 2.1.238 `STv` → `DJa` → `Ixf`): Auto mode + a
    /// PreToolUse hook ask floor + a session that cannot surface prompts is a
    /// hard DENY carrying the 2.1.238 wrapper copy — the inner transport is
    /// never consulted. 2.1.220's `G8s` forwarded the ask message verbatim.
    #[tokio::test]
    async fn prompts_unavailable_ask_floor_denies_with_2_1_238_copy() {
        // The builder itself, byte-for-byte.
        assert_eq!(
            prompts_unavailable_deny_message("<ask>"),
            "Permission for this tool use was denied: it requires interactive approval, and permission prompts are not available in this session. The action was NOT performed. Do not claim it succeeded, and do not retry it in this session \u{2014} report the limitation to the user, or suggest an alternative. What was requested: <ask>"
        );
        assert_eq!(
            PROMPTS_UNAVAILABLE_ASYNC_AGENT_REASON,
            "Action requires interactive approval and permission prompts are not available in this context"
        );

        let input = serde_json::json!({ "command": "cargo test -p permission" });
        let policy = Arc::new(PermissionPolicy::from_rules(
            PermissionMode::Auto,
            std::iter::empty(),
        ));
        let inner = RecordingInner::new(PermissionDecision::Allow);
        let gate = PolicyPermissionGate::new(policy, inner.clone());
        let ctx = PermissionCheckContext {
            hook_ask_floor: true,
            is_non_interactive_session: true,
            ..Default::default()
        };
        match gate.check_with_context("Bash", &input, &ctx).await {
            PermissionOutcome::Deny { reason } => {
                assert!(
                    reason.starts_with(
                        "Permission for this tool use was denied: it requires interactive approval,"
                    ),
                    "deny must carry the Ixf wrapper: {reason}"
                );
                assert!(
                    reason.contains(" What was requested: "),
                    "the ask message must be interpolated: {reason}"
                );
                // NOT the separate, unchanged headless `xxf`/`GRu` message.
                assert!(
                    !reason.starts_with("Permission to use Bash has been denied."),
                    "must not fall through to the headless GRu message: {reason}"
                );
            }
            other => panic!("expected Deny, got {other:?}"),
        }
        assert_eq!(
            inner.calls(),
            0,
            "the prompts-unavailable deny must not reach the inner transport"
        );

        // An INTERACTIVE session with the same floor still delegates the ask.
        let policy2 = Arc::new(PermissionPolicy::from_rules(
            PermissionMode::Auto,
            std::iter::empty(),
        ));
        let inner2 = RecordingInner::new(PermissionDecision::Allow);
        let gate2 = PolicyPermissionGate::new(policy2, inner2.clone());
        let ctx2 = PermissionCheckContext {
            hook_ask_floor: true,
            ..Default::default()
        };
        let _ = gate2.check_with_context("Bash", &input, &ctx2).await;
        assert!(
            inner2.calls() >= 1,
            "an interactive ask floor must still delegate to the prompt"
        );
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
            PermissionResolution::Deny {
                source,
                reason,
                decision_reason_type,
                decision_reason,
                ..
            } => {
                assert_eq!(source, PermissionDecisionSource::Classifier);
                assert!(
                    reason.contains("Auto mode classifier blocked action"),
                    "reason carries classifier block text: {reason}"
                );
                assert_eq!(decision_reason_type.as_deref(), Some("classifier"));
                assert_eq!(
                    decision_reason.as_deref(),
                    Some("Auto-mode BLOCK policy matched shell command")
                );
            }
            other => panic!("expected classifier deny, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn auto_mode_session_transcript_deny_feeds_denial_tracking() {
        // A transcript-tamper edit denies with the CC "Session Transcript
        // Tampering" category and increments the denial breaker exactly like any
        // other auto-mode BLOCK category (record_auto_deny).
        let policy = Arc::new(PermissionPolicy::from_rules(
            PermissionMode::Auto,
            std::iter::empty(),
        ));
        let gate = PolicyPermissionGate::new(
            policy.clone(),
            RecordingInner::new(PermissionDecision::Allow),
        );

        match gate
            .resolve_detailed(
                "Edit",
                &serde_json::json!({ "file_path": "/Users/x/.lingxi/projects/p/s.jsonl" }),
            )
            .await
        {
            PermissionResolution::Deny { source, reason, .. } => {
                assert_eq!(source, PermissionDecisionSource::Classifier);
                assert!(
                    reason.contains("Session Transcript Tampering"),
                    "reason names the CC category: {reason}"
                );
            }
            other => panic!("expected transcript-tamper deny, got {other:?}"),
        }

        let tracking = policy.denial_tracking.lock().unwrap();
        assert_eq!(tracking.total_denials, 1, "deny must feed denial tracking");
        assert_eq!(tracking.consecutive_denials, 1);
    }

    #[tokio::test]
    async fn headless_auto_mode_denial_limit_returns_exact_abort() {
        let policy = Arc::new(PermissionPolicy::from_rules(
            PermissionMode::Auto,
            std::iter::empty(),
        ));
        let inner = RecordingInner::new(PermissionDecision::Allow);
        let gate = PolicyPermissionGate::new(policy, inner.clone());
        let input = serde_json::json!({ "command": "git reset --hard" });
        let ctx = PermissionCheckContext {
            is_non_interactive_session: true,
            ..Default::default()
        };

        for _ in 0..2 {
            assert!(matches!(
                gate.resolve_detailed_or_abort("Bash", &input, &ctx)
                    .await
                    .expect("below the limit is an ordinary classifier decision"),
                PermissionResolution::Deny {
                    source: PermissionDecisionSource::Classifier,
                    ..
                }
            ));
        }

        let abort = gate
            .resolve_detailed_or_abort("Bash", &input, &ctx)
            .await
            .expect_err("the third consecutive headless denial must abort");
        assert_eq!(
            abort.message,
            "Agent aborted: too many classifier denials in headless mode"
        );
        assert_eq!(
            inner.calls(),
            0,
            "headless breaker must abort before delegating to the interactive inner gate"
        );
    }

    #[tokio::test]
    async fn interactive_auto_mode_denial_limit_still_falls_back_to_ask() {
        let policy = Arc::new(PermissionPolicy::from_rules(
            PermissionMode::Auto,
            std::iter::empty(),
        ));
        let gate =
            PolicyPermissionGate::new(policy, RecordingInner::new(PermissionDecision::Allow));
        let input = serde_json::json!({ "command": "git reset --hard" });
        let ctx = PermissionCheckContext::default();

        for _ in 0..2 {
            assert!(matches!(
                gate.resolve_detailed_or_abort("Bash", &input, &ctx)
                    .await
                    .expect("below the limit is an ordinary classifier decision"),
                PermissionResolution::Deny {
                    source: PermissionDecisionSource::Classifier,
                    ..
                }
            ));
        }

        assert_eq!(
            gate.resolve_detailed_or_abort("Bash", &input, &ctx)
                .await
                .expect("interactive sessions fall back to prompting"),
            PermissionResolution::AskWithContext {
                decision_reason_type: Some("classifier".into()),
                decision_reason: Some(
                    "3 consecutive actions were blocked. Please review the transcript before continuing.\n\nLatest blocked action: Auto-mode BLOCK policy matched shell command".into(),
                ),
            }
        );
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
    async fn permission_mode_getter_reflects_boot_mode_and_live_override() {
        // Boot mode is reported as its wire string (the `/resume` snapshot source).
        let policy = Arc::new(
            PermissionPolicy::from_rules(PermissionMode::AcceptEdits, Vec::new())
                .with_bypass_available(true),
        );
        let inner = RecordingInner::new(PermissionDecision::Allow);
        let gate = PolicyPermissionGate::new(policy, inner);
        assert_eq!(gate.permission_mode().as_deref(), Some("acceptEdits"));

        // A live Shift+Tab switch is reflected too, so the re-mount carries it.
        gate.set_permission_mode("bypassPermissions").await.unwrap();
        assert_eq!(gate.permission_mode().as_deref(), Some("bypassPermissions"));

        // An unknown mode is a no-op — the getter keeps the prior mode.
        gate.set_permission_mode("gibberish").await.unwrap();
        assert_eq!(gate.permission_mode().as_deref(), Some("bypassPermissions"));
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
    async fn set_permission_mode_manual_aliases_to_default() {
        // 2.1.211 `LE`/PERMISSION_MODE_MANUAL_ALIAS: "manual" → default. Boot in
        // bypassPermissions (Write allows), switch to "manual" → the mode really
        // CHANGES to default (Write now asks → delegates to Deny), distinguishing
        // it from the unknown-string no-op (which would keep bypass → Allow).
        let policy = Arc::new(PermissionPolicy::from_rules(
            PermissionMode::BypassPermissions,
            Vec::new(),
        ));
        let inner = RecordingInner::new(PermissionDecision::Deny {
            reason: "prompt-denied".into(),
        });
        let gate = PolicyPermissionGate::new(policy, inner.clone());
        assert_eq!(
            gate.check("Write", &serde_json::json!({})).await,
            PermissionDecision::Allow
        );
        gate.set_permission_mode("manual").await.unwrap();
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

    #[tokio::test]
    async fn interactive_confirmation_unlocks_bypass_for_the_current_session() {
        let policy = PermissionPolicy::from_rules(PermissionMode::Default, Vec::new());
        let gate = PolicyPermissionGate::new(
            Arc::new(policy),
            RecordingInner::new(PermissionDecision::Deny {
                reason: "prompt-denied".into(),
            }),
        );

        assert!(gate.can_request_bypass_permissions());
        assert!(gate.set_permission_mode("bypassPermissions").await.is_err());

        gate.confirm_bypass_permissions()
            .expect("interactive acknowledgement unlocks bypass mode");
        gate.set_permission_mode("bypassPermissions")
            .await
            .expect("confirmed bypass mode is accepted");
        assert_eq!(
            gate.check("Write", &serde_json::json!({})).await,
            PermissionDecision::Allow
        );

        gate.set_permission_mode("plan")
            .await
            .expect("the user can leave bypass mode");
        assert!(matches!(
            gate.check("Write", &serde_json::json!({})).await,
            PermissionDecision::Deny { .. }
        ));
        assert!(gate.set_permission_mode("bypassPermissions").await.is_err());
    }

    #[tokio::test]
    async fn restored_session_bypass_does_not_require_a_new_live_confirmation() {
        let policy = PermissionPolicy::from_rules(PermissionMode::Default, Vec::new());
        let gate = PolicyPermissionGate::new(
            Arc::new(policy),
            RecordingInner::new(PermissionDecision::Deny {
                reason: "prompt-denied".into(),
            }),
        );

        gate.restore_session_permission_mode("bypassPermissions")
            .await
            .expect("a session's persisted bypass mode restores");
        assert_eq!(
            gate.check("Write", &serde_json::json!({})).await,
            PermissionDecision::Allow
        );
    }

    #[tokio::test]
    async fn set_permission_mode_rejects_auto_when_disabled_by_settings() {
        // `Nle`: auto is gated by `!P0()`; the `disableAutoMode` killswitch
        // (auto_mode_disabled) makes `One()` return "settings".
        let mut policy = PermissionPolicy::from_rules(PermissionMode::Default, Vec::new());
        policy.auto_mode_disabled = true;
        let gate = PolicyPermissionGate::new(
            Arc::new(policy),
            RecordingInner::new(PermissionDecision::Allow),
        );
        assert_eq!(
            gate.set_permission_mode("auto").await.unwrap_err(),
            "Cannot set permission mode to auto: auto mode disabled by settings"
        );
    }

    #[tokio::test]
    async fn set_permission_mode_rejects_auto_when_circuit_broken() {
        // Killswitch off but the local denial breaker has tripped → "circuit-breaker".
        let policy = PermissionPolicy::from_rules(PermissionMode::Default, Vec::new());
        {
            let mut t = policy.denial_tracking.lock().unwrap();
            t.record_auto_deny();
            t.record_auto_deny();
            t.record_auto_deny(); // 3 consecutive ≥ maxConsecutive → broken
            assert!(t.is_circuit_broken());
        }
        let gate = PolicyPermissionGate::new(
            Arc::new(policy),
            RecordingInner::new(PermissionDecision::Allow),
        );
        assert_eq!(
            gate.set_permission_mode("auto").await.unwrap_err(),
            "Cannot set permission mode to auto: auto mode is unavailable for your plan"
        );
    }

    #[tokio::test]
    async fn set_permission_mode_auto_settings_precedes_circuit_breaker() {
        // Both closed → `One()` reports "settings" first.
        let mut policy = PermissionPolicy::from_rules(PermissionMode::Default, Vec::new());
        policy.auto_mode_disabled = true;
        {
            let mut t = policy.denial_tracking.lock().unwrap();
            t.record_auto_deny();
            t.record_auto_deny();
            t.record_auto_deny();
        }
        let gate = PolicyPermissionGate::new(
            Arc::new(policy),
            RecordingInner::new(PermissionDecision::Allow),
        );
        assert_eq!(
            gate.set_permission_mode("auto").await.unwrap_err(),
            "Cannot set permission mode to auto: auto mode disabled by settings"
        );
    }

    #[tokio::test]
    async fn set_permission_mode_accepts_auto_when_gate_open() {
        // Killswitch off, breaker not tripped, and NO live-model cell set (so the
        // model gate is skipped, fail-open) → auto is accepted.
        let policy = PermissionPolicy::from_rules(PermissionMode::Default, Vec::new());
        let gate = PolicyPermissionGate::new(
            Arc::new(policy),
            RecordingInner::new(PermissionDecision::Allow),
        );
        gate.set_permission_mode("auto")
            .await
            .expect("auto accepted when the gate is open");
    }

    #[tokio::test]
    async fn set_permission_mode_rejects_auto_on_unsupported_live_model() {
        // Killswitch off, breaker not tripped, but the LIVE session model is on
        // the `dUe` shared exclusion list → `One()` returns "model" (`Nle` rejects
        // with the byte-exact model message). Mirrors a `/model` switch to an
        // auto-unsupported model followed by a live switch to auto.
        let policy = PermissionPolicy::from_rules(PermissionMode::Default, Vec::new());
        let gate = PolicyPermissionGate::new(
            Arc::new(policy),
            RecordingInner::new(PermissionDecision::Allow),
        );
        let provider: crate::policy_gate::LiveModelProvider = Arc::new(|| {
            Some(crate::policy_gate::LiveModelContext {
                model: "claude-sonnet-4-5".to_string(),
                provider: "firstParty".to_string(),
            })
        });
        gate.live_model_provider_handle()
            .set(provider)
            .unwrap_or_else(|_| panic!("cell set once"));
        assert_eq!(
            gate.set_permission_mode("auto").await.unwrap_err(),
            "Cannot set permission mode to auto: auto mode unavailable for this model"
        );
    }

    #[tokio::test]
    async fn set_permission_mode_accepts_auto_on_supported_live_model() {
        // Live model supports auto → the model branch passes; auto is accepted.
        let policy = PermissionPolicy::from_rules(PermissionMode::Default, Vec::new());
        let gate = PolicyPermissionGate::new(
            Arc::new(policy),
            RecordingInner::new(PermissionDecision::Allow),
        );
        let provider: crate::policy_gate::LiveModelProvider = Arc::new(|| {
            Some(crate::policy_gate::LiveModelContext {
                model: "claude-opus-4-8".to_string(),
                provider: "firstParty".to_string(),
            })
        });
        gate.live_model_provider_handle()
            .set(provider)
            .unwrap_or_else(|_| panic!("cell set once"));
        gate.set_permission_mode("auto")
            .await
            .expect("auto accepted on a supported live model");
    }

    #[tokio::test]
    async fn set_permission_mode_settings_precedes_live_model() {
        // Both the settings killswitch AND an unsupported live model close the
        // gate → `One()` precedence reports "settings" (settings > model).
        let mut policy = PermissionPolicy::from_rules(PermissionMode::Default, Vec::new());
        policy.auto_mode_disabled = true;
        let gate = PolicyPermissionGate::new(
            Arc::new(policy),
            RecordingInner::new(PermissionDecision::Allow),
        );
        let provider: crate::policy_gate::LiveModelProvider = Arc::new(|| {
            Some(crate::policy_gate::LiveModelContext {
                model: "claude-sonnet-4-5".to_string(),
                provider: "firstParty".to_string(),
            })
        });
        gate.live_model_provider_handle()
            .set(provider)
            .unwrap_or_else(|_| panic!("cell set once"));
        assert_eq!(
            gate.set_permission_mode("auto").await.unwrap_err(),
            "Cannot set permission mode to auto: auto mode disabled by settings"
        );
    }

    #[tokio::test]
    async fn set_permission_mode_auto_fails_open_when_live_model_unreadable() {
        // The provider returns None (a contended session lock) → the model gate is
        // skipped (fail-open) and auto is accepted, matching every other post-orch
        // live cell's non-blocking read.
        let policy = PermissionPolicy::from_rules(PermissionMode::Default, Vec::new());
        let gate = PolicyPermissionGate::new(
            Arc::new(policy),
            RecordingInner::new(PermissionDecision::Allow),
        );
        let provider: crate::policy_gate::LiveModelProvider = Arc::new(|| None);
        gate.live_model_provider_handle()
            .set(provider)
            .unwrap_or_else(|_| panic!("cell set once"));
        gate.set_permission_mode("auto")
            .await
            .expect("auto accepted when the live model is unreadable");
    }

    #[tokio::test]
    async fn set_permission_mode_uses_live_non_first_party_provider() {
        let policy = PermissionPolicy::from_rules(PermissionMode::Default, Vec::new());
        let gate = PolicyPermissionGate::new(
            Arc::new(policy),
            RecordingInner::new(PermissionDecision::Allow),
        );
        let provider: crate::policy_gate::LiveModelProvider = Arc::new(|| {
            Some(crate::policy_gate::LiveModelContext {
                model: "claude-sonnet-4-6".to_string(),
                provider: "vertex".to_string(),
            })
        });
        gate.live_model_provider_handle()
            .set(provider)
            .unwrap_or_else(|_| panic!("cell set once"));
        assert_eq!(
            gate.set_permission_mode("auto").await.unwrap_err(),
            "Cannot set permission mode to auto: auto mode unavailable for this model"
        );
    }

    #[tokio::test]
    async fn mcp_permission_mode_override_downgrades_target_server_only() {
        let policy = Arc::new(
            PermissionPolicy::from_rules(PermissionMode::BypassPermissions, Vec::new())
                .with_bypass_available(true),
        );
        let inner = RecordingInner::new(PermissionDecision::Deny {
            reason: "prompt-denied".into(),
        });
        let gate = PolicyPermissionGate::new(policy, inner.clone());

        assert_eq!(
            gate.check("Write", &serde_json::json!({})).await,
            PermissionDecision::Allow
        );
        assert_eq!(
            gate.check("mcp__context7__lookup", &serde_json::json!({}))
                .await,
            PermissionDecision::Allow
        );

        gate.set_mcp_permission_mode_override("context7", Some("default"))
            .await
            .unwrap();

        assert!(matches!(
            gate.check("mcp__context7__lookup", &serde_json::json!({}))
                .await,
            PermissionDecision::Deny { .. }
        ));
        assert_eq!(
            gate.check("mcp__other__lookup", &serde_json::json!({}))
                .await,
            PermissionDecision::Allow
        );
        assert_eq!(
            gate.check("Write", &serde_json::json!({})).await,
            PermissionDecision::Allow
        );

        gate.set_mcp_permission_mode_override("context7", None)
            .await
            .unwrap();
        assert_eq!(
            gate.check("mcp__context7__lookup", &serde_json::json!({}))
                .await,
            PermissionDecision::Allow
        );
    }

    #[tokio::test]
    async fn mcp_permission_override_uses_the_shared_claude_ai_normalizer() {
        let policy = Arc::new(
            PermissionPolicy::from_rules(PermissionMode::BypassPermissions, Vec::new())
                .with_bypass_available(true),
        );
        let gate = PolicyPermissionGate::new(
            policy,
            RecordingInner::new(PermissionDecision::Deny {
                reason: "prompt-denied".into(),
            }),
        );

        gate.set_mcp_permission_mode_override("claude.ai .a..b ", Some("default"))
            .await
            .unwrap();

        assert!(matches!(
            gate.check("mcp__claude_ai_a_b__lookup", &serde_json::json!({}))
                .await,
            PermissionDecision::Deny { .. }
        ));
        assert_eq!(
            gate.check("mcp__claude_ai_a__lookup", &serde_json::json!({}))
                .await,
            PermissionDecision::Allow
        );
    }

    #[tokio::test]
    async fn mcp_permission_mode_override_does_not_loosen_default_session() {
        let policy = Arc::new(PermissionPolicy::from_rules(
            PermissionMode::Default,
            Vec::new(),
        ));
        let inner = RecordingInner::new(PermissionDecision::Deny {
            reason: "prompt-denied".into(),
        });
        let gate = PolicyPermissionGate::new(policy, inner);

        gate.set_mcp_permission_mode_override("context7", Some("auto"))
            .await
            .unwrap();

        assert!(matches!(
            gate.check("mcp__context7__lookup", &serde_json::json!({}))
                .await,
            PermissionDecision::Deny { .. }
        ));
    }

    #[tokio::test]
    async fn mcp_auto_override_rejects_when_disabled_by_settings() {
        let mut policy = PermissionPolicy::from_rules(PermissionMode::Default, Vec::new());
        policy.auto_mode_disabled = true;
        let gate = PolicyPermissionGate::new(
            Arc::new(policy),
            RecordingInner::new(PermissionDecision::Allow),
        );

        assert_eq!(
            gate.set_mcp_permission_mode_override("context7", Some("auto"))
                .await
                .unwrap_err(),
            "Cannot pin MCP server 'context7' to auto: auto mode disabled by settings"
        );
    }

    #[tokio::test]
    async fn mcp_auto_override_rejects_when_circuit_broken() {
        let policy = PermissionPolicy::from_rules(PermissionMode::Default, Vec::new());
        {
            let mut tracking = policy.denial_tracking.lock().unwrap();
            tracking.record_auto_deny();
            tracking.record_auto_deny();
            tracking.record_auto_deny();
        }
        let gate = PolicyPermissionGate::new(
            Arc::new(policy),
            RecordingInner::new(PermissionDecision::Allow),
        );

        assert_eq!(
            gate.set_mcp_permission_mode_override("context7", Some("auto"))
                .await
                .unwrap_err(),
            "Cannot pin MCP server 'context7' to auto: auto mode is unavailable for your plan"
        );
    }

    #[tokio::test]
    async fn mcp_auto_override_rejects_unsupported_live_model() {
        let policy = PermissionPolicy::from_rules(PermissionMode::Default, Vec::new());
        let gate = PolicyPermissionGate::new(
            Arc::new(policy),
            RecordingInner::new(PermissionDecision::Allow),
        );
        let provider: crate::policy_gate::LiveModelProvider = Arc::new(|| {
            Some(crate::policy_gate::LiveModelContext {
                model: "claude-sonnet-4-5".to_string(),
                provider: "firstParty".to_string(),
            })
        });
        gate.live_model_provider_handle()
            .set(provider)
            .unwrap_or_else(|_| panic!("cell set once"));

        assert_eq!(
            gate.set_mcp_permission_mode_override("context7", Some("auto"))
                .await
                .unwrap_err(),
            "Cannot pin MCP server 'context7' to auto: auto mode unavailable for this model"
        );
    }

    #[tokio::test]
    async fn mcp_permission_mode_override_rejects_non_tightening_modes() {
        let policy = Arc::new(PermissionPolicy::from_rules(
            PermissionMode::Default,
            Vec::new(),
        ));
        let gate =
            PolicyPermissionGate::new(policy, RecordingInner::new(PermissionDecision::Allow));

        assert_eq!(
            gate.set_mcp_permission_mode_override("context7", Some("bypassPermissions"))
                .await
                .unwrap_err(),
            "Permission mode override over the control channel is tighten-only ('default', 'auto', or null); rejected 'bypassPermissions'"
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

    // ---- PERM-GATE-UPDATES-01: live in-memory update apply (Xb) ----

    #[test]
    fn apply_permission_update_setmode_changes_live_mode() {
        let policy = policy_with(r#"{ "permissions": {} }"#, PermissionMode::Default);
        let gate =
            PolicyPermissionGate::new(policy, RecordingInner::new(PermissionDecision::Allow));
        assert_eq!(gate.effective_mode(), PermissionMode::Default);
        // A host allow carrying setMode:'plan' takes effect on the LIVE session.
        gate.apply_permission_update(&serde_json::json!({"type": "setMode", "mode": "plan"}));
        assert_eq!(gate.effective_mode(), PermissionMode::Plan);
    }

    #[test]
    fn apply_permission_update_setmode_bypass_rejected_when_unavailable() {
        // Default policy: bypass NOT available (not launched with the flag) ⇒
        // Xb rejects the setMode with no live change.
        let policy = policy_with(r#"{ "permissions": {} }"#, PermissionMode::Default);
        let gate =
            PolicyPermissionGate::new(policy, RecordingInner::new(PermissionDecision::Allow));
        gate.apply_permission_update(
            &serde_json::json!({"type": "setMode", "mode": "bypassPermissions"}),
        );
        assert_eq!(
            gate.effective_mode(),
            PermissionMode::Default,
            "bypassPermissions must be rejected when not available"
        );
    }

    #[test]
    fn apply_permission_update_setmode_bypass_applied_when_available() {
        let policy = Arc::new(
            PermissionPolicy::from_rules(PermissionMode::Default, Vec::new())
                .with_bypass_available(true),
        );
        let gate =
            PolicyPermissionGate::new(policy, RecordingInner::new(PermissionDecision::Allow));
        gate.apply_permission_update(
            &serde_json::json!({"type": "setMode", "mode": "bypassPermissions"}),
        );
        assert_eq!(gate.effective_mode(), PermissionMode::BypassPermissions);
    }

    #[test]
    fn apply_permission_update_addrules_live_allows_subsequent_calls() {
        let policy = policy_with(r#"{ "permissions": {} }"#, PermissionMode::Default);
        let inner = RecordingInner::new(PermissionDecision::Deny {
            reason: "should not prompt".into(),
        });
        let gate = PolicyPermissionGate::new(policy, inner.clone());
        gate.apply_permission_update(&serde_json::json!({
            "type": "addRules",
            "rules": [{"toolName": "Bash"}],
            "behavior": "allow",
            "destination": "session"
        }));
        let rt = tokio::runtime::Runtime::new().unwrap();
        assert_eq!(
            rt.block_on(gate.check("Bash", &json!({}))),
            PermissionDecision::Allow
        );
        assert_eq!(
            inner.calls(),
            0,
            "live addRules must short-circuit the prompt"
        );
    }

    #[test]
    fn noninteractive_shell_check_reads_live_rules_mode_and_transient_allows() {
        use traits::permission_gate::NonInteractivePermissionDecision;

        let policy = Arc::new(
            PermissionPolicy::from_rules(PermissionMode::Default, Vec::new())
                .with_bypass_available(true),
        );
        let gate =
            PolicyPermissionGate::new(policy, RecordingInner::new(PermissionDecision::Allow));
        let input = json!({"command":"git push origin main"});

        assert!(matches!(
            gate.check_noninteractive_with_allow_rules("Bash", &input, &[]),
            Some(NonInteractivePermissionDecision::Deny { .. })
        ));
        assert_eq!(
            gate.check_noninteractive_with_allow_rules("Bash", &input, &["Bash".into()]),
            Some(NonInteractivePermissionDecision::Allow),
            "frontmatter allow rules are scoped to this check"
        );
        assert!(matches!(
            gate.check_noninteractive_with_allow_rules("Bash", &input, &[]),
            Some(NonInteractivePermissionDecision::Deny { .. })
        ));

        gate.apply_permission_update(&json!({
            "type": "addRules",
            "rules": [{"toolName": "Bash"}],
            "behavior": "allow",
            "destination": "session"
        }));
        assert_eq!(
            gate.check_noninteractive_with_allow_rules("Bash", &input, &[]),
            Some(NonInteractivePermissionDecision::Allow),
            "live updatedPermissions rules must reach prompt-shell expansion"
        );

        gate.apply_permission_update(&json!({
            "type": "replaceRules",
            "rules": [],
            "behavior": "allow",
            "destination": "session"
        }));
        gate.apply_permission_update(&json!({
            "type": "setMode",
            "mode": "bypassPermissions"
        }));
        assert_eq!(
            gate.check_noninteractive_with_allow_rules("Bash", &input, &[]),
            Some(NonInteractivePermissionDecision::Allow),
            "live mode changes must reach prompt-shell expansion"
        );
    }

    #[test]
    fn read_deny_search_globs_follow_live_rule_updates() {
        let policy = policy_with_roots(r#"{ "permissions": {} }"#, PermissionMode::Default);
        let gate =
            PolicyPermissionGate::new(policy, RecordingInner::new(PermissionDecision::Allow));

        assert_eq!(
            gate.read_deny_exclude_globs(std::path::Path::new("/proj")),
            Some(Vec::new())
        );
        gate.apply_permission_update(&json!({
            "type": "addRules",
            "rules": [{"toolName": "Read", "ruleContent": "/secrets/**"}],
            "behavior": "deny",
            "destination": "projectSettings"
        }));
        assert_eq!(
            gate.read_deny_exclude_globs(std::path::Path::new("/proj")),
            Some(vec!["/secrets/**".to_string()]),
            "Glob/Grep must see a live Read deny without rebuilding tools"
        );
        gate.apply_permission_update(&json!({
            "type": "removeRules",
            "rules": [{"toolName": "Read", "ruleContent": "/secrets/**"}],
            "behavior": "deny",
            "destination": "projectSettings"
        }));
        assert_eq!(
            gate.read_deny_exclude_globs(std::path::Path::new("/proj")),
            Some(Vec::new()),
            "removing a live deny must restore search visibility"
        );
    }

    #[test]
    fn apply_permission_updates_folds_over_array() {
        let policy = policy_with(r#"{ "permissions": {} }"#, PermissionMode::Default);
        let gate =
            PolicyPermissionGate::new(policy, RecordingInner::new(PermissionDecision::Allow));
        gate.apply_permission_updates(&[
            serde_json::json!({"type": "addRules", "rules": [], "behavior": "allow", "destination": "session"}),
            serde_json::json!({"type": "setMode", "mode": "acceptEdits"}),
        ]);
        assert_eq!(gate.effective_mode(), PermissionMode::AcceptEdits);
    }

    #[test]
    fn apply_permission_update_replacerules_live_replaces_bucket() {
        let policy = policy_with(
            r#"{ "permissions": { "allow": ["Bash"] } }"#,
            PermissionMode::Default,
        );
        let inner = RecordingInner::new(PermissionDecision::Deny {
            reason: "replaced bucket should no longer allow".into(),
        });
        let gate = PolicyPermissionGate::new(policy, inner.clone());
        gate.apply_permission_update(&json!({
            "type": "replaceRules",
            "rules": [{"toolName": "Read"}],
            "behavior": "allow",
            "destination": "userSettings"
        }));
        let rt = tokio::runtime::Runtime::new().unwrap();
        assert_eq!(
            rt.block_on(gate.check("Bash", &json!({}))),
            PermissionDecision::Deny {
                reason: "replaced bucket should no longer allow".into()
            }
        );
        assert_eq!(inner.calls(), 1);
    }

    #[test]
    fn apply_permission_update_removerules_live_removes_matching_rule() {
        let policy = policy_with(
            r#"{ "permissions": { "allow": ["Bash"] } }"#,
            PermissionMode::Default,
        );
        let inner = RecordingInner::new(PermissionDecision::Deny {
            reason: "removed rule should fall through".into(),
        });
        let gate = PolicyPermissionGate::new(policy, inner.clone());
        gate.apply_permission_update(&json!({
            "type": "removeRules",
            "rules": [{"toolName": "Bash"}],
            "behavior": "allow",
            "destination": "userSettings"
        }));
        let rt = tokio::runtime::Runtime::new().unwrap();
        assert_eq!(
            rt.block_on(gate.check("Bash", &json!({}))),
            PermissionDecision::Deny {
                reason: "removed rule should fall through".into()
            }
        );
        assert_eq!(inner.calls(), 1);
    }

    #[test]
    fn apply_permission_update_directories_live_change_working_dir_allowance() {
        let policy = policy_with_roots(r#"{ "permissions": {} }"#, PermissionMode::AcceptEdits);
        let gate =
            PolicyPermissionGate::new(policy, RecordingInner::new(PermissionDecision::Allow));
        let outside = json!({"file_path":"/extra/file.txt"});

        let (_, before) = gate.effective_authorize("Edit", &outside);
        assert!(matches!(before, PermissionResult::Ask { .. }));

        gate.apply_permission_update(&json!({
            "type": "addDirectories",
            "directories": ["/extra"],
            "destination": "session"
        }));
        let (_, during) = gate.effective_authorize("Edit", &outside);
        assert!(matches!(during, PermissionResult::Allow { .. }));

        gate.apply_permission_update(&json!({
            "type": "removeDirectories",
            "directories": ["/extra"],
            "destination": "session"
        }));
        let (_, after) = gate.effective_authorize("Edit", &outside);
        assert!(matches!(after, PermissionResult::Ask { .. }));
    }

    #[tokio::test]
    async fn per_call_mode_override_still_enforces_live_rules() {
        let policy = Arc::new(
            PermissionPolicy::from_rules(PermissionMode::Default, Vec::new())
                .with_bypass_available(true),
        );
        let gate =
            PolicyPermissionGate::new(policy, RecordingInner::new(PermissionDecision::Allow));
        gate.apply_permission_update(&json!({
            "type": "addRules",
            "rules": [{"toolName": "Bash"}],
            "behavior": "deny",
            "destination": "session"
        }));

        let outcome = gate
            .check_with_context(
                "Bash",
                &json!({"command": "echo unsafe"}),
                &PermissionCheckContext {
                    mode_override: Some("bypassPermissions".into()),
                    ..PermissionCheckContext::default()
                },
            )
            .await;
        assert!(
            matches!(outcome, PermissionOutcome::Deny { .. }),
            "a worker mode override must not bypass a live deny rule"
        );
    }

    #[tokio::test]
    async fn plan_mode_and_catalog_queries_read_live_rules() {
        let policy = policy_with(r#"{ "permissions": {} }"#, PermissionMode::Default);
        let inner = RecordingInner::new(PermissionDecision::Deny {
            reason: "plan should honor the live allow".into(),
        });
        let gate = PolicyPermissionGate::new(policy, inner.clone());
        gate.apply_permission_updates(&[
            json!({
                "type": "addRules",
                "rules": [{"toolName": "Bash"}],
                "behavior": "allow",
                "destination": "session"
            }),
            json!({
                "type": "addRules",
                "rules": [
                    {"toolName": "Write"},
                    {"toolName": "Task", "ruleContent": "reviewer"}
                ],
                "behavior": "deny",
                "destination": "session"
            }),
        ]);

        assert_eq!(
            gate.check_in_plan_mode("Bash", &json!({"command": "echo ok"}))
                .await,
            PermissionDecision::Allow
        );
        assert_eq!(inner.calls(), 0);
        assert!(gate.tool_wide_deny_names().await.contains(&"Write".into()));
        assert_eq!(
            gate.agent_type_deny("reviewer").await.as_deref(),
            Some("session")
        );
        assert!(gate
            .agent_deny_content_types()
            .await
            .contains(&"reviewer".into()));
    }

    #[tokio::test]
    async fn malformed_or_managed_live_updates_are_ignored_atomically() {
        let policy = policy_with(
            r#"{ "permissions": { "allow": ["Bash"] } }"#,
            PermissionMode::Default,
        );
        let gate = PolicyPermissionGate::new(
            policy,
            RecordingInner::new(PermissionDecision::Deny {
                reason: "boot allow was unexpectedly removed".into(),
            }),
        );

        gate.apply_permission_update(&json!({
            "type": "replaceRules",
            "rules": [{"toolName": "Read"}, {"ruleContent": "missing tool"}],
            "behavior": "allow",
            "destination": "userSettings"
        }));
        gate.apply_permission_update(&json!({
            "type": "replaceRules",
            "rules": [],
            "behavior": "allow",
            "destination": "policySettings"
        }));

        assert_eq!(
            gate.check("Bash", &json!({"command": "echo ok"})).await,
            PermissionDecision::Allow,
            "invalid updates must not partially replace a valid rule bucket"
        );
    }
}

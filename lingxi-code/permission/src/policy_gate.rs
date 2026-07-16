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
//! bypass; subagent + teammate tool calls consult this same gate).
//!
//! ## `Plan` mode (Batch 3)
//! `Plan` mode now imposes a mutation backstop in
//! [`PermissionPolicy::authorize`]: a tool that is NOT on the read-only /
//! planning-safe allowlist ([`crate::mode_policy::is_plan_safe_tool`]) and that
//! matched no allow rule returns `Ask` tagged with `Plan`. That ask is a
//! `DenyByDefault` outcome for mutating tools (`Edit`/`Write`/`Bash`/…), so it
//! is DELEGATED to the inner prompt transport here — it is NOT short-circuited
//! to auto-allow. Plan-safe READ-ONLY tools (`Read`/`Grep`/`Glob`/`LSP`/…) do
//! NOT trip the backstop; they fall through to the generic mode-fallback ask
//! and, being [`PromptDefault::AllowByDefault`], are AUTO-ALLOWED here without a
//! prompt — so plan mode keeps read access frictionless while still gating
//! mutations. (`AcceptEdits`' edit auto-allow is a separate batch.) This gate
//! is built at boot only behind an OPT-IN toggle; the default remains the
//! always-allow `NoOpPermissionGate`.

use crate::classifier::{classify_tool_call, reason_allows_classifier, AutoModeClassifierVerdict};
use crate::defaults_per_tool::tool_default;
use crate::gate::{
    PermissionCheckContext, PermissionDecision, PermissionDecisionSource, PermissionGate,
    PermissionOutcome, PermissionResolution, PromptDefault,
};
use crate::mode::PermissionMode;
use crate::policy::PermissionPolicy;
use crate::result::{
    ClassifierKind, PermissionDecisionReason, PermissionMetadata, PermissionResult,
};
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
    /// LIVE mode override set by [`PermissionGate::set_permission_mode`]
    /// (the `set_permission_mode` control_request). `None` ⇒ use the policy's
    /// boot mode (`authorize`); `Some(mode)` ⇒ authorize under that mode on every
    /// check, mirroring claude-code reading `toolPermissionContext.mode` LIVE.
    /// Read briefly per check (the value is `Copy`, never held across an `await`).
    mode_override: std::sync::RwLock<Option<PermissionMode>>,
}

impl PolicyPermissionGate {
    /// Wrap `policy` with `inner` as the `Ask`-delegation prompt transport.
    #[must_use]
    pub fn new(policy: Arc<PermissionPolicy>, inner: Arc<dyn PermissionGate>) -> Self {
        Self {
            policy,
            inner,
            mode_override: std::sync::RwLock::new(None),
        }
    }

    /// Authorize under the LIVE mode: the `set_permission_mode` override when
    /// set, else the policy's boot mode. Shared by every non-plan check path so a
    /// runtime mode change takes effect everywhere at once.
    fn effective_mode(&self) -> PermissionMode {
        self.mode_override
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .unwrap_or(self.policy.mode)
    }

    fn effective_authorize(&self, name: &str, input: &Value) -> (PermissionMode, PermissionResult) {
        let mode = self.effective_mode();
        (mode, self.policy.authorize_with_mode(name, input, mode))
    }

    /// Map a 3-valued [`PermissionResult`] onto the 2-valued
    /// [`PermissionDecision`] the orchestrator consumes: `Allow`/`Deny` pass
    /// through (the deny reason rendered), and an `Ask` either AUTO-ALLOWS a
    /// read-only / agent-local tool ([`PromptDefault::AllowByDefault`]) or
    /// DELEGATES to the inner prompt transport. Shared by [`Self::check`] (boot
    /// mode) and [`PermissionGate::check_in_plan_mode`] (live Plan mode) so an
    /// `Ask` is mapped identically regardless of which mode produced it.
    async fn decide(
        &self,
        mode: PermissionMode,
        result: PermissionResult,
        name: &str,
        input: &Value,
    ) -> PermissionDecision {
        // No worker attribution (main turn loop / plan-mode path).
        self.decide_with_worker(mode, result, name, input, None)
            .await
    }

    /// Like [`Self::decide`], but forwards the originating-worker identity to the
    /// inner prompt transport on the Ask-delegate path so the prompt is
    /// attributed (claude-code's worker permission badge). `None` → identical to
    /// [`Self::decide`].
    async fn decide_with_worker(
        &self,
        mode: PermissionMode,
        result: PermissionResult,
        name: &str,
        input: &Value,
        worker: Option<crate::gate::PromptWorker>,
    ) -> PermissionDecision {
        match result {
            PermissionResult::Allow { .. } => {
                self.record_auto_mode_non_deny(mode);
                PermissionDecision::Allow
            }
            PermissionResult::Deny {
                reason,
                explanation,
                ..
            } => PermissionDecision::Deny {
                reason: explanation.unwrap_or_else(|| deny_reason_string(&reason, name)),
            },
            PermissionResult::Ask { ref reason, .. } => {
                if let Some(classified) =
                    self.auto_mode_classifier_result(mode, reason, name, input)
                {
                    return self.classified_result_to_decision(classified, name);
                }
                if read_only_default_auto_allows(name, reason) {
                    // Read-only / agent-local tool with NO explicit `ask` rule —
                    // auto-allow rather than ask-storm (phase-2 stand-in for the
                    // per-tool default). An explicit `ask` rule (tool-wide or
                    // content) tags the ask `MatchedRule` and is NOT short-circuited
                    // here — it falls through to the prompt transport below.
                    PermissionDecision::Allow
                } else {
                    // Surface the prompt through the host's transport, carrying
                    // the worker identity so it is attributed in the dialog.
                    let decision = self.inner.check_with_worker(name, input, worker).await;
                    if matches!(decision, PermissionDecision::Allow) {
                        self.record_auto_mode_non_deny(mode);
                    }
                    decision
                }
            }
        }
    }

    /// Like [`Self::decide_with_worker`] but returns a [`PermissionOutcome`] that
    /// can carry the host/policy `updatedInput` rewrite, forwarding the full
    /// [`PermissionCheckContext`] (real `tool_use_id`, decision reason) to the
    /// inner transport on the Ask-delegate path. Used by the tool-dispatch
    /// `check_with_context` seam.
    async fn decide_outcome_with_context(
        &self,
        mode: PermissionMode,
        result: PermissionResult,
        name: &str,
        input: &Value,
        ctx: &PermissionCheckContext,
    ) -> PermissionOutcome {
        match result {
            // A policy-rule allow may itself carry a rewritten input — surface it
            // (previously dropped at the `PermissionDecision::Allow` boundary). The
            // local policy gate never derives `updatedPermissions` rule updates
            // (those originate from the stdio host's `can_use_tool` response), so
            // `permission_updates` is always empty on this path.
            PermissionResult::Allow { updated_input, .. } => {
                self.record_auto_mode_non_deny(mode);
                PermissionOutcome::Allow {
                    updated_input,
                    permission_updates: Vec::new(),
                }
            }
            PermissionResult::Deny {
                reason,
                explanation,
                ..
            } => PermissionOutcome::Deny {
                reason: explanation.unwrap_or_else(|| deny_reason_string(&reason, name)),
            },
            PermissionResult::Ask { ref reason, .. } => {
                if let Some(classified) =
                    self.auto_mode_classifier_result(mode, reason, name, input)
                {
                    return self.classified_result_to_outcome(classified, name);
                }
                if read_only_default_auto_allows(name, reason) {
                    self.record_auto_mode_non_deny(mode);
                    PermissionOutcome::Allow {
                        updated_input: None,
                        permission_updates: Vec::new(),
                    }
                } else {
                    // Delegate to the inner transport WITH the context so a stdio
                    // `can_use_tool` request carries the real tool_use_id and its
                    // allow can return the host's `updatedInput`. Enrich the ctx
                    // here (where the policy `reason` is in scope) with the
                    // serialized `decision_reason` — claude-code
                    // `createCanUseTool` sets `decision_reason:
                    // serializeDecisionReason(mainPermissionResult.decisionReason)`.
                    // The turn loop builds the ctx with only `tool_use_id`; we add
                    // the reason without touching the unit `PermissionResolution::Ask`.
                    // (permission_suggestions/blocked_path stay `None` — LingXi's
                    // policy Ask does not model claude-code's
                    // PermissionAskDecision.suggestions/blockedPath; see the
                    // PermissionCheckContext field docs.)
                    let ctx2 = PermissionCheckContext {
                        decision_reason: serialize_decision_reason(reason),
                        ..ctx.clone()
                    };
                    let outcome = self.inner.check_with_context(name, input, &ctx2).await;
                    if matches!(outcome, PermissionOutcome::Allow { .. }) {
                        self.record_auto_mode_non_deny(mode);
                    }
                    outcome
                }
            }
        }
    }

    /// Like [`Self::decide`] but WITHOUT consulting the inner prompt transport:
    /// returns a [`PermissionResolution`] that carries the deny SOURCE and, for a
    /// would-be prompt, an [`PermissionResolution::Ask`] instead of resolving it.
    /// The turn loop uses this to fire the source-gated permission hooks
    /// (`PermissionRequest` on `Ask`, `PermissionDenied` on a classifier `Deny`)
    /// before delegating to the transport. See [`PermissionGate::resolve_detailed`].
    fn resolve_with_mode(
        &self,
        mode: PermissionMode,
        result: PermissionResult,
        name: &str,
        input: &Value,
    ) -> PermissionResolution {
        match result {
            PermissionResult::Allow { .. } => PermissionResolution::Allow,
            PermissionResult::Deny {
                reason,
                explanation,
                ..
            } => PermissionResolution::Deny {
                source: map_decision_source(&reason),
                reason: explanation.unwrap_or_else(|| deny_reason_string(&reason, name)),
                // The policy gate's rule/mode/classifier denials are never an
                // `ask`-behavior rejection carrying contentBlocks (the external
                // build has no classifier/contentBlocks producer), so the
                // top-level-image path stays dormant — byte-identical to before.
                behavior_ask: false,
                content_blocks: Vec::new(),
            },
            PermissionResult::Ask { ref reason, .. } => {
                if let Some(classified) =
                    self.auto_mode_classifier_result(mode, reason, name, input)
                {
                    return self.resolve_with_mode(mode, classified, name, input);
                }
                if read_only_default_auto_allows(name, reason) {
                    // Read-only / agent-local tool with NO explicit `ask` rule —
                    // auto-allowed, no prompt.
                    PermissionResolution::Allow
                } else {
                    // A would-be prompt (a mutating tool, OR an explicit `ask`
                    // rule on a read-only tool): the turn loop fires
                    // PermissionRequest before this is delegated to the transport.
                    PermissionResolution::Ask
                }
            }
        }
    }

    fn auto_mode_classifier_result(
        &self,
        mode: PermissionMode,
        reason: &PermissionDecisionReason,
        name: &str,
        input: &Value,
    ) -> Option<PermissionResult> {
        if mode != PermissionMode::Auto
            || !crate::classifier::is_classifier_permissions_enabled()
            || !reason_allows_classifier(reason)
        {
            return None;
        }
        {
            let tracking = self
                .policy
                .denial_tracking
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if tracking.is_circuit_broken() {
                return None;
            }
        }
        match classify_tool_call(name, input) {
            AutoModeClassifierVerdict::Allow { score, .. } => {
                self.record_auto_mode_non_deny(mode);
                Some(PermissionResult::Allow {
                    reason: PermissionDecisionReason::ClassifierApproved {
                        classifier: ClassifierKind::Transcript,
                        score,
                    },
                    updated_input: None,
                    update_destination: None,
                    metadata: PermissionMetadata::default(),
                })
            }
            AutoModeClassifierVerdict::Deny { score, reason, .. } => {
                let mut tracking = self
                    .policy
                    .denial_tracking
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                tracking.record_auto_deny();
                if tracking.trip(false).is_some() {
                    return None;
                }
                Some(PermissionResult::Deny {
                    reason: PermissionDecisionReason::ClassifierRejected {
                        classifier: ClassifierKind::Transcript,
                        score,
                    },
                    explanation: Some(format!("Auto mode classifier blocked action: {reason}")),
                    metadata: PermissionMetadata::default(),
                })
            }
            AutoModeClassifierVerdict::Pass { .. } => None,
        }
    }

    fn classified_result_to_decision(
        &self,
        result: PermissionResult,
        name: &str,
    ) -> PermissionDecision {
        match result {
            PermissionResult::Allow { .. } => PermissionDecision::Allow,
            PermissionResult::Deny {
                reason,
                explanation,
                ..
            } => PermissionDecision::Deny {
                reason: explanation.unwrap_or_else(|| deny_reason_string(&reason, name)),
            },
            PermissionResult::Ask { .. } => PermissionDecision::Deny {
                reason: format!("Permission to use {name} has been denied."),
            },
        }
    }

    fn classified_result_to_outcome(
        &self,
        result: PermissionResult,
        name: &str,
    ) -> PermissionOutcome {
        match result {
            PermissionResult::Allow { updated_input, .. } => PermissionOutcome::Allow {
                updated_input,
                permission_updates: Vec::new(),
            },
            PermissionResult::Deny {
                reason,
                explanation,
                ..
            } => PermissionOutcome::Deny {
                reason: explanation.unwrap_or_else(|| deny_reason_string(&reason, name)),
            },
            PermissionResult::Ask { .. } => PermissionOutcome::Deny {
                reason: format!("Permission to use {name} has been denied."),
            },
        }
    }

    fn record_auto_mode_non_deny(&self, mode: PermissionMode) {
        if mode == PermissionMode::Auto {
            self.policy
                .denial_tracking
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .record_non_deny();
        }
    }
}

/// Whether the read-only / agent-local default-allow stand-in applies to an
/// `Ask`. It does ONLY when the tool is [`PromptDefault::AllowByDefault`] AND the
/// ask was NOT produced by an explicit `ask` rule. claude-code's read-only
/// default-allow (`checkPermissions`' allow verdict) is reached only AFTER the
/// ask-rule walk (`mSm` steps 1c/1d precede the per-tool allow), so an explicit
/// `ask:["Read(...)"]` / `ask:["Glob"]` rule — tool-wide or content, tagged
/// [`PermissionDecisionReason::MatchedRule`] — PRE-EMPTS it and forces the prompt
/// (firing the `PermissionRequest` hook). A mode-fallback ask
/// ([`PermissionDecisionReason::PermissionMode`]) keeps the frictionless
/// read-only auto-allow so the common no-rule case never ask-storms.
fn read_only_default_auto_allows(name: &str, reason: &PermissionDecisionReason) -> bool {
    matches!(tool_default(name), PromptDefault::AllowByDefault)
        && !matches!(reason, PermissionDecisionReason::MatchedRule { .. })
}

#[async_trait]
impl PermissionGate for PolicyPermissionGate {
    async fn check(&self, name: &str, input: &Value) -> PermissionDecision {
        // Authorize under the LIVE mode (boot mode or a set_permission_mode
        // override), then map the 3-valued result (an `Ask` auto-allows read-only
        // tools or delegates to the prompt).
        let (mode, result) = self.effective_authorize(name, input);
        self.decide(mode, result, name, input).await
    }

    /// As [`Self::check`], but forwards the originating subagent/teammate
    /// worker identity to the inner prompt transport when an `Ask` is delegated,
    /// so the dialog attributes the request to that worker. The rule/mode
    /// decision is unchanged — only the prompt presentation gains attribution.
    async fn check_with_worker(
        &self,
        name: &str,
        input: &Value,
        worker: Option<crate::gate::PromptWorker>,
    ) -> PermissionDecision {
        let (mode, result) = self.effective_authorize(name, input);
        self.decide_with_worker(mode, result, name, input, worker)
            .await
    }

    /// As [`Self::check_with_worker`], but returns a [`PermissionOutcome`] that
    /// can carry the host/policy `updatedInput` rewrite and forwards the full
    /// context (real `tool_use_id`) to the inner transport on the Ask path.
    async fn check_with_context(
        &self,
        name: &str,
        input: &Value,
        ctx: &PermissionCheckContext,
    ) -> PermissionOutcome {
        // A PER-CALL mode override (a spawned subagent's clamped spawn mode,
        // claude-code 2.1.207 `ve` → the child's `toolPermissionContext.mode`)
        // authorizes THIS call under that mode; else the live/boot mode. Only this
        // dispatch seam reads it, so the shared gate's mode is never mutated (the
        // parent's own checks are unaffected).
        let (mode, result) = match ctx.mode_override.as_deref().and_then(parse_settable_mode) {
            Some(m) => (m, self.policy.authorize_with_mode(name, input, m)),
            None => self.effective_authorize(name, input),
        };
        self.decide_outcome_with_context(mode, result, name, input, ctx)
            .await
    }

    /// A PreToolUse / PermissionRequest hook `allow` skips the PROMPT but still
    /// applies rule-based deny/ask (claude-code `resolveHookPermissionDecision` +
    /// `checkRuleBasedPermissions`): a hook cannot override an explicit deny
    /// rule or the active mode's mutation backstop. So run `authorize` and map
    /// `Deny → Deny` (deny rules + mode still bind) but `Ask → Allow` (the hook
    /// approved, so the would-be prompt is skipped) and `Allow → Allow`. Unlike
    /// `check`, an `Ask` NEVER delegates to the inner prompt transport here — the
    /// hook already resolved the prompt.
    async fn check_after_hook_allow(&self, name: &str, input: &Value) -> PermissionDecision {
        let (mode, result) = self.effective_authorize(name, input);
        match result {
            PermissionResult::Allow { .. } | PermissionResult::Ask { .. } => {
                self.record_auto_mode_non_deny(mode);
                PermissionDecision::Allow
            }
            PermissionResult::Deny {
                reason,
                explanation,
                ..
            } => PermissionDecision::Deny {
                reason: explanation.unwrap_or_else(|| deny_reason_string(&reason, name)),
            },
        }
    }

    /// In PLAN mode the gate authorizes under [`PermissionMode::Plan`] regardless
    /// of the policy's boot mode, so a runtime `EnterPlanMode` dynamically
    /// activates the mutation backstop: a plan-safe READ-ONLY tool falls through
    /// to the read-only auto-allow (no prompt), while a mutating tool that matched
    /// no allow rule trips the backstop → `Ask` → DELEGATES to the inner prompt
    /// transport (an interactive prompt, or a headless deny). Deny rules and
    /// explicit allow rules still bind — they are resolved before the mode layer
    /// inside `authorize_with_mode`. The `Ask` mapping is shared with
    /// [`Self::check`] via [`Self::decide`].
    async fn check_in_plan_mode(&self, name: &str, input: &Value) -> PermissionDecision {
        self.decide(
            PermissionMode::Plan,
            self.policy
                .authorize_with_mode(name, input, PermissionMode::Plan),
            name,
            input,
        )
        .await
    }

    async fn resolve_detailed(&self, name: &str, input: &Value) -> PermissionResolution {
        // Authorize under the LIVE mode WITHOUT delegating to the inner prompt,
        // so the turn loop can read the decision source (and an about-to-ask) and
        // fire PermissionRequest / PermissionDenied before the prompt resolves.
        let (mode, result) = self.effective_authorize(name, input);
        self.resolve_with_mode(mode, result, name, input)
    }

    /// Surface the wrapped policy's TOOL-WIDE deny-rule names so the orchestrator
    /// strips blanket-denied tools from the wire `tools` array before the model
    /// sees them (claude-code `filterToolsByDenyRules`). Content deny rules are
    /// excluded by [`PermissionPolicy::tool_wide_deny_names`] (they deny calls,
    /// not the tool). With zero deny rules this is empty ⇒ no tools stripped.
    async fn tool_wide_deny_names(&self) -> Vec<String> {
        self.policy.tool_wide_deny_names()
    }

    /// Surface the source of a matching `Agent(<type>)` deny rule so the Agent
    /// tool can reject a denied subagent type with the byte-exact
    /// `AgentTypeError` message (claude-code `getDenyRuleForAgent`). The source is
    /// rendered as the raw `SettingSource` identifier
    /// ([`PermissionRuleSource::lingxi_settings_source`]), matching the binary's
    /// `… from ${rule.source}.`.
    async fn agent_type_deny(&self, agent_type: &str) -> Option<String> {
        self.policy
            .agent_type_deny_source(agent_type)
            .map(|s| s.lingxi_settings_source().to_string())
    }

    /// Surface the wrapped policy's CONTENT-ful `Agent(<x>)` deny set so the
    /// advertised agent catalog and `Available agents:` lists exclude denied
    /// types (claude-code `Pxe`).
    async fn agent_deny_content_types(&self) -> Vec<String> {
        self.policy.agent_deny_content_types()
    }

    /// Apply a LIVE `set_permission_mode` override (claude-code
    /// `handleSetPermissionMode`), byte-faithful to the binary's gating:
    ///
    /// - `bypassPermissions` is gated by TWO ordered checks: first the
    ///   disabled-by-settings killswitch, then the launch-flag availability
    ///   (`isBypassPermissionsModeAvailable` / `bypass_permissions_available`) —
    ///   each with its byte-exact error string. So a session NOT launched with
    ///   `--dangerously-skip-permissions` cannot switch live into bypass.
    /// - an UNKNOWN mode is NOT an error: the binary reads the mode raw, acks
    ///   `{mode}`, and `transitionPermissionMode` no-ops an unrecognized mode
    ///   (classifier off), so we ack WITHOUT changing the live mode.
    /// - `auto` is gated by claude-code 2.1.207's `Nle`
    ///   (`setPermissionModeWithGuards`): `if(e==="auto"&&!P0()){...error:
    ///   \`Cannot set permission mode to auto: ${Jce(One())}\`}`. We evaluate the
    ///   two runtime-available `P0()` inputs — the `disableAutoMode` settings
    ///   killswitch ([`crate::PermissionPolicy::auto_mode_disabled`]) and the
    ///   local denial circuit-breaker — and reject with the byte-exact message
    ///   for the [`crate::auto_gate::AutoGateDenialReason::Settings`] /
    ///   [`crate::auto_gate::AutoGateDenialReason::CircuitBreaker`] cases.
    ///
    ///   REMAINDER (documented): `P0()`'s third input, the model gate
    ///   (`dUe(wi())`), is not evaluated at THIS live surface — the policy does
    ///   not carry the active model/provider. The model gate is enforced
    ///   authoritatively at BOOT (the [`crate::auto_gate::apply_auto_mode_gate`]
    ///   mode-load downgrade in the engine boot), so a session on an
    ///   auto-unsupported model boots in `Default` and never reaches this path
    ///   in `Auto`. A live runtime switch to `auto` on an unsupported model is
    ///   the only uncovered case (parity gap: it is accepted here where the
    ///   binary would reject with `auto mode unavailable for this model`).
    async fn set_permission_mode(&self, mode: &str) -> Result<(), String> {
        let Some(parsed) = parse_settable_mode(mode) else {
            // Unknown mode: accept + ack, but do not mutate the live mode.
            return Ok(());
        };
        if parsed == PermissionMode::BypassPermissions {
            if self.policy.bypass_killswitch_active {
                return Err("Cannot set permission mode to bypassPermissions because it is disabled by settings or configuration".to_string());
            }
            if !self.policy.bypass_permissions_available {
                return Err("Cannot set permission mode to bypassPermissions because the session was not launched with --dangerously-skip-permissions".to_string());
            }
        }
        if parsed == PermissionMode::Auto {
            // `Nle`: reject `auto` when `!P0()`. Report `One()`'s reason
            // (settings precedes circuit-breaker), rendering the byte-exact
            // `Cannot set permission mode to auto: <Jce(reason)>`.
            let reason = if self.policy.auto_mode_disabled {
                Some(crate::auto_gate::AutoGateDenialReason::Settings)
            } else if self
                .policy
                .denial_tracking
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_circuit_broken()
            {
                Some(crate::auto_gate::AutoGateDenialReason::CircuitBreaker)
            } else {
                None
            };
            if let Some(reason) = reason {
                return Err(crate::auto_gate::cannot_set_auto_message(reason));
            }
        }
        *self
            .mode_override
            .write()
            .unwrap_or_else(|e| e.into_inner()) = Some(parsed);
        Ok(())
    }
}

/// Parse a `set_permission_mode` wire string into a [`PermissionMode`].
///
/// Accepts the five external modes plus the internal `auto` (the binary's
/// settable set is `default`/`plan`/`acceptEdits`/`bypassPermissions`/`dontAsk`/
/// `auto`), with the 2.1.211 `manual` alias (`LE`/`PERMISSION_MODE_MANUAL_ALIAS`)
/// normalized to `default` — applied in both the wire schema `preprocess` and the
/// `-p` engine handler. Returns `None` for an unrecognized mode — the binary
/// accepts any string and no-ops an unknown one (it does NOT error), so the
/// caller acks without mutating rather than returning an error frame.
fn parse_settable_mode(s: &str) -> Option<PermissionMode> {
    match s {
        "default" | "manual" => Some(PermissionMode::Default),
        "plan" => Some(PermissionMode::Plan),
        "acceptEdits" => Some(PermissionMode::AcceptEdits),
        "bypassPermissions" => Some(PermissionMode::BypassPermissions),
        "dontAsk" => Some(PermissionMode::DontAsk),
        "auto" => Some(PermissionMode::Auto),
        _ => None,
    }
}

/// Render a [`PermissionDecisionReason`] to the model-facing deny string for
/// `tool_name` (used when the `Deny` carries no explicit `explanation`).
///
/// claude-code's generic permission deny message is
/// `Permission to use ${tool} has been denied.` (`permissions.ts:1087,1179`);
/// the `dontAsk` mode appends the don't-ask clause + the shared workaround
/// guidance (`DONT_ASK_REJECT_MESSAGE`, `messages.ts:237`). Tool-specific deny
/// checks (e.g. Bash's `Permission to use Bash with command ${cmd} has been
/// denied.`) set an explicit `explanation` that takes precedence over this.
fn deny_reason_string(reason: &PermissionDecisionReason, tool_name: &str) -> String {
    match reason {
        PermissionDecisionReason::PermissionMode {
            mode: PermissionMode::DontAsk,
        } => format!(
            "Permission to use {tool_name} has been denied because LingXi is running in don't ask mode. {DENIAL_WORKAROUND_GUIDANCE}"
        ),
        _ => format!("Permission to use {tool_name} has been denied."),
    }
}

/// Shared guidance appended to certain permission denials (`messages.ts:226`),
/// instructing the model on acceptable workarounds.
///
/// This is claude-code's `Rws` (`lDd` + the "If you believe this capability is
/// essential…" suffix). `GRu(tool)` — the headless deny message — is
/// `` `Permission to use ${tool} has been denied. ${Rws}` `` (see
/// [`crate::headless_gate::DenyOnAskGate`]).
pub(crate) const DENIAL_WORKAROUND_GUIDANCE: &str = "IMPORTANT: You *may* attempt to accomplish this action using other tools that might naturally be used to accomplish this goal, e.g. using head instead of cat. But you *should not* attempt to work around this denial in malicious ways, e.g. do not use your ability to run tests to execute non-test actions. You should only try to work around this restriction in reasonable ways that do not attempt to bypass the intent behind this denial. If you believe this capability is essential to complete the user's request, STOP and explain to the user what you were trying to do and why you need this permission. Let the user decide how to proceed.";

/// Serialize a [`PermissionDecisionReason`] to the free-text `decision_reason`
/// string a stdio `can_use_tool` control_request carries — 1:1 with claude-code
/// `serializeDecisionReason` (`cli/structuredIO.ts:64-91`).
///
/// The oracle returns `undefined` (⇒ `None`, the key is OMITTED) for the
/// `rule`/`mode`/`subcommandResults`/`permissionPromptTool` reasons (the common
/// ask cases — an SDK host parses `decision_reason_type` for those, not the
/// text), and the reason STRING for `hook`/`asyncAgent`/`sandboxOverride`/
/// `workingDir`/`safetyCheck`/`other` (+ `classifier` only behind the
/// `BASH_CLASSIFIER`/`TRANSCRIPT_CLASSIFIER` feature flags, which are `false` in
/// the external build — so `ClassifierApproved`/`ClassifierRejected` map to
/// `None` here, matching the gated-off posture used elsewhere in this crate).
///
/// Field-shape notes vs claude-code:
/// - [`PermissionDecisionReason::HookOverride`] carries `reason: Option<String>`
///   (not a bare string): `Some(r)` ⇒ that text, `None` ⇒ `None` (the hook
///   supplied no reason, so there is nothing to serialize).
/// - [`PermissionDecisionReason::SandboxOverride`] carries a
///   [`SandboxOverrideReason`] ENUM, not a free string. claude-code's
///   `sandboxOverride` reason is itself a string; rather than fabricate a
///   rendering, this returns `None` (the override is an allow-side reason that
///   does not reach the ask path in the external build anyway).
fn serialize_decision_reason(reason: &PermissionDecisionReason) -> Option<String> {
    match reason {
        // Oracle: rule/mode/subcommandResults/permissionPromptTool ⇒ undefined.
        PermissionDecisionReason::MatchedRule { .. }
        | PermissionDecisionReason::PermissionMode { .. }
        | PermissionDecisionReason::SubcommandResults { .. }
        | PermissionDecisionReason::PermissionPromptTool { .. } => None,
        // Oracle: hook/asyncAgent/workingDir/safetyCheck/other ⇒ reason string.
        PermissionDecisionReason::HookOverride { reason, .. } => reason.clone(),
        PermissionDecisionReason::AsyncAgent { reason }
        | PermissionDecisionReason::WorkingDirectory { reason }
        | PermissionDecisionReason::SafetyCheck { reason, .. }
        | PermissionDecisionReason::Other { reason } => Some(reason.clone()),
        // sandboxOverride: enum reason, no faithful string rendering → None
        // (see fn doc); classifier only behind a feature flag that is off in the
        // external build → None.
        PermissionDecisionReason::SandboxOverride { .. }
        | PermissionDecisionReason::ClassifierApproved { .. }
        | PermissionDecisionReason::ClassifierRejected { .. }
        | PermissionDecisionReason::DenialLimitExceeded
        | PermissionDecisionReason::AutoModeFallback
        | PermissionDecisionReason::BypassPermissions => None,
    }
}

/// Map a [`PermissionDecisionReason`] to the coarse [`PermissionDecisionSource`]
/// the turn loop gates its permission hooks on (claude-code `decisionReason.type`).
/// Only the classifier source unblocks the `PermissionDenied` hook.
fn map_decision_source(reason: &PermissionDecisionReason) -> PermissionDecisionSource {
    match reason {
        PermissionDecisionReason::MatchedRule { .. } => PermissionDecisionSource::Rule,
        PermissionDecisionReason::PermissionMode { .. } => PermissionDecisionSource::Mode,
        PermissionDecisionReason::ClassifierRejected { .. } => PermissionDecisionSource::Classifier,
        _ => PermissionDecisionSource::Unspecified,
    }
}

#[cfg(test)]
#[path = "policy_gate_test.rs"]
mod policy_gate_test;

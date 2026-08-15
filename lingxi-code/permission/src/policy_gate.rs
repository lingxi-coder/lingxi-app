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
    MatchedAskRule, PermissionAbort, PermissionCheckContext, PermissionDecision,
    PermissionDecisionSource, PermissionGate, PermissionOutcome, PermissionResolution,
    PromptDefault,
};
use crate::mode::PermissionMode;
use crate::policy::PermissionPolicy;
use crate::result::{
    ClassifierKind, PermissionDecisionReason, PermissionMetadata, PermissionResult,
};
use crate::rule::{PermissionBehavior, PermissionRule, PermissionRuleSource, PermissionRuleValue};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// Live model/provider inputs read by the auto-mode permission gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveModelContext {
    /// Canonical model id (`wi()` / `getModel()`).
    pub model: String,
    /// Claude provider tag (`"firstParty"`, `"anthropicAws"`, or another
    /// non-first-party tag) used by `dUe`.
    pub provider: String,
}

/// A source of the LIVE main-loop model and provider, read by the auto
/// `set_permission_mode` gate to evaluate `One()` against the route the session
/// is CURRENTLY on. Mirrors `agent::DefaultModelProvider`: returns `None` when
/// the value cannot be read without blocking (a contended session lock), so the
/// caller fails OPEN rather than stall the control request.
pub type LiveModelProvider = Arc<dyn Fn() -> Option<LiveModelContext> + Send + Sync>;

/// Result of the optional auto-mode classifier pass. The interactive breaker
/// fallback carries its rewritten reason forward to the prompt transport;
/// keeping it in this value avoids a shared mutable side-channel.
enum AutoModeClassifierResult {
    NoDecision,
    Classified(PermissionResult),
    PromptFallback { decision_reason: String },
}

#[derive(Debug, Clone)]
struct LivePermissionState {
    allow_rules: HashMap<PermissionRuleSource, Vec<PermissionRule>>,
    deny_rules: HashMap<PermissionRuleSource, Vec<PermissionRule>>,
    ask_rules: HashMap<PermissionRuleSource, Vec<PermissionRule>>,
    additional_working_dirs: Vec<PathBuf>,
}

impl LivePermissionState {
    fn from_policy(policy: &PermissionPolicy) -> Self {
        Self {
            allow_rules: policy.allow_rules.clone(),
            deny_rules: policy.deny_rules.clone(),
            ask_rules: policy.ask_rules.clone(),
            additional_working_dirs: policy.additional_working_dirs.clone(),
        }
    }
}

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
    /// LIVE tighten-only per-MCP-server permission-mode overrides from the
    /// control channel (`set_mcp_permission_mode_override`). Keys are the
    /// normalized MCP server token used in `mcp__<server>__<tool>` names.
    mcp_mode_overrides: std::sync::RwLock<std::collections::HashMap<String, PermissionMode>>,
    /// LIVE main-loop model source (claude-code `wi()`), filled post-orchestrator
    /// via [`Self::live_model_provider_handle`] so the auto `set_permission_mode`
    /// gate can evaluate `One()`'s model reason (`dUe(wi())`) against the model
    /// the session is CURRENTLY on (mutated by `/model` switches / resume), not
    /// the boot snapshot. Empty ⇒ the live model gate is skipped (the boot gate
    /// already downgraded an auto-unsupported boot model, so a fresh session
    /// never reaches this path in `Auto`); a contended read returns `None` and
    /// likewise skips (fail-open, matching every other post-orch live cell).
    live_model_provider: Arc<std::sync::OnceLock<LiveModelProvider>>,
    /// Live session rule/directory state after applying host
    /// `updatedPermissions`. Starts as a clone of the boot policy's mutable
    /// permission state, then diverges only through
    /// [`Self::apply_permission_update`].
    live_state: std::sync::RwLock<LivePermissionState>,
}

impl PolicyPermissionGate {
    fn parse_update_destination(value: &Value) -> Option<PermissionRuleSource> {
        match value.as_str()? {
            "userSettings" => Some(PermissionRuleSource::UserSettings),
            "projectSettings" => Some(PermissionRuleSource::ProjectSettings),
            "localSettings" => Some(PermissionRuleSource::LocalSettings),
            "cliArg" => Some(PermissionRuleSource::CliArg),
            "session" => Some(PermissionRuleSource::Session),
            _ => None,
        }
    }

    fn parse_update_behavior(value: &Value) -> Option<PermissionBehavior> {
        match value.as_str()? {
            "allow" => Some(PermissionBehavior::Allow),
            "deny" => Some(PermissionBehavior::Deny),
            "ask" => Some(PermissionBehavior::Ask),
            _ => None,
        }
    }

    fn parse_update_rules(
        value: &Value,
        behavior: PermissionBehavior,
        source: PermissionRuleSource,
    ) -> Option<Vec<PermissionRule>> {
        value
            .as_array()?
            .iter()
            .map(|rule| {
                let object = rule.as_object()?;
                let tool_name = object.get("toolName").and_then(Value::as_str)?;
                let rule_content = match object.get("ruleContent") {
                    None => None,
                    Some(Value::String(content)) => Some(content.clone()),
                    Some(_) => return None,
                };
                Some(PermissionRule {
                    value: PermissionRuleValue {
                        tool_name: crate::rule::normalize_legacy_tool_name(tool_name),
                        rule_content,
                    },
                    behavior,
                    source,
                })
            })
            .collect()
    }

    fn rule_bucket_mut(
        live: &mut LivePermissionState,
        behavior: PermissionBehavior,
    ) -> &mut HashMap<PermissionRuleSource, Vec<PermissionRule>> {
        match behavior {
            PermissionBehavior::Allow => &mut live.allow_rules,
            PermissionBehavior::Deny => &mut live.deny_rules,
            PermissionBehavior::Ask => &mut live.ask_rules,
        }
    }

    /// Wrap `policy` with `inner` as the `Ask`-delegation prompt transport.
    #[must_use]
    pub fn new(policy: Arc<PermissionPolicy>, inner: Arc<dyn PermissionGate>) -> Self {
        inner.set_permission_persistence_enabled(!policy.allow_managed_permission_rules_only);
        Self {
            live_state: std::sync::RwLock::new(LivePermissionState::from_policy(&policy)),
            policy,
            inner,
            mode_override: std::sync::RwLock::new(None),
            mcp_mode_overrides: std::sync::RwLock::new(std::collections::HashMap::new()),
            live_model_provider: Arc::new(std::sync::OnceLock::new()),
        }
    }

    /// Handle to the set-once LIVE-model cell (cycle-break): the composition root
    /// grabs this BEFORE coercing the gate to `Arc<dyn PermissionGate>`, then
    /// fills it once the orchestrator (owner of the live `session.model`) exists,
    /// so the auto `set_permission_mode` gate reads the CURRENT model. Same
    /// pattern as `agent::OrchestratorHandle::default_model_provider_handle`.
    #[must_use]
    pub fn live_model_provider_handle(&self) -> Arc<std::sync::OnceLock<LiveModelProvider>> {
        Arc::clone(&self.live_model_provider)
    }

    /// `dUe(wi())` at the LIVE `set_permission_mode` surface: does the model the
    /// session is CURRENTLY on FAIL the auto-mode model gate? Reads the live-model
    /// provider ([`Self::live_model_provider_handle`]) and returns `false`
    /// (fail-open, no rejection) when the provider is unset or the model cannot be
    /// read without blocking.
    fn live_model_unsupported_for_auto(&self) -> bool {
        let Some(provider) = self.live_model_provider.get() else {
            return false;
        };
        let Some(context) = provider() else {
            return false;
        };
        !crate::auto_gate::model_supports_auto_mode(&context.model, &context.provider)
    }

    /// `One()` — the live reason auto mode is unavailable, in the oracle's
    /// user-facing precedence. Shared by the session-wide mode switch and the
    /// per-MCP-server auto pin so neither control surface can bypass the other.
    fn auto_mode_denial_reason(&self) -> Option<crate::auto_gate::AutoGateDenialReason> {
        if self.policy.auto_mode_disabled {
            Some(crate::auto_gate::AutoGateDenialReason::Settings)
        } else if self
            .policy
            .denial_tracking
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_circuit_broken()
        {
            Some(crate::auto_gate::AutoGateDenialReason::CircuitBreaker)
        } else if self.live_model_unsupported_for_auto() {
            Some(crate::auto_gate::AutoGateDenialReason::Model)
        } else {
            None
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

    /// Effective mode for one specific tool, including the tighten-only
    /// per-MCP-server override used by Claude's control channel. The override is
    /// only consulted on MCP tools and only when the session-wide mode is one of
    /// the modes Claude lets the per-server pin narrow: `bypassPermissions`,
    /// `auto`, or `plan` while bypass remains available.
    fn effective_mode_for_tool(&self, tool_name: &str) -> PermissionMode {
        let base = self.effective_mode();
        let Some(server) = mcp_server_token(tool_name) else {
            return base;
        };
        let should_apply = matches!(
            base,
            PermissionMode::BypassPermissions | PermissionMode::Auto
        ) || (base == PermissionMode::Plan
            && self.policy.bypass_permissions_available
            && !self.policy.bypass_killswitch_active);
        if !should_apply {
            return base;
        }
        self.mcp_mode_overrides
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(server)
            .copied()
            .unwrap_or(base)
    }

    fn effective_authorize(&self, name: &str, input: &Value) -> (PermissionMode, PermissionResult) {
        self.effective_authorize_with_lease(name, input, None)
    }

    fn effective_authorize_with_lease(
        &self,
        name: &str,
        input: &Value,
        workspace_lease_token: Option<u64>,
    ) -> (PermissionMode, PermissionResult) {
        let mode = self.effective_mode_for_tool(name);
        (mode, self.authorize_with_live_state(name, input, mode, workspace_lease_token))
    }

    /// claude-code `_pt` — the RULE + SAFETY verdict for a call, with **NO MODE
    /// BACKSTOP**.
    ///
    /// `_pt` walks deny rules → allow rules → the tool's own `checkPermissions`
    /// → ask rules → `requiresUserInteraction` → mcp `effectiveMaxPermission`,
    /// and returns **`null`** when none of them decides. It never consults the
    /// permission MODE — so an ordinary "Default mode, no matching rule,
    /// mutating tool" call yields NO verdict, and a caller resolving a hook
    /// `allow` lets that allow stand.
    ///
    /// This port folds rules + safety + mode into one `authorize_with_mode`, so
    /// the mode layer is SUBTRACTED here: a result whose reason is
    /// [`PermissionDecisionReason::PermissionMode`] is exactly what the mode tail
    /// produced (`ask_with_mode` / `allow_with_mode` / the plan-mutation ask) and
    /// therefore corresponds to `_pt`'s `null`. Everything else — deny rules, ask
    /// rules, and the safety walks (dangerous-removal, path constraints, sed,
    /// background-operator) — is a genuine rule/safety verdict and is returned.
    ///
    /// The evaluation runs under a NEUTRAL [`PermissionMode::Default`] rather
    /// than the session's mode. `_pt` consults no mode at all, and every
    /// mode-specific tail transform would otherwise masquerade as a rule verdict:
    /// `DontAsk` rewrites a surviving ask into a mode-tagged **deny**
    /// (`policy.rs:477`, the only `deny_with_mode` site), plan adds its mutation
    /// backstop, and `bypassPermissions`/`acceptEdits` add allows. Under
    /// `Default` none of those fire, and the remaining mode fallback
    /// (`ask_with_mode(Default)`) is filtered out below — leaving exactly the
    /// rule + safety walks. The session's real mode is still returned for
    /// auto-mode bookkeeping.
    /// Shared impl behind [`PermissionGate::check_after_hook_allow`] and its
    /// rich-outcome counterpart. The `lin` re-check of a
    /// hook `allow`: deny rule → deny; ask rule/safety → delegate to the inner
    /// transport (prompt / headless deny) carrying the dispatch context (real
    /// tool_use_id) enriched with the ask's serialized `decision_reason`, so the
    /// stdio `can_use_tool` is byte-faithful (was a fresh UUID + no reason); no
    /// verdict → the hook's allow stands.
    async fn check_after_hook_allow_impl(
        &self,
        name: &str,
        input: &Value,
        ctx: &PermissionCheckContext,
    ) -> PermissionOutcome {
        let (mode, verdict) = self.rule_or_safety_verdict(name, input, ctx.workspace_lease_token);
        match verdict {
            Some(PermissionResult::Deny {
                reason,
                explanation,
                ..
            }) => {
                let msg = explanation.unwrap_or_else(|| deny_reason_string(&reason, name));
                tracing::warn!(
                    target: "permission",
                    "Hook returned 'allow' for {name}, but deny rule overrides: {msg}"
                );
                PermissionOutcome::Deny { reason: msg }
            }
            Some(PermissionResult::Ask {
                ref reason,
                ref metadata,
                ..
            }) => {
                tracing::warn!(
                    target: "permission",
                    "Hook returned 'allow' for {name}, but ask rule/safety check requires full permission pipeline"
                );
                // Re-check stays MODE-LESS (`rule_or_safety_verdict` evaluated
                // under `PermissionMode::Default`) — the oracle's post-hook-allow
                // handler hands the ask to the permission pipeline WITHOUT a mode
                // backstop, so we must not re-run the auto-mode classifier here
                // (HOOKALLOW-01). We DO carry the same control-request metadata the
                // normal Ask delegation builds so the stdio `can_use_tool` payload
                // matches, and apply any `updatedPermissions` the host allow
                // returns to the live session (parity with `setToolPermissionContext`).
                let mut ctx2 = ctx.clone();
                ctx2.decision_reason = serialize_decision_reason(reason);
                ctx2.decision_reason_type = decision_reason_type(reason).map(str::to_string);
                ctx2.classifier_approvable = classifier_approvable(reason);
                ctx2.matched_ask_rule = matched_ask_rule(reason);
                if metadata.permission_suggestions.is_some() {
                    ctx2.permission_suggestions = metadata.permission_suggestions.clone();
                }
                if metadata.blocked_path.is_some() {
                    ctx2.blocked_path = metadata.blocked_path.clone();
                }
                match self.inner.check_with_context(name, input, &ctx2).await {
                    PermissionOutcome::Allow {
                        updated_input,
                        permission_updates,
                        decision_classification,
                    } => {
                        if !permission_updates.is_empty() {
                            self.apply_permission_updates(&permission_updates);
                        }
                        PermissionOutcome::Allow {
                            updated_input,
                            permission_updates,
                            // This branch exists only because the resolver
                            // required an interactive ask. A host that omits
                            // the optional classification still represents a
                            // temporary user grant, not a standing hook allow.
                            decision_classification: decision_classification.or(Some(
                                traits::permission_gate::ToolDecisionClassification::UserTemporary,
                            )),
                        }
                    }
                    PermissionOutcome::Deny { reason } => PermissionOutcome::Deny { reason },
                }
            }
            _ => {
                self.record_auto_mode_non_deny(mode);
                PermissionOutcome::Allow {
                    updated_input: None,
                    permission_updates: Vec::new(),
                    decision_classification: None,
                }
            }
        }
    }

    fn flatten_permission_outcome(outcome: PermissionOutcome) -> PermissionDecision {
        match outcome {
            PermissionOutcome::Allow { .. } => PermissionDecision::Allow,
            PermissionOutcome::Deny { reason } => PermissionDecision::Deny { reason },
        }
    }

    fn rule_or_safety_verdict(
        &self,
        name: &str,
        input: &Value,
        workspace_lease_token: Option<u64>,
    ) -> (PermissionMode, Option<PermissionResult>) {
        let mode = self.effective_mode_for_tool(name);
        let result = self.authorize_with_live_state(
            name,
            input,
            PermissionMode::Default,
            workspace_lease_token,
        );
        let verdict = match &result {
            // Deny rules + the tool's own `checkPermissions` denies. (A
            // mode-sourced deny cannot occur here: `deny_with_mode` is reachable
            // only under `DontAsk`, which this neutral evaluation excludes.)
            PermissionResult::Deny { .. } => Some(result),
            // `_pt` reports an ASK only for its three arms — an ask RULE (`MFo`,
            // including a nested `subcommandResults` ask), a `safetyCheck`
            // (`W9`), or a `sandboxOverride`. Guard asks tagged `Other` (path
            // constraints, `$IFS`/bash-safety, sed, PowerShell containment,
            // edit-read-deny) are `type:"other"` in the oracle too and `_pt`
            // SKIPS them — so they must NOT block a hook allow.
            PermissionResult::Ask { reason, .. } if Self::is_pt_ask_reason(reason) => Some(result),
            _ => None,
        };
        (mode, verdict)
    }

    /// Does this ask reason correspond to one of `_pt`'s ask arms?
    /// `MFo(reason)` = a matched rule whose behavior is `ask` (recursing into
    /// `subcommandResults`), `W9(reason)` = `safetyCheck`, plus `sandboxOverride`.
    fn is_pt_ask_reason(reason: &PermissionDecisionReason) -> bool {
        match reason {
            PermissionDecisionReason::MatchedRule { rule } => {
                rule.behavior == PermissionBehavior::Ask
            }
            PermissionDecisionReason::SubcommandResults { reasons } => {
                reasons.values().any(|r| match r.as_ref() {
                    PermissionResult::Ask { reason, .. } => Self::is_pt_ask_reason(reason),
                    _ => false,
                })
            }
            PermissionDecisionReason::SafetyCheck { .. }
            | PermissionDecisionReason::SandboxOverride { .. } => true,
            _ => false,
        }
    }

    /// Evaluate one call against the current in-memory permission overlay while
    /// forcing a caller-selected mode. Per-worker and plan-mode checks must see
    /// the same live rules and working directories as the ordinary check path.
    fn authorize_with_live_state(
        &self,
        name: &str,
        input: &Value,
        mode: PermissionMode,
        workspace_lease_token: Option<u64>,
    ) -> PermissionResult {
        self.live_policy().authorize_with_mode_and_workspace_lease(
            name,
            input,
            mode,
            workspace_lease_token,
        )
    }

    fn live_policy(&self) -> PermissionPolicy {
        let live = self
            .live_state
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        self.policy.clone_with_live_state(
            live.allow_rules,
            live.deny_rules,
            live.ask_rules,
            live.additional_working_dirs,
        )
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
                match self.auto_mode_classifier_result(mode, reason, name, input, false) {
                    Ok(AutoModeClassifierResult::Classified(classified)) => {
                        return self.classified_result_to_decision(classified, name);
                    }
                    Ok(AutoModeClassifierResult::NoDecision)
                    | Ok(AutoModeClassifierResult::PromptFallback { .. }) => {}
                    Err(abort) => {
                        // `false` above makes this unreachable; retain a
                        // fail-closed mapping for custom/future classifiers on
                        // this legacy two-valued surface.
                        return PermissionDecision::Deny {
                            reason: abort.message,
                        };
                    }
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
    ) -> Result<PermissionOutcome, PermissionAbort> {
        match result {
            // A policy-rule allow may itself carry a rewritten input — surface it
            // (previously dropped at the `PermissionDecision::Allow` boundary). The
            // local policy gate never derives `updatedPermissions` rule updates
            // (those originate from the stdio host's `can_use_tool` response), so
            // `permission_updates` is always empty on this path.
            PermissionResult::Allow { updated_input, .. } => {
                self.record_auto_mode_non_deny(mode);
                Ok(PermissionOutcome::Allow {
                    updated_input,
                    permission_updates: Vec::new(),
                    decision_classification: None,
                })
            }
            PermissionResult::Deny {
                reason,
                explanation,
                ..
            } => {
                // GATE-SYSMSG-01: notify the transport of this LOCAL deny so the
                // stdio control-plane can emit a `permission_denied` system message
                // (createCanUseTool's deny arm). Compute the reason discriminant +
                // `oin`-filtered reason text from the deny reason BEFORE it is moved
                // into the rendered message. No-op on non-stdio transports.
                let drt = decision_reason_type(&reason);
                let dr = sysmsg_decision_reason(&reason);
                let message = explanation.unwrap_or_else(|| deny_reason_string(&reason, name));
                self.inner
                    .on_permission_denied(name, ctx, drt, dr.as_deref(), &message)
                    .await;
                Ok(PermissionOutcome::Deny { reason: message })
            }
            PermissionResult::Ask {
                ref reason,
                ref metadata,
                ..
            } => {
                // HOOK-ASKFLOOR-03: when a PreToolUse hook returned `ask`
                // (`ctx.hook_ask_floor`), the Auto-mode classifier's ALLOW must NOT
                // silently defeat the hook's ask — CC's `hookAskFloor` keeps the ask
                // (the classifier callback re-surfaces `behavior:"ask"`, and a
                // prompt-avoiding context returns the asyncAgent deny). Skipping the
                // classifier here lets the ask fall through to the normal path,
                // which prompts interactively and denies in headless
                // (`DenyOnAskGate`) — the same outcome as CC's floor. Without this,
                // Auto mode re-allows the tool, the 2.1.207 regression 211/215
                // removed.
                let fallback_decision_reason = if !ctx.hook_ask_floor {
                    match self.auto_mode_classifier_result(
                        mode,
                        reason,
                        name,
                        input,
                        ctx.is_non_interactive_session,
                    )? {
                        AutoModeClassifierResult::Classified(classified) => {
                            return Ok(self.classified_result_to_outcome(classified, name));
                        }
                        AutoModeClassifierResult::PromptFallback { decision_reason } => {
                            Some(decision_reason)
                        }
                        AutoModeClassifierResult::NoDecision => None,
                    }
                } else {
                    None
                };
                if read_only_default_auto_allows(name, reason) {
                    self.record_auto_mode_non_deny(mode);
                    Ok(PermissionOutcome::Allow {
                        updated_input: None,
                        permission_updates: Vec::new(),
                        decision_classification: None,
                    })
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
                    let mut ctx2 = ctx.clone();
                    let breaker_fallback = fallback_decision_reason.is_some();
                    if ctx2.decision_reason.is_none() {
                        ctx2.decision_reason = fallback_decision_reason
                            .clone()
                            .or_else(|| serialize_decision_reason(reason));
                    }
                    if ctx2.decision_reason_type.is_none() {
                        ctx2.decision_reason_type = if breaker_fallback {
                            Some("classifier".to_string())
                        } else {
                            decision_reason_type(reason).map(str::to_string)
                        };
                    }
                    // These two fields are functions of the authoritative
                    // decision reason. Clear any stale caller value when the
                    // reason does not support it instead of emitting metadata
                    // for an unrelated Ask.
                    ctx2.classifier_approvable = classifier_approvable(reason);
                    ctx2.matched_ask_rule = matched_ask_rule(reason);
                    // Tool-specific policy producers take precedence; retain an
                    // explicitly supplied transport context only when the
                    // permission result has no structured value.
                    if metadata.permission_suggestions.is_some() {
                        ctx2.permission_suggestions = metadata.permission_suggestions.clone();
                    }
                    if metadata.blocked_path.is_some() {
                        ctx2.blocked_path = metadata.blocked_path.clone();
                    }
                    let outcome = self.inner.check_with_context(name, input, &ctx2).await;
                    if let PermissionOutcome::Allow {
                        permission_updates, ..
                    } = &outcome
                    {
                        self.record_auto_mode_non_deny(mode);
                        // In-memory apply (claude-code `setToolPermissionContext(
                        // u=>bJ(u,updates))`): when the host's allow carried
                        // `updatedPermissions`, apply them to the LIVE session so
                        // subsequent checks see the change — not only the next
                        // session load. Mode, rule, and directory arms all fold
                        // through the same live reducer below.
                        if !permission_updates.is_empty() {
                            self.apply_permission_updates(permission_updates);
                        }
                    }
                    Ok(outcome)
                }
            }
        }
    }

    /// Like [`Self::decide`] but WITHOUT consulting the inner prompt transport:
    /// returns a [`PermissionResolution`] that carries the deny SOURCE and, for a
    /// would-be prompt, an [`PermissionResolution::Ask`] (or
    /// [`PermissionResolution::AskWithContext`] when the classifier breaker
    /// rewrites the reason) instead of resolving it.
    /// The turn loop uses this to fire the source-gated permission hooks
    /// (`PermissionRequest` on `Ask`, `PermissionDenied` on a classifier `Deny`)
    /// before delegating to the transport. See [`PermissionGate::resolve_detailed`].
    fn resolve_with_mode(
        &self,
        mode: PermissionMode,
        result: PermissionResult,
        name: &str,
        input: &Value,
        is_non_interactive_session: bool,
    ) -> Result<PermissionResolution, PermissionAbort> {
        match result {
            PermissionResult::Allow { ref reason, .. } => Ok(PermissionResolution::Allow {
                rule_source: rule_settings_source(reason),
            }),
            PermissionResult::Deny {
                reason,
                explanation,
                ..
            } => Ok(PermissionResolution::Deny {
                source: map_decision_source(&reason),
                rule_source: rule_settings_source(&reason),
                // GATE-SYSMSG-01: pre-compute the system-message discriminants from
                // the FULL reason (in scope here) so the turn loop can emit the
                // `permission_denied` frame on the main-conversation deny path —
                // faithful for rule/mode AND classifier (the classifier deny
                // recurses back into this arm with a `ClassifierRejected` reason).
                decision_reason_type: decision_reason_type(&reason).map(str::to_string),
                decision_reason: sysmsg_decision_reason(&reason),
                reason: explanation.unwrap_or_else(|| deny_reason_string(&reason, name)),
                // The policy gate's rule/mode/classifier denials are never an
                // `ask`-behavior rejection carrying contentBlocks (the external
                // build has no classifier/contentBlocks producer), so the
                // top-level-image path stays dormant — byte-identical to before.
                behavior_ask: false,
                content_blocks: Vec::new(),
            }),
            PermissionResult::Ask { ref reason, .. } => {
                match self.auto_mode_classifier_result(
                    mode,
                    reason,
                    name,
                    input,
                    is_non_interactive_session,
                )? {
                    AutoModeClassifierResult::Classified(classified) => {
                        return self.resolve_with_mode(
                            mode,
                            classified,
                            name,
                            input,
                            is_non_interactive_session,
                        );
                    }
                    AutoModeClassifierResult::PromptFallback { decision_reason } => {
                        return Ok(PermissionResolution::AskWithContext {
                            decision_reason_type: Some("classifier".to_string()),
                            decision_reason: Some(decision_reason),
                        });
                    }
                    AutoModeClassifierResult::NoDecision => {}
                }
                if read_only_default_auto_allows(name, reason) {
                    // Read-only / agent-local tool with NO explicit `ask` rule —
                    // auto-allowed, no prompt. No rule matched, so no scope.
                    Ok(PermissionResolution::Allow { rule_source: None })
                } else {
                    // A would-be prompt (a mutating tool, OR an explicit `ask`
                    // rule on a read-only tool): the turn loop fires
                    // PermissionRequest before this is delegated to the transport.
                    Ok(PermissionResolution::Ask)
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
        is_non_interactive_session: bool,
    ) -> Result<AutoModeClassifierResult, PermissionAbort> {
        if mode != PermissionMode::Auto
            || !crate::classifier::is_classifier_permissions_enabled()
            || !reason_allows_classifier(reason)
        {
            return Ok(AutoModeClassifierResult::NoDecision);
        }
        {
            let tracking = self
                .policy
                .denial_tracking
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if tracking.is_circuit_broken() {
                return Ok(AutoModeClassifierResult::NoDecision);
            }
        }
        match classify_tool_call(name, input) {
            AutoModeClassifierVerdict::Allow { score, .. } => {
                self.record_auto_mode_non_deny(mode);
                Ok(AutoModeClassifierResult::Classified(
                    PermissionResult::Allow {
                        reason: PermissionDecisionReason::ClassifierApproved {
                            classifier: ClassifierKind::Transcript,
                            score,
                        },
                        updated_input: None,
                        update_destination: None,
                        metadata: PermissionMetadata::default(),
                    },
                ))
            }
            AutoModeClassifierVerdict::Deny { score, reason, .. } => {
                let mut tracking = self
                    .policy
                    .denial_tracking
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                tracking.record_auto_deny();
                if let Some(trip) = tracking.trip(is_non_interactive_session) {
                    telemetry::emit_auto_mode_denial_limit_exceeded(
                        trip.mode_tag(),
                        trip.consecutive_denials,
                        trip.total_denials,
                        name,
                    );
                    if trip.headless {
                        return Err(PermissionAbort {
                            message:
                                crate::denial_tracking::DenialBreakerTrip::HEADLESS_ABORT_MESSAGE
                                    .to_string(),
                        });
                    }
                    tracing::warn!(target: "permission", "{}", trip.fallback_warn_line());
                    return Ok(AutoModeClassifierResult::PromptFallback {
                        // Claude's breaker receives the classifier's own
                        // human-readable blocked-action reason (`q.reason`),
                        // not a synthesized tool-name prefix.
                        decision_reason: trip.decision_reason(&reason),
                    });
                }
                Ok(AutoModeClassifierResult::Classified(
                    PermissionResult::Deny {
                        reason: PermissionDecisionReason::ClassifierRejected {
                            classifier: ClassifierKind::Transcript,
                            score,
                            reason: reason.clone(),
                        },
                        explanation: Some(format!("Auto mode classifier blocked action: {reason}")),
                        metadata: PermissionMetadata::default(),
                    },
                ))
            }
            AutoModeClassifierVerdict::Pass { .. } => Ok(AutoModeClassifierResult::NoDecision),
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
                decision_classification: None,
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

    /// Apply a SINGLE host `updatedPermissions` entry to the LIVE session
    /// in-memory (claude-code `Xb` — the per-update reducer folded by `bJ`).
    ///
    /// This is the in-memory half of `applyPermissionUpdate`: when a
    /// `can_use_tool` (or `PermissionRequest` hook) ALLOW response carries
    /// `updatedPermissions`, the SAME session's later checks must see the change
    /// — not only the NEXT session load. Here we port the **`setMode`** arm
    /// byte-faithfully (the only arm that maps cleanly onto the gate's existing
    /// interior-mutable state, [`Self::mode_override`]):
    ///
    /// - `bypassPermissions` is REJECTED (no mode change) when bypass is not
    ///   available — the disabled-by-settings killswitch OR a session not
    ///   launched with `--dangerously-skip-permissions`
    ///   (`!isBypassPermissionsModeAvailable`) — with the byte-exact
    ///   `Ignoring permission update: setMode 'bypassPermissions' rejected …`
    ///   debug log. NOTE this differs from
    ///   [`PermissionGate::set_permission_mode`]: `Xb` applies the mode RAW with
    ///   ONLY the bypass-availability guard (no `auto` `Nle` gate — that guards
    ///   the interactive `setPermissionMode` handler, not this reducer).
    /// - any other mode is applied, logged `Applying permission update: Setting
    ///   mode to '<mode>'`. An UNKNOWN mode string is logged (matching `Xb`,
    ///   which sets the raw string and lets `transitionPermissionMode` no-op it)
    ///   but leaves the typed [`Self::mode_override`] unchanged.
    ///
    /// Rule and directory updates are applied to the gate's live overlay and
    /// therefore affect subsequent model, subagent, and prompt-shell checks in
    /// the current session. Persistence remains the control plane's concern.
    pub fn apply_permission_update(&self, update: &Value) {
        match update.get("type").and_then(Value::as_str) {
            Some("setMode") => {
                let Some(mode_str) = update.get("mode").and_then(Value::as_str) else {
                    return;
                };
                let bypass_unavailable = self.policy.bypass_killswitch_active
                    || !self.policy.bypass_permissions_available;
                if mode_str == "bypassPermissions" && bypass_unavailable {
                    tracing::debug!(
                        "Ignoring permission update: setMode 'bypassPermissions' rejected — mode is not available (disableBypassPermissionsMode set, or session not launched in bypassPermissions mode)"
                    );
                    return;
                }
                tracing::debug!("Applying permission update: Setting mode to '{mode_str}'");
                if let Some(parsed) = parse_settable_mode(mode_str) {
                    *self
                        .mode_override
                        .write()
                        .unwrap_or_else(|e| e.into_inner()) = Some(parsed);
                }
            }
            Some("addRules") | Some("replaceRules") | Some("removeRules") => {
                let Some(source) = Self::parse_update_destination(
                    update.get("destination").unwrap_or(&Value::Null),
                ) else {
                    return;
                };
                let Some(behavior) =
                    Self::parse_update_behavior(update.get("behavior").unwrap_or(&Value::Null))
                else {
                    return;
                };
                let Some(rules) = Self::parse_update_rules(
                    update.get("rules").unwrap_or(&Value::Null),
                    behavior,
                    source,
                ) else {
                    return;
                };
                let mut live = self.live_state.write().unwrap_or_else(|e| e.into_inner());
                let bucket = Self::rule_bucket_mut(&mut live, behavior);
                match update.get("type").and_then(Value::as_str) {
                    Some("addRules") => {
                        let entry = bucket.entry(source).or_default();
                        for rule in rules {
                            if !entry.iter().any(|existing| existing.value == rule.value) {
                                entry.push(rule);
                            }
                        }
                    }
                    Some("replaceRules") => {
                        bucket.insert(source, rules);
                    }
                    Some("removeRules") => {
                        let to_remove: Vec<_> = rules.into_iter().map(|rule| rule.value).collect();
                        if let Some(entry) = bucket.get_mut(&source) {
                            entry.retain(|existing| {
                                !to_remove.iter().any(|rule| rule == &existing.value)
                            });
                        }
                    }
                    _ => {}
                }
            }
            Some("addDirectories") | Some("removeDirectories") => {
                if Self::parse_update_destination(update.get("destination").unwrap_or(&Value::Null))
                    .is_none()
                {
                    return;
                }
                let Some(directories) = update.get("directories").and_then(Value::as_array) else {
                    return;
                };
                let Some(directories) = directories
                    .iter()
                    .map(Value::as_str)
                    .collect::<Option<Vec<_>>>()
                else {
                    return;
                };
                let mut live = self.live_state.write().unwrap_or_else(|e| e.into_inner());
                match update.get("type").and_then(Value::as_str) {
                    Some("addDirectories") => {
                        for dir in directories {
                            let dir = PathBuf::from(dir);
                            if !live
                                .additional_working_dirs
                                .iter()
                                .any(|existing| existing == &dir)
                            {
                                live.additional_working_dirs.push(dir);
                            }
                        }
                    }
                    Some("removeDirectories") => {
                        let to_remove: Vec<PathBuf> =
                            directories.into_iter().map(PathBuf::from).collect();
                        live.additional_working_dirs
                            .retain(|dir| !to_remove.iter().any(|remove| remove == dir));
                    }
                    _ => {}
                }
            }
            _ => {
                // Unknown update type: ignored, matching the JS reducer.
            }
        }
    }

    /// Fold [`Self::apply_permission_update`] over a host `updatedPermissions`
    /// array (claude-code `bJ`), applying each entry to the live session.
    pub fn apply_permission_updates(&self, updates: &[Value]) {
        for update in updates {
            self.apply_permission_update(update);
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
    fn read_deny_exclude_globs(&self, cwd: &std::path::Path) -> Option<Vec<String>> {
        Some(crate::read_deny_exclude_globs(&self.live_policy(), cwd))
    }

    fn check_noninteractive_with_allow_rules(
        &self,
        name: &str,
        input: &Value,
        transient_allow_rules: &[String],
    ) -> Option<traits::permission_gate::NonInteractivePermissionDecision> {
        use traits::permission_gate::NonInteractivePermissionDecision;

        let mode = self.effective_mode_for_tool(name);
        let mut live = self
            .live_state
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let command_rules = live
            .allow_rules
            .entry(PermissionRuleSource::Command)
            .or_default();
        command_rules.extend(transient_allow_rules.iter().map(|spec| PermissionRule {
            value: PermissionRuleValue::from_rule_string(spec),
            behavior: PermissionBehavior::Allow,
            source: PermissionRuleSource::Command,
        }));
        let policy = self.policy.clone_with_live_state(
            live.allow_rules,
            live.deny_rules,
            live.ask_rules,
            live.additional_working_dirs,
        );
        Some(match policy.authorize_with_mode(name, input, mode) {
            PermissionResult::Allow { .. } => NonInteractivePermissionDecision::Allow,
            PermissionResult::Deny { explanation, .. } => NonInteractivePermissionDecision::Deny {
                reason: explanation,
            },
            PermissionResult::Ask { prompt, .. } => NonInteractivePermissionDecision::Deny {
                reason: Some(prompt.message),
            },
        })
    }

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
        match self.check_with_context_or_abort(name, input, ctx).await {
            Ok(outcome) => outcome,
            Err(abort) => PermissionOutcome::Deny {
                reason: abort.message,
            },
        }
    }

    async fn check_with_context_or_abort(
        &self,
        name: &str,
        input: &Value,
        ctx: &PermissionCheckContext,
    ) -> Result<PermissionOutcome, PermissionAbort> {
        // A PER-CALL mode override (a spawned subagent's clamped spawn mode,
        // claude-code 2.1.207 `ve` → the child's `toolPermissionContext.mode`)
        // authorizes THIS call under that mode; else the live/boot mode. Only this
        // dispatch seam reads it, so the shared gate's mode is never mutated (the
        // parent's own checks are unaffected).
        let (mode, result) = match ctx.mode_override.as_deref().and_then(parse_settable_mode) {
            Some(m) => (
                m,
                self.authorize_with_live_state(name, input, m, ctx.workspace_lease_token),
            ),
            None => self.effective_authorize_with_lease(name, input, ctx.workspace_lease_token),
        };
        self.decide_outcome_with_context(mode, result, name, input, ctx)
            .await
    }

    fn apply_permission_updates(&self, updates: &[Value]) {
        PolicyPermissionGate::apply_permission_updates(self, updates);
    }

    async fn persist_permission_updates(&self, updates: &[Value]) {
        if !self.policy.allow_managed_permission_rules_only {
            self.inner.persist_permission_updates(updates).await;
        }
    }

    fn set_permission_persistence_enabled(&self, enabled: bool) {
        self.inner.set_permission_persistence_enabled(
            enabled && !self.policy.allow_managed_permission_rules_only,
        );
    }

    /// GATE-SYSMSG-01: forward a deny notification to the inner transport (the
    /// only layer with an outbound stream). The main turn loop calls this on the
    /// OUTER gate for a main-conversation deny (which resolves via
    /// `resolve_detailed`, not `check_with_context`), so the stdio control-plane
    /// still emits the `permission_denied` system message; the subagent-dispatch
    /// path emits directly from `decide_outcome_with_context`.
    async fn on_permission_denied(
        &self,
        name: &str,
        ctx: &PermissionCheckContext,
        decision_reason_type: Option<&str>,
        decision_reason: Option<&str>,
        message: &str,
    ) {
        self.inner
            .on_permission_denied(name, ctx, decision_reason_type, decision_reason, message)
            .await;
    }

    /// A PreToolUse / PermissionRequest hook `allow` skips the PROMPT but still
    /// applies rule-based deny/ask (claude-code `resolveHookPermissionDecision` +
    /// `checkRuleBasedPermissions`): a hook cannot override an explicit deny
    /// rule or the active mode's mutation backstop. So run `authorize` and map
    /// `Deny → Deny` (deny rules + mode still bind) but `Ask → Allow` (the hook
    /// approved, so the would-be prompt is skipped) and `Allow → Allow`. Unlike
    /// `check`, an `Ask` NEVER delegates to the inner prompt transport here — the
    /// hook already resolved the prompt.
    /// claude-code `lin` — resolve a **PreToolUse** hook `allow`.
    ///
    /// `lin` re-runs the rule/safety check ([`Self::rule_or_safety_verdict`],
    /// the `_pt` analog) UNCONDITIONALLY — there is no "only if the hook rewrote
    /// the input" gate — and then:
    ///
    /// * `deny` ⇒ the deny rule OVERRIDES the hook
    ///   (`"…but deny rule overrides: ${u.message}"`);
    /// * `ask`  ⇒ the call goes to the FULL permission pipeline
    ///   (`"…but ask rule/safety check requires full permission pipeline"`), i.e.
    ///   it PROMPTS — headless resolves that to a deny via the inner gate;
    /// * no verdict ⇒ the hook's allow stands, prompt skipped.
    ///
    /// The third arm is why the mode layer must be subtracted: an ordinary
    /// Default-mode mutating call has no rule verdict at all, so the hook allow
    /// is honoured. Feeding the mode-backstop ask in here instead would deny
    /// almost every hook-rescued call.
    async fn check_after_hook_allow(&self, name: &str, input: &Value) -> PermissionDecision {
        Self::flatten_permission_outcome(
            self.check_after_hook_allow_impl(name, input, &PermissionCheckContext::default())
                .await,
        )
    }

    async fn check_after_hook_allow_ctx(
        &self,
        name: &str,
        input: &Value,
        ctx: &PermissionCheckContext,
    ) -> PermissionDecision {
        Self::flatten_permission_outcome(self.check_after_hook_allow_impl(name, input, ctx).await)
    }

    async fn check_after_hook_allow_outcome_ctx(
        &self,
        name: &str,
        input: &Value,
        ctx: &PermissionCheckContext,
    ) -> PermissionOutcome {
        self.check_after_hook_allow_impl(name, input, ctx).await
    }

    /// claude-code `Fxy` + `epr` — resolve a **PermissionRequest** (headless
    /// rescue) hook `allow` that came with a REWRITTEN input.
    ///
    /// Same `_pt` re-check, but an `ask` becomes a **hard deny** carrying the
    /// ASK's own message (`{behavior:"deny", message:c.message, …}`) — that
    /// surface has no prompt to fall back to, and the hook already consumed the
    /// one opportunity to resolve it. Without this a hook could rewrite a tool's
    /// arguments into something an ask rule covers and have it run unprompted.
    async fn check_after_hook_allow_rewritten(
        &self,
        name: &str,
        input: &Value,
    ) -> PermissionDecision {
        let (mode, verdict) = self.rule_or_safety_verdict(name, input, None);
        match verdict {
            Some(PermissionResult::Deny {
                reason,
                explanation,
                ..
            }) => {
                let msg = explanation.unwrap_or_else(|| deny_reason_string(&reason, name));
                tracing::warn!(
                    target: "permission",
                    "PermissionRequest hook allowed {name} with updatedInput, but deny rule overrides: {msg}"
                );
                PermissionDecision::Deny { reason: msg }
            }
            Some(PermissionResult::Ask { prompt, .. }) => {
                tracing::warn!(
                    target: "permission",
                    "PermissionRequest hook allowed {name} with updatedInput, but ask rule overrides: {}",
                    prompt.message
                );
                // `c.behavior==="ask" ? {behavior:"deny", message:c.message, …}`
                PermissionDecision::Deny {
                    reason: prompt.message,
                }
            }
            _ => {
                self.record_auto_mode_non_deny(mode);
                PermissionDecision::Allow
            }
        }
    }

    /// claude-code `Fxy` — the STANDING PermissionRequest-hook `allow` (no
    /// `updatedInput`, tool does not `requiresUserInteraction`). The oracle SKIPS
    /// the `_pt` re-check entirely and returns the allow, so we must NOT run
    /// [`Self::rule_or_safety_verdict`]: an ask rule (the reason the gate resolved
    /// `Ask` in the first place) would otherwise re-prompt / hard-deny and defeat
    /// the rescue. A matching deny rule cannot reach here — it resolves `Deny`
    /// before the PermissionRequest hook fires — so honouring the unchanged input
    /// is safe. We keep only the mode-less auto-mode non-deny bookkeeping every
    /// allow arm records (reset of the classifier breaker's consecutive-denial
    /// counter), matching the pre-HOOKALLOW-01 behavior for this arm.
    async fn honour_hook_allow(&self, name: &str, _input: &Value) -> PermissionDecision {
        self.record_auto_mode_non_deny(self.effective_mode_for_tool(name));
        PermissionDecision::Allow
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
            self.authorize_with_live_state(name, input, PermissionMode::Plan, None),
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
        match self.resolve_with_mode(mode, result, name, input, false) {
            Ok(resolution) => resolution,
            Err(abort) => PermissionResolution::Deny {
                reason: abort.message,
                source: PermissionDecisionSource::Unspecified,
                rule_source: None,
                decision_reason_type: None,
                decision_reason: None,
                behavior_ask: false,
                content_blocks: Vec::new(),
            },
        }
    }

    async fn resolve_detailed_or_abort(
        &self,
        name: &str,
        input: &Value,
        ctx: &PermissionCheckContext,
    ) -> Result<PermissionResolution, PermissionAbort> {
        let (mode, result) = self.effective_authorize_with_lease(name, input, ctx.workspace_lease_token);
        self.resolve_with_mode(mode, result, name, input, ctx.is_non_interactive_session)
    }

    /// Surface the wrapped policy's TOOL-WIDE deny-rule names so the orchestrator
    /// strips blanket-denied tools from the wire `tools` array before the model
    /// sees them (claude-code `filterToolsByDenyRules`). Content deny rules are
    /// excluded by [`PermissionPolicy::tool_wide_deny_names`] (they deny calls,
    /// not the tool). With zero deny rules this is empty ⇒ no tools stripped.
    async fn tool_wide_deny_names(&self) -> Vec<String> {
        self.live_policy().tool_wide_deny_names()
    }

    /// Surface the source of a matching `Agent(<type>)` deny rule so the Agent
    /// tool can reject a denied subagent type with the byte-exact
    /// `AgentTypeError` message (claude-code `getDenyRuleForAgent`). The source is
    /// rendered as the raw `SettingSource` identifier
    /// ([`PermissionRuleSource::lingxi_settings_source`]), matching the binary's
    /// `… from ${rule.source}.`.
    async fn agent_type_deny(&self, agent_type: &str) -> Option<String> {
        self.live_policy()
            .agent_type_deny_source(agent_type)
            .map(|s| s.lingxi_settings_source().to_string())
    }

    /// Surface the wrapped policy's CONTENT-ful `Agent(<x>)` deny set so the
    /// advertised agent catalog and `Available agents:` lists exclude denied
    /// types (claude-code `Pxe`).
    async fn agent_deny_content_types(&self) -> Vec<String> {
        self.live_policy().agent_deny_content_types()
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
    ///   three runtime-available `P0()` inputs in `One()`'s precedence — the
    ///   `disableAutoMode` settings killswitch
    ///   ([`crate::PermissionPolicy::auto_mode_disabled`]), the local denial
    ///   circuit-breaker, then the model gate (`dUe(wi())`) against the LIVE
    ///   session model (via [`Self::live_model_provider_handle`]) — and reject
    ///   with the byte-exact message for the
    ///   [`crate::auto_gate::AutoGateDenialReason::Settings`] /
    ///   [`crate::auto_gate::AutoGateDenialReason::CircuitBreaker`] /
    ///   [`crate::auto_gate::AutoGateDenialReason::Model`] cases. So a live
    ///   runtime switch to `auto` after a `/model` to an auto-unsupported model
    ///   is now rejected here (`auto mode unavailable for this model`), matching
    ///   the binary — not only downgraded at boot.
    ///
    ///   When the live model/provider cell is unset (headless /
    ///   pre-orchestrator), the model check is skipped and the boot downgrade
    ///   remains the authoritative enforcement.
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
            // `Nle`: reject `auto` when `!P0()`. Report `One()`'s reason in the
            // binary's precedence (settings → circuit-breaker → model), rendering
            // the byte-exact `Cannot set permission mode to auto: <Jce(reason)>`.
            if let Some(reason) = self.auto_mode_denial_reason() {
                return Err(crate::auto_gate::cannot_set_auto_message(reason));
            }
        }
        *self
            .mode_override
            .write()
            .unwrap_or_else(|e| e.into_inner()) = Some(parsed);
        Ok(())
    }

    async fn set_mcp_permission_mode_override(
        &self,
        server_name: &str,
        mode: Option<&str>,
    ) -> Result<(), String> {
        let normalized = protocol::normalize_name_for_mcp(server_name);
        let mut overrides = self
            .mcp_mode_overrides
            .write()
            .unwrap_or_else(|e| e.into_inner());
        match mode {
            None => {
                overrides.remove(&normalized);
                Ok(())
            }
            Some("default") => {
                overrides.insert(normalized, PermissionMode::Default);
                Ok(())
            }
            Some("auto") => {
                // `set_mcp_permission_mode_override` applies the same `_k()` /
                // `Zse()` availability gate as the session-wide auto switch.
                // A disabled auto mode must not be reintroduced through a
                // server pin; the control response uses this byte-exact text.
                if let Some(reason) = self.auto_mode_denial_reason() {
                    return Err(format!(
                        "Cannot pin MCP server '{server_name}' to auto: {}",
                        reason.message()
                    ));
                }
                overrides.insert(normalized, PermissionMode::Auto);
                Ok(())
            }
            Some(other) => Err(format!(
                "Permission mode override over the control channel is tighten-only ('default', 'auto', or null); rejected '{other}'"
            )),
        }
    }

    /// The LIVE effective mode as a wire string — the override when set, else the
    /// boot mode ([`Self::effective_mode`]). Lets the in-process `/resume`
    /// re-mount snapshot and restore the user's mid-session Shift+Tab mode.
    fn permission_mode(&self) -> Option<String> {
        Some(self.effective_mode().wire_str().to_string())
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

fn mcp_server_token(tool_name: &str) -> Option<&str> {
    let mut parts = tool_name.splitn(3, "__");
    let Some("mcp") = parts.next() else {
        return None;
    };
    parts.next().filter(|server| !server.is_empty())
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

/// Compute the stdio `classifier_approvable` value from the structured
/// decision reason.  Claude Code emits this field only when at least one
/// `safetyCheck` exists and folds composite subcommand reasons so one
/// non-approvable check makes the whole request non-approvable.
fn classifier_approvable(reason: &PermissionDecisionReason) -> Option<bool> {
    fn visit(reason: &PermissionDecisionReason) -> (bool, bool) {
        match reason {
            PermissionDecisionReason::SafetyCheck {
                classifier_approvable,
                ..
            } => (true, *classifier_approvable),
            PermissionDecisionReason::SubcommandResults { reasons } => {
                reasons.values().fold((false, true), |(any, all), result| {
                    let nested = match result.as_ref() {
                        PermissionResult::Allow { reason, .. }
                        | PermissionResult::Deny { reason, .. }
                        | PermissionResult::Ask { reason, .. } => visit(reason),
                    };
                    (any || nested.0, all && nested.1)
                })
            }
            _ => (false, true),
        }
    }

    let (has_safety_check, all_approvable) = visit(reason);
    has_safety_check.then_some(all_approvable)
}

/// The `matched_ask_rule` control-request field — 1:1 with the 2.1.218
/// `matched_ask_rule` SDK schema (`b.object({source,tool_name,rule_content?})`).
///
/// The oracle sets this ONLY in the *ask-rule substitution* case: a
/// `permissions.ask` rule forced the prompt **but the ask carries the tool's own
/// `decision_reason`** (a richer tool-minted ask), so the rule "rides here
/// **instead of** `decision_reason_type: 'rule'`". `matched_ask_rule` and
/// `decision_reason_type: "rule"` are therefore MUTUALLY EXCLUSIVE.
///
/// This port collapses a plain ask-RULE match into a top-level
/// [`PermissionDecisionReason::MatchedRule`] ⇒ `decision_reason_type: "rule"`, so
/// the rule is already conveyed by the type and there is no tool-minted reason to
/// substitute against — the substitution case is never produced. Deriving the
/// field from a bare `MatchedRule` (the previous behavior) emitted BOTH
/// `decision_reason_type: "rule"` AND `matched_ask_rule` for the same ask, which
/// the oracle never does. There is no substitution producer in the port, so this
/// is always `None`.
fn matched_ask_rule(_reason: &PermissionDecisionReason) -> Option<MatchedAskRule> {
    None
}

/// Serialize a [`PermissionDecisionReason`] to the free-text `decision_reason`
/// string a stdio `can_use_tool` control_request carries — 1:1 with claude-code
/// `serializeDecisionReason` (`cli/structuredIO.ts:64-91`).
///
/// The oracle returns `undefined` (⇒ `None`, the key is OMITTED) for the
/// `rule`/`mode`/`subcommandResults`/`permissionPromptTool` reasons (the common
/// ask cases — an SDK host parses `decision_reason_type` for those, not the
/// text), and the reason STRING for `hook`/`asyncAgent`/`sandboxOverride`/
/// `workingDir`/`safetyCheck`/`other`/`classifier`.
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
        // (see fn doc). Classifier denials preserve the classifier's own
        // human-readable reason, as `createCanUseTool` does in claude-code.
        PermissionDecisionReason::SandboxOverride { .. }
        | PermissionDecisionReason::ClassifierApproved { .. }
        | PermissionDecisionReason::DenialLimitExceeded
        | PermissionDecisionReason::AutoModeFallback
        | PermissionDecisionReason::BypassPermissions => None,
        PermissionDecisionReason::ClassifierRejected { reason, .. } => Some(reason.clone()),
    }
}

/// The claude-code `decisionReason.type` discriminant string for this reason —
/// sent as the `decision_reason_type` field of a `can_use_tool` request so an SDK
/// host can classify Ask reasons where the free-text `decision_reason` is
/// `undefined` (rule/mode/subcommandResults/permissionPromptTool). Every string
/// is byte-confirmed present in the 2.1.215 binary; the LingXi-internal
/// denial/auto-mode/bypass variants carry no CC `.type` and return `None`.
pub(crate) fn decision_reason_type(reason: &PermissionDecisionReason) -> Option<&'static str> {
    Some(match reason {
        PermissionDecisionReason::MatchedRule { .. } => "rule",
        PermissionDecisionReason::PermissionMode { .. } => "mode",
        PermissionDecisionReason::SubcommandResults { .. } => "subcommandResults",
        PermissionDecisionReason::PermissionPromptTool { .. } => "permissionPromptTool",
        PermissionDecisionReason::ClassifierApproved { .. }
        | PermissionDecisionReason::ClassifierRejected { .. } => "classifier",
        PermissionDecisionReason::HookOverride { .. } => "hook",
        PermissionDecisionReason::AsyncAgent { .. } => "asyncAgent",
        PermissionDecisionReason::WorkingDirectory { .. } => "workingDir",
        PermissionDecisionReason::SafetyCheck { .. } => "safetyCheck",
        PermissionDecisionReason::SandboxOverride { .. } => "sandboxOverride",
        PermissionDecisionReason::Other { .. } => "other",
        PermissionDecisionReason::DenialLimitExceeded
        | PermissionDecisionReason::AutoModeFallback
        | PermissionDecisionReason::BypassPermissions => return None,
    })
}

/// GATE-SYSMSG-01 `oin(decisionReason)`: the reason TEXT surfaced in a
/// `permission_denied` system message. Only the free-text reason kinds expose
/// their reason; `rule`/`mode`/`subcommandResults`/`permissionPromptTool` return
/// `None` (byte-faithful to `oin`).
///
/// The oracle also returns `e.reason` for `classifier` and `sandboxOverride`.
/// Classifier denials retain that text; `SandboxOverride` still carries a
/// structured [`SandboxOverrideReason`] rather than the oracle's plain string,
/// so that allow-side reason remains omitted here.
pub(crate) fn sysmsg_decision_reason(reason: &PermissionDecisionReason) -> Option<String> {
    match reason {
        PermissionDecisionReason::HookOverride { reason, .. } => reason.clone(),
        PermissionDecisionReason::AsyncAgent { reason } => Some(reason.clone()),
        PermissionDecisionReason::WorkingDirectory { reason } => Some(reason.clone()),
        PermissionDecisionReason::SafetyCheck { reason, .. } => Some(reason.clone()),
        PermissionDecisionReason::ClassifierRejected { reason, .. } => Some(reason.clone()),
        PermissionDecisionReason::Other { reason } => Some(reason.clone()),
        _ => None,
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

/// Raw `SettingSource` token of the rule behind a `decisionReason`, or `None`
/// when the decision did not come from a rule. Feeds the OTEL decision-source
/// label the turn loop derives with claude-code `ZX_` — which reads exactly
/// `decisionReason.rule.source` and only for `decisionReason.type === 'rule'`
/// (a `subcommandResults` composite maps to `"config"` there, so it is NOT
/// unwrapped here).
fn rule_settings_source(reason: &PermissionDecisionReason) -> Option<String> {
    match reason {
        PermissionDecisionReason::MatchedRule { rule } => {
            Some(rule.source.lingxi_settings_source().to_string())
        }
        _ => None,
    }
}

#[cfg(test)]
#[path = "policy_gate_test.rs"]
mod policy_gate_test;

// GATE-SYSMSG-01: separate inline module (kept out of the concurrently-edited
// policy_gate_test.rs) covering the `oin` reason filter and the
// `on_permission_denied` hook firing on a local Deny.
#[cfg(test)]
mod gate_sysmsg_test {
    use super::*;
    use crate::{
        PermissionBehavior, PermissionMode, PermissionPolicy, PermissionRule, PermissionRuleSource,
        PermissionRuleValue,
    };
    use serde_json::json;
    use std::sync::Mutex as StdMutex;

    #[test]
    fn sysmsg_decision_reason_matches_oin_filter() {
        // Free-text kinds expose their reason.
        assert_eq!(
            sysmsg_decision_reason(&PermissionDecisionReason::Other {
                reason: "nope".into()
            })
            .as_deref(),
            Some("nope")
        );
        assert_eq!(
            sysmsg_decision_reason(&PermissionDecisionReason::SafetyCheck {
                reason: "danger".into(),
                classifier_approvable: false,
            })
            .as_deref(),
            Some("danger")
        );
        assert_eq!(
            sysmsg_decision_reason(&PermissionDecisionReason::WorkingDirectory {
                reason: "escapes".into()
            })
            .as_deref(),
            Some("escapes")
        );
        // rule / mode → None (byte-faithful to `oin`).
        assert_eq!(
            sysmsg_decision_reason(&PermissionDecisionReason::PermissionMode {
                mode: PermissionMode::Default
            }),
            None
        );
        // classifier → the classifier's own free-text reason.
        assert_eq!(
            sysmsg_decision_reason(&PermissionDecisionReason::ClassifierRejected {
                classifier: crate::ClassifierKind::Bash,
                score: 0.9,
                reason: "blocked".into(),
            }),
            Some("blocked".to_string())
        );
    }

    #[derive(Default)]
    struct RecordingGate {
        denied: Arc<
            StdMutex<
                Vec<(
                    String,
                    Option<String>,
                    Option<String>,
                    Option<String>,
                    String,
                )>,
            >,
        >,
    }

    #[async_trait::async_trait]
    impl crate::gate::PermissionGate for RecordingGate {
        async fn check(&self, _name: &str, _input: &Value) -> PermissionDecision {
            PermissionDecision::Allow
        }
        async fn on_permission_denied(
            &self,
            name: &str,
            ctx: &PermissionCheckContext,
            decision_reason_type: Option<&str>,
            decision_reason: Option<&str>,
            message: &str,
        ) {
            self.denied.lock().unwrap().push((
                name.to_string(),
                ctx.tool_use_id.clone(),
                decision_reason_type.map(str::to_string),
                decision_reason.map(str::to_string),
                message.to_string(),
            ));
        }
    }

    #[tokio::test]
    async fn deny_fires_on_permission_denied_with_ctx_and_reason() {
        // A tool-wide Bash DENY rule → any Bash call denies.
        let deny = PermissionRule {
            value: PermissionRuleValue {
                tool_name: "Bash".into(),
                rule_content: None,
            },
            behavior: PermissionBehavior::Deny,
            source: PermissionRuleSource::UserSettings,
        };
        let policy = Arc::new(PermissionPolicy::from_rules(
            PermissionMode::Default,
            vec![deny],
        ));
        let recorder = Arc::new(RecordingGate::default());
        let calls = recorder.denied.clone();
        let gate = PolicyPermissionGate::new(policy, recorder);

        let ctx = PermissionCheckContext {
            tool_use_id: Some("tu-77".into()),
            ..PermissionCheckContext::default()
        };
        let outcome = gate
            .check_with_context("Bash", &json!({"command": "rm -rf /"}), &ctx)
            .await;
        assert!(matches!(outcome, PermissionOutcome::Deny { .. }));

        let recorded = calls.lock().unwrap();
        assert_eq!(recorded.len(), 1, "on_permission_denied fired exactly once");
        let (name, tuid, drt, _dr, message) = &recorded[0];
        assert_eq!(name, "Bash");
        assert_eq!(tuid.as_deref(), Some("tu-77"));
        assert_eq!(drt.as_deref(), Some("rule")); // a deny RULE → type "rule"
        assert!(!message.is_empty());
    }

    #[tokio::test]
    async fn allow_does_not_fire_on_permission_denied() {
        // No rules, Default mode: a read-only tool allows → no deny notification.
        let policy = Arc::new(PermissionPolicy::from_rules(
            PermissionMode::Default,
            std::iter::empty(),
        ));
        let recorder = Arc::new(RecordingGate::default());
        let calls = recorder.denied.clone();
        let gate = PolicyPermissionGate::new(policy, recorder);
        let _ = gate
            .check_with_context("Read", &json!({}), &PermissionCheckContext::default())
            .await;
        assert!(calls.lock().unwrap().is_empty());
    }
}

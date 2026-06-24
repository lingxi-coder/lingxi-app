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

use crate::defaults_per_tool::tool_default;
use crate::gate::{
    PermissionCheckContext, PermissionDecision, PermissionDecisionSource, PermissionGate,
    PermissionOutcome, PermissionResolution, PromptDefault,
};
use crate::mode::PermissionMode;
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
    fn effective_authorize(&self, name: &str, input: &Value) -> PermissionResult {
        match *self.mode_override.read().unwrap_or_else(|e| e.into_inner()) {
            Some(mode) => self.policy.authorize_with_mode(name, input, mode),
            None => self.policy.authorize(name, input),
        }
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
        result: PermissionResult,
        name: &str,
        input: &Value,
    ) -> PermissionDecision {
        // No worker attribution (main turn loop / plan-mode path).
        self.decide_with_worker(result, name, input, None).await
    }

    /// Like [`Self::decide`], but forwards the originating-worker identity to the
    /// inner prompt transport on the Ask-delegate path so the prompt is
    /// attributed (claude-code's worker permission badge). `None` → identical to
    /// [`Self::decide`].
    async fn decide_with_worker(
        &self,
        result: PermissionResult,
        name: &str,
        input: &Value,
        worker: Option<crate::gate::PromptWorker>,
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
            PermissionResult::Ask { ref reason, .. } => {
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
                    self.inner.check_with_worker(name, input, worker).await
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
            PermissionResult::Ask { ref reason, .. } => {
                if read_only_default_auto_allows(name, reason) {
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
                    self.inner.check_with_context(name, input, &ctx2).await
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
    fn resolve(&self, result: PermissionResult, name: &str) -> PermissionResolution {
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
        self.decide(self.effective_authorize(name, input), name, input)
            .await
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
        self.decide_with_worker(self.effective_authorize(name, input), name, input, worker)
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
        self.decide_outcome_with_context(self.effective_authorize(name, input), name, input, ctx)
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
        match self.effective_authorize(name, input) {
            PermissionResult::Allow { .. } | PermissionResult::Ask { .. } => {
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
        self.resolve(self.effective_authorize(name, input), name)
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
    /// ([`PermissionRuleSource::claude_settings_source`]), matching the binary's
    /// `… from ${rule.source}.`.
    async fn agent_type_deny(&self, agent_type: &str) -> Option<String> {
        self.policy
            .agent_type_deny_source(agent_type)
            .map(|s| s.claude_settings_source().to_string())
    }

    /// Surface the wrapped policy's CONTENT-ful `Agent(<x>)` deny set so the
    /// advertised agent catalog and `Available agents:` lists exclude denied
    /// types (claude-code `Pxe`).
    async fn agent_deny_content_types(&self) -> Vec<String> {
        self.policy.agent_deny_content_types()
    }

    /// Apply a LIVE `set_permission_mode` override (claude-code
    /// `handleSetPermissionMode`). Parses the wire string, rejects an unknown
    /// mode and a `bypassPermissions` request when the killswitch is active, and
    /// stores the override read by [`Self::effective_authorize`].
    async fn set_permission_mode(&self, mode: &str) -> Result<(), String> {
        let parsed = parse_settable_mode(mode)?;
        if parsed == PermissionMode::BypassPermissions && self.policy.bypass_killswitch_active {
            return Err("Bypass permissions mode was disabled by settings".to_string());
        }
        *self.mode_override.write().unwrap_or_else(|e| e.into_inner()) = Some(parsed);
        Ok(())
    }
}

/// Parse a `set_permission_mode` wire string into a [`PermissionMode`].
///
/// Accepts the five external modes plus the internal `auto` (the binary's
/// settable set is `default`/`plan`/`acceptEdits`/`bypassPermissions`/`dontAsk`/
/// `auto`). Unlike [`crate::cli_mode::permission_mode_from_cli_string`] (which
/// silently coerces unknown → `Default` for settings/CLI parsing), this REJECTS
/// an unknown mode so the control handler can return an error frame.
fn parse_settable_mode(s: &str) -> Result<PermissionMode, String> {
    match s {
        "default" => Ok(PermissionMode::Default),
        "plan" => Ok(PermissionMode::Plan),
        "acceptEdits" => Ok(PermissionMode::AcceptEdits),
        "bypassPermissions" => Ok(PermissionMode::BypassPermissions),
        "dontAsk" => Ok(PermissionMode::DontAsk),
        "auto" => Ok(PermissionMode::Auto),
        other => Err(format!("Invalid permission mode: {other}")),
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
            "Permission to use {tool_name} has been denied because Claude Code is running in don't ask mode. {DENIAL_WORKAROUND_GUIDANCE}"
        ),
        _ => format!("Permission to use {tool_name} has been denied."),
    }
}

/// Shared guidance appended to certain permission denials (`messages.ts:226`),
/// instructing the model on acceptable workarounds.
const DENIAL_WORKAROUND_GUIDANCE: &str = "IMPORTANT: You *may* attempt to accomplish this action using other tools that might naturally be used to accomplish this goal, e.g. using head instead of cat. But you *should not* attempt to work around this denial in malicious ways, e.g. do not use your ability to run tests to execute non-test actions. You should only try to work around this restriction in reasonable ways that do not attempt to bypass the intent behind this denial. If you believe this capability is essential to complete the user's request, STOP and explain to the user what you were trying to do and why you need this permission. Let the user decide how to proceed.";

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
            format!("Permission to use Edit has been denied because Claude Code is running in don't ask mode. {DENIAL_WORKAROUND_GUIDANCE}")
        );
        // The guidance text itself is byte-locked.
        assert!(DENIAL_WORKAROUND_GUIDANCE.starts_with("IMPORTANT: You *may* attempt to accomplish this action using other tools"));
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
            gate.resolve_detailed("Read", &serde_json::json!({ "file_path": "./secrets/key.pem" }))
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
        let seen = inner.worker.lock().unwrap().clone().expect("inner consulted");
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
        assert_eq!(inner.calls(), 1, "non-read-only ask delegates to the inner gate");
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
        let policy2 =
            policy_with(r#"{ "permissions": { "allow": ["Bash"] } }"#, PermissionMode::Default);
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
        let policy = policy_with(r#"{ "permissions": { "deny": ["Bash"] } }"#, PermissionMode::Default);
        let gate = PolicyPermissionGate::new(
            policy,
            RecordingInner::new(PermissionDecision::Allow),
        );
        assert!(
            matches!(
                gate.check_after_hook_allow("Bash", &serde_json::json!({})).await,
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
            gate2.check_after_hook_allow("Bash", &serde_json::json!({})).await,
            PermissionDecision::Allow,
            "hook 'allow' skips the prompt for an un-ruled mutating tool"
        );
        assert_eq!(inner.calls(), 0, "hook 'allow' must NOT delegate to the prompt");

        // An explicit allow rule → Allow.
        let policy3 =
            policy_with(r#"{ "permissions": { "allow": ["Bash"] } }"#, PermissionMode::Default);
        let gate3 = PolicyPermissionGate::new(policy3, RecordingInner::new(PermissionDecision::Allow));
        assert_eq!(
            gate3.check_after_hook_allow("Bash", &serde_json::json!({})).await,
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
            gate.check_after_hook_allow("Bash", &serde_json::json!({})).await,
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
            gate.check_in_plan_mode("Read", &serde_json::json!({})).await,
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
            gate.check_in_plan_mode("Edit", &serde_json::json!({})).await,
            PermissionDecision::Deny { .. }
        ));
        assert_eq!(inner.calls(), 0, "deny rule binds under plan mode, no prompt");
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
            gate.check_in_plan_mode("Edit", &serde_json::json!({})).await,
            PermissionDecision::Allow
        );
        assert_eq!(inner.calls(), 0, "allow rule wins under plan mode, no prompt");
    }

    #[tokio::test]
    async fn default_check_in_plan_mode_delegates_to_check() {
        // A rule-less gate (no mode layer) has nothing extra to enforce under plan
        // mode, so the default impl just delegates to check().
        let gate = RecordingInner::new(PermissionDecision::Allow);
        assert_eq!(
            gate.check_in_plan_mode("Edit", &serde_json::json!({})).await,
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
        assert_eq!(inner.calls(), 0, "resolve_detailed never consults the inner");
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
            PermissionOutcome::Allow { updated_input: None, permission_updates: Vec::new() }
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
        assert_eq!(outcome, PermissionOutcome::Allow { updated_input: None, permission_updates: Vec::new() });
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

    // ── set_permission_mode (live mode override) ─────────────────────────────

    #[tokio::test]
    async fn set_permission_mode_override_changes_authorize_outcome() {
        // No rules, boot mode Default: a mutating tool with no allow rule is an
        // Ask → delegates to the inner prompt transport (here: Deny).
        let policy = policy_with(r#"{ "permissions": {} }"#, PermissionMode::Default);
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
    async fn set_permission_mode_rejects_unknown_mode() {
        let policy = policy_with(r#"{ "permissions": {} }"#, PermissionMode::Default);
        let gate = PolicyPermissionGate::new(policy, RecordingInner::new(PermissionDecision::Allow));
        assert_eq!(
            gate.set_permission_mode("nonsense").await.unwrap_err(),
            "Invalid permission mode: nonsense"
        );
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
            "Bypass permissions mode was disabled by settings"
        );
    }

    #[test]
    fn parse_settable_mode_accepts_six_modes_rejects_unknown() {
        assert_eq!(parse_settable_mode("default"), Ok(PermissionMode::Default));
        assert_eq!(parse_settable_mode("plan"), Ok(PermissionMode::Plan));
        assert_eq!(
            parse_settable_mode("acceptEdits"),
            Ok(PermissionMode::AcceptEdits)
        );
        assert_eq!(
            parse_settable_mode("bypassPermissions"),
            Ok(PermissionMode::BypassPermissions)
        );
        assert_eq!(parse_settable_mode("dontAsk"), Ok(PermissionMode::DontAsk));
        assert_eq!(parse_settable_mode("auto"), Ok(PermissionMode::Auto));
        assert!(parse_settable_mode("bubble").is_err());
    }
}

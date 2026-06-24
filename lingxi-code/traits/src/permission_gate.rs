//! `PermissionGate` trait — promoted from the M5-02 in-orchestrator local.
//!
//! This is the workspace-wide authoritative trait. `lingxi-permission` and
//! `lingxi-orchestrator` both re-export it. The orchestrator's old
//! `pub trait PermissionGate` in `test_support.rs` is now a `pub use`
//! re-export of this type — see M5-05 Task 2.
//!
//! **Plan deviation (M5-05):** The plan called for `check` to return
//! `Result<PermissionDecision, PermErr>`. The current orchestrator + M5-02
//! `NoOpPermissionGate` use the simpler `-> PermissionDecision` signature.
//! Keeping the simpler signature avoids touching every M5-02 / M5-04 call
//! site; the interactive gate (lingxi-permission) absorbs its own IO /
//! retry errors into [`PermissionDecision::Deny`].
#![forbid(unsafe_code)]

use async_trait::async_trait;
use protocol::ContentBlock;
use serde_json::Value;

/// Identity of the SUBAGENT / in-process-teammate worker a permission prompt is
/// being raised on behalf of, so the prompt UI can ATTRIBUTE it.
///
/// claude-code 2.1.186 surfaces a background worker's permission prompt in the
/// main session attributed to the asking agent (`${agent_id} needs permission
/// for ${tool_name}` + the `● @name` worker badge). Threaded from
/// [`crate::tool_invoker::SubagentInvocationContext`] through the dispatch
/// invoker into the prompt-building gate. `None` for a main-thread / leader
/// tool call (the turn loop's own [`PermissionGate::check`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptWorker {
    /// Worker DISPLAY name (claude-code `getAgentName()` — e.g. `"researcher"`).
    pub name: String,
    /// Team the worker belongs to (claude-code `getTeammateContext()?.teamName`).
    pub team: Option<String>,
    /// Whether the worker runs ASYNC (backgrounded).
    pub is_async: bool,
}

/// Outcome of a [`PermissionGate::check`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionDecision {
    /// The tool call is permitted.
    Allow,
    /// The tool call is rejected.
    Deny {
        /// Reason surfaced to the model as the `tool_result`.
        reason: String,
    },
}

/// Extra inputs for the tool-dispatch permission check beyond `name` + `input`,
/// so a stdio `can_use_tool` control_request can be byte-faithful and an allow
/// can carry the host's `updatedInput` rewrite.
///
/// Threaded from the orchestrator's tool dispatch into
/// [`PermissionGate::check_with_context`]. All fields are optional; the default
/// (`PermissionCheckContext::default()`) behaves exactly like
/// [`PermissionGate::check_with_worker`] with no worker.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PermissionCheckContext {
    /// Subagent/teammate worker attribution (as in [`PermissionGate::check_with_worker`]).
    pub worker: Option<PromptWorker>,
    /// The assistant message's `tool_use` block id this check is for — the REAL
    /// id a stdio `can_use_tool` request should carry (claude-code
    /// `createCanUseTool(toolUseID)`), so the host can correlate + dedup. `None`
    /// ⇒ the gate mints a fresh id.
    pub tool_use_id: Option<String>,
    /// The policy Ask's human-readable decision reason, forwarded as the
    /// `decision_reason` field of a stdio `can_use_tool` request. `None` ⇒
    /// omitted. (Suggestions / blocked_path are not yet surfaced through the
    /// resolution seam; tracked as a follow-up.)
    pub decision_reason: Option<String>,
}

/// Richer outcome of [`PermissionGate::check_with_context`]: an allow may carry
/// the host/policy-rewritten tool input (`updatedInput`) the dispatcher should
/// run the tool with instead of the original.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionOutcome {
    /// Permitted. `updated_input` is the rewritten input to use instead of the
    /// original (claude-code `updatedInput` when it has keys), or `None` to keep
    /// the original.
    Allow {
        /// Host/policy-rewritten tool input, or `None` to keep the original.
        updated_input: Option<Value>,
    },
    /// Rejected, with the reason surfaced to the model as the `tool_result`.
    Deny {
        /// Reason surfaced to the model.
        reason: String,
    },
}

/// What produced a [`PermissionGate`] decision — lets the turn loop fire the
/// source-gated permission hooks the way claude-code does: `PermissionRequest`
/// on an about-to-ask, `PermissionDenied` ONLY on an auto-mode classifier deny
/// (`toolExecution.ts:1075`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionDecisionSource {
    /// An explicit allow/deny/ask RULE matched (claude-code decisionReason 'rule').
    Rule,
    /// The active permission MODE produced the decision (no rule matched).
    Mode,
    /// An auto-mode CLASSIFIER produced the decision (claude-code 'classifier').
    /// `PermissionDenied` hooks fire only on a deny from this source.
    Classifier,
    /// No richer source is available — a gate with no rule/mode layer, or a
    /// decision class that carries no distinguishable source.
    Unspecified,
}

/// A [`PermissionGate`] resolution that carries its [`PermissionDecisionSource`]
/// and distinguishes an about-to-ASK (which the turn loop surfaces to a
/// `PermissionRequest` hook BEFORE delegating to the prompt transport) from a
/// resolved allow/deny. Returned by [`PermissionGate::resolve_detailed`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionResolution {
    /// Permitted outright (rule/mode allow, or a read-only auto-allow).
    Allow,
    /// Rejected, with the rendered deny reason and its source.
    Deny {
        /// Reason surfaced to the model as the `tool_result`.
        reason: String,
        /// Where the denial came from (gates the `PermissionDenied` hook).
        source: PermissionDecisionSource,
        /// `true` when the underlying `permissionDecision.behavior === 'ask'`
        /// (a rejection that came from an ASK prompt the user declined), vs a
        /// rule/mode `deny`. claude-code only appends the rejection's
        /// `contentBlocks` to the outgoing user message when the behavior is
        /// `ask` (`toolExecution.ts:1040-1043`), so the turn loop branches on
        /// this. `false` for every existing producer (rule/mode/classifier
        /// denies and the default `resolve_detailed`), keeping the common deny
        /// path byte-identical.
        behavior_ask: bool,
        /// Image (or other non-text) content blocks the `ask`-behavior
        /// rejection supplied — `permissionDecision.contentBlocks`
        /// (`toolExecution.ts:1040`). claude-code appends these at the TOP LEVEL
        /// of the deny user message (alongside, NOT inside, the text-only
        /// `tool_result`, which rejects non-text when `is_error` is set). EMPTY
        /// for every existing producer; only consulted when
        /// [`Self::Deny::behavior_ask`] is also `true`. Dormant in the public
        /// build (no gate supplies them), faithful to claude-code where the
        /// `ask`+contentBlocks rejection path is itself gated.
        content_blocks: Vec<ContentBlock>,
    },
    /// The gate would PROMPT for this call — delegate to its inner transport in
    /// interactive mode, or auto-deny it in headless. The turn loop fires the
    /// `PermissionRequest` hook here before resolving via the transport.
    Ask,
}

/// The workspace-wide authorization gate consulted before every tool dispatch.
///
/// M5-02 introduced this trait as a `pub` item inside
/// `lingxi-orchestrator::test_support`. M5-05 promotes it to the traits
/// crate so that `lingxi-permission` can carry the real
/// `InteractivePromptingGate` impl without a circular dep.
#[async_trait]
pub trait PermissionGate: Send + Sync {
    /// Authorize a tool call by `name` with `input`.
    ///
    /// Implementations may consult policy rules, prompt the user, or run
    /// permission classifiers. All error paths are folded into
    /// [`PermissionDecision::Deny`] — the caller never has to handle an
    /// error tier.
    async fn check(&self, name: &str, input: &Value) -> PermissionDecision;

    /// Like [`Self::check`], but carrying the identity of the SUBAGENT/teammate
    /// worker the call originates from so a prompt-building gate can ATTRIBUTE
    /// the prompt to that worker — claude-code 2.1.186 surfaces a background
    /// worker's permission prompt in the main session with a `● @name` badge
    /// (`${agent_id} needs permission for ${tool_name}`). The subagent dispatch
    /// invoker ([`crate::tool_invoker::ToolInvoker`]) calls this; the main turn
    /// loop uses [`Self::check`] (no worker).
    ///
    /// Additive DEFAULTED (frozen-trait safe): the default IGNORES `worker` and
    /// delegates to [`Self::check`], so every existing impl is unchanged. Only
    /// the prompt-building gates (`TuiPermissionGate` / `AdapterPermissionGate`)
    /// and the wrapping `PolicyPermissionGate` (which forwards it to its inner
    /// transport) OVERRIDE it.
    async fn check_with_worker(
        &self,
        name: &str,
        input: &Value,
        worker: Option<PromptWorker>,
    ) -> PermissionDecision {
        let _ = worker;
        self.check(name, input).await
    }

    /// Like [`Self::check_with_worker`], but carrying a [`PermissionCheckContext`]
    /// (real `tool_use_id`, decision reason) and returning a [`PermissionOutcome`]
    /// that may carry the host's `updatedInput` rewrite.
    ///
    /// The tool-dispatch path calls this so the stdio `can_use_tool` gate can
    /// emit a byte-faithful request (real tool_use_id) and apply the host's
    /// rewritten input. The default IGNORES the extra context and delegates to
    /// [`Self::check_with_worker`], mapping `Allow`→`Allow{updated_input:None}`
    /// — so every existing impl is unchanged (frozen-trait safe). Only
    /// `StdioControlPermissionGate` (the stdio transport) and the wrapping
    /// `PolicyPermissionGate` OVERRIDE it.
    async fn check_with_context(
        &self,
        name: &str,
        input: &Value,
        ctx: &PermissionCheckContext,
    ) -> PermissionOutcome {
        match self.check_with_worker(name, input, ctx.worker.clone()).await {
            PermissionDecision::Allow => PermissionOutcome::Allow { updated_input: None },
            PermissionDecision::Deny { reason } => PermissionOutcome::Deny { reason },
        }
    }

    /// Resolve permission when a `PreToolUse` / `PermissionRequest` hook has
    /// already returned `allow` (`HookDecision::Approve`).
    ///
    /// claude-code's `resolveHookPermissionDecision`: a hook `allow` skips the
    /// interactive PROMPT but still applies rule-based deny/ask
    /// (`checkRuleBasedPermissions`) — a hook cannot override an explicit deny
    /// rule. The default impl treats a hook `allow` as a wholesale bypass
    /// ([`PermissionDecision::Allow`]), which is correct for gates that carry no
    /// rule layer (the interactive / no-op / adapter prompt transports — they
    /// have nothing to deny). A rule-evaluating gate (the `PolicyPermissionGate`)
    /// OVERRIDES this to keep enforcing deny rules while skipping the prompt.
    ///
    /// Additive DEFAULTED method (frozen-trait safe): every existing impl keeps
    /// the prior wholesale-bypass behavior unless it opts in.
    async fn check_after_hook_allow(&self, name: &str, input: &Value) -> PermissionDecision {
        let _ = (name, input);
        PermissionDecision::Allow
    }

    /// Resolve permission when the session is in PLAN mode — i.e. the model has
    /// run `EnterPlanMode` and not yet exited.
    ///
    /// claude-code reads `toolPermissionContext.mode = 'plan'` LIVE on every
    /// permission check, so entering plan mode immediately activates the
    /// mutation backstop (plan-safe reads stay frictionless; un-ruled mutations
    /// are asked/denied). LingXi instead builds its `PermissionPolicy` once at
    /// boot with a fixed mode and holds it behind a shared `Arc`, so the boot
    /// mode would otherwise ignore a runtime `EnterPlanMode`. This method is the
    /// seam the turn loop calls (instead of [`Self::check`]) whenever the live
    /// `SessionState.plan_mode` flag is set.
    ///
    /// The default impl delegates to [`Self::check`]: a gate with no rule/mode
    /// layer (the interactive / no-op / adapter prompt transports) has nothing
    /// extra to enforce under plan mode, so it behaves identically. The
    /// rule-evaluating `PolicyPermissionGate` OVERRIDES this to authorize under
    /// [`crate`]'s `PermissionMode::Plan` via `authorize_with_mode`.
    ///
    /// Additive DEFAULTED method (frozen-trait safe): every existing impl keeps
    /// the prior behavior unless it opts in. Mirrors
    /// [`Self::check_after_hook_allow`].
    async fn check_in_plan_mode(&self, name: &str, input: &Value) -> PermissionDecision {
        self.check(name, input).await
    }

    /// Resolve a tool call to a SOURCED [`PermissionResolution`] WITHOUT yet
    /// consulting the inner prompt transport.
    ///
    /// This lets the turn loop fire the source-gated permission hooks the way
    /// claude-code does: `PermissionRequest` on an [`PermissionResolution::Ask`]
    /// (before the prompt), and `PermissionDenied` only on a
    /// [`PermissionResolution::Deny`] whose source is
    /// [`PermissionDecisionSource::Classifier`] (`toolExecution.ts:1075`).
    ///
    /// The default impl derives from [`Self::check`] — `Allow` → `Allow`, `Deny`
    /// → `Deny { source: Unspecified }` — so a gate with NO rule/source layer
    /// (the no-op / adapter transports that the turn loop uses directly when
    /// enforcement is off) never yields an `Ask` here; it has already resolved.
    /// The rule-evaluating `PolicyPermissionGate` OVERRIDES this to authorize
    /// WITHOUT delegating, returning the rule/mode/classifier source and an `Ask`
    /// for a would-be prompt. (Inner prompt transports — interactive / TUI — are
    /// never the turn loop's gate directly; they sit behind the policy gate, so
    /// the default's `check` call never triggers a prompt for the real gate
    /// types.) Additive DEFAULTED (frozen-trait safe).
    async fn resolve_detailed(&self, name: &str, input: &Value) -> PermissionResolution {
        match self.check(name, input).await {
            PermissionDecision::Allow => PermissionResolution::Allow,
            PermissionDecision::Deny { reason } => PermissionResolution::Deny {
                reason,
                source: PermissionDecisionSource::Unspecified,
                behavior_ask: false,
                content_blocks: Vec::new(),
            },
        }
    }

    /// Tool-name targets of every TOOL-WIDE deny rule the gate enforces (a deny
    /// rule with NO rule-content — a blanket strip of that tool).
    ///
    /// The orchestrator filters the wire `tools` array by these names BEFORE the
    /// model sees them, 1:1 with claude-code `filterToolsByDenyRules` /
    /// `getDenyRuleForTool` (`tools.ts:262-269`): a tool whose name matches one of
    /// these (exact, OR an `mcp__server` prefix covering `mcp__server__tool`) is
    /// dropped from the advertised set. Each returned string is matched against an
    /// advertised tool's name by the SAME matcher the runtime check uses
    /// (`permission::tool_wide_name_matches`).
    ///
    /// The default returns `Vec::new()` — a gate with no rule layer (the
    /// interactive / no-op / adapter prompt transports) denies nothing tool-wide,
    /// so the wire-tool set is UNCHANGED (byte-identical / regression-safe). Only
    /// the rule-evaluating `PolicyPermissionGate` OVERRIDES this to surface its
    /// policy's tool-wide deny names. Additive DEFAULTED (frozen-trait safe).
    async fn tool_wide_deny_names(&self) -> Vec<String> {
        Vec::new()
    }

    /// If `Agent(<agent_type>)` is DENIED by a permission rule, the rule's source
    /// identifier (claude-code `SettingSource` raw string — e.g. `localSettings`,
    /// `projectSettings`, `cliArg`), else `None`.
    ///
    /// 1:1 with claude-code `getDenyRuleForAgent` (`o5e(ctx, "Agent", type)`):
    /// finds a DENY rule whose `toolName === "Agent"` and whose `ruleContent`
    /// equals the agent type exactly. The Agent tool uses this to reject a model
    /// selection of a denied subagent type with the byte-exact message
    /// `Agent type '<t>' has been denied by permission rule 'Agent(<t>)' from
    /// <source>.` (`AgentTypeError`). The deny rule keys on the `"Agent"` tool
    /// name even when invoked via the legacy `Task` alias.
    ///
    /// The default returns `None` — a gate with no rule layer denies no agent
    /// type. Only `PolicyPermissionGate` OVERRIDES it. Additive DEFAULTED
    /// (frozen-trait safe).
    async fn agent_type_deny(&self, agent_type: &str) -> Option<String> {
        let _ = agent_type;
        None
    }

    /// The set of agent-type names that are denied by a CONTENT-ful `Agent(<x>)`
    /// deny rule — the listing-filter set.
    ///
    /// 1:1 with claude-code `Pxe(list, ctx, "Agent")`: collects every DENY rule
    /// whose `toolName === "Agent"` and that carries a (non-undefined)
    /// `ruleContent`, so the advertised agent catalog and the `Available agents:`
    /// error lists exclude denied types — the 2.1.186 Agent(type)-restriction
    /// change that filters the prompt/list the model sees.
    ///
    /// The default returns `Vec::new()` — no agent types filtered. Only
    /// `PolicyPermissionGate` OVERRIDES it. Additive DEFAULTED (frozen-trait
    /// safe).
    async fn agent_deny_content_types(&self) -> Vec<String> {
        Vec::new()
    }

    /// Set the LIVE session permission mode (claude-code `set_permission_mode`
    /// control_request / `handleSetPermissionMode`). `mode` is the wire string
    /// (`default`/`plan`/`acceptEdits`/`bypassPermissions`/`dontAsk`/`auto`); the
    /// gate parses + validates it and applies it to subsequent checks.
    ///
    /// Returns `Ok(())` on success, `Err(msg)` for an invalid or disallowed mode
    /// (e.g. `bypassPermissions` disabled by settings). The mode is passed as a
    /// `&str` rather than a `PermissionMode` because this crate is below
    /// `lingxi-permission` in the dependency graph and cannot name that enum.
    ///
    /// The default is a no-op `Ok(())`: a gate with no rule/mode layer (the
    /// interactive / no-op / adapter prompt transports) has no mode to mutate.
    /// Only the rule-evaluating `PolicyPermissionGate` OVERRIDES this with an
    /// interior-mutable mode cell read per check. Additive DEFAULTED
    /// (frozen-trait safe).
    async fn set_permission_mode(&self, mode: &str) -> Result<(), String> {
        let _ = mode;
        Ok(())
    }
}

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
use std::path::Path;

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

/// Terminal permission outcome for a prompt-avoiding agent session.
///
/// Unlike [`PermissionDecision::Deny`], this stops the owning agent loop rather
/// than producing a recoverable tool-result denial. The policy gate emits it
/// only when the auto-mode classifier denial breaker trips in a non-interactive
/// context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionAbort {
    /// Byte-exact terminal message surfaced by the owning agent or turn loop.
    pub message: String,
}

/// Synchronous, prompt-free authorization used while expanding embedded shell
/// commands from slash-command/skill prompt text. `None` from the trait method
/// means the gate has no local rule layer and the caller should use its static
/// fallback policy; a rule-evaluating gate returns one of these decisions from
/// its live mode/rule snapshot without consulting an interactive prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NonInteractivePermissionDecision {
    /// The command is permitted.
    Allow,
    /// The command is rejected. The policy may intentionally omit explanatory
    /// text, matching `PermissionResult::Deny::explanation`.
    Deny {
        /// Optional user-facing denial reason.
        reason: Option<String>,
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
    /// omitted (claude-code `serializeDecisionReason` returns `undefined` for
    /// rule/mode/subcommandResults/permissionPromptTool, so the common ask
    /// omits it). Populated by the `PolicyPermissionGate` Ask path from the
    /// policy result's [`PermissionDecisionReason`].
    pub decision_reason: Option<String>,
    /// The policy Ask's decision-reason TYPE discriminant, forwarded as the
    /// `decision_reason_type` field of a stdio `can_use_tool` request (claude-code
    /// `decision_reason_type: decisionReason?.type` — `rule`/`mode`/
    /// `subcommandResults`/`permissionPromptTool`/`hook`/`asyncAgent`/`workingDir`/
    /// `safetyCheck`/`sandboxOverride`/`classifier`/`other`). An SDK host reads
    /// this to classify Ask reasons where the free-text [`Self::decision_reason`]
    /// is omitted. `None` ⇒ the key is OMITTED. Populated by the
    /// `PolicyPermissionGate` Ask path alongside `decision_reason`.
    pub decision_reason_type: Option<String>,
    /// Whether every safety-check reason contributing to this Ask may be
    /// approved by the automated classifier.  `None` means the decision has no
    /// safety-check reason, so the stdio key is omitted.  For composite
    /// subcommand reasons the policy gate sets this to `false` if any nested
    /// safety check is not classifier-approvable.
    pub classifier_approvable: Option<bool>,
    /// Whether the tool itself requires interaction while it runs (for example
    /// `AskUserQuestion`).  The tool registry is the authoritative producer;
    /// the stdio transport emits the key only when this is `true`, matching the
    /// upstream `requiresUserInteraction?.() || undefined` shape.
    pub requires_user_interaction: bool,
    /// The explicit Ask rule that matched this call, if the policy result
    /// retains one.  Kept transport-neutral here because this crate sits below
    /// the permission rule implementation.
    pub matched_ask_rule: Option<MatchedAskRule>,
    /// `true` when a PreToolUse hook returned `ask`, establishing claude-code's
    /// `hookAskFloor` (BIN 224697675/225722419). With the floor set, an Auto-mode
    /// classifier ALLOW must NOT silently defeat the hook's ask — the ask is kept
    /// (prompts interactively, denies headless), matching CC's floor
    /// (`if(hookAskFloor && !interactive && shouldAvoidPermissionPrompts) return
    /// asyncAgent-deny; else the classifier callback preserves behavior "ask"`).
    /// The 2.1.207 classifier-allow-over-hook-ask behavior 2.1.211/215 removed.
    /// Additive default `false`, so a non-hook-ask check is unaffected.
    pub hook_ask_floor: bool,
    /// The policy Ask's permission-rule SUGGESTIONS, forwarded as the
    /// `permission_suggestions` field of a stdio `can_use_tool` request
    /// (claude-code `mainPermissionResult.suggestions` — a `PermissionUpdate[]`).
    /// Carried as the RAW wire array so this crate need not name
    /// `lingxi-permission`'s `PermissionUpdate` (dep direction, see the note at
    /// the bottom of this file). `None` ⇒ the key is OMITTED from the request.
    ///
    /// Producers populate only suggestions they can derive without widening a
    /// rule (currently the exact matched-Ask-rule session update). Tool-specific
    /// suggestion builders may leave this absent.
    pub permission_suggestions: Option<Value>,
    /// The policy Ask's BLOCKED PATH, forwarded as the `blocked_path` field of a
    /// stdio `can_use_tool` request (claude-code `mainPermissionResult.blockedPath`
    /// — the filesystem path a path-scoped ask is gated on). `None` ⇒ the key is
    /// OMITTED.
    ///
    /// Path validators populate this only when they retain a structured concrete
    /// path; non-path and expansion-only asks leave it absent.
    pub blocked_path: Option<String>,
    /// A PER-CALL permission mode OVERRIDE as a WIRE string
    /// (`default`/`plan`/`acceptEdits`/`bypassPermissions`/`dontAsk`/`auto`).
    /// `Some` ⇒ the rule-evaluating gate authorizes THIS call under that mode
    /// instead of its live/boot mode — the seam a spawned subagent uses to run its
    /// tool dispatch under a clamped spawn mode (claude-code 2.1.207 Agent `mode` →
    /// the child's `toolPermissionContext.mode`, threaded from
    /// [`crate::tool_invoker::SubagentInvocationContext::mode_override`]). `None` ⇒
    /// the gate's live/boot mode applies (main loop / no override), byte-identical
    /// to before. Only the rule-evaluating `PolicyPermissionGate` consults it;
    /// other transports ignore it.
    pub mode_override: Option<String>,
    /// Whether the owning agent session cannot surface permission prompts.
    ///
    /// This is the per-dispatch equivalent of Claude Code's
    /// `shouldAvoidPermissionPrompts`. It is deliberately carried on the call
    /// context rather than stored process-globally so synchronous and async
    /// subagents can inherit the correct owner semantics without races.
    pub is_non_interactive_session: bool,
    /// Ephemeral local-app workspace lease used to bind automatic filesystem
    /// authorization to the workflow that owns the call. `None` for ordinary
    /// session and main-loop dispatches.
    pub workspace_lease_token: Option<u64>,
}

/// Wire-neutral description of a matched permission Ask rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchedAskRule {
    /// Raw settings source (`userSettings`, `projectSettings`, …).
    pub source: String,
    /// Canonical tool name stored in the rule.
    pub tool_name: String,
    /// Optional rule-specific content (command/path/domain pattern).
    pub rule_content: Option<String>,
}

/// Host-provided classification for a resolved interactive tool decision.
///
/// This is telemetry metadata only: it never changes whether the tool runs.
/// Unknown wire values are ignored by transports and therefore fall back to
/// the existing temporary/reject classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolDecisionClassification {
    /// Allowed for this invocation/session only.
    UserTemporary,
    /// Allowed and persisted by the host.
    UserPermanent,
    /// Explicitly rejected by the user.
    UserReject,
}

impl ToolDecisionClassification {
    /// Stable OTEL label used by Claude Code's `tool_decision` event.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UserTemporary => "user_temporary",
            Self::UserPermanent => "user_permanent",
            Self::UserReject => "user_reject",
        }
    }
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
        /// The host's `updatedPermissions` payload from a `can_use_tool` ALLOW
        /// response — the RAW wire array of permission-rule updates the host wants
        /// applied + persisted (claude-code `applyPermissionUpdates` +
        /// `persistPermissionUpdates`, fired in
        /// `permissionPromptToolResultToPermissionDecision`). Each element is a
        /// `permissionUpdateSchema` discriminated union (`addRules`,
        /// `replaceRules`, `removeRules`, `setMode`, and directory updates are
        /// supported). EMPTY for every gate but
        /// `StdioControlPermissionGate`. The value is a raw `serde_json::Value`
        /// (not the typed `permission::PermissionUpdate`) because this crate sits
        /// BELOW `lingxi-permission` in the dependency graph and cannot name that
        /// type — the permission-aware consumer parses + applies it.
        permission_updates: Vec<Value>,
        /// Optional host classification for telemetry attribution.
        decision_classification: Option<ToolDecisionClassification>,
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
    Allow {
        /// Raw `SettingSource` token of the RULE that produced this allow
        /// (`userSettings`, `localSettings`, `session`, …), or `None` when no
        /// rule matched (a mode allow, a read-only auto-allow, or a gate with no
        /// rule layer). Feeds the OTEL decision-source label — claude-code
        /// `ZX_(decisionReason.rule.source, behavior)`.
        rule_source: Option<String>,
    },
    /// Rejected, with the rendered deny reason and its source.
    Deny {
        /// Reason surfaced to the model as the `tool_result`.
        reason: String,
        /// Where the denial came from (gates the `PermissionDenied` hook).
        source: PermissionDecisionSource,
        /// Raw `SettingSource` token of the matched deny RULE, mirroring
        /// [`Self::Allow::rule_source`]. `None` for every non-rule denial.
        rule_source: Option<String>,
        /// GATE-SYSMSG-01: the discriminated reason kind
        /// (`decisionReason?.type` — `rule`/`mode`/`safetyCheck`/…), pre-computed
        /// where the full [`crate`]-external `PermissionDecisionReason` is in scope
        /// (that type lives in the `permission` crate, which depends on this one,
        /// so it cannot be a field here). Feeds the `permission_denied` system
        /// message the turn loop emits on the main-conversation deny path. `None`
        /// for producers without a structured reason.
        decision_reason_type: Option<String>,
        /// GATE-SYSMSG-01: the `oin(decisionReason)`-filtered reason TEXT (only
        /// the free-text reason kinds surface it). Pre-computed alongside
        /// [`Self::Deny::decision_reason_type`]; `None` otherwise.
        decision_reason: Option<String>,
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
    /// The gate would prompt, carrying the classifier breaker context that
    /// must be forwarded to the prompt transport without a global side-channel.
    /// This is emitted only for the interactive auto-mode denial-limit fallback.
    AskWithContext {
        /// Discriminant sent as `decision_reason_type`.
        decision_reason_type: Option<String>,
        /// Free-text reason sent as `decision_reason`.
        decision_reason: Option<String>,
    },
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

    /// Resolve a tool call synchronously and without prompting, after injecting
    /// transient allow rules such as a prompt command's frontmatter
    /// `allowed-tools`. This is the live-policy seam used by embedded `!cmd``
    /// expansion: an unresolved `Ask` is returned as `Deny`, exactly as Claude
    /// Code's prompt-shell path rejects every non-allow decision.
    ///
    /// The default returns `None`, preserving gates that are only prompt
    /// transports or no-ops; callers then evaluate their construction-time
    /// fallback policy. `PolicyPermissionGate` overrides this and reads its live
    /// mode plus `updatedPermissions` overlay on every call.
    fn check_noninteractive_with_allow_rules(
        &self,
        name: &str,
        input: &Value,
        transient_allow_rules: &[String],
    ) -> Option<NonInteractivePermissionDecision> {
        let _ = (name, input, transient_allow_rules);
        None
    }

    /// Return the current `Read`-deny patterns rebased to `cwd` for search
    /// result filtering. `None` means this gate has no live policy layer and
    /// the caller should use its construction-time fallback.
    ///
    /// `PolicyPermissionGate` overrides this so `updatedPermissions` changes
    /// take effect in `Glob`/`Grep` on the next call, matching Claude Code's
    /// per-call read of `toolPermissionContext`.
    fn read_deny_exclude_globs(&self, cwd: &Path) -> Option<Vec<String>> {
        let _ = cwd;
        None
    }

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
        match self
            .check_with_worker(name, input, ctx.worker.clone())
            .await
        {
            PermissionDecision::Allow => PermissionOutcome::Allow {
                updated_input: None,
                permission_updates: Vec::new(),
                decision_classification: None,
            },
            PermissionDecision::Deny { reason } => PermissionOutcome::Deny { reason },
        }
    }

    /// Like [`Self::check_with_context`], but preserves a terminal
    /// [`PermissionAbort`] instead of folding every failure into a recoverable
    /// [`PermissionOutcome::Deny`].
    ///
    /// The default keeps existing gates source-compatible and never aborts.
    /// Rule-evaluating gates override this only for prompt-avoiding auto-mode
    /// denial-breaker trips.
    async fn check_with_context_or_abort(
        &self,
        name: &str,
        input: &Value,
        ctx: &PermissionCheckContext,
    ) -> Result<PermissionOutcome, PermissionAbort> {
        Ok(self.check_with_context(name, input, ctx).await)
    }

    /// Apply permission-context updates to the live in-memory gate state.
    ///
    /// The default is a no-op for prompt-only transports. A rule-evaluating
    /// gate overrides this so normal and orphaned permission responses update
    /// the same live context.
    fn apply_permission_updates(&self, _updates: &[Value]) {}

    /// Persist permission updates through the transport that owns the settings
    /// roots. This is separate from live application because an outer policy
    /// gate owns the former while its inner stdio transport owns the latter.
    async fn persist_permission_updates(&self, _updates: &[Value]) {}

    /// Enable or disable permission persistence for this prompt transport.
    /// Managed policy calls this with `false` when only centrally managed rules
    /// are permitted. The default is a no-op for transports without persistence.
    fn set_permission_persistence_enabled(&self, _enabled: bool) {}

    /// GATE-SYSMSG-01: notify the transport that a tool call was DENIED by the
    /// local policy pre-check, so a stdio/SDK transport can emit a
    /// `permission_denied` system message on its output stream — 1:1 with
    /// claude-code `createCanUseTool`'s deny arm (`{type:"system",
    /// subtype:"permission_denied", tool_name, tool_use_id, agent_id,
    /// decision_reason_type, decision_reason, message, uuid, session_id}`).
    ///
    /// `decision_reason_type` is the discriminated reason kind
    /// (`rule`/`mode`/`safetyCheck`/…, the `decisionReason?.type`), and
    /// `decision_reason` is the `oin(decisionReason)`-filtered reason text (only
    /// the classifier/hook/asyncAgent/sandboxOverride/workingDir/safetyCheck/other
    /// kinds surface text). `message` is the rendered deny message.
    ///
    /// Called by [`PolicyPermissionGate`] on its local Deny decision. Additive
    /// DEFAULTED no-op (frozen-trait safe): only the stdio control-plane gate
    /// overrides it to enqueue the frame; interactive/headless transports (which
    /// have no outbound NDJSON stream) keep the no-op.
    async fn on_permission_denied(
        &self,
        name: &str,
        ctx: &PermissionCheckContext,
        decision_reason_type: Option<&str>,
        decision_reason: Option<&str>,
        message: &str,
    ) {
        let _ = (name, ctx, decision_reason_type, decision_reason, message);
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

    /// [`Self::check_after_hook_allow`] carrying the dispatch [`PermissionCheckContext`].
    ///
    /// When a hook `allow` is overridden by an ask rule/safety check, the oracle's
    /// `lin` re-enters the FULL permission pipeline WITH the real `toolUseId` and
    /// rule metadata (`o(t,c,n,i,s)`), so the resulting stdio `can_use_tool`
    /// request is byte-faithful (correlatable id + `decision_reason`). The
    /// no-context [`Self::check_after_hook_allow`] instead reached the inner
    /// transport with a default context (a random id, no reason). This method
    /// threads the context so a rule-evaluating gate can delegate that ask via
    /// [`Self::check_with_context`]. Additive DEFAULTED (frozen-trait safe):
    /// defaults to the context-less method.
    async fn check_after_hook_allow_ctx(
        &self,
        name: &str,
        input: &Value,
        ctx: &PermissionCheckContext,
    ) -> PermissionDecision {
        let _ = ctx;
        self.check_after_hook_allow(name, input).await
    }

    /// Rich-outcome variant of [`Self::check_after_hook_allow_ctx`].
    ///
    /// A rule-evaluating gate may need to re-enter an interactive permission
    /// transport after a hook allow. If that transport rewrites the tool input,
    /// the rewrite must reach the dispatcher just like it does on the ordinary
    /// permission path. The default preserves the frozen trait behavior by
    /// projecting the existing two-valued decision into an outcome without a
    /// rewrite.
    async fn check_after_hook_allow_outcome_ctx(
        &self,
        name: &str,
        input: &Value,
        ctx: &PermissionCheckContext,
    ) -> PermissionOutcome {
        match self.check_after_hook_allow_ctx(name, input, ctx).await {
            PermissionDecision::Allow => PermissionOutcome::Allow {
                updated_input: None,
                permission_updates: Vec::new(),
                decision_classification: None,
            },
            PermissionDecision::Deny { reason } => PermissionOutcome::Deny { reason },
        }
    }

    /// The PERMISSION-REQUEST-hook twin of [`Self::check_after_hook_allow`].
    ///
    /// claude-code has TWO hook-allow resolvers and they differ in what an
    /// ask-rule does:
    ///
    /// * `lin` (PreToolUse) re-checks the rules and, on an `ask`, hands the call
    ///   to the FULL permission pipeline — i.e. it PROMPTS
    ///   ([`Self::check_after_hook_allow`]).
    /// * `Fxy`/`epr` (the headless PermissionRequest rescue) re-checks only when
    ///   the hook supplied `updatedInput` (or the tool requires user
    ///   interaction) and converts an `ask` into a **hard deny** — that agent has
    ///   no prompt available, and the hook already consumed the one chance to
    ///   resolve it.
    ///
    /// Additive DEFAULTED method (frozen-trait safe): the default delegates to
    /// [`Self::check_after_hook_allow`], preserving prior behavior for gates that
    /// carry no rule layer.
    async fn check_after_hook_allow_rewritten(
        &self,
        name: &str,
        input: &Value,
    ) -> PermissionDecision {
        self.check_after_hook_allow(name, input).await
    }

    /// `Fxy`'s STANDING PermissionRequest-hook `allow` — the arm where the hook
    /// supplied no `updatedInput` and the tool does not require user interaction,
    /// so the oracle's re-check gate `if(a.updatedInput||e.requiresUserInteraction
    /// ?.())` is false and the allow returns UNCHECKED
    /// (`return {behavior:"allow", updatedInput:l, decisionReason:{type:"hook",…}}`).
    ///
    /// Unlike [`Self::check_after_hook_allow`] (`lin`, PreToolUse) and
    /// [`Self::check_after_hook_allow_rewritten`] (`Fxy` WITH a re-check), this
    /// runs NO rule/mode verdict at all: an ordinary ask rule — the very reason
    /// the gate resolved `Ask` and fired the PermissionRequest hook — must not be
    /// re-evaluated, or the headless rescue is defeated in its primary use case.
    /// A rule-evaluating gate still records its per-allow auto-mode bookkeeping.
    ///
    /// Additive DEFAULTED method (frozen-trait safe): the default is a wholesale
    /// [`PermissionDecision::Allow`], correct for gates with no rule layer.
    async fn honour_hook_allow(&self, name: &str, input: &Value) -> PermissionDecision {
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
            PermissionDecision::Allow => PermissionResolution::Allow { rule_source: None },
            PermissionDecision::Deny { reason } => PermissionResolution::Deny {
                reason,
                source: PermissionDecisionSource::Unspecified,
                rule_source: None,
                // The default has only a rendered String reason (no structured
                // `PermissionDecisionReason`), so the system-message discriminants
                // are unavailable here.
                decision_reason_type: None,
                decision_reason: None,
                behavior_ask: false,
                content_blocks: Vec::new(),
            },
        }
    }

    /// Like [`Self::resolve_detailed`], but carries dispatch context and
    /// preserves a terminal [`PermissionAbort`].
    ///
    /// The main turn loop uses this source-first surface so permission hooks
    /// retain their ordering. The default delegates to the legacy method and
    /// therefore never aborts.
    async fn resolve_detailed_or_abort(
        &self,
        name: &str,
        input: &Value,
        ctx: &PermissionCheckContext,
    ) -> Result<PermissionResolution, PermissionAbort> {
        let _ = ctx;
        Ok(self.resolve_detailed(name, input).await)
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

    /// Set or clear the LIVE per-MCP-server permission-mode override used by
    /// Claude's control-channel `set_mcp_permission_mode_override` surface.
    ///
    /// `mode == Some("default")` or `Some("auto")` stores a tighten-only
    /// override for `server_name`; `mode == None` clears it. The default is a
    /// no-op `Ok(())`: only `PolicyPermissionGate` carries a live per-server mode
    /// layer. Additive DEFAULTED (frozen-trait safe).
    async fn set_mcp_permission_mode_override(
        &self,
        server_name: &str,
        mode: Option<&str>,
    ) -> Result<(), String> {
        let _ = (server_name, mode);
        Ok(())
    }

    /// The LIVE effective permission mode as a wire string
    /// (`default`/`plan`/`acceptEdits`/`bypassPermissions`/`dontAsk`/`auto`), or
    /// `None` for a gate with no mode layer (the interactive / no-op / adapter
    /// transports). Read when snapshotting session state across the in-process
    /// `/resume`, `/branch`, and `/rewind` re-mount so the user's mid-session
    /// Shift+Tab mode is carried into the rebuilt runtime instead of resetting to
    /// the CLI/config default. Only [`PolicyPermissionGate`] overrides this;
    /// additive DEFAULTED (frozen-trait safe), returning `None`.
    fn permission_mode(&self) -> Option<String> {
        None
    }
}

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
}

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
}

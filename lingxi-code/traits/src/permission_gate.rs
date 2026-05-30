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
}

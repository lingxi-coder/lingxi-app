//! `DenyOnAskGate` — the non-interactive inner permission transport.
//!
//! [`crate::PolicyPermissionGate`] consults its INNER gate only when a tool's
//! policy outcome is `Ask` AND the tool is not auto-allowed (a mutating /
//! [`crate::gate::PromptDefault::DenyByDefault`] tool with no matching allow
//! rule). In a HEADLESS / `--print` session there is no interactive prompt to
//! surface that `Ask`, so — matching claude-code's non-interactive behavior,
//! where a tool call with no `canUseTool` resolution is rejected rather than
//! silently allowed — this gate DENIES.
//!
//! Allow/deny RULES, the active permission MODE, and read-only auto-allow are
//! all resolved by [`crate::PolicyPermissionGate`] BEFORE it delegates here, so
//! this gate only ever turns an otherwise-unresolvable `Ask` into a denial —
//! never overriding an explicit allow rule or a read-only tool.

use crate::gate::{PermissionDecision, PermissionGate};
use async_trait::async_trait;
use serde_json::Value;

/// Non-interactive inner gate: turns an unresolved `Ask` into a `Deny`
/// (claude-code headless / `--print` parity). See the module docs for why this
/// is safe to use only as the INNER gate under [`crate::PolicyPermissionGate`].
#[derive(Debug, Default, Clone, Copy)]
pub struct DenyOnAskGate;

#[async_trait]
impl PermissionGate for DenyOnAskGate {
    async fn check(&self, name: &str, _input: &Value) -> PermissionDecision {
        PermissionDecision::Deny {
            reason: format!(
                "{name} requires permission, but this is a non-interactive session and no \
                 allow rule matched. Add a permission rule (settings.json `permissions.allow`) \
                 or run interactively to approve it."
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn denies_with_tool_named_reason() {
        let gate = DenyOnAskGate;
        match gate.check("Bash", &serde_json::json!({})).await {
            PermissionDecision::Deny { reason } => {
                assert!(
                    reason.contains("Bash"),
                    "reason should name the tool: {reason}"
                );
                assert!(
                    reason.contains("non-interactive"),
                    "reason should explain the non-interactive denial: {reason}"
                );
            }
            PermissionDecision::Allow => panic!("DenyOnAskGate must never allow"),
        }
    }
}

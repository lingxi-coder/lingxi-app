//! `PromptingGate` sub-trait — interactive y/N permission UX.
//!
//! Adds a `prompt_user` method on top of [`PermissionGate`]. Production
//! impls (the `InteractivePromptingGate` in `lingxi-permission`) drive the
//! prompt against stdin/stderr; tests pipe scripted I/O via
//! `tokio::io::duplex`. See M5-05 plan §"Design locks" for the wire-locked
//! prompt formats.
#![forbid(unsafe_code)]

use async_trait::async_trait;
use serde_json::Value;
use thiserror::Error;

use crate::permission_gate::PermissionGate;

/// Per-tool default decision when the user just presses Enter on the prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptDefault {
    /// `[Y/n]` — bare Enter ⇒ Allow.
    AllowByDefault,
    /// `[y/N]` — bare Enter ⇒ Deny.
    DenyByDefault,
}

/// Inputs to a single permission prompt.
#[derive(Debug, Clone)]
pub struct PermissionRequest {
    /// Canonical tool name (e.g. `"Bash"`, `"Agent"`).
    pub tool_name: String,
    /// The model's `tool_input` JSON (preserved for context; not currently
    /// shown in the M5-05 prompt — M5-06 hooks may use it).
    pub tool_input: Value,
    /// Default decision when the user presses Enter only.
    pub default_decision: PromptDefault,
}

/// Outcome of one prompt round-trip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptDecision {
    /// True ⇒ Allow, false ⇒ Deny.
    pub allow: bool,
    /// Free-form audit reason.
    pub reason: String,
    /// True ⇒ persist this decision (future: write to settings). M5-05 always
    /// sets this to `false` (session-only) — M5-11 `/permissions` adds the
    /// persistence path.
    pub persist: bool,
}

/// Errors specific to the prompting path.
#[derive(Debug, Error)]
pub enum PromptError {
    /// stdin / stderr I/O failure.
    #[error("prompt io: {0}")]
    Io(String),
    /// User typed 3 invalid inputs in a row.
    #[error("prompt invalid input after {attempts} attempts")]
    InvalidInput {
        /// How many invalid inputs were consumed (always 3 at the cap).
        attempts: u32,
    },
    /// stdin closed / user cancelled before answering.
    #[error("prompt cancelled: {reason}")]
    Cancelled {
        /// Free-form reason.
        reason: String,
    },
}

/// Interactive permission gate — extends [`PermissionGate`] with a
/// stdin/stderr prompt round-trip.
#[async_trait]
pub trait PromptingGate: PermissionGate {
    /// Drive one prompt round-trip. Returns `Ok(PromptDecision)` on a valid
    /// answer, `Err(PromptError::InvalidInput)` after 3 invalid inputs.
    async fn prompt_user(&self, request: &PermissionRequest)
        -> Result<PromptDecision, PromptError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn permission_request_constructs_with_allow_default() {
        let req = PermissionRequest {
            tool_name: "Read".to_string(),
            tool_input: json!({ "path": "/tmp/foo" }),
            default_decision: PromptDefault::AllowByDefault,
        };
        assert_eq!(req.tool_name, "Read");
        assert_eq!(req.default_decision, PromptDefault::AllowByDefault);
    }

    #[test]
    fn permission_request_constructs_with_deny_default() {
        let req = PermissionRequest {
            tool_name: "Bash".to_string(),
            tool_input: json!({ "command": "rm -rf /" }),
            default_decision: PromptDefault::DenyByDefault,
        };
        assert_eq!(req.tool_name, "Bash");
        assert_eq!(req.default_decision, PromptDefault::DenyByDefault);
    }

    #[test]
    fn prompt_decision_constructs() {
        let d = PromptDecision {
            allow: true,
            reason: "user typed y".to_string(),
            persist: false,
        };
        assert!(d.allow);
        assert_eq!(d.reason, "user typed y");
        assert!(!d.persist);
    }

    #[test]
    fn prompt_default_is_copy() {
        let a = PromptDefault::AllowByDefault;
        let b = a; // copy
        assert_eq!(a, b);
    }
}

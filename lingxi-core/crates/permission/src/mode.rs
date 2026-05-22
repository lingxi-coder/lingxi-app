//! `PermissionMode` — top-level authorization mode that gates unmatched tool calls.
//!
//! Five modes are external (settable from `settings.json` and CLI flags) and
//! two are internal-only (rejected by configuration validation but used by
//! the engine for bubbled prompts and auto-fallback flows).

use serde::{Deserialize, Serialize};

/// The active permission mode applied when no rule matches a tool call.
///
/// `Default` asks the user, `Plan` restricts to read-only operations,
/// `AcceptEdits` auto-allows file edits, `BypassPermissions` allows
/// everything (unless the killswitch is set), and `DontAsk` denies
/// everything that does not have an explicit allow rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionMode {
    // External / user-addressable (settings.json + CLI accept these).
    /// Ask the user before each unmatched tool call.
    Default,
    /// Plan-only mode: read-only operations and planning, no mutations.
    Plan,
    /// Auto-accept file edits without prompting.
    AcceptEdits,
    /// Allow everything (subject to the bypass killswitch).
    BypassPermissions,
    /// Deny everything that lacks an explicit allow rule.
    DontAsk,
    // Internal-only (rejected by settings/CLI validation).
    /// Internal: bubble the decision to a parent agent.
    Bubble,
    /// Internal: classifier-driven auto-accept with denial-tracking fallback.
    Auto,
}

impl PermissionMode {
    /// Returns true when this mode may appear in user-facing configuration.
    ///
    /// External modes are accepted by `settings.json` and CLI flag validation.
    /// Internal modes (`Bubble`, `Auto`) are rejected as input but produced
    /// by the engine at runtime.
    #[must_use]
    pub fn is_external(self) -> bool {
        !matches!(self, Self::Bubble | Self::Auto)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bubble_is_internal() {
        assert!(!PermissionMode::Bubble.is_external());
    }

    #[test]
    fn default_is_external() {
        assert!(PermissionMode::Default.is_external());
    }
}

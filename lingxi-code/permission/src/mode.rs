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

/// The next mode in the Shift+Tab UI cycle (claude-code `getNextPermissionMode`,
/// EXTERNAL / non-`ant` path): `Default→AcceptEdits→Plan→…`, where `Plan`
/// advances to `BypassPermissions` only when `bypass_available`, else back to
/// `Default`; `BypassPermissions`/`DontAsk`/internal modes return `Default`.
///
/// The `ant`-only `auto`/`bubble` cycle targets (and the `canCycleToAuto`
/// gate) are intentionally omitted — external builds never cycle to `auto`
/// (TS guards them behind `USER_TYPE==='ant'` + the `TRANSCRIPT_CLASSIFIER`
/// feature), so the non-`ant` cycle is byte-faithful.
#[must_use]
pub fn next_permission_mode(current: PermissionMode, bypass_available: bool) -> PermissionMode {
    use PermissionMode::{AcceptEdits, BypassPermissions, Default, Plan};
    match current {
        Default => AcceptEdits,
        AcceptEdits => Plan,
        Plan if bypass_available => BypassPermissions,
        // Plan (no bypass), BypassPermissions, DontAsk, Bubble, Auto → Default.
        _ => Default,
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

    #[test]
    fn cycle_external_path_matches_ts() {
        use PermissionMode::{AcceptEdits, BypassPermissions, Default, DontAsk, Plan};
        // bypass NOT available
        assert_eq!(next_permission_mode(Default, false), AcceptEdits);
        assert_eq!(next_permission_mode(AcceptEdits, false), Plan);
        assert_eq!(next_permission_mode(Plan, false), Default);
        assert_eq!(next_permission_mode(BypassPermissions, false), Default);
        assert_eq!(next_permission_mode(DontAsk, false), Default);
        // bypass available → Plan advances to BypassPermissions
        assert_eq!(next_permission_mode(Plan, true), BypassPermissions);
        assert_eq!(next_permission_mode(Default, true), AcceptEdits); // unchanged
        assert_eq!(next_permission_mode(BypassPermissions, true), Default);
        // internal modes fall back to Default
        assert_eq!(next_permission_mode(PermissionMode::Bubble, true), Default);
        assert_eq!(next_permission_mode(PermissionMode::Auto, true), Default);
    }
}

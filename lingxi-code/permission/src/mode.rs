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

/// The next mode in the Shift+Tab UI cycle (claude-code `fJn`, binary v2.1.183
/// at offset ~206604306):
///
/// ```text
/// switch(e.mode){
///   case"default":            return"acceptEdits";
///   case"acceptEdits":        return"plan";
///   case"plan":               if(e.isBypassPermissionsModeAvailable) return"bypassPermissions";
///                             if(mJn(e)) return"auto";
///                             return"default";
///   case"bypassPermissions":  if(mJn(e)) return"auto";
///                             return"default";
///   case"dontAsk":            return"default";
///   default:                  return"default";
/// }
/// ```
///
/// So `Default→AcceptEdits→Plan→…`, where `Plan` advances to `BypassPermissions`
/// when `bypass_available`, else to `Auto` when `can_cycle_to_auto`, else
/// `Default`; `BypassPermissions` advances to `Auto` when `can_cycle_to_auto`,
/// else `Default`; `DontAsk`/internal modes return `Default`.
///
/// `can_cycle_to_auto` is the `mJn(e)` gate, NOT a `USER_TYPE==='ant'` check.
/// In the binary `mJn(e) = !!e.isAutoModeAvailable && bx() && !GPo()`, where
/// `bx()` requires `!isAutoModeCircuitBroken() && disableAutoMode !== "disable"
/// && model-gate XCe(js())`, and `GPo()` is `autoModeOptInDismissed && !nDe()`.
/// None of those signals exist in this port yet, so callers currently pass
/// `false` (byte-faithful: when the gate is unavailable, `Plan`/`BypassPermissions`
/// fall through to `Default` exactly as `fJn` does).
#[must_use]
pub fn next_permission_mode(
    current: PermissionMode,
    bypass_available: bool,
    can_cycle_to_auto: bool,
) -> PermissionMode {
    use PermissionMode::{AcceptEdits, Auto, BypassPermissions, Default, Plan};
    match current {
        Default => AcceptEdits,
        AcceptEdits => Plan,
        // `plan`: bypass first, then the auto gate, else default.
        Plan if bypass_available => BypassPermissions,
        Plan if can_cycle_to_auto => Auto,
        // `bypassPermissions`: auto gate, else default.
        BypassPermissions if can_cycle_to_auto => Auto,
        // Plan (no bypass, no auto), BypassPermissions (no auto), DontAsk,
        // Bubble, Auto → Default.
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
        // Gate OFF (can_cycle_to_auto = false) — the pre-#33 behavior, unchanged.
        // bypass NOT available
        assert_eq!(next_permission_mode(Default, false, false), AcceptEdits);
        assert_eq!(next_permission_mode(AcceptEdits, false, false), Plan);
        assert_eq!(next_permission_mode(Plan, false, false), Default);
        assert_eq!(next_permission_mode(BypassPermissions, false, false), Default);
        assert_eq!(next_permission_mode(DontAsk, false, false), Default);
        // bypass available → Plan advances to BypassPermissions
        assert_eq!(next_permission_mode(Plan, true, false), BypassPermissions);
        assert_eq!(next_permission_mode(Default, true, false), AcceptEdits); // unchanged
        assert_eq!(next_permission_mode(BypassPermissions, true, false), Default);
        // internal modes fall back to Default
        assert_eq!(next_permission_mode(PermissionMode::Bubble, true, false), Default);
        assert_eq!(next_permission_mode(PermissionMode::Auto, true, false), Default);
    }

    #[test]
    fn cycle_to_auto_matches_fjn() {
        use PermissionMode::{AcceptEdits, Auto, BypassPermissions, Default, DontAsk, Plan};
        // `fJn` case"plan": bypass is checked BEFORE the auto gate, so when both
        // are available bypass wins.
        assert_eq!(next_permission_mode(Plan, true, true), BypassPermissions);
        // `plan` with no bypass but auto available → Auto.
        assert_eq!(next_permission_mode(Plan, false, true), Auto);
        // `bypassPermissions` with the auto gate → Auto.
        assert_eq!(next_permission_mode(BypassPermissions, false, true), Auto);
        assert_eq!(next_permission_mode(BypassPermissions, true, true), Auto);
        // The auto gate does NOT alter the head of the cycle.
        assert_eq!(next_permission_mode(Default, false, true), AcceptEdits);
        assert_eq!(next_permission_mode(AcceptEdits, false, true), Plan);
        // `dontAsk` and internal modes still go to Default even with the gate on.
        assert_eq!(next_permission_mode(DontAsk, false, true), Default);
        assert_eq!(next_permission_mode(PermissionMode::Bubble, false, true), Default);
        assert_eq!(next_permission_mode(Auto, false, true), Default);
    }
}

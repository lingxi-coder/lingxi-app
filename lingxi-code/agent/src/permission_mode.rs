//! Subagent-spawn permission-mode resolution — claude-code 2.1.207 `wKe`/`zol`
//! plus the runAgent context-override `ye=wKe(x,z); ve=ye??e.permissionMode`.
//!
//! The Agent tool's `mode` field ("plan"/"acceptEdits"/…) is CLAMPED against the
//! spawning parent's live permission mode so a child can never ESCALATE its own
//! privilege above the parent's, then the clamped value (or, failing that, the
//! agent definition's own permission mode) becomes the child's effective
//! permission-context mode. In claude-code every permission check the child runs
//! consults `toolPermissionContext.mode = ve`; in LingXi that override is threaded
//! into the subagent tool-dispatch permission gate (see
//! [`traits::tool_invoker::SubagentInvocationContext::mode_override`] →
//! [`traits::permission_gate::PermissionCheckContext::mode_override`]).
//!
//! `AgentPermissionMode` (`definition.rs`) is re-exported for callers that build
//! agent definitions.

use crate::definition::AgentPermissionMode;
use permission::PermissionMode;

/// Rank of a permission mode — claude-code `zol`
/// (`{plan:0,bubble:1,default:1,dontAsk:1,acceptEdits:2,auto:3,bypassPermissions:4}`).
/// A LOWER rank is MORE restrictive; a child may only request a mode whose rank
/// is `<=` the parent's (it can never escalate). `plan` (0) is therefore always
/// honored; `bypassPermissions` (4) only under a bypass parent.
fn mode_rank(mode: PermissionMode) -> u8 {
    match mode {
        PermissionMode::Plan => 0,
        PermissionMode::Bubble | PermissionMode::Default | PermissionMode::DontAsk => 1,
        PermissionMode::AcceptEdits => 2,
        PermissionMode::Auto => 3,
        PermissionMode::BypassPermissions => 4,
    }
}

/// Parse a spawn-mode WIRE string — the Agent tool `mode` enum
/// (`acceptEdits`/`auto`/`bypassPermissions`/`default`/`dontAsk`/`plan`) — into a
/// [`PermissionMode`]. Returns `None` for an unrecognized string (the schema enum
/// forbids one, so this is defensive). `bubble` is engine-internal and never
/// appears on the wire.
#[must_use]
pub(crate) fn parse_wire_mode(s: &str) -> Option<PermissionMode> {
    match s {
        "default" => Some(PermissionMode::Default),
        "plan" => Some(PermissionMode::Plan),
        "acceptEdits" => Some(PermissionMode::AcceptEdits),
        "bypassPermissions" => Some(PermissionMode::BypassPermissions),
        "dontAsk" => Some(PermissionMode::DontAsk),
        "auto" => Some(PermissionMode::Auto),
        _ => None,
    }
}

/// Canonical wire string for a [`PermissionMode`] — the inverse of
/// [`parse_wire_mode`] (extended with `bubble` for completeness, though it never
/// reaches the wire).
#[must_use]
pub(crate) fn wire_mode_str(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Default => "default",
        PermissionMode::Plan => "plan",
        PermissionMode::AcceptEdits => "acceptEdits",
        PermissionMode::BypassPermissions => "bypassPermissions",
        PermissionMode::DontAsk => "dontAsk",
        PermissionMode::Auto => "auto",
        PermissionMode::Bubble => "bubble",
    }
}

/// Clamp a REQUESTED child spawn mode against the PARENT's live mode — claude
/// `wKe(e,t)` with `e`=requested, `t`=parent:
///
/// - no requested mode ⇒ `None` (`if(!e)return`);
/// - parent `auto` + requested `acceptEdits` ⇒ `None` (the special-case drop,
///   `if(t==="auto"&&e==="acceptEdits")return`);
/// - otherwise honor the requested mode only when `zol[requested] <= zol[parent]`
///   (`return zol[e]<=zol[t]?e:void 0`) — the child cannot escalate above the
///   parent.
#[must_use]
fn clamp_spawn_mode(
    requested: Option<PermissionMode>,
    parent: PermissionMode,
) -> Option<PermissionMode> {
    let requested = requested?;
    if parent == PermissionMode::Auto && requested == PermissionMode::AcceptEdits {
        return None;
    }
    (mode_rank(requested) <= mode_rank(parent)).then_some(requested)
}

/// The [`PermissionMode`] fallback for a subagent definition's own
/// [`AgentPermissionMode`], used as claude's `ve = ye ?? e.permissionMode`.
///
/// Only `Plan`/`Auto` carry a permission-check semantics worth applying as an
/// override; `Bubble` (LingXi's default, ≈ claude's *undefined* frontmatter
/// `permissionMode`) and `Isolated` (no claude mode equivalent) map to `None` so
/// the overwhelmingly common case leaves the child inheriting the parent/live
/// mode unchanged — byte-identical to pre-2.1.207.
#[must_use]
fn definition_mode_fallback(mode: AgentPermissionMode) -> Option<PermissionMode> {
    match mode {
        AgentPermissionMode::Plan => Some(PermissionMode::Plan),
        AgentPermissionMode::Auto => Some(PermissionMode::Auto),
        AgentPermissionMode::Bubble | AgentPermissionMode::Isolated => None,
    }
}

/// Compute a spawned child's EFFECTIVE permission-context mode override — claude
/// runAgent's `ye=wKe(x,z); ve=ye??e.permissionMode` plus the
/// `getToolPermissionContext` guard
/// `if(ve&&(ye||ht.mode!=="bypassPermissions"&&ht.mode!=="acceptEdits"&&ht.mode!=="auto"))`.
///
/// Returns `Some(mode)` when the child's permission checks must run under an
/// OVERRIDE mode (threaded into the dispatch permission gate), or `None` when the
/// child simply inherits the parent/live mode (no override).
///
/// - `requested` — the parsed spawn `mode` (`None` when the caller omitted it).
/// - `parent` — the spawner's live permission-mode anchor (≈ claude's `ht.mode`).
/// - `def_mode` — the resolved agent definition's own permission mode.
#[must_use]
pub(crate) fn effective_child_mode(
    requested: Option<PermissionMode>,
    parent: PermissionMode,
    def_mode: AgentPermissionMode,
) -> Option<PermissionMode> {
    let clamped = clamp_spawn_mode(requested, parent); // `ye`
    let effective = clamped.or_else(|| definition_mode_fallback(def_mode))?; // `ve`
    // Context-override guard: apply `ve` UNLESS it came only from the definition
    // fallback (no explicit clamped spawn mode) while the parent is already a
    // permissive mode the fallback must not silently downgrade.
    if clamped.is_none()
        && matches!(
            parent,
            PermissionMode::BypassPermissions | PermissionMode::AcceptEdits | PermissionMode::Auto
        )
    {
        return None;
    }
    Some(effective)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── wKe clamp semantics (claude 2.1.207 `wKe`/`zol`) ──────────────────────

    #[test]
    fn clamp_none_requested_is_none() {
        assert_eq!(clamp_spawn_mode(None, PermissionMode::Default), None);
    }

    #[test]
    fn clamp_child_cannot_escalate_above_parent() {
        // parent default (rank 1) + requested bypassPermissions (rank 4) → dropped.
        assert_eq!(
            clamp_spawn_mode(Some(PermissionMode::BypassPermissions), PermissionMode::Default),
            None
        );
        // parent default (1) + requested acceptEdits (2) → dropped.
        assert_eq!(
            clamp_spawn_mode(Some(PermissionMode::AcceptEdits), PermissionMode::Default),
            None
        );
        // parent default (1) + requested auto (3) → dropped.
        assert_eq!(
            clamp_spawn_mode(Some(PermissionMode::Auto), PermissionMode::Default),
            None
        );
    }

    #[test]
    fn clamp_auto_parent_drops_requested_accept_edits() {
        // Special-case: parent auto + requested acceptEdits → dropped even though
        // rank 2 <= rank 3.
        assert_eq!(
            clamp_spawn_mode(Some(PermissionMode::AcceptEdits), PermissionMode::Auto),
            None
        );
        // But a different requested mode <= auto is still honored.
        assert_eq!(
            clamp_spawn_mode(Some(PermissionMode::Default), PermissionMode::Auto),
            Some(PermissionMode::Default)
        );
    }

    #[test]
    fn clamp_plan_is_honored_under_every_parent() {
        for parent in [
            PermissionMode::Default,
            PermissionMode::Plan,
            PermissionMode::AcceptEdits,
            PermissionMode::Auto,
            PermissionMode::BypassPermissions,
            PermissionMode::DontAsk,
            PermissionMode::Bubble,
        ] {
            assert_eq!(
                clamp_spawn_mode(Some(PermissionMode::Plan), parent),
                Some(PermissionMode::Plan),
                "plan (rank 0) must be honored under parent {parent:?}"
            );
        }
    }

    #[test]
    fn clamp_equal_or_lower_rank_is_honored() {
        // parent acceptEdits (2) + requested default (1) → honored (downgrade ok).
        assert_eq!(
            clamp_spawn_mode(Some(PermissionMode::Default), PermissionMode::AcceptEdits),
            Some(PermissionMode::Default)
        );
        // parent bypass (4) + requested bypass (4) → honored (equal rank).
        assert_eq!(
            clamp_spawn_mode(
                Some(PermissionMode::BypassPermissions),
                PermissionMode::BypassPermissions
            ),
            Some(PermissionMode::BypassPermissions)
        );
    }

    // ── ve computation + context-override guard ───────────────────────────────

    #[test]
    fn explicit_spawn_mode_wins_over_definition() {
        // Requested plan, definition Bubble, any parent → plan (clamp keeps plan).
        assert_eq!(
            effective_child_mode(
                Some(PermissionMode::Plan),
                PermissionMode::BypassPermissions,
                AgentPermissionMode::Bubble
            ),
            Some(PermissionMode::Plan)
        );
    }

    #[test]
    fn definition_plan_fallback_applies_under_non_permissive_parent() {
        // No spawn mode, definition Plan, parent default → the definition fallback
        // supplies Plan.
        assert_eq!(
            effective_child_mode(None, PermissionMode::Default, AgentPermissionMode::Plan),
            Some(PermissionMode::Plan)
        );
    }

    #[test]
    fn definition_fallback_suppressed_under_permissive_parent() {
        // No spawn mode + definition Plan, but the parent is already permissive
        // (bypassPermissions/acceptEdits/auto) → the fallback is SUPPRESSED so the
        // permissive parent is not silently downgraded (claude's `ht.mode` guard).
        for parent in [
            PermissionMode::BypassPermissions,
            PermissionMode::AcceptEdits,
            PermissionMode::Auto,
        ] {
            assert_eq!(
                effective_child_mode(None, parent, AgentPermissionMode::Plan),
                None,
                "definition fallback must be suppressed under permissive parent {parent:?}"
            );
        }
    }

    #[test]
    fn explicit_spawn_mode_not_suppressed_under_permissive_parent() {
        // An EXPLICIT clamped spawn mode is applied even under a permissive parent
        // (the guard only suppresses the definition-only fallback).
        assert_eq!(
            effective_child_mode(
                Some(PermissionMode::Plan),
                PermissionMode::Auto,
                AgentPermissionMode::Bubble
            ),
            Some(PermissionMode::Plan)
        );
    }

    #[test]
    fn bubble_definition_default_yields_no_override() {
        // The common case: no spawn mode, Bubble definition (LingXi default),
        // parent default → no override (child inherits the live mode).
        assert_eq!(
            effective_child_mode(None, PermissionMode::Default, AgentPermissionMode::Bubble),
            None
        );
    }

    #[test]
    fn escalating_spawn_mode_falls_back_to_definition() {
        // Requested bypass under a default parent is dropped by the clamp; with a
        // Plan definition the fallback still yields Plan (never the escalation).
        assert_eq!(
            effective_child_mode(
                Some(PermissionMode::BypassPermissions),
                PermissionMode::Default,
                AgentPermissionMode::Plan
            ),
            Some(PermissionMode::Plan)
        );
    }

    #[test]
    fn wire_mode_round_trips() {
        for (s, m) in [
            ("default", PermissionMode::Default),
            ("plan", PermissionMode::Plan),
            ("acceptEdits", PermissionMode::AcceptEdits),
            ("bypassPermissions", PermissionMode::BypassPermissions),
            ("dontAsk", PermissionMode::DontAsk),
            ("auto", PermissionMode::Auto),
        ] {
            assert_eq!(parse_wire_mode(s), Some(m));
            assert_eq!(wire_mode_str(m), s);
        }
        assert_eq!(parse_wire_mode("nonsense"), None);
    }
}

//! Permission modal dialogs — 3 variants per the M6-05 plan.
//!
//! - [`tool_use_confirm::ToolUseConfirm`] — generic per-tool prompt.
//! - [`exit_plan_mode::ExitPlanMode`] — approve a multi-line plan body.
//! - [`bypass_permissions::BypassPermissionsMode`] — typed-`yes`
//!   confirmation for dangerous mode.
//!
//! Each submodule pairs an iocraft `#[component]` with a pure
//! `handle_key` helper that owns the dialog's state machine. The
//! `DialogResolution` returned by `handle_key` is what the keymap-level
//! focus-trap in `events::keymap` ultimately ships back to the
//! orchestrator over the oneshot reply channel.
#![forbid(unsafe_code)]

pub mod bypass_permissions;
pub mod exit_plan_mode;
pub mod tool_use_confirm;

use lingxi_permission::gate::PermissionResponse;

/// What the dialog produces when the user resolves it.
///
/// Two-field record: the `PermissionResponse` variant + whether the
/// resolution should be persisted as a session rule. `AllowAlways`
/// implies `persist == true`; `AllowOnce` and `Deny` both imply
/// `persist == false`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DialogResolution {
    /// The choice the user made.
    pub response: PermissionResponse,
    /// Whether the orchestrator should append a session-scoped allow rule
    /// for this tool. Always `false` for `AllowOnce` and `Deny`; `true`
    /// for `AllowAlways`.
    pub persist: bool,
}

impl DialogResolution {
    /// Allow only this tool call.
    #[must_use]
    pub fn allow_once() -> Self {
        Self {
            response: PermissionResponse::AllowOnce,
            persist: false,
        }
    }
    /// Allow this tool for the rest of the session (session rule).
    #[must_use]
    pub fn allow_always() -> Self {
        Self {
            response: PermissionResponse::AllowAlways,
            persist: true,
        }
    }
    /// Reject this tool call.
    #[must_use]
    pub fn deny() -> Self {
        Self {
            response: PermissionResponse::Deny,
            persist: false,
        }
    }
}

/// Which of the three buttons is currently highlighted (arrow-key state).
///
/// Used by `ToolUseConfirm` and `ExitPlanMode`; the bypass dialog has a
/// typed-input state machine instead of button focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogFocus {
    /// `[1] Allow Once` is highlighted.
    AllowOnce,
    /// `[2] Allow Always` is highlighted.
    AllowAlways,
    /// `[N] Deny` is highlighted.
    Deny,
}

impl Default for DialogFocus {
    /// Per the M6-05 task brief: "Allow Once highlighted by default".
    fn default() -> Self {
        Self::AllowOnce
    }
}

impl DialogFocus {
    /// Step focus down through the buttons (wraps at the end).
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::AllowOnce => Self::AllowAlways,
            Self::AllowAlways => Self::Deny,
            Self::Deny => Self::AllowOnce,
        }
    }
    /// Step focus up through the buttons (wraps at the start).
    #[must_use]
    pub fn prev(self) -> Self {
        match self {
            Self::AllowOnce => Self::Deny,
            Self::AllowAlways => Self::AllowOnce,
            Self::Deny => Self::AllowAlways,
        }
    }
    /// Resolve to the matching [`DialogResolution`].
    #[must_use]
    pub fn resolve(self) -> DialogResolution {
        match self {
            Self::AllowOnce => DialogResolution::allow_once(),
            Self::AllowAlways => DialogResolution::allow_always(),
            Self::Deny => DialogResolution::deny(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_focus_is_allow_once() {
        assert_eq!(DialogFocus::default(), DialogFocus::AllowOnce);
    }

    #[test]
    fn next_cycles_through_three_buttons() {
        let mut f = DialogFocus::default();
        f = f.next();
        assert_eq!(f, DialogFocus::AllowAlways);
        f = f.next();
        assert_eq!(f, DialogFocus::Deny);
        f = f.next();
        assert_eq!(f, DialogFocus::AllowOnce);
    }

    #[test]
    fn prev_cycles_back_through_three_buttons() {
        let mut f = DialogFocus::default();
        f = f.prev();
        assert_eq!(f, DialogFocus::Deny);
        f = f.prev();
        assert_eq!(f, DialogFocus::AllowAlways);
        f = f.prev();
        assert_eq!(f, DialogFocus::AllowOnce);
    }

    #[test]
    fn resolution_helpers_set_persist_correctly() {
        assert!(!DialogResolution::allow_once().persist);
        assert!(DialogResolution::allow_always().persist);
        assert!(!DialogResolution::deny().persist);
    }

    #[test]
    fn focus_resolve_maps_to_three_responses() {
        assert_eq!(
            DialogFocus::AllowOnce.resolve().response,
            PermissionResponse::AllowOnce
        );
        assert_eq!(
            DialogFocus::AllowAlways.resolve().response,
            PermissionResponse::AllowAlways
        );
        assert_eq!(
            DialogFocus::Deny.resolve().response,
            PermissionResponse::Deny
        );
    }
}

//! `ExitPlanMode` dialog — user approves a plan-mode exit (M6-05).
//!
//! Header literal `"LingXi needs your approval for the plan"` is
//! byte-locked from `claude-code/src/components/permissions/PermissionRequest.tsx:131`.
//! The 3-button bindings and grammar are identical to
//! [`super::tool_use_confirm`].
#![forbid(unsafe_code)]

use crossterm::event::{KeyCode, KeyEvent};
use iocraft::prelude::*;

use super::{DialogFocus, DialogResolution};

/// Mutable state for the `ExitPlanMode` dialog.
#[derive(Debug, Clone, Default)]
pub struct ExitPlanModeState {
    /// Which button is currently highlighted.
    pub focus: DialogFocus,
}

/// Props for [`ExitPlanMode`].
#[derive(Default, Props)]
pub struct ExitPlanModeProps {
    /// Plan markdown body, rendered as a multi-line block.
    pub plan: String,
    /// Which button to highlight on this render.
    pub focus: DialogFocus,
}

/// Pure key handler — bit-identical to
/// [`super::tool_use_confirm::handle_key`].
#[must_use]
pub fn handle_key(state: &mut ExitPlanModeState, key: KeyEvent) -> Option<DialogResolution> {
    match key.code {
        KeyCode::Char('1') => Some(DialogResolution::allow_once()),
        KeyCode::Char('2') => Some(DialogResolution::allow_always()),
        KeyCode::Char('n' | 'N') | KeyCode::Esc => Some(DialogResolution::deny()),
        KeyCode::Enter => Some(state.focus.resolve()),
        KeyCode::Down | KeyCode::Tab => {
            state.focus = state.focus.next();
            None
        }
        KeyCode::Up | KeyCode::BackTab => {
            state.focus = state.focus.prev();
            None
        }
        _ => None,
    }
}

/// iocraft component rendering the plan body + 3 buttons.
#[component]
pub fn ExitPlanMode(props: &ExitPlanModeProps) -> impl Into<AnyElement<'static>> {
    // (perm-06) claude-code `ExitPlanMode` dialog: bold "Ready to code?" title +
    // the plan + the plan-approval option list (NOT the generic [1]/[2]/[N]).
    // AllowAlways persists → "auto-accept edits"; AllowOnce proceeds once →
    // "manually approve edits"; Deny stays in plan mode → "No, keep planning".
    let header = "Ready to code?".to_string();
    let plan = props.plan.clone();
    let focus = props.focus;
    let button_label = move |for_focus: DialogFocus, label: &str| -> String {
        if for_focus == focus {
            format!("> {label}")
        } else {
            format!("  {label}")
        }
    };
    let allow_once = button_label(DialogFocus::AllowOnce, "Yes, manually approve edits");
    let allow_always = button_label(DialogFocus::AllowAlways, "Yes, auto-accept edits");
    let deny = button_label(DialogFocus::Deny, "No, keep planning");
    element! {
        View(
            flex_direction: FlexDirection::Column,
            border_style: BorderStyle::Round,
            padding: 1,
        ) {
            Text(content: header, weight: Weight::Bold)
            View(flex_direction: FlexDirection::Column, padding_top: 1) {
                Text(content: plan)
            }
            View(flex_direction: FlexDirection::Column, padding_top: 1) {
                Text(content: allow_once)
                Text(content: allow_always)
                Text(content: deny)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use permission::gate::PermissionResponse;

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn key_1_returns_allow_once() {
        let mut state = ExitPlanModeState::default();
        let res = handle_key(&mut state, k(KeyCode::Char('1')));
        assert_eq!(res.unwrap().response, PermissionResponse::AllowOnce);
    }

    #[test]
    fn key_2_returns_allow_always() {
        let mut state = ExitPlanModeState::default();
        let res = handle_key(&mut state, k(KeyCode::Char('2')));
        let r = res.unwrap();
        assert_eq!(r.response, PermissionResponse::AllowAlways);
        assert!(r.persist);
    }

    #[test]
    fn key_lowercase_n_returns_deny() {
        let mut state = ExitPlanModeState::default();
        let res = handle_key(&mut state, k(KeyCode::Char('n')));
        assert_eq!(res.unwrap().response, PermissionResponse::Deny);
    }

    #[test]
    fn key_uppercase_n_returns_deny() {
        let mut state = ExitPlanModeState::default();
        let res = handle_key(&mut state, k(KeyCode::Char('N')));
        assert_eq!(res.unwrap().response, PermissionResponse::Deny);
    }

    #[test]
    fn key_esc_returns_deny() {
        let mut state = ExitPlanModeState::default();
        let res = handle_key(&mut state, k(KeyCode::Esc));
        assert_eq!(res.unwrap().response, PermissionResponse::Deny);
    }

    #[test]
    fn key_enter_resolves_on_highlighted_button() {
        let mut state = ExitPlanModeState::default();
        let res = handle_key(&mut state, k(KeyCode::Enter));
        assert_eq!(res.unwrap().response, PermissionResponse::AllowOnce);
    }

    #[test]
    fn key_down_arrow_advances_focus_no_resolution() {
        let mut state = ExitPlanModeState::default();
        assert_eq!(state.focus, DialogFocus::AllowOnce);
        let res = handle_key(&mut state, k(KeyCode::Down));
        assert!(res.is_none());
        assert_eq!(state.focus, DialogFocus::AllowAlways);
    }

    #[test]
    fn key_text_does_not_resolve() {
        let mut state = ExitPlanModeState::default();
        let res = handle_key(&mut state, k(KeyCode::Char('x')));
        assert!(res.is_none());
    }
}

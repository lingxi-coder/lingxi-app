//! `ToolUseConfirm` dialog — generic per-tool permission prompt (M6-05).
//!
//! Renders the LingXi-locked three-button layout:
//! ```text
//! Claude needs your permission to use {tool}
//! Input: {pretty}
//!
//! > [1] Allow Once
//!   [2] Allow Always
//!   [N] Deny
//! ```
//!
//! Bindings:
//! - `1` → `AllowOnce`
//! - `2` → `AllowAlways`
//! - `n` / `N` / `Esc` → `Deny`
//! - `Enter` → resolve on the currently focused button
//! - `Down` / `Tab` → step focus forward
//! - `Up` / `BackTab` → step focus backward
#![forbid(unsafe_code)]

use crossterm::event::{KeyCode, KeyEvent};
use iocraft::prelude::*;

use super::{DialogFocus, DialogResolution};

/// Mutable state carried across renders for this dialog.
#[derive(Debug, Clone, Default)]
pub struct ToolUseConfirmState {
    /// Which button is currently highlighted.
    pub focus: DialogFocus,
}

/// Props for [`ToolUseConfirm`].
#[derive(Default, Props)]
pub struct ToolUseConfirmProps {
    /// Tool name shown in the header (e.g. `"Bash"`).
    pub tool_name: String,
    /// Pretty-printed JSON input shown under the header.
    pub tool_input_pretty: String,
    /// Which button to highlight on this render.
    pub focus: DialogFocus,
    /// (M9-07) Worker badge `● @name` prepended when worker-originated.
    /// `None` (the live default) leaves the dialog byte-identical to before.
    pub worker_badge: Option<String>,
}

/// Pure key handler. Returns `Some(resolution)` when the user picks an
/// option, `None` otherwise (focus moved, or ignored key).
#[must_use]
pub fn handle_key(state: &mut ToolUseConfirmState, key: KeyEvent) -> Option<DialogResolution> {
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

/// iocraft component rendering the dialog frame + 3 buttons.
#[component]
pub fn ToolUseConfirm(props: &ToolUseConfirmProps) -> impl Into<AnyElement<'static>> {
    let header = format!("Claude needs your permission to use {}", props.tool_name);
    let input_line = format!("Input: {}", props.tool_input_pretty);
    let focus = props.focus;
    let worker_badge = props.worker_badge.clone();
    let button_label = move |for_focus: DialogFocus, label: &str| -> String {
        if for_focus == focus {
            format!("> {label}")
        } else {
            format!("  {label}")
        }
    };
    let allow_once = button_label(DialogFocus::AllowOnce, "[1] Allow Once");
    let allow_always = button_label(DialogFocus::AllowAlways, "[2] Allow Always");
    let deny = button_label(DialogFocus::Deny, "[N] Deny");
    element! {
        View(
            flex_direction: FlexDirection::Column,
            border_style: BorderStyle::Round,
            padding: 1,
        ) {
            #(worker_badge.as_deref().map(|badge| element! {
                Text(content: badge.to_string())
            }))
            Text(content: header)
            Text(content: input_line)
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
        let mut state = ToolUseConfirmState::default();
        let res = handle_key(&mut state, k(KeyCode::Char('1')));
        assert_eq!(res.unwrap().response, PermissionResponse::AllowOnce);
    }

    #[test]
    fn key_2_returns_allow_always() {
        let mut state = ToolUseConfirmState::default();
        let res = handle_key(&mut state, k(KeyCode::Char('2')));
        let r = res.unwrap();
        assert_eq!(r.response, PermissionResponse::AllowAlways);
        assert!(r.persist);
    }

    #[test]
    fn key_lowercase_n_returns_deny() {
        let mut state = ToolUseConfirmState::default();
        let res = handle_key(&mut state, k(KeyCode::Char('n')));
        assert_eq!(res.unwrap().response, PermissionResponse::Deny);
    }

    #[test]
    fn key_uppercase_n_returns_deny() {
        let mut state = ToolUseConfirmState::default();
        let res = handle_key(&mut state, k(KeyCode::Char('N')));
        assert_eq!(res.unwrap().response, PermissionResponse::Deny);
    }

    #[test]
    fn key_esc_returns_deny() {
        let mut state = ToolUseConfirmState::default();
        let res = handle_key(&mut state, k(KeyCode::Esc));
        assert_eq!(res.unwrap().response, PermissionResponse::Deny);
    }

    #[test]
    fn key_enter_resolves_on_highlighted_button() {
        let mut state = ToolUseConfirmState::default();
        // Default focus = AllowOnce.
        let res = handle_key(&mut state, k(KeyCode::Enter));
        assert_eq!(res.unwrap().response, PermissionResponse::AllowOnce);
    }

    #[test]
    fn key_down_arrow_advances_focus_no_resolution() {
        let mut state = ToolUseConfirmState::default();
        assert_eq!(state.focus, DialogFocus::AllowOnce);
        let res = handle_key(&mut state, k(KeyCode::Down));
        assert!(res.is_none());
        assert_eq!(state.focus, DialogFocus::AllowAlways);
    }

    #[test]
    fn key_text_does_not_resolve() {
        let mut state = ToolUseConfirmState::default();
        let res = handle_key(&mut state, k(KeyCode::Char('x')));
        assert!(res.is_none());
    }
}

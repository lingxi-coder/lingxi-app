//! `BypassPermissionsMode` dialog — user must type `yes` to enable (M6-05).
//!
//! Body literals are byte-locked from
//! `claude-code/src/components/BypassPermissionsModeDialog.tsx` (lines 53,
//! 73). The typed-`yes` confirmation is a `LingXi` divergence from
//! claude-code's React `Select`; the M6-05 task brief locks this stricter
//! friction step intentionally. See the M6-05 plan §"`LingXi` divergence".
//!
//! Bindings:
//! - Typing letters → buffered into `state.typed` (lowercased).
//! - `Backspace` → pop last char.
//! - `Enter` → resolves `AllowOnce` IFF `state.typed == "yes"` (case-insensitive);
//!   otherwise no-op.
//! - `Esc` → always `Deny` (even with partial input).
//! - `n` / `N` → always `Deny`.
#![forbid(unsafe_code)]

use crossterm::event::{KeyCode, KeyEvent};
use iocraft::prelude::*;

use super::DialogResolution;

/// Mutable state carried across renders.
#[derive(Debug, Clone, Default)]
pub struct BypassPermissionsState {
    /// Letters typed so far (lowercased internally). When
    /// `typed == "yes"`, Enter resolves to `AllowOnce`.
    pub typed: String,
}

/// Props for [`BypassPermissionsMode`].
#[derive(Default, Props)]
pub struct BypassPermissionsProps {
    /// What the user has typed so far (renders next to the prompt).
    pub typed: String,
}

/// Pure key handler.
///
/// Two completion paths:
/// 1. User types `yes` (case-insensitive) then Enter → `AllowOnce`.
/// 2. User presses Esc or `n`/`N` → `Deny`.
///
/// Wrong letters are accepted into the buffer so the user sees the typo
/// and can correct it via Backspace.
#[must_use]
pub fn handle_key(state: &mut BypassPermissionsState, key: KeyEvent) -> Option<DialogResolution> {
    match key.code {
        KeyCode::Esc | KeyCode::Char('n' | 'N') => Some(DialogResolution::deny()),
        KeyCode::Enter => {
            if state.typed.to_ascii_lowercase() == "yes" {
                Some(DialogResolution::allow_once())
            } else {
                None
            }
        }
        KeyCode::Backspace => {
            state.typed.pop();
            None
        }
        KeyCode::Char(c) => {
            if c.is_ascii_alphabetic() {
                state.typed.push(c.to_ascii_lowercase());
            }
            None
        }
        _ => None,
    }
}

/// iocraft component rendering the warning dialog.
#[component]
pub fn BypassPermissionsMode(props: &BypassPermissionsProps) -> impl Into<AnyElement<'static>> {
    // Byte-locked literals from BypassPermissionsModeDialog.tsx (lines 53, 73).
    let title = "WARNING: Claude Code running in Bypass Permissions mode".to_string();
    let body1 = "In Bypass Permissions mode, Claude Code will not ask for your approval before running potentially dangerous commands.\nThis mode should only be used in a sandboxed container/VM that has restricted internet access and can easily be restored if damaged.".to_string();
    let body2 = "By proceeding, you accept all responsibility for actions taken while running in Bypass Permissions mode.".to_string();
    let prompt_line = format!(
        "Type \"yes\" + Enter to enable, Esc to cancel: {}",
        props.typed
    );
    element! {
        View(
            flex_direction: FlexDirection::Column,
            border_style: BorderStyle::Round,
            padding: 1,
        ) {
            Text(content: title)
            View(flex_direction: FlexDirection::Column, padding_top: 1) {
                Text(content: body1)
                Text(content: body2)
            }
            View(flex_direction: FlexDirection::Column, padding_top: 1) {
                Text(content: prompt_line)
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
    fn kc(c: char) -> KeyEvent {
        k(KeyCode::Char(c))
    }

    #[test]
    fn typed_yes_then_enter_returns_allow_once() {
        let mut state = BypassPermissionsState::default();
        assert!(handle_key(&mut state, kc('y')).is_none());
        assert_eq!(state.typed, "y");
        assert!(handle_key(&mut state, kc('e')).is_none());
        assert_eq!(state.typed, "ye");
        assert!(handle_key(&mut state, kc('s')).is_none());
        assert_eq!(state.typed, "yes");
        let res = handle_key(&mut state, k(KeyCode::Enter));
        assert_eq!(res.unwrap().response, PermissionResponse::AllowOnce);
    }

    #[test]
    fn enter_without_typed_yes_does_not_resolve() {
        let mut state = BypassPermissionsState::default();
        let res = handle_key(&mut state, k(KeyCode::Enter));
        assert!(res.is_none());
    }

    #[test]
    fn esc_returns_deny_even_with_partial_input() {
        let mut state = BypassPermissionsState::default();
        let _ = handle_key(&mut state, kc('y'));
        let res = handle_key(&mut state, k(KeyCode::Esc));
        assert_eq!(res.unwrap().response, PermissionResponse::Deny);
    }

    #[test]
    fn key_uppercase_n_returns_deny() {
        let mut state = BypassPermissionsState::default();
        let res = handle_key(&mut state, kc('N'));
        assert_eq!(res.unwrap().response, PermissionResponse::Deny);
    }

    #[test]
    fn typing_wrong_letters_buffers_and_does_not_resolve() {
        let mut state = BypassPermissionsState::default();
        let _ = handle_key(&mut state, kc('y'));
        let _ = handle_key(&mut state, kc('o'));
        // `o` is accepted into buffer (user can backspace), but Enter does
        // not resolve because the buffer is not "yes".
        assert_eq!(state.typed, "yo");
        let res = handle_key(&mut state, k(KeyCode::Enter));
        assert!(res.is_none());
    }

    #[test]
    fn backspace_pops_typed_buffer() {
        let mut state = BypassPermissionsState::default();
        let _ = handle_key(&mut state, kc('y'));
        let _ = handle_key(&mut state, kc('e'));
        let _ = handle_key(&mut state, k(KeyCode::Backspace));
        assert_eq!(state.typed, "y");
    }

    #[test]
    fn typed_is_case_insensitive() {
        let mut state = BypassPermissionsState::default();
        let _ = handle_key(&mut state, kc('Y'));
        let _ = handle_key(&mut state, kc('E'));
        let _ = handle_key(&mut state, kc('S'));
        // Buffer keeps lowercase form internally.
        assert_eq!(state.typed, "yes");
        let res = handle_key(&mut state, k(KeyCode::Enter));
        assert_eq!(res.unwrap().response, PermissionResponse::AllowOnce);
    }
}

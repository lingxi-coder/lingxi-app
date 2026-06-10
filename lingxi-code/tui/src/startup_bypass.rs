//! Startup bypass-permissions confirmation — pure state machine + render for
//! the TTY-only `BypassPermissionsModeDialog` (claude-code
//! `components/BypassPermissionsModeDialog.tsx`, shown by `interactiveHelpers
//! showSetupScreens` before the REPL when bypass is resolved and
//! `skipDangerousModePermissionPrompt` is not yet set).
//!
//! Pure: `handle_key` drives a two-option Select; the terminal mount loop
//! lives in the CLI (`apps/cli/src/mode.rs`) and is thin (cannot be driven
//! headless, same caveat as `run_tui_session`).

use crossterm::event::{KeyCode, KeyEvent};

/// The two dialog choices (Select order: decline first, accept second).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BypassChoice {
    /// `No, exit`.
    Decline,
    /// `Yes, I accept`.
    Accept,
}

/// Dialog state — just the highlighted choice.
#[derive(Debug, Clone, Copy)]
pub struct BypassDialogState {
    /// Currently highlighted option.
    pub selected: BypassChoice,
}

impl Default for BypassDialogState {
    fn default() -> Self {
        // Decline-first highlight (matches the TS Select option order).
        Self {
            selected: BypassChoice::Decline,
        }
    }
}

/// Terminal outcome of the dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BypassDialogOutcome {
    /// User accepted: persist `skipDangerousModePermissionPrompt` + proceed.
    Accept,
    /// User declined (or pressed Esc): exit 1.
    Decline,
}

/// Handle one key. `Some(outcome)` ends the dialog; `None` keeps it open.
#[must_use]
pub fn handle_key(state: &mut BypassDialogState, key: KeyEvent) -> Option<BypassDialogOutcome> {
    match key.code {
        KeyCode::Up | KeyCode::Down | KeyCode::Char('k' | 'j') | KeyCode::Tab | KeyCode::BackTab => {
            state.selected = match state.selected {
                BypassChoice::Decline => BypassChoice::Accept,
                BypassChoice::Accept => BypassChoice::Decline,
            };
            None
        }
        KeyCode::Enter => Some(match state.selected {
            BypassChoice::Accept => BypassDialogOutcome::Accept,
            BypassChoice::Decline => BypassDialogOutcome::Decline,
        }),
        KeyCode::Esc => Some(BypassDialogOutcome::Decline),
        _ => None,
    }
}

/// The byte-exact display lines (title, body, link, options).
#[must_use]
pub fn render_lines() -> Vec<String> {
    vec![
        "WARNING: Claude Code running in Bypass Permissions mode".to_string(),
        "In Bypass Permissions mode, Claude Code will not ask for your approval before running potentially dangerous commands.".to_string(),
        "This mode should only be used in a sandboxed container/VM that has restricted internet access and can easily be restored if damaged.".to_string(),
        "By proceeding, you accept all responsibility for actions taken while running in Bypass Permissions mode.".to_string(),
        "https://code.claude.com/docs/en/security".to_string(),
        "No, exit".to_string(),
        "Yes, I accept".to_string(),
    ]
}

/// Whether the bypass dialog must be shown: bypass mode resolved AND no
/// settings tier has `skipDangerousModePermissionPrompt` truthy
/// (`hasSkipDangerousModePermissionPrompt`, claude-code settings.ts:882-889 —
/// user+local here; flag/policy tiers have no Rust substrate).
#[must_use]
pub fn should_show_bypass_dialog(is_bypass_mode: bool, skip_prompt_already_set: bool) -> bool {
    is_bypass_mode && !skip_prompt_already_set
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    #[test]
    fn default_highlights_decline() {
        // Select order is [No, exit] then [Yes, I accept]; default highlight on
        // the first (decline-first), matching the TS Select option order.
        let s = BypassDialogState::default();
        assert_eq!(s.selected, BypassChoice::Decline);
    }

    #[test]
    fn arrows_toggle_choice() {
        let mut s = BypassDialogState::default();
        assert_eq!(handle_key(&mut s, key(KeyCode::Down)), None);
        assert_eq!(s.selected, BypassChoice::Accept);
        let _ = handle_key(&mut s, key(KeyCode::Up));
        assert_eq!(s.selected, BypassChoice::Decline);
    }

    #[test]
    fn enter_on_accept_returns_accept() {
        let mut s = BypassDialogState {
            selected: BypassChoice::Accept,
        };
        assert_eq!(
            handle_key(&mut s, key(KeyCode::Enter)),
            Some(BypassDialogOutcome::Accept)
        );
    }

    #[test]
    fn enter_on_decline_returns_decline() {
        let mut s = BypassDialogState::default();
        assert_eq!(
            handle_key(&mut s, key(KeyCode::Enter)),
            Some(BypassDialogOutcome::Decline)
        );
    }

    #[test]
    fn esc_declines() {
        let mut s = BypassDialogState::default();
        assert_eq!(
            handle_key(&mut s, key(KeyCode::Esc)),
            Some(BypassDialogOutcome::Decline)
        );
    }

    #[test]
    fn render_lines_are_byte_exact() {
        let lines = render_lines();
        assert_eq!(
            lines[0],
            "WARNING: Claude Code running in Bypass Permissions mode"
        );
        assert!(lines.iter().any(|l| l
            == "In Bypass Permissions mode, Claude Code will not ask for your approval before running potentially dangerous commands."));
        assert!(lines.iter().any(|l| l
            == "This mode should only be used in a sandboxed container/VM that has restricted internet access and can easily be restored if damaged."));
        assert!(lines.iter().any(|l| l
            == "By proceeding, you accept all responsibility for actions taken while running in Bypass Permissions mode."));
        assert!(lines
            .iter()
            .any(|l| l == "https://code.claude.com/docs/en/security"));
        assert!(lines.iter().any(|l| l == "No, exit"));
        assert!(lines.iter().any(|l| l == "Yes, I accept"));
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }
}

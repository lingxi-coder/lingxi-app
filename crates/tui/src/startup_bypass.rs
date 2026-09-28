//! Startup bypass-permissions confirmation — pure state machine + a minimal
//! crossterm render for the TTY-only `BypassPermissionsModeDialog` (claude-code
//! `components/BypassPermissionsModeDialog.tsx`, shown before the REPL when
//! bypass is resolved and `skipDangerousModePermissionPrompt` is not yet set).
//! Ported from the iocraft `tui` crate verbatim (already iocraft-free — pure
//! crossterm); only the guard changed to [`crate::raw_screen::RawAltGuard`].

use crossterm::event::{Event, KeyCode, KeyEvent};

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
        KeyCode::Up
        | KeyCode::Down
        | KeyCode::Char('k' | 'j')
        | KeyCode::Tab
        | KeyCode::BackTab => {
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
        "WARNING: LingXi running in Bypass Permissions mode".to_string(),
        "In Bypass Permissions mode, LingXi will not ask for your approval before running potentially dangerous commands.".to_string(),
        "This mode should only be used in a sandboxed container/VM that has restricted internet access and can easily be restored if damaged.".to_string(),
        "By proceeding, you accept all responsibility for actions taken while running in Bypass Permissions mode.".to_string(),
        "https://code.claude.com/docs/en/security".to_string(),
        "No, exit".to_string(),
        "Yes, I accept".to_string(),
    ]
}

/// Whether the bypass dialog must be shown: bypass mode resolved AND no
/// settings tier has `skipDangerousModePermissionPrompt` truthy.
#[must_use]
pub fn should_show_bypass_dialog(is_bypass_mode: bool, skip_prompt_already_set: bool) -> bool {
    is_bypass_mode && !skip_prompt_already_set
}

/// Mount the bypass-permissions confirmation dialog on a real TTY and block
/// until the user accepts or declines. Raw-mode + alt-screen guarded
/// (panic-safe restore). `async` by contract (the CLI `.await`s it); the body
/// is synchronous crossterm terminal I/O.
///
/// # Errors
/// Returns the underlying `std::io::Error` from entering/leaving raw mode, a
/// draw, or a `crossterm::event::read()`. The terminal is restored regardless.
#[allow(clippy::unused_async)]
pub async fn mount_bypass_dialog() -> std::io::Result<BypassDialogOutcome> {
    let guard = crate::raw_screen::RawAltGuard::enter()?;
    let mut state = BypassDialogState::default();

    if let Err(e) = draw_dialog(state) {
        let _ = guard.exit();
        return Err(e);
    }

    let outcome = loop {
        match crossterm::event::read() {
            Ok(Event::Key(key)) => {
                if let Some(outcome) = handle_key(&mut state, key) {
                    break outcome;
                }
                if let Err(e) = draw_dialog(state) {
                    let _ = guard.exit();
                    return Err(e);
                }
            }
            Ok(_) => {
                if let Err(e) = draw_dialog(state) {
                    let _ = guard.exit();
                    return Err(e);
                }
            }
            Err(e) => {
                let _ = guard.exit();
                return Err(e);
            }
        }
    };

    guard.exit()?;
    Ok(outcome)
}

/// Clear the screen and paint the dialog, `> ` marking the highlighted option.
/// Text comes from the byte-locked [`render_lines`].
fn draw_dialog(state: BypassDialogState) -> std::io::Result<()> {
    use crossterm::cursor::MoveTo;
    use crossterm::execute;
    use crossterm::terminal::{Clear, ClearType};
    use std::io::Write;

    let lines = render_lines();
    let (body, decline, accept) = (&lines[..5], &lines[5], &lines[6]);

    let mut out = std::io::stdout();
    execute!(out, Clear(ClearType::All), MoveTo(0, 0))?;
    let mut row: u16 = 0;
    for line in body {
        execute!(out, MoveTo(0, row))?;
        write!(out, "{line}")?;
        row += 1;
    }
    row += 1;
    for (choice, label) in [
        (BypassChoice::Decline, decline),
        (BypassChoice::Accept, accept),
    ] {
        execute!(out, MoveTo(0, row))?;
        let marker = if state.selected == choice { "> " } else { "  " };
        write!(out, "{marker}{label}")?;
        row += 1;
    }
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn default_highlights_decline() {
        assert_eq!(BypassDialogState::default().selected, BypassChoice::Decline);
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
    fn enter_returns_highlighted_choice() {
        let mut a = BypassDialogState {
            selected: BypassChoice::Accept,
        };
        assert_eq!(
            handle_key(&mut a, key(KeyCode::Enter)),
            Some(BypassDialogOutcome::Accept)
        );
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
    fn should_show_only_when_bypass_and_not_already_skipped() {
        assert!(should_show_bypass_dialog(true, false));
        assert!(!should_show_bypass_dialog(true, true));
        assert!(!should_show_bypass_dialog(false, false));
    }

    #[test]
    fn render_lines_are_byte_exact() {
        let lines = render_lines();
        assert_eq!(
            lines[0],
            "WARNING: LingXi running in Bypass Permissions mode"
        );
        assert!(lines.iter().any(|l| l == "In Bypass Permissions mode, LingXi will not ask for your approval before running potentially dangerous commands."));
        assert!(lines.iter().any(|l| l == "This mode should only be used in a sandboxed container/VM that has restricted internet access and can easily be restored if damaged."));
        assert!(lines.iter().any(|l| l == "By proceeding, you accept all responsibility for actions taken while running in Bypass Permissions mode."));
        assert!(lines
            .iter()
            .any(|l| l == "https://code.claude.com/docs/en/security"));
        assert!(lines.iter().any(|l| l == "No, exit"));
        assert!(lines.iter().any(|l| l == "Yes, I accept"));
    }
}

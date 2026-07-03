//! Startup project-trust confirmation — pure state machine + a minimal
//! crossterm render for the TTY-only `TrustDialog` (claude-code
//! `components/TrustDialog/TrustDialog.tsx`, shown by `showSetupScreens` BEFORE
//! the REPL/session when the cwd has not yet been trusted). Ported from the
//! iocraft `tui` crate verbatim (the dialog was already iocraft-free — pure
//! crossterm); only the raw-mode/alt-screen guard changed to
//! [`crate::raw_screen::RawAltGuard`].
//!
//! Pure: `handle_key` drives a two-option Select; the mount loop is thin
//! terminal I/O and cannot be driven headless. ALL decision logic lives in the
//! pure, tested `handle_key` / `render_lines`. The trust STORE
//! (`check_/mark_trust_dialog_accepted`) is owned by the CLI gate.

use crossterm::event::{Event, KeyCode, KeyEvent};
use std::path::Path;

/// The two dialog choices. Select order (`TrustDialog.tsx:227-233`) is
/// ACCEPT-first ("Yes, I trust this folder" then "No, exit"), so the default
/// highlight lands on [`TrustChoice::Accept`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustChoice {
    /// `Yes, I trust this folder`.
    Accept,
    /// `No, exit`.
    Decline,
}

/// Dialog state — just the highlighted choice.
#[derive(Debug, Clone, Copy)]
pub struct TrustDialogState {
    /// Currently highlighted option.
    pub selected: TrustChoice,
}

impl Default for TrustDialogState {
    fn default() -> Self {
        Self {
            selected: TrustChoice::Accept,
        }
    }
}

/// Terminal outcome of the dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustDialogOutcome {
    /// User accepted: persist `hasTrustDialogAccepted` + proceed.
    Accept,
    /// User declined (or pressed Esc/cancel): exit 1.
    Decline,
}

/// Handle one key. `Some(outcome)` ends the dialog; `None` keeps it open.
#[must_use]
pub fn handle_key(state: &mut TrustDialogState, key: KeyEvent) -> Option<TrustDialogOutcome> {
    match key.code {
        KeyCode::Up
        | KeyCode::Down
        | KeyCode::Char('k' | 'j')
        | KeyCode::Tab
        | KeyCode::BackTab => {
            state.selected = match state.selected {
                TrustChoice::Accept => TrustChoice::Decline,
                TrustChoice::Decline => TrustChoice::Accept,
            };
            None
        }
        KeyCode::Enter => Some(match state.selected {
            TrustChoice::Accept => TrustDialogOutcome::Accept,
            TrustChoice::Decline => TrustDialogOutcome::Decline,
        }),
        KeyCode::Esc => Some(TrustDialogOutcome::Decline),
        _ => None,
    }
}

/// The byte-exact display lines (title, cwd, body, link, options, footer).
/// Transcribes `TrustDialog.tsx` faithfully.
#[must_use]
pub fn render_lines(cwd: &Path) -> Vec<String> {
    vec![
        "Accessing workspace:".to_string(),
        cwd.to_string_lossy().into_owned(),
        "Quick safety check: Is this a project you created or one you trust? (Like your own code, a well-known open source project, or work from your team). If not, take a moment to review what's in this folder first.".to_string(),
        "LingXi'll be able to read, edit, and execute files here.".to_string(),
        "Security guide: https://code.claude.com/docs/en/security".to_string(),
        "Yes, I trust this folder".to_string(),
        "No, exit".to_string(),
        "Enter to confirm \u{00B7} Esc to cancel".to_string(),
    ]
}

/// Mount the project-trust confirmation dialog on a real TTY and block until
/// the user accepts or declines. Raw-mode + alt-screen guarded (panic-safe
/// restore). The mount is `async` by contract (the CLI `.await`s it), but the
/// body is synchronous crossterm terminal I/O.
///
/// # Errors
/// Returns the underlying `std::io::Error` from entering/leaving raw mode, a
/// draw, or a `crossterm::event::read()`. The terminal is restored regardless.
#[allow(clippy::unused_async)]
pub async fn mount_trust_dialog(cwd: &Path) -> std::io::Result<TrustDialogOutcome> {
    let guard = crate::raw_screen::RawAltGuard::enter()?;
    let mut state = TrustDialogState::default();

    if let Err(e) = draw_dialog(state, cwd) {
        let _ = guard.exit();
        return Err(e);
    }

    let outcome = loop {
        match crossterm::event::read() {
            Ok(Event::Key(key)) => {
                if let Some(outcome) = handle_key(&mut state, key) {
                    break outcome;
                }
                if let Err(e) = draw_dialog(state, cwd) {
                    let _ = guard.exit();
                    return Err(e);
                }
            }
            Ok(_) => {
                if let Err(e) = draw_dialog(state, cwd) {
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

/// Clear the screen and paint the dialog, `❯ ` marking the highlighted option.
/// Text comes from the byte-locked [`render_lines`].
fn draw_dialog(state: TrustDialogState, cwd: &Path) -> std::io::Result<()> {
    use crossterm::cursor::MoveTo;
    use crossterm::style::{Attribute, SetAttribute};
    use crossterm::terminal::{Clear, ClearType};
    use crossterm::execute;
    use std::io::Write;

    let lines = render_lines(cwd);
    let (body, accept, decline, footer) = (&lines[..5], &lines[5], &lines[6], &lines[7]);

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
        (TrustChoice::Accept, accept),
        (TrustChoice::Decline, decline),
    ] {
        execute!(out, MoveTo(0, row))?;
        let marker = if state.selected == choice {
            "\u{276F} "
        } else {
            "  "
        };
        write!(out, "{marker}{label}")?;
        row += 1;
    }
    row += 1;
    execute!(out, MoveTo(0, row), SetAttribute(Attribute::Dim))?;
    write!(out, "{footer}")?;
    execute!(out, SetAttribute(Attribute::Reset))?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;
    use std::path::PathBuf;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn default_highlights_accept() {
        assert_eq!(TrustDialogState::default().selected, TrustChoice::Accept);
    }

    #[test]
    fn arrows_toggle_choice() {
        let mut s = TrustDialogState::default();
        assert_eq!(handle_key(&mut s, key(KeyCode::Down)), None);
        assert_eq!(s.selected, TrustChoice::Decline);
        let _ = handle_key(&mut s, key(KeyCode::Up));
        assert_eq!(s.selected, TrustChoice::Accept);
    }

    #[test]
    fn enter_returns_highlighted_choice() {
        let mut s = TrustDialogState::default();
        assert_eq!(handle_key(&mut s, key(KeyCode::Enter)), Some(TrustDialogOutcome::Accept));
        let mut d = TrustDialogState { selected: TrustChoice::Decline };
        assert_eq!(handle_key(&mut d, key(KeyCode::Enter)), Some(TrustDialogOutcome::Decline));
    }

    #[test]
    fn esc_declines() {
        let mut s = TrustDialogState::default();
        assert_eq!(handle_key(&mut s, key(KeyCode::Esc)), Some(TrustDialogOutcome::Decline));
    }

    #[test]
    fn render_lines_are_byte_exact() {
        let lines = render_lines(&PathBuf::from("/home/me/project"));
        assert_eq!(lines[0], "Accessing workspace:");
        assert_eq!(lines[1], "/home/me/project");
        assert_eq!(lines[2], "Quick safety check: Is this a project you created or one you trust? (Like your own code, a well-known open source project, or work from your team). If not, take a moment to review what's in this folder first.");
        assert_eq!(lines[3], "LingXi'll be able to read, edit, and execute files here.");
        assert_eq!(lines[4], "Security guide: https://code.claude.com/docs/en/security");
        assert_eq!(lines[5], "Yes, I trust this folder");
        assert_eq!(lines[6], "No, exit");
        assert_eq!(lines[7], "Enter to confirm \u{00B7} Esc to cancel");
    }
}

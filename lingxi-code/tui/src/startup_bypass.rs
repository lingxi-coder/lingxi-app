//! Startup bypass-permissions confirmation — pure state machine + render for
//! the TTY-only `BypassPermissionsModeDialog` (claude-code
//! `components/BypassPermissionsModeDialog.tsx`, shown by `interactiveHelpers
//! showSetupScreens` before the REPL when bypass is resolved and
//! `skipDangerousModePermissionPrompt` is not yet set).
//!
//! Pure: `handle_key` drives a two-option Select; the terminal mount loop
//! lives in the CLI (`apps/cli/src/mode.rs`) and is thin (cannot be driven
//! headless, same caveat as `run_tui_session`).

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
/// settings tier has `skipDangerousModePermissionPrompt` truthy
/// (`hasSkipDangerousModePermissionPrompt`, claude-code settings.ts:882-889 —
/// user+local here; flag/policy tiers have no Rust substrate).
#[must_use]
pub fn should_show_bypass_dialog(is_bypass_mode: bool, skip_prompt_already_set: bool) -> bool {
    is_bypass_mode && !skip_prompt_already_set
}

/// Mount the bypass-permissions confirmation dialog on a real TTY and block
/// until the user accepts or declines.
///
/// Acquires the crate's [`crate::terminal::RawGuard`] (raw mode + alt screen +
/// panic-safe restore), paints [`render_lines`] with the highlighted option,
/// then loops on `crossterm::event::read()` feeding [`handle_key`] until it
/// returns an outcome. The guard restores the terminal on EVERY return path
/// (outcome OR error) via its `exit()` / `Drop`.
///
/// UNTESTABLE-HEADLESS CAVEAT: this is the one piece that can't be driven
/// without a TTY (same caveat as `run_tui_session`) — it is deliberately
/// minimal and side-effect-only. ALL decision logic lives in the pure,
/// fully-tested [`handle_key`] / [`should_show_bypass_dialog`]; this wrapper
/// only does terminal I/O.
///
/// # Errors
/// Returns the underlying `std::io::Error` if entering/leaving raw mode, a
/// draw, or a `crossterm::event::read()` fails. The terminal is restored
/// regardless.
// `async` with no `.await`: the body is synchronous crossterm terminal I/O, but
// the fn is `async` BY CONTRACT — the mount seam is `.await`-ed in the CLI
// (`mode::dispatch`), mirroring `run_tui_session`, so the interactive entry
// points stay uniformly async. The lint is allowed, not worked around.
#[allow(clippy::unused_async)]
pub async fn mount_bypass_dialog() -> std::io::Result<BypassDialogOutcome> {
    let guard = crate::terminal::RawGuard::enter()?;
    let mut state = BypassDialogState::default();

    // First paint, then re-paint after each navigation key.
    if let Err(e) = draw_dialog(state) {
        // Best-effort restore before surfacing the draw error.
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
            // Resize / focus / paste / mouse: re-paint defensively and keep going.
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

    // Voluntary restore so an IO error leaving the alt screen is surfaced
    // rather than swallowed in `Drop`.
    guard.exit()?;
    Ok(outcome)
}

/// Clear the screen and paint the dialog with the selected option marked
/// (`> ` prefix on the highlighted row). Minimal crossterm render — the exact
/// text comes from the byte-locked [`render_lines`]. Takes `state` by value
/// (it is `Copy` — one enum field).
fn draw_dialog(state: BypassDialogState) -> std::io::Result<()> {
    use crossterm::{
        cursor::MoveTo,
        execute,
        terminal::{Clear, ClearType},
    };
    use std::io::Write;

    let lines = render_lines();
    // render_lines layout: [title, body1, body2, body3, link, decline, accept].
    let (body, decline, accept) = (&lines[..5], &lines[5], &lines[6]);

    let mut out = std::io::stdout();
    execute!(out, Clear(ClearType::All), MoveTo(0, 0))?;
    let mut row: u16 = 0;
    for line in body {
        execute!(out, MoveTo(0, row))?;
        write!(out, "{line}")?;
        row += 1;
    }
    // Blank spacer row before the two options.
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
            "WARNING: LingXi running in Bypass Permissions mode"
        );
        assert!(lines.iter().any(|l| l
            == "In Bypass Permissions mode, LingXi will not ask for your approval before running potentially dangerous commands."));
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

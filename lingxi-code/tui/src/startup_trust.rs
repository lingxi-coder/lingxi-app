//! Startup project-trust confirmation — pure state machine + render for the
//! TTY-only `TrustDialog` (claude-code `components/TrustDialog/TrustDialog.tsx`,
//! shown by `showSetupScreens` BEFORE the REPL/session when the cwd has not yet
//! been trusted, i.e. `!checkHasTrustDialogAccepted()`).
//!
//! Pure: `handle_key` drives a two-option Select; the terminal mount loop is
//! thin terminal I/O and (like `run_tui_session` / `mount_bypass_dialog`)
//! cannot be driven headless. ALL decision logic lives in the pure, tested
//! `handle_key` / `render_lines`.
//!
//! The trust STORE itself (`check_/mark_trust_dialog_accepted`,
//! `global_config_path`) is owned by the CLI gate (`apps/cli/src/mode.rs`),
//! which already depends on `migrations`; this module stays a pure terminal
//! component (no `migrations` dep), mirroring how `startup_bypass` splits the
//! predicate/store logic out of the `tui` crate.

use crossterm::event::{Event, KeyCode, KeyEvent};
use std::path::Path;

/// The two dialog choices.
///
/// Select option order (`TrustDialog.tsx:227-233`) is ACCEPT-first
/// ("Yes, I trust this folder" then "No, exit") — the OPPOSITE of
/// `startup_bypass` (which is decline-first). The default highlight follows
/// the TS Select's "first option highlighted" behaviour, so the default lands
/// on [`TrustChoice::Accept`].
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
        // Accept-first highlight (matches the TS Select option order, which
        // lists "Yes, I trust this folder" first — `TrustDialog.tsx:227-233`).
        Self {
            selected: TrustChoice::Accept,
        }
    }
}

/// Terminal outcome of the dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustDialogOutcome {
    /// User accepted: persist `hasTrustDialogAccepted` + proceed
    /// (`TrustDialog.tsx:177` → `saveCurrentProjectConfig`).
    Accept,
    /// User declined (or pressed Esc/cancel): exit 1
    /// (`TrustDialog.tsx:158-160` `value === "exit"` → `gracefulShutdownSync(1)`,
    /// and `onCancel={() => onChange("exit")}`).
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
        // Esc → `onCancel` → `onChange("exit")` → decline (`TrustDialog.tsx:240`).
        KeyCode::Esc => Some(TrustDialogOutcome::Decline),
        _ => None,
    }
}

/// The byte-exact display lines (title, cwd, body, link, options).
///
/// Transcribes `TrustDialog.tsx` faithfully:
///   - `[0]` title       `:257` `title="Accessing workspace:"`
///   - `[1]` cwd (bold)  `:207` `<Text bold>{getFsImplementation().cwd()}</Text>`
///   - `[2]` body        `:208` "Quick safety check: …"
///   - `[3]` body        `:209` "Claude Code'll be able to read, edit, and execute files here."
///   - `[4]` link        `:220` security guide URL
///   - `[5]` option      `:228` "Yes, I trust this folder" (Accept-first)
///   - `[6]` option      `:231` "No, exit"
///   - `[7]` footer      `:248` "Enter to confirm · Esc to cancel" (dimmed)
#[must_use]
pub fn render_lines(cwd: &Path) -> Vec<String> {
    vec![
        "Accessing workspace:".to_string(),
        cwd.to_string_lossy().into_owned(),
        "Quick safety check: Is this a project you created or one you trust? (Like your own code, a well-known open source project, or work from your team). If not, take a moment to review what's in this folder first.".to_string(),
        "LingXi'll be able to read, edit, and execute files here.".to_string(),
        // TrustDialog.tsx:220 `<Link url=".../security">Security guide</Link>`.
        // Terminals can't render Ink's clickable link, so surface the label +
        // href (matching the REPL gate's `Security guide: {url}` rendering).
        "Security guide: https://code.claude.com/docs/en/security".to_string(),
        "Yes, I trust this folder".to_string(),
        "No, exit".to_string(),
        // (TRUST-1) dimmed footer hint (TrustDialog.tsx:248). Rendered dim +
        // after a spacer by `draw_dialog`.
        "Enter to confirm \u{00B7} Esc to cancel".to_string(),
    ]
}

/// Mount the project-trust confirmation dialog on a real TTY and block until
/// the user accepts or declines.
///
/// Acquires the crate's [`crate::terminal::RawGuard`] (raw mode + alt screen +
/// panic-safe restore), paints [`render_lines`] with the highlighted option,
/// then loops on `crossterm::event::read()` feeding [`handle_key`] until it
/// returns an outcome. The guard restores the terminal on EVERY return path
/// (outcome OR error) via its `exit()` / `Drop`.
///
/// UNTESTABLE-HEADLESS CAVEAT: this is the one piece that can't be driven
/// without a TTY (same caveat as `run_tui_session` / `mount_bypass_dialog`) —
/// it is deliberately minimal and side-effect-only. ALL decision logic lives in
/// the pure, fully-tested [`handle_key`] / [`render_lines`]; this wrapper only
/// does terminal I/O.
///
/// # Errors
/// Returns the underlying `std::io::Error` if entering/leaving raw mode, a
/// draw, or a `crossterm::event::read()` fails. The terminal is restored
/// regardless.
// `async` with no `.await`: the body is synchronous crossterm terminal I/O, but
// the fn is `async` BY CONTRACT — the mount seam is `.await`-ed in the CLI
// (`mode::dispatch`), mirroring `run_tui_session` / `mount_bypass_dialog`, so
// the interactive entry points stay uniformly async. The lint is allowed.
#[allow(clippy::unused_async)]
pub async fn mount_trust_dialog(cwd: &Path) -> std::io::Result<TrustDialogOutcome> {
    let guard = crate::terminal::RawGuard::enter()?;
    let mut state = TrustDialogState::default();

    // First paint, then re-paint after each navigation key.
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
            // Resize / focus / paste / mouse: re-paint defensively and keep going.
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

    // Voluntary restore so an IO error leaving the alt screen is surfaced
    // rather than swallowed in `Drop`.
    guard.exit()?;
    Ok(outcome)
}

/// Clear the screen and paint the dialog with the selected option marked
/// (`> ` prefix on the highlighted row). Minimal crossterm render — the exact
/// text comes from the byte-locked [`render_lines`]. Takes `state` by value
/// (it is `Copy` — one enum field).
fn draw_dialog(state: TrustDialogState, cwd: &Path) -> std::io::Result<()> {
    use crossterm::{
        cursor::MoveTo,
        execute,
        terminal::{Clear, ClearType},
    };
    use std::io::Write;

    use crossterm::style::{Attribute, SetAttribute};

    let lines = render_lines(cwd);
    // render_lines layout: [title, cwd, body1, body2, link, accept, decline, footer].
    let (body, accept, decline, footer) = (&lines[..5], &lines[5], &lines[6], &lines[7]);

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
        (TrustChoice::Accept, accept),
        (TrustChoice::Decline, decline),
    ] {
        execute!(out, MoveTo(0, row))?;
        // (TRUST-2) figures.pointer `❯ ` on the highlighted row (CustomSelect).
        let marker = if state.selected == choice {
            "\u{276F} "
        } else {
            "  "
        };
        write!(out, "{marker}{label}")?;
        row += 1;
    }
    // (TRUST-1) dimmed footer hint after a spacer row.
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
        // Select order is [Yes, I trust this folder] then [No, exit]; default
        // highlight on the first (accept-first), matching the TS Select option
        // order (`TrustDialog.tsx:227-233`).
        let s = TrustDialogState::default();
        assert_eq!(s.selected, TrustChoice::Accept);
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
    fn enter_on_accept_returns_accept() {
        let mut s = TrustDialogState::default();
        assert_eq!(
            handle_key(&mut s, key(KeyCode::Enter)),
            Some(TrustDialogOutcome::Accept)
        );
    }

    #[test]
    fn enter_on_decline_returns_decline() {
        let mut s = TrustDialogState {
            selected: TrustChoice::Decline,
        };
        assert_eq!(
            handle_key(&mut s, key(KeyCode::Enter)),
            Some(TrustDialogOutcome::Decline)
        );
    }

    #[test]
    fn esc_declines() {
        let mut s = TrustDialogState::default();
        assert_eq!(
            handle_key(&mut s, key(KeyCode::Esc)),
            Some(TrustDialogOutcome::Decline)
        );
    }

    #[test]
    fn render_lines_are_byte_exact() {
        let cwd = PathBuf::from("/home/me/project");
        let lines = render_lines(&cwd);
        assert_eq!(lines[0], "Accessing workspace:");
        assert_eq!(lines[1], "/home/me/project");
        assert_eq!(lines[2], "Quick safety check: Is this a project you created or one you trust? (Like your own code, a well-known open source project, or work from your team). If not, take a moment to review what's in this folder first.");
        assert_eq!(
            lines[3],
            "LingXi'll be able to read, edit, and execute files here."
        );
        assert_eq!(
            lines[4],
            "Security guide: https://code.claude.com/docs/en/security"
        );
        assert_eq!(lines[5], "Yes, I trust this folder");
        assert_eq!(lines[6], "No, exit");
        // (TRUST-1) dimmed footer hint.
        assert_eq!(lines[7], "Enter to confirm \u{00B7} Esc to cancel");
    }
}

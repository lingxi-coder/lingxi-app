//! Startup approval dialog for LINGXI.md imports outside the working directory.

#![forbid(unsafe_code)]

use crossterm::event::{Event, KeyCode, KeyEvent};
use std::path::{Path, PathBuf};

/// User decision returned by the external-includes dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalIncludesDialogOutcome {
    /// Enable the listed imports for this project.
    Accept,
    /// Keep external imports disabled and continue startup.
    Decline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Choice {
    Accept,
    Decline,
}

#[derive(Debug, Clone, Copy)]
struct State {
    selected: Choice,
}

impl Default for State {
    fn default() -> Self {
        Self {
            selected: Choice::Accept,
        }
    }
}

fn handle_key(state: &mut State, key: KeyEvent) -> Option<ExternalIncludesDialogOutcome> {
    match key.code {
        KeyCode::Up | KeyCode::Left => state.selected = Choice::Accept,
        KeyCode::Down | KeyCode::Right | KeyCode::Tab => state.selected = Choice::Decline,
        KeyCode::Char('1') => return Some(ExternalIncludesDialogOutcome::Accept),
        KeyCode::Char('2') => return Some(ExternalIncludesDialogOutcome::Decline),
        KeyCode::Enter => {
            return Some(match state.selected {
                Choice::Accept => ExternalIncludesDialogOutcome::Accept,
                Choice::Decline => ExternalIncludesDialogOutcome::Decline,
            });
        }
        KeyCode::Esc => return Some(ExternalIncludesDialogOutcome::Decline),
        _ => {}
    }
    None
}

/// Mount the one-shot external-includes dialog in a restored-on-drop alternate
/// screen. The caller persists the returned decision.
pub async fn mount_external_includes_dialog(
    paths: &[PathBuf],
) -> std::io::Result<ExternalIncludesDialogOutcome> {
    let guard = crate::raw_screen::RawAltGuard::enter()?;
    let mut state = State::default();

    if let Err(error) = draw_dialog(state, paths) {
        let _ = guard.exit();
        return Err(error);
    }
    let outcome = loop {
        match crossterm::event::read() {
            Ok(Event::Key(key)) => {
                if let Some(outcome) = handle_key(&mut state, key) {
                    break outcome;
                }
                if let Err(error) = draw_dialog(state, paths) {
                    let _ = guard.exit();
                    return Err(error);
                }
            }
            Ok(_) => {
                if let Err(error) = draw_dialog(state, paths) {
                    let _ = guard.exit();
                    return Err(error);
                }
            }
            Err(error) => {
                let _ = guard.exit();
                return Err(error);
            }
        }
    };
    guard.exit()?;
    Ok(outcome)
}

fn render_lines(paths: &[PathBuf]) -> Vec<String> {
    let mut lines = vec![
        "External LINGXI.md includes".to_string(),
        "This project's LINGXI.md imports files outside the current working directory. Never allow this for third-party repositories.".to_string(),
        "External imports:".to_string(),
    ];
    lines.extend(paths.iter().map(|path| format!("  {}", display_path(path))));
    lines.push(String::new());
    lines.push("Allow external LINGXI.md file imports?".to_string());
    lines
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn draw_dialog(state: State, paths: &[PathBuf]) -> std::io::Result<()> {
    use crossterm::cursor::MoveTo;
    use crossterm::execute;
    use crossterm::style::{Attribute, SetAttribute};
    use crossterm::terminal::{Clear, ClearType};
    use std::io::Write;

    let mut out = std::io::stdout();
    execute!(out, Clear(ClearType::All), MoveTo(0, 0))?;
    let mut row = 0u16;
    for line in render_lines(paths) {
        execute!(out, MoveTo(0, row))?;
        write!(out, "{line}")?;
        row = row.saturating_add(1);
    }
    for (choice, label) in [
        (Choice::Accept, "Yes, allow external imports"),
        (Choice::Decline, "No, disable external imports"),
    ] {
        execute!(out, MoveTo(0, row))?;
        let marker = if state.selected == choice {
            "\u{276F} "
        } else {
            "  "
        };
        write!(out, "{marker}{label}")?;
        row = row.saturating_add(1);
    }
    row = row.saturating_add(1);
    execute!(out, MoveTo(0, row), SetAttribute(Attribute::Dim))?;
    write!(out, "Enter to confirm \u{00B7} Esc to cancel")?;
    execute!(out, SetAttribute(Attribute::Reset))?;
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
    fn dialog_copy_and_path_order_are_locked() {
        let lines = render_lines(&[
            PathBuf::from("/outside/a.md"),
            PathBuf::from("/outside/b.md"),
        ]);
        assert_eq!(lines[0], "External LINGXI.md includes");
        assert_eq!(lines[1], "This project's LINGXI.md imports files outside the current working directory. Never allow this for third-party repositories.");
        assert_eq!(lines[2], "External imports:");
        assert_eq!(lines[3], "  /outside/a.md");
        assert_eq!(lines[4], "  /outside/b.md");
        assert_eq!(lines[6], "Allow external LINGXI.md file imports?");
    }

    #[test]
    fn accept_is_default_and_escape_declines() {
        let mut state = State::default();
        assert_eq!(
            handle_key(&mut state, key(KeyCode::Enter)),
            Some(ExternalIncludesDialogOutcome::Accept)
        );
        assert_eq!(
            handle_key(&mut state, key(KeyCode::Esc)),
            Some(ExternalIncludesDialogOutcome::Decline)
        );
    }

    #[test]
    fn arrows_and_number_shortcuts_choose_both_outcomes() {
        let mut state = State::default();
        assert_eq!(handle_key(&mut state, key(KeyCode::Down)), None);
        assert_eq!(
            handle_key(&mut state, key(KeyCode::Enter)),
            Some(ExternalIncludesDialogOutcome::Decline)
        );
        assert_eq!(
            handle_key(&mut state, key(KeyCode::Char('1'))),
            Some(ExternalIncludesDialogOutcome::Accept)
        );
        assert_eq!(
            handle_key(&mut state, key(KeyCode::Char('2'))),
            Some(ExternalIncludesDialogOutcome::Decline)
        );
    }
}

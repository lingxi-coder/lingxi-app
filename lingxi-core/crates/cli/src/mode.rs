//! Three-way mode dispatch added in M6-01. Routes argv to one of:
//! - `Mode::Print(prompt)`: existing `run::run_oneshot` (v0.6.0, unchanged)
//! - `Mode::Tui`: new `lingxi_tui::run_tui_session` (v0.7.0 default)
//! - `Mode::StdioRepl`: existing `repl::run_repl` (v0.6.0, kept as `--no-tui`
//!   and non-TTY fallback)
//!
//! TTY detection uses [`std::io::IsTerminal`] (stable since Rust 1.70). If
//! stdin OR stdout isn't a terminal, we fall back to `StdioRepl` (CI, pipes,
//! redirected I/O). The `--no-tui` flag forces `StdioRepl` regardless of
//! TTY state.

use crate::argv::Argv;
use std::io::IsTerminal;

/// The resolved execution mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// One-shot print mode: prompt is set, exit after first end_turn.
    /// Carries the prompt string so the caller doesn't re-clone argv.
    Print(String),
    /// Fullscreen iocraft TUI (default when no prompt + TTY + no `--no-tui`).
    Tui,
    /// Line-based stdio REPL from v0.6.0 (`--no-tui` or non-TTY).
    StdioRepl,
}

/// Pick a mode from argv + the current process's TTY state.
///
/// Decision tree:
/// 1. If `argv.prompt` has a non-empty value → `Print(prompt)`.
/// 2. Else if `argv.no_tui` is set OR stdin/stdout isn't a TTY → `StdioRepl`.
/// 3. Else → `Tui`.
#[must_use]
pub fn decide_mode(argv: &Argv) -> Mode {
    decide_mode_with(argv, is_full_tty())
}

/// Test seam: `is_tty` argument decouples the decision from the real
/// process TTY state.
#[must_use]
pub fn decide_mode_with(argv: &Argv, is_tty: bool) -> Mode {
    if let Some(p) = argv.prompt.as_deref() {
        if !p.trim().is_empty() {
            return Mode::Print(p.to_string());
        }
    }
    if argv.no_tui || !is_tty {
        return Mode::StdioRepl;
    }
    Mode::Tui
}

fn is_full_tty() -> bool {
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(prompt: Option<&str>, no_tui: bool) -> Argv {
        Argv {
            prompt: prompt.map(String::from),
            print: false,
            resume: None,
            model: None,
            cwd: None,
            no_stream: false,
            json: false,
            debug: false,
            no_tui,
        }
    }

    #[test]
    fn prompt_routes_to_print() {
        let a = argv(Some("fix it"), false);
        assert_eq!(decide_mode_with(&a, true), Mode::Print("fix it".into()));
    }

    #[test]
    fn empty_prompt_does_not_route_to_print() {
        let a = argv(Some("   "), false);
        assert_eq!(decide_mode_with(&a, true), Mode::Tui);
    }

    #[test]
    fn no_prompt_tty_routes_to_tui() {
        let a = argv(None, false);
        assert_eq!(decide_mode_with(&a, true), Mode::Tui);
    }

    #[test]
    fn no_tui_flag_forces_stdio_repl() {
        let a = argv(None, true);
        assert_eq!(decide_mode_with(&a, true), Mode::StdioRepl);
    }

    #[test]
    fn non_tty_routes_to_stdio_repl() {
        let a = argv(None, false);
        assert_eq!(decide_mode_with(&a, false), Mode::StdioRepl);
    }

    #[test]
    fn non_tty_with_prompt_still_prints() {
        // -p with piped input should still print one-shot (matches v0.6.0).
        let a = argv(Some("hi"), false);
        assert_eq!(decide_mode_with(&a, false), Mode::Print("hi".into()));
    }
}

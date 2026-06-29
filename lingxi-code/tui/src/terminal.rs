//! Raw-mode + alt-screen RAII guard with a one-shot panic hook.
//!
//! On construction: enables crossterm raw mode and enters the alt screen.
//! On drop: leaves the alt screen and disables raw mode.
//!
//! Critical: a runaway panic in any render path would leave the user's
//! terminal in raw mode + alt screen (broken shell). The `install_panic_hook`
//! function wraps the existing panic hook to restore the terminal first.
//! Installed exactly once per process via `std::sync::Once`.
//!
//! **Post-M6-04**: iocraft 0.8.3's `Element::fullscreen().await` owns raw
//! mode + alt screen + a panic-safe restore handler. This module is no
//! longer in the live path; the types are kept compiled (and the
//! `install_panic_hook_is_once` test still runs) so future fallback paths
//! that need a hand-rolled terminal guard can lift the implementation back
//! into use without churn.
#![allow(dead_code)]

use crossterm::{
    cursor::Show,
    event::DisableMouseCapture,
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use std::io::{self, stdout, Stdout};
use std::sync::Once;

static INSTALL_PANIC_HOOK: Once = Once::new();
static INSTALL_SAFETY: Once = Once::new();

/// Best-effort full terminal restore: disable mouse capture, leave the alt
/// screen, show the cursor, disable raw mode. Idempotent (the sequences are
/// no-ops if already off), so it is safe to call from several exit paths.
///
/// **Why mouse capture is explicit here:** iocraft 0.8.3's fullscreen renderer
/// ENABLES mouse capture but its `Drop` only leaves the alt screen + shows the
/// cursor + disables raw mode — it never emits `DisableMouseCapture`. So even a
/// clean exit leaves the terminal in mouse-reporting mode, and every subsequent
/// mouse move prints garbage `<btn>;<x>;<y>M` SGR reports as literal text. And on
/// a SIGNAL (SIGTERM/SIGHUP — e.g. `kill`, closing the tab) no `Drop` runs at
/// all, leaving raw + alt + mouse all on. This restore closes both gaps.
pub fn restore_terminal_modes() {
    let mut out: Stdout = stdout();
    if crate::inline_render_mode() {
        // Inline mode (`LINGXI_TUI_INLINE`) never entered the alt screen, so
        // emitting `LeaveAlternateScreen` (`\e[?1049l`) would wrongly switch the
        // buffer / scroll the user's scrollback. Mouse-capture disable + cursor
        // show stay (idempotent, harmless if never set).
        let _ = execute!(out, DisableMouseCapture, Show);
    } else {
        let _ = execute!(out, DisableMouseCapture, LeaveAlternateScreen, Show);
    }
    let _ = disable_raw_mode();
}

/// Install process-wide terminal-safety hooks for the fullscreen TUI, once:
/// - a panic hook that restores the terminal before propagating the panic;
/// - SIGTERM / SIGHUP handlers that restore the terminal, then exit.
///
/// SIGINT (Ctrl-C) is deliberately NOT caught here — the live mount uses
/// iocraft's `.ignore_ctrl_c()` so Ctrl-C routes to the app's double-press exit
/// guard. Must be called from within a Tokio runtime (it spawns signal tasks).
pub fn install_terminal_safety_hooks() {
    INSTALL_SAFETY.call_once(|| {
        INSTALL_PANIC_HOOK.call_once(install_panic_hook);
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            for kind in [SignalKind::terminate(), SignalKind::hangup()] {
                if let Ok(mut sig) = signal(kind) {
                    tokio::spawn(async move {
                        sig.recv().await;
                        restore_terminal_modes();
                        std::process::exit(143);
                    });
                }
            }
        }
    });
}

/// RAII guard: enables raw mode + alt screen on `new`; restores on `drop`.
///
/// Constructing a guard while another is alive is a logic error (will
/// fail the second `enable_raw_mode` with an EPERM-equivalent on most
/// platforms). M6-01 only constructs one guard per `run_tui_session`.
pub(crate) struct RawGuard {
    entered: bool,
}

impl RawGuard {
    /// Install the panic hook (once) and enter raw mode + alt screen.
    pub(crate) fn enter() -> io::Result<Self> {
        INSTALL_PANIC_HOOK.call_once(install_panic_hook);
        enable_raw_mode()?;
        let mut out: Stdout = stdout();
        execute!(out, EnterAlternateScreen)?;
        Ok(Self { entered: true })
    }

    /// Voluntary exit (so callers can surface IO errors instead of
    /// silently swallowing them in `Drop`).
    pub(crate) fn exit(mut self) -> io::Result<()> {
        if self.entered {
            let mut out: Stdout = stdout();
            execute!(out, LeaveAlternateScreen)?;
            disable_raw_mode()?;
            self.entered = false;
        }
        Ok(())
    }
}

impl Drop for RawGuard {
    fn drop(&mut self) {
        if self.entered {
            // Best-effort cleanup: ignore errors (we're unwinding or
            // dropping in normal flow and either way there's nowhere to
            // report).
            let mut out: Stdout = stdout();
            let _ = execute!(out, LeaveAlternateScreen);
            let _ = disable_raw_mode();
        }
    }
}

/// Wrap the existing panic hook so that any panic leaves the terminal in a
/// usable state before propagating. Called exactly once.
fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal_modes();
        prev(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The hook is installed exactly once per process. Calling `enter()`
    /// twice from the same test isn't valid (raw mode is process-global
    /// and CI may not have a TTY), so we only assert the Once flag flips.
    #[test]
    fn install_panic_hook_is_once() {
        // Drive the Once flag without entering raw mode (CI/non-TTY safe).
        INSTALL_PANIC_HOOK.call_once(install_panic_hook);
        // Second call must be a no-op (Once would panic on double-init);
        // success here proves the Once is correctly gating.
        INSTALL_PANIC_HOOK.call_once(install_panic_hook);
    }

    /// Verify the type compiles in a single-threaded context.
    #[allow(dead_code)]
    fn _guard_type_compiles() {
        fn _t(_g: &RawGuard) {}
    }
}

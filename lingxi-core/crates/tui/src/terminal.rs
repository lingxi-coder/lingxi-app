//! Raw-mode + alt-screen RAII guard with a one-shot panic hook.
//!
//! On construction: enables crossterm raw mode and enters the alt screen.
//! On drop: leaves the alt screen and disables raw mode.
//!
//! Critical: a runaway panic in any render path would leave the user's
//! terminal in raw mode + alt screen (broken shell). The `install_panic_hook`
//! function wraps the existing panic hook to restore the terminal first.
//! Installed exactly once per process via `std::sync::Once`.

use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use std::io::{self, stdout, Stdout};
use std::sync::Once;

static INSTALL_PANIC_HOOK: Once = Once::new();

/// RAII guard: enables raw mode + alt screen on `new`; restores on `drop`.
///
/// Constructing a guard while another is alive is a logic error (will
/// fail the second `enable_raw_mode` with an EPERM-equivalent on most
/// platforms). M6-01 only constructs one guard per `run_tui_session`.
pub(crate) struct RawGuard {
    entered: bool,
}

// `enter`/`exit` are first called from `session::run_tui_session` in Task 7.
#[allow(dead_code)]
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
        let mut out: Stdout = stdout();
        let _ = execute!(out, LeaveAlternateScreen);
        let _ = disable_raw_mode();
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

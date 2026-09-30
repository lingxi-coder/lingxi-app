//! A minimal raw-mode + alternate-screen guard for the standalone startup
//! dialogs (project-trust, bypass-permissions). Ported from the `tui` crate's
//! `RawGuard` (iocraft-free — pure crossterm). Restores the terminal on every
//! path including panic/unwind via `Drop`; callers can also `exit()` voluntarily
//! to surface a restore IO error instead of swallowing it in `Drop`.

use std::io::{self, Stdout};

use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};

/// RAII guard: raw mode + alternate screen on [`Self::enter`], restored on
/// [`Self::exit`] or `Drop`.
pub(crate) struct RawAltGuard {
    entered: bool,
}

impl RawAltGuard {
    /// Enter raw mode + the alternate screen.
    ///
    /// # Errors
    /// Returns any terminal IO error from enabling raw mode or switching screens.
    pub(crate) fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut out: Stdout = io::stdout();
        execute!(out, EnterAlternateScreen)?;
        Ok(Self { entered: true })
    }

    /// Voluntary restore (leave alt screen, disable raw mode) so callers can
    /// surface an IO error instead of swallowing it in `Drop`.
    ///
    /// # Errors
    /// Returns any terminal IO error from leaving the alt screen or disabling raw mode.
    pub(crate) fn exit(mut self) -> io::Result<()> {
        if self.entered {
            let mut out: Stdout = io::stdout();
            execute!(out, LeaveAlternateScreen)?;
            disable_raw_mode()?;
            self.entered = false;
        }
        Ok(())
    }
}

impl Drop for RawAltGuard {
    fn drop(&mut self) {
        if self.entered {
            // Best-effort cleanup on unwind/normal drop; nowhere to report.
            let mut out: Stdout = io::stdout();
            let _ = execute!(out, LeaveAlternateScreen);
            let _ = disable_raw_mode();
        }
    }
}

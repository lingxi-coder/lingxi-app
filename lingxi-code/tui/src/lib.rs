//! `tui` — Ratatui-based terminal UI runtime for LingXi.
//!
//! The target backend of the iocraft → ratatui migration. It renders the
//! backend-neutral model in `tui-core` (state, render model, theme, message
//! types) and owns the terminal lifecycle + draw/event loop. It must NEVER
//! depend on `iocraft`.
//!
//! The runtime terminal substrate is [`terminal`]: a codex-derived
//! bottom-anchored inline terminal whose viewport is an absolute on-screen
//! rect. Finalized history is inserted ABOVE the viewport into the terminal's
//! native scrollback ([`terminal::Terminal::insert_history_lines`]); the
//! bottom viewport (status + composer + overlays) is diff-redrawn in place.
//!
//! See `.omo/plans/2026-07-02-tui-rata-codex-ui-structure-parity.md`.
#![forbid(unsafe_code)]

pub mod agents_screen;
pub mod app;
pub mod bottom_pane;
pub mod chat_widget;
pub mod color;
pub mod command;
pub mod composer;
pub mod connect;
pub mod copy;
pub mod diff;
pub mod export;
pub mod files;
pub mod history_cell;
pub mod image_view;
pub mod message;
pub mod rate_limit_messages;
pub mod render;
pub mod resume;
pub mod replay;
pub mod screen_reader;
pub mod permission_gate;
pub mod startup_trust;
pub mod startup_bypass;
pub mod raw_screen;
pub mod renderable;
pub mod session;
pub mod spinner;
pub mod spinner_status;
pub mod status_line;
pub(crate) mod style;
pub mod style_adapter;
pub mod term_image;
pub mod terminal;
pub mod transcript;
pub mod vim;
pub mod web;

use std::io::Stdout;

use ratatui::backend::CrosstermBackend;
pub use terminal::TerminalSession;
pub use tui_core::message::RenderedMessage;
pub use tui_core::orchestrator_bridge::TurnEvent;

/// The concrete runtime terminal: the bottom-anchored custom [`terminal`]
/// over crossterm stdout. This replaced the earlier scaffold alias to
/// `ratatui::Terminal` with `Viewport::Inline` (which ghost-stacked frames
/// and required full terminal re-creation on every height change).
pub type RataTerminal = terminal::Terminal<CrosstermBackend<Stdout>>;

/// Standard ratatui terminal for standalone full-screen views — the M7
/// `claude agents` view (`agents_screen`) owns the whole alternate screen for
/// its lifetime and renders into a plain [`ratatui::Frame`], distinct from the
/// chat app's bottom-anchored custom [`RataTerminal`]. Kept a separate type so
/// the standalone screen (its own event loop, no `ChatWidget`/`BottomPane`) is
/// not coupled to the custom terminal's inline-viewport machinery.
pub type AgentsTerminal = ratatui::Terminal<CrosstermBackend<Stdout>>;

/// Enter raw mode + the alternate screen and construct a standalone ratatui
/// terminal for a full-screen view (M7 `claude agents`).
///
/// # Errors
/// Returns any terminal IO error from enabling raw mode, switching screens, or
/// constructing the backend.
pub fn setup_terminal() -> std::io::Result<AgentsTerminal> {
    crossterm::terminal::enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    crossterm::execute!(stdout, crossterm::terminal::EnterAlternateScreen)?;
    ratatui::Terminal::new(CrosstermBackend::new(stdout))
}

/// Restore the terminal to its pre-view state (leave the alt screen, disable
/// raw mode, show the cursor). Safe to call during unwind/exit.
///
/// # Errors
/// Returns any terminal IO error from restoring screen/cursor state.
pub fn restore_terminal(terminal: &mut AgentsTerminal) -> std::io::Result<()> {
    crossterm::terminal::disable_raw_mode()?;
    crossterm::execute!(
        terminal.backend_mut(),
        crossterm::terminal::LeaveAlternateScreen
    )?;
    terminal.show_cursor()
}

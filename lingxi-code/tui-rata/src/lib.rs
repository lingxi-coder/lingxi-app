//! `tui-rata` — Ratatui-based terminal UI runtime for LingXi.
//!
//! The target backend of the iocraft → ratatui migration. It renders the
//! backend-neutral model in `tui-core` (state, render model, theme, message
//! types) and owns the terminal lifecycle + draw/event loop. It must NEVER
//! depend on `iocraft`.
//!
//! Phase 1 (this file) is the runtime shell: enter raw mode + alternate
//! screen, draw the 3-zone layout (scrollback / status / composer), block for
//! a key, then restore the terminal cleanly. Later phases fill the zones by
//! consuming `tui_core` renderers.
//!
//! See `.omo/plans/2026-07-01-tui-iocraft-to-ratatui-migration.md`.
#![forbid(unsafe_code)]

pub mod agents_screen;
pub mod app;
pub mod composer;
pub mod files;
pub mod image_view;
pub mod message;
pub mod overlay;
pub mod palette;
pub mod picker;
pub mod render;
pub mod screens;
pub mod session;
pub mod style_adapter;
pub mod term_image;
pub mod vim;

use std::io::{self, Stdout};

use crossterm::event::{self, Event, KeyCode};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Terminal;
pub use tui_core::message::RenderedMessage;
pub use tui_core::orchestrator_bridge::TurnEvent;
use tui_core::render::StyledLine;
use tui_core::theme::Theme;

/// The concrete ratatui terminal type used by the runtime.
pub type RataTerminal = Terminal<CrosstermBackend<Stdout>>;

/// Enter raw mode + the alternate screen and construct a ratatui terminal.
///
/// # Errors
/// Returns any terminal IO error from enabling raw mode, switching screens, or
/// constructing the backend.
pub fn setup_terminal() -> io::Result<RataTerminal> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    Terminal::new(CrosstermBackend::new(stdout))
}

/// Restore the terminal to its pre-session state (leave alt screen, disable raw
/// mode, show the cursor). Safe to call during unwind/exit.
///
/// # Errors
/// Returns any terminal IO error from restoring screen/cursor state.
pub fn restore_terminal(terminal: &mut RataTerminal) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()
}

/// Phase-1 runtime shell: set up the terminal, run the draw/event loop until
/// the user presses `q`/`Esc`, then restore the terminal. Restoration runs even
/// if the loop errors.
///
/// # Errors
/// Propagates the draw/event loop's first IO error (after restoring the
/// terminal).
pub fn run_shell() -> io::Result<()> {
    let mut terminal = setup_terminal()?;
    let result = draw_loop(&mut terminal);
    restore_terminal(&mut terminal)?;
    result
}

fn draw_loop(terminal: &mut RataTerminal) -> io::Result<()> {
    loop {
        terminal.draw(render_frame)?;
        if let Event::Key(key) = event::read()? {
            if matches!(key.code, KeyCode::Char('q') | KeyCode::Esc) {
                return Ok(());
            }
        }
    }
}

/// Draw the fixed 3-zone frame: a growable scrollback area on top, a one-row
/// status line, and a bottom-pinned composer box. The scrollback renders
/// `tui_core` styled lines through [`render`]; the other zones are placeholders
/// until later phases render more `tui_core` content into them.
fn render_frame(frame: &mut ratatui::Frame) {
    let zones = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(3),
        ])
        .split(frame.area());

    let scrollback: Vec<ratatui::text::Line> = demo_scrollback()
        .iter()
        .map(render::styled_line_to_ratatui)
        .collect();
    frame.render_widget(Paragraph::new(scrollback), zones[0]);
    frame.render_widget(Paragraph::new("status — press q to exit"), zones[1]);
    frame.render_widget(Block::new().borders(Borders::ALL), zones[2]);
}

/// A sample conversation rendered through `tui-rata`'s real message renderer
/// ([`message::render_message`]) — proving `tui-rata` displays actual
/// `RenderedMessage`s (user prompt, markdown assistant reply, system line) in
/// ratatui. Replaced by live session state once the event loop is wired.
fn demo_scrollback() -> Vec<StyledLine> {
    let theme = Theme::dark();
    let conversation = [
        RenderedMessage::UserText {
            body: "How do I center a div?".to_string(),
            timestamp: 0,
        },
        RenderedMessage::AssistantText {
            body: "Use **flexbox** on the parent:\n\n```css\n.parent { display: flex; }\n```"
                .to_string(),
            timestamp: 0,
        },
        RenderedMessage::SystemText {
            body: "tui-rata is rendering real RenderedMessages via tui-core.".to_string(),
            timestamp: 0,
            is_error: false,
        },
    ];
    conversation
        .iter()
        .flat_map(|m| message::render_message(m, 80, &theme, false))
        .collect()
}

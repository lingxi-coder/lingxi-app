//! `tui-rata` — Ratatui-based terminal UI runtime for LingXi.
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

pub mod app;
pub mod bottom_pane;
pub mod chat_widget;
pub mod color;
pub mod command;
pub mod composer;
pub mod copy;
pub mod export;
pub mod files;
pub mod history_cell;
pub mod image_view;
pub mod message;
pub mod render;
pub mod renderable;
pub mod session;
pub mod style_adapter;
pub mod term_image;
pub mod terminal;
pub mod transcript;
pub mod vim;

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

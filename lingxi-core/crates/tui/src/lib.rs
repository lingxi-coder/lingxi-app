//! `lingxi-tui` — iocraft-based fullscreen TUI for the `LingXi` CLI.
//!
//! M6-01 ships the crate skeleton: terminal guard, event loop merge,
//! placeholder root component, and the `run_tui_session` entry point.
//! Components (`StatusLine`, `PromptInput`, `Scrollback`, message
//! renderers, permission dialogs) land in M6-02..M6-05.
//!
//! See plan `docs/superpowers/plans/2026-05-28-m6-01-foundation.md`.

#![forbid(unsafe_code)]

pub mod app;
pub mod components;
pub mod error;
pub mod events;
pub mod screens;
pub mod session;
pub mod state;
pub mod telemetry;
pub(crate) mod terminal;
pub mod theme;

pub use app::TuiApp;
pub use error::TuiError;
pub use events::orchestrator_bridge::{BridgeOutputStream, TurnEvent};
pub use events::{OrchestratorOutputEvent, TuiEvent};
pub use session::{run_tui_session, Runtime};

// Re-export points are filled in by later tasks; the stubs above keep the
// crate compiling task-by-task.

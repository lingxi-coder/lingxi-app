//! `tui-core` — backend-neutral core for the LingXi terminal UI.
//!
//! Holds the render model, reducers, and state that are independent of any
//! specific TUI backend (iocraft or ratatui). Modules are extracted from the
//! `tui` crate during the iocraft → ratatui migration; `tui` re-exports each
//! moved module so downstream code keeps its existing paths until cutover.
//!
//! See `.omo/plans/2026-07-01-tui-iocraft-to-ratatui-migration.md`.
#![forbid(unsafe_code)]

pub mod active_turn;
pub mod ask_user_question_bridge;
pub mod bash_runner;
pub mod error;
pub mod key_hint;
pub mod collapse;
pub mod message;
pub mod message_render;
pub mod multiagent;
pub mod orchestrator_bridge;
pub mod permission_bridge;
pub mod recent_models;
pub mod status_line_command;
pub mod render;
pub mod retry_ux;
pub mod telemetry;
pub mod theme;
pub mod theme_detect;
pub mod terminal_setup;
pub mod theme_persist;

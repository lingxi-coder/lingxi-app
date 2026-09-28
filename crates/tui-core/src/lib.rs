//! `tui-core` — backend-neutral core for the LingXi terminal UI.
//!
//! Holds the render model, reducers, and state that are independent of any
//! specific TUI backend (iocraft or ratatui). Modules are extracted from the
//! `tui` crate during the iocraft → ratatui migration; `tui` re-exports each
//! moved module so downstream code keeps its existing paths until cutover.
//!
//! See `.omo/plans/2026-07-01-tui-iocraft-to-ratatui-migration.md`.
#![forbid(unsafe_code)]
// Documentation debt, not a decision that docs do not matter: this crate had
// 16 undocumented public item(s) when `missing_docs` was measured across the
// workspace (2026-09-16). The lint stays `warn` at the workspace level so a NEW
// crate still inherits the requirement; this allow is scoped here so the debt
// is visible per crate and can be repaid one crate at a time by deleting this
// line.
#![allow(missing_docs)]

pub mod active_turn;
pub mod background_detach;
pub mod collapse;
pub mod error;
pub mod key_hint;
pub mod left_arrow_gesture;
pub mod message;
pub mod message_render;
pub mod multiagent;
pub mod orchestrator_bridge;
pub mod permission_bridge;
pub mod recent_models;
pub use client_presentation::render;
pub mod retry_ux;
pub mod status_line_command;
pub mod telemetry;
pub mod terminal_setup;
pub mod theme;
pub mod theme_detect;
pub mod theme_persist;
pub use client_presentation::tool_display;

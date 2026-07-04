//! `/web` WebSearch-config pure reducer screens.
//!
//! Ported verbatim from the deleted iocraft `tui` crate's
//! `screens/web_picker.rs` and `screens/web_config.rs` (git ref
//! `f4ddad16f`). These are backend-neutral `KeyCode -> Outcome` reducers
//! with no ratatui rendering yet; the ratatui views land in a later task.
pub mod config;
pub mod picker;

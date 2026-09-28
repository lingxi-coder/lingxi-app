//! `/connect` provider OAuth/API-key pure reducer screens.
//!
//! Ported verbatim from the deleted iocraft `tui` crate's
//! `screens/connect.rs`, `screens/connect_picker.rs`, and
//! `screens/connect_method.rs` (git ref `f4ddad16f`). These are
//! backend-neutral `KeyCode -> Outcome` reducers with no ratatui rendering
//! yet; the ratatui views land in a later task.
pub mod method;
pub mod picker;
pub mod screen;

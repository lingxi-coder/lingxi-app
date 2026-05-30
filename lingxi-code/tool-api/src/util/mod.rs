//! Cross-tool pure helpers shared by multiple tool crates.
//!
//! Moved here from `tools/src/shared/` in M8-P5 so per-category tool crates
//! (`tool-file`, `tool-shell`, …) and engine code can use them without
//! depending on the `tools` monolith.

pub mod output_truncation;
pub mod path_validation;

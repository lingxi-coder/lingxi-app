//! Output-style subsystem: registry, model, and built-in markdown default.
//!
//! See spec §21.

#![forbid(unsafe_code)]

pub mod builtin;
pub mod model;
pub mod registry;

pub use model::*;
pub use registry::{
    resolve_builtin_output_style, BuiltinOutputStyle, OutputStyleError, OutputStyleRegistry,
    DEFAULT_OUTPUT_STYLE_NAME,
};

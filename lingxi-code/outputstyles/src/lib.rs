//! Output-style subsystem: registry, model, and built-in markdown default.
//!
//! See spec §21.

#![forbid(unsafe_code)]

pub mod model;
pub mod registry;

pub use model::*;
pub use registry::{OutputStyleError, OutputStyleRegistry};

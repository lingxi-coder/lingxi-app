//! Cross-tool shared helpers — used by every M4 sub-plan.
//!
//! Each helper is intentionally pure-functional (or thin async over the bus)
//! so concurrent callers don't share state.

pub mod file_kit;
pub mod output_truncation;
pub mod path_validation;

pub use output_truncation::{
    truncate, truncate_default, MAX_TOOL_OUTPUT_LENGTH, TRUNCATION_SUFFIX,
};
pub use path_validation::{
    canonicalize_and_validate, emit_blocked_event, PathValidationError, PATH_BLOCKED_EVENT,
};

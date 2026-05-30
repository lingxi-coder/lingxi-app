//! Cross-tool shared helpers.
//!
//! M8-P5: `output_truncation` and `path_validation` moved to `tool-api`'s
//! `util` module (shared across tool crates + engine); re-exported here so
//! the monolith's remaining tools keep using `crate::shared::…` until P7.
//! `ansi_strip` (shell-only) and `file_kit` (file-only) stay until their
//! tool crates take them.

pub mod ansi_strip;
pub mod file_kit;

pub use tool_api::util::output_truncation::{
    self, truncate, truncate_default, MAX_TOOL_OUTPUT_LENGTH, TRUNCATION_SUFFIX,
};
pub use tool_api::util::path_validation::{
    self, canonicalize_and_validate, emit_blocked_event, PathValidationError, PATH_BLOCKED_EVENT,
};

pub use ansi_strip::{strip_ansi, strip_ansi_count, ANSI_ESCAPE_REGEX_LITERAL};

//! Cross-tool shared helpers.
//!
//! M8-P5: the helpers all moved out of the monolith. `output_truncation` and
//! `path_validation` went to `tool-api`'s `util` module (shared across tool
//! crates + engine); `file_kit` went to the `tool-file` crate; `ansi_strip`
//! went to the `tool-shell` crate. They are re-exported here so the monolith's
//! remaining tools and the parity tests keep using `crate::shared::…` /
//! `tools::shared::…` until P7 finishes the split.

// file-only and shell-only helpers, re-exported from their tool crates.
pub use tool_file::shared as file_kit;
pub use tool_shell::shared as ansi_strip;

pub use tool_api::util::output_truncation::{
    self, truncate, truncate_default, MAX_TOOL_OUTPUT_LENGTH, TRUNCATION_SUFFIX,
};
pub use tool_api::util::path_validation::{
    self, canonicalize_and_validate, emit_blocked_event, PathValidationError, PATH_BLOCKED_EVENT,
};

pub use ansi_strip::{strip_ansi, strip_ansi_count, ANSI_ESCAPE_REGEX_LITERAL};

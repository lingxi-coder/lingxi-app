//! Tool API — the abstract surface every tool implementation and every
//! engine-side consumer depends on: the [`Tool`] trait, [`ToolUseContext`],
//! [`ToolRegistry`], progress channel, and content-replacement state.
//!
//! Extracted from the `tools` monolith in M8-P3 so engine crates can depend
//! on the trait surface without pulling in the 41 builtin implementations.
//! Trait semantics, error variants, and field shapes are byte-equivalent to
//! the pre-split `tools` crate. The §5 `ToolCtx` reshape (full Platform
//! handles, split `error`/`schema` modules) is deferred to P4.

#![forbid(unsafe_code)]
#![allow(
    clippy::module_name_repetitions,
    clippy::needless_pass_by_value,
    clippy::too_many_lines,
    clippy::doc_markdown
)]

pub mod content_replacement;
pub mod context;
pub mod progress;
pub mod registry;
pub mod tool_trait;

pub use content_replacement::ContentReplacementState;
pub use context::{ToolUseContext, ToolUseOptions};
pub use progress::{progress_channel, ToolProgress, ToolProgressReceiver, ToolProgressSender};
pub use registry::ToolRegistry;
pub use tool_trait::*;

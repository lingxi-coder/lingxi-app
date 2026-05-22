//! 4-tier memory subsystem for `LingXi` Core.
//!
//! Provides the tier model, frontmatter parser, LLM-driven selector,
//! prefetch channel, and per-agent snapshots described in spec §6.
//! The selector and team-memory watcher are scaffolded here; the
//! production implementations land in Plans 08 and 10.

#![forbid(unsafe_code)]

pub mod file;
pub mod prefetch;
pub mod selector;
pub mod session_memory;
pub mod snapshot;
pub mod team_memory;
pub mod tier;

pub use file::{
    parse_markdown_with_frontmatter, MemoryError, MemoryFile, MemoryFrontmatter,
    MAX_ENTRYPOINT_BYTES, MAX_ENTRYPOINT_LINES,
};
pub use tier::MemoryTier;

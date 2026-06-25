//! Skill subsystem abstraction (M8-P8): the [`Skill`] data model, the
//! [`SkillRegistry`] with trigger-based discovery, the markdown frontmatter
//! loader, MCP-derived skill builders, and a discovery-prefetch placeholder.
//!
//! Extracted from the former monolithic `skills` crate. The model-visible
//! `Skill` *tool* lives in `tool-skill` (P7); compiled-in builtin skill
//! templates + their registration entry points live in the [`builtin`] module
//! (folded in from the former `skill-builtin` crate). This crate is the shared
//! skill abstraction — the skill analogue of `tool-api`.
//!
//! See spec §18 for the Skill subsystem overview.

#![forbid(unsafe_code)]

pub mod builtin;
pub mod frontmatter;
pub mod listing;
pub mod mcp_builders;
pub mod model;
pub mod prefetch;
pub mod registry;

pub use builtin::{register_desktop, register_mobile};
pub use frontmatter::{parse_skill_markdown, SkillLoadError};
pub use listing::{
    load_file_skill_sections, load_file_skill_sections_with_roots, FileSkillRow, FileSkillSection,
};
pub use mcp_builders::skill_from_mcp_tool;
pub use model::{LoadedFrom, Skill, SkillFrontmatter, SkillSource};
pub use prefetch::SkillDiscoveryPrefetch;
pub use registry::SkillRegistry;

//! Skill subsystem abstraction (M8-P8): the [`Skill`] data model, the
//! [`SkillRegistry`] with trigger-based discovery, the markdown frontmatter
//! loader, MCP-derived skill builders, and a discovery-prefetch placeholder.
//!
//! Extracted from the former monolithic `skills` crate. The model-visible
//! `Skill` *tool* lives in `tool-skill` (P7); compiled-in builtin skill
//! templates live in `skill-builtin` (P8). This crate is the shared
//! abstraction both depend on — the skill analogue of `tool-api`.
//!
//! See spec §18 for the Skill subsystem overview.

#![forbid(unsafe_code)]

pub mod frontmatter;
pub mod mcp_builders;
pub mod model;
pub mod prefetch;
pub mod registry;

pub use frontmatter::{parse_skill_markdown, SkillLoadError};
pub use mcp_builders::skill_from_mcp_tool;
pub use model::{LoadedFrom, Skill, SkillFrontmatter, SkillSource};
pub use prefetch::SkillDiscoveryPrefetch;
pub use registry::SkillRegistry;

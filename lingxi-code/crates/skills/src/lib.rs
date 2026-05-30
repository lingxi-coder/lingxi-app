//! Skill subsystem: model, registry, frontmatter loader, MCP-derived skills,
//! discovery prefetch, and the model-visible `SkillTool`.
//!
//! See spec §18 for the design overview.

#![forbid(unsafe_code)]

pub mod frontmatter;
pub mod mcp_builders;
pub mod model;
pub mod prefetch;
pub mod registry;
pub mod skill_tool;

pub use model::*;
pub use registry::SkillRegistry;
pub use skill_tool::SkillTool;

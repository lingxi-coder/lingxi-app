//! Helpers for synthesizing [`Skill`]s from MCP server tool descriptions.

use crate::model::{LoadedFrom, Skill, SkillFrontmatter, SkillSource};
use traits::McpToolDto;

/// Build a [`Skill`] whose triggers are derived from an MCP tool's name and
/// whose content mirrors the tool's description.
#[must_use]
pub fn skill_from_mcp_tool(tool: &McpToolDto) -> Skill {
    let description = tool.description().to_string();
    Skill {
        name: tool.full_name.clone(),
        description: description.clone(),
        frontmatter: SkillFrontmatter {
            name: tool.full_name.clone(),
            description: description.clone(),
            triggers: vec![tool.tool_name().to_lowercase()],
            ..Default::default()
        },
        content: description,
        source: SkillSource::Mcp {},
        loaded_from: LoadedFrom::Mcp,
        plugin_id: None,
        file_path: "<mcp>".into(),
    }
}

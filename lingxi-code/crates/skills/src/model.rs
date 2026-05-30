//! Skill data model: in-memory representation of a discoverable, model-visible
//! "skill" loaded from markdown frontmatter or derived from an MCP tool.
//!
//! See spec §18 for the Skill subsystem overview.

use lingxi_protocol::PluginId;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A registered skill — a named, discoverable behaviour the model can invoke
/// via the `Skill` tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    /// Canonical skill name (matches the tool input "name" field).
    pub name: String,
    /// Short human-readable description shown to the model.
    pub description: String,
    /// Parsed YAML frontmatter from the originating markdown file.
    pub frontmatter: SkillFrontmatter,
    /// Body of the skill markdown (after the frontmatter block, trimmed).
    pub content: String,
    /// Origin classification (bundled / user / project / plugin / managed / mcp).
    pub source: SkillSource,
    /// Mechanism by which the registry discovered this skill.
    pub loaded_from: LoadedFrom,
    /// Owning plugin, if loaded via the plugin manager.
    pub plugin_id: Option<PluginId>,
    /// Filesystem path of the originating markdown (or `<mcp>` for MCP-derived).
    pub file_path: PathBuf,
}

/// YAML frontmatter shape parsed from skill markdown files.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SkillFrontmatter {
    /// Skill name (must match the filename slug in practice).
    pub name: String,
    /// Short description shown to the model.
    pub description: String,
    /// Long-form guidance about when the model should invoke this skill.
    pub when_to_use: Option<String>,
    /// Optional allow-list of tool names this skill may use.
    ///
    /// Field name aligned with `AgentDefinition::allowed_tools` (§10.2) and
    /// `CommandFrontmatter::allowed_tools` (§19.1). Accepts legacy
    /// `tools_allowed` alias.
    #[serde(alias = "tools_allowed")]
    pub allowed_tools: Option<Vec<String>>,
    /// When true the registry may surface this skill via auto-search.
    #[serde(default = "default_true")]
    pub auto_search: bool,
    /// Phrases that should trigger discovery of this skill.
    pub triggers: Vec<String>,
}

fn default_true() -> bool {
    true
}

/// Where the skill originally came from (provenance classification).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SkillSource {
    /// Compiled into the binary.
    Bundled,
    /// User-level configuration directory.
    User,
    /// Project-local configuration directory.
    Project,
    /// Loaded by an installed plugin.
    Plugin,
    /// Loaded from a managed (admin-controlled) source.
    Managed,
    /// Derived from an MCP server tool definition.
    Mcp {},
}

/// How the registry loaded this skill (discovery mechanism).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LoadedFrom {
    /// Bundled with the binary.
    Bundled,
    /// User or project `skills/` directory.
    Skills,
    /// Plugin manager.
    Plugin,
    /// Managed (admin-controlled) source.
    Managed,
    /// Derived from an MCP server tool.
    Mcp,
    /// Legacy: loaded from the older `commands/` directory layout.
    CommandsDeprecated,
}

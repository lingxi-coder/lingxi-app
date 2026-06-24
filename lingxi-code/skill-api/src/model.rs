//! Skill data model: in-memory representation of a discoverable, model-visible
//! "skill" loaded from markdown frontmatter or derived from an MCP tool.
//!
//! See spec §18 for the Skill subsystem overview.

use protocol::PluginId;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
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
///
/// Key names match the binary's full frontmatter key list
/// (bytes 196457593): `name`, `description`, `model`, `allowed-tools`,
/// `argument-hint`, `arguments`, `disable-model-invocation`, `user-invocable`,
/// `effort`, `shell`, `version`, `when_to_use`, `paths`, `hooks`, `context`,
/// `agent`, `created_by`, `improved_by`, `hide-from-slash-command-tool`.
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
    /// Tools removed from the model while this skill is active. Comma-separated
    /// string or YAML list. Cleared when the user sends the next message.
    /// Binary bytes 94993840, 155728865. Accepts canonical alias `disallowedTools`.
    #[serde(rename = "disallowed-tools", alias = "disallowedTools")]
    pub disallowed_tools: Option<Vec<String>>,
    /// When `false` the model cannot invoke this via the Skill tool; only users
    /// can type the slash command. When unset (`None`) the default behaviour
    /// applies (both user and model may invoke). Binary bytes 71015792, 94993776.
    #[serde(rename = "user-invocable")]
    pub user_invocable: Option<bool>,
    /// When `true` the `Skill` tool is not permitted to invoke this skill — only
    /// a user typing the slash command may invoke it.
    /// Binary bytes 94993776 + SkillTool error string `disable-model-invocation`.
    #[serde(rename = "disable-model-invocation")]
    pub disable_model_invocation: bool,
    /// Placeholder text shown after the slash command name in the UI.
    /// Binary bytes 94993872. Accepts the `arguments` alias used in some docs.
    #[serde(rename = "argument-hint", alias = "arguments")]
    pub argument_hint: Option<String>,
    /// Named argument list for positional expansion (`$name` in the body).
    /// Populated into `SkillDescriptor::argument_names` at load time.
    /// Binary bytes 196457593 (full key list).
    #[serde(rename = "named-arguments")]
    pub named_arguments: Option<Vec<String>>,
    /// Effort level hint (forwarded to agent execution context).
    /// Binary bytes 196457593.
    pub effort: Option<String>,
    /// Shell selector for embedded `!command` expansion.
    /// Binary bytes 196457593.
    pub shell: Option<String>,
    /// Semantic version of this skill. Binary bytes 196457593.
    pub version: Option<String>,
    /// Gitignore-style file-path patterns; skill activates only when the model
    /// touches a matching path. Binary bytes 196457593.
    pub paths: Option<Vec<String>>,
    /// Per-skill hook configuration (same shape as global hooks).
    /// Binary bytes 196457593.
    pub hooks: Option<JsonValue>,
    /// Execution context: `"inline"` (default) or `"fork"` (spawns a
    /// subagent). Binary bytes 155734026, 196453457.
    pub context: Option<String>,
    /// Agent type to spawn when `context: fork`. Binary bytes 155734026.
    pub agent: Option<String>,
    /// Model override for this skill invocation. Binary bytes 196457593.
    pub model: Option<String>,
    /// Hide this skill from the slash-command listing shown to the model.
    /// Binary bytes 155749616.
    #[serde(rename = "hide-from-slash-command-tool")]
    pub hide_from_slash_command_tool: bool,
    /// Author metadata (survey tracking). Binary bytes 94994016.
    pub created_by: Option<String>,
    /// Improver metadata (survey tracking). Binary bytes 94994048.
    pub improved_by: Option<String>,
    /// When true the registry may surface this skill via auto-search.
    /// LingXi-only extension (not in binary's frontmatter key list).
    #[serde(default = "default_true")]
    pub auto_search: bool,
    /// Phrases that should trigger discovery of this skill.
    /// LingXi-only extension (not in binary's frontmatter key list).
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

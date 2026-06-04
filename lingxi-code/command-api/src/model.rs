//! Data model for slash commands: registry entries, frontmatter, results, and
//! the trait every built-in handler implements.

use crate::parser::ParsedSlashCommand;
use protocol::{Effect, McpConnectionId, PluginId};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A registered slash command (built-in, markdown-defined, plugin-supplied,
/// or MCP-derived).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlashCommand {
    /// Command name (without the leading `/`).
    pub name: String,
    /// Short user-facing description.
    pub description: String,
    /// Origin classification.
    pub source: CommandSource,
    /// Concrete dispatch shape — built-in handler, markdown template, etc.
    pub kind: SlashCommandKind,
}

/// What a `SlashCommand` actually dispatches to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SlashCommandKind {
    /// Dispatched to a Rust handler keyed by id.
    Builtin {
        /// Registry key used by [`crate::registry::CommandRegistry::get_handler`].
        handler_id: String,
    },
    /// Markdown file with frontmatter — body is injected after argument substitution.
    Markdown {
        /// Source file on disk.
        file_path: PathBuf,
        /// Parsed YAML frontmatter.
        frontmatter: CommandFrontmatter,
        /// Markdown body (template with `$ARGUMENTS` / `$N` placeholders).
        prompt_template: String,
    },
    /// Markdown command shipped by a plugin.
    Plugin {
        /// Plugin that owns this command.
        plugin_id: PluginId,
        /// Source file on disk.
        file_path: PathBuf,
        /// Parsed YAML frontmatter.
        frontmatter: CommandFrontmatter,
        /// Markdown body (template).
        prompt_template: String,
    },
    /// Bridged to an MCP server's named prompt.
    Mcp {
        /// MCP connection that owns the prompt.
        connection_id: McpConnectionId,
        /// Prompt name to fetch via `prompts/get`.
        prompt_name: String,
    },
}

/// YAML frontmatter shape for markdown-defined slash commands.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CommandFrontmatter {
    /// Short user-facing description.
    pub description: String,
    /// Optional allow-list of tool names.
    #[serde(alias = "tools_allowed")]
    pub allowed_tools: Option<Vec<String>>,
    /// Optional pinned model name.
    pub model: Option<String>,
    /// Free-form hints describing positional argument shape.
    pub argument_hints: Vec<String>,
    /// Declared positional argument names (frontmatter `arguments`). Used by the
    /// loader's named-argument substitution. Mirrors the TS frontmatter
    /// `arguments: string | string[]` union that feeds `parseArgumentNames`.
    pub argument_names: Vec<String>,
    /// Optional thinking-token budget.
    pub thinking: Option<u32>,
    /// Shell to route embedded shell-expansion blocks through. When absent the
    /// runtime defaults to bash (mirrors the TS `frontmatterParser`'s
    /// `shell?: FrontmatterShell`).
    pub shell: Option<FrontmatterShell>,
}

/// Shell selected by a markdown command's frontmatter for embedded shell
/// expansion. Mirrors the TS `FrontmatterShell` union (`bash` | `powershell`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FrontmatterShell {
    /// Route embedded shell commands through bash (the default).
    Bash,
    /// Route embedded shell commands through `PowerShell`.
    PowerShell,
}

/// Where the command came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandSource {
    /// Compiled-in handler.
    Builtin,
    /// User-level configuration directory.
    User,
    /// Project-local configuration directory.
    Project,
    /// `.local`-style override (per-user, per-project).
    Local,
    /// Loaded by an installed plugin.
    Plugin,
    /// Loaded from a managed (admin-controlled) source.
    Managed,
    /// Derived from an MCP server prompt.
    Mcp,
}

/// Possible outcomes from invoking a slash command.
#[derive(Debug)]
pub enum CommandResult {
    /// Synchronous completion with an optional inline display string.
    Done {
        /// Optional human-readable text to show.
        display: Option<String>,
    },
    /// Inject `content` into the conversation as the next user message.
    InjectMessage {
        /// Content to inject as a user message.
        content: String,
    },
    /// Emit effects to be processed by the run loop.
    EmitEffects {
        /// Effects to route through the engine.
        effects: Vec<Effect>,
        /// Optional human-readable text to show before effects apply.
        display: Option<String>,
    },
    /// Ask the user to confirm before applying the supplied effects.
    RequestConfirmation {
        /// Prompt shown to the user.
        prompt: String,
        /// Effects applied iff the user confirms.
        on_confirm: Vec<Effect>,
    },
}

/// Trait implemented by every built-in slash-command handler.
#[async_trait::async_trait]
pub trait BuiltinCommandHandler: Send + Sync {
    /// Execute the command with parsed arguments and return a result.
    async fn handle(&self, args: &ParsedSlashCommand) -> CommandResult;
    /// Canonical command name (without the leading `/`).
    fn name(&self) -> &str;
    /// Short user-facing description.
    fn description(&self) -> &str;
}

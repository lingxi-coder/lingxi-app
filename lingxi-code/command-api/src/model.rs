//! Data model for slash commands: registry entries, frontmatter, results, and
//! the trait every built-in handler implements.

use crate::parser::ParsedSlashCommand;
use protocol::{Effect, McpConnectionId, PluginId};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

/// Port of the reference bundled-skill `getPromptForCommand(args)` seam
/// (`claude-code/src/skills/bundledSkills.ts` → `loop.ts:84`). Builds the
/// model-facing prompt dynamically from the raw (untrimmed) args, enabling the
/// empty→usage vs non-empty→buildPrompt two-branch behavior that static
/// `$ARGUMENTS` templating cannot express (the two texts share no template).
///
/// Carried on [`SlashCommandKind::Bundled`] and projected onto the Skill tool's
/// descriptor; [`SkillTool::call`] invokes `build(args)` INSTEAD of the static
/// argument substitution when it is present.
pub trait BundledPromptFn: Send + Sync {
    /// Produce the model-facing prompt for the given raw args string. The
    /// implementation does its own trimming/branching (mirrors the reference's
    /// `args.trim()` inside `getPromptForCommand`, `loop.ts:85`).
    fn build(&self, args: &str) -> String;
}

impl std::fmt::Debug for dyn BundledPromptFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BundledPromptFn")
    }
}

/// A registered slash command (built-in, markdown-defined, plugin-supplied,
/// or MCP-derived).
///
/// The trailing metadata fields mirror the TS `CommandBase` shape
/// (`claude-code/src/types/command.ts:175`): `disableModelInvocation`,
/// `hasUserSpecifiedDescription`, `loadedFrom`, `whenToUse`, `aliases`, and
/// `argumentHint`. They drive model-invocable filtering, alias resolution, and
/// the source-annotated description (see [`crate::describe`]). Each carries
/// `#[serde(default, skip_serializing_if = …)]` so that when absent the wire
/// JSON is byte-identical to the pre-existing `{name, description, source,
/// kind}` shape.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SlashCommand {
    /// Command name (without the leading `/`).
    pub name: String,
    /// Short user-facing description.
    pub description: String,
    /// Origin classification.
    pub source: CommandSource,
    /// Concrete dispatch shape — built-in handler, markdown template, etc.
    pub kind: SlashCommandKind,
    /// Whether the model is forbidden from invoking this command (TS
    /// `disableModelInvocation`). Filtered out by
    /// [`crate::registry::CommandRegistry::model_invocable_commands`].
    #[serde(default, skip_serializing_if = "is_false")]
    pub disable_model_invocation: bool,
    /// Whether the description came from an explicit user/frontmatter
    /// `description` (TS `hasUserSpecifiedDescription`) rather than being
    /// auto-derived from the markdown body.
    #[serde(default, skip_serializing_if = "is_false")]
    pub has_user_specified_description: bool,
    /// Where the command was loaded from (TS `loadedFrom` string union:
    /// `commands_DEPRECATED` | `skills` | `plugin` | `managed` | `bundled` |
    /// `mcp`). Kept as a free-form string to mirror the TS union without a
    /// closed Rust enum.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loaded_from: Option<String>,
    /// Detailed "when to use" guidance for the model (TS `whenToUse`, from the
    /// Skill spec).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when_to_use: Option<String>,
    /// Alternate names this command also resolves by (TS `aliases`). Indexed by
    /// [`crate::registry::CommandRegistry::register_command`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    /// Hint text for the command's arguments, shown after the name (TS
    /// `argumentHint`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub argument_hint: Option<String>,
    /// Declared positional argument names (TS `argNames`), already filtered
    /// through `parseArgumentNames` (no empty/numeric-only names). Drives the
    /// in-TUI progressive argument-hint (TS `generateProgressiveArgumentHint`
    /// shown by `useTypeahead` → `BaseTextInput` in the `commandWithoutArgs`
    /// state). Empty for every built-in, so the `skip_serializing_if` keeps the
    /// wire JSON byte-identical to the pre-existing `{name, description, source,
    /// kind, …}` shape when no names are declared.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub argument_names: Vec<String>,
    /// Directory root for a directory-format skill command. Unset for ordinary
    /// `.lingxi/commands/*.md` markdown commands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_root: Option<PathBuf>,
    /// Whether a skill command is user-invocable. Kept optional so legacy command
    /// JSON stays unchanged when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_invocable: Option<bool>,
    /// Raw markdown byte length for file-backed skills.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_length: Option<usize>,
    /// Compact label shown in the `/` command menu (TS `menuDescription`). The
    /// reference's completion-popup row builder uses `menuDescription ??
    /// description` for the visible text; `None` falls back to
    /// [`Self::description`]. Set for bundled skills whose menu label is shorter
    /// than their full model-facing description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub menu_description: Option<String>,
}

/// Serde `skip_serializing_if` predicate for `bool` fields that default to
/// `false` — keeps absent flags out of the wire JSON.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(b: &bool) -> bool {
    !*b
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
    /// Programmatically-registered bundled skill (port of the reference
    /// `registerBundledSkill`, `bundledSkills.ts`). Unlike [`Self::Markdown`],
    /// its body is produced dynamically by `prompt_fn.build(args)` rather than
    /// templated, so it can branch on empty vs non-empty args (`loop.ts:84-90`).
    Bundled {
        /// Carries `model` / `allowed_tools` for descriptor parity with
        /// [`Self::Markdown`] (the reference bundled-skill spec also accepts
        /// these fields).
        frontmatter: CommandFrontmatter,
        /// Dynamic prompt builder. `#[serde(skip)]` keeps `SlashCommandKind`
        /// (de)serializable — bundled commands are reconstructed at boot, never
        /// loaded from disk, so a deserialized one is inert (`None`) by design.
        #[serde(skip)]
        prompt_fn: Option<Arc<dyn BundledPromptFn>>,
    },
}

impl Default for SlashCommandKind {
    /// An empty built-in dispatch shape. Exists so [`SlashCommand`] can derive
    /// [`Default`] (the derive macro cannot pick a default for an enum whose
    /// variants all carry fields).
    fn default() -> Self {
        Self::Builtin {
            handler_id: String::new(),
        }
    }
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
    /// SLASH.1: TS `disable-model-invocation` frontmatter
    /// (`parseBooleanFrontmatter`). When true the command is excluded from
    /// model-driven invocation (carried onto [`SlashCommand::disable_model_invocation`]).
    pub disable_model_invocation: bool,
    /// SLASH.4: TS `when_to_use` frontmatter — advisory text describing when the
    /// command applies (carried onto [`SlashCommand::when_to_use`]).
    pub when_to_use: Option<String>,
    /// Execution context: `Some("fork")` runs the command as a subagent under
    /// its own permission scoping. Anything else (including `None`) is inline.
    pub context: Option<String>,
    /// Whether a forking command runs in the BACKGROUND. `None` ⇒ background
    /// (claude's `background ?? true`).
    pub background: Option<bool>,
    /// Agent type a forking command spawns. `None` ⇒ `general-purpose`.
    pub agent: Option<String>,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CommandSource {
    /// Compiled-in handler.
    #[default]
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
    /// Programmatically-registered bundled skill (TS `loadedFrom: 'bundled'` /
    /// `registerBundledSkill`). Model-invocable; never truncated in the skill
    /// listing.
    Bundled,
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
    /// This command's declared allowed-tools (TS `command.allowedTools`), in the
    /// permission-rule syntax (e.g. `Bash(git add:*)`). Injected on top of the
    /// base policy for a fresh per-command effective policy before its embedded
    /// `!`cmd`` shell bodies are expanded (see
    /// [`crate::shell_expansion::ShellExpansionProvider`]).
    ///
    /// Defaults to empty: only the shell-embedding prompt builtins (`/commit`,
    /// `/commit-push-pr`, `/security-review`) override it, so every other
    /// handler is unaffected.
    fn allowed_tools(&self) -> &'static [&'static str] {
        &[]
    }
}

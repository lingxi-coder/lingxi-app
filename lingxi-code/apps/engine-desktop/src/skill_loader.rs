//! `CommandRegistrySkillLoader` — bridges the `Skill` tool's [`SkillLoader`]
//! seam to the desktop [`CommandRegistry`].
//!
//! SKILLEXEC.2: the `Skill` tool resolves a model-supplied skill name to a real
//! slash command. TS does this via `getAllCommands()` + `findCommand`
//! (`tools/SkillTool/SkillTool.ts:399-409`); the Rust port injects this adapter
//! so `SkillTool::with_loader` reaches the same `CommandRegistry` the slash
//! dispatcher uses. The registry handle is shared (`Arc<RwLock<…>>`) with the
//! dispatcher, so plugin lifecycle mutations are visible to both.
//!
//! Mapping `SlashCommand` → [`SkillDescriptor`]:
//! - markdown / plugin-markdown commands are the model-invocable prompt skills
//!   (`command_type = Prompt`), carrying the markdown body as `body`, plus the
//!   frontmatter `model` / `allowed_tools` / `argument_names`;
//! - builtin and MCP-prompt commands map to `command_type = Other`, so the tool
//!   rejects them with the locked "is not a prompt-based skill" string (MCP
//!   prompt fetch has no Rust substrate — see the `SkillTool` module doc).

use std::sync::Arc;

use command_api::{CommandRegistry, SlashCommand, SlashCommandKind};
use tokio::sync::RwLock;
use tool_api::tool_trait::ToolError;
use tool_skill::skill::{SkillCommandType, SkillDescriptor, SkillLoader};

/// Project a registered [`SlashCommand`] onto the [`SkillDescriptor`] subset the
/// `Skill` tool surfaces.
fn to_descriptor(cmd: &SlashCommand) -> SkillDescriptor {
    match &cmd.kind {
        // Markdown-defined (project/user/managed) or plugin-shipped markdown
        // commands are the model-invocable prompt skills.
        SlashCommandKind::Markdown {
            frontmatter,
            prompt_template,
            ..
        }
        | SlashCommandKind::Plugin {
            frontmatter,
            prompt_template,
            ..
        } => SkillDescriptor {
            name: cmd.name.clone(),
            description: cmd.description.clone(),
            body: prompt_template.clone(),
            disable_model_invocation: cmd.disable_model_invocation,
            command_type: SkillCommandType::Prompt,
            model: frontmatter.model.clone(),
            allowed_tools: frontmatter.allowed_tools.clone().unwrap_or_default(),
            argument_names: frontmatter.argument_names.clone(),
        },
        // Builtin handlers and MCP-prompt bridges are not prompt-based skills.
        SlashCommandKind::Builtin { .. } | SlashCommandKind::Mcp { .. } => SkillDescriptor {
            name: cmd.name.clone(),
            description: cmd.description.clone(),
            disable_model_invocation: cmd.disable_model_invocation,
            command_type: SkillCommandType::Other,
            ..SkillDescriptor::default()
        },
    }
}

/// [`SkillLoader`] backed by the shared desktop [`CommandRegistry`].
pub struct CommandRegistrySkillLoader {
    registry: Arc<RwLock<CommandRegistry>>,
}

impl CommandRegistrySkillLoader {
    /// Wrap the shared command registry handle. The same `Arc` is later filled
    /// with the fully-built registry and handed to the slash dispatcher, so the
    /// loader and dispatcher observe one command set.
    #[must_use]
    pub fn new(registry: Arc<RwLock<CommandRegistry>>) -> Self {
        Self { registry }
    }
}

#[async_trait::async_trait]
impl SkillLoader for CommandRegistrySkillLoader {
    async fn load(&self, name: &str) -> Result<Option<SkillDescriptor>, ToolError> {
        // `resolve` follows aliases (TS `findCommand` over name + aliases).
        let reg = self.registry.read().await;
        Ok(reg.resolve(name).map(to_descriptor))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use command_api::{CommandFrontmatter, CommandSource};
    use std::path::PathBuf;

    fn markdown_cmd(name: &str, body: &str) -> SlashCommand {
        SlashCommand {
            name: name.to_string(),
            description: format!("{name} description"),
            source: CommandSource::Project,
            kind: SlashCommandKind::Markdown {
                file_path: PathBuf::from(format!("/x/{name}.md")),
                frontmatter: CommandFrontmatter {
                    model: Some("opus".to_string()),
                    allowed_tools: Some(vec!["Bash".to_string()]),
                    argument_names: vec!["name".to_string()],
                    ..CommandFrontmatter::default()
                },
                prompt_template: body.to_string(),
            },
            ..SlashCommand::default()
        }
    }

    #[tokio::test]
    async fn resolves_registered_markdown_command_to_prompt_descriptor() {
        let mut reg = CommandRegistry::new();
        reg.register_command(markdown_cmd("review-pr", "Review $ARGUMENTS"));
        let loader = CommandRegistrySkillLoader::new(Arc::new(RwLock::new(reg)));

        let desc = loader
            .load("review-pr")
            .await
            .expect("load ok")
            .expect("command present");
        assert_eq!(desc.name, "review-pr");
        assert_eq!(desc.description, "review-pr description");
        assert_eq!(desc.body, "Review $ARGUMENTS");
        assert_eq!(desc.command_type, SkillCommandType::Prompt);
        assert_eq!(desc.model.as_deref(), Some("opus"));
        assert_eq!(desc.allowed_tools, vec!["Bash".to_string()]);
        assert_eq!(desc.argument_names, vec!["name".to_string()]);
        assert!(!desc.disable_model_invocation);
    }

    #[tokio::test]
    async fn unregistered_name_resolves_to_none() {
        let reg = CommandRegistry::new();
        let loader = CommandRegistrySkillLoader::new(Arc::new(RwLock::new(reg)));
        assert!(loader.load("nope").await.expect("load ok").is_none());
    }

    #[tokio::test]
    async fn builtin_command_maps_to_non_prompt_other() {
        let mut reg = CommandRegistry::new();
        reg.register_command(SlashCommand {
            name: "help".to_string(),
            description: "Show help".to_string(),
            source: CommandSource::Builtin,
            kind: SlashCommandKind::Builtin {
                handler_id: "help".to_string(),
            },
            ..SlashCommand::default()
        });
        let loader = CommandRegistrySkillLoader::new(Arc::new(RwLock::new(reg)));
        let desc = loader.load("help").await.expect("load ok").expect("present");
        // → rejected by the tool with the locked "is not a prompt-based skill".
        assert_eq!(desc.command_type, SkillCommandType::Other);
    }

    #[tokio::test]
    async fn disable_model_invocation_flag_is_carried() {
        let mut reg = CommandRegistry::new();
        let mut cmd = markdown_cmd("internal", "body");
        cmd.disable_model_invocation = true;
        reg.register_command(cmd);
        let loader = CommandRegistrySkillLoader::new(Arc::new(RwLock::new(reg)));
        let desc = loader
            .load("internal")
            .await
            .expect("load ok")
            .expect("present");
        assert!(desc.disable_model_invocation);
        assert_eq!(desc.command_type, SkillCommandType::Prompt);
    }
}

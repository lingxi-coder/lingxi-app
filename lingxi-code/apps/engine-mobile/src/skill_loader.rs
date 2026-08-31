//! Live-registry `Skill`-tool loader for the mobile engine.
//!
//! Mobile now shares ONE `Arc<RwLock<CommandRegistry>>` across the slash
//! dispatcher, skill listing provider, and the `Skill` tool. That keeps bundled
//! skills, programmatic bundled prompts such as `/loop`, and on-disk
//! `.lingxi/commands/**.md` / `.lingxi/skills/<name>/SKILL.md` in one live
//! command set, so boot-time loading and later `/reload-skills` refreshes are
//! visible everywhere instead of freezing the `Skill` tool at construction.
//!
//! The `to_descriptor` projection stays byte-faithful to the desktop
//! `CommandRegistrySkillLoader`, so a prompt command resolves identically on both
//! platforms.

use std::path::Path;
use std::sync::Arc;

use command_api::{CommandRegistry, CommandSource, SlashCommand, SlashCommandKind};
use protocol::ContentBlock;
use tokio::sync::RwLock;
use tool_api::tool_trait::ToolError;
use tool_skill::skill::{SkillCommandType, SkillDescriptor, SkillLoader as ToolSkillLoader};
use traits::skill_loader::{SkillLoad, SkillLoader as AgentSkillLoader};

/// Project a registered [`SlashCommand`] onto the [`SkillDescriptor`] subset the
/// `Skill` tool surfaces (verbatim with engine-desktop's `skill_loader::to_descriptor`).
/// `session_id` is stamped on every descriptor so the tool can substitute
/// `${LINGXI_SESSION_ID}` in the body.
fn to_descriptor(cmd: &SlashCommand, session_id: Option<&str>) -> SkillDescriptor {
    let session_id = session_id.map(str::to_owned);
    let mut descriptor = match &cmd.kind {
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
            disallowed_tools: frontmatter.disallowed_tools.clone().unwrap_or_default(),
            argument_names: frontmatter.argument_names.clone(),
            shell: frontmatter.shell,
            skip_shell_expansion: false,
            skill_root: cmd.skill_root.clone(),
            context: frontmatter.context.clone(),
            background: frontmatter.background,
            agent: frontmatter.agent.clone(),
            session_id,
            dynamic_body: None,
        },
        SlashCommandKind::Bundled {
            frontmatter,
            prompt_fn,
        } => SkillDescriptor {
            name: cmd.name.clone(),
            description: cmd.description.clone(),
            body: String::new(),
            disable_model_invocation: cmd.disable_model_invocation,
            command_type: SkillCommandType::Prompt,
            model: frontmatter.model.clone(),
            allowed_tools: frontmatter.allowed_tools.clone().unwrap_or_default(),
            disallowed_tools: frontmatter.disallowed_tools.clone().unwrap_or_default(),
            argument_names: Vec::new(),
            shell: frontmatter.shell,
            skip_shell_expansion: false,
            skill_root: None,
            context: frontmatter.context.clone(),
            background: frontmatter.background,
            agent: frontmatter.agent.clone(),
            session_id,
            dynamic_body: prompt_fn.clone(),
        },
        SlashCommandKind::Builtin { .. } => SkillDescriptor {
            name: cmd.name.clone(),
            description: cmd.description.clone(),
            disable_model_invocation: cmd.disable_model_invocation,
            command_type: SkillCommandType::Other,
            session_id,
            ..SkillDescriptor::default()
        },
        SlashCommandKind::Mcp { .. } => SkillDescriptor {
            name: cmd.name.clone(),
            description: cmd.description.clone(),
            disable_model_invocation: cmd.disable_model_invocation,
            command_type: SkillCommandType::Other,
            skip_shell_expansion: true,
            session_id,
            ..SkillDescriptor::default()
        },
    };
    if cmd.source == CommandSource::Plugin {
        if let Some(root) = descriptor.skill_root.as_deref() {
            descriptor.body = append_bundled_references(descriptor.body, root);
        }
    }
    descriptor
}

/// Mobile Plugin agents can intentionally omit filesystem tools, so a
/// file-backed skill that only says "read references/router.md" is not usable
/// when preloaded. Inline the verified Markdown reference closure into the
/// descriptor. Desktop remains file-backed; this is the mobile bundled form.
fn append_bundled_references(mut body: String, skill_root: &Path) -> String {
    let references = skill_root.join("references");
    let mut pending = vec![references.clone()];
    let mut files = Vec::new();
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if metadata.file_type().is_dir() {
                pending.push(path);
            } else if metadata.file_type().is_file()
                && path.extension().and_then(|ext| ext.to_str()) == Some("md")
            {
                files.push(path);
            }
        }
    }
    files.sort();
    for path in files {
        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue;
        };
        let relative = path.strip_prefix(skill_root).unwrap_or(&path).display();
        body.push_str(&format!(
            "\n\n## Bundled resource: {relative}\n\n{contents}"
        ));
    }
    body
}

fn product_prompt_token(suffix: &str) -> String {
    format!(
        "${{{}_{suffix}}}",
        branding::PRODUCT_NAME.to_ascii_uppercase()
    )
}

/// [`SkillLoader`] backed by the shared live mobile [`CommandRegistry`].
pub struct MobileDiskSkillLoader {
    registry: Arc<RwLock<CommandRegistry>>,
    session_id: Option<String>,
}

impl MobileDiskSkillLoader {
    #[must_use]
    pub fn new(registry: Arc<RwLock<CommandRegistry>>) -> Self {
        Self {
            registry,
            session_id: None,
        }
    }

    #[must_use]
    pub fn with_session_id(registry: Arc<RwLock<CommandRegistry>>, session_id: String) -> Self {
        Self {
            session_id: Some(session_id),
            ..Self::new(registry)
        }
    }

    /// Back-compat helper for tests that want a disk-populated live registry in
    /// one call.
    pub async fn load_from_disk(
        cwd: &Path,
        lingxi_home: &Path,
        home: &Path,
        session_id: Option<String>,
    ) -> Self {
        let registry = Arc::new(RwLock::new(CommandRegistry::new()));
        {
            let mut reg = registry.write().await;
            load_mobile_disk_commands_into_registry(&mut reg, cwd, lingxi_home, home).await;
        }
        Self {
            registry,
            session_id,
        }
    }
}

/// Load the mobile on-disk prompt commands into `registry`.
///
/// Verified Plugin skills are registered separately by `PluginManager` after
/// this base load and therefore remain the authoritative file-backed source.
pub async fn load_mobile_disk_commands_into_registry(
    registry: &mut CommandRegistry,
    cwd: &Path,
    lingxi_home: &Path,
    home: &Path,
) {
    let no_managed = lingxi_home.join("__lingxi_no_managed_settings__");
    command_core::load_and_register_custom_commands(registry, cwd, lingxi_home, &no_managed, home)
        .await;
    command_core::load_and_register_skill_commands_with_roots(
        registry,
        cwd,
        lingxi_home,
        None,
        home,
        &[],
    )
    .await;
}

#[async_trait::async_trait]
impl ToolSkillLoader for MobileDiskSkillLoader {
    async fn load(&self, name: &str) -> Result<Option<SkillDescriptor>, ToolError> {
        let reg = self.registry.read().await;
        Ok(reg
            .resolve(name)
            .map(|cmd| to_descriptor(cmd, self.session_id.as_deref())))
    }
}

impl MobileDiskSkillLoader {
    fn resolve_agent_skill_name(
        registry: &CommandRegistry,
        skill_name: &str,
        agent_type: &str,
    ) -> Option<String> {
        if let Some(command) = registry.resolve(skill_name) {
            return Some(command.name.clone());
        }
        if let Some(plugin_name) = agent_type.split(':').next().filter(|name| !name.is_empty()) {
            let qualified = format!("{plugin_name}:{skill_name}");
            if let Some(command) = registry.resolve(&qualified) {
                return Some(command.name.clone());
            }
        }
        let suffix = format!(":{skill_name}");
        registry
            .list_all()
            .into_iter()
            .find(|command| command.name.ends_with(&suffix))
            .map(|command| command.name.clone())
    }
}

#[async_trait::async_trait]
impl AgentSkillLoader for MobileDiskSkillLoader {
    async fn resolve_and_load(&self, skill_name: &str, agent_type: &str) -> Option<SkillLoad> {
        let registry = self.registry.read().await;
        let resolved = Self::resolve_agent_skill_name(&registry, skill_name, agent_type)?;
        let command = registry.resolve(&resolved)?.clone();
        drop(registry);

        let descriptor = to_descriptor(&command, self.session_id.as_deref());
        if descriptor.command_type != SkillCommandType::Prompt {
            return None;
        }
        let mut body = match descriptor.dynamic_body {
            Some(builder) => builder.build(""),
            None => command_api::substitute_arguments_faithful(
                &descriptor.body,
                Some(""),
                true,
                &descriptor.argument_names,
            )
            .ok()?,
        };
        if let Some(root) = descriptor.skill_root {
            let root = root.to_string_lossy();
            let root = if cfg!(windows) {
                root.replace('\\', "/")
            } else {
                root.into_owned()
            };
            body = body.replace(&product_prompt_token("SKILL_DIR"), &root);
        }
        if let Some(session_id) = descriptor.session_id {
            body = body.replace(&product_prompt_token("SESSION_ID"), &session_id);
        }
        Some(SkillLoad {
            display_name: skill_name.to_string(),
            progress_message: None,
            content: vec![ContentBlock::Text { text: body }],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The deferred #14 test: a disk-authored `.lingxi/commands/*.md` under the
    /// app-private root resolves through the loader as a prompt skill (proving the
    /// mobile Skill tool is no longer inert), and an unknown name resolves to None.
    #[tokio::test]
    async fn resolves_on_disk_command_as_prompt_skill() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let cmd_dir = root.join(".lingxi").join("commands");
        tokio::fs::create_dir_all(&cmd_dir).await.unwrap();
        tokio::fs::write(
            cmd_dir.join("review-pr.md"),
            "---\ndescription: Review a PR\n---\nReview $ARGUMENTS\n",
        )
        .await
        .unwrap();

        let registry = Arc::new(RwLock::new(CommandRegistry::new()));
        {
            let mut reg = registry.write().await;
            load_mobile_disk_commands_into_registry(&mut reg, root, &root.join(".lingxi"), root)
                .await;
        }
        let loader = MobileDiskSkillLoader::new(registry);

        let desc = loader
            .load("review-pr")
            .await
            .expect("load ok")
            .expect("disk command resolves as a skill");
        assert_eq!(desc.name, "review-pr");
        assert_eq!(desc.command_type, SkillCommandType::Prompt);
        assert!(
            desc.body.contains("Review $ARGUMENTS"),
            "body carries the markdown prompt template: {:?}",
            desc.body
        );
        assert!(loader
            .load("does-not-exist")
            .await
            .expect("load ok")
            .is_none());
    }

    #[tokio::test]
    async fn mobile_plugin_preload_inlines_the_verified_reference_closure() {
        let tmp = tempfile::tempdir().unwrap();
        let skill_root = tmp.path().join("frontend-design");
        std::fs::create_dir_all(skill_root.join("references/profiles")).unwrap();
        std::fs::write(
            skill_root.join("references/router.md"),
            "ROUTER-ONLY-MARKER",
        )
        .unwrap();
        std::fs::write(
            skill_root.join("references/profiles/ios.md"),
            "IOS-PROFILE-ONLY-MARKER",
        )
        .unwrap();
        let plugin_id = protocol::PluginId::new();
        let command = SlashCommand {
            name: "lingxi-local-app:frontend-design".into(),
            source: CommandSource::Plugin,
            skill_root: Some(skill_root.clone()),
            kind: SlashCommandKind::Plugin {
                plugin_id,
                file_path: skill_root.join("SKILL.md"),
                frontmatter: Default::default(),
                prompt_template: "Read references/router.md".into(),
            },
            ..SlashCommand::default()
        };
        let registry = Arc::new(RwLock::new(CommandRegistry::new()));
        registry
            .write()
            .await
            .register_plugin_commands(plugin_id, vec![command]);
        let loader = MobileDiskSkillLoader::new(registry);

        let preload = AgentSkillLoader::resolve_and_load(
            &loader,
            "frontend-design",
            "lingxi-local-app:designer",
        )
        .await
        .expect("namespaced Plugin skill must preload");
        assert!(matches!(
            preload.content.as_slice(),
            [ContentBlock::Text { text }]
                if text.contains("ROUTER-ONLY-MARKER")
                    && text.contains("IOS-PROFILE-ONLY-MARKER")
                    && text.contains("## Bundled resource: references/router.md")
        ));
    }
}

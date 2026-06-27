//! Disk-backed `Skill`-tool loader for the mobile engine (audit fix #14).
//!
//! Makes the mobile `Skill` tool functional. It discovers on-disk
//! `.lingxi/commands/**.md` + directory-format `.lingxi/skills/<name>/SKILL.md`
//! under the device's app-private root (`app_files_root` = Android `filesDir`,
//! with `lingxi_home = <app_files_root>/.claude`) and resolves a model-supplied
//! skill name to a real prompt [`SkillDescriptor`] — the mobile analog of
//! engine-desktop's `CommandRegistrySkillLoader` (which is apps-local and so
//! cannot be reused across the `app → app` dependency boundary).
//!
//! ## Why a self-contained registry
//!
//! Desktop shares ONE `Arc<RwLock<CommandRegistry>>` between the loader and the
//! slash dispatcher. This loader instead owns a registry built ONCE from disk at
//! construction, which confines the change to the Skill-tool seam — no reordering
//! of `build_mobile_inner`'s composition root. The cost is reading the
//! command/skill dirs once for the tool; mobile does not yet surface on-disk
//! slash commands through the dispatcher, so there is nothing to share with.
//!
//! The `to_descriptor` projection is byte-faithful to the desktop loader's, so a
//! disk-authored skill resolves identically on both platforms.

use std::path::Path;

use command_api::{CommandRegistry, SlashCommand, SlashCommandKind};
use tool_api::tool_trait::ToolError;
use tool_skill::skill::{SkillCommandType, SkillDescriptor, SkillLoader};

/// Project a registered [`SlashCommand`] onto the [`SkillDescriptor`] subset the
/// `Skill` tool surfaces (verbatim with engine-desktop's `skill_loader::to_descriptor`).
/// `session_id` is stamped on every descriptor so the tool can substitute
/// `${CLAUDE_SESSION_ID}` in the body.
fn to_descriptor(cmd: &SlashCommand, session_id: Option<&str>) -> SkillDescriptor {
    let session_id = session_id.map(str::to_owned);
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
            shell: frontmatter.shell,
            // On-disk / plugin markdown is NOT MCP-sourced, so shell expansion runs.
            skip_shell_expansion: false,
            skill_root: cmd.skill_root.clone(),
            session_id,
            dynamic_body: None,
        },
        // Bundled programmatic skills (the `/loop` family, port of
        // `registerBundledSkill`). Prompt-typed; body produced dynamically by
        // `prompt_fn` at call time (`getPromptForCommand`, loop.ts:84).
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
            argument_names: Vec::new(),
            shell: frontmatter.shell,
            skip_shell_expansion: false,
            skill_root: None,
            session_id,
            dynamic_body: prompt_fn.clone(),
        },
        // Builtin handlers are not prompt-based skills.
        SlashCommandKind::Builtin { .. } => SkillDescriptor {
            name: cmd.name.clone(),
            description: cmd.description.clone(),
            disable_model_invocation: cmd.disable_model_invocation,
            command_type: SkillCommandType::Other,
            session_id,
            ..SkillDescriptor::default()
        },
        // MCP-prompt bridges are not prompt-based skills AND are remote/untrusted.
        SlashCommandKind::Mcp { .. } => SkillDescriptor {
            name: cmd.name.clone(),
            description: cmd.description.clone(),
            disable_model_invocation: cmd.disable_model_invocation,
            command_type: SkillCommandType::Other,
            skip_shell_expansion: true,
            session_id,
            ..SkillDescriptor::default()
        },
    }
}

/// [`SkillLoader`] backed by a registry built once from the device's on-disk
/// `.lingxi/commands` + `.lingxi/skills` layers.
pub struct MobileDiskSkillLoader {
    registry: CommandRegistry,
    /// Per-session id stamped on resolved descriptors (`${CLAUDE_SESSION_ID}`),
    /// or `None` to leave the token un-substituted.
    session_id: Option<String>,
}

impl MobileDiskSkillLoader {
    /// Build the loader by discovering on-disk custom commands + directory-format
    /// skills under `cwd` (project) and `lingxi_home` (user). `home` is the
    /// home-walk root the loader uses for the user layer; on a device pass `cwd`
    /// so `home/.claude` resolves to the same app-private `.claude` as
    /// `lingxi_home` (the underlying loaders dedup by name across layers, so the
    /// collision is harmless). Mobile has no managed-settings layer, so a
    /// non-existent managed dir is passed (the loader skips missing dirs).
    pub async fn load_from_disk(
        cwd: &Path,
        lingxi_home: &Path,
        home: &Path,
        session_id: Option<String>,
    ) -> Self {
        let mut registry = CommandRegistry::new();
        // Mobile has no managed (enterprise policy) settings dir; a path that does
        // not exist makes the loader's managed layer a no-op.
        let no_managed = lingxi_home.join("__lingxi_no_managed_settings__");
        command_core::load_and_register_custom_commands(
            &mut registry,
            cwd,
            lingxi_home,
            &no_managed,
            home,
        )
        .await;
        command_core::load_and_register_skill_commands_with_roots(
            &mut registry,
            cwd,
            lingxi_home,
            None,
            home,
            &[],
        )
        .await;
        Self {
            registry,
            session_id,
        }
    }
}

#[async_trait::async_trait]
impl SkillLoader for MobileDiskSkillLoader {
    async fn load(&self, name: &str) -> Result<Option<SkillDescriptor>, ToolError> {
        // `resolve` follows aliases (findCommand over name + aliases).
        Ok(self
            .registry
            .resolve(name)
            .map(|cmd| to_descriptor(cmd, self.session_id.as_deref())))
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

        // Mirror the mobile host wiring: lingxi_home = <root>/.claude, home = root
        // (so home/.claude == lingxi_home; the loaders dedup by name across layers).
        let loader =
            MobileDiskSkillLoader::load_from_disk(root, &root.join(".lingxi"), root, None).await;

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

        // Unknown skill → None (would surface as "Unknown skill" at the tool).
        assert!(loader
            .load("does-not-exist")
            .await
            .expect("load ok")
            .is_none());
    }
}

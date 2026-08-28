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

use command_api::{CommandRegistry, SlashCommand, SlashCommandKind};
use tokio::sync::RwLock;
use tool_api::tool_trait::ToolError;
use tool_skill::skill::{SkillCommandType, SkillDescriptor, SkillLoader};

/// Project a registered [`SlashCommand`] onto the [`SkillDescriptor`] subset the
/// `Skill` tool surfaces (verbatim with engine-desktop's `skill_loader::to_descriptor`).
/// `session_id` is stamped on every descriptor so the tool can substitute
/// `${LINGXI_SESSION_ID}` in the body.
fn to_descriptor(cmd: &SlashCommand, session_id: Option<&str>) -> SkillDescriptor {
    let session_id = session_id.map(str::to_owned);
    match &cmd.kind {
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
    }
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
/// Callers that need bundled mobile skills to stay authoritative over same-name
/// disk entries must re-register those bundled commands after this function
/// returns.
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
impl SkillLoader for MobileDiskSkillLoader {
    async fn load(&self, name: &str) -> Result<Option<SkillDescriptor>, ToolError> {
        let reg = self.registry.read().await;
        Ok(reg
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
    async fn resolves_bundled_create_local_app_with_an_empty_disk() {
        let registry = Arc::new(RwLock::new(CommandRegistry::new()));
        {
            let mut reg = registry.write().await;
            crate::register_mobile_skill_commands(&mut reg);
        }
        let loader = MobileDiskSkillLoader::with_session_id(registry, "session-1".into());

        let desc = loader
            .load("create-local-app")
            .await
            .expect("load ok")
            .expect("the bundled mobile skill resolves without any disk state");
        assert_eq!(desc.name, "create-local-app");
        assert_eq!(desc.command_type, SkillCommandType::Prompt);
        assert!(!desc.disable_model_invocation);
        let built = desc
            .dynamic_body
            .as_ref()
            .expect("bundled mobile skills build their prompt dynamically")
            .build("");
        assert!(built.contains("local-app-build"));
        assert!(
            built.contains(concat!(
                "For a\n",
                "   whole-surface `surface: canvas` app, hand off to the matched runtime\n",
                "   specialist. For `canvas_2d` or `three_3d`, use the profile-managed\n",
                "   `createFrameLoop` helper from `lib/frame-loop.js`; never call\n",
                "   `requestAnimationFrame` directly or hand-write a replacement loop."
            )),
            "whole-surface canvas guidance must require the shipped frame-loop helper: {:?}",
            built
        );
        assert!(
            built.contains(
                concat!(
                    "For a routed `surface: dom` app that only\n",
                    "   embeds a canvas or WebGL region, own exactly one\n",
                    "   `requestAnimationFrame` loop for that region, cancel it in the effect\n",
                    "   cleanup, make that loop's lifecycle responsible for DPR-aware buffer\n",
                    "   sizing, viewport or layout resize, and clamping or resetting the first\n",
                    "   delta after resume, and keep per-frame state out of React state and Zustand\n",
                    "   stores."
                )
            ),
            "dom embedded-canvas guidance must retain the scoped requestAnimationFrame fallback: {:?}",
            built
        );
        assert!(
            !built.contains(
                "A drawn surface — a game, a 3D scene, a custom visualization — renders into a `<canvas>` with a `requestAnimationFrame` loop you own and cancel on unmount;"
            ),
            "the old unscoped canvas sentence must be absent: {:?}",
            built
        );
    }

    #[tokio::test]
    async fn resolves_bundled_local_app_specialists_with_router_profiles() {
        let registry = Arc::new(RwLock::new(CommandRegistry::new()));
        {
            let mut reg = registry.write().await;
            crate::register_mobile_skill_commands(&mut reg);
        }
        let loader = MobileDiskSkillLoader::new(registry);

        for (name, profile) in [
            (
                "ionic-react-local-app",
                "references/profiles/app-shell-and-routing.md",
            ),
            (
                "canvas-2d-local-app",
                "references/profiles/game-loop-and-state.md",
            ),
            (
                "threejs-local-app",
                "references/profiles/lifecycle-and-performance.md",
            ),
            (
                "phaser-2d-local-app",
                "references/profiles/scene-lifecycle-and-input.md",
            ),
            (
                "babylon-3d-local-app",
                "references/profiles/engine-lifecycle-and-performance.md",
            ),
        ] {
            let desc = loader
                .load(name)
                .await
                .expect("load ok")
                .expect("bundled local-app specialist resolves without disk state");
            assert_eq!(desc.name, name);
            assert_eq!(desc.command_type, SkillCommandType::Prompt);
            let built = desc
                .dynamic_body
                .as_ref()
                .expect("bundled mobile skills build their prompt dynamically")
                .build("");
            assert!(
                built.contains("## Bundled resource: `references/router.md`"),
                "{name} body must include the router resource"
            );
            assert!(
                built.contains("Follow `references/router.md` first"),
                "{name} body must retain the router-first selection guard"
            );
            assert!(
                built.contains(&format!("## Bundled resource: `{profile}`")),
                "{name} body must include its routed profile resource"
            );
        }
    }

    #[tokio::test]
    async fn bundled_skill_shadows_a_same_named_disk_command() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let cmd_dir = root.join(".lingxi").join("commands");
        tokio::fs::create_dir_all(&cmd_dir).await.unwrap();
        tokio::fs::write(
            cmd_dir.join("create-local-app.md"),
            "---\ndescription: stale decoy\n---\nDECOY BODY\n",
        )
        .await
        .unwrap();

        let registry = Arc::new(RwLock::new(CommandRegistry::new()));
        {
            let mut reg = registry.write().await;
            load_mobile_disk_commands_into_registry(&mut reg, root, &root.join(".lingxi"), root)
                .await;
            crate::register_mobile_skill_commands(&mut reg);
        }
        let loader = MobileDiskSkillLoader::new(registry);
        let desc = loader
            .load("create-local-app")
            .await
            .expect("load ok")
            .expect("resolves");
        let built = desc
            .dynamic_body
            .as_ref()
            .expect("bundled mobile skills build their prompt dynamically")
            .build("");
        assert!(
            !built.contains("DECOY BODY"),
            "the compiled-in skill wins over the on-disk decoy"
        );
        assert!(built.contains("local-app-build"));
    }
}

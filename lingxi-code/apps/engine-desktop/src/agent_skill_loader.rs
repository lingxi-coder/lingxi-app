//! `AgentSkillLoader` — the [`traits::skill_loader::SkillLoader`] impl that backs
//! G5 subagent skill preloading (claude `runAgent.ts:577-646`).
//!
//! claude's `runAgent` resolves each frontmatter `skills:` entry to a registered
//! command (`resolveSkillName`: exact match → the agent's plugin prefix
//! `pluginName:skill` → a `:skill` suffix match), loads its prompt
//! (`skill.getPromptForCommand('', ctx)`), and injects it as a meta user message.
//! This adapter ports that over the shared desktop [`CommandRegistry`] (the same
//! registry the slash dispatcher + `Skill` tool read), so a child agent runner
//! can preload skills WITHOUT the `agent` crate depending on `commands-core`
//! (the leaf [`traits::skill_loader`] seam breaks that cycle).
//!
//! DOCUMENTED APPROXIMATION (flagged): claude's `getPromptForCommand('', ctx)`
//! runs the FULL skill-body pipeline — `$ARGUMENTS`/`$N` argument substitution
//! (here: empty args), `${LINGXI_SKILL_DIR}`/`${LINGXI_SESSION_ID}` token
//! replacement, AND embedded `!command` shell expansion. This loader applies the
//! empty-args argument substitution and the `${LINGXI_SESSION_ID}` token (carried
//! on the descriptor) but does NOT run shell `!command` expansion or
//! `${LINGXI_SKILL_DIR}` substitution — those live in the `Skill` tool's shell
//! substrate, unreachable from this composition-root adapter without re-plumbing
//! the tool. The model still receives the skill's prompt body; full shell
//! expansion parity is a follow-up.

use std::sync::Arc;

use command_api::{CommandRegistry, SlashCommand, SlashCommandKind};
use protocol::ContentBlock;
use tokio::sync::RwLock;
use traits::skill_loader::{SkillLoad, SkillLoader};

/// [`SkillLoader`] backed by the shared desktop [`CommandRegistry`].
pub struct AgentSkillLoader {
    registry: Arc<RwLock<CommandRegistry>>,
    /// Per-session id substituted for `${LINGXI_SESSION_ID}` in the skill body
    /// (claude `getSessionId()`); `None` leaves the token un-substituted.
    session_id: Option<String>,
}

impl AgentSkillLoader {
    /// Wrap the shared command-registry handle (the SAME `Arc` the slash
    /// dispatcher + `Skill` tool read), stamping `session_id` for
    /// `${LINGXI_SESSION_ID}` substitution.
    #[must_use]
    pub fn new(registry: Arc<RwLock<CommandRegistry>>, session_id: Option<String>) -> Self {
        Self {
            registry,
            session_id,
        }
    }

    /// Resolve `skill_name` to a registered command name, mirroring claude
    /// `resolveSkillName` (runAgent.ts:945-973): (1) exact match (incl. aliases,
    /// via `resolve`), (2) the agent's plugin prefix `pluginName:skill`, (3) a
    /// `:skill` suffix match against any registered command.
    fn resolve_name(reg: &CommandRegistry, skill_name: &str, agent_type: &str) -> Option<String> {
        // 1. Direct match (resolve follows aliases).
        if let Some(cmd) = reg.resolve(skill_name) {
            return Some(cmd.name.clone());
        }
        // 2. Prefix with the agent's plugin name (agentType `"pluginName:agentName"`).
        if let Some(prefix) = agent_type.split(':').next() {
            if !prefix.is_empty() {
                let qualified = format!("{prefix}:{skill_name}");
                if let Some(cmd) = reg.resolve(&qualified) {
                    return Some(cmd.name.clone());
                }
            }
        }
        // 3. Suffix match — any command whose name ends with `:skillName`.
        let suffix = format!(":{skill_name}");
        reg.list_all()
            .into_iter()
            .find(|c| c.name.ends_with(&suffix))
            .map(|c| c.name.clone())
    }

    /// Build the skill's content blocks from a resolved markdown/plugin command.
    /// Returns `None` for non-prompt commands (builtin / MCP — claude's
    /// `skill.type !== 'prompt'` skip). Applies empty-args `$ARGUMENTS`/`$N`
    /// substitution + the `${LINGXI_SESSION_ID}` token (see the module-level
    /// approximation note for what is intentionally NOT expanded).
    fn to_skill_load(&self, cmd: &SlashCommand, display_name: &str) -> Option<SkillLoad> {
        let (frontmatter, prompt_template) = match &cmd.kind {
            SlashCommandKind::Markdown {
                frontmatter,
                prompt_template,
                ..
            }
            | SlashCommandKind::Plugin {
                frontmatter,
                prompt_template,
                ..
            } => (frontmatter, prompt_template),
            // Bundled programmatic skill (`/loop`): prompt-typed, but the body is
            // produced by the dynamic builder with empty args (claude
            // `getPromptForCommand('')`). `${LINGXI_SESSION_ID}` still applies.
            SlashCommandKind::Bundled {
                prompt_fn: Some(builder),
                ..
            } => {
                let body = builder.build("");
                let body = match &self.session_id {
                    Some(sid) => body.replace("${LINGXI_SESSION_ID}", sid),
                    None => body,
                };
                return Some(SkillLoad {
                    display_name: display_name.to_string(),
                    progress_message: None,
                    content: vec![ContentBlock::Text { text: body }],
                });
            }
            // Builtin / MCP / inert-bundled commands are not prompt-based skills
            // (claude `skill.type !== 'prompt'`).
            SlashCommandKind::Builtin { .. }
            | SlashCommandKind::Mcp { .. }
            | SlashCommandKind::Bundled { .. } => return None,
        };
        // Empty-args argument substitution (claude `getPromptForCommand('', …)`;
        // matches the `Skill` tool's call: `Some(""), append=true`).
        let body = command_api::substitute_arguments_faithful(
            prompt_template,
            Some(""),
            true,
            &frontmatter.argument_names,
        )
        .unwrap_or_else(|_| prompt_template.clone());
        // `${LINGXI_SESSION_ID}` token (claude step 3, runs regardless of source).
        let body = match &self.session_id {
            Some(sid) => body.replace("${LINGXI_SESSION_ID}", sid),
            None => body,
        };
        Some(SkillLoad {
            display_name: display_name.to_string(),
            // The descriptor's frontmatter `model` is unrelated to progressMessage;
            // the command model carries no progressMessage in the Rust port, so
            // claude's default ("loading") applies — represented as `None`.
            progress_message: None,
            content: vec![ContentBlock::Text { text: body }],
        })
    }
}

#[async_trait::async_trait]
impl SkillLoader for AgentSkillLoader {
    async fn resolve_and_load(&self, skill_name: &str, agent_type: &str) -> Option<SkillLoad> {
        let reg = self.registry.read().await;
        let resolved = Self::resolve_name(&reg, skill_name, agent_type)?;
        let cmd = reg.resolve(&resolved)?;
        // claude passes the ORIGINAL `skillName` (the frontmatter entry) to
        // `formatSkillLoadingMetadata` (runAgent.ts:634), so the loading-metadata
        // block shows the name as authored, not the resolved/qualified name.
        self.to_skill_load(cmd, skill_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use command_api::{CommandFrontmatter, CommandSource};
    use std::path::PathBuf;

    fn md(name: &str, body: &str) -> SlashCommand {
        SlashCommand {
            name: name.to_string(),
            description: format!("{name} desc"),
            source: CommandSource::Project,
            kind: SlashCommandKind::Markdown {
                file_path: PathBuf::from(format!("/x/{name}.md")),
                frontmatter: CommandFrontmatter::default(),
                prompt_template: body.to_string(),
            },
            ..SlashCommand::default()
        }
    }

    #[tokio::test]
    async fn exact_match_loads_body_as_text_block() {
        let mut reg = CommandRegistry::new();
        reg.register_command(md("review", "REVIEW BODY"));
        let loader = AgentSkillLoader::new(Arc::new(RwLock::new(reg)), None);
        let load = loader
            .resolve_and_load("review", "general-purpose")
            .await
            .expect("resolved");
        assert_eq!(load.display_name, "review");
        assert_eq!(load.content.len(), 1);
        assert!(matches!(
            &load.content[0],
            ContentBlock::Text { text } if text == "REVIEW BODY"
        ));
    }

    #[tokio::test]
    async fn plugin_prefix_resolution() {
        // bare "feat" resolves via the agent's plugin prefix "pm:feat".
        let mut reg = CommandRegistry::new();
        reg.register_command(md("pm:feat", "FEATURE"));
        let loader = AgentSkillLoader::new(Arc::new(RwLock::new(reg)), None);
        let load = loader
            .resolve_and_load("feat", "pm:planner")
            .await
            .expect("resolved via plugin prefix");
        // display_name is the ORIGINAL frontmatter entry.
        assert_eq!(load.display_name, "feat");
    }

    #[tokio::test]
    async fn suffix_match_resolution() {
        let mut reg = CommandRegistry::new();
        reg.register_command(md("some-plugin:deep", "DEEP"));
        let loader = AgentSkillLoader::new(Arc::new(RwLock::new(reg)), None);
        let load = loader
            .resolve_and_load("deep", "other-agent")
            .await
            .expect("resolved via suffix");
        assert_eq!(load.display_name, "deep");
    }

    #[tokio::test]
    async fn unknown_skill_is_none() {
        let reg = CommandRegistry::new();
        let loader = AgentSkillLoader::new(Arc::new(RwLock::new(reg)), None);
        assert!(loader.resolve_and_load("nope", "a").await.is_none());
    }

    #[tokio::test]
    async fn builtin_command_is_not_a_prompt_skill() {
        let mut reg = CommandRegistry::new();
        reg.register_command(SlashCommand {
            name: "help".to_string(),
            description: "h".to_string(),
            source: CommandSource::Builtin,
            kind: SlashCommandKind::Builtin {
                handler_id: "help".to_string(),
            },
            ..SlashCommand::default()
        });
        let loader = AgentSkillLoader::new(Arc::new(RwLock::new(reg)), None);
        assert!(loader.resolve_and_load("help", "a").await.is_none());
    }

    #[tokio::test]
    async fn session_id_token_substituted() {
        let mut reg = CommandRegistry::new();
        reg.register_command(md("s", "id=${LINGXI_SESSION_ID}"));
        let loader = AgentSkillLoader::new(Arc::new(RwLock::new(reg)), Some("sess:42".to_string()));
        let load = loader.resolve_and_load("s", "a").await.expect("resolved");
        assert!(matches!(
            &load.content[0],
            ContentBlock::Text { text } if text == "id=sess:42"
        ));
    }
}

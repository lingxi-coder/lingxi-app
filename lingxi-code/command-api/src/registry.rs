//! In-memory registry of slash commands with alias and plugin-scoped lookup.

use crate::model::{BuiltinCommandHandler, CommandSource, SlashCommand, SlashCommandKind};
use protocol::PluginId;
use std::collections::HashMap;
use std::sync::Arc;

/// Owns all known slash commands and built-in handler implementations.
pub struct CommandRegistry {
    commands: HashMap<String, SlashCommand>,
    aliases: HashMap<String, String>,
    builtin_handlers: HashMap<String, Arc<dyn BuiltinCommandHandler>>,
    plugin_commands: HashMap<PluginId, Vec<String>>,
}

impl CommandRegistry {
    /// Construct an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            commands: HashMap::new(),
            aliases: HashMap::new(),
            builtin_handlers: HashMap::new(),
            plugin_commands: HashMap::new(),
        }
    }

    /// Insert a fully-formed [`SlashCommand`] (used for markdown / plugin / MCP
    /// entries), indexing every entry in `cmd.aliases` so the command resolves by
    /// any of its alternate names. Mirrors the TS `findCommand` search over
    /// `name` + `aliases` (`claude-code/src/commands.ts:690`).
    pub fn register_command(&mut self, cmd: SlashCommand) {
        self.index_aliases(&cmd.name, &cmd.aliases);
        self.commands.insert(cmd.name.clone(), cmd);
    }

    /// Point each alias at `target` in the alias map (used by
    /// [`Self::register_command`] / [`Self::register_plugin_commands`]).
    fn index_aliases(&mut self, target: &str, aliases: &[String]) {
        for alias in aliases {
            self.aliases.insert(alias.clone(), target.to_string());
        }
    }

    /// Register a Rust-side built-in handler under its `name()`.
    pub fn register_builtin_handler(&mut self, h: Arc<dyn BuiltinCommandHandler>) {
        let cmd = SlashCommand {
            name: h.name().to_string(),
            description: h.description().to_string(),
            source: CommandSource::Builtin,
            kind: SlashCommandKind::Builtin {
                handler_id: h.name().to_string(),
            },
            ..SlashCommand::default()
        };
        self.commands.insert(h.name().to_string(), cmd);
        self.builtin_handlers.insert(h.name().to_string(), h);
    }

    /// Make `alias` resolve to `target`.
    pub fn register_alias(&mut self, alias: String, target: String) {
        self.aliases.insert(alias, target);
    }

    /// Resolve a name (possibly an alias) to a registered command.
    #[must_use]
    pub fn resolve(&self, name: &str) -> Option<&SlashCommand> {
        let canon = self.aliases.get(name).map_or(name, String::as_str);
        self.commands.get(canon)
    }

    /// Fetch a built-in handler by id.
    ///
    /// Canonicalizes through the aliases map exactly like [`Self::resolve`] does,
    /// then looks up the built-in handler under the canonical name. For a
    /// non-alias `handler_id`, `canon == handler_id`, so behavior is identical to
    /// a plain `builtin_handlers` lookup; aliases (e.g. `continue` → `resume`)
    /// route to the target's handler.
    #[must_use]
    pub fn get_handler(&self, handler_id: &str) -> Option<Arc<dyn BuiltinCommandHandler>> {
        let canon = self
            .aliases
            .get(handler_id)
            .map_or(handler_id, String::as_str);
        self.builtin_handlers.get(canon).cloned()
    }

    /// Register a batch of commands owned by `plugin_id`.
    pub fn register_plugin_commands(&mut self, plugin_id: PluginId, cmds: Vec<SlashCommand>) {
        let names: Vec<String> = cmds.iter().map(|c| c.name.clone()).collect();
        for c in cmds {
            self.index_aliases(&c.name, &c.aliases);
            self.commands.insert(c.name.clone(), c);
        }
        self.plugin_commands.insert(plugin_id, names);
    }

    /// Every registered command, in arbitrary order. Mirrors TS `getCommands`
    /// returning the full command list before any UI/model filtering.
    #[must_use]
    pub fn list_all(&self) -> Vec<&SlashCommand> {
        self.commands.values().collect()
    }

    /// Commands the model is allowed to invoke — every entry whose
    /// `disable_model_invocation` flag is unset. Mirrors the TS filter
    /// `!cmd.disableModelInvocation` used when building the model-facing skill
    /// surface (`claude-code/src/commands.ts:571`).
    #[must_use]
    pub fn model_invocable_commands(&self) -> Vec<&SlashCommand> {
        self.commands
            .values()
            .filter(|c| !c.disable_model_invocation)
            .collect()
    }

    /// Remove every command previously registered under `plugin_id`, along with
    /// any aliases that pointed at those commands.
    pub fn unregister_plugin(&mut self, plugin_id: &PluginId) {
        if let Some(names) = self.plugin_commands.remove(plugin_id) {
            for n in &names {
                self.commands.remove(n);
            }
            self.aliases.retain(|_, target| !names.contains(target));
        }
    }

    /// Remove every command attributed to `loaded_from`, including aliases
    /// pointing at those commands.
    ///
    /// Catalog refreshers use this before a full disk re-scan so deleted skill
    /// files disappear from the live registry instead of surviving forever as
    /// stale entries.
    pub fn unregister_loaded_from(&mut self, loaded_from: &str) -> usize {
        let names: Vec<String> = self
            .commands
            .iter()
            .filter(|(_, command)| command.loaded_from.as_deref() == Some(loaded_from))
            .map(|(name, _)| name.clone())
            .collect();
        for name in &names {
            self.commands.remove(name);
        }
        self.aliases.retain(|_, target| !names.contains(target));
        names.len()
    }
}

impl Default for CommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin_support::unimplemented::UnimplementedCommandHandler;
    use crate::model::{CommandSource, SlashCommandKind};

    fn markdown_cmd(name: &str, aliases: Vec<String>) -> SlashCommand {
        SlashCommand {
            name: name.to_string(),
            description: format!("{name} cmd"),
            source: CommandSource::Project,
            kind: SlashCommandKind::Builtin {
                handler_id: name.to_string(),
            },
            aliases,
            ..SlashCommand::default()
        }
    }

    /// A command registered with aliases resolves by its canonical name and by
    /// each alias (TS `findCommand` over `name` + `aliases`).
    #[test]
    fn register_command_indexes_each_alias() {
        let mut reg = CommandRegistry::new();
        reg.register_command(markdown_cmd(
            "resume",
            vec!["continue".to_string(), "unpause".to_string()],
        ));

        assert_eq!(
            reg.resolve("resume").map(|c| c.name.as_str()),
            Some("resume")
        );
        assert_eq!(
            reg.resolve("continue").map(|c| c.name.as_str()),
            Some("resume"),
            "alias `continue` should resolve to `resume`"
        );
        assert_eq!(
            reg.resolve("unpause").map(|c| c.name.as_str()),
            Some("resume"),
            "alias `unpause` should resolve to `resume`"
        );
        assert!(reg.resolve("missing").is_none());
    }

    /// `model_invocable_commands` excludes entries flagged
    /// `disable_model_invocation` (TS `!cmd.disableModelInvocation`).
    #[test]
    fn model_invocable_commands_excludes_disabled() {
        let mut reg = CommandRegistry::new();
        reg.register_command(markdown_cmd("visible", vec![]));

        let mut hidden = markdown_cmd("hidden", vec![]);
        hidden.disable_model_invocation = true;
        reg.register_command(hidden);

        let invocable: Vec<&str> = reg
            .model_invocable_commands()
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert!(invocable.contains(&"visible"));
        assert!(!invocable.contains(&"hidden"));

        // `list_all` still includes both.
        assert_eq!(reg.list_all().len(), 2);
    }

    /// A `SlashCommand::default()` leaves every new metadata field unset.
    #[test]
    fn default_slash_command_has_defaulted_metadata() {
        let c = SlashCommand::default();
        assert!(!c.disable_model_invocation);
        assert!(!c.has_user_specified_description);
        assert!(c.loaded_from.is_none());
        assert!(c.when_to_use.is_none());
        assert!(c.aliases.is_empty());
        assert!(c.argument_hint.is_none());
        assert!(c.skill_root.is_none());
        assert!(c.user_invocable.is_none());
        assert!(c.content_length.is_none());
    }

    #[test]
    fn unregister_loaded_from_removes_commands_and_aliases() {
        let mut reg = CommandRegistry::new();
        let mut skill = markdown_cmd("review", vec!["rv".to_string()]);
        skill.loaded_from = Some("skills".to_string());
        reg.register_command(skill);
        reg.register_command(markdown_cmd("keep", vec!["k".to_string()]));

        assert_eq!(reg.unregister_loaded_from("skills"), 1);
        assert!(reg.resolve("review").is_none());
        assert!(reg.resolve("rv").is_none());
        assert!(reg.resolve("keep").is_some());
        assert!(reg.resolve("k").is_some());
    }

    /// `get_handler` canonicalizes through the aliases map (mirroring `resolve`),
    /// so an alias (`continue` → `resume`) returns the target's handler while a
    /// non-alias name remains a plain lookup and an unknown name yields `None`.
    #[test]
    fn get_handler_follows_alias() {
        let mut reg = CommandRegistry::new();
        reg.register_builtin_handler(Arc::new(UnimplementedCommandHandler::new(
            "resume",
            "Resume a previous conversation",
        )));
        reg.register_alias("continue".to_string(), "resume".to_string());

        // The alias resolves to the resume handler.
        let via_alias = reg
            .get_handler("continue")
            .expect("alias should map to the resume handler");
        assert_eq!(via_alias.name(), "resume");

        // The canonical name still resolves directly (non-alias no-op path).
        let direct = reg
            .get_handler("resume")
            .expect("canonical name should resolve");
        assert_eq!(direct.name(), "resume");

        // Canonicalization is a no-op for an unknown, non-alias name.
        assert!(reg.get_handler("nonexistent").is_none());
    }
}

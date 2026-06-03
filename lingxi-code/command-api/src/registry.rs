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

    /// Insert a fully-formed [`SlashCommand`] (used for markdown / plugin / MCP entries).
    pub fn register_command(&mut self, cmd: SlashCommand) {
        self.commands.insert(cmd.name.clone(), cmd);
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
        let canon = self.aliases.get(handler_id).map_or(handler_id, String::as_str);
        self.builtin_handlers.get(canon).cloned()
    }

    /// Register a batch of commands owned by `plugin_id`.
    pub fn register_plugin_commands(&mut self, plugin_id: PluginId, cmds: Vec<SlashCommand>) {
        let names: Vec<String> = cmds.iter().map(|c| c.name.clone()).collect();
        for c in cmds {
            self.commands.insert(c.name.clone(), c);
        }
        self.plugin_commands.insert(plugin_id, names);
    }

    /// Remove every command previously registered under `plugin_id`.
    pub fn unregister_plugin(&mut self, plugin_id: &PluginId) {
        if let Some(names) = self.plugin_commands.remove(plugin_id) {
            for n in &names {
                self.commands.remove(n);
            }
        }
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

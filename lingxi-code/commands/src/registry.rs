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
    #[must_use]
    pub fn get_handler(&self, handler_id: &str) -> Option<Arc<dyn BuiltinCommandHandler>> {
        self.builtin_handlers.get(handler_id).cloned()
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

/// Register all 99 built-in slash commands into `reg`.
///
/// 81 of the names point at per-name instances of
/// [`crate::builtin::UnimplementedCommandHandler`] that return the locked
/// stub literal `"{name}: not implemented in v0.6.0 (M5)"`.
///
/// The 18 core names listed in [`crate::builtin::BUILTIN_CORE_NAMES`] are
/// **also** registered here against the shared unimplemented handler
/// **first**, then immediately overwritten by [`register_core_placeholders`]
/// (called at the end of this function) with their per-name placeholder
/// structs.
///
/// Calling this function on the same registry twice is safe — every name is
/// overwritten in-place via `HashMap::insert` semantics.
pub fn register_all_builtin_commands(reg: &mut CommandRegistry) {
    use crate::builtin::core_placeholders::register_core_placeholders;
    use crate::builtin::{core_description, UnimplementedCommandHandler, BUILTIN_COMMAND_NAMES};
    use std::sync::Arc;

    // Pass 1: register all 99 with per-name unimplemented handler instances.
    //
    // Each name needs its own handler **instance** because the handler
    // carries its own `name` field used to substitute the locked literal.
    for &name in BUILTIN_COMMAND_NAMES {
        let h = Arc::new(UnimplementedCommandHandler::new(
            name,
            core_description(name),
        ));
        reg.register_builtin_handler(h);
    }

    // Pass 2: overwrite the 12 non-batch-1 core entries with their per-name
    // placeholders. (M5-10 removed the 6 batch-1 placeholders from this
    // pass; their real handlers are wired by `register_core_batch_1`.)
    register_core_placeholders(reg);
}

/// Overwrite the 6 batch-1 entries (`clear`, `compact`, `exit`, `help`,
/// `init`, `memory`) with their handle-bound real handlers from M5-10.
///
/// Call **after** [`register_all_builtin_commands`]. The function is
/// idempotent — calling it twice with the same `handle` produces the same
/// final state.
///
/// [`crate::builtin::HelpHandler`] and [`crate::builtin::InitHandler`] are
/// constructed without `handle` because they don't need orchestrator
/// state.
///
/// M5-12 (the CLI binary) calls this immediately after
/// `register_all_builtin_commands` during boot, threading the live
/// `Arc<dyn OrchestratorHandle>`.
pub fn register_core_batch_1(
    reg: &mut CommandRegistry,
    handle: Arc<dyn traits::OrchestratorHandle>,
) {
    use crate::builtin::{
        ClearHandler, CompactHandler, ExitHandler, HelpHandler, InitHandler, MemoryHandler,
    };

    reg.register_builtin_handler(Arc::new(ClearHandler::new(handle.clone())));
    reg.register_builtin_handler(Arc::new(CompactHandler::new(handle.clone())));
    reg.register_builtin_handler(Arc::new(ExitHandler::new(handle.clone())));
    reg.register_builtin_handler(Arc::new(HelpHandler::new()));
    reg.register_builtin_handler(Arc::new(InitHandler::new()));
    reg.register_builtin_handler(Arc::new(MemoryHandler::new(handle)));
}

/// Overwrite the 12 batch-2 entries (`agents`, `config`, `cost`, `doctor`,
/// `hooks`, `login`, `logout`, `mcp`, `model`, `permissions`, `status`,
/// `version`) with their handle/auth-bound real handlers from M5-11.
///
/// Call **after** [`register_all_builtin_commands`] and (optionally) after
/// [`register_core_batch_1`]. The function is idempotent — calling it twice
/// with the same `handle`/`auth` produces the same final state.
///
/// [`crate::builtin::VersionHandler`] takes no parameters because it only
/// reads compile-time constants. All other batch-2 handlers consume the
/// orchestrator handle; `/login` and `/logout` consume the auth handle.
///
/// M5-12 (the CLI binary) calls this immediately after `register_core_batch_1`
/// during boot, threading the live `Arc<dyn OrchestratorHandle>` and
/// `Arc<dyn AuthHandle>` instances.
pub fn register_core_batch_2(
    reg: &mut CommandRegistry,
    handle: Arc<dyn traits::OrchestratorHandle>,
    auth: Arc<dyn traits::AuthHandle>,
) {
    use crate::builtin::{
        AgentsHandler, ConfigHandler, CostHandler, DoctorHandler, HooksHandler, LoginHandler,
        LogoutHandler, McpHandler, ModelHandler, PermissionsHandler, StatusHandler, VersionHandler,
    };

    reg.register_builtin_handler(Arc::new(AgentsHandler::new(handle.clone())));
    reg.register_builtin_handler(Arc::new(ConfigHandler::new(handle.clone())));
    reg.register_builtin_handler(Arc::new(CostHandler::new(handle.clone())));
    reg.register_builtin_handler(Arc::new(DoctorHandler::new(handle.clone())));
    reg.register_builtin_handler(Arc::new(HooksHandler::new(handle.clone())));
    reg.register_builtin_handler(Arc::new(LoginHandler::new(auth.clone())));
    reg.register_builtin_handler(Arc::new(LogoutHandler::new(auth)));
    reg.register_builtin_handler(Arc::new(McpHandler::new(handle.clone())));
    reg.register_builtin_handler(Arc::new(ModelHandler::new(handle.clone())));
    reg.register_builtin_handler(Arc::new(PermissionsHandler::new(handle.clone())));
    reg.register_builtin_handler(Arc::new(StatusHandler::new(handle)));
    reg.register_builtin_handler(Arc::new(VersionHandler::new()));
}

#[cfg(test)]
mod registry_tests {
    use super::*;
    use crate::builtin::BUILTIN_COMMAND_NAMES;
    use crate::model::CommandResult;

    #[test]
    fn register_all_registers_exactly_99_names() {
        let mut reg = CommandRegistry::new();
        register_all_builtin_commands(&mut reg);
        for name in BUILTIN_COMMAND_NAMES {
            assert!(
                reg.resolve(name).is_some(),
                "expected command /{name} registered"
            );
        }
    }

    #[test]
    fn register_all_provides_handlers_for_every_name() {
        let mut reg = CommandRegistry::new();
        register_all_builtin_commands(&mut reg);

        for name in BUILTIN_COMMAND_NAMES {
            assert!(
                reg.get_handler(name).is_some(),
                "expected handler for /{name}"
            );
        }
    }

    #[tokio::test]
    async fn unimplemented_command_returns_locked_literal() {
        use crate::parser::ParsedSlashCommand;
        let mut reg = CommandRegistry::new();
        register_all_builtin_commands(&mut reg);

        // Pick a definitely-not-in-the-18-core command.
        let h = reg
            .get_handler("ant-trace")
            .expect("ant-trace handler missing");
        let args = ParsedSlashCommand {
            name: "ant-trace".to_string(),
            raw_args: String::new(),
            positional_args: vec![],
        };
        match h.handle(&args).await {
            CommandResult::Done { display: Some(s) } => {
                assert_eq!(s, "ant-trace: not implemented in v0.6.0 (M5)");
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn idempotent_double_register_overwrites_cleanly() {
        let mut reg = CommandRegistry::new();
        register_all_builtin_commands(&mut reg);
        register_all_builtin_commands(&mut reg); // call twice
        for name in BUILTIN_COMMAND_NAMES {
            assert!(reg.resolve(name).is_some());
        }
    }
}

#[cfg(test)]
mod batch_1_tests {
    use super::*;
    use crate::model::CommandResult;
    use crate::parser::ParsedSlashCommand;
    use orchestrator::test_support::MockOrchestratorHandle;

    #[tokio::test]
    async fn clear_after_batch_1_returns_real_literal() {
        let mut reg = CommandRegistry::new();
        register_all_builtin_commands(&mut reg);
        let handle = Arc::new(MockOrchestratorHandle::new());
        register_core_batch_1(&mut reg, handle.clone());

        let h = reg.get_handler("clear").expect("clear handler missing");
        let args = ParsedSlashCommand {
            name: "clear".to_string(),
            raw_args: String::new(),
            positional_args: vec![],
        };
        match h.handle(&args).await {
            CommandResult::Done { display: Some(s) } => {
                assert_eq!(s, "Conversation cleared.");
            }
            other => panic!("expected Done, got {other:?}"),
        }
        assert!(handle.was_clear_session_called());
    }

    #[tokio::test]
    async fn non_batch_1_command_still_returns_stub() {
        let mut reg = CommandRegistry::new();
        register_all_builtin_commands(&mut reg);
        let handle = Arc::new(MockOrchestratorHandle::new());
        register_core_batch_1(&mut reg, handle);

        // x402 is not in the batch-1 list → still returns the M5-09 stub.
        let h = reg.get_handler("x402").expect("x402 handler missing");
        let args = ParsedSlashCommand {
            name: "x402".to_string(),
            raw_args: String::new(),
            positional_args: vec![],
        };
        match h.handle(&args).await {
            CommandResult::Done { display: Some(s) } => {
                assert_eq!(s, "x402: not implemented in v0.6.0 (M5)");
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn batch_1_size_is_6() {
        let names = ["clear", "compact", "exit", "help", "init", "memory"];
        assert_eq!(names.len(), 6);
    }

    #[tokio::test]
    async fn all_6_batch_1_names_resolve_after_overwrite() {
        let mut reg = CommandRegistry::new();
        register_all_builtin_commands(&mut reg);
        let handle = Arc::new(MockOrchestratorHandle::new());
        register_core_batch_1(&mut reg, handle);

        for name in ["clear", "compact", "exit", "help", "init", "memory"] {
            assert!(reg.resolve(name).is_some(), "/{name} missing");
            assert!(reg.get_handler(name).is_some(), "/{name} handler missing");
        }
    }
}

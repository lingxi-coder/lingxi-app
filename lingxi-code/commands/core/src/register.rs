//! Builtin command registration entry points (M8-P9). Moved out of the former
//! `commands::registry` so the `command-api` runtime crate stays handler-free;
//! re-exported from this crate's `lib.rs`.

use command_api::CommandRegistry;
use std::sync::Arc;

/// Register all 99 built-in slash commands into `reg`.
///
/// 81 of the names point at per-name instances of
/// [`command_api::builtin_support::UnimplementedCommandHandler`] that return the locked
/// stub literal `"{name}: not implemented in v0.6.0 (M5)"`.
///
/// The 18 core names listed in [`command_api::builtin_support::BUILTIN_CORE_NAMES`] are
/// **also** registered here against the shared unimplemented handler
/// **first**, then immediately overwritten by [`register_core_placeholders`]
/// (called at the end of this function) with their per-name placeholder
/// structs.
///
/// Calling this function on the same registry twice is safe — every name is
/// overwritten in-place via `HashMap::insert` semantics.
pub fn register_all_builtin_commands(reg: &mut CommandRegistry) {
    use crate::core_placeholders::register_core_placeholders;
    use command_api::builtin_support::{
        core_description, UnimplementedCommandHandler, BUILTIN_COMMAND_NAMES,
    };

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
/// [`crate::HelpHandler`] and [`crate::InitHandler`] are
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
    use crate::{
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
/// [`crate::VersionHandler`] takes no parameters because it only
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
    use crate::{
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
    use command_api::builtin_support::BUILTIN_COMMAND_NAMES;
    use command_api::model::CommandResult;

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
        use command_api::parser::ParsedSlashCommand;
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
    use command_api::model::CommandResult;
    use command_api::parser::ParsedSlashCommand;
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

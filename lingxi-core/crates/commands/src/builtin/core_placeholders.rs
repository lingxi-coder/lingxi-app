//! Per-name placeholder handler structs for the 18 core commands.
//!
//! In M5-09 each placeholder returns the same locked stub literal as
//! [`super::unimplemented::UnimplementedCommandHandler`]. M5-10 / M5-11
//! replace each placeholder's body with the real implementation, one
//! struct at a time, without touching registry wiring.
//!
//! See plan `docs/superpowers/plans/2026-05-25-m5-09-commands-surface.md`
//! Task 4.

use crate::builtin::names::core_description;
use crate::builtin::unimplemented::UnimplementedCommandHandler;
use crate::model::{BuiltinCommandHandler, CommandResult};
use crate::parser::ParsedSlashCommand;
use crate::registry::CommandRegistry;
use async_trait::async_trait;
use std::sync::Arc;

/// Generate a per-name placeholder struct + `BuiltinCommandHandler` impl.
///
/// In M5-09 every per-name placeholder's body simply returns
/// `UnimplementedCommandHandler::stub_literal(name)`. M5-10 / M5-11 replace
/// the macro-generated `handle` body with the real implementation by
/// changing the macro call to a hand-written impl for the affected names.
macro_rules! core_placeholder {
    ($struct_name:ident, $name_literal:literal) => {
        #[doc = concat!("Placeholder for /", $name_literal, " — body returns the M5-09 stub literal until M5-10/M5-11 lands the real implementation.")]
        #[derive(Debug, Default)]
        pub struct $struct_name;

        impl $struct_name {
            #[doc = concat!("Construct a new ", stringify!($struct_name), " placeholder.")]
            #[must_use]
            pub fn new() -> Self {
                Self
            }
        }

        #[async_trait]
        impl BuiltinCommandHandler for $struct_name {
            async fn handle(&self, _args: &ParsedSlashCommand) -> CommandResult {
                CommandResult::Done {
                    display: Some(UnimplementedCommandHandler::stub_literal($name_literal)),
                }
            }
            fn name(&self) -> &str {
                $name_literal
            }
            fn description(&self) -> &str {
                core_description($name_literal)
            }
        }
    };
}

core_placeholder!(AgentsHandler, "agents");
core_placeholder!(ClearHandler, "clear");
core_placeholder!(CompactHandler, "compact");
core_placeholder!(ConfigHandler, "config");
core_placeholder!(CostHandler, "cost");
core_placeholder!(DoctorHandler, "doctor");
core_placeholder!(ExitHandler, "exit");
core_placeholder!(HelpHandler, "help");
core_placeholder!(HooksHandler, "hooks");
core_placeholder!(InitHandler, "init");
core_placeholder!(LoginHandler, "login");
core_placeholder!(LogoutHandler, "logout");
core_placeholder!(McpHandler, "mcp");
core_placeholder!(MemoryHandler, "memory");
core_placeholder!(ModelHandler, "model");
core_placeholder!(PermissionsHandler, "permissions");
core_placeholder!(StatusHandler, "status");
core_placeholder!(VersionHandler, "version");

/// Register the 18 per-name core placeholders, overwriting the shared
/// unimplemented entries put down by [`crate::register_all_builtin_commands`].
///
/// Each placeholder is a distinct struct (with its own `TypeId`) so M5-10 and
/// M5-11 can replace the bodies one name at a time without touching registry
/// wiring or the other 17 placeholders.
pub fn register_core_placeholders(reg: &mut CommandRegistry) {
    reg.register_builtin_handler(Arc::new(AgentsHandler::new()));
    reg.register_builtin_handler(Arc::new(ClearHandler::new()));
    reg.register_builtin_handler(Arc::new(CompactHandler::new()));
    reg.register_builtin_handler(Arc::new(ConfigHandler::new()));
    reg.register_builtin_handler(Arc::new(CostHandler::new()));
    reg.register_builtin_handler(Arc::new(DoctorHandler::new()));
    reg.register_builtin_handler(Arc::new(ExitHandler::new()));
    reg.register_builtin_handler(Arc::new(HelpHandler::new()));
    reg.register_builtin_handler(Arc::new(HooksHandler::new()));
    reg.register_builtin_handler(Arc::new(InitHandler::new()));
    reg.register_builtin_handler(Arc::new(LoginHandler::new()));
    reg.register_builtin_handler(Arc::new(LogoutHandler::new()));
    reg.register_builtin_handler(Arc::new(McpHandler::new()));
    reg.register_builtin_handler(Arc::new(MemoryHandler::new()));
    reg.register_builtin_handler(Arc::new(ModelHandler::new()));
    reg.register_builtin_handler(Arc::new(PermissionsHandler::new()));
    reg.register_builtin_handler(Arc::new(StatusHandler::new()));
    reg.register_builtin_handler(Arc::new(VersionHandler::new()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::names::BUILTIN_CORE_NAMES;
    use crate::model::CommandResult;

    #[test]
    fn all_18_core_placeholders_have_distinct_type_ids() {
        let mut reg = CommandRegistry::new();
        crate::register_all_builtin_commands(&mut reg);

        // After overwrite, each of the 18 core names still resolves.
        for name in BUILTIN_CORE_NAMES {
            assert!(reg.resolve(name).is_some(), "core /{name} disappeared");
            assert!(
                reg.get_handler(name).is_some(),
                "core /{name} has no handler"
            );
        }
    }

    #[tokio::test]
    async fn core_placeholder_returns_same_locked_literal_in_m5_09() {
        let mut reg = CommandRegistry::new();
        crate::register_all_builtin_commands(&mut reg);

        // Pick a core command — its placeholder returns the M5-09 stub.
        let h = reg.get_handler("clear").expect("clear handler missing");
        let args = ParsedSlashCommand {
            name: "clear".to_string(),
            raw_args: String::new(),
            positional_args: vec![],
        };
        match h.handle(&args).await {
            CommandResult::Done { display: Some(s) } => {
                assert_eq!(s, "clear: not implemented in v0.6.0 (M5)");
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn core_placeholder_carries_real_description() {
        let mut reg = CommandRegistry::new();
        crate::register_all_builtin_commands(&mut reg);
        let h = reg.get_handler("help").expect("help handler missing");
        assert_eq!(h.description(), "Show help and available commands");
    }

    #[tokio::test]
    async fn all_18_core_placeholders_return_locked_literal() {
        let mut reg = CommandRegistry::new();
        crate::register_all_builtin_commands(&mut reg);
        for name in BUILTIN_CORE_NAMES {
            let h = reg
                .get_handler(name)
                .unwrap_or_else(|| panic!("no handler /{name}"));
            let args = ParsedSlashCommand {
                name: (*name).to_string(),
                raw_args: String::new(),
                positional_args: vec![],
            };
            match h.handle(&args).await {
                CommandResult::Done { display: Some(s) } => {
                    assert_eq!(s, format!("{name}: not implemented in v0.6.0 (M5)"));
                }
                other => panic!("/{name} did not return Done, got {other:?}"),
            }
        }
    }

    #[test]
    fn core_placeholder_struct_is_distinct_from_unimplemented() {
        use std::any::Any;
        // Construct each placeholder directly and check its TypeId is different
        // from UnimplementedCommandHandler.
        let unimpl = UnimplementedCommandHandler::new("test", "");
        let unimpl_tid = (&unimpl as &dyn Any).type_id();

        let clear = ClearHandler::new();
        assert_ne!(
            (&clear as &dyn Any).type_id(),
            unimpl_tid,
            "ClearHandler must be a distinct type"
        );

        let help = HelpHandler::new();
        assert_ne!(
            (&help as &dyn Any).type_id(),
            unimpl_tid,
            "HelpHandler must be a distinct type"
        );

        // Spot-check a couple more.
        let cost = CostHandler::new();
        assert_ne!((&cost as &dyn Any).type_id(), unimpl_tid);
        let init = InitHandler::new();
        assert_ne!((&init as &dyn Any).type_id(), unimpl_tid);
    }
}

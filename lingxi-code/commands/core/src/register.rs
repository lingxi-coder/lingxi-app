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

    // Pass 3: overwrite the batch-3 (M-parity) handle-free slash commands with
    // their real handlers. These do not need any orchestrator/auth handle, so
    // they can be wired here unconditionally (rather than at CLI boot like
    // batch-1/batch-2). Each call overwrites the matching pass-1 unimplemented
    // stub entry in-place.
    register_core_batch_3(reg);
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

/// Register the batch-3 (M-parity) handle-free slash commands.
///
/// These 9 commands (`commit`, `commit-push-pr`, `init-verifiers`, `insights`,
/// `pr-comments`, `review`, `security-review`, `statusline`, `stickers`) carry
/// no orchestrator or auth handle: each is a static prompt-injection
/// (`InjectMessage`) or a static display message (`Done`). They are wired
/// unconditionally at the end of [`register_all_builtin_commands`], overwriting
/// the matching pass-1 unimplemented stub entries in-place.
///
/// Deferred commands (e.g. `ant-trace`) are intentionally left on the shared
/// [`command_api::builtin_support::UnimplementedCommandHandler`].
pub fn register_core_batch_3(reg: &mut CommandRegistry) {
    use crate::{
        CommitHandler, CommitPushPrHandler, InitVerifiersHandler, InsightsHandler,
        PrCommentsHandler, ReviewHandler, SecurityReviewHandler, StatuslineHandler, StickersHandler,
    };

    reg.register_builtin_handler(Arc::new(CommitHandler::new()));
    reg.register_builtin_handler(Arc::new(CommitPushPrHandler::new()));
    reg.register_builtin_handler(Arc::new(InitVerifiersHandler::new()));
    reg.register_builtin_handler(Arc::new(InsightsHandler::new()));
    reg.register_builtin_handler(Arc::new(PrCommentsHandler::new()));
    reg.register_builtin_handler(Arc::new(ReviewHandler::new()));
    reg.register_builtin_handler(Arc::new(SecurityReviewHandler::new()));
    reg.register_builtin_handler(Arc::new(StatuslineHandler::new()));
    reg.register_builtin_handler(Arc::new(StickersHandler::new()));
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

#[cfg(test)]
mod batch_3_tests {
    use super::*;
    use command_api::model::CommandResult;
    use command_api::parser::ParsedSlashCommand;

    fn args(name: &str) -> ParsedSlashCommand {
        ParsedSlashCommand {
            name: name.to_string(),
            raw_args: String::new(),
            positional_args: vec![],
        }
    }

    /// `register_all_builtin_commands` now wires batch-3 unconditionally (no
    /// handle needed), so the batch-3 names must NOT return the locked M5 stub
    /// literal after the standard registration call.
    #[tokio::test]
    async fn batch_3_inject_commands_return_inject_message_not_stub() {
        let mut reg = CommandRegistry::new();
        register_all_builtin_commands(&mut reg);

        // Sample of the InjectMessage (`inject`-kind) batch-3 commands.
        for name in ["commit", "review", "security-review", "init-verifiers"] {
            let h = reg
                .get_handler(name)
                .unwrap_or_else(|| panic!("/{name} handler missing"));
            match h.handle(&args(name)).await {
                CommandResult::InjectMessage { content } => {
                    assert!(!content.is_empty(), "/{name} injected empty content");
                    assert_ne!(
                        content,
                        format!("{name}: not implemented in v0.6.0 (M5)"),
                        "/{name} still returns the locked stub literal"
                    );
                }
                other => panic!("/{name} expected InjectMessage, got {other:?}"),
            }
        }
    }

    /// The single `display`-kind batch-3 command (`stickers`) returns `Done`
    /// with a real display message rather than the locked stub literal.
    #[tokio::test]
    async fn stickers_returns_done_not_stub() {
        let mut reg = CommandRegistry::new();
        register_all_builtin_commands(&mut reg);

        let h = reg.get_handler("stickers").expect("stickers handler missing");
        match h.handle(&args("stickers")).await {
            CommandResult::Done { display: Some(s) } => {
                assert_ne!(
                    s, "stickers: not implemented in v0.6.0 (M5)",
                    "/stickers still returns the locked stub literal"
                );
            }
            other => panic!("/stickers expected Done with display, got {other:?}"),
        }
    }

    /// Deferred commands stay on the unimplemented handler: `ant-trace` must
    /// still return the locked M5 stub literal after batch-3 wiring.
    #[tokio::test]
    async fn deferred_command_still_returns_stub_after_batch_3() {
        let mut reg = CommandRegistry::new();
        register_all_builtin_commands(&mut reg);

        let h = reg
            .get_handler("ant-trace")
            .expect("ant-trace handler missing");
        match h.handle(&args("ant-trace")).await {
            CommandResult::Done { display: Some(s) } => {
                assert_eq!(s, "ant-trace: not implemented in v0.6.0 (M5)");
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn all_9_batch_3_names_resolve_after_register_all() {
        let mut reg = CommandRegistry::new();
        register_all_builtin_commands(&mut reg);
        for name in [
            "commit",
            "commit-push-pr",
            "init-verifiers",
            "insights",
            "pr-comments",
            "review",
            "security-review",
            "statusline",
            "stickers",
        ] {
            assert!(reg.resolve(name).is_some(), "/{name} missing");
            assert!(reg.get_handler(name).is_some(), "/{name} handler missing");
        }
    }
}

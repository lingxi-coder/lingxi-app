//! Builtin command registration entry points (M8-P9). Moved out of the former
//! `commands::registry` so the `command-api` runtime crate stays handler-free;
//! re-exported from this crate's `lib.rs`.

use command_api::CommandRegistry;
use std::sync::Arc;

/// Register all 94 built-in slash commands into `reg`.
///
/// The non-core names point at per-name instances of
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

    // Pass 1: register all 94 with per-name unimplemented handler instances.
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

    // Pass 4: overwrite the handle-free `/skills` handler. The TUI intercepts
    // `/skills` and opens the full-screen viewer; this handler covers registry
    // / bridge / headless paths with the same text rendering.
    register_core_batch_7(reg);

    // Pass 5: overwrite the batch-6 handle-free `/keybindings` handler. Like
    // batch-3 it carries no orchestrator/auth handle, so it is wired here
    // unconditionally with the external-user default gate (disabled → preview
    // branch). Composition roots that resolve the customization flag at boot may
    // re-call `register_core_batch_6` with `enabled = true` to overwrite this
    // entry in-place.
    register_core_batch_6(reg, false);

    register_interactive_only_commands(reg);
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

    // claude-code parity (#61/#68): `/clear` carries aliases ["reset","new"] and
    // `/exit` carries ["quit"]. Like `continue→resume`, these are NOT builtin
    // names (absent from `BUILTIN_COMMAND_NAMES`) — they dispatch to the target
    // handler without their own palette row.
    reg.register_alias("reset".to_string(), "clear".to_string());
    reg.register_alias("new".to_string(), "clear".to_string());
    reg.register_alias("quit".to_string(), "exit".to_string());
}

/// Overwrite the 11 batch-2 entries (`agents`, `config`, `doctor`, `hooks`,
/// `login`, `logout`, `mcp`, `model`, `permissions`, `status`, `version`) with
/// their handle/auth-bound real handlers from M5-11.
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
        AgentsHandler, ConfigHandler, DoctorHandler, HooksHandler, LoginHandler, LogoutHandler,
        McpHandler, ModelHandler, PermissionsHandler, StatusHandler, VersionHandler,
    };

    reg.register_builtin_handler(Arc::new(AgentsHandler::new(handle.clone())));
    reg.register_builtin_handler(Arc::new(ConfigHandler::new(handle.clone())));
    reg.register_builtin_handler(Arc::new(DoctorHandler::new(handle.clone())));
    reg.register_builtin_handler(Arc::new(HooksHandler::new(handle.clone())));
    reg.register_builtin_handler(Arc::new(LoginHandler::new(auth.clone())));
    reg.register_builtin_handler(Arc::new(LogoutHandler::new(auth)));
    reg.register_builtin_handler(Arc::new(McpHandler::new(handle.clone())));
    reg.register_builtin_handler(Arc::new(ModelHandler::new(handle.clone())));
    reg.register_builtin_handler(Arc::new(PermissionsHandler::new(handle.clone())));
    reg.register_builtin_handler(Arc::new(StatusHandler::new(handle)));
    reg.register_builtin_handler(Arc::new(VersionHandler::new()));

    // claude-code parity (#61): `/config` carries the alias ["settings"] and
    // `/permissions` carries ["allowed-tools"]. Resolution-only aliases (no
    // separate palette row), like `continue→resume`.
    reg.register_alias("settings".to_string(), "config".to_string());
    reg.register_alias("allowed-tools".to_string(), "permissions".to_string());
}

/// Overwrite the 4 batch-4 (engine-data) entries (`context`, `export`,
/// `files`, `resume`) with their handle-bound real handlers.
///
/// Call **after** [`register_all_builtin_commands`] and (optionally) after
/// [`register_core_batch_2`]. The function is idempotent — calling it twice
/// with the same `handle` produces the same final state.
///
/// All four handlers consume the orchestrator handle: `/context` reads the
/// context-window usage + status snapshot, `/export` reads the conversation
/// transcript, `/files` reads the read-file-state cache, and `/resume`
/// enumerates the on-disk session store. Each call overwrites the matching
/// pass-1 unimplemented stub entry in-place.
///
/// `/resume` additionally carries the `continue` alias (claude-code parity:
/// `src/commands/resume/index.ts` declares `aliases: ['continue']`), registered
/// here adjacent to the handler it targets. The alias is intentionally *not* a
/// builtin name (it is absent from `BUILTIN_COMMAND_NAMES`), so `/continue`
/// dispatches to the resume handler but does not appear as its own palette row —
/// matching claude-code, where `continue` shares `resume`'s palette row (it
/// carries no separate `Command` object, only an `aliases: ['continue']` entry).
///
/// The composition roots (`apps/engine-desktop`, `apps/engine-mobile`) call
/// this immediately after `register_core_batch_2`, threading the live
/// `Arc<dyn OrchestratorHandle>`.
pub fn register_core_batch_4(
    reg: &mut CommandRegistry,
    handle: Arc<dyn traits::OrchestratorHandle>,
) {
    use crate::{ContextHandler, ExportHandler, FilesHandler, ResumeHandler};

    reg.register_builtin_handler(Arc::new(ContextHandler::new(handle.clone())));
    reg.register_builtin_handler(Arc::new(ExportHandler::new(handle.clone())));
    reg.register_builtin_handler(Arc::new(FilesHandler::new(handle.clone())));
    reg.register_builtin_handler(Arc::new(ResumeHandler::new(handle)));

    // claude-code parity: `/resume` carries the `continue` alias
    // (src/commands/resume/index.ts: aliases: ['continue']).
    reg.register_alias("continue".to_string(), "resume".to_string());
}

/// Register the batch-3 (M-parity) handle-free slash commands.
///
/// These 9 commands (`commit`, `commit-push-pr`, `init-verifiers`, `insights`,
/// `release-notes`, `review`, `security-review`, `statusline`, `stickers`)
/// carry no orchestrator or auth handle: each is a static prompt-injection
/// (`InjectMessage`) or a static display message (`Done`). They are wired
/// unconditionally at the end of [`register_all_builtin_commands`], overwriting
/// the matching pass-1 unimplemented stub entries in-place.
///
/// `release-notes` (the changelog URL fallback) joins the batch-3 set here as a
/// handle-free `Done` display.
///
/// NOTE: `output-style` and `pr-comments` were removed in claude-code v2.1.183
/// (0 command objects in the binary) and are no longer LingXi builtins (slash
/// parity #67), so their former batch-3 handlers were deleted.
///
/// Deferred commands (e.g. `ant-trace`) are intentionally left on the shared
/// [`command_api::builtin_support::UnimplementedCommandHandler`].
pub fn register_core_batch_3(reg: &mut CommandRegistry) {
    use crate::{
        CommitHandler, CommitPushPrHandler, InitVerifiersHandler, InsightsHandler,
        ReleaseNotesHandler, ReviewHandler, SecurityReviewHandler, StatuslineHandler,
        StickersHandler,
    };

    reg.register_builtin_handler(Arc::new(CommitHandler::new()));
    reg.register_builtin_handler(Arc::new(CommitPushPrHandler::new()));
    reg.register_builtin_handler(Arc::new(InitVerifiersHandler::new()));
    reg.register_builtin_handler(Arc::new(InsightsHandler::new()));
    reg.register_builtin_handler(Arc::new(ReleaseNotesHandler::new()));
    reg.register_builtin_handler(Arc::new(ReviewHandler::new()));
    reg.register_builtin_handler(Arc::new(SecurityReviewHandler::new()));
    reg.register_builtin_handler(Arc::new(StatuslineHandler::new()));
    reg.register_builtin_handler(Arc::new(StickersHandler::new()));
}

/// Overwrite the batch-5 entries (`effort`, `usage`) with handle-bound real
/// handlers.
///
/// Call **after** [`register_all_builtin_commands`] and (optionally) after
/// [`register_core_batch_4`]. The function is idempotent — calling it twice
/// with the same `handle` produces the same final state.
///
/// [`crate::EffortHandler`] consumes the orchestrator handle: the
/// `/effort` / `/effort current` branch reads the current model string via
/// `OrchestratorHandle::get_status_snapshot` (the model-default effort resolver
/// itself is deferred — see the handler docs). The call overwrites the matching
/// pass-1 unimplemented stub entry in-place.
///
/// The composition roots (`apps/engine-desktop`, `apps/engine-mobile`) call
/// this immediately after `register_core_batch_4`, threading the live
/// `Arc<dyn OrchestratorHandle>`.
pub fn register_core_batch_5(
    reg: &mut CommandRegistry,
    handle: Arc<dyn traits::OrchestratorHandle>,
) {
    use crate::{EffortHandler, UsageHandler};

    reg.register_builtin_handler(Arc::new(EffortHandler::new(handle.clone())));
    reg.register_builtin_handler(Arc::new(UsageHandler::new(handle)));
}

/// Overwrite the batch-6 entry (`keybindings`) with its handle-free real
/// handler.
///
/// `/keybindings` is a `type: 'local'` claude-code command that writes the
/// keybindings template (exclusive-create) and opens it in `$EDITOR`. It needs
/// no orchestrator or auth handle — it does the filesystem work and editor
/// spawn directly — so it is wired unconditionally at the end of
/// [`register_all_builtin_commands`] (mirroring the batch-3 pattern), with the
/// external-user default gate (`enabled = false` → the "currently in preview"
/// branch).
///
/// `enabled` mirrors the TS `isKeybindingCustomizationEnabled()` `GrowthBook`
/// gate: composition roots that resolve that flag at boot may re-call this with
/// `enabled = true` to overwrite the entry in-place and unlock the
/// create/open branches. The function is idempotent — calling it twice with the
/// same `enabled` produces the same final state.
pub fn register_core_batch_6(reg: &mut CommandRegistry, enabled: bool) {
    use crate::KeybindingsHandler;

    reg.register_builtin_handler(Arc::new(KeybindingsHandler::with_enabled(enabled)));
}

/// Register the additive `/connect` command (Plan 3c). Not a locked builtin name,
/// so this is a pure addition; idempotent. Composition roots call this after the
/// core batch registrars, threading the engine-built seams.
pub fn register_core_connect(
    reg: &mut CommandRegistry,
    writer: Arc<dyn crate::ConnectCredentialWriter>,
    copilot: Arc<dyn crate::CopilotConnectDriver>,
    chatgpt: Arc<dyn crate::ChatGptConnectDriver>,
) {
    use crate::ConnectHandler;
    reg.register_builtin_handler(Arc::new(ConnectHandler::new(writer, copilot, chatgpt)));
}

/// Overwrite the handle-free `/skills` entry with its non-TUI real handler.
///
/// Claude Code implements `/skills` as an interactive `local-jsx` menu. `LingXi`'s
/// TUI opens the full-screen viewer before the slash dispatcher runs; this
/// command handler exists for non-TUI dispatcher paths and renders the same
/// project/user skill list as plain text.
pub fn register_core_batch_7(reg: &mut CommandRegistry) {
    use crate::SkillsHandler;

    reg.register_builtin_handler(Arc::new(SkillsHandler::new()));
}

/// Register headless fallback handlers for commands whose local implementation
/// requires the interactive TUI surface.
pub fn register_interactive_only_commands(reg: &mut CommandRegistry) {
    use crate::InteractiveOnlyHandler;
    use command_api::builtin_support::core_description;

    for name in [
        "add-dir",
        "branch",
        "color",
        "copy",
        "diff",
        "plan",
        "plugin",
        "privacy-settings",
        "rename",
        "rewind",
        "tasks",
        "terminal-setup",
        "theme",
        "usage",
    ] {
        reg.register_builtin_handler(Arc::new(InteractiveOnlyHandler::new(
            name,
            core_description(name),
        )));
    }

    // claude-code parity (#61): resolution aliases for the interactive-only
    // commands whose target stub is registered above (byte-verified alias sets).
    // Like continue→resume these are NOT builtin names (no separate palette row).
    reg.register_alias("checkpoint".to_string(), "rewind".to_string());
    reg.register_alias("undo".to_string(), "rewind".to_string());
    reg.register_alias("name".to_string(), "rename".to_string());
    reg.register_alias("plugins".to_string(), "plugin".to_string());
    reg.register_alias("marketplace".to_string(), "plugin".to_string());
    reg.register_alias("bashes".to_string(), "tasks".to_string());

    // claude-code parity (#61/#66): `/usage` carries aliases ["cost","stats"]
    // (BIN v2.1.183: `name:"usage",aliases:["cost","stats"]`). `cost`/`stats`
    // are NOT standalone commands upstream (0 `name:"cost"`/`name:"stats"`
    // objects); LingXi removed them as builtins and routes them here as
    // resolution-only aliases of the `usage` command (registered above, and
    // overwritten with the real UsageHandler by `register_core_batch_5`).
    reg.register_alias("cost".to_string(), "usage".to_string());
    reg.register_alias("stats".to_string(), "usage".to_string());

    // claude-code parity (#61): `/desktop` aliases ["app"], `/mobile` aliases
    // ["ios","android"], `/session` aliases ["remote"] — all three target
    // commands ARE LingXi builtins (registered as pass-1 stubs in
    // `register_all_builtin_commands`), so the aliases resolve.
    reg.register_alias("app".to_string(), "desktop".to_string());
    reg.register_alias("ios".to_string(), "mobile".to_string());
    reg.register_alias("android".to_string(), "mobile".to_string());
    reg.register_alias("remote".to_string(), "session".to_string());
}

#[cfg(test)]
mod registry_tests {
    use super::*;
    use command_api::builtin_support::BUILTIN_COMMAND_NAMES;
    use command_api::model::CommandResult;

    #[test]
    fn register_all_registers_exactly_94_names() {
        let mut reg = CommandRegistry::new();
        register_all_builtin_commands(&mut reg);
        assert_eq!(BUILTIN_COMMAND_NAMES.len(), 94);
        for name in BUILTIN_COMMAND_NAMES {
            assert!(
                reg.resolve(name).is_some(),
                "expected command /{name} registered"
            );
        }
    }

    /// claude-code parity (#61/#66): `/cost` and `/stats` resolve to `/usage`,
    /// and `/desktop`/`/mobile`/`/session` aliases resolve to their targets.
    /// Also asserts cost/stats/vim/pr-comments/output-style are NOT standalone
    /// builtins anymore (removed per #66/#67).
    #[test]
    fn usage_cost_stats_and_platform_aliases_resolve() {
        let mut reg = CommandRegistry::new();
        register_all_builtin_commands(&mut reg);
        for (alias, target) in [
            ("cost", "usage"),
            ("stats", "usage"),
            ("app", "desktop"),
            ("ios", "mobile"),
            ("android", "mobile"),
            ("remote", "session"),
        ] {
            let cmd = reg
                .resolve(alias)
                .unwrap_or_else(|| panic!("alias /{alias} must resolve"));
            assert_eq!(cmd.name, target, "/{alias} should resolve to /{target}");
        }
        // Removed standalone commands are gone from the builtin name surface.
        for removed in ["vim", "pr-comments", "output-style"] {
            assert!(
                !BUILTIN_COMMAND_NAMES.contains(&removed),
                "/{removed} should have been removed from BUILTIN_COMMAND_NAMES"
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

    #[test]
    fn interactive_only_aliases_resolve_to_targets() {
        // claude-code parity (#61): the rewind/rename/plugin/tasks alias sets,
        // registered by register_interactive_only_commands (called inside
        // register_all_builtin_commands).
        let mut reg = CommandRegistry::new();
        register_all_builtin_commands(&mut reg);
        for (alias, target) in [
            ("checkpoint", "rewind"),
            ("undo", "rewind"),
            ("name", "rename"),
            ("plugins", "plugin"),
            ("marketplace", "plugin"),
            ("bashes", "tasks"),
        ] {
            let cmd = reg
                .resolve(alias)
                .unwrap_or_else(|| panic!("alias /{alias} must resolve"));
            assert_eq!(cmd.name, target, "/{alias} should resolve to /{target}");
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

        let h = reg
            .get_handler("stickers")
            .expect("stickers handler missing");
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
            "release-notes",
            "review",
            "security-review",
            "statusline",
            "stickers",
        ] {
            assert!(reg.resolve(name).is_some(), "/{name} missing");
            assert!(reg.get_handler(name).is_some(), "/{name} handler missing");
        }
    }

    /// The handle-free batch-3 `release-notes` `Done` command must NOT return
    /// the locked M5 stub literal after the standard registration call.
    /// (`output-style` was removed upstream — slash parity #67.)
    #[tokio::test]
    async fn release_notes_returns_done_not_stub() {
        let mut reg = CommandRegistry::new();
        register_all_builtin_commands(&mut reg);

        for name in ["release-notes"] {
            let h = reg
                .get_handler(name)
                .unwrap_or_else(|| panic!("/{name} handler missing"));
            match h.handle(&args(name)).await {
                CommandResult::Done { display: Some(s) } => {
                    assert_ne!(
                        s,
                        format!("{name}: not implemented in v0.6.0 (M5)"),
                        "/{name} still returns the locked stub literal"
                    );
                }
                other => panic!("/{name} expected Done with display, got {other:?}"),
            }
        }
    }
}

#[cfg(test)]
mod batch_5_tests {
    use super::*;
    use command_api::model::CommandResult;
    use command_api::parser::ParsedSlashCommand;
    use orchestrator::test_support::MockOrchestratorHandle;

    fn args(name: &str) -> ParsedSlashCommand {
        ParsedSlashCommand {
            name: name.to_string(),
            raw_args: String::new(),
            positional_args: vec![],
        }
    }

    #[test]
    fn batch_5_name_resolves_after_overwrite() {
        let mut reg = CommandRegistry::new();
        register_all_builtin_commands(&mut reg);
        let handle = Arc::new(MockOrchestratorHandle::new());
        register_core_batch_5(&mut reg, handle);

        assert!(reg.resolve("effort").is_some(), "/effort missing");
        assert!(
            reg.get_handler("effort").is_some(),
            "/effort handler missing"
        );
        assert!(reg.resolve("usage").is_some(), "/usage missing");
        assert!(reg.get_handler("usage").is_some(), "/usage handler missing");
    }

    /// After batch-5 wiring `/effort` must NOT return the locked M5 stub
    /// literal — it returns its real `Done` display.
    #[tokio::test]
    async fn effort_returns_real_display_not_stub() {
        let mut reg = CommandRegistry::new();
        register_all_builtin_commands(&mut reg);
        let handle = Arc::new(MockOrchestratorHandle::new());
        register_core_batch_5(&mut reg, handle);

        let h = reg.get_handler("effort").expect("effort handler missing");
        match h.handle(&args("effort")).await {
            CommandResult::Done { display: Some(s) } => {
                assert_ne!(
                    s, "effort: not implemented in v0.6.0 (M5)",
                    "/effort still returns the locked stub literal"
                );
            }
            other => panic!("/effort expected Done with display, got {other:?}"),
        }
    }

    /// After batch-5 wiring `/usage` must render a real usage snapshot rather
    /// than the interactive-only fallback.
    #[tokio::test]
    async fn usage_returns_real_display_not_interactive_only() {
        let mut reg = CommandRegistry::new();
        register_all_builtin_commands(&mut reg);
        let handle = Arc::new(MockOrchestratorHandle::new());
        register_core_batch_5(&mut reg, handle);

        let h = reg.get_handler("usage").expect("usage handler missing");
        match h.handle(&args("usage")).await {
            CommandResult::Done { display: Some(s) } => {
                assert!(s.starts_with("Usage\nTotal cost: $"));
                assert!(!s.contains("interactive TUI mode"));
                assert_ne!(s, "usage: not implemented in v0.6.0 (M5)");
            }
            other => panic!("/usage expected Done with display, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod batch_6_tests {
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

    #[test]
    fn keybindings_resolves_with_handler_after_register_all() {
        let mut reg = CommandRegistry::new();
        register_all_builtin_commands(&mut reg);
        assert!(reg.resolve("keybindings").is_some(), "/keybindings missing");
        assert!(
            reg.get_handler("keybindings").is_some(),
            "/keybindings handler missing"
        );
    }

    /// After `register_all_builtin_commands` (default gate = disabled),
    /// `/keybindings` must NOT return the locked M5 stub literal — it returns
    /// the real "currently in preview" display.
    #[tokio::test]
    async fn keybindings_returns_preview_not_stub() {
        let mut reg = CommandRegistry::new();
        register_all_builtin_commands(&mut reg);

        let h = reg
            .get_handler("keybindings")
            .expect("keybindings handler missing");
        match h.handle(&args("keybindings")).await {
            CommandResult::Done { display: Some(s) } => {
                assert_ne!(
                    s, "keybindings: not implemented in v0.6.0 (M5)",
                    "/keybindings still returns the locked stub literal"
                );
                assert_eq!(s, crate::keybindings::KEYBINDINGS_PREVIEW_DISABLED);
            }
            other => panic!("/keybindings expected Done with display, got {other:?}"),
        }
    }

    /// Re-calling batch-6 with `enabled = true` overwrites the entry in-place;
    /// dispatch then leaves the preview branch (the create/open path requires
    /// the real editor, so we only assert it is no longer the preview string is
    /// covered by the handler's own unit tests — here we just confirm the
    /// in-place overwrite resolves a handler).
    #[test]
    fn batch_6_overwrite_is_idempotent_and_resolves() {
        let mut reg = CommandRegistry::new();
        register_all_builtin_commands(&mut reg);
        register_core_batch_6(&mut reg, true);
        register_core_batch_6(&mut reg, true); // twice
        assert!(reg.get_handler("keybindings").is_some());
    }
}

#[cfg(test)]
mod batch_4_tests {
    use super::*;
    use command_api::model::CommandResult;
    use command_api::parser::ParsedSlashCommand;
    use orchestrator::test_support::MockOrchestratorHandle;

    fn args(name: &str) -> ParsedSlashCommand {
        ParsedSlashCommand {
            name: name.to_string(),
            raw_args: String::new(),
            positional_args: vec![],
        }
    }

    #[test]
    fn all_4_batch_4_names_resolve_after_overwrite() {
        let mut reg = CommandRegistry::new();
        register_all_builtin_commands(&mut reg);
        let handle = Arc::new(MockOrchestratorHandle::new());
        register_core_batch_4(&mut reg, handle);

        for name in ["context", "export", "files", "resume"] {
            assert!(reg.resolve(name).is_some(), "/{name} missing");
            assert!(reg.get_handler(name).is_some(), "/{name} handler missing");
        }
    }

    /// After batch-4 wiring the names must NOT return the locked M5 stub
    /// literal — they return their real `Done` displays. `/export` is covered
    /// by its own test (it performs file I/O against a temp dir) and is
    /// excluded here to keep this assertion side-effect-free.
    #[tokio::test]
    async fn batch_4_commands_return_real_display_not_stub() {
        let mut reg = CommandRegistry::new();
        register_all_builtin_commands(&mut reg);
        let handle = Arc::new(MockOrchestratorHandle::new());
        register_core_batch_4(&mut reg, handle);

        for name in ["context", "files", "resume"] {
            let h = reg
                .get_handler(name)
                .unwrap_or_else(|| panic!("/{name} handler missing"));
            match h.handle(&args(name)).await {
                CommandResult::Done { display: Some(s) } => {
                    assert_ne!(
                        s,
                        format!("{name}: not implemented in v0.6.0 (M5)"),
                        "/{name} still returns the locked stub literal"
                    );
                }
                other => panic!("/{name} expected Done with display, got {other:?}"),
            }
        }
    }

    /// `/resume` against the default mock (no on-disk store) renders the
    /// locked "none found" notice rather than the stub.
    #[tokio::test]
    async fn resume_renders_none_found_with_default_mock() {
        let mut reg = CommandRegistry::new();
        register_all_builtin_commands(&mut reg);
        let handle = Arc::new(MockOrchestratorHandle::new());
        register_core_batch_4(&mut reg, handle);

        let h = reg.get_handler("resume").expect("resume handler missing");
        match h.handle(&args("resume")).await {
            CommandResult::Done { display: Some(s) } => {
                assert_eq!(s, "No resumable sessions found.");
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    /// claude-code parity: `/resume` carries the `continue` alias
    /// (`src/commands/resume/index.ts`). After batch-4 wiring the alias must
    /// resolve to the `resume` command entry.
    #[test]
    fn continue_alias_resolves_to_resume_command() {
        let mut reg = CommandRegistry::new();
        register_all_builtin_commands(&mut reg);
        let handle = Arc::new(MockOrchestratorHandle::new());
        register_core_batch_4(&mut reg, handle);

        let cmd = reg
            .resolve("continue")
            .expect("continue alias should resolve");
        assert_eq!(cmd.name, "resume");
    }

    /// The `continue` alias must also dispatch to the resume *handler* (proves
    /// the alias-aware `CommandRegistry::get_handler`): looking it up under the
    /// alias returns the resume handler, which renders the same "none found"
    /// notice as `/resume` against the default mock.
    #[tokio::test]
    async fn continue_alias_dispatches_to_resume_handler() {
        let mut reg = CommandRegistry::new();
        register_all_builtin_commands(&mut reg);
        let handle = Arc::new(MockOrchestratorHandle::new());
        register_core_batch_4(&mut reg, handle);

        let h = reg
            .get_handler("continue")
            .expect("continue alias should map to the resume handler");
        match h.handle(&args("continue")).await {
            CommandResult::Done { display: Some(s) } => {
                assert_eq!(s, "No resumable sessions found.");
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod connect_tests {
    use super::*;
    use crate::connect::{
        ChatGptConnectDriver, ConnectCredentialWriter, ConnectError, CopilotConnectDriver,
        CopilotConnectStep,
    };
    use async_trait::async_trait;

    struct NoopWriter;
    #[async_trait]
    impl ConnectCredentialWriter for NoopWriter {
        async fn prompt_and_store_key(&self, _id: &str) -> Result<(), ConnectError> {
            Ok(())
        }
    }
    struct NoopCopilot;
    #[async_trait]
    impl CopilotConnectDriver for NoopCopilot {
        async fn begin(&self, _domain: Option<&str>) -> Result<CopilotConnectStep, ConnectError> {
            Ok(CopilotConnectStep {
                user_code: "X".into(),
                verification_uri: "https://github.com/login/device".into(),
            })
        }
        async fn poll_to_completion(&self, _s: &CopilotConnectStep) -> Result<(), ConnectError> {
            Ok(())
        }
    }
    struct NoopChatGpt;
    #[async_trait]
    impl ChatGptConnectDriver for NoopChatGpt {
        async fn connect(&self) -> Result<String, ConnectError> {
            Ok("Connected chatgpt.".into())
        }
    }

    #[test]
    fn connect_resolves_after_registration() {
        let mut reg = CommandRegistry::new();
        register_all_builtin_commands(&mut reg);
        register_core_connect(
            &mut reg,
            Arc::new(NoopWriter),
            Arc::new(NoopCopilot),
            Arc::new(NoopChatGpt),
        );
        assert!(reg.resolve("connect").is_some(), "/connect missing");
        assert!(
            reg.get_handler("connect").is_some(),
            "/connect handler missing"
        );
    }
}

#[cfg(test)]
mod interactive_only_tests {
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

    #[tokio::test]
    async fn local_tui_commands_return_interactive_only_not_m5_stub() {
        let mut reg = CommandRegistry::new();
        register_all_builtin_commands(&mut reg);

        for name in [
            "add-dir",
            "branch",
            "color",
            "copy",
            "diff",
            "plan",
            "plugin",
            "privacy-settings",
            "rename",
            "rewind",
            "tasks",
            "terminal-setup",
            "theme",
            "usage",
        ] {
            let h = reg
                .get_handler(name)
                .unwrap_or_else(|| panic!("/{name} handler missing"));
            match h.handle(&args(name)).await {
                CommandResult::Done { display: Some(s) } => {
                    assert_ne!(s, format!("{name}: not implemented in v0.6.0 (M5)"));
                    assert!(s.contains("interactive TUI mode"), "/{name}: {s}");
                }
                other => panic!("/{name} expected Done with display, got {other:?}"),
            }
        }
    }
}

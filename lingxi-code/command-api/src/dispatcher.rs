//! Implementation of [`traits::SlashCommandDispatcher`] that routes
//! `/<name> <args>` into the in-crate [`CommandRegistry`].
//!
//! See plan `docs/superpowers/plans/2026-05-25-m5-09-commands-surface.md` Task 5.

use crate::expand::{expand_markdown_command, ExpandCtx};
use crate::model::{CommandResult, CommandSource, SlashCommandKind};
use crate::parser::parse_slash_command;
use crate::registry::CommandRegistry;
use crate::shell_expansion::{
    ShellExpansionCtx, ShellOut, ShellPermissionDecision, ShellPermissionGate, ShellRunError,
    ShellRunner,
};
use async_trait::async_trait;
use hooks::events::{HookEvent, PromptExpansionType};
use hooks::registry::HookContext;
use hooks::HookExecutorImpl;
use std::sync::Arc;
use tokio::sync::RwLock;
use traits::{SlashCommandDispatcher, SlashDispatchResult};

/// Supplies the per-dispatch [`HookContext`] (session id, cwd, transcript path,
/// permission mode) for the `UserPromptExpansion` hook. The dispatcher does not
/// own the session/engine state, so the wiring layer (which holds the
/// orchestrator) injects this provider. Mirrors claude-code's `b$t`, where the
/// base hook input is built from the tool-use context's `getAppState`
/// (minified `vd`).
///
/// Async because the orchestrator reads its `session_id` behind a lock
/// (`ConversationOrchestrator::expansion_hook_context`); the closure is invoked
/// only on a real markdown/MCP-prompt expansion, so the await is off the hot
/// path.
pub type ExpansionHookContextProvider = Arc<
    dyn Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = HookContext> + Send>>
        + Send
        + Sync,
>;

/// The `UserPromptExpansion` firing dependencies threaded into a
/// [`RegistrySlashDispatcher`]: the engine's hook executor plus the
/// context-provider. Both are present together or the feature is off.
struct ExpansionHooks {
    executor: Arc<HookExecutorImpl>,
    context: ExpansionHookContextProvider,
}

/// Map LingXi's [`CommandSource`] to the lowercase `command_source` string
/// claude-code threads into the `UserPromptExpansion` payload (`r = e.source`,
/// values `"builtin" | "user" | "project" | "local" | "plugin" | "mcp"`; the
/// admin-managed tier is reported as `"managed"`). This is the per-command
/// provenance string, distinct from the PascalCase serde encoding of the enum.
fn command_source_str(source: CommandSource) -> &'static str {
    match source {
        CommandSource::Builtin => "builtin",
        CommandSource::User => "user",
        CommandSource::Project => "project",
        CommandSource::Local => "local",
        CommandSource::Plugin => "plugin",
        CommandSource::Managed => "managed",
        CommandSource::Mcp => "mcp",
        // Programmatic bundled skills (TS `source: 'bundled'`).
        CommandSource::Bundled => "bundled",
    }
}

struct UnavailableShellRunner;

#[async_trait]
impl ShellRunner for UnavailableShellRunner {
    async fn run(
        &self,
        _command: &str,
        _shell: Option<crate::FrontmatterShell>,
    ) -> Result<ShellOut, ShellRunError> {
        Err(ShellRunError {
            stdout: String::new(),
            stderr: String::new(),
            interrupted: false,
            generic_message: Some("shell expansion is unavailable in slash dispatcher".into()),
        })
    }
}

struct DenyShellPermissionGate;

impl ShellPermissionGate for DenyShellPermissionGate {
    fn check(
        &self,
        _command: &str,
        _shell: Option<crate::FrontmatterShell>,
    ) -> ShellPermissionDecision {
        ShellPermissionDecision::Deny {
            message: Some("shell expansion is unavailable in slash dispatcher".into()),
        }
    }
}

/// Concrete `SlashCommandDispatcher` backed by an `Arc<RwLock<CommandRegistry>>`.
///
/// The registry is wrapped in an `RwLock` because plugin lifecycle events
/// ([`CommandRegistry::register_plugin_commands`] /
/// [`CommandRegistry::unregister_plugin`]) need exclusive write access at
/// runtime. Dispatching only takes a read lock.
pub struct RegistrySlashDispatcher {
    registry: Arc<RwLock<CommandRegistry>>,
    /// `UserPromptExpansion` firing deps (#39). `None` keeps the dispatcher a
    /// strict no-op for that event — the default for every existing caller and
    /// for hosts without a hook-capable engine. Wired by the composition root
    /// via [`Self::with_expansion_hooks`] once the orchestrator's executor
    /// exists.
    expansion_hooks: Option<ExpansionHooks>,
}

impl RegistrySlashDispatcher {
    /// Construct a dispatcher backed by the given shared registry.
    #[must_use]
    pub fn new(registry: Arc<RwLock<CommandRegistry>>) -> Self {
        Self {
            registry,
            expansion_hooks: None,
        }
    }

    /// Wire the `UserPromptExpansion` hook (#39): the engine's hook executor
    /// plus a [`HookContext`] provider. After this, expanding a markdown /
    /// MCP-prompt slash command fires the `UserPromptExpansion` event with the
    /// command metadata, faithful to claude-code `WFa`→`b$t` (BIN off
    /// 201270310). A no-op for handler/builtin commands (which are never
    /// "expanded") and a strict no-op when no `UserPromptExpansion` hook is
    /// registered (the executor itself no-ops on an empty match set).
    #[must_use]
    pub fn with_expansion_hooks(
        mut self,
        executor: Arc<HookExecutorImpl>,
        context: ExpansionHookContextProvider,
    ) -> Self {
        self.expansion_hooks = Some(ExpansionHooks { executor, context });
        self
    }

    /// Fire the `UserPromptExpansion` hook for a just-expanded markdown /
    /// MCP-prompt slash command, matching claude-code `WFa`'s firing site
    /// (after building the `/name args` prompt line, before the expanded
    /// prompt is queried). Strict no-op when no expansion hook is wired. The
    /// aggregate is discarded — best-effort, so a failing / blocking
    /// `UserPromptExpansion` hook never breaks dispatch.
    async fn fire_user_prompt_expansion(&self, name: &str, raw_args: &str, source: CommandSource) {
        let Some(eh) = self.expansion_hooks.as_ref() else {
            return;
        };
        // claude-code `o = t ? `/${e.name} ${t}` : `/${e.name}``.
        let prompt = if raw_args.is_empty() {
            format!("/{name}")
        } else {
            format!("/{name} {raw_args}")
        };
        // claude-code `e.source==="mcp" ? "mcp_prompt" : "slash_command"`.
        let expansion_type = if matches!(source, CommandSource::Mcp) {
            PromptExpansionType::McpPrompt
        } else {
            PromptExpansionType::SlashCommand
        };
        let ctx = (eh.context)().await;
        let _ = eh
            .executor
            .execute(
                HookEvent::UserPromptExpansion {
                    expansion_type,
                    command_name: name.to_string(),
                    command_args: raw_args.to_string(),
                    command_source: Some(command_source_str(source).to_string()),
                    prompt,
                },
                ctx,
            )
            .await;
    }

    /// The shared registry handle backing this dispatcher.
    ///
    /// (ARGS.3) Hands the same `Arc<RwLock<CommandRegistry>>` out so a host can
    /// read declared `argNames` once at init (e.g. the TUI's progressive
    /// argument-hint map) without owning a second dispatcher. Cloning the `Arc`
    /// keeps both views pointed at the SAME registry, so later
    /// register/unregister events are visible through either handle.
    #[must_use]
    pub fn registry(&self) -> Arc<RwLock<CommandRegistry>> {
        self.registry.clone()
    }

    /// A second dispatcher pointing at the SAME shared registry.
    ///
    /// Dispatching is identical (both clone the same `Arc<RwLock<CommandRegistry>>`).
    /// Used when a host keeps the original dispatcher (e.g. by value on a runtime
    /// struct) but a second consumer — like the bridge-server's command router —
    /// needs an owned `RegistrySlashDispatcher`/`Arc<dyn SlashCommandDispatcher>`
    /// over the same command set.
    #[must_use]
    pub fn clone_shared(&self) -> Self {
        Self {
            registry: self.registry.clone(),
            // The `UserPromptExpansion` wiring (executor + context provider) is
            // cheap to clone (two `Arc`s) and must travel with the shared
            // dispatcher so every consumer fires the event identically.
            expansion_hooks: self.expansion_hooks.as_ref().map(|eh| ExpansionHooks {
                executor: eh.executor.clone(),
                context: eh.context.clone(),
            }),
        }
    }

    /// Format the locked unknown-command literal.
    ///
    /// Public so callers can render the same string outside the dispatch loop
    /// (e.g. when reporting an error from a `/help` lookup that finds a
    /// dangling alias).
    #[must_use]
    pub fn unknown_command_literal(name: &str) -> String {
        format!("Unknown command: /{name}")
    }
}

#[async_trait]
impl SlashCommandDispatcher for RegistrySlashDispatcher {
    async fn dispatch(&self, raw: &str) -> SlashDispatchResult {
        // 1. Detect slash prefix.
        if !raw.starts_with('/') {
            return SlashDispatchResult::NotASlashCommand;
        }

        // 2. "/" alone — parse returns Some(name=""), so we still get an
        //    Unknown branch below. But we surface it explicitly so the test
        //    is unambiguous.
        if raw == "/" {
            return SlashDispatchResult::Unknown {
                name: String::new(),
                display: Self::unknown_command_literal(""),
            };
        }

        // 3. Parse into name + args via the existing M1.15 parser.
        //    parse_slash_command expects input WITH leading '/' and returns
        //    Option<ParsedSlashCommand>.
        let Some(parsed) = parse_slash_command(raw) else {
            // parse_slash_command only returns None when the input does not
            // start with '/' — already handled above. Belt-and-braces:
            return SlashDispatchResult::NotASlashCommand;
        };

        // 4. Look up the command.
        let reg = self.registry.read().await;
        let Some(command) = reg.resolve(&parsed.name).cloned() else {
            return SlashDispatchResult::Unknown {
                name: parsed.name.clone(),
                display: Self::unknown_command_literal(&parsed.name),
            };
        };

        // 4a. Honor the `DISABLE_*_COMMAND` env gates for the affected builtins
        //     (claude-code `isEnabled:()=>!je.DISABLE_X`). A disabled command is
        //     dropped from findCommand's search, so it must NOT resolve — fall
        //     through to the Unknown branch. Keyed by the command's canonical
        //     name so a disabled command is unreachable via an alias too.
        if matches!(command.kind, SlashCommandKind::Builtin { .. })
            && crate::builtin_support::names::is_command_env_disabled(&command.name)
        {
            return SlashDispatchResult::Unknown {
                name: parsed.name.clone(),
                display: Self::unknown_command_literal(&parsed.name),
            };
        }

        if matches!(
            command.kind,
            SlashCommandKind::Markdown { .. } | SlashCommandKind::Plugin { .. }
        ) {
            drop(reg);
            let shell_ctx = ShellExpansionCtx {
                runner: Arc::new(UnavailableShellRunner),
                permission_gate: Arc::new(DenyShellPermissionGate),
            };
            let expand_ctx = ExpandCtx {
                session_id: "",
                shell: &shell_ctx,
            };
            return match expand_markdown_command(&command, &parsed, &expand_ctx).await {
                Ok(content) => {
                    // hooks #39: a markdown / MCP-prompt slash command WAS
                    // expanded — fire `UserPromptExpansion` (claude-code `WFa`
                    // →`b$t`) with the command metadata, at the same point the
                    // binary does: after the `/name args` line is formed and
                    // the body resolved, before the expanded prompt is queried.
                    self.fire_user_prompt_expansion(
                        &command.name,
                        &parsed.raw_args,
                        command.source,
                    )
                    .await;
                    SlashDispatchResult::Handled { display: content }
                }
                Err(e) => SlashDispatchResult::Handled {
                    display: format!("{} expansion failed: {e}", command.name),
                },
            };
        }

        let Some(handler) = reg.get_handler(&parsed.name) else {
            return SlashDispatchResult::Unknown {
                name: parsed.name.clone(),
                display: Self::unknown_command_literal(&parsed.name),
            };
        };

        // 5. Drop the registry lock before awaiting handler (handler may
        //    re-lock the registry or take a while).
        drop(reg);
        let result = handler.handle(&parsed).await;

        match result {
            // M5-09 placeholders only ever return Done — but route the
            // other variants safely anyway so M5-10/M5-11 can extend.
            CommandResult::Done { display } | CommandResult::EmitEffects { display, .. } => {
                SlashDispatchResult::Handled {
                    display: display.unwrap_or_default(),
                }
            }
            CommandResult::InjectMessage { content } => {
                SlashDispatchResult::Handled { display: content }
            }
            CommandResult::RequestConfirmation { prompt, .. } => {
                SlashDispatchResult::Handled { display: prompt }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin_support::names::{core_description, BUILTIN_COMMAND_NAMES};
    use crate::builtin_support::unimplemented::UnimplementedCommandHandler;
    use crate::model::{CommandFrontmatter, CommandSource, SlashCommand, SlashCommandKind};
    use crate::registry::CommandRegistry;
    use std::path::PathBuf;

    // M8-P9: seed directly from the api-side scaffolding (the real
    // `register_all_builtin_commands` lives in `command-core`, which depends on
    // this crate — using it here would cycle). Registering every name as an
    // unimplemented stub is behaviourally identical for dispatch tests: the 18
    // core placeholders return the same locked literal as the stub.
    fn seeded_dispatcher() -> RegistrySlashDispatcher {
        let mut reg = CommandRegistry::new();
        for &name in BUILTIN_COMMAND_NAMES {
            reg.register_builtin_handler(Arc::new(UnimplementedCommandHandler::new(
                name,
                core_description(name),
            )));
        }
        RegistrySlashDispatcher::new(Arc::new(RwLock::new(reg)))
    }

    /// Like [`seeded_dispatcher`] but also wires the `continue` → `resume` alias
    /// (registered by `register_core_batch_4` in `command-core`, which can't be
    /// used here without a dependency cycle). Used to prove the alias survives
    /// the real dispatch path end-to-end.
    fn seeded_dispatcher_with_resume_alias() -> RegistrySlashDispatcher {
        let mut reg = CommandRegistry::new();
        for &name in BUILTIN_COMMAND_NAMES {
            reg.register_builtin_handler(Arc::new(UnimplementedCommandHandler::new(
                name,
                core_description(name),
            )));
        }
        reg.register_alias("continue".to_string(), "resume".to_string());
        RegistrySlashDispatcher::new(Arc::new(RwLock::new(reg)))
    }

    #[tokio::test]
    async fn dispatches_known_command_to_locked_stub() {
        let d = seeded_dispatcher();
        let result = d.dispatch("/ant-trace").await;
        match result {
            SlashDispatchResult::Handled { display } => {
                assert_eq!(display, "ant-trace: not implemented in v0.6.0 (M5)");
            }
            other => panic!("expected Handled, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatches_known_command_with_args_to_locked_stub() {
        let d = seeded_dispatcher();
        let result = d.dispatch("/clear --force").await;
        match result {
            SlashDispatchResult::Handled { display } => {
                assert_eq!(display, "clear: not implemented in v0.6.0 (M5)");
            }
            other => panic!("expected Handled, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatches_markdown_command_via_registry_resolve() {
        let mut reg = CommandRegistry::new();
        reg.register_command(SlashCommand {
            name: "demo".to_string(),
            description: "Demo".to_string(),
            source: CommandSource::Project,
            kind: SlashCommandKind::Markdown {
                file_path: PathBuf::from("/tmp/demo.md"),
                frontmatter: CommandFrontmatter::default(),
                prompt_template: "Use $ARGUMENTS".to_string(),
            },
            ..SlashCommand::default()
        });
        let d = RegistrySlashDispatcher::new(Arc::new(RwLock::new(reg)));
        match d.dispatch("/demo this").await {
            SlashDispatchResult::Handled { display } => assert_eq!(display, "Use this"),
            other => panic!("expected markdown command to dispatch, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn markdown_command_shadows_same_named_builtin_in_dispatch() {
        let mut reg = CommandRegistry::new();
        reg.register_builtin_handler(Arc::new(UnimplementedCommandHandler::new(
            "commit",
            core_description("commit"),
        )));
        reg.register_command(SlashCommand {
            name: "commit".to_string(),
            description: "Custom commit".to_string(),
            source: CommandSource::Project,
            kind: SlashCommandKind::Markdown {
                file_path: PathBuf::from("/tmp/commit.md"),
                frontmatter: CommandFrontmatter::default(),
                prompt_template: "Custom $ARGUMENTS".to_string(),
            },
            ..SlashCommand::default()
        });
        let d = RegistrySlashDispatcher::new(Arc::new(RwLock::new(reg)));
        match d.dispatch("/commit now").await {
            SlashDispatchResult::Handled { display } => assert_eq!(display, "Custom now"),
            other => panic!("expected custom markdown dispatch, got {other:?}"),
        }
    }

    /// End-to-end guard: the `continue` alias survives the real dispatch path.
    /// `/continue` must route through the alias-aware `get_handler` to the
    /// `resume` handler — yielding `Handled`, not `Unknown`. (Mirrors
    /// claude-code's `aliases: ['continue']` on `/resume`.)
    #[tokio::test]
    async fn dispatches_continue_alias_to_resume() {
        let d = seeded_dispatcher_with_resume_alias();
        let result = d.dispatch("/continue").await;
        match result {
            SlashDispatchResult::Handled { display } => {
                // Seeded with the stub handler under "resume", so the alias
                // routes there and we get the resume stub literal back.
                assert_eq!(display, "resume: not implemented in v0.6.0 (M5)");
            }
            other => panic!("expected Handled for /continue, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unknown_command_returns_locked_literal() {
        let d = seeded_dispatcher();
        let result = d.dispatch("/notacommand").await;
        match result {
            SlashDispatchResult::Unknown { name, display } => {
                assert_eq!(name, "notacommand");
                assert_eq!(display, "Unknown command: /notacommand");
            }
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    /// #63: a builtin whose `DISABLE_*_COMMAND` env gate is tripped does NOT
    /// resolve — it falls through to the Unknown branch, mirroring claude-code's
    /// `isEnabled:()=>!je.DISABLE_X` dropping it from findCommand.
    #[tokio::test]
    async fn env_disabled_builtin_does_not_resolve() {
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _g = ENV_LOCK.lock().unwrap();
        let var = "DISABLE_DOCTOR_COMMAND";
        let d = seeded_dispatcher();

        // Default: /doctor resolves to its (stub) handler → Handled.
        std::env::remove_var(var);
        assert!(matches!(
            d.dispatch("/doctor").await,
            SlashDispatchResult::Handled { .. }
        ));

        // Gate tripped: /doctor is Unknown.
        std::env::set_var(var, "1");
        match d.dispatch("/doctor").await {
            SlashDispatchResult::Unknown { name, display } => {
                assert_eq!(name, "doctor");
                assert_eq!(display, "Unknown command: /doctor");
            }
            other => panic!("expected Unknown for gated /doctor, got {other:?}"),
        }
        std::env::remove_var(var);
    }

    #[tokio::test]
    async fn non_slash_input_is_not_a_slash_command() {
        let d = seeded_dispatcher();
        let result = d.dispatch("hello world").await;
        assert!(matches!(result, SlashDispatchResult::NotASlashCommand));
    }

    #[tokio::test]
    async fn empty_input_is_not_a_slash_command() {
        let d = seeded_dispatcher();
        let result = d.dispatch("").await;
        assert!(matches!(result, SlashDispatchResult::NotASlashCommand));
    }

    #[tokio::test]
    async fn just_slash_is_not_a_command() {
        let d = seeded_dispatcher();
        let result = d.dispatch("/").await;
        // After stripping the leading '/', the name is empty — treat as unknown.
        match result {
            SlashDispatchResult::Unknown { name, display } => {
                assert_eq!(name, "");
                assert_eq!(display, "Unknown command: /");
            }
            other => panic!("expected Unknown for '/', got {other:?}"),
        }
    }

    #[tokio::test]
    async fn uppercase_name_is_unknown() {
        let d = seeded_dispatcher();
        // Per Task 0 step 4 L3 expansion: the parser does NOT case-fold; the
        // registry only holds lowercase keys.
        let result = d.dispatch("/CLEAR").await;
        match result {
            SlashDispatchResult::Unknown { name, display } => {
                assert_eq!(name, "CLEAR");
                assert_eq!(display, "Unknown command: /CLEAR");
            }
            other => panic!("expected Unknown for /CLEAR, got {other:?}"),
        }
    }

    /// (ARGS.3) `registry()` hands back an `Arc` to the SAME shared registry:
    /// a markdown command registered through the dispatcher's `Arc` is visible
    /// through the returned handle, proving the live argument-hint seam reads
    /// the dispatcher's actual command set.
    #[tokio::test]
    async fn registry_accessor_shares_the_same_registry() {
        use crate::model::{CommandSource, SlashCommand, SlashCommandKind};

        let shared = Arc::new(RwLock::new(CommandRegistry::new()));
        let d = RegistrySlashDispatcher::new(shared.clone());

        // Register a markdown command (with argNames) via the ORIGINAL Arc.
        shared.write().await.register_command(SlashCommand {
            name: "deploy".to_string(),
            description: "Deploy".to_string(),
            source: CommandSource::Project,
            kind: SlashCommandKind::Markdown {
                file_path: std::path::PathBuf::from("/x/deploy.md"),
                frontmatter: crate::model::CommandFrontmatter::default(),
                prompt_template: String::new(),
            },
            argument_names: vec!["env".to_string(), "region".to_string()],
            ..SlashCommand::default()
        });

        // The accessor's handle observes that same command + its argNames.
        let via_accessor = d.registry();
        let guard = via_accessor.read().await;
        let cmd = guard
            .resolve("deploy")
            .expect("command visible via registry()");
        assert_eq!(
            cmd.argument_names,
            vec!["env".to_string(), "region".to_string()]
        );
    }

    // ---- #39 UserPromptExpansion firing (R-O1) ----
    //
    // claude-code fires `UserPromptExpansion` (`WFa`→`b$t`, BIN off 201270310)
    // the moment a markdown / MCP-prompt slash command is expanded, with the
    // command metadata: `expansion_type` (`slash_command` | `mcp_prompt`),
    // `command_name`, `command_args` (the raw arg string), `command_source`
    // (the command's provenance), and `prompt` (the `/name args` line). These
    // tests prove the dispatcher fires that event at the markdown-expansion
    // site once an executor + context provider are wired, and is a strict
    // no-op otherwise.

    use async_trait::async_trait;
    use hooks::definition::{HookDefinition, HookExecutor as DefHookExecutor, HookSource};
    use hooks::events::HookEventType;
    use hooks::executor::BuiltinHookHandler;
    use hooks::response::{HookOutcome, HookResult};
    use hooks::HookRegistry;
    use protocol::{HookId, HttpRequest, HttpResponse};
    use std::time::Duration;

    struct UnusedHttp;
    #[async_trait]
    impl traits::HttpTransport for UnusedHttp {
        async fn request(&self, _req: HttpRequest) -> Result<HttpResponse, traits::HttpError> {
            Err(traits::HttpError::InvalidRequest("unused".into()))
        }
        async fn stream_sse(
            &self,
            _req: HttpRequest,
        ) -> Result<traits::http::SseStream, traits::HttpError> {
            Err(traits::HttpError::InvalidRequest("unused".into()))
        }
    }
    struct UnusedRuntime;
    #[async_trait]
    impl traits::RuntimeSpawner for UnusedRuntime {
        async fn spawn(
            &self,
            _name: &str,
            _task: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>>,
        ) -> Result<traits::BackgroundTaskHandle, traits::RuntimeError> {
            Err(traits::RuntimeError::Internal("unused".into()))
        }
        async fn sleep(&self, _d: Duration) {}
        async fn cancel(
            &self,
            _h: &traits::BackgroundTaskHandle,
        ) -> Result<(), traits::RuntimeError> {
            Ok(())
        }
    }

    /// One captured `UserPromptExpansion` payload.
    #[derive(Clone, Debug, PartialEq, Eq)]
    struct FiredExpansion {
        expansion_type: PromptExpansionType,
        command_name: String,
        command_args: String,
        command_source: Option<String>,
        prompt: String,
    }

    type ExpansionLog = std::sync::Mutex<Vec<FiredExpansion>>;

    struct RecordingExpansionHandler {
        log: Arc<ExpansionLog>,
    }
    #[async_trait]
    impl BuiltinHookHandler for RecordingExpansionHandler {
        fn id(&self) -> &str {
            "record-user-prompt-expansion"
        }
        async fn handle(&self, event: &HookEvent, _ctx: &HookContext) -> HookResult {
            if let HookEvent::UserPromptExpansion {
                expansion_type,
                command_name,
                command_args,
                command_source,
                prompt,
            } = event
            {
                self.log.lock().unwrap().push(FiredExpansion {
                    expansion_type: *expansion_type,
                    command_name: command_name.clone(),
                    command_args: command_args.clone(),
                    command_source: command_source.clone(),
                    prompt: prompt.clone(),
                });
            }
            HookResult {
                outcome: HookOutcome::Success,
                stdout: String::new(),
                stderr: String::new(),
                exit_code: None,
                response: None,
            }
        }
    }

    fn user_prompt_expansion_hook() -> HookDefinition {
        HookDefinition {
            id: HookId::new(),
            name: "record-user-prompt-expansion".into(),
            events: vec![HookEventType::UserPromptExpansion],
            if_condition: None,
            executor: DefHookExecutor::Builtin {
                handler_id: "record-user-prompt-expansion".into(),
            },
            source: HookSource::User,
            blocking: true,
            timeout: None,
            priority: 0,
            once: false,
            status_message: None,
        }
    }

    /// Build a `HookExecutorImpl` carrying a registered `UserPromptExpansion`
    /// hook + the recording handler, returning it with the shared log.
    async fn recording_expansion_executor() -> (Arc<HookExecutorImpl>, Arc<ExpansionLog>) {
        let log: Arc<ExpansionLog> = Arc::new(std::sync::Mutex::new(Vec::new()));
        let registry = Arc::new(RwLock::new(HookRegistry::new()));
        registry.write().await.register(user_prompt_expansion_hook());
        let mut exec =
            HookExecutorImpl::new(registry, Arc::new(UnusedHttp), Arc::new(UnusedRuntime));
        exec.register_builtin(Arc::new(RecordingExpansionHandler { log: log.clone() }));
        (Arc::new(exec), log)
    }

    fn registry_with_demo_markdown(source: CommandSource) -> Arc<RwLock<CommandRegistry>> {
        use crate::model::{CommandFrontmatter, SlashCommand};
        let mut reg = CommandRegistry::new();
        reg.register_command(SlashCommand {
            name: "demo".to_string(),
            description: "Demo".to_string(),
            source,
            kind: SlashCommandKind::Markdown {
                file_path: PathBuf::from("/tmp/demo.md"),
                frontmatter: CommandFrontmatter::default(),
                prompt_template: "Use $ARGUMENTS".to_string(),
            },
            ..SlashCommand::default()
        });
        Arc::new(RwLock::new(reg))
    }

    /// A wired dispatcher fires `UserPromptExpansion` when it expands a markdown
    /// slash command, carrying the byte-faithful metadata (claude-code `WFa`).
    #[tokio::test]
    async fn expanding_a_markdown_command_fires_user_prompt_expansion() {
        let (exec, log) = recording_expansion_executor().await;
        let ctx_session = protocol::SessionId::new();
        let provider: ExpansionHookContextProvider = Arc::new(move || {
            Box::pin(async move {
                HookContext {
                    session_id: ctx_session,
                    ..Default::default()
                }
            })
        });
        let d = RegistrySlashDispatcher::new(registry_with_demo_markdown(CommandSource::Project))
            .with_expansion_hooks(exec, provider);

        // Expansion still produces the body, AND the hook fires.
        match d.dispatch("/demo this and that").await {
            SlashDispatchResult::Handled { display } => {
                assert_eq!(display, "Use this and that")
            }
            other => panic!("expected markdown expansion, got {other:?}"),
        }

        let seen = log.lock().unwrap().clone();
        assert_eq!(
            seen,
            vec![FiredExpansion {
                expansion_type: PromptExpansionType::SlashCommand,
                command_name: "demo".to_string(),
                command_args: "this and that".to_string(),
                command_source: Some("project".to_string()),
                // claude-code `o = `/${e.name} ${t}`` — the command LINE, not
                // the expanded body.
                prompt: "/demo this and that".to_string(),
            }],
            "UserPromptExpansion must fire once with byte-faithful metadata: {seen:?}"
        );
    }

    /// An MCP-prompt-sourced command reports `expansion_type: mcp_prompt` and
    /// `command_source: "mcp"` (claude-code `e.source==="mcp"?"mcp_prompt":…`).
    #[tokio::test]
    async fn expanding_an_mcp_prompt_command_reports_mcp_expansion_type() {
        let (exec, log) = recording_expansion_executor().await;
        let provider: ExpansionHookContextProvider =
            Arc::new(|| Box::pin(async { HookContext::default() }));
        let d = RegistrySlashDispatcher::new(registry_with_demo_markdown(CommandSource::Mcp))
            .with_expansion_hooks(exec, provider);

        let _ = d.dispatch("/demo arg").await;

        let seen = log.lock().unwrap().clone();
        assert_eq!(seen.len(), 1, "exactly one expansion: {seen:?}");
        assert_eq!(seen[0].expansion_type, PromptExpansionType::McpPrompt);
        assert_eq!(seen[0].command_source, Some("mcp".to_string()));
        assert_eq!(seen[0].prompt, "/demo arg");
    }

    /// A command with NO args reports an empty `command_args` and the bare
    /// `/name` prompt (claude-code `o = `/${e.name}`` when `t` is empty).
    #[tokio::test]
    async fn expansion_with_no_args_uses_bare_command_prompt() {
        let (exec, log) = recording_expansion_executor().await;
        let provider: ExpansionHookContextProvider =
            Arc::new(|| Box::pin(async { HookContext::default() }));
        let d = RegistrySlashDispatcher::new(registry_with_demo_markdown(CommandSource::User))
            .with_expansion_hooks(exec, provider);

        let _ = d.dispatch("/demo").await;

        let seen = log.lock().unwrap().clone();
        assert_eq!(seen.len(), 1, "exactly one expansion: {seen:?}");
        assert_eq!(seen[0].command_args, "");
        assert_eq!(seen[0].command_source, Some("user".to_string()));
        assert_eq!(seen[0].prompt, "/demo");
    }

    /// Without `with_expansion_hooks` wired, expanding a markdown command is a
    /// strict no-op for `UserPromptExpansion` — the default for every existing
    /// dispatcher caller. (The expansion itself still happens.)
    #[tokio::test]
    async fn unwired_dispatcher_does_not_fire_user_prompt_expansion() {
        let (exec, log) = recording_expansion_executor().await;
        // Note: executor exists but is NOT wired into the dispatcher.
        let _ = exec;
        let d = RegistrySlashDispatcher::new(registry_with_demo_markdown(CommandSource::Project));

        match d.dispatch("/demo x").await {
            SlashDispatchResult::Handled { display } => assert_eq!(display, "Use x"),
            other => panic!("expected markdown expansion, got {other:?}"),
        }

        assert!(
            log.lock().unwrap().is_empty(),
            "an unwired dispatcher must not fire UserPromptExpansion"
        );
    }

    /// A non-markdown (builtin handler) command never "expands", so it must NOT
    /// fire `UserPromptExpansion` even when the hook is wired — faithful to
    /// claude-code, where `b$t` fires only from the command-EXPANSION path.
    #[tokio::test]
    async fn builtin_command_does_not_fire_user_prompt_expansion() {
        let (exec, log) = recording_expansion_executor().await;
        let provider: ExpansionHookContextProvider =
            Arc::new(|| Box::pin(async { HookContext::default() }));

        let mut reg = CommandRegistry::new();
        reg.register_builtin_handler(Arc::new(UnimplementedCommandHandler::new(
            "clear",
            core_description("clear"),
        )));
        let d = RegistrySlashDispatcher::new(Arc::new(RwLock::new(reg)))
            .with_expansion_hooks(exec, provider);

        let _ = d.dispatch("/clear").await;

        assert!(
            log.lock().unwrap().is_empty(),
            "a builtin handler command must not fire UserPromptExpansion"
        );
    }
}

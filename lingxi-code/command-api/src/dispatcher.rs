//! Implementation of [`traits::SlashCommandDispatcher`] that routes
//! `/<name> <args>` into the in-crate [`CommandRegistry`].
//!
//! See plan `docs/superpowers/plans/2026-05-25-m5-09-commands-surface.md` Task 5.

use crate::model::CommandResult;
use crate::parser::parse_slash_command;
use crate::registry::CommandRegistry;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::RwLock;
use traits::{SlashCommandDispatcher, SlashDispatchResult};

/// Concrete `SlashCommandDispatcher` backed by an `Arc<RwLock<CommandRegistry>>`.
///
/// The registry is wrapped in an `RwLock` because plugin lifecycle events
/// ([`CommandRegistry::register_plugin_commands`] /
/// [`CommandRegistry::unregister_plugin`]) need exclusive write access at
/// runtime. Dispatching only takes a read lock.
pub struct RegistrySlashDispatcher {
    registry: Arc<RwLock<CommandRegistry>>,
}

impl RegistrySlashDispatcher {
    /// Construct a dispatcher backed by the given shared registry.
    #[must_use]
    pub fn new(registry: Arc<RwLock<CommandRegistry>>) -> Self {
        Self { registry }
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

        // 4. Look up the handler.
        let reg = self.registry.read().await;
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
    use crate::registry::CommandRegistry;

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
}

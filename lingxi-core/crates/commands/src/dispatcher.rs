//! Implementation of [`lingxi_traits::SlashCommandDispatcher`] that routes
//! `/<name> <args>` into the in-crate [`CommandRegistry`].
//!
//! See plan `docs/superpowers/plans/2026-05-25-m5-09-commands-surface.md` Task 5.

use crate::model::CommandResult;
use crate::parser::parse_slash_command;
use crate::registry::CommandRegistry;
use async_trait::async_trait;
use lingxi_traits::{SlashCommandDispatcher, SlashDispatchResult};
use std::sync::Arc;
use tokio::sync::RwLock;

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
    use crate::registry::{register_all_builtin_commands, CommandRegistry};

    fn seeded_dispatcher() -> RegistrySlashDispatcher {
        let mut reg = CommandRegistry::new();
        register_all_builtin_commands(&mut reg);
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
}

//! `/help` — emits the locked rendering of the 99-command surface.
//!
//! See plan `docs/superpowers/plans/2026-05-25-m5-10-commands-batch-1.md`
//! Task 5.

use async_trait::async_trait;
use command_api::builtin_support::help_render::render_help_screen;
use command_api::builtin_support::names::core_description;
use command_api::model::{BuiltinCommandHandler, CommandResult};
use command_api::parser::ParsedSlashCommand;
use telemetry::tengu::command as cmd_evt;

/// `/help` handler — pure function of the static command tables.
///
/// No orchestrator dependency: `/help` is a render of the byte-locked
/// `BUILTIN_COMMAND_NAMES` slice.
#[derive(Debug, Default)]
pub struct HelpHandler;

impl HelpHandler {
    /// Construct a new `HelpHandler`.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl BuiltinCommandHandler for HelpHandler {
    async fn handle(&self, _args: &ParsedSlashCommand) -> CommandResult {
        telemetry::emit_command_started(cmd_evt::HELP_STARTED);
        let s = render_help_screen();
        let details = format!("{{\"lines\":{}}}", s.matches('\n').count());
        telemetry::emit_command_completed(cmd_evt::HELP_COMPLETED, &details);
        CommandResult::Done { display: Some(s) }
    }

    fn name(&self) -> &str {
        "help"
    }

    fn description(&self) -> &str {
        core_description("help")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn returns_done_with_render_help_screen_output() {
        let h = HelpHandler::new();
        let args = ParsedSlashCommand {
            name: "help".to_string(),
            raw_args: String::new(),
            positional_args: vec![],
        };
        match h.handle(&args).await {
            CommandResult::Done { display: Some(s) } => {
                assert!(s.starts_with("Commands:\n"));
                // (M4 cc2.1.198) /agents carries the removed-wizard description.
                assert!(s.contains(
                    "(removed) Ask Claude to create/manage subagents, or edit .claude/agents/"
                ));
                // /x402 stays visible (claude-code ships it with no isHidden gate).
                assert!(s.contains("/x402"));
                // Hidden/disabled commands are filtered out of /help.
                assert!(!s.contains("  /heapdump "));
                assert!(!s.contains("  /ant-trace "));
                // 75 newlines total (1 header + 74 visible commands).
                assert_eq!(s.matches('\n').count(), 75);
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn name_and_description() {
        let h = HelpHandler::new();
        assert_eq!(h.name(), "help");
        assert_eq!(h.description(), "Show help and available commands");
    }
}

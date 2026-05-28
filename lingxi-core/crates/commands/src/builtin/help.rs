//! `/help` — emits the locked rendering of the 99-command surface.
//!
//! See plan `docs/superpowers/plans/2026-05-25-m5-10-commands-batch-1.md`
//! Task 5.

use crate::builtin::help_render::render_help_screen;
use crate::builtin::names::core_description;
use crate::model::{BuiltinCommandHandler, CommandResult};
use crate::parser::ParsedSlashCommand;
use async_trait::async_trait;
use lingxi_telemetry::tengu::command as cmd_evt;

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
        lingxi_telemetry::emit_command_started(cmd_evt::HELP_STARTED);
        let s = render_help_screen();
        let details = format!("{{\"lines\":{}}}", s.matches('\n').count());
        lingxi_telemetry::emit_command_completed(cmd_evt::HELP_COMPLETED, &details);
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
                assert!(s.contains("Manage subagents"));
                assert!(s.contains("/x402"));
                // 100 newlines total (1 header + 99 commands).
                assert_eq!(s.matches('\n').count(), 100);
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

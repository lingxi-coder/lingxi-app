//! `/help` handler.

use crate::model::{BuiltinCommandHandler, CommandResult};
use crate::parser::ParsedSlashCommand;
use async_trait::async_trait;

/// Handler for `/help` — points the user at `/commands` for the full listing.
pub struct HelpHandler;

#[async_trait]
impl BuiltinCommandHandler for HelpHandler {
    fn name(&self) -> &str {
        "help"
    }
    fn description(&self) -> &str {
        "Show available commands."
    }
    async fn handle(&self, _: &ParsedSlashCommand) -> CommandResult {
        CommandResult::Done {
            display: Some("see /commands for full list".into()),
        }
    }
}

//! `/compact` handler (stub).

use crate::model::{BuiltinCommandHandler, CommandResult};
use crate::parser::ParsedSlashCommand;
use async_trait::async_trait;

/// Handler for `/compact` — placeholder; full impl triggers the compaction
/// orchestrator via an effect.
pub struct CompactHandler;

#[async_trait]
impl BuiltinCommandHandler for CompactHandler {
    fn name(&self) -> &str {
        "compact"
    }
    fn description(&self) -> &str {
        "Manually trigger compaction."
    }
    async fn handle(&self, _: &ParsedSlashCommand) -> CommandResult {
        CommandResult::Done {
            display: Some("compact: see /compact help".into()),
        }
    }
}

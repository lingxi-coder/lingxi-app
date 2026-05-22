//! `/memory` handler (stub).

use crate::model::{BuiltinCommandHandler, CommandResult};
use crate::parser::ParsedSlashCommand;
use async_trait::async_trait;

/// Handler for `/memory` — placeholder; full impl wires memory CRUD via Effects.
pub struct MemoryHandler;

#[async_trait]
impl BuiltinCommandHandler for MemoryHandler {
    fn name(&self) -> &str {
        "memory"
    }
    fn description(&self) -> &str {
        "Manage agent memory entries."
    }
    async fn handle(&self, _: &ParsedSlashCommand) -> CommandResult {
        CommandResult::Done {
            display: Some("memory: see /memory help".into()),
        }
    }
}

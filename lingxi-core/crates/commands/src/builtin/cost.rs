//! `/cost` handler — surfaces the current cost snapshot via an effect.

use crate::model::{BuiltinCommandHandler, CommandResult};
use crate::parser::ParsedSlashCommand;
use async_trait::async_trait;
use lingxi_protocol::Effect;

/// Handler for `/cost` — emits a [`Effect::DisplayCostUpdate`] with a
/// placeholder payload. Real snapshot wiring lives in `lingxi-cost`.
pub struct CostHandler;

#[async_trait]
impl BuiltinCommandHandler for CostHandler {
    fn name(&self) -> &str {
        "cost"
    }
    fn description(&self) -> &str {
        "Show session cost."
    }
    async fn handle(&self, _: &ParsedSlashCommand) -> CommandResult {
        CommandResult::EmitEffects {
            effects: vec![Effect::DisplayCostUpdate {
                snapshot_json: serde_json::json!({ "placeholder": true }),
            }],
            display: None,
        }
    }
}

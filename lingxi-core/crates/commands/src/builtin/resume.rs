//! `/resume` handler (stub).

use crate::model::{BuiltinCommandHandler, CommandResult};
use crate::parser::ParsedSlashCommand;
use async_trait::async_trait;

/// Handler for `/resume` — placeholder; full impl loads a persisted session
/// via [`lingxi_protocol::Effect::LoadSession`].
pub struct ResumeHandler;

#[async_trait]
impl BuiltinCommandHandler for ResumeHandler {
    fn name(&self) -> &str {
        "resume"
    }
    fn description(&self) -> &str {
        "Resume a paused session."
    }
    async fn handle(&self, _: &ParsedSlashCommand) -> CommandResult {
        CommandResult::Done {
            display: Some("resume: see /resume help".into()),
        }
    }
}

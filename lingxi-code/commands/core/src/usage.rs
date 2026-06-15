//! `/usage` — render the current session's flat usage/cost snapshot.
//!
//! The interactive TUI Usage tab reads the same `OrchestratorHandle::snapshot_cost`
//! seam. This handler covers headless slash-dispatch paths without opening the
//! settings screen.

use async_trait::async_trait;
use command_api::model::{BuiltinCommandHandler, CommandResult};
use command_api::parser::ParsedSlashCommand;
use std::sync::Arc;
use traits::{CostSnapshot, OrchestratorHandle};

const DESCRIPTION: &str = "Show current session usage";
const M8_GAP_LINE: &str = "Per-model cost breakdown is not available yet (M8).";

/// `/usage` handler backed by the orchestrator's cumulative cost snapshot.
#[derive(Clone)]
pub struct UsageHandler {
    handle: Arc<dyn OrchestratorHandle>,
}

impl UsageHandler {
    /// Construct a `UsageHandler` bound to the given orchestrator handle.
    #[must_use]
    pub fn new(handle: Arc<dyn OrchestratorHandle>) -> Self {
        Self { handle }
    }
}

#[async_trait]
impl BuiltinCommandHandler for UsageHandler {
    async fn handle(&self, _args: &ParsedSlashCommand) -> CommandResult {
        let cost = self.handle.snapshot_cost().await;
        CommandResult::Done {
            display: Some(render_usage_snapshot(&cost)),
        }
    }

    fn name(&self) -> &str {
        "usage"
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }
}

fn render_usage_snapshot(cost: &CostSnapshot) -> String {
    format!(
        "Usage\nTotal cost: ${:.4}\nInput tokens: {}\nOutput tokens: {}\nAPI calls: {}\nSession duration: {}s\n{}\nEsc to close",
        cost.total_usd,
        cost.input_tokens,
        cost.output_tokens,
        cost.api_calls,
        cost.session_duration.as_secs(),
        M8_GAP_LINE
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use orchestrator::test_support::MockOrchestratorHandle;
    use std::time::Duration;

    fn args() -> ParsedSlashCommand {
        ParsedSlashCommand {
            name: "usage".to_string(),
            raw_args: String::new(),
            positional_args: Vec::new(),
        }
    }

    #[tokio::test]
    async fn renders_flat_usage_snapshot() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        mock.set_cost_snapshot(CostSnapshot {
            total_usd: 0.1234,
            input_tokens: 5000,
            output_tokens: 2000,
            api_calls: 7,
            session_duration: Duration::from_secs(125),
            ..CostSnapshot::default()
        });
        let h = UsageHandler::new(mock);
        match h.handle(&args()).await {
            CommandResult::Done { display: Some(s) } => {
                assert_eq!(
                    s,
                    "Usage\nTotal cost: $0.1234\nInput tokens: 5000\nOutput tokens: 2000\nAPI calls: 7\nSession duration: 125s\nPer-model cost breakdown is not available yet (M8).\nEsc to close"
                );
            }
            other => panic!("expected display, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn name_and_description() {
        let h = UsageHandler::new(Arc::new(MockOrchestratorHandle::new()));
        assert_eq!(h.name(), "usage");
        assert_eq!(h.description(), DESCRIPTION);
    }
}

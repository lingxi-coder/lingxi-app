//! `/agents` — list registered subagents.
//!
//! Locked display template (`LingXi` UX, M5-11 T0 step 2 L8):
//!   `"Agents ({count}):\n  {name}  {description}\n…"`
//! Description is truncated to 80 chars + `…` if longer (per `char_indices`).
//! Failure prefix: `"Could not list agents: "` (currently unreachable —
//! `list_agents` is infallible).

use async_trait::async_trait;
use command_api::builtin_support::list_render::render_list;
use command_api::builtin_support::names::core_description;
use command_api::model::{BuiltinCommandHandler, CommandResult};
use command_api::parser::ParsedSlashCommand;
use std::sync::Arc;
use telemetry::tengu::command as cmd_evt;
use traits::{AgentInfo, OrchestratorHandle};

/// `/agents` handler — list mode.
#[derive(Clone)]
pub struct AgentsHandler {
    handle: Arc<dyn OrchestratorHandle>,
}

impl AgentsHandler {
    /// Construct an `AgentsHandler` bound to the given orchestrator handle.
    #[must_use]
    pub fn new(handle: Arc<dyn OrchestratorHandle>) -> Self {
        Self { handle }
    }
}

#[async_trait]
impl BuiltinCommandHandler for AgentsHandler {
    async fn handle(&self, _args: &ParsedSlashCommand) -> CommandResult {
        telemetry::emit_command_started(cmd_evt::AGENTS_STARTED);
        let agents = self.handle.list_agents().await;
        let rows: Vec<String> = agents.iter().map(format_row).collect();
        let s = render_list("Agents", rows, "No subagents configured");
        telemetry::emit_command_completed(cmd_evt::AGENTS_COMPLETED, "");
        CommandResult::Done { display: Some(s) }
    }
    fn name(&self) -> &str {
        "agents"
    }
    fn description(&self) -> &str {
        core_description("agents")
    }
}

fn format_row(a: &AgentInfo) -> String {
    let desc = truncate_with_ellipsis(&a.description, 80);
    format!("{}  {}", a.name, desc)
}

/// Truncate `s` to `max_chars` Unicode code points; append `…` if any
/// characters were dropped. Multibyte-safe.
fn truncate_with_ellipsis(s: &str, max_chars: usize) -> String {
    let mut byte_idx = s.len();
    for (count, (i, _)) in s.char_indices().enumerate() {
        if count == max_chars {
            byte_idx = i;
            break;
        }
    }
    if byte_idx == s.len() {
        s.to_string()
    } else {
        format!("{}…", &s[..byte_idx])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orchestrator::test_support::MockOrchestratorHandle;

    fn args() -> ParsedSlashCommand {
        ParsedSlashCommand {
            name: "agents".into(),
            raw_args: String::new(),
            positional_args: vec![],
        }
    }

    #[tokio::test]
    async fn empty_list() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        let h = AgentsHandler::new(mock);
        if let CommandResult::Done { display: Some(s) } = h.handle(&args()).await {
            assert_eq!(s, "No subagents configured\n");
        } else {
            panic!();
        }
    }

    #[tokio::test]
    async fn one_agent_short_description() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        mock.set_agents(vec![AgentInfo {
            name: "reviewer".into(),
            description: "review code".into(),
            tools_allowed: vec![],
            wildcard_tools: false,
            ..AgentInfo::default()
        }]);
        let h = AgentsHandler::new(mock);
        if let CommandResult::Done { display: Some(s) } = h.handle(&args()).await {
            assert_eq!(s, "Agents (1):\n  reviewer  review code\n");
        } else {
            panic!();
        }
    }

    #[tokio::test]
    async fn description_truncated_at_80_chars() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        let long: String = "a".repeat(100);
        mock.set_agents(vec![AgentInfo {
            name: "x".into(),
            description: long,
            tools_allowed: vec![],
            wildcard_tools: false,
            ..AgentInfo::default()
        }]);
        let h = AgentsHandler::new(mock);
        if let CommandResult::Done { display: Some(s) } = h.handle(&args()).await {
            let expected = format!("Agents (1):\n  x  {}…\n", "a".repeat(80));
            assert_eq!(s, expected);
        } else {
            panic!();
        }
    }

    #[test]
    fn truncate_handles_multibyte_correctly() {
        let s = "中".repeat(85);
        let t = truncate_with_ellipsis(&s, 80);
        assert_eq!(t.chars().count(), 81); // 80 中 + 1 …
    }

    #[tokio::test]
    async fn name_and_description() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        let h = AgentsHandler::new(mock);
        assert_eq!(h.name(), "agents");
        assert_eq!(h.description(), "Manage agent configurations");
    }
}

//! Coordinator-only tool assembly.
//!
//! Builds the coordinator-only tools that carry net-new behavior — `TeamCreate`
//! and `TeamDelete` — each wired to the shared [`TeamRegistry`], the
//! [`CoordinatorMode`] gate, and the [`TeamSpawnSeam`] used to start / stop the
//! real backing teammate task.
//!
//! `SendMessage` / `StructuredOutput` are DELIBERATELY NOT assembled here. Their
//! tool names collide byte-for-byte with already-registered builtins
//! (`tool_ui`'s `SendMessage` / `StructuredOutput`), and the tool registry is
//! push-no-dedup with first-match-wins — so registering coordinator copies would
//! silently shadow nothing useful. Once the shared `MailboxRouter` is wired into
//! `BuiltinToolContext` (engine-desktop `build()`), the in-tree builtins already
//! satisfy those two roles. The source files
//! ([`crate::tool_send_message`] / [`crate::tool_synthetic_output`]) are kept in
//! place but no longer returned from this factory.
//!
//! In §15 (Plugin) and §22 (cli-demo) the host wires the returned tools into
//! `ToolRegistry` IN PLACE OF `tool_team`'s `TeamCreate` / `TeamDelete`, decided
//! at BUILD time, only when [`crate::CoordinatorMode`] is coordinator-capable.

use crate::mode::CoordinatorMode;
use crate::team_registry::TeamRegistry;
use crate::tool_team_create::TeamCreateTool;
use crate::tool_team_delete::TeamDeleteTool;
use std::sync::Arc;
use tool_api::Tool;
use traits::team_spawn::TeamSpawnSeam;
use traits::OutputStream;

/// Build the coordinator-only tools carrying net-new behavior.
///
/// Returns EXACTLY `TeamCreate` + `TeamDelete` as `Arc<dyn Tool>` trait objects.
/// Each constructor clones the shared [`TeamRegistry`], the [`CoordinatorMode`]
/// gate, and the [`TeamSpawnSeam`] into its handler state. `TeamCreate`
/// additionally takes the orchestrator-facing [`OutputStream`] so it can PUSH
/// the live active-worker count the moment a spawn is reconciled (deterministic
/// activation, independent of the teammate's racy startup status emit).
#[must_use]
pub fn coordinator_internal_tools(
    team: Arc<TeamRegistry>,
    mode: Arc<CoordinatorMode>,
    spawn_seam: Arc<dyn TeamSpawnSeam>,
    output: Arc<dyn OutputStream>,
) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(TeamCreateTool::new(
            team.clone(),
            mode.clone(),
            spawn_seam.clone(),
            output,
        )) as Arc<dyn Tool>,
        Arc::new(TeamDeleteTool::new(team, mode, spawn_seam)) as Arc<dyn Tool>,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use protocol::AgentId;
    use traits::team_spawn::TeamSpawnError;

    /// No-op spawn seam — the factory under test never invokes it; it only needs
    /// a concrete `Arc<dyn TeamSpawnSeam>` to construct the tools.
    struct NoopSeam;

    #[async_trait]
    impl TeamSpawnSeam for NoopSeam {
        async fn spawn_teammate(
            &self,
            _agent_id: AgentId,
            _name: String,
            _description: String,
        ) -> Result<String, TeamSpawnError> {
            Ok(String::new())
        }
        async fn kill(&self, _task_id: &str) -> Result<(), TeamSpawnError> {
            Ok(())
        }
    }

    /// No-op output — the factory under test never emits; it only needs a
    /// concrete `Arc<dyn OutputStream>` to construct `TeamCreate`.
    struct NoopOutput;

    #[async_trait]
    impl OutputStream for NoopOutput {
        async fn emit_text(&self, _text: &str) {}
        async fn emit_tool_call(
            &self,
            _id: &protocol::ToolUseId,
            _tool: &str,
            _input: &serde_json::Value,
        ) {
        }
        async fn emit_tool_result(
            &self,
            _id: &protocol::ToolUseId,
            _tool: &str,
            _result: &serde_json::Value,
        ) {
        }
        async fn emit_end_turn(&self, _stop_reason: &str, _cost: &traits::CostSnapshot) {}
    }

    #[test]
    fn factory_returns_exactly_team_create_and_delete() {
        let team = Arc::new(TeamRegistry::new(AgentId::new()));
        let mode = Arc::new(CoordinatorMode::new());
        let seam: Arc<dyn TeamSpawnSeam> = Arc::new(NoopSeam);
        let output: Arc<dyn OutputStream> = Arc::new(NoopOutput);

        let tools = coordinator_internal_tools(team, mode, seam, output);

        // EXACTLY two tools — SendMessage / StructuredOutput are dropped.
        assert_eq!(
            tools.len(),
            2,
            "factory must return exactly TeamCreate + TeamDelete (not the 4-tool set)"
        );

        let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        assert_eq!(names, vec!["TeamCreate", "TeamDelete"]);

        // SendMessage / StructuredOutput must NOT be present (builtins satisfy them).
        assert!(
            !names.contains(&"SendMessage"),
            "SendMessage must be dropped from the coordinator factory"
        );
        assert!(
            !names.contains(&"StructuredOutput"),
            "StructuredOutput must be dropped from the coordinator factory"
        );
    }
}

//! Coordinator-only tool assembly.
//!
//! Builds the coordinator-only tools that carry net-new behavior — `TeamCreate`,
//! `TeamDelete`, and `SendMessage` — each wired to the shared [`TeamRegistry`],
//! the [`CoordinatorMode`] gate, and (where relevant) the [`TeamSpawnSeam`] used
//! to start / stop / cancel the real backing teammate task.
//!
//! Each of these tool names collides byte-for-byte with an already-registered
//! builtin (`tool_ui`'s `SendMessage`, `tool_team`'s `TeamCreate` /
//! `TeamDelete`). The host wires the returned tools into `ToolRegistry` IN PLACE
//! OF those builtins, decided at BUILD time, only when [`crate::CoordinatorMode`]
//! is coordinator-capable (§15 Plugin, §22 cli-demo). The coordinator
//! `SendMessage` carries the full swarm routing surface — broadcast fan-out,
//! teammate-name resolution, and the shutdown / plan-approval structured-message
//! handshake — over the registry's `MailboxRouter`, which the leaner `tool_ui`
//! builtin does not.
//!
//! `StructuredOutput` ([`crate::tool_synthetic_output`]) is still NOT assembled
//! here: the in-tree builtin already satisfies that role once the shared
//! `MailboxRouter` is wired into `BuiltinToolContext`.

use crate::mode::CoordinatorMode;
use crate::team_registry::TeamRegistry;
use crate::tool_send_message::SendMessageTool;
use crate::tool_team_create::TeamCreateTool;
use crate::tool_team_delete::TeamDeleteTool;
use std::sync::Arc;
use telemetry::AnalyticsBus;
use tool_api::Tool;
use traits::team_spawn::TeamSpawnSeam;
use traits::{OutputStream, RuntimeSpawner};

/// Build the coordinator-only tools carrying net-new behavior.
///
/// Returns `TeamCreate` + `TeamDelete` + `SendMessage` as `Arc<dyn Tool>` trait
/// objects. Each constructor clones the shared [`TeamRegistry`] into its handler
/// state; the team tools also take the [`CoordinatorMode`] gate, and `SendMessage`
/// / the team tools take the [`TeamSpawnSeam`] (`SendMessage` uses it to cancel
/// an approved in-process shutdown's backing task). `TeamCreate` additionally
/// takes the orchestrator-facing [`OutputStream`] so it can PUSH the live
/// active-worker count the moment a spawn is reconciled (deterministic
/// activation, independent of the teammate's racy startup status emit).
///
/// `bus` is the (optional) analytics bus the team tools fire their coordinator
/// telemetry through (`tengu_team_created` / `tengu_team_deleted`). `None` ⇒
/// telemetry falls back to `tracing` (hermetic tests pass `None`).
///
/// `runtime` is the (optional) background-task spawner the `TeamCreate` tool
/// uses to start each teammate's mailbox→runner PUMP after a spawn — the bridge
/// that delivers a coordinator `SendMessage` into the teammate's turn loop.
/// `None` ⇒ no pump is started (routed messages still queue in the mailbox but
/// are not auto-drained); the desktop composition root passes the session
/// `RuntimeSpawner`.
#[must_use]
pub fn coordinator_internal_tools(
    team: Arc<TeamRegistry>,
    mode: Arc<CoordinatorMode>,
    spawn_seam: Arc<dyn TeamSpawnSeam>,
    output: Arc<dyn OutputStream>,
    bus: Option<Arc<AnalyticsBus>>,
    runtime: Option<Arc<dyn RuntimeSpawner>>,
) -> Vec<Arc<dyn Tool>> {
    let mut team_create =
        TeamCreateTool::new(team.clone(), mode.clone(), spawn_seam.clone(), output)
            .with_analytics_bus(bus.clone());
    if let Some(runtime) = runtime {
        team_create = team_create.with_runtime(runtime);
    }
    vec![
        Arc::new(team_create) as Arc<dyn Tool>,
        Arc::new(
            TeamDeleteTool::new(team.clone(), mode, spawn_seam.clone()).with_analytics_bus(bus),
        ) as Arc<dyn Tool>,
        Arc::new(SendMessageTool::new(team).with_spawn_seam(spawn_seam)) as Arc<dyn Tool>,
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
    fn factory_returns_team_tools_plus_send_message() {
        let team = Arc::new(TeamRegistry::new(AgentId::new()));
        let mode = Arc::new(CoordinatorMode::new());
        let seam: Arc<dyn TeamSpawnSeam> = Arc::new(NoopSeam);
        let output: Arc<dyn OutputStream> = Arc::new(NoopOutput);

        let tools = coordinator_internal_tools(team, mode, seam, output, None, None);

        // TeamCreate + TeamDelete + SendMessage (the coordinator SendMessage now
        // carries the full swarm routing surface). StructuredOutput stays dropped.
        let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        assert_eq!(names, vec!["TeamCreate", "TeamDelete", "SendMessage"]);

        assert!(
            !names.contains(&"StructuredOutput"),
            "StructuredOutput must be dropped from the coordinator factory"
        );
    }
}

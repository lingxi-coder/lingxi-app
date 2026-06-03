//! `TeamDeleteTool` — coordinator-only tool that removes a worker from the
//! [`TeamRegistry`] and unregisters its mailbox.
//!
//! 1:1 Rust port of the TS `TeamDeleteTool` (`src/tools/TeamDeleteTool/`).
//! The TS tool name is `"TeamDelete"` and `userFacingName()` returns `''`.
//!
//! Adaptation note: the lingxi `coordinator::TeamRegistry` tracks workers
//! 1:per-`AgentId` (rather than the TS per-team-file model), and its
//! operational mutator is [`TeamRegistry::delete_worker`]. The TS schema is
//! `z.strictObject({})` (team name read from session `AppState`); the lingxi
//! port instead takes the target worker `agent_id` as input — required so the
//! tool can perform `delete_worker(&agent_id)` and map an unknown id to the
//! trait's `InvalidInput` error variant (the lingxi-side equivalent of the
//! TS "nothing to clean up" / active-member guards). Schema shape
//! (`type:object`, `additionalProperties:false`) mirrors the TS strict object.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde_json::{json, Value};
use uuid::Uuid;

use protocol::AgentId;
use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, InterruptBehavior, PromptOptions, Tool, ToolCallResult, ToolError,
    ToolStaticContext,
};
use traits::team_spawn::TeamSpawnSeam;

use crate::mode::CoordinatorMode;
use crate::team_registry::TeamRegistry;

/// Canonical tool name in the registry (matches TS `TEAM_DELETE_TOOL_NAME`).
pub const TEAM_DELETE_TOOL_NAME: &str = "TeamDelete";

/// Backing store for the cached input schema (built once, on first access).
///
/// `std::sync::OnceLock` is used instead of `once_cell::sync::Lazy` so the
/// coordinator crate does not take a new dependency on `once_cell` (mirrors the
/// sibling coordinator tools `tool_team_create.rs` / `tool_send_message.rs` /
/// `tool_synthetic_output.rs`).
static TEAM_DELETE_SCHEMA: OnceLock<Value> = OnceLock::new();

/// Input schema. Mirrors the TS `z.strictObject` shape
/// (`additionalProperties:false`) but exposes the `agent_id` the lingxi
/// registry needs to identify the worker to remove.
fn team_delete_schema() -> &'static Value {
    TEAM_DELETE_SCHEMA.get_or_init(|| {
        json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["agent_id"],
            "properties": {
                "agent_id": {
                    "type": "string",
                    "description": "Agent ID (UUID) of the worker to remove from the team."
                }
            }
        })
    })
}

/// Coordinator-only tool: remove a worker and unregister its mailbox.
///
/// Holds the shared [`TeamRegistry`], the [`CoordinatorMode`] gate, and the
/// [`TeamSpawnSeam`] used to kill the backing teammate task. `call()` (T06)
/// gates on coordinator mode, kills the worker's backing teammate task
/// best-effort (reading its handler-generated `task_id`), then removes the
/// worker + unregisters its mailbox.
pub struct TeamDeleteTool {
    team: Arc<TeamRegistry>,
    /// Coordinator-mode gate consulted by `call()` (defense-in-depth).
    mode: Arc<CoordinatorMode>,
    /// Seam used by `call()` to kill the backing teammate task on delete.
    spawn_seam: Arc<dyn TeamSpawnSeam>,
}

impl TeamDeleteTool {
    /// Construct a new tool wired to the shared coordinator registry, the mode
    /// gate, and the teammate spawn seam.
    #[must_use]
    pub fn new(
        team: Arc<TeamRegistry>,
        mode: Arc<CoordinatorMode>,
        spawn_seam: Arc<dyn TeamSpawnSeam>,
    ) -> Self {
        Self {
            team,
            mode,
            spawn_seam,
        }
    }
}

#[async_trait]
impl Tool for TeamDeleteTool {
    fn name(&self) -> &str {
        TEAM_DELETE_TOOL_NAME
    }

    fn input_schema(&self) -> &Value {
        team_delete_schema()
    }

    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        // TS: isEnabled() => isAgentSwarmsEnabled(). Coordinator tools are only
        // wired into the registry when coordinator mode is enabled, so the
        // gate is upstream; this tool is always enabled once present.
        true
    }

    fn should_defer(&self) -> bool {
        // TS: shouldDefer: true.
        true
    }

    fn search_hint(&self) -> Option<&str> {
        // TS: searchHint: 'disband a swarm team and clean up'.
        Some("disband a swarm team and clean up")
    }

    fn max_result_size_chars(&self) -> usize {
        tool_api::util::output_truncation::MAX_TOOL_OUTPUT_LENGTH
    }

    fn is_concurrency_safe(&self, _: &Value) -> bool {
        // Mutates the shared worker map, but the registry is RwLock-guarded.
        true
    }

    fn is_read_only(&self, _: &Value) -> bool {
        false
    }

    fn is_destructive(&self, _: &Value) -> bool {
        // Mirrors the destructive `TeamDeleteTool` semantics: removes a worker.
        true
    }

    fn is_open_world(&self, _: &Value) -> bool {
        false
    }

    fn interrupt_behavior(&self, _: &Value) -> InterruptBehavior {
        InterruptBehavior::Block
    }

    async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
        // TS: no explicit checkPermissions override -> allow. The tool only
        // mutates coordinator-local in-memory team state.
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other {
                reason: "TeamDelete mutates coordinator-local team state only".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        // TS: 'Clean up team and task directories when the swarm is complete'.
        "Clean up team and task directories when the swarm is complete".into()
    }

    async fn prompt(&self, _: &PromptOptions) -> String {
        // 1:1 with TS `getPrompt()`.
        "# TeamDelete\n\nRemove team and task directories when the swarm work is complete.\n\nThis operation:\n- Removes the team directory (`~/.claude/teams/{team-name}/`)\n- Removes the task directory (`~/.claude/tasks/{team-name}/`)\n- Clears team context from the current session\n\n**IMPORTANT**: TeamDelete will fail if the team still has active members. Gracefully terminate teammates first, then call TeamDelete after all teammates have shut down.\n\nUse this when all teammates have finished their work and you want to clean up the team resources. The team name is automatically determined from the current session's team context.".into()
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        _progress: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        // 1. Mode early-return (defense-in-depth, mirrors TeamCreate's T05 gate).
        //    Lets a future `/coordinator exit()` neutralize the tool without a
        //    registry rebuild. Net-new in call(); distinct from `is_enabled`'s
        //    static gate (which only governs whether the tool is advertised).
        if !self.mode.is_enabled() {
            return Err(ToolError::InvalidInput("coordinator mode not active".into()));
        }

        // Parse the target worker id defensively (do not rely on schema alone).
        let agent_id_str = input
            .get("agent_id")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidInput("TeamDelete: missing 'agent_id'".into()))?
            .trim();

        if agent_id_str.is_empty() {
            return Err(ToolError::InvalidInput(
                "TeamDelete: 'agent_id' is required".into(),
            ));
        }

        let uuid = Uuid::parse_str(agent_id_str).map_err(|_| {
            ToolError::InvalidInput(format!(
                "TeamDelete: 'agent_id' is not a valid UUID: {agent_id_str}"
            ))
        })?;
        let agent_id = AgentId::from_uuid(uuid);

        // 2. `delete_worker` is infallible and a no-op for unknown ids, so probe
        //    the registry first to surface an unknown-agent error to the model.
        //    Capture the matched WorkerAgent so we can read its backing task_id.
        let worker = self
            .team
            .list()
            .await
            .into_iter()
            .find(|w| w.agent_id == agent_id);
        let Some(worker) = worker else {
            return Err(ToolError::InvalidInput(format!(
                "TeamDelete: unknown agent id: {agent_id_str}"
            )));
        };

        // 3. Kill the backing teammate task (best-effort). If the worker was
        //    never linked to a real task (empty task_id), there is nothing to
        //    kill. A kill error is surfaced as a warning on the result but does
        //    NOT abort the worker removal below.
        let mut kill_warning: Option<String> = None;
        if !worker.task_id.is_empty() {
            if let Err(e) = self.spawn_seam.kill(&worker.task_id).await {
                kill_warning = Some(format!("failed to kill backing task: {e}"));
            }
        }

        // 4. Remove the worker and unregister its mailbox.
        self.team.delete_worker(&agent_id).await;

        let mut data = json!({
            "success": true,
            "message": format!("Removed worker {agent_id_str} from the team"),
            "agent_id": agent_id_str,
        });
        if let Some(warning) = kill_warning {
            data["warning"] = Value::String(warning);
        }

        Ok(ToolCallResult {
            data,
            new_messages: Vec::new(),
            context_modifier: None,
            mcp_meta: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tool_api::context::{ToolUseContext, ToolUseOptions};
    use tool_api::progress::progress_channel;

    fn fresh_ctx() -> ToolUseContext {
        ToolUseContext {
            options: ToolUseOptions {
                debug: false,
                verbose: false,
                main_loop_model: "test".into(),
                max_budget_nano_usd: None,
                mcp_clients: vec![],
                is_non_interactive_session: false,
                custom_system_prompt: None,
                append_system_prompt: None,
            },
            messages: vec![],
            tool_use_id: None,
            agent_id: None,
            content_replacement_state: None,
            session: None,
            subagent_registry: None,
        }
    }

    fn fresh_tx() -> ToolProgressSender {
        let (tx, _rx) = progress_channel();
        tx
    }

    fn registry() -> Arc<TeamRegistry> {
        Arc::new(TeamRegistry::new(AgentId::new()))
    }

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    /// Recording spawn seam: counts `kill` invocations and captures the last
    /// `task_id` passed to `kill`, so T06 tests can assert the backing teammate
    /// task was killed with the right id. `spawn_teammate` is unused here
    /// (delete tests spawn workers directly via the registry / a fixed id).
    struct RecordingSeam {
        kills: AtomicUsize,
        last_killed: Mutex<Option<String>>,
        /// When set, `kill` returns this error to exercise the best-effort path.
        kill_err: Option<String>,
    }

    impl RecordingSeam {
        fn new() -> Self {
            Self {
                kills: AtomicUsize::new(0),
                last_killed: Mutex::new(None),
                kill_err: None,
            }
        }

        fn with_kill_err(msg: impl Into<String>) -> Self {
            Self {
                kills: AtomicUsize::new(0),
                last_killed: Mutex::new(None),
                kill_err: Some(msg.into()),
            }
        }
    }

    #[async_trait]
    impl TeamSpawnSeam for RecordingSeam {
        async fn spawn_teammate(
            &self,
            _agent_id: AgentId,
            _name: String,
            _description: String,
        ) -> Result<String, traits::team_spawn::TeamSpawnError> {
            Ok(String::new())
        }
        async fn kill(&self, task_id: &str) -> Result<(), traits::team_spawn::TeamSpawnError> {
            self.kills.fetch_add(1, Ordering::SeqCst);
            *self.last_killed.lock().unwrap() = Some(task_id.to_string());
            match &self.kill_err {
                Some(msg) => Err(traits::team_spawn::TeamSpawnError::NotFound(msg.clone())),
                None => Ok(()),
            }
        }
    }

    /// Build a `TeamDeleteTool` wired to `team` with coordinator mode ENABLED
    /// (the normal path) and a recording seam.
    fn make_tool(team: Arc<TeamRegistry>) -> TeamDeleteTool {
        let (tool, _seam) = make_tool_with_seam(team, Arc::new(RecordingSeam::new()));
        tool
    }

    /// Build a `TeamDeleteTool` wired to `team`, mode ENABLED, with the given
    /// recording seam (so callers can assert kill behavior).
    fn make_tool_with_seam(
        team: Arc<TeamRegistry>,
        seam: Arc<RecordingSeam>,
    ) -> (TeamDeleteTool, Arc<RecordingSeam>) {
        let mode = Arc::new(CoordinatorMode::new());
        mode.enter();
        let tool = TeamDeleteTool::new(team, mode, seam.clone() as Arc<dyn TeamSpawnSeam>);
        (tool, seam)
    }

    #[test]
    fn name_matches_ts_constant() {
        let tool = make_tool(registry());
        assert_eq!(tool.name(), "TeamDelete");
        assert_eq!(tool.name(), TEAM_DELETE_TOOL_NAME);
    }

    #[test]
    fn schema_is_strict_object() {
        let tool = make_tool(registry());
        let schema = tool.input_schema();
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["properties"]["agent_id"]["type"], "string");
    }

    #[test]
    fn flags_match_ts() {
        let tool = make_tool(registry());
        assert!(tool.should_defer());
        assert!(tool.is_destructive(&json!({})));
        assert!(!tool.is_read_only(&json!({})));
        assert!(tool.is_enabled(&ToolStaticContext::default()));
    }

    #[tokio::test]
    async fn deletes_existing_worker() {
        let team = registry();
        let agent_id = team
            .spawn_worker("explorer".into(), "alice".into(), "task-1".into())
            .await
            .expect("spawn must succeed");
        assert_eq!(team.list().await.len(), 1);

        let tool = make_tool(team.clone());
        let res = tool
            .call(
                json!({ "agent_id": agent_id.as_uuid().to_string() }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("delete must succeed for a known worker");

        assert_eq!(res.data["success"], true);
        assert_eq!(res.data["agent_id"], agent_id.as_uuid().to_string());

        // Effect: worker is gone from the registry.
        assert!(team.list().await.is_empty());

        // Effect: the mailbox was unregistered — routing to it now fails.
        let route_err = team
            .mailbox_router
            .route(
                &agent_id,
                crate::mailbox::TeammateMessage {
                    from: crate::mailbox::MessageSender::Coordinator,
                    content: "ping".into(),
                    message_id: "m1".into(),
                    timestamp: std::time::SystemTime::now(),
                },
            )
            .await;
        assert!(matches!(
            route_err,
            Err(crate::mailbox::MailboxError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn unknown_agent_id_is_invalid_input() {
        let team = registry();
        let tool = make_tool(team);
        let err = tool
            .call(
                json!({ "agent_id": Uuid::new_v4().to_string() }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect_err("delete must fail for an unknown agent id");
        assert!(matches!(err, ToolError::InvalidInput(_)));
        assert!(format!("{err}").contains("unknown agent id"));
    }

    #[tokio::test]
    async fn missing_agent_id_is_invalid_input() {
        let team = registry();
        let tool = make_tool(team);
        let err = tool
            .call(json!({}), fresh_ctx(), fresh_tx())
            .await
            .expect_err("delete must fail when agent_id is absent");
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn malformed_uuid_is_invalid_input() {
        let team = registry();
        let tool = make_tool(team);
        let err = tool
            .call(
                json!({ "agent_id": "not-a-uuid" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect_err("delete must fail for a malformed UUID");
        assert!(matches!(err, ToolError::InvalidInput(_)));
        assert!(format!("{err}").contains("valid UUID"));
    }

    // ---- T06: mode gate + kill backing teammate ----

    #[tokio::test]
    async fn delete_when_mode_disabled_errors() {
        // Mode OFF: the call() early-return fires before any lookup/kill/delete.
        let team = registry();
        let agent_id = team
            .spawn_worker("explorer".into(), "alice".into(), "task-1".into())
            .await
            .expect("spawn must succeed");

        let mode = Arc::new(CoordinatorMode::new()); // disabled by default
        let seam = Arc::new(RecordingSeam::new());
        let tool = TeamDeleteTool::new(
            team.clone(),
            mode,
            seam.clone() as Arc<dyn TeamSpawnSeam>,
        );

        let err = tool
            .call(
                json!({ "agent_id": agent_id.as_uuid().to_string() }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect_err("call must fail when coordinator mode is disabled");
        assert!(matches!(err, ToolError::InvalidInput(_)));
        assert_eq!(
            format!("{err}"),
            "invalid input: coordinator mode not active"
        );

        // No kill issued and the worker is still present.
        assert_eq!(seam.kills.load(Ordering::SeqCst), 0);
        assert_eq!(team.list().await.len(), 1, "worker untouched on disabled mode");
    }

    #[tokio::test]
    async fn delete_kills_backing_task_then_removes_worker() {
        // Worker is spawned with a non-empty backing task_id.
        let team = registry();
        let agent_id = team
            .spawn_worker("explorer".into(), "alice".into(), "handler-task-7".into())
            .await
            .expect("spawn must succeed");
        assert_eq!(team.list().await.len(), 1);

        let (tool, seam) = make_tool_with_seam(team.clone(), Arc::new(RecordingSeam::new()));
        let res = tool
            .call(
                json!({ "agent_id": agent_id.as_uuid().to_string() }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("delete must succeed for a known worker");

        assert_eq!(res.data["success"], true);

        // The backing teammate task was killed with the worker's task_id.
        assert_eq!(seam.kills.load(Ordering::SeqCst), 1, "kill invoked once");
        assert_eq!(
            seam.last_killed.lock().unwrap().clone(),
            Some("handler-task-7".to_string()),
            "kill received the worker's backing task_id"
        );

        // The worker was removed from the registry.
        assert!(team.list().await.is_empty(), "worker removed after delete");
    }

    #[tokio::test]
    async fn delete_with_empty_task_id_skips_kill() {
        // Worker spawned but never linked to a backing task (empty task_id).
        let team = registry();
        let agent_id = team
            .spawn_worker("explorer".into(), "alice".into(), String::new())
            .await
            .expect("spawn must succeed");

        let (tool, seam) = make_tool_with_seam(team.clone(), Arc::new(RecordingSeam::new()));
        tool.call(
            json!({ "agent_id": agent_id.as_uuid().to_string() }),
            fresh_ctx(),
            fresh_tx(),
        )
        .await
        .expect("delete must succeed");

        // No kill issued (nothing to kill) but the worker is still removed.
        assert_eq!(seam.kills.load(Ordering::SeqCst), 0, "no kill for empty task_id");
        assert!(team.list().await.is_empty(), "worker removed");
    }

    #[tokio::test]
    async fn delete_completes_even_if_kill_errors() {
        // Best-effort kill: a seam error must NOT abort the worker removal.
        let team = registry();
        let agent_id = team
            .spawn_worker("explorer".into(), "alice".into(), "handler-task-9".into())
            .await
            .expect("spawn must succeed");

        let (tool, seam) =
            make_tool_with_seam(team.clone(), Arc::new(RecordingSeam::with_kill_err("boom")));
        let res = tool
            .call(
                json!({ "agent_id": agent_id.as_uuid().to_string() }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("delete must succeed even when kill errors");

        assert_eq!(res.data["success"], true);
        assert_eq!(seam.kills.load(Ordering::SeqCst), 1, "kill was attempted");
        assert!(team.list().await.is_empty(), "worker removed despite kill error");
    }
}

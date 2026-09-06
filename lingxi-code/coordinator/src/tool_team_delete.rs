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

use platform_api::team_spawn::TeamSpawnSeam;
use protocol::AgentId;
use telemetry::sink::{AnalyticsValue, LogEventMetadata};
use telemetry::tengu::coordinator::TEAM_DELETED;
use telemetry::AnalyticsBus;
use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, InterruptBehavior, PromptOptions, Tool, ToolCallResult, ToolError,
    ToolStaticContext,
};

use crate::mode::CoordinatorMode;
use crate::team_file;
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
    /// Optional analytics bus for `tengu_team_deleted`. `None` (test default) ⇒
    /// the event is logged via `tracing` only.
    bus: Option<Arc<AnalyticsBus>>,
    /// Optional `~/.claude` root override (tests inject a tempdir for hermetic
    /// directory cleanup). `None` (production) ⇒ resolve from `$HOME`.
    home_override: Option<std::path::PathBuf>,
}

impl TeamDeleteTool {
    /// Construct a new tool wired to the shared coordinator registry, the mode
    /// gate, and the teammate spawn seam. No analytics bus is attached
    /// (telemetry falls back to `tracing`); use [`Self::with_analytics_bus`].
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
            bus: None,
            home_override: None,
        }
    }

    /// Attach an analytics bus so `call()` fires `tengu_team_deleted`.
    #[must_use]
    pub fn with_analytics_bus(mut self, bus: Option<Arc<AnalyticsBus>>) -> Self {
        self.bus = bus;
        self
    }

    /// Override the `~/.claude` root for directory cleanup (tests use a tempdir).
    #[must_use]
    pub fn with_home(mut self, home: std::path::PathBuf) -> Self {
        self.home_override = Some(home);
        self
    }

    /// Fire `tengu_team_deleted { team_name }` (TeamDeleteTool.ts:111-114).
    async fn emit_team_deleted(&self, team_name: &str) {
        if let Some(bus) = &self.bus {
            let mut md: LogEventMetadata = LogEventMetadata::new();
            md.insert(
                "_PROTO_team_name".into(),
                AnalyticsValue::String(
                    telemetry::pii::PiiTagged::assert_pii_tagged_column(team_name.to_string())
                        .into_inner(),
                ),
            );
            bus.log_event(TEAM_DELETED, md).await;
        } else {
            tracing::info!(event = TEAM_DELETED, team_name, "team deleted");
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
        "# TeamDelete\n\nRemove team and task directories when the swarm work is complete.\n\nThis operation:\n- Removes the team directory (`~/.lingxi/teams/{team-name}/`)\n- Removes the task directory (`~/.lingxi/tasks/{team-name}/`)\n- Clears team context from the current session\n\n**IMPORTANT**: TeamDelete will fail if the team still has active members. Gracefully terminate teammates first, then call TeamDelete after all teammates have shut down.\n\nUse this when all teammates have finished their work and you want to clean up the team resources. The team name is automatically determined from the current session's team context.".into()
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
            return Err(ToolError::InvalidInput(
                "coordinator mode not active".into(),
            ));
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

        // 2a. Active-member guard (TeamDeleteTool.ts:76-98). Refuse to disband a
        //     team that still has active members — the caller must gracefully
        //     terminate teammates first. We count non-terminal workers OTHER than
        //     the delete target (the TS guard filters out the lead and counts
        //     only other `isActive !== false` members); the target itself is the
        //     one being torn down, so it never blocks its own removal. On a
        //     positive count we return a SUCCESSFUL ToolCallResult with
        //     `success: false` (NOT a tool error), mirroring the TS data shape.
        let total_active = self.team.active_worker_count().await;
        let target_is_active = matches!(
            worker.status,
            crate::team_registry::WorkerStatus::Idle
                | crate::team_registry::WorkerStatus::Working { .. }
                | crate::team_registry::WorkerStatus::AwaitingMessage
        );
        let other_active = total_active.saturating_sub(u32::from(target_is_active));
        if other_active > 0 {
            return Ok(ToolCallResult {
                data: json!({
                    "success": false,
                    "message": format!(
                        "Cannot cleanup team with {other_active} active member(s). \
                         Use requestShutdown to gracefully terminate teammates first."
                    ),
                    "agent_id": agent_id_str,
                }),
                model_content: None,
                new_messages: Vec::new(),
                context_modifier: None,
                is_error: false,
                mcp_meta: None,
            });
        }

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

        // 4a. Team-level cleanup (TeamDeleteTool.ts:101-124): once the last
        //     worker is gone, remove the on-disk team + task directories
        //     (`~/.lingxi/teams/{name}/` + `~/.lingxi/tasks/{name}/`), clear the
        //     coordinator's team context (`set_team_name(None)`), and fire
        //     `tengu_team_deleted`. Cleanup only runs when no live workers
        //     remain, so deleting one worker of a (now otherwise-terminal) team
        //     finishes the teardown.
        let team_name = self.team.team_name().await;
        let remaining = self.team.list().await.len();
        if remaining == 0 {
            let home = self.home_override.clone().or_else(team_file::lingxi_home);
            if let (Some(name), Some(home)) = (&team_name, home) {
                team_file::cleanup_team_directories(&home, name);
            }
            self.team.set_team_name(None).await;
            // Clear the process-global leader team name (claude-code
            // `clearLeaderTeamName`, `utils/tasks.ts:43`) so the leader's
            // `getTaskListId()` falls back to the session id once the team is
            // gone.
            platform_api::team_registry::clear_leader_team_name();
            if let Some(name) = &team_name {
                self.emit_team_deleted(name).await;
            }
        }

        let mut data = json!({
            "success": true,
            "message": format!("Removed worker {agent_id_str} from the team"),
            "agent_id": agent_id_str,
        });
        if let Some(name) = &team_name {
            data["team_name"] = Value::String(name.clone());
        }
        if let Some(warning) = kill_warning {
            data["warning"] = Value::String(warning);
        }

        Ok(ToolCallResult {
            data,
            model_content: None,
            new_messages: Vec::new(),
            context_modifier: None,
            is_error: false,
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
                model_profile: None,
                max_budget_nano_usd: None,
                mcp_clients: vec![],
                is_non_interactive_session: false,
                custom_system_prompt: None,
                append_system_prompt: None,
            },
            messages: vec![],
            tool_use_id: None,
            assistant_message_id: None,
            agent_id: None,
            agent_name: None,
            observer: None,
            team_name: None,
            origin_session_id: None,
            tool_execution_policy: platform_api::tool_invoker::ToolExecutionPolicy::Ordinary,
            content_replacement_state: None,
            session: None,
            subagent_registry: None,
            cancel: None,
            fork_parent_system_prompt: None,
            cwd: None,
            depth: 0,
            file_history: None,
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
            _team_name: String,
            _description: String,
        ) -> Result<String, platform_api::team_spawn::TeamSpawnError> {
            Ok(String::new())
        }
        async fn kill(
            &self,
            task_id: &str,
        ) -> Result<(), platform_api::team_spawn::TeamSpawnError> {
            self.kills.fetch_add(1, Ordering::SeqCst);
            *self.last_killed.lock().unwrap() = Some(task_id.to_string());
            match &self.kill_err {
                Some(msg) => Err(platform_api::team_spawn::TeamSpawnError::NotFound(
                    msg.clone(),
                )),
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
                    from_name: "team-lead".into(),
                    content: "ping".into(),
                    summary: None,
                    message_id: "m1".into(),
                    timestamp: std::time::SystemTime::now(),
                    request_id: None,
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
            .call(json!({ "agent_id": "not-a-uuid" }), fresh_ctx(), fresh_tx())
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
        let tool = TeamDeleteTool::new(team.clone(), mode, seam.clone() as Arc<dyn TeamSpawnSeam>);

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
        assert_eq!(
            team.list().await.len(),
            1,
            "worker untouched on disabled mode"
        );
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
        assert_eq!(
            seam.kills.load(Ordering::SeqCst),
            0,
            "no kill for empty task_id"
        );
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
        assert!(
            team.list().await.is_empty(),
            "worker removed despite kill error"
        );
    }

    // ---- D1 ITEM 3: active-member guard + dir cleanup + telemetry ----

    use crate::team_registry::WorkerStatus;

    /// Active-member guard (TeamDeleteTool.ts:76-98): deleting one worker while
    /// OTHER workers are still active returns `success: false` (NOT a tool
    /// error), with the "Cannot cleanup team with N active member(s)…" message,
    /// and removes nothing.
    #[tokio::test]
    async fn delete_blocked_by_active_members() {
        let team = registry();
        let target = team
            .spawn_worker("explorer".into(), "alice".into(), "t-1".into())
            .await
            .unwrap();
        // Two MORE active workers besides the target → guard must block.
        let _other1 = team
            .spawn_worker("explorer".into(), "bob".into(), "t-2".into())
            .await
            .unwrap();
        let _other2 = team
            .spawn_worker("explorer".into(), "carol".into(), "t-3".into())
            .await
            .unwrap();

        let (tool, seam) = make_tool_with_seam(team.clone(), Arc::new(RecordingSeam::new()));
        let res = tool
            .call(
                json!({ "agent_id": target.as_uuid().to_string() }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("guard returns a successful ToolCallResult with success:false");

        assert_eq!(res.data["success"], false, "blocked → success:false");
        let msg = res.data["message"].as_str().unwrap();
        assert!(
            msg.contains("Cannot cleanup team with 2 active member(s)"),
            "message must report the OTHER active count; got: {msg}"
        );
        assert!(msg.contains("requestShutdown"));
        // Nothing removed, no kill issued.
        assert_eq!(team.list().await.len(), 3, "no worker removed on guard");
        assert_eq!(seam.kills.load(Ordering::SeqCst), 0, "no kill on guard");
    }

    /// Deleting the LAST worker (no other active members) succeeds, removes the
    /// on-disk team + task directories, clears the team name, and fires
    /// `tengu_team_deleted`.
    #[tokio::test]
    async fn delete_last_worker_cleans_up_and_fires_telemetry() {
        use telemetry::sinks::InMemorySink;
        let bus = Arc::new(AnalyticsBus::new());
        let sink = Arc::new(InMemorySink::new());
        bus.attach_sink(sink.clone()).await;

        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join(".lingxi");

        let team = registry();
        team.set_team_name(Some("alpha".into())).await;
        let target = team
            .spawn_worker("explorer".into(), "alice".into(), "t-1".into())
            .await
            .unwrap();
        // Mark it terminal so it is NOT counted active (so the guard passes even
        // before removal — proving the guard counts only the target here).
        team.update_status(&target, WorkerStatus::Completed).await;

        // Seed the on-disk team + task dirs that cleanup must remove.
        std::fs::create_dir_all(team_file::team_dir(&home, "alpha")).unwrap();
        std::fs::write(team_file::team_file_path(&home, "alpha"), "{}").unwrap();
        std::fs::create_dir_all(team_file::task_dir(&home, "alpha")).unwrap();
        assert!(team_file::team_file_exists(&home, "alpha"));

        let mode = Arc::new(CoordinatorMode::new());
        mode.enter();
        let tool = TeamDeleteTool::new(
            team.clone(),
            mode,
            Arc::new(RecordingSeam::new()) as Arc<dyn TeamSpawnSeam>,
        )
        .with_analytics_bus(Some(bus))
        .with_home(home.clone());

        let res = tool
            .call(
                json!({ "agent_id": target.as_uuid().to_string() }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("delete of last worker succeeds");

        assert_eq!(res.data["success"], true);
        assert_eq!(res.data["team_name"], "alpha");
        // Worker removed.
        assert!(team.list().await.is_empty(), "last worker removed");
        // Team name cleared (set_team_name(None)).
        assert_eq!(team.team_name().await, None, "team context cleared");
        // Directories removed.
        assert!(
            !team_file::team_dir(&home, "alpha").exists(),
            "team dir removed"
        );
        assert!(
            !team_file::task_dir(&home, "alpha").exists(),
            "task dir removed"
        );

        // Telemetry fired.
        let events = sink.events().await;
        let ev = events
            .iter()
            .find(|e| e.name == TEAM_DELETED)
            .expect("tengu_team_deleted must be emitted");
        assert!(ev.metadata.contains_key("_PROTO_team_name"));
    }
}

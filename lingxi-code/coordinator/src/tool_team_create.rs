//! `TeamCreateTool` — coordinator-only tool, 1:1 Rust port of the TS
//! `TeamCreateTool` (`src/tools/TeamCreateTool/`).
//!
//! TS name: `TeamCreate` (`TEAM_CREATE_TOOL_NAME`). In the lingxi coordinator
//! the team-file / `AppState` machinery collapses onto [`TeamRegistry`]: creating
//! a "team" registers/spawns a worker via
//! [`TeamRegistry::spawn_worker`](crate::team_registry::TeamRegistry::spawn_worker)
//! and returns the freshly-minted agent id. That agent id IS the `task_id` the
//! coordinator subsequently uses with `SendMessage` to continue the worker.
//!
//! The tool holds an `Arc<TeamRegistry>` directly (no `BuiltinToolContext`,
//! no telemetry) per the coordinator-tool pattern.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde_json::{json, Value};

use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, InterruptBehavior, PromptOptions, Tool, ToolCallResult, ToolError,
    ToolStaticContext, ValidationError,
};
use traits::team_spawn::TeamSpawnSeam;
use traits::OutputStream;

use crate::mode::CoordinatorMode;
use crate::team_registry::{TeamRegistry, WorkerStatus};

/// Canonical tool name — mirrors the TS `TEAM_CREATE_TOOL_NAME` constant
/// (`src/tools/TeamCreateTool/constants.ts`).
pub const TEAM_CREATE_TOOL_NAME: &str = "TeamCreate";

/// Feature-flag key gating the swarm/team tools. Mirrors the TS
/// `isAgentSwarmsEnabled()` enablement check; absence defaults to enabled.
const AGENT_SWARMS_FLAG: &str = "agent_swarms_enabled";

/// Backing store for the cached input schema (built once, on first access).
static TEAM_CREATE_SCHEMA: OnceLock<Value> = OnceLock::new();

/// 1:1 with the TS `z.strictObject({...})` input schema. `team_name` is the
/// only required field; `description` and `agent_type` are optional. Extra keys
/// are rejected (`additionalProperties: false`).
fn team_create_schema() -> &'static Value {
    TEAM_CREATE_SCHEMA.get_or_init(|| {
        json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["team_name"],
            "properties": {
                "team_name": {
                    "type": "string",
                    "description": "Name for the new team to create."
                },
                "description": {
                    "type": "string",
                    "description": "Team description/purpose."
                },
                "agent_type": {
                    "type": "string",
                    "description": "Type/role of the team lead (e.g., \"researcher\", \"test-runner\"). Used for team file and inter-agent coordination."
                }
            }
        })
    })
}

/// Coordinator-only `TeamCreate` tool. Holds a shared [`TeamRegistry`] handle,
/// the coordinator [`CoordinatorMode`] gate, the [`TeamSpawnSeam`] used to start
/// the real teammate task, and a cached input-schema `Value`.
///
/// `mode` and `spawn_seam` are threaded here in T03 so the factory can wire the
/// full dependency set; they are consumed by `call()` in T05 (the real-spawn /
/// mode-gate behavior). Storing them now keeps the assembly seam stable.
pub struct TeamCreateTool {
    team: Arc<TeamRegistry>,
    mode: Arc<CoordinatorMode>,
    spawn_seam: Arc<dyn TeamSpawnSeam>,
    /// The orchestrator-facing PUSH sink. After a spawn is fully reconciled
    /// (worker registered, teammate started, worker↔`task_id` link written),
    /// `call()` emits the live active-worker count through this so every client
    /// learns `active_workers > 0` deterministically — independent of the
    /// teammate's own racy startup `Running` emit (which is dispatched on a
    /// concurrent task and can fire before the link is written, leaving a
    /// status sink keyed on the not-yet-written `task_id` unable to resolve it).
    output: Arc<dyn OutputStream>,
}

impl TeamCreateTool {
    /// Construct a `TeamCreate` tool wired to the shared coordinator registry,
    /// the mode gate, the teammate spawn seam, and the orchestrator-facing
    /// output stream used to PUSH the live active-worker count after a spawn.
    #[must_use]
    pub fn new(
        team: Arc<TeamRegistry>,
        mode: Arc<CoordinatorMode>,
        spawn_seam: Arc<dyn TeamSpawnSeam>,
        output: Arc<dyn OutputStream>,
    ) -> Self {
        Self {
            team,
            mode,
            spawn_seam,
            output,
        }
    }
}

#[async_trait]
impl Tool for TeamCreateTool {
    fn name(&self) -> &str {
        TEAM_CREATE_TOOL_NAME
    }

    fn input_schema(&self) -> &Value {
        team_create_schema()
    }

    fn is_enabled(&self, ctx: &ToolStaticContext) -> bool {
        // TS: isEnabled() = isAgentSwarmsEnabled(). Map onto the static feature
        // flag; default to enabled when the host did not set the flag.
        ctx.feature_flags
            .get(AGENT_SWARMS_FLAG)
            .copied()
            .unwrap_or(true)
    }

    fn max_result_size_chars(&self) -> usize {
        // TS: maxResultSizeChars: 100_000.
        100_000
    }

    fn is_concurrency_safe(&self, _: &Value) -> bool {
        // Registry mutation is `RwLock`-guarded, so concurrent spawns are safe.
        true
    }

    fn is_read_only(&self, _: &Value) -> bool {
        false
    }

    fn is_destructive(&self, _: &Value) -> bool {
        false
    }

    fn is_open_world(&self, _: &Value) -> bool {
        false
    }

    fn should_defer(&self) -> bool {
        // TS: shouldDefer: true.
        true
    }

    fn search_hint(&self) -> Option<&str> {
        // TS: searchHint: 'create a multi-agent swarm team'.
        Some("create a multi-agent swarm team")
    }

    fn interrupt_behavior(&self, _: &Value) -> InterruptBehavior {
        InterruptBehavior::Block
    }

    async fn validate_input(
        &self,
        input: &Value,
        _ctx: &ToolUseContext,
    ) -> Result<(), ValidationError> {
        // TS validateInput: reject empty/whitespace team_name (errorCode 9).
        let team_name = input.get("team_name").and_then(Value::as_str);
        match team_name {
            Some(name) if !name.trim().is_empty() => Ok(()),
            _ => Err(ValidationError(
                "team_name is required for TeamCreate".to_string(),
            )),
        }
    }

    async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
        // TS TeamCreate has no checkPermissions override -> allow.
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other {
                reason: "TeamCreate registers a coordinator-owned worker".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        // TS: description() returns this exact string.
        "Create a new team for coordinating multiple agents".into()
    }

    async fn prompt(&self, _: &PromptOptions) -> String {
        // Condensed from the TS getPrompt(); the framing (Team = TaskList,
        // spawn teammates, coordinate via SendMessage) is preserved.
        "Create a new team to coordinate multiple agents working on a project. \
         Use this proactively when the user asks to use a team/swarm or when a \
         task benefits from parallel work by multiple agents. Teams have a 1:1 \
         correspondence with task lists (Team = TaskList). Creating a team \
         registers a coordinator-owned worker and returns its agent id, which is \
         the id you later pass to SendMessage to continue that worker."
            .into()
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        _progress: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        // 1. Mode early-return (defense-in-depth). Lets a future `/coordinator
        //    exit()` neutralize the tool without a registry rebuild. This is
        //    net-new in call() — distinct from `is_enabled`'s static feature
        //    flag, which only gates whether the tool is advertised at all.
        if !self.mode.is_enabled() {
            return Err(ToolError::InvalidInput("coordinator mode not active".into()));
        }

        // Parse defensively (do not rely on schema validation alone), matching
        // the builtin pattern. `team_name` is required; `agent_type` optional
        // (TS lead agent type defaults to TEAM_LEAD_NAME = "team-lead").
        let team_name = input
            .get("team_name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ToolError::InvalidInput("team_name is required for TeamCreate".into()))?
            .to_string();

        let agent_type = input
            .get("agent_type")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or("team-lead")
            .to_string();

        // Optional free-form team description/purpose; threaded into the
        // teammate spawn so the handler can seed the worker's context.
        let description = input
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();

        // 2. Register/spawn the worker metadata. The registry mints the AgentId,
        //    registers a mailbox, and inserts a WorkerAgent (status Idle). The
        //    worker's name is the team name; task_id starts empty and is written
        //    back below from the handler-generated id.
        let agent_id = self
            .team
            .spawn_worker(agent_type, team_name.clone(), String::new())
            .await
            .map_err(|e| ToolError::Internal(format!("TeamCreate: {e}")))?;

        // 3. Start the REAL InProcessTeammate task via the spawn seam. Returns
        //    the handler-generated task_id (distinct from the worker AgentId).
        let task_id = self
            .spawn_seam
            .spawn_teammate(agent_id, team_name.clone(), description)
            .await
            .map_err(|e| ToolError::Internal(format!("TeamCreate: {e}")))?;

        // 4. Reconcile the two id spaces: key the worker↔task link on the
        //    handler-returned id so the status sink (find_by_task_id) and
        //    TeamDelete (kill) can resolve back to this worker.
        self.team.set_task_id(&agent_id, task_id.clone()).await;

        // 5. Record the team name (source of the CoordinatorStatus { team } DTO).
        self.team.set_team_name(Some(team_name.clone())).await;

        // 5a. Mark the freshly-spawned, now-linked worker `Working` and PUSH the
        //     live active-worker count to every client. This is the deterministic
        //     activation point: by here the teammate task is confirmed started
        //     (`spawn_teammate` returned) and the worker↔task_id link is written,
        //     so the count is authoritative. We drive it from the tool rather
        //     than relying on the teammate's own startup `Running` emit: that
        //     emit runs on a CONCURRENT runtime task and can fire before this
        //     `call()` writes the link (step 4), in which case the
        //     `CoordinatorStatusSink` — keyed on the still-unwritten `task_id` —
        //     silently drops it and no client ever learns `active_workers > 0`.
        //     The sink remains authoritative for the later terminal transitions
        //     (`Failed` / `Killed`) and their pushes; this initial push is
        //     idempotent w.r.t. a sink `Running` that happens to land afterward
        //     (same count).
        self.team
            .update_status(
                &agent_id,
                WorkerStatus::Working {
                    activity: "running".to_string(),
                },
            )
            .await;
        let active = self.team.active_worker_count().await;
        let pushed_team = self.team.team_name().await;
        self.output
            .emit_coordinator_status(active, pushed_team.as_deref())
            .await;

        // 6. Return the worker agent id (model-facing lead id) and the REAL
        //    handler-generated task_id (replaces the old task_id == agent_id
        //    placeholder).
        let agent_id_str = agent_id.as_uuid().to_string();
        Ok(ToolCallResult {
            data: json!({
                "team_name": team_name,
                "lead_agent_id": agent_id_str,
                "task_id": task_id,
                "spawned": true,
            }),
            new_messages: Vec::new(),
            context_modifier: None,
            mcp_meta: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::AgentId;
    use tool_api::context::{ToolUseContext, ToolUseOptions};
    use tool_api::progress::progress_channel;

    // Local test-context builders (mirrors the sibling coordinator tools'
    // pattern; avoids depending on the `tool-api` `test-support` feature).
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

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    /// Recording spawn seam: returns a fixed handler-generated `task_id`,
    /// counts `spawn_teammate` invocations, and captures the args of the last
    /// spawn so tests can assert what the tool threaded through.
    struct RecordingSeam {
        task_id: String,
        spawns: AtomicUsize,
        last_args: Mutex<Option<(AgentId, String, String)>>,
    }

    impl RecordingSeam {
        fn new(task_id: impl Into<String>) -> Self {
            Self {
                task_id: task_id.into(),
                spawns: AtomicUsize::new(0),
                last_args: Mutex::new(None),
            }
        }
    }

    #[async_trait]
    impl TeamSpawnSeam for RecordingSeam {
        async fn spawn_teammate(
            &self,
            agent_id: AgentId,
            name: String,
            description: String,
        ) -> Result<String, traits::team_spawn::TeamSpawnError> {
            self.spawns.fetch_add(1, Ordering::SeqCst);
            *self.last_args.lock().unwrap() = Some((agent_id, name, description));
            Ok(self.task_id.clone())
        }
        async fn kill(&self, _task_id: &str) -> Result<(), traits::team_spawn::TeamSpawnError> {
            Ok(())
        }
    }

    /// Spy [`OutputStream`] recording every `emit_coordinator_status` call as
    /// `(active_workers, team)`; the four required callbacks are inert no-ops.
    #[derive(Default)]
    struct SpyOutput {
        statuses: Mutex<Vec<(u32, Option<String>)>>,
    }

    impl SpyOutput {
        fn last(&self) -> Option<(u32, Option<String>)> {
            self.statuses.lock().unwrap().last().cloned()
        }
        fn calls(&self) -> usize {
            self.statuses.lock().unwrap().len()
        }
    }

    #[async_trait]
    impl OutputStream for SpyOutput {
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
        async fn emit_coordinator_status(&self, active_workers: u32, team: Option<&str>) {
            self.statuses
                .lock()
                .unwrap()
                .push((active_workers, team.map(str::to_string)));
        }
    }

    /// Build a `TeamCreate` tool with coordinator mode ENABLED (the normal path),
    /// a recording seam returning the given handler `task_id`, and a spy output
    /// the test can inspect for the activation PUSH.
    fn make_tool_with_seam_and_spy(
        seam: Arc<RecordingSeam>,
    ) -> (
        TeamCreateTool,
        Arc<TeamRegistry>,
        Arc<CoordinatorMode>,
        Arc<SpyOutput>,
    ) {
        let registry = Arc::new(TeamRegistry::new(AgentId::new()));
        let mode = Arc::new(CoordinatorMode::new());
        mode.enter();
        let spy = Arc::new(SpyOutput::default());
        let tool = TeamCreateTool::new(
            registry.clone(),
            mode.clone(),
            seam as Arc<dyn TeamSpawnSeam>,
            spy.clone() as Arc<dyn OutputStream>,
        );
        (tool, registry, mode, spy)
    }

    /// Build a `TeamCreate` tool with coordinator mode ENABLED (the normal path)
    /// and a recording seam returning the given handler `task_id`.
    fn make_tool_with_seam(
        seam: Arc<RecordingSeam>,
    ) -> (TeamCreateTool, Arc<TeamRegistry>, Arc<CoordinatorMode>) {
        let (tool, registry, mode, _spy) = make_tool_with_seam_and_spy(seam);
        (tool, registry, mode)
    }

    fn make_tool() -> (TeamCreateTool, Arc<TeamRegistry>) {
        let seam = Arc::new(RecordingSeam::new("task-handler-id"));
        let (tool, registry, _mode) = make_tool_with_seam(seam);
        (tool, registry)
    }

    #[test]
    fn name_matches_ts_constant() {
        let (tool, _registry) = make_tool();
        assert_eq!(tool.name(), "TeamCreate");
        assert_eq!(tool.name(), TEAM_CREATE_TOOL_NAME);
    }

    #[test]
    fn schema_is_strict_object_with_required_team_name() {
        let (tool, _registry) = make_tool();
        let schema = tool.input_schema();
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["required"], json!(["team_name"]));
        assert_eq!(schema["properties"]["team_name"]["type"], "string");
        assert_eq!(schema["properties"]["description"]["type"], "string");
        assert_eq!(schema["properties"]["agent_type"]["type"], "string");
    }

    #[test]
    fn flag_metadata_defaults() {
        let (tool, _registry) = make_tool();
        assert!(tool.is_enabled(&ToolStaticContext::default()));
        assert!(tool.should_defer());
        assert!(!tool.is_read_only(&json!({})));
        assert!(!tool.is_destructive(&json!({})));
        assert_eq!(tool.max_result_size_chars(), 100_000);
    }

    #[tokio::test]
    async fn enabled_respects_feature_flag_off() {
        let (tool, _registry) = make_tool();
        let mut ctx = ToolStaticContext::default();
        ctx.feature_flags
            .insert(AGENT_SWARMS_FLAG.to_string(), false);
        assert!(!tool.is_enabled(&ctx));
    }

    #[tokio::test]
    async fn call_spawns_worker_and_returns_agent_id() {
        let (tool, registry) = make_tool();
        assert!(registry.list().await.is_empty(), "registry starts empty");

        let res = tool
            .call(
                json!({ "team_name": "alpha-team", "agent_type": "researcher" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("TeamCreate must succeed on valid input");

        assert_eq!(res.data["team_name"], "alpha-team");
        assert_eq!(res.data["spawned"], true);
        let lead = res.data["lead_agent_id"]
            .as_str()
            .expect("lead_agent_id must be a string");
        // task_id is now the handler-generated id (T05), NOT the agent id.
        assert_eq!(res.data["task_id"], "task-handler-id");
        assert_ne!(res.data["task_id"].as_str().unwrap(), lead);

        // Effect: the registry now lists exactly one worker matching the input.
        let workers = registry.list().await;
        assert_eq!(workers.len(), 1, "exactly one worker spawned");
        let w = &workers[0];
        assert_eq!(w.name, "alpha-team");
        assert_eq!(w.agent_type, "researcher");
        assert_eq!(w.agent_id.as_uuid().to_string(), lead);
    }

    #[tokio::test]
    async fn call_defaults_agent_type_to_team_lead() {
        let (tool, registry) = make_tool();
        tool.call(json!({ "team_name": "beta" }), fresh_ctx(), fresh_tx())
            .await
            .expect("valid call");
        let workers = registry.list().await;
        assert_eq!(workers.len(), 1);
        assert_eq!(workers[0].agent_type, "team-lead");
    }

    #[tokio::test]
    async fn call_rejects_missing_team_name() {
        let (tool, registry) = make_tool();
        let err = tool
            .call(json!({ "agent_type": "x" }), fresh_ctx(), fresh_tx())
            .await
            .expect_err("missing team_name must fail");
        assert!(matches!(err, ToolError::InvalidInput(_)));
        assert_eq!(
            format!("{err}"),
            "invalid input: team_name is required for TeamCreate"
        );
        assert!(
            registry.list().await.is_empty(),
            "no worker spawned on bad input"
        );
    }

    #[tokio::test]
    async fn call_rejects_blank_team_name() {
        let (tool, registry) = make_tool();
        let err = tool
            .call(json!({ "team_name": "   " }), fresh_ctx(), fresh_tx())
            .await
            .expect_err("whitespace team_name must fail");
        assert!(matches!(err, ToolError::InvalidInput(_)));
        assert!(registry.list().await.is_empty());
    }

    #[tokio::test]
    async fn validate_input_matches_ts_message() {
        let (tool, _registry) = make_tool();
        let ctx = fresh_ctx();
        assert!(tool
            .validate_input(&json!({ "team_name": "ok" }), &ctx)
            .await
            .is_ok());
        let err = tool
            .validate_input(&json!({ "team_name": "  " }), &ctx)
            .await
            .expect_err("blank must fail validation");
        assert_eq!(err.0, "team_name is required for TeamCreate");
    }

    // ---- T05: mode gate + real spawn + task_id write-back + team_name ----

    #[tokio::test]
    async fn call_when_mode_disabled_errors() {
        // Mode OFF: the call() early-return fires before any spawn.
        let registry = Arc::new(TeamRegistry::new(AgentId::new()));
        let mode = Arc::new(CoordinatorMode::new()); // disabled by default
        let seam = Arc::new(RecordingSeam::new("task-handler-id"));
        let spy = Arc::new(SpyOutput::default());
        let tool = TeamCreateTool::new(
            registry.clone(),
            mode,
            seam.clone() as Arc<dyn TeamSpawnSeam>,
            spy.clone() as Arc<dyn OutputStream>,
        );

        let err = tool
            .call(
                json!({ "team_name": "alpha-team", "agent_type": "researcher" }),
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

        // No worker spawned, seam never invoked, team name untouched, and the
        // mode-gate early-return fired BEFORE any activation PUSH.
        assert!(registry.list().await.is_empty(), "no worker on disabled mode");
        assert_eq!(seam.spawns.load(Ordering::SeqCst), 0);
        assert_eq!(registry.team_name().await, None);
        assert_eq!(spy.calls(), 0, "no CoordinatorStatus push when mode is off");
    }

    #[tokio::test]
    async fn call_spawns_worker_and_writes_back_task_id() {
        let seam = Arc::new(RecordingSeam::new("handler-task-42"));
        let (tool, registry, _mode) = make_tool_with_seam(seam.clone());

        let res = tool
            .call(
                json!({ "team_name": "alpha-team", "agent_type": "researcher", "description": "do work" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("TeamCreate must succeed when mode is enabled");

        // Exactly one worker, its task_id reconciled to the seam-returned id.
        let workers = registry.list().await;
        assert_eq!(workers.len(), 1, "exactly one worker spawned");
        let w = &workers[0];
        assert_eq!(w.task_id, "handler-task-42", "task_id is the handler id");
        assert_ne!(w.task_id, "", "task_id is NOT empty");
        assert_ne!(
            w.task_id,
            w.agent_id.as_uuid().to_string(),
            "task_id is NOT the agent_id"
        );

        // team_name set on the registry.
        assert_eq!(registry.team_name().await, Some("alpha-team".to_string()));

        // Result surfaces both the worker agent id and the real task id.
        let lead = res.data["lead_agent_id"].as_str().unwrap();
        assert_eq!(lead, w.agent_id.as_uuid().to_string());
        assert_eq!(res.data["task_id"], "handler-task-42");

        // The description was threaded through to the seam.
        let (seam_agent_id, seam_name, seam_desc) =
            seam.last_args.lock().unwrap().clone().expect("seam called");
        assert_eq!(seam_agent_id, w.agent_id);
        assert_eq!(seam_name, "alpha-team");
        assert_eq!(seam_desc, "do work");
    }

    #[tokio::test]
    async fn call_invokes_spawn_seam_once() {
        let seam = Arc::new(RecordingSeam::new("handler-task-1"));
        let (tool, _registry, _mode) = make_tool_with_seam(seam.clone());

        tool.call(
            json!({ "team_name": "beta" }),
            fresh_ctx(),
            fresh_tx(),
        )
        .await
        .expect("valid call");

        assert_eq!(
            seam.spawns.load(Ordering::SeqCst),
            1,
            "spawn_teammate invoked exactly once"
        );
    }

    /// T15 fix: after a successful spawn, `call()` deterministically transitions
    /// the now-linked worker to `Working` and PUSHES the live active-worker
    /// count to the orchestrator-facing output stream — independent of the
    /// teammate's own racy startup status emit. This is the activation signal
    /// every client observes.
    #[tokio::test]
    async fn call_transitions_worker_to_working_and_pushes_active_count() {
        let seam = Arc::new(RecordingSeam::new("handler-task-7"));
        let (tool, registry, _mode, spy) = make_tool_with_seam_and_spy(seam);

        tool.call(
            json!({ "team_name": "alpha", "agent_type": "researcher" }),
            fresh_ctx(),
            fresh_tx(),
        )
        .await
        .expect("valid call");

        // The worker is Working (deterministic Idle -> Working), not stranded Idle.
        let workers = registry.list().await;
        assert_eq!(workers.len(), 1);
        assert_eq!(
            workers[0].status,
            WorkerStatus::Working {
                activity: "running".to_string()
            },
            "the spawned worker is deterministically Working after call()"
        );

        // Exactly one activation PUSH carrying active_workers >= 1 + the team name.
        assert_eq!(spy.calls(), 1, "exactly one CoordinatorStatus push on spawn");
        assert_eq!(
            spy.last(),
            Some((1, Some("alpha".to_string()))),
            "the push carries the live active-worker count and team name"
        );
    }
}

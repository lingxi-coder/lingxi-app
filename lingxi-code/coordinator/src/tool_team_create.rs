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

use crate::team_registry::TeamRegistry;

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

/// Coordinator-only `TeamCreate` tool. Holds a shared [`TeamRegistry`] handle
/// and a cached input-schema `Value`.
pub struct TeamCreateTool {
    team: Arc<TeamRegistry>,
}

impl TeamCreateTool {
    /// Construct a `TeamCreate` tool wired to the shared coordinator registry.
    #[must_use]
    pub fn new(team: Arc<TeamRegistry>) -> Self {
        Self { team }
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

        // Register/spawn the worker. The registry mints the AgentId, registers a
        // mailbox, and inserts a WorkerAgent (status Idle). The worker's name is
        // the team name; task_id starts empty (assigned later by the coordinator).
        let task_id = String::new();
        let agent_id = self
            .team
            .spawn_worker(agent_type, team_name.clone(), task_id)
            .await
            .map_err(|e| ToolError::Internal(format!("TeamCreate: {e}")))?;

        // Return the new agent id as both the model-facing `lead_agent_id` and
        // the `task_id` the coordinator uses with SendMessage to continue it.
        let agent_id_str = agent_id.as_uuid().to_string();
        Ok(ToolCallResult {
            data: json!({
                "team_name": team_name,
                "lead_agent_id": agent_id_str,
                "task_id": agent_id_str,
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

    fn make_tool() -> (TeamCreateTool, Arc<TeamRegistry>) {
        let registry = Arc::new(TeamRegistry::new(AgentId::new()));
        let tool = TeamCreateTool::new(registry.clone());
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
        // task_id mirrors the agent id (the SendMessage continuation handle).
        assert_eq!(res.data["task_id"], lead);

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
}

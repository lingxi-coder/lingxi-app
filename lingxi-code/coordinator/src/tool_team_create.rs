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

use telemetry::sink::{AnalyticsValue, LogEventMetadata};
use telemetry::tengu::coordinator::TEAM_CREATED;
use telemetry::AnalyticsBus;
use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, InterruptBehavior, PromptOptions, Tool, ToolCallResult, ToolError,
    ToolStaticContext, ValidationError,
};
use traits::team_spawn::TeamSpawnSeam;
use traits::{OutputStream, RuntimeSpawner};

use crate::mode::CoordinatorMode;
use crate::team_file::{self, TeamFile, TeamMember};
use crate::team_registry::{TeamRegistry, WorkerStatus};

/// The lead member's name (TS `TEAM_LEAD_NAME = "team-lead"`,
/// `utils/swarm/constants.ts:1`).
const TEAM_LEAD_NAME: &str = "team-lead";

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
    /// Optional analytics bus for `tengu_team_created`. `None` (the default for
    /// tests) ⇒ the event is logged via `tracing` only. Threaded additively so
    /// existing call sites that don't carry a bus keep compiling.
    bus: Option<Arc<AnalyticsBus>>,
    /// Optional `~/.claude` root override. `None` (production) ⇒ resolve from
    /// `$HOME` via [`team_file::lingxi_home`]. Tests inject a tempdir so the
    /// on-disk team file is written under a scratch path (hermetic, no real
    /// `~/.lingxi/teams/` pollution, no cross-test interference).
    home_override: Option<std::path::PathBuf>,
    /// Optional background-task spawner (D17 — no direct `tokio::spawn`) used to
    /// start the per-teammate mailbox→runner PUMP right after a spawn is
    /// reconciled. `None` (the default for tests / the offline factory) ⇒ no
    /// pump is started: routed messages still queue in the teammate's mailbox,
    /// they just are not auto-drained into the runner. The desktop composition
    /// root injects a real `PosixRuntime` via [`Self::with_runtime`] so a
    /// coordinator `SendMessage` actually reaches the teammate's turn loop.
    runtime: Option<Arc<dyn RuntimeSpawner>>,
}

impl TeamCreateTool {
    /// Construct a `TeamCreate` tool wired to the shared coordinator registry,
    /// the mode gate, the teammate spawn seam, and the orchestrator-facing
    /// output stream used to PUSH the live active-worker count after a spawn.
    /// No analytics bus is attached (telemetry falls back to `tracing`); use
    /// [`Self::with_analytics_bus`] to wire one.
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
            bus: None,
            home_override: None,
            runtime: None,
        }
    }

    /// Attach an analytics bus so `call()` fires `tengu_team_created`.
    #[must_use]
    pub fn with_analytics_bus(mut self, bus: Option<Arc<AnalyticsBus>>) -> Self {
        self.bus = bus;
        self
    }

    /// Attach the background-task spawner used to start the per-teammate
    /// mailbox→runner PUMP after a spawn is reconciled. Production (desktop)
    /// wires the session `RuntimeSpawner` here; without it, routed messages
    /// still queue in the teammate mailbox but are not auto-drained into the
    /// runner.
    #[must_use]
    pub fn with_runtime(mut self, runtime: Arc<dyn RuntimeSpawner>) -> Self {
        self.runtime = Some(runtime);
        self
    }

    /// Override the `~/.claude` root the team file is written under (tests use a
    /// tempdir for hermeticity). Production leaves this unset → resolves `$HOME`.
    #[must_use]
    pub fn with_home(mut self, home: std::path::PathBuf) -> Self {
        self.home_override = Some(home);
        self
    }

    /// Fire `tengu_team_created { team_name, teammate_count: 1, lead_agent_type,
    /// teammate_mode }` (TeamCreateTool.ts:214-222). Logs through the attached
    /// bus when present, else falls back to `tracing`. `team_name` /
    /// `lead_agent_type` are user-derived → routed through the PII-tagged /
    /// verified columns like the in-tree `tool_team` emitters.
    async fn emit_team_created(&self, team_name: &str, lead_agent_type: &str) {
        if let Some(bus) = &self.bus {
            let mut md: LogEventMetadata = LogEventMetadata::new();
            md.insert(
                "_PROTO_team_name".into(),
                AnalyticsValue::String(
                    telemetry::pii::PiiTagged::assert_pii_tagged_column(team_name.to_string())
                        .into_inner(),
                ),
            );
            md.insert("teammate_count".into(), AnalyticsValue::Int(1));
            md.insert(
                "lead_agent_type".into(),
                AnalyticsValue::String(
                    telemetry::pii::Verified::assert_safe(lead_agent_type.to_string()).into_inner(),
                ),
            );
            // The lingxi coordinator runs InProcessTeammate exclusively
            // (getResolvedTeammateMode analog → "in-process").
            md.insert(
                "teammate_mode".into(),
                AnalyticsValue::String("in-process".to_string()),
            );
            bus.log_event(TEAM_CREATED, md).await;
        } else {
            tracing::info!(
                event = TEAM_CREATED,
                team_name,
                teammate_count = 1,
                lead_agent_type,
                teammate_mode = "in-process",
                "team created"
            );
        }
    }
}

/// `generateUniqueTeamName` (TeamCreateTool.ts:60-72): if no team file exists for
/// `requested`, use it as-is; otherwise auto-rename (NOT an error) by appending a
/// short unique suffix until a free name is found. TS generates a fresh word
/// slug; here a `{requested}-{suffix}` form keeps the user's name recognizable
/// while guaranteeing uniqueness. Bounded retry loop with a final
/// timestamp-suffixed fallback so it always terminates.
fn generate_unique_team_name(home: &std::path::Path, requested: &str) -> String {
    if !team_file::team_file_exists(home, requested) {
        return requested.to_string();
    }
    for _ in 0..16 {
        let suffix = &tool_api::util::ids::ulid_or_uuid()[..6];
        let candidate = format!("{requested}-{suffix}");
        if !team_file::team_file_exists(home, &candidate) {
            return candidate;
        }
    }
    // Exceedingly unlikely fallback: a full-id suffix is effectively unique.
    format!("{requested}-{}", tool_api::util::ids::ulid_or_uuid())
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

    // The spawn → reconcile → disk-file → telemetry → activation-push sequence is
    // inherently linear; splitting it would obscure the ordered side effects.
    #[allow(clippy::too_many_lines)]
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
            return Err(ToolError::InvalidInput(
                "coordinator mode not active".into(),
            ));
        }

        // 1a. One-team-per-leader guard (TeamCreateTool.ts:132-140). If this
        //     coordinator is already leading a team, refuse — a leader manages
        //     exactly one team at a time. The registry's `team_name` is the
        //     lingxi analog of TS `appState.teamContext?.teamName`.
        if let Some(existing) = self.team.team_name().await {
            return Err(ToolError::InvalidInput(format!(
                "Already leading team \"{existing}\". A leader can only manage one team at a time. \
                 Use TeamDelete to end the current team before creating a new one."
            )));
        }

        // Parse defensively (do not rely on schema validation alone), matching
        // the builtin pattern. `team_name` is required; `agent_type` optional
        // (TS lead agent type defaults to TEAM_LEAD_NAME = "team-lead").
        let requested_name = input
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
            .unwrap_or(TEAM_LEAD_NAME)
            .to_string();

        // Optional free-form team description/purpose; threaded into the
        // teammate spawn so the handler can seed the worker's context.
        let description = input
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        // `description` is moved into `spawn_teammate` below; keep a copy for the
        // on-disk team file's `description` field.
        let description_for_file = description.clone();

        // 1b. Unique-name-on-collision (TeamCreateTool.ts:60-72,143
        //     generateUniqueTeamName). If a team file with this name already
        //     exists on disk, AUTO-RENAME (NOT an error) by appending a short
        //     unique suffix until a free name is found. When `$HOME` is unset we
        //     cannot probe disk — fall through with the requested name.
        let home = self.home_override.clone().or_else(team_file::lingxi_home);
        let team_name = match &home {
            Some(h) => generate_unique_team_name(h, &requested_name),
            None => requested_name.clone(),
        };

        // 2. Register/spawn the worker metadata. The registry mints the AgentId,
        //    registers a mailbox, and inserts a WorkerAgent (status Idle). The
        //    worker's name is the team name; task_id starts empty and is written
        //    back below from the handler-generated id.
        let agent_id = self
            .team
            .spawn_worker(agent_type.clone(), team_name.clone(), String::new())
            .await
            .map_err(|e| ToolError::Internal(format!("TeamCreate: {e}")))?;

        // 3. Start the REAL InProcessTeammate task via the spawn seam. Returns
        //    the handler-generated task_id (distinct from the worker AgentId).
        //    The teammate's DISPLAY name and TEAM name are threaded so its
        //    dispatched tools resolve `getAgentName()` /
        //    `getTeammateContext()?.teamName` (the swarm `TaskUpdate` side-effects
        //    key on them). In this single-worker-per-team design the worker's
        //    display name IS the team name (step 2 uses `team_name` for both), so
        //    both args carry `team_name`.
        let task_id = self
            .spawn_seam
            .spawn_teammate(agent_id, team_name.clone(), team_name.clone(), description)
            .await
            .map_err(|e| ToolError::Internal(format!("TeamCreate: {e}")))?;

        // 4. Reconcile the two id spaces: key the worker↔task link on the
        //    handler-returned id so the status sink (find_by_task_id) and
        //    TeamDelete (kill) can resolve back to this worker.
        self.team.set_task_id(&agent_id, task_id.clone()).await;

        // 4a. Start the per-teammate mailbox→runner PUMP. The teammate's mailbox
        //     was registered by `spawn_worker` (step 2); now that the worker↔
        //     task_id link is written and the real task is started (step 3), the
        //     pump can park on that mailbox and inject each delivered message
        //     into the teammate's turn loop via the spawn seam's `send_message`
        //     (the `injectUserMessageToTeammate` analogue). WITHOUT this, a
        //     coordinator `SendMessage` lands in a mailbox no runner reads.
        //
        //     The pump runs through the injected `RuntimeSpawner` (D17 — never a
        //     direct `tokio::spawn`). When no runtime is wired (tests / the
        //     offline factory) the pump is skipped: messages still queue in the
        //     mailbox, they are just not auto-drained. The mailbox lookup is
        //     resolved here (not inside the pump) so a missing mailbox — which
        //     should never happen right after `spawn_worker` — is a no-op rather
        //     than a parked pump on a phantom inbox. The pump exits on its own
        //     when the teammate's task is gone (`send_message` → Terminated), so
        //     its lifetime is tied to the teammate; no stop handle is retained.
        if let Some(runtime) = &self.runtime {
            if let Some(mailbox) = self.team.mailbox_router.get(&agent_id).await {
                let seam = self.spawn_seam.clone();
                let pump_task_id = task_id.clone();
                let pump = Box::pin(async move {
                    crate::teammate_pump::run_teammate_pump(mailbox, pump_task_id, seam).await;
                });
                // A spawn failure (runtime shutting down) is non-fatal to the
                // TeamCreate itself — the teammate is already started; only its
                // auto-drain pump failed to launch. Log and continue.
                if let Err(e) = runtime
                    .spawn(&format!("teammate-pump:{task_id}"), pump)
                    .await
                {
                    tracing::warn!(
                        task_id = %task_id,
                        error = %e,
                        "TeamCreate: failed to start teammate mailbox→runner pump"
                    );
                }
            } else {
                tracing::warn!(
                    agent_id = %agent_id.as_uuid(),
                    "TeamCreate: no mailbox registered for freshly-spawned worker; pump not started"
                );
            }
        }

        // 5. Record the team name (source of the CoordinatorStatus { team } DTO).
        self.team.set_team_name(Some(team_name.clone())).await;

        // 5-bis. Record the LEADER team name in the process-global slot
        //        (claude-code `setLeaderTeamName`, TeamCreateTool.ts via
        //        `utils/tasks.ts:31`). `getTaskListId()` consults this (priority
        //        4) so the leader's V2 tasks land under the team name — the same
        //        on-disk directory its in-process teammates resolve to — instead
        //        of under the session id.
        traits::team_registry::set_leader_team_name(&team_name);

        // 5b. Write the on-disk team file `~/.lingxi/teams/{name}/config.json`
        //     mirroring the TS `TeamFile` shape (TeamCreateTool.ts:157-177 →
        //     teamHelpers.ts:175-182). Best-effort: a write failure is surfaced
        //     as a `warning` on the result but does NOT abort the spawn (the
        //     in-memory registry is the source of truth for live routing). The
        //     lead member is the freshly-spawned worker; `leadSessionId` is the
        //     coordinator session id when the host threads one through (we do not
        //     have it here, so it is omitted — TS stores `getSessionId()`).
        let agent_id_str = agent_id.as_uuid().to_string();
        let lead_agent_id = format!("{TEAM_LEAD_NAME}@{team_name}");
        let mut team_file_warning: Option<String> = None;
        let team_file_path = if let Some(h) = &home {
            let now = team_file::now_unix_millis();
            let cwd = std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            let file = TeamFile {
                name: team_name.clone(),
                description: if description_for_file.is_empty() {
                    None
                } else {
                    Some(description_for_file.clone())
                },
                created_at: now,
                lead_agent_id: lead_agent_id.clone(),
                lead_session_id: None,
                members: vec![TeamMember {
                    agent_id: lead_agent_id.clone(),
                    name: TEAM_LEAD_NAME.to_string(),
                    agent_type: Some(agent_type.clone()),
                    model: None,
                    joined_at: now,
                    tmux_pane_id: String::new(),
                    cwd,
                    subscriptions: vec![],
                }],
            };
            let path = team_file::team_file_path(h, &team_name);
            if let Err(e) = team_file::write_team_file(h, &team_name, &file) {
                team_file_warning = Some(format!("failed to write team file: {e}"));
                None
            } else {
                Some(path.display().to_string())
            }
        } else {
            None
        };

        // 5c. Fire `tengu_team_created` (TeamCreateTool.ts:214-222):
        //     { team_name, teammate_count: 1, lead_agent_type, teammate_mode }.
        //     `teammate_mode` is "in-process" — the lingxi coordinator runs
        //     `InProcessTeammate` exclusively (getResolvedTeammateMode analog).
        self.emit_team_created(&team_name, &agent_type).await;

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
        //    placeholder). `agent_id_str` was computed in 5b.
        let mut data = json!({
            "team_name": team_name,
            "lead_agent_id": agent_id_str,
            "task_id": task_id,
            "spawned": true,
        });
        if let Some(path) = team_file_path {
            data["team_file_path"] = Value::String(path);
        }
        if let Some(warning) = team_file_warning {
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
                model_profile: None,
                max_budget_nano_usd: None,
                mcp_clients: vec![],
                is_non_interactive_session: false,
                custom_system_prompt: None,
                append_system_prompt: None,
            },
            messages: vec![],
            tool_use_id: None,
            agent_id: None,
            agent_name: None,
            team_name: None,
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

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    /// Recording spawn seam: returns a fixed handler-generated `task_id`,
    /// counts `spawn_teammate` invocations, and captures the args of the last
    /// spawn so tests can assert what the tool threaded through. Also records
    /// every `send_message` (as the injected text) so the pump-integration test
    /// can assert a routed message reached the seam's runner inject.
    struct RecordingSeam {
        task_id: String,
        spawns: AtomicUsize,
        last_args: Mutex<Option<(AgentId, String, String, String)>>,
        injected: Mutex<Vec<String>>,
    }

    impl RecordingSeam {
        fn new(task_id: impl Into<String>) -> Self {
            Self {
                task_id: task_id.into(),
                spawns: AtomicUsize::new(0),
                last_args: Mutex::new(None),
                injected: Mutex::new(Vec::new()),
            }
        }
        fn injected(&self) -> Vec<String> {
            self.injected.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl TeamSpawnSeam for RecordingSeam {
        async fn spawn_teammate(
            &self,
            agent_id: AgentId,
            name: String,
            team_name: String,
            description: String,
        ) -> Result<String, traits::team_spawn::TeamSpawnError> {
            self.spawns.fetch_add(1, Ordering::SeqCst);
            *self.last_args.lock().unwrap() = Some((agent_id, name, team_name, description));
            Ok(self.task_id.clone())
        }
        async fn kill(&self, _task_id: &str) -> Result<(), traits::team_spawn::TeamSpawnError> {
            Ok(())
        }
        async fn send_message(
            &self,
            _task_id: &str,
            message: String,
        ) -> Result<(), traits::team_spawn::TeamSpawnError> {
            self.injected.lock().unwrap().push(message);
            Ok(())
        }
    }

    /// A tiny tokio-backed [`RuntimeSpawner`] for the pump-integration test. The
    /// coordinator crate does not depend on `test-harness`, so we inline the
    /// minimal spawner here (tests may use tokio directly; the D17 no-direct-
    /// `tokio::spawn` rule is for production engine code).
    struct TestSpawner {
        handles: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    }
    impl TestSpawner {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                handles: Mutex::new(Vec::new()),
            })
        }
    }
    #[async_trait]
    impl traits::RuntimeSpawner for TestSpawner {
        async fn spawn(
            &self,
            name: &str,
            task: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>>,
        ) -> Result<traits::BackgroundTaskHandle, traits::RuntimeError> {
            let h = tokio::spawn(task);
            self.handles.lock().unwrap().push(h);
            Ok(traits::BackgroundTaskHandle {
                task_name: name.into(),
                task_id: 1,
            })
        }
        async fn sleep(&self, duration: std::time::Duration) {
            tokio::time::sleep(duration).await;
        }
        async fn cancel(
            &self,
            _handle: &traits::BackgroundTaskHandle,
        ) -> Result<(), traits::RuntimeError> {
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
            _model_text: &str,
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
    ///
    /// The tool's `~/.claude` root is overridden to a fresh tempdir so the
    /// on-disk team file is written under a scratch path (hermetic; no real
    /// `~/.lingxi/teams/` pollution, no cross-test interference). The returned
    /// `TempDir` MUST be held alive for the duration of the test.
    fn make_tool_with_seam_and_spy(
        seam: Arc<RecordingSeam>,
    ) -> (
        TeamCreateTool,
        Arc<TeamRegistry>,
        Arc<CoordinatorMode>,
        Arc<SpyOutput>,
        tempfile::TempDir,
    ) {
        let registry = Arc::new(TeamRegistry::new(AgentId::new()));
        let mode = Arc::new(CoordinatorMode::new());
        mode.enter();
        let spy = Arc::new(SpyOutput::default());
        let tmp = tempfile::tempdir().expect("tempdir");
        let tool = TeamCreateTool::new(
            registry.clone(),
            mode.clone(),
            seam as Arc<dyn TeamSpawnSeam>,
            spy.clone() as Arc<dyn OutputStream>,
        )
        .with_home(tmp.path().join(".lingxi"));
        (tool, registry, mode, spy, tmp)
    }

    /// Build a `TeamCreate` tool with coordinator mode ENABLED (the normal path)
    /// and a recording seam returning the given handler `task_id`. Returns the
    /// hermetic-home `TempDir` (hold it alive for the test).
    fn make_tool_with_seam(
        seam: Arc<RecordingSeam>,
    ) -> (
        TeamCreateTool,
        Arc<TeamRegistry>,
        Arc<CoordinatorMode>,
        tempfile::TempDir,
    ) {
        let (tool, registry, mode, _spy, tmp) = make_tool_with_seam_and_spy(seam);
        (tool, registry, mode, tmp)
    }

    fn make_tool() -> (TeamCreateTool, Arc<TeamRegistry>, tempfile::TempDir) {
        let seam = Arc::new(RecordingSeam::new("task-handler-id"));
        let (tool, registry, _mode, tmp) = make_tool_with_seam(seam);
        (tool, registry, tmp)
    }

    #[test]
    fn name_matches_ts_constant() {
        let (tool, _registry, _tmp) = make_tool();
        assert_eq!(tool.name(), "TeamCreate");
        assert_eq!(tool.name(), TEAM_CREATE_TOOL_NAME);
    }

    #[test]
    fn schema_is_strict_object_with_required_team_name() {
        let (tool, _registry, _tmp) = make_tool();
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
        let (tool, _registry, _tmp) = make_tool();
        assert!(tool.is_enabled(&ToolStaticContext::default()));
        assert!(tool.should_defer());
        assert!(!tool.is_read_only(&json!({})));
        assert!(!tool.is_destructive(&json!({})));
        assert_eq!(tool.max_result_size_chars(), 100_000);
    }

    #[tokio::test]
    async fn enabled_respects_feature_flag_off() {
        let (tool, _registry, _tmp) = make_tool();
        let mut ctx = ToolStaticContext::default();
        ctx.feature_flags
            .insert(AGENT_SWARMS_FLAG.to_string(), false);
        assert!(!tool.is_enabled(&ctx));
    }

    #[tokio::test]
    async fn call_spawns_worker_and_returns_agent_id() {
        let (tool, registry, _tmp) = make_tool();
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
        let (tool, registry, _tmp) = make_tool();
        tool.call(json!({ "team_name": "beta" }), fresh_ctx(), fresh_tx())
            .await
            .expect("valid call");
        let workers = registry.list().await;
        assert_eq!(workers.len(), 1);
        assert_eq!(workers[0].agent_type, "team-lead");
    }

    #[tokio::test]
    async fn call_rejects_missing_team_name() {
        let (tool, registry, _tmp) = make_tool();
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
        let (tool, registry, _tmp) = make_tool();
        let err = tool
            .call(json!({ "team_name": "   " }), fresh_ctx(), fresh_tx())
            .await
            .expect_err("whitespace team_name must fail");
        assert!(matches!(err, ToolError::InvalidInput(_)));
        assert!(registry.list().await.is_empty());
    }

    #[tokio::test]
    async fn validate_input_matches_ts_message() {
        let (tool, _registry, _tmp) = make_tool();
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
        assert!(
            registry.list().await.is_empty(),
            "no worker on disabled mode"
        );
        assert_eq!(seam.spawns.load(Ordering::SeqCst), 0);
        assert_eq!(registry.team_name().await, None);
        assert_eq!(spy.calls(), 0, "no CoordinatorStatus push when mode is off");
    }

    #[tokio::test]
    async fn call_spawns_worker_and_writes_back_task_id() {
        let seam = Arc::new(RecordingSeam::new("handler-task-42"));
        let (tool, registry, _mode, _tmp) = make_tool_with_seam(seam.clone());

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

        // The display name, team name, and description were threaded to the seam.
        let (seam_agent_id, seam_name, seam_team_name, seam_desc) =
            seam.last_args.lock().unwrap().clone().expect("seam called");
        assert_eq!(seam_agent_id, w.agent_id);
        assert_eq!(seam_name, "alpha-team");
        assert_eq!(
            seam_team_name, "alpha-team",
            "team name threaded into spawn_teammate (getTeammateContext()?.teamName)"
        );
        assert_eq!(seam_desc, "do work");
    }

    #[tokio::test]
    async fn call_invokes_spawn_seam_once() {
        let seam = Arc::new(RecordingSeam::new("handler-task-1"));
        let (tool, _registry, _mode, _tmp) = make_tool_with_seam(seam.clone());

        tool.call(json!({ "team_name": "beta" }), fresh_ctx(), fresh_tx())
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
        let (tool, registry, _mode, spy, _tmp) = make_tool_with_seam_and_spy(seam);

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
        assert_eq!(
            spy.calls(),
            1,
            "exactly one CoordinatorStatus push on spawn"
        );
        assert_eq!(
            spy.last(),
            Some((1, Some("alpha".to_string()))),
            "the push carries the live active-worker count and team name"
        );
    }

    // ---- D1 ITEM 3: one-team guard + unique-name + disk file + telemetry ----

    /// One-team-per-leader guard (TeamCreateTool.ts:132-140): a second
    /// `TeamCreate` while a team is already set on the registry → error, and no
    /// second worker is spawned.
    #[tokio::test]
    async fn second_team_create_while_leading_errors() {
        let (tool, registry, _tmp) = make_tool();
        tool.call(json!({ "team_name": "alpha" }), fresh_ctx(), fresh_tx())
            .await
            .expect("first create succeeds");
        assert_eq!(registry.list().await.len(), 1);
        assert_eq!(registry.team_name().await, Some("alpha".to_string()));

        let err = tool
            .call(json!({ "team_name": "beta" }), fresh_ctx(), fresh_tx())
            .await
            .expect_err("second create while leading must fail");
        assert!(matches!(err, ToolError::InvalidInput(_)));
        let msg = format!("{err}");
        assert!(
            msg.contains("Already leading team \"alpha\""),
            "guard message must name the existing team; got: {msg}"
        );
        assert!(msg.contains("A leader can only manage one team at a time"));
        // No second worker spawned, team name unchanged.
        assert_eq!(registry.list().await.len(), 1, "no second worker on guard");
        assert_eq!(registry.team_name().await, Some("alpha".to_string()));
    }

    /// Name-collision auto-rename (TeamCreateTool.ts:60-72,143
    /// generateUniqueTeamName): when a team file with the requested name already
    /// exists, the tool RENAMES (not errors) — the result carries a different,
    /// derived team name and a fresh worker is spawned under it.
    #[tokio::test]
    async fn name_collision_auto_renames() {
        let seam = Arc::new(RecordingSeam::new("task-x"));
        let (tool, registry, _mode, _spy, tmp) = make_tool_with_seam_and_spy(seam);
        let home = tmp.path().join(".lingxi");
        // Pre-seed a colliding team file at `~/.lingxi/teams/alpha/config.json`.
        std::fs::create_dir_all(team_file::team_dir(&home, "alpha")).unwrap();
        std::fs::write(team_file::team_file_path(&home, "alpha"), "{}").unwrap();

        let res = tool
            .call(json!({ "team_name": "alpha" }), fresh_ctx(), fresh_tx())
            .await
            .expect("create with a colliding name must succeed (rename, not error)");

        let final_name = res.data["team_name"].as_str().unwrap();
        assert_ne!(
            final_name, "alpha",
            "must have been renamed off the collision"
        );
        assert!(
            final_name.starts_with("alpha-"),
            "rename keeps the requested name as a prefix; got: {final_name}"
        );
        // Exactly one worker, named with the renamed team.
        let workers = registry.list().await;
        assert_eq!(workers.len(), 1);
        assert_eq!(workers[0].name, final_name);
        assert_eq!(registry.team_name().await.as_deref(), Some(final_name));
    }

    /// The on-disk team file is written at `~/.lingxi/teams/{name}/config.json`
    /// with the TS `TeamFile` shape (TeamCreateTool.ts:157-177).
    #[tokio::test]
    async fn writes_team_file_to_disk() {
        let seam = Arc::new(RecordingSeam::new("task-y"));
        let (tool, _registry, _mode, _spy, tmp) = make_tool_with_seam_and_spy(seam);
        let home = tmp.path().join(".lingxi");

        let res = tool
            .call(
                json!({ "team_name": "alpha-team", "agent_type": "researcher", "description": "do work" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("create succeeds");

        // Result surfaces the written path.
        let path = res.data["team_file_path"]
            .as_str()
            .expect("team_file_path present");
        assert!(
            path.ends_with("teams/alpha-team/config.json"),
            "path: {path}"
        );

        // The file exists and has the TS shape.
        let on_disk = team_file::team_file_path(&home, "alpha-team");
        assert!(on_disk.is_file(), "team file written to disk");
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&on_disk).unwrap()).unwrap();
        assert_eq!(v["name"], "alpha-team");
        assert_eq!(v["leadAgentId"], "team-lead@alpha-team");
        assert_eq!(v["description"], "do work");
        assert_eq!(v["members"][0]["name"], "team-lead");
        assert_eq!(v["members"][0]["agentType"], "researcher");
        assert_eq!(v["members"][0]["subscriptions"], json!([]));
        assert!(v["createdAt"].is_number());
    }

    /// `tengu_team_created` is fired through the attached analytics bus with the
    /// TS field set (TeamCreateTool.ts:214-222).
    #[tokio::test]
    async fn fires_team_created_telemetry() {
        use telemetry::sinks::InMemorySink;
        let bus = Arc::new(AnalyticsBus::new());
        let sink = Arc::new(InMemorySink::new());
        bus.attach_sink(sink.clone()).await;

        let seam = Arc::new(RecordingSeam::new("task-z"));
        let (tool, _registry, _mode, _spy, _tmp) = make_tool_with_seam_and_spy(seam);
        let tool = tool.with_analytics_bus(Some(bus));

        tool.call(
            json!({ "team_name": "alpha", "agent_type": "researcher" }),
            fresh_ctx(),
            fresh_tx(),
        )
        .await
        .expect("create succeeds");

        let events = sink.events().await;
        let ev = events
            .iter()
            .find(|e| e.name == TEAM_CREATED)
            .expect("tengu_team_created must be emitted");
        assert!(matches!(
            ev.metadata.get("teammate_count"),
            Some(AnalyticsValue::Int(1))
        ));
        assert!(matches!(
            ev.metadata.get("teammate_mode"),
            Some(AnalyticsValue::String(s)) if s == "in-process"
        ));
        assert!(matches!(
            ev.metadata.get("lead_agent_type"),
            Some(AnalyticsValue::String(s)) if s == "researcher"
        ));
        assert!(ev.metadata.contains_key("_PROTO_team_name"));
    }

    // ---- mailbox→runner PUMP integration -----------------------------------

    /// End-to-end: `TeamCreate` with a `RuntimeSpawner` wired starts the
    /// per-teammate pump; a message routed to that teammate's mailbox (the same
    /// thing a coordinator `SendMessage` does) is drained by the pump and
    /// reaches the spawn seam's `send_message` — i.e. it would reach the
    /// teammate's turn loop (`injectUserMessageToTeammate`). Without the pump
    /// (the other tests pass no runtime) such a message would just sit unread.
    #[tokio::test]
    async fn teamcreate_starts_pump_that_drains_to_runner() {
        let seam = Arc::new(RecordingSeam::new("handler-task-pump"));
        let (tool, registry, _mode, _tmp) = make_tool_with_seam(seam.clone());
        let spawner = TestSpawner::new();
        let tool = tool.with_runtime(spawner as Arc<dyn traits::RuntimeSpawner>);

        // Create the team → spawns the worker (registers its mailbox), starts
        // the real teammate (recording seam), and launches the pump.
        tool.call(
            json!({ "team_name": "alpha", "agent_type": "researcher" }),
            fresh_ctx(),
            fresh_tx(),
        )
        .await
        .expect("TeamCreate must succeed");

        // Resolve the freshly-spawned worker + its mailbox (the same router the
        // coordinator `SendMessage` routes through).
        let worker = registry
            .list()
            .await
            .into_iter()
            .next()
            .expect("one worker");
        let mailbox = registry
            .mailbox_router
            .get(&worker.agent_id)
            .await
            .expect("the worker's mailbox is registered");

        // Deliver a message exactly like SendMessage's `route` would.
        mailbox
            .deliver(crate::mailbox::TeammateMessage {
                from: crate::mailbox::MessageSender::Coordinator,
                content: "pick up the new task".into(),
                message_id: "m-1".into(),
                timestamp: std::time::SystemTime::now(),
                request_id: None,
            })
            .expect("deliver to the registered mailbox");

        // The pump (running on the TestSpawner) drains it into the seam's
        // send_message. Bounded retry to avoid a scheduling flake.
        for _ in 0..200 {
            if !seam.injected().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert_eq!(
            seam.injected(),
            vec!["pick up the new task".to_string()],
            "the pump drained the routed message into the runner via the seam"
        );
    }

    /// Control: WITHOUT a wired runtime, `TeamCreate` starts NO pump, so a
    /// routed message stays queued in the mailbox (the seam's `send_message` is
    /// never called). Locks the opt-in behavior.
    #[tokio::test]
    async fn teamcreate_without_runtime_starts_no_pump() {
        let seam = Arc::new(RecordingSeam::new("handler-task-nopump"));
        let (tool, registry, _mode, _tmp) = make_tool_with_seam(seam.clone());
        // Note: no `.with_runtime(..)`.

        tool.call(json!({ "team_name": "alpha" }), fresh_ctx(), fresh_tx())
            .await
            .expect("TeamCreate must succeed without a runtime");

        let worker = registry
            .list()
            .await
            .into_iter()
            .next()
            .expect("one worker");
        let mailbox = registry
            .mailbox_router
            .get(&worker.agent_id)
            .await
            .expect("mailbox registered");
        mailbox
            .deliver(crate::mailbox::TeammateMessage {
                from: crate::mailbox::MessageSender::Coordinator,
                content: "unread".into(),
                message_id: "m-1".into(),
                timestamp: std::time::SystemTime::now(),
                request_id: None,
            })
            .unwrap();

        // Give any (erroneously-started) pump a chance to run; assert nothing
        // was injected and the message is still queued.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(
            seam.injected().is_empty(),
            "no pump ⇒ the routed message is never injected into the runner"
        );
        assert_eq!(
            mailbox.drain().len(),
            1,
            "the message stays queued in the mailbox (the pending-queue)"
        );
    }
}

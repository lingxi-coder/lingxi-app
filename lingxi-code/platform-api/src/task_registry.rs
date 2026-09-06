//! `TaskRegistryHandle` — narrow trait abstracting `TaskRegistry` CRUD so the
//! six `Task*` tools in `lingxi-tools` can dispatch into the production
//! registry without taking a cyclic dep on `lingxi-tasks`.
//!
//! Concrete impl lives in `lingxi-tasks`. Tests inject an in-memory mock.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Input to [`TaskRegistryHandle::create`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskCreateInput {
    /// Wire string for the task type — one of the 9 byte-locked variants.
    pub task_type: String,
    /// Human-readable description shown in UI listings.
    pub description: String,
}

/// Registration input for a backgrounded MCP tool call (claude-code 2.1.212
/// `callMcpToolWithAutoBackground`'s `NZu` register-input builder). Minted when
/// a single `tools/call` exceeds `getMcpAutoBackgroundMs` and is detached from
/// the turn.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpTaskRegistration {
    /// MCP server name (`serverName`).
    pub server_name: String,
    /// MCP tool name (`toolName`).
    pub tool_name: String,
    /// Originating assistant `tool_use_id`, if any (`toolUseId`).
    pub tool_use_id: Option<String>,
    /// Creator ownership used to defer a resting parent's notification.
    pub creator_teammate_name: Option<String>,
    /// Team containing the creator, when it belongs to one.
    pub creator_team_name: Option<String>,
    /// Persistent identity of the creator agent, when available.
    pub creator_agent_id: Option<protocol::AgentId>,
}

/// Launch input for a background stdout event monitor.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MonitorRegistration {
    /// Shell command whose stdout lines become events.
    pub command: String,
    /// Human-readable description repeated in notifications.
    pub description: String,
    /// Effective timeout in milliseconds. `0` means session-persistent.
    pub timeout_ms: u64,
    /// Whether the monitor lives until TaskStop/session teardown.
    pub persistent: bool,
    /// Invocation working directory.
    pub cwd: Option<String>,
    /// Originating assistant tool-use id, when available.
    pub tool_use_id: Option<String>,
    /// Creator ownership used to defer a resting parent's notification.
    pub creator_teammate_name: Option<String>,
    /// Team containing the creator, when it belongs to one.
    pub creator_team_name: Option<String>,
    /// Persistent identity of the creator agent, when available.
    pub creator_agent_id: Option<protocol::AgentId>,
}

/// Filter for [`TaskRegistryHandle::list`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskListFilter {
    /// Optional status filter — one of the 5 byte-locked status strings.
    pub status: Option<String>,
}

/// Patch shape for [`TaskRegistryHandle::update`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskUpdatePatch {
    /// New status, if changed.
    pub status: Option<String>,
}

/// One task as surfaced to the tool layer.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskRecord {
    /// 9-char `[bartwmdks][0-9a-z]{8}` task id.
    pub task_id: String,
    /// Task type wire string.
    pub task_type: String,
    /// Status wire string.
    pub status: String,
    /// Human-readable description.
    pub description: String,
    /// Wall-clock start as Unix epoch milliseconds, when known. This feeds
    /// the `subagentStatusLine.tasks[].startTime` payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_ms: Option<u64>,
    /// The shell command, for `local_bash` tasks only (claude-code
    /// `LocalShellTaskState.command`). `None` for every other task type. The
    /// `TaskStop` tool surfaces this in preference to `description` for
    /// `local_bash`, mirroring claude-code `stopTask.ts:97`
    /// (`isLocalShellTask(task) ? task.command : task.description`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// `local_agent` only: the agent's type label, surfaced as the
    /// `background_tasks[].agent_type` field of a `Stop` / `SubagentStop` hook
    /// payload (claude-code `Lic`'s `r.agent_type = n.agentType`). `None` for
    /// every other task type. Additive default `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<String>,
    /// `monitor_mcp` / `mcp_task` only: the MCP server name, surfaced as the
    /// `background_tasks[].server` field (claude-code `Lic`'s `r.server`).
    /// `None` for every other task type. Additive default `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    /// `monitor_mcp` / `mcp_task` only: the MCP tool name, surfaced as the
    /// `background_tasks[].tool` field (claude-code `Lic`'s `r.tool`). The port
    /// `MonitorMcpTaskState` carries no per-tool name (it watches resources, not
    /// a single tool), so this stays `None` for `monitor_mcp`; the `mcp_task`
    /// type (`McpTaskState`) DOES carry a single `tool_name` and populates it.
    /// Additive default `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// `local_workflow` only: the workflow name, surfaced as the
    /// `background_tasks[].name` field (claude-code `Lic`'s
    /// `r.name = n.workflowName`). The port carries a `workflow_id` rather than a
    /// separate display name, so the id is used. `None` for every other task
    /// type. Additive default `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `local_agent` only: the skill this agent IS, when a `context: fork`
    /// skill launched it (claude `forkedSkillName`). Keys the live-duplicate
    /// guard — a skill already running as a live fork does not fork again — and
    /// is the task-record half of the identity a resume corroborates against
    /// the on-disk scoping record. Additive default `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forked_skill_name: Option<String>,
    /// `local_agent` only: whether the agent is currently backgrounded, used by
    /// the `Stop` / `SubagentStop` `background_tasks` filter (claude-code `wA`:
    /// drop a task when `"isBackgrounded" in e && e.isBackgrounded === false`).
    /// Only `local_agent` tasks carry an `isBackgrounded` field in claude-code,
    /// so this is `None` for every other task type (and such tasks are never
    /// dropped by that filter clause). Additive default `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_backgrounded: Option<bool>,
    /// Terminal failure reason for a task whose `status` is `failed`, when the
    /// producing handler reported one. `local_workflow` fills it from
    /// [`WorkflowTerminalOutcome::error`] and `local_agent` from the agent's
    /// recorded error; every other task type leaves it `None`. Appended field
    /// (default `None`) — the clients render it next to `description` so a
    /// failed background task is not a bare id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// `local_fusion` only (F005): the run's current progress-stage label —
    /// e.g. "Running panels 2/3" — the SAME text `FusionStage::label()`
    /// produces for the Agent-tool path's `subagent_activity` forwarding, so
    /// a `/fusion` task's DTO/list entry can render identical progress.
    /// `None` for every other task type, and for a `local_fusion` task before
    /// its first `FusionProgress` event lands. Additive default `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
}

/// A `local_workflow` run projected for the interactive `/workflows` picker
/// ("Browse running and completed workflows"). Richer than [`TaskRecord`]: it
/// surfaces the `wf_…` run id, the launcher-minted current phase step, and the
/// start/end wall-clock (epoch millis) so the picker can show elapsed time.
/// These come from the concrete [`crate::task_registry::TaskRegistryHandle`]
/// impl's access to the full task state; the trait's default `list_workflows`
/// fills only what the reduced [`TaskRecord`] carries.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowRecord {
    /// 9-char task id (`w…`) — the handle key for `output`/`kill`.
    pub task_id: String,
    /// The effective `wf_…` run id, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// Display name — the workflow's `meta.name` / `workflow_id`.
    pub name: String,
    /// Status wire string (`pending`/`running`/`paused`/`completed`/`failed`/`killed`).
    pub status: String,
    /// The workflow's launch description (script summary), when present.
    #[serde(default)]
    pub description: String,
    /// Index of the currently-executing phase step (0-based).
    #[serde(default)]
    pub current_step: usize,
    /// Wall-clock start (epoch millis), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_ms: Option<u64>,
    /// Wall-clock end (epoch millis) for a terminal run, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at_ms: Option<u64>,
    /// Stored script source for a saveable dynamic workflow run. Claude keeps
    /// the resolved script on every live workflow task, regardless of whether
    /// it was launched inline, by name, or by path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script: Option<String>,
    /// Script path needed by an explicitly resumed paused workflow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script_path: Option<String>,
    /// Serialized workflow args needed by an explicitly resumed paused workflow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<String>,
    /// Distinct workflow agents known for this run. Terminal runs surface the
    /// runtime's aggregate count; live runs may leave this `0` until the TUI
    /// enriches from the spool/progress feed.
    #[serde(default)]
    pub agent_count: u64,
    /// Aggregate workflow tokens when the runtime has them. Live runs may leave
    /// this `0` until progress updates land.
    #[serde(default)]
    pub total_tokens: u64,
}

/// Everything a terminating `local_agent` run reports beyond its status — the
/// payload claude-code passes to `enqueueAgentNotification` (`yNt`) in the SAME
/// call that carries the terminal status.
///
/// The binary builds this at the moment of termination: `finalMessage` (the
/// agent's final text), the `{totalTokens,toolUses,durationMs}` usage object,
/// the `error` string on the failed path, and the worktree result spread
/// (`...await getWorktreeResult()`) whose two keys gate and fill the optional
/// `<worktree>` section. Modeling it as one struct keeps the port's write
/// ATOMIC with respect to the notification drain: a terminal status published
/// before its payload would let a drain fire the byte-shape of a result-less
/// completion.
///
/// Every field is optional and a `None` never clears an already-stored value
/// (see [`TaskRegistryHandle::set_agent_outcome`]), so a partial report — e.g.
/// a killed run with a worktree but no result — is safe.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentTerminalOutcome {
    /// The agent's final text response → the `<result>` section (claude
    /// `finalMessage`, `wc(content,"\n")` — text blocks joined with `\n`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    /// Run usage → the `<usage>` section.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<AgentRunUsage>,
    /// Failure reason → folded into the `failed` summary (claude `error ||
    /// 'Unknown error'`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The KEPT isolation worktree's path → gates and fills `<worktree>`. A
    /// worktree the terminal judgment auto-removed reports `None` (claude's
    /// `getWorktreeResult` resolves to `{}`), so the section is omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_path: Option<String>,
    /// The kept worktree's branch → `<worktreeBranch>` inside that section.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_branch: Option<String>,
}

/// Agent-run usage for a `local_agent` task-notification's optional `<usage>`
/// section — mirrors claude-code's `enqueueAgentNotification` usage object
/// (`{ totalTokens, toolUses, durationMs }`, rendered as
/// `<subagent_tokens>/<tool_uses>/<duration_ms>`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentRunUsage {
    /// `totalTokens` → `<subagent_tokens>`.
    pub subagent_tokens: u64,
    /// `totalToolUseCount` → `<tool_uses>`.
    pub tool_uses: u64,
    /// `totalDurationMs` → `<duration_ms>`.
    pub duration_ms: u64,
}

/// Everything a terminating `local_workflow` run reports beyond its status.
/// Claude keeps the script result, non-fatal item failures, and aggregate usage
/// as separate notification fields; keeping them together here makes the
/// status-sink update atomic with the terminal transition.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowTerminalOutcome {
    /// The script's serialized return value, if it returned one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    /// Non-fatal `parallel()` / `pipeline()` diagnostics.
    #[serde(default)]
    pub failures: Vec<String>,
    /// Fatal script/engine error, if the workflow failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Number of real `agent()` calls, including cached calls.
    #[serde(default)]
    pub agent_count: u64,
    /// Aggregate terminal subagent tokens.
    #[serde(default)]
    pub total_tokens: u64,
    /// Aggregate terminal subagent tool calls.
    #[serde(default)]
    pub total_tool_calls: u64,
    /// End-to-end workflow duration.
    #[serde(default)]
    pub duration_ms: u64,
    /// Terminal workflow-agent counts used by Claude's enriched `<usage>`.
    #[serde(default)]
    pub agents_done: u64,
    /// Terminal workflow agents that failed.
    #[serde(default)]
    pub agents_error: u64,
    /// Terminal workflow agents that were skipped.
    #[serde(default)]
    pub agents_skipped: u64,
    /// Successful workflow agents whose result was structurally empty.
    #[serde(default)]
    pub agents_empty_result: u64,
    /// Whether the terminal counts above came from real workflow progress
    /// instrumentation instead of a default zero-fill.
    #[serde(default)]
    pub progress_counts_available: bool,
}

/// A terminal task that has not yet been surfaced to the model, snapshotted at
/// drain time for the `<task-notification>` renderer (claude-code's per-task-type
/// `enqueue*Notification`, e.g. `enqueueShellNotification` /
/// `enqueueAgentNotification`). Each field maps to a tag the renderer emits;
/// fields that a given task type does not carry stay `None` and the renderer
/// omits the corresponding clause/tag.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskNotification {
    /// 9-char task id → `<task-id>`.
    pub task_id: String,
    /// Task type wire string (one of the 9 byte-locked variants). Selects the
    /// per-type notification format (bash / agent / monitor / generic).
    pub task_type: String,
    /// Terminal status wire string — one of `completed` / `failed` / `killed`
    /// → `<status>` and the human-readable summary verb.
    pub status: String,
    /// Human-readable description → interpolated into `<summary>`.
    pub description: String,
    /// Originating `tool_use_id`, if the task was launched from a tool call →
    /// the optional `<tool-use-id>` line. `None` ⇒ the line is omitted.
    pub tool_use_id: Option<String>,
    /// Absolute on-disk spool path → `<output-file>`. `None` ⇒ the renderer
    /// falls back to the bare `<task_id>.output` filename.
    pub output_path: Option<String>,
    /// Process exit code for `local_bash` / `monitor_ws` tasks, folded into the
    /// summary (e.g. `(exit code 1)`). `None` ⇒ the exit clause is omitted.
    pub exit_code: Option<i32>,
    /// Failure reason for a `local_agent` task, folded into the `failed`
    /// summary (`Agent "…" came to rest with an error: {error}`). `None` falls
    /// back to `Unknown error` (claude-code `error || 'Unknown error'`).
    pub error: Option<String>,
    /// `local_agent`: final text response; `monitor_ws` while running: batched
    /// stdout event. Both render in an escaped `<result>` section.
    pub result: Option<String>,
    /// `local_agent` only: run usage → the optional `<usage>` section.
    /// `None` ⇒ omitted (claude-code's `i ? <usage>… : ''`).
    pub usage: Option<AgentRunUsage>,
    /// `local_agent` only: the stop reason for a `killed` task — selects the
    /// killed-summary verb (claude-code `enqueueAgentNotification`'s `killedBy`
    /// param). `Some("parent")` → `was stopped by Claude` (a parent-agent /
    /// `TaskStop`-initiated stop), `Some("user")` → `was stopped by user`,
    /// `None`/other → the generic `was stopped` (the binary's
    /// `n==="parent"?…:n==="user"?…:"was stopped"` fallback for an undefined
    /// `killedBy`). Additive default `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub killed_by: Option<String>,
    /// `local_agent` only: the isolation worktree's absolute path → gates AND
    /// fills the optional `<worktree><worktreePath>…</worktreePath>…</worktree>`
    /// section (claude-code `enqueueAgentNotification`'s `worktreePath`; tags
    /// `pZo="worktree"` / `fZo="worktreePath"`). `None` ⇒ the whole worktree
    /// section is omitted (the binary's `c ? … : ''`). Additive default `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_path: Option<String>,
    /// `local_agent` only: the isolation worktree's branch name → the optional
    /// `<worktreeBranch>…</worktreeBranch>` tag rendered INSIDE the worktree
    /// section (claude-code `worktreeBranch`; tag `mZo="worktreeBranch"`,
    /// independently gated on `u`). `None` while `worktree_path` is `Some` ⇒ the
    /// branch tag is omitted but the section still renders. Additive default
    /// `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_branch: Option<String>,
    /// `local_workflow`: non-fatal per-item diagnostics rendered in a separate
    /// `<failures>` section, never appended to `<result>`.
    #[serde(default)]
    pub workflow_failures: Vec<String>,
    /// `local_workflow`: aggregate workflow usage fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_agent_count: Option<u64>,
    /// `local_workflow`: aggregate terminal subagent token count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_total_tokens: Option<u64>,
    /// `local_workflow`: aggregate terminal subagent tool-call count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_total_tool_calls: Option<u64>,
    /// `local_workflow`: end-to-end workflow duration in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_duration_ms: Option<u64>,
    /// `local_workflow`: session-owned persisted script path and run identity,
    /// used to render recovery/diagnostics instructions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_script_path: Option<String>,
    /// `local_workflow`: stable run identifier used by resume instructions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_run_id: Option<String>,
    /// `local_workflow`: serialized workflow arguments used by rerun instructions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_args: Option<String>,
    /// `local_workflow`: directory containing per-agent transcript journals.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_transcript_dir: Option<String>,
    /// Terminal workflow-agent counters included inside `<usage>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_agents_done: Option<u64>,
    /// `local_workflow`: number of failed terminal agent calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_agents_error: Option<u64>,
    /// `local_workflow`: number of skipped terminal agent calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_agents_skipped: Option<u64>,
    /// `local_workflow`: number of terminal agent calls with empty results.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_agents_empty_result: Option<u64>,
    /// `local_fusion`: provider profiles that received prompt data on this
    /// run (parent's profile plus any cross-provider panels) → the optional
    /// `<egress-profiles>` line. Never includes model names, only profile
    /// ids. Empty ⇒ the section is omitted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub egress_profiles: Vec<String>,
}

/// One chunk of a task's accumulated stdout/stderr spool.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskOutputChunk {
    /// 9-char task id.
    pub task_id: String,
    /// Spooled content for this chunk.
    pub content: String,
    /// Total line count of the spool.
    pub total_lines: u64,
    /// `true` when the surfaced content was truncated by a limit.
    pub truncated: bool,
    /// Task status wire string at the chunk point (one of the 5 byte-locked
    /// status strings), if the registry could resolve it. Mirrors the TS
    /// `task.status` carried by `TaskOutputTool`'s `TaskOutput`.
    pub status: Option<String>,
    /// Process exit code at the chunk point, if terminal and applicable.
    /// Mirrors the TS `exitCode` (`bashTask.result?.code ?? null`).
    pub exit_code: Option<i32>,
    /// `true` when the task has reached a terminal status (completed / failed /
    /// killed). Lets the tool compute `block`/`retrieval_status` without a
    /// second registry round-trip.
    pub done: bool,
    /// Agent-task error message, if any. Mirrors the TS `TaskOutput.error`
    /// (`agentTask.error`); only populated for `local_agent` tasks. Surfaced by
    /// `TaskOutputTool` as a trailing `<error>…</error>` element.
    pub error: Option<String>,
    /// Agent-task initial prompt. Mirrors the TS `TaskOutput.prompt`
    /// (`agentTask.prompt`); only populated for `local_agent` tasks.
    pub prompt: Option<String>,
    /// Clean final answer extracted from the agent's last assistant message
    /// (the `text` content blocks joined by `\n`). Mirrors the TS
    /// `cleanResult = extractTextContent(agentTask.result.content, '\n')`; only
    /// populated for `local_agent` tasks. `TaskOutputTool` prefers this over the
    /// raw on-disk transcript for the model-facing `<output>`.
    pub result: Option<String>,
    /// Absolute on-disk path of the task's spool file, when the registry can
    /// resolve it. Mirrors the path `getTaskOutputPath(taskId)` returns in
    /// claude-code (`<projectTempDir>/<sessionId>/tasks/<taskId>.output`).
    /// `TaskOutputTool` uses it for the `[Truncated. Full output: <path>]`
    /// header (`outputFormatting.ts:31-34`), so the model sees the real absolute
    /// path it can read. `None` ⟶ the tool falls back to the bare
    /// `<taskId>.output` filename.
    pub output_path: Option<String>,
}

/// Failure modes for [`TaskRegistryHandle`] operations.
#[derive(Debug, Error)]
pub enum TaskRegistryError {
    /// No task with that id exists.
    #[error("Task: not found: {0}")]
    NotFound(String),
    /// The input is malformed (unknown `task_type`, malformed id, bad status).
    #[error("Task: invalid input: {0}")]
    InvalidInput(String),
    /// Any other internal failure.
    #[error("Task: internal error: {0}")]
    Internal(String),
}

/// CRUD surface used by the 6 `Task*` tools.
#[async_trait]
pub trait TaskRegistryHandle: Send + Sync {
    /// Create a new task, returning the freshly generated record.
    async fn create(&self, input: TaskCreateInput) -> Result<TaskRecord, TaskRegistryError>;

    /// Look up a task by id.
    async fn get(&self, id: &str) -> Result<Option<TaskRecord>, TaskRegistryError>;

    /// List tasks, optionally filtered.
    async fn list(&self, filter: TaskListFilter) -> Result<Vec<TaskRecord>, TaskRegistryError>;

    /// List `local_workflow` runs (running, adopted-paused, and completed) for the
    /// `/workflows` picker. The default impl derives them from [`Self::list`],
    /// filling only the reduced [`TaskRecord`] fields; the concrete
    /// `TaskRegistry` impl overrides this to add the `wf_…` run id, phase step,
    /// and start/end timestamps from the full task state. Defaulted so existing
    /// mock handles compile unchanged (frozen-trait idiom).
    async fn list_workflows(&self) -> Result<Vec<WorkflowRecord>, TaskRegistryError> {
        let records = self.list(TaskListFilter::default()).await?;
        Ok(records
            .into_iter()
            .filter(|r| r.task_type == "local_workflow")
            .map(|r| WorkflowRecord {
                task_id: r.task_id,
                run_id: None,
                name: r.name.unwrap_or_else(|| r.description.clone()),
                status: r.status,
                description: r.description,
                current_step: 0,
                started_at_ms: None,
                ended_at_ms: None,
                script: None,
                script_path: None,
                args: None,
                agent_count: 0,
                total_tokens: 0,
            })
            .collect())
    }

    /// Apply a patch (currently: status transition).
    async fn update(
        &self,
        id: &str,
        patch: TaskUpdatePatch,
    ) -> Result<TaskRecord, TaskRegistryError>;

    /// Force a specific status string (covers `TaskUpdate` for variants whose
    /// status field is exposed in the M1 registry surface). Wired directly to
    /// the concrete `TaskRegistry::set_status` method.
    async fn set_status(&self, id: &str, status: &str) -> Result<TaskRecord, TaskRegistryError>;

    /// Kill the task (cancels any background handle, marks status `killed`).
    async fn kill(&self, id: &str) -> Result<TaskRecord, TaskRegistryError>;

    /// Kill the task, recording WHO stopped it so a `local_agent`'s killed
    /// notification renders the right verb.
    ///
    /// Claude-code's stop entry point (`H$e`) takes `killedBy` from its caller
    /// and stores it on the task; the renderer then picks
    /// `"parent"` → `was stopped by Claude`, `"user"` → `was stopped by user`,
    /// anything else → the bare `was stopped`. The two production callers are
    /// byte-visible in 2.1.220: the `TaskStop` TOOL passes `killedBy:"parent"`
    /// (a parent agent stopping a child), and the UI/control-channel `stopTask`
    /// passes `source:"user"` and inherits `H$e`'s `killedBy = "user"`
    /// destructuring default.
    ///
    /// Defaulted to plain [`kill`](Self::kill) — a host that does not
    /// distinguish the two stores no reason, which is exactly the binary's
    /// `undefined killedBy` case.
    async fn kill_with_reason(
        &self,
        id: &str,
        _killed_by: &str,
    ) -> Result<TaskRecord, TaskRegistryError> {
        self.kill(id).await
    }

    /// Record a terminating `local_agent`'s notification payload
    /// ([`AgentTerminalOutcome`]).
    ///
    /// MUST be called BEFORE the terminal `set_status`: the drain
    /// ([`take_pending_task_notifications`](Self::take_pending_task_notifications))
    /// is gated on terminal-and-not-notified, so publishing the status first
    /// opens a window where the notification renders without its `<result>` /
    /// `<usage>` / `<worktree>` sections. Claude-code has no such window — it
    /// passes status and payload to `enqueueAgentNotification` in one call.
    ///
    /// Merge semantics: a `Some` field overwrites, a `None` field leaves the
    /// stored value alone. Default no-op so existing mock handles compile
    /// unchanged.
    async fn set_agent_outcome(&self, _id: &str, _outcome: AgentTerminalOutcome) {}

    /// Record a terminating `local_workflow`'s result, failures, and usage
    /// before its terminal status is published. Defaulted for existing hosts
    /// and mocks that do not expose workflow notifications.
    async fn set_workflow_outcome(&self, _id: &str, _outcome: WorkflowTerminalOutcome) {}

    /// Spawn a real background monitor and return its registry task id.
    /// Hosts without a task runtime fail closed rather than minting a fake id.
    async fn spawn_monitor(&self, reg: MonitorRegistration) -> Result<String, TaskRegistryError> {
        let _ = reg;
        Err(TaskRegistryError::Internal(
            "monitor registration unwired".into(),
        ))
    }

    /// Enqueue one live stdout event for the next task-notification drain.
    async fn notify_monitor_event(&self, _id: &str, _event: &str) {}

    /// Stop the background agent work that Claude Code tears down when the
    /// session reaches `--max-budget-usd`.
    ///
    /// Claude Code 2.1.217's `rcr` budget cleanup selects running
    /// `local_agent` tasks unless `isBackgrounded === false`, plus running
    /// `local_workflow` tasks. It deliberately leaves shell, MCP, remote-agent,
    /// and foreground-agent tasks alone. Keep the selection here at the shared
    /// registry boundary so the print and stream-json turn drivers cannot drift
    /// from one another.
    ///
    /// `local_fusion` has no claude-code analogue but is added here
    /// unconditionally (LingXi-specific, finding G003(b)): before this, a
    /// running Fusion run was invisible to `_ => false` and kept spending
    /// past `--max-budget-usd` — the one runtime kill switch for background
    /// spend never reached it.
    ///
    /// `before_stop` is called exactly once, after at least one matching task is
    /// found and before any cancellation begins. The print driver uses that
    /// seam to preserve Claude's observable ordering: write the budget notice,
    /// then tear down background work.
    ///
    /// Returns the number of matching tasks for which a stop was attempted.
    /// Individual stop races are best-effort: one task finishing between the
    /// list and kill calls must not prevent the remaining agents from stopping.
    async fn stop_background_agents_for_budget(
        &self,
        before_stop: &(dyn Fn() + Send + Sync),
    ) -> Result<usize, TaskRegistryError> {
        let running = self
            .list(TaskListFilter {
                status: Some("running".to_string()),
            })
            .await?;
        let ids: Vec<String> = running
            .into_iter()
            .filter(|task| match task.task_type.as_str() {
                "local_agent" => task.is_backgrounded != Some(false),
                "local_workflow" | "local_fusion" => true,
                _ => false,
            })
            .map(|task| task.task_id)
            .collect();
        if !ids.is_empty() {
            before_stop();
        }
        for id in &ids {
            let _ = self.kill(id).await;
        }
        Ok(ids.len())
    }

    /// Register a backgrounded MCP tool call and return the minted `k…` task id
    /// (claude-code 2.1.212 `callMcpToolWithAutoBackground`'s `i.register(g)`,
    /// where `g = NZu({serverName, toolName, toolUseId, abortController})`). The
    /// task is inserted `running` with `mcpStatus:"working"`; `cancel` is fired
    /// when the task is later killed (`TaskStop`), aborting the still-running
    /// in-flight call — the port equivalent of the state's `abortController` +
    /// the poll loop's `cancelTask` on `status==="killed"`.
    ///
    /// Default impl returns [`TaskRegistryError::Internal`] so a host that has
    /// not wired a real registry never auto-backgrounds — the [`crate`] MCP tool
    /// falls back to the direct await. Frozen-trait defaulted-method idiom so
    /// existing mock handles compile unchanged.
    async fn register_mcp_task(
        &self,
        reg: McpTaskRegistration,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<String, TaskRegistryError> {
        let _ = (reg, cancel);
        Err(TaskRegistryError::Internal(
            "mcp_task registration unwired".into(),
        ))
    }

    /// Settle a backgrounded MCP tool call once the detached `tools/call`
    /// resolves (claude-code `callMcpToolWithAutoBackground`'s inner `E`
    /// callback + `p.then(…)`): write `result_text` into the task spool so the
    /// drained `<task-notification>`'s `output-file` carries the real result,
    /// then mark the task terminal — `completed` on success, `failed` otherwise
    /// (`{...O, status:S, mcpStatus:S, endTime, notified:true}`). A task already
    /// terminal (e.g. killed via `TaskStop`) is left untouched (the binary's
    /// `if(O.notified) return O` guard). Default no-op so existing mock handles
    /// compile unchanged.
    ///
    /// Returns `Ok(true)` when this call won the terminal transition and
    /// `Ok(false)` when the task was already terminal (the no-op path) — callers
    /// gate the `mcp_auto_background` outcome counter on this so a killed /
    /// already-settled task never re-emits it. Default returns `Ok(false)`
    /// (the no-op mock never settles anything).
    async fn settle_mcp_task(
        &self,
        id: &str,
        result_text: &str,
        failed: bool,
    ) -> Result<bool, TaskRegistryError> {
        let _ = (id, result_text, failed);
        Ok(false)
    }

    /// Read the task's spool starting at `offset` (or from 0 if `None`).
    async fn output(
        &self,
        id: &str,
        offset: Option<u64>,
    ) -> Result<TaskOutputChunk, TaskRegistryError>;

    /// Mark a task as having had its terminal output consumed by a reader,
    /// suppressing a later duplicate `<task-notification>`. Mirrors claude-code
    /// `TaskOutputTool`'s `updateTaskState(task_id, t => ({ ...t, notified: true
    /// }))` in both the non-blocking and blocking terminal branches. The task is
    /// retained after notification so callers can still list/read completed
    /// background work until an explicit cleanup/delete path removes it. A
    /// `None`/unknown id is a no-op for callers that cannot guarantee the task
    /// still exists; the default impl is a no-op so existing mock handles compile
    /// unchanged.
    async fn mark_notified(&self, _id: &str) -> Result<(), TaskRegistryError> {
        Ok(())
    }

    /// Record a `local_bash` task's child exit code once the process exits
    /// (M8 cc2.1.198 "Task panels: no stuck Running"): the registry's
    /// `output()` projection derives `exit_code`/`done` from it. Defaulted
    /// no-op so existing mock handles compile unchanged (the frozen-trait
    /// defaulted-method idiom).
    async fn set_exit_code(&self, _id: &str, _exit_code: i32) -> Result<(), TaskRegistryError> {
        Ok(())
    }

    /// Arm a one-shot "came to rest" notification for a PERSISTENT, still-alive
    /// task — the read side of which is surfaced (without eviction) by
    /// [`take_pending_task_notifications`]. Called via the task status sink's
    /// `notify_rest` each time a backgrounded agent parks after a turn-set.
    /// `result` is the agent's final-text response and `usage` its run usage —
    /// both surfaced as the optional `<result>` / `<usage>` notification sections
    /// (the binary `enqueueAgentNotification` always passes them when a result
    /// exists). Default no-op so existing mock handles compile unchanged.
    async fn mark_rested(&self, _id: &str, _result: Option<String>, _usage: Option<AgentRunUsage>) {
    }

    /// Drain the terminal tasks that have NOT yet been surfaced to the model,
    /// marking each `notified` so a given completion is reported exactly once.
    /// Returns a snapshot of each drained task for the
    /// `<task-notification>` renderer, in registry-iteration order.
    ///
    /// 1:1 with claude-code's per-task-type completion path: a task that reaches
    /// a terminal status enqueues exactly one `<task-notification>` and is then
    /// `notified` (guarded by the same `notified` flag's check-and-set), so this
    /// drain is the turn-boundary equivalent of those per-type
    /// `enqueue*Notification` callbacks. A task ALREADY `notified` (e.g. by the
    /// `TaskOutput`/`TaskStop` tool consuming its output) is skipped — no
    /// duplicate. The default impl returns empty so existing mock handles compile
    /// unchanged and builds with no registry stay byte-identical (no reminder).
    async fn take_pending_task_notifications(
        &self,
    ) -> Result<Vec<TaskNotification>, TaskRegistryError> {
        Ok(Vec::new())
    }

    /// Total number of subagents spawned so far this session (claude 2.1.212
    /// `taskRegistry.getTotalAgentSpawns()`). The `Agent` tool reads this before
    /// every spawn and rejects the launch once it reaches
    /// `CLAUDE_CODE_MAX_SUBAGENTS_PER_SESSION` (default 200). Default impl returns
    /// `0` so existing mock handles compile unchanged and stay uncapped
    /// (frozen-trait defaulted-method idiom); the concrete `TaskRegistry`
    /// overrides this and the reservation methods below with a real per-session
    /// atomic counter.
    ///
    /// [`increment_total_agent_spawns`]: Self::increment_total_agent_spawns
    fn get_total_agent_spawns(&self) -> u64 {
        0
    }

    /// Increment the per-session subagent-spawn counter (claude 2.1.212
    /// `taskRegistry.incrementTotalAgentSpawns()`), called by the `Agent` tool
    /// once a spawn clears the cap gate. Default impl is a no-op so existing mock
    /// handles compile unchanged.
    fn increment_total_agent_spawns(&self) {}

    /// Atomically reserve one per-session subagent-spawn slot, returning the new
    /// count on success or the already-reached count on failure. The default
    /// preserves legacy mock behavior; the production registry overrides it
    /// with a compare/update loop so parallel `Agent` calls cannot race past the
    /// session cap.
    fn try_reserve_total_agent_spawn(&self, cap: u64) -> Result<u64, u64> {
        let current = self.get_total_agent_spawns();
        if current >= cap {
            Err(current)
        } else {
            self.increment_total_agent_spawns();
            Ok(current.saturating_add(1))
        }
    }

    /// Release a reservation when the runtime rejects the launch before a
    /// subagent slot is allocated (for example, a concurrent pool-cap race).
    /// Default no-op keeps legacy stateless handles source-compatible.
    fn release_total_agent_spawn_reservation(&self) {}

    /// Reserve `n` lifetime spawn slots at once (Fusion panels). Rolls back
    /// on failure so a partial hold cannot leak. Default loops the single-slot
    /// reserve so mocks keep working.
    fn try_reserve_total_agent_spawns(&self, n: u64, cap: u64) -> Result<u64, u64> {
        if n == 0 {
            return Ok(self.get_total_agent_spawns());
        }
        let mut last = 0_u64;
        for i in 0..n {
            match self.try_reserve_total_agent_spawn(cap) {
                Ok(count) => last = count,
                Err(count) => {
                    self.release_total_agent_spawn_reservations(i);
                    return Err(count);
                }
            }
        }
        Ok(last)
    }

    /// Release `n` previously reserved lifetime spawn slots.
    fn release_total_agent_spawn_reservations(&self, n: u64) {
        for _ in 0..n {
            self.release_total_agent_spawn_reservation();
        }
    }

    /// Session-wide count of WebSearch calls executed so far — 1:1 with
    /// claude-code's `taskRegistry.getWebSearchCalls(){return n}` (parity
    /// 2.1.212). The `WebSearch` tool reads this before every search and compares
    /// it against `CLAUDE_CODE_MAX_WEB_SEARCHES_PER_SESSION` (default 200) to gate
    /// the session budget. Default `0` so existing mock handles compile unchanged
    /// and a null / no-registry context never caps — byte-identical to the
    /// binary's stub `getWebSearchCalls(){return 0}`.
    fn web_search_calls(&self) -> u32 {
        0
    }

    /// Increment the session WebSearch counter — 1:1 with
    /// `taskRegistry.incrementWebSearchCalls(){n++}`. Called once for every
    /// non-capped `WebSearch` invocation. Default no-op so mock handles and
    /// registry-less contexts are unaffected.
    fn increment_web_search_calls(&self) {}

    /// Reset the session WebSearch counter to zero — 1:1 with
    /// `taskRegistry.resetWebSearchCalls(){n=0}`. Default no-op.
    fn reset_web_search_calls(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    #[test]
    fn trait_is_object_safe() {
        let _: Option<Arc<dyn TaskRegistryHandle>> = None;
    }

    /// Fixed roster of running tasks, one of each `task_type` the ceiling
    /// teardown must decide about, plus the two it must leave alone.
    struct FixedRoster {
        tasks: Vec<TaskRecord>,
        killed: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl TaskRegistryHandle for FixedRoster {
        async fn create(&self, _input: TaskCreateInput) -> Result<TaskRecord, TaskRegistryError> {
            unimplemented!("not exercised by this test")
        }
        async fn get(&self, _id: &str) -> Result<Option<TaskRecord>, TaskRegistryError> {
            unimplemented!("not exercised by this test")
        }
        async fn list(&self, _filter: TaskListFilter) -> Result<Vec<TaskRecord>, TaskRegistryError> {
            Ok(self.tasks.clone())
        }
        async fn update(
            &self,
            _id: &str,
            _patch: TaskUpdatePatch,
        ) -> Result<TaskRecord, TaskRegistryError> {
            unimplemented!("not exercised by this test")
        }
        async fn set_status(&self, _id: &str, _status: &str) -> Result<TaskRecord, TaskRegistryError> {
            unimplemented!("not exercised by this test")
        }
        async fn kill(&self, id: &str) -> Result<TaskRecord, TaskRegistryError> {
            self.killed.lock().unwrap().push(id.to_string());
            self.tasks
                .iter()
                .find(|t| t.task_id == id)
                .cloned()
                .ok_or_else(|| TaskRegistryError::NotFound(id.to_string()))
        }
        async fn output(
            &self,
            _id: &str,
            _offset: Option<u64>,
        ) -> Result<TaskOutputChunk, TaskRegistryError> {
            unimplemented!("not exercised by this test")
        }
    }

    fn running(task_id: &str, task_type: &str) -> TaskRecord {
        TaskRecord {
            task_id: task_id.into(),
            task_type: task_type.into(),
            status: "running".into(),
            description: "d".into(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn ceiling_teardown_stops_local_fusion_alongside_agent_and_workflow() {
        // Finding G003(b): before `local_fusion` was added to this match, the
        // `_ => false` arm made a running Fusion run invisible to
        // `--max-budget-usd`'s ceiling teardown — it kept spending past the
        // cap while `local_agent`/`local_workflow` tasks correctly stopped.
        let registry = FixedRoster {
            tasks: vec![
                running("a1", "local_agent"),
                running("w1", "local_workflow"),
                running("f1", "local_fusion"),
                running("b1", "local_bash"),
                running("m1", "mcp_task"),
            ],
            killed: Mutex::new(Vec::new()),
        };
        let before_stop_calls = AtomicUsize::new(0);
        let count = registry
            .stop_background_agents_for_budget(&|| {
                before_stop_calls.fetch_add(1, Ordering::SeqCst);
            })
            .await
            .unwrap();
        assert_eq!(count, 3, "local_agent + local_workflow + local_fusion");
        assert_eq!(before_stop_calls.load(Ordering::SeqCst), 1);
        let mut killed = registry.killed.lock().unwrap().clone();
        killed.sort();
        assert_eq!(
            killed,
            vec!["a1".to_string(), "f1".to_string(), "w1".to_string()],
            "local_bash and mcp_task must be left alone; local_fusion must be stopped"
        );
    }
}

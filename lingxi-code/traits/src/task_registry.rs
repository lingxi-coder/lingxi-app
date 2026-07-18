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
    /// Wire string for the task type — one of the 7 byte-locked variants.
    pub task_type: String,
    /// Human-readable description shown in UI listings.
    pub description: String,
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
    /// 9-char `[bartwmd][0-9a-z]{8}` task id.
    pub task_id: String,
    /// Task type wire string.
    pub task_type: String,
    /// Status wire string.
    pub status: String,
    /// Human-readable description.
    pub description: String,
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
    /// a single tool), so this stays `None` for `monitor_mcp`; `mcp_task` is not
    /// a port task type. Additive default `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// `local_workflow` only: the workflow name, surfaced as the
    /// `background_tasks[].name` field (claude-code `Lic`'s
    /// `r.name = n.workflowName`). The port carries a `workflow_id` rather than a
    /// separate display name, so the id is used. `None` for every other task
    /// type. Additive default `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `local_agent` only: whether the agent is currently backgrounded, used by
    /// the `Stop` / `SubagentStop` `background_tasks` filter (claude-code `wA`:
    /// drop a task when `"isBackgrounded" in e && e.isBackgrounded === false`).
    /// Only `local_agent` tasks carry an `isBackgrounded` field in claude-code,
    /// so this is `None` for every other task type (and such tasks are never
    /// dropped by that filter clause). Additive default `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_backgrounded: Option<bool>,
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
    /// Status wire string (`pending`/`running`/`completed`/`failed`/`killed`).
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
    /// Task type wire string (one of the 7 byte-locked variants). Selects the
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
    /// Process exit code for `local_bash` / `monitor_mcp` tasks, folded into the
    /// summary (e.g. `(exit code 1)`). `None` ⇒ the exit clause is omitted.
    pub exit_code: Option<i32>,
    /// Failure reason for a `local_agent` task, folded into the `failed`
    /// summary (`Agent "…" came to rest with an error: {error}`). `None` falls
    /// back to `Unknown error` (claude-code `error || 'Unknown error'`).
    pub error: Option<String>,
    /// `local_agent` only: the agent's final text response → the optional
    /// `<result>` section (escaped). `None` ⇒ the section is omitted (the
    /// byte-faithful "no result" case — claude-code's `s ? <result>… : ''`).
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

    /// List `local_workflow` runs (running AND completed-but-not-evicted) for the
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
    /// overrides both this and [`increment_total_agent_spawns`] with a real
    /// per-session counter.
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn trait_is_object_safe() {
        let _: Option<Arc<dyn TaskRegistryHandle>> = None;
    }
}

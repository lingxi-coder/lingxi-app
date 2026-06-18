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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskRecord {
    /// 9-char `[bartwmd][0-9a-z]{8}` task id.
    pub task_id: String,
    /// Task type wire string.
    pub task_type: String,
    /// Status wire string.
    pub status: String,
    /// Human-readable description.
    pub description: String,
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
    /// }))` in both the non-blocking and blocking terminal branches. A terminal +
    /// now-notified task is eagerly evicted from the registry (claude-code
    /// `evictTerminalTask`). A `None`/unknown id is a no-op for callers that
    /// cannot guarantee the task still exists; the default impl is a no-op so
    /// existing mock handles compile unchanged.
    async fn mark_notified(&self, _id: &str) -> Result<(), TaskRegistryError> {
        Ok(())
    }
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

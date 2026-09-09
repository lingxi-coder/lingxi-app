//! `PollerFeed` — the live [`MultiAgentFeed`], reading the production task
//! registry through the narrow `platform_api::task_registry::TaskRegistryHandle`
//! trait (no dependency on the concrete `tasks` crate). (M9-01)

use crate::multiagent::adapter::MultiAgentFeed;
use crate::multiagent::event::MultiAgentEvent;
use crate::multiagent::state::{TaskRow, WorkflowRow};
use async_trait::async_trait;
use platform_api::task_registry::{TaskListFilter, TaskRecord, TaskRegistryHandle, WorkflowRecord};
use std::sync::Arc;

/// Maps a `TaskRecord` (the trait's wire shape) onto a `TaskRow` (the TUI's
/// presentation shape). Total — every `TaskRecord` field has a `TaskRow` home.
#[must_use]
pub fn task_row_from_record(r: TaskRecord) -> TaskRow {
    TaskRow {
        unread: !r.notified,
        model: r.model,
        effort: r.effort,
        awaiting_plan_approval: r.awaiting_plan_approval,
        task_id: r.task_id,
        task_type: r.task_type,
        status: r.status,
        description: r.description,
        command: r.command,
    }
}

/// Maps a `WorkflowRecord` onto a `WorkflowRow` (the picker's presentation
/// shape). Total — every field has a home.
#[must_use]
pub fn workflow_row_from_record(r: WorkflowRecord) -> WorkflowRow {
    WorkflowRow {
        task_id: r.task_id,
        run_id: r.run_id,
        name: r.name,
        status: r.status,
        description: r.description,
        current_step: r.current_step,
        started_at_ms: r.started_at_ms,
        ended_at_ms: r.ended_at_ms,
        script: r.script,
        script_path: r.script_path,
        args: r.args,
        agent_count: usize::try_from(r.agent_count).unwrap_or(usize::MAX),
        total_tokens: r.total_tokens,
        phases: Vec::new(),
    }
}

/// Sort workflow rows newest-first for the `/workflows` picker (oracle `zoa`
/// sorts `b.task.startTime - a.task.startTime`). The backing `TaskRegistry` is a
/// `HashMap` with no inherent order, so this is the sole ordering: started runs
/// by `started_at_ms` descending, never-started (`None`) last, `task_id` as a
/// stable tiebreak.
pub fn sort_workflows_newest_first(rows: &mut [WorkflowRow]) {
    rows.sort_by(|a, b| {
        b.started_at_ms
            .cmp(&a.started_at_ms)
            .then_with(|| a.task_id.cmp(&b.task_id))
    });
}

/// Live feed: each `poll()` lists the registry and emits one `TasksRefreshed`.
pub struct PollerFeed {
    tasks: Arc<dyn TaskRegistryHandle>,
}

impl PollerFeed {
    /// Wrap a task-registry handle.
    #[must_use]
    pub fn new(tasks: Arc<dyn TaskRegistryHandle>) -> Self {
        Self { tasks }
    }
}

#[async_trait]
impl MultiAgentFeed for PollerFeed {
    async fn poll(&self) -> Vec<MultiAgentEvent> {
        // A failed list is surfaced as "no change" (empty) rather than a panic;
        // the registry error path is owned by the tools, not the read-only UI.
        let rows = self
            .tasks
            .list(TaskListFilter::default())
            .await
            .unwrap_or_default()
            .into_iter()
            .map(task_row_from_record)
            .collect::<Vec<_>>();
        vec![MultiAgentEvent::TasksRefreshed(rows)]
    }

    /// (BGTASK-3) Delegate to the registry's `kill`; the next `poll()` picks
    /// up the resulting status change.
    async fn kill(&self, task_id: &str) -> Result<(), String> {
        self.tasks
            .kill(task_id)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use platform_api::task_registry::{
        TaskCreateInput, TaskOutputChunk, TaskRegistryError, TaskUpdatePatch,
    };

    /// Minimal stand-in `TaskRegistryHandle`: `list` returns a canned set; the
    /// other methods are unused by `PollerFeed::poll` and return trivially.
    /// (BGTASK-3) `killed` records every id passed to `kill`, for
    /// `PollerFeed::kill` delegation tests.
    struct StubTasks {
        rows: Vec<TaskRecord>,
        killed: std::sync::Mutex<Vec<String>>,
    }

    impl StubTasks {
        fn new(rows: Vec<TaskRecord>) -> Self {
            Self {
                rows,
                killed: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl TaskRegistryHandle for StubTasks {
        async fn create(&self, _: TaskCreateInput) -> Result<TaskRecord, TaskRegistryError> {
            Err(TaskRegistryError::Internal("unused".into()))
        }
        async fn get(&self, _: &str) -> Result<Option<TaskRecord>, TaskRegistryError> {
            Ok(None)
        }
        async fn list(&self, _: TaskListFilter) -> Result<Vec<TaskRecord>, TaskRegistryError> {
            Ok(self.rows.clone())
        }
        async fn update(
            &self,
            _: &str,
            _: TaskUpdatePatch,
        ) -> Result<TaskRecord, TaskRegistryError> {
            Err(TaskRegistryError::Internal("unused".into()))
        }
        async fn set_status(&self, _: &str, _: &str) -> Result<TaskRecord, TaskRegistryError> {
            Err(TaskRegistryError::Internal("unused".into()))
        }
        async fn kill(&self, id: &str) -> Result<TaskRecord, TaskRegistryError> {
            self.killed.lock().expect("poisoned").push(id.to_string());
            self.rows
                .iter()
                .find(|r| r.task_id == id)
                .cloned()
                .ok_or_else(|| TaskRegistryError::NotFound(id.to_string()))
        }
        async fn output(
            &self,
            _: &str,
            _: Option<u64>,
        ) -> Result<TaskOutputChunk, TaskRegistryError> {
            Ok(TaskOutputChunk::default())
        }
    }

    #[tokio::test]
    async fn poll_maps_registry_records_to_task_rows() {
        let stub = Arc::new(StubTasks::new(vec![
            TaskRecord {
                task_id: "b00000001".into(),
                task_type: "local_bash".into(),
                status: "running".into(),
                description: "cargo build".into(),
                command: None,
                ..Default::default()
            },
            TaskRecord {
                task_id: "a00000002".into(),
                task_type: "local_agent".into(),
                status: "completed".into(),
                description: "explore".into(),
                command: None,
                ..Default::default()
            },
        ]));
        let feed = PollerFeed::new(stub);
        match feed.poll().await.as_slice() {
            [MultiAgentEvent::TasksRefreshed(rows)] => {
                assert_eq!(rows.len(), 2);
                assert_eq!(rows[0].task_id, "b00000001");
                assert_eq!(rows[0].task_type, "local_bash");
                assert_eq!(rows[1].status, "completed");
                assert_eq!(rows[1].description, "explore");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[tokio::test]
    async fn kill_delegates_to_the_registry_handle() {
        // (BGTASK-3)
        let stub = Arc::new(StubTasks::new(vec![TaskRecord {
            task_id: "b00000001".into(),
            task_type: "local_bash".into(),
            status: "running".into(),
            description: "cargo build".into(),
            command: None,
            ..Default::default()
        }]));
        let feed = PollerFeed::new(stub.clone());
        assert!(feed.kill("b00000001").await.is_ok());
        assert_eq!(stub.killed.lock().unwrap().as_slice(), ["b00000001"]);
        // Unknown id -> the registry's NotFound surfaces as Err.
        assert!(feed.kill("nonexistent").await.is_err());
    }

    #[test]
    fn sort_workflows_newest_first_orders_by_start_desc_then_id() {
        let mk = |id: &str, start: Option<u64>| WorkflowRow {
            task_id: id.to_string(),
            started_at_ms: start,
            ..WorkflowRow::default()
        };
        let mut rows = vec![
            mk("wc", Some(100)),
            mk("wa", None),      // never-started -> last
            mk("wb", Some(300)), // newest
            mk("wd", Some(300)), // tie with wb -> id breaks it (wb < wd)
        ];
        sort_workflows_newest_first(&mut rows);
        let ids: Vec<&str> = rows.iter().map(|r| r.task_id.as_str()).collect();
        assert_eq!(ids, ["wb", "wd", "wc", "wa"]);
    }

    #[test]
    fn workflow_row_from_record_preserves_inline_script() {
        let row = workflow_row_from_record(WorkflowRecord {
            task_id: "w12345678".into(),
            script: Some("export const meta = {};".into()),
            ..WorkflowRecord::default()
        });
        assert_eq!(row.script.as_deref(), Some("export const meta = {};"));
    }
}

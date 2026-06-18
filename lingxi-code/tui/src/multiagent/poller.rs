//! `PollerFeed` — the live [`MultiAgentFeed`], reading the production task
//! registry through the narrow `traits::task_registry::TaskRegistryHandle`
//! trait (no dependency on the concrete `tasks` crate). (M9-01)

use crate::multiagent::adapter::MultiAgentFeed;
use crate::multiagent::event::MultiAgentEvent;
use crate::multiagent::state::TaskRow;
use async_trait::async_trait;
use std::sync::Arc;
use traits::task_registry::{TaskListFilter, TaskRecord, TaskRegistryHandle};

/// Maps a `TaskRecord` (the trait's wire shape) onto a `TaskRow` (the TUI's
/// presentation shape). Total — every `TaskRecord` field has a `TaskRow` home.
#[must_use]
pub fn task_row_from_record(r: TaskRecord) -> TaskRow {
    TaskRow {
        task_id: r.task_id,
        task_type: r.task_type,
        status: r.status,
        description: r.description,
    }
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use traits::task_registry::{
        TaskCreateInput, TaskOutputChunk, TaskRegistryError, TaskUpdatePatch,
    };

    /// Minimal stand-in `TaskRegistryHandle`: `list` returns a canned set; the
    /// other methods are unused by `PollerFeed::poll` and return trivially.
    struct StubTasks {
        rows: Vec<TaskRecord>,
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
        async fn kill(&self, _: &str) -> Result<TaskRecord, TaskRegistryError> {
            Err(TaskRegistryError::Internal("unused".into()))
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
        let stub = Arc::new(StubTasks {
            rows: vec![
                TaskRecord {
                    task_id: "b00000001".into(),
                    task_type: "local_bash".into(),
                    status: "running".into(),
                    description: "cargo build".into(),
                    command: None,
                },
                TaskRecord {
                    task_id: "a00000002".into(),
                    task_type: "local_agent".into(),
                    status: "completed".into(),
                    description: "explore".into(),
                    command: None,
                },
            ],
        });
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
}

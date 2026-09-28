use super::EngineCommandRouter;
use client_adapter::lowering::lower_task_record;
use client_adapter::ClientEventSink;
use client_protocol::events::ClientEvent;
use client_protocol::events::ErrorKindDto;
use client_protocol::listings::TaskStatusDto;
use platform_api::task_registry::TaskListFilter;
use platform_api::task_registry::TaskRegistryHandle;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// A correlated snapshot cannot be confused with periodic task-row pushes.
pub(super) async fn emit_correlated_task_list(
    tasks: &dyn TaskRegistryHandle,
    sink: &dyn ClientEventSink,
    filter: TaskListFilter,
    request_id: String,
) {
    let (active_count, error) = match tasks.list(filter).await {
        Ok(records) => {
            let active_count = records
                .iter()
                .filter(|record| {
                    !matches!(
                        record.status.as_str(),
                        "completed" | "failed" | "killed" | "cancelled"
                    )
                })
                .count() as u64;
            for record in &records {
                sink.emit(ClientEvent::TaskRow {
                    task: lower_task_record(record),
                })
                .await;
            }
            (active_count, None)
        }
        Err(error) => (0, Some(format!("task list failed: {error}"))),
    };
    sink.emit(ClientEvent::TaskListComplete {
        request_id,
        active_count,
        error,
    })
    .await;
}

/// Shared helper: list tasks through the handle and emit one `TaskRow` per task.
pub(super) async fn emit_task_rows(
    tasks: &dyn TaskRegistryHandle,
    sink: &dyn ClientEventSink,
    filter: TaskListFilter,
) {
    match tasks.list(filter).await {
        Ok(records) => {
            for rec in &records {
                sink.emit(ClientEvent::TaskRow {
                    task: lower_task_record(rec),
                })
                .await;
            }
        }
        Err(e) => {
            sink.emit(ClientEvent::Error {
                kind: ErrorKindDto::Internal,
                message: format!("task list failed: {e}"),
            })
            .await;
        }
    }
}

/// Handle to the background-task poll loop spawned by
/// [`EngineCommandRouter::spawn_task_poll`]. Cancels the loop on [`Self::stop`]
/// or drop.
pub struct TaskPoll {
    pub(super) token: CancellationToken,
    pub(super) join: Option<tokio::task::JoinHandle<()>>,
}

impl TaskPoll {
    /// Stop the poll loop. Idempotent.
    pub fn stop(&self) {
        self.token.cancel();
    }

    /// Stop the loop and await its termination.
    pub async fn shutdown(mut self) {
        self.token.cancel();
        if let Some(join) = self.join.take() {
            let _ = join.await;
        }
    }
}

impl Drop for TaskPoll {
    fn drop(&mut self) {
        self.token.cancel();
        if let Some(join) = self.join.as_ref() {
            join.abort();
        }
    }
}

/// Lower a [`TaskStatusDto`] back to the registry's wire status string for the
/// `TaskListFilter` (the inverse of `client_adapter::lowering::lower_task_status`).
pub(super) fn task_status_wire(status: TaskStatusDto) -> String {
    match status {
        TaskStatusDto::Running => "running",
        TaskStatusDto::Paused => "paused",
        TaskStatusDto::Completed => "completed",
        TaskStatusDto::Failed => "failed",
        // The DTO's user-stop variant maps to the registry's terminal "killed".
        TaskStatusDto::Cancelled => "killed",
        // `Pending` and the `#[non_exhaustive]` catch-all both map to the safe
        // non-terminal default (the inverse of `lower_task_status`'s fallback).
        _ => "pending",
    }
    .to_string()
}

impl EngineCommandRouter {
    /// Spawn the background-task poll loop (matches the TUI: `TaskRegistryHandle::list`
    /// on an interval). Each tick lists the tasks and emits one
    /// [`ClientEvent::TaskRow`] per task through `sink`. The returned
    /// [`TaskPoll`] stops the loop on [`TaskPoll::stop`] or drop.
    #[must_use]
    pub fn spawn_task_poll(&self, sink: Arc<dyn ClientEventSink>, interval: Duration) -> TaskPoll {
        let tasks = self.tasks.clone();
        let token = CancellationToken::new();
        let child = token.clone();
        let join = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            loop {
                tokio::select! {
                    () = child.cancelled() => break,
                    _ = ticker.tick() => {
                        emit_task_rows(&*tasks, &*sink, TaskListFilter::default()).await;
                    }
                }
            }
        });
        TaskPoll {
            token,
            join: Some(join),
        }
    }
    /// List tasks (optionally filtered) and emit one `TaskRow` per task.
    pub(super) async fn emit_task_list(&self, filter: TaskListFilter, sink: &dyn ClientEventSink) {
        emit_task_rows(&*self.tasks, sink, filter).await;
    }
}

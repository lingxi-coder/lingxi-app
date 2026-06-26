//! Bridge a teammate's [`tasks::handlers::TaskStatusSink`] transitions onto
//! [`TeamRegistry::update_status`].
//!
//! The in-process teammate handler reports lifecycle transitions through the
//! narrow [`TaskStatusSink`] seam (it does not hold a [`TeamRegistry`]
//! reference). [`CoordinatorStatusSink`] adapts those transitions back onto the
//! coordinator's [`TeamRegistry`], scoped to the transitions the sink can
//! actually drive, and (re-)pushes the live active-worker count to the
//! orchestrator-facing [`OutputStream`] after each state change.
//!
//! ## Lossy surface (by design)
//!
//! A *persistent* teammate emits [`TaskStatus::Completed`] at the end of every
//! turn-set yet keeps running (it then parks awaiting the next message — see
//! `tasks::handlers::in_process_teammate::terminal_status`). `Completed` is
//! therefore NOT a terminal transition here: mapping it to
//! [`WorkerStatus::Completed`] would prematurely flip a still-running worker to
//! a terminal state and drop it from the active-worker count. Only `Failed` /
//! `Killed` truly end the teammate, so only those (plus `Running`) drive a
//! status transition.

use crate::team_registry::{TeamRegistry, WorkerStatus};
use async_trait::async_trait;
use std::sync::Arc;
use tasks::handlers::TaskStatusSink;
use tasks::state::TaskStatus;
use traits::OutputStream;

/// The fixed activity label used when a teammate transitions to `Running`.
///
/// [`WorkerStatus`] derives `PartialEq` over the `activity` string and the
/// [`TaskStatusSink`] carries no per-turn activity text, so a fixed label keeps
/// the mapping deterministic. Richer per-turn text is a follow-up.
const RUNNING_ACTIVITY: &str = "running";

/// The fixed error message recorded when a teammate transitions to `Failed`.
///
/// [`TaskStatus::Failed`] is a payload-less enum variant — the sink does not
/// carry the originating error string — so we record a fixed sentinel. The
/// authoritative error text remains observable on the task's spool/state.
const FAILED_ERROR: &str = "teammate task failed";

/// Adapts [`TaskStatusSink`] transitions onto a coordinator's
/// [`TeamRegistry`], then pushes the live active-worker count downstream.
pub struct CoordinatorStatusSink {
    team: Arc<TeamRegistry>,
    output: Arc<dyn OutputStream>,
}

impl CoordinatorStatusSink {
    /// Construct a sink bridging `team` and pushing status updates to `output`.
    #[must_use]
    pub fn new(team: Arc<TeamRegistry>, output: Arc<dyn OutputStream>) -> Self {
        Self { team, output }
    }

    /// Map a [`TaskStatus`] to the [`WorkerStatus`] it drives, if any.
    ///
    /// `Pending`/`Completed` produce `None` (no transition — see the module
    /// docs on the persistent-teammate `Completed` surface).
    fn worker_status_for(status: TaskStatus) -> Option<WorkerStatus> {
        match status {
            TaskStatus::Running => Some(WorkerStatus::Working {
                activity: RUNNING_ACTIVITY.to_string(),
            }),
            TaskStatus::Failed => Some(WorkerStatus::Failed {
                error: FAILED_ERROR.to_string(),
            }),
            TaskStatus::Killed => Some(WorkerStatus::Killed),
            // No transition: `Pending` predates the worker link; `Completed` is
            // emitted per turn-set by a still-running persistent teammate.
            TaskStatus::Pending | TaskStatus::Completed => None,
        }
    }
}

#[async_trait]
impl TaskStatusSink for CoordinatorStatusSink {
    async fn set_status(&self, task_id: &str, status: TaskStatus) {
        let Some(worker_status) = Self::worker_status_for(status) else {
            return;
        };
        // Resolve the worker keyed on the handler-generated task id. Unknown
        // ids are a no-op (no panic) — the link may not be written back yet, or
        // the worker may already have been deleted.
        let Some(worker) = self.team.find_by_task_id(task_id).await else {
            return;
        };
        self.team
            .update_status(&worker.agent_id, worker_status)
            .await;

        // PUSH the freshly-computed active-worker count + team name downstream.
        let active = self.team.active_worker_count().await;
        let team_name = self.team.team_name().await;
        self.output
            .emit_coordinator_status(active, team_name.as_deref())
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::AgentId;
    use std::sync::Mutex as StdMutex;
    use traits::CostSnapshot;

    /// Spy [`OutputStream`] recording every `emit_coordinator_status` call as
    /// `(active_workers, team)`. All other emit methods are no-ops.
    #[derive(Default)]
    struct SpyOutput {
        statuses: StdMutex<Vec<(u32, Option<String>)>>,
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
        async fn emit_end_turn(&self, _stop_reason: &str, _cost: &CostSnapshot) {}
        async fn emit_coordinator_status(&self, active_workers: u32, team: Option<&str>) {
            self.statuses
                .lock()
                .unwrap()
                .push((active_workers, team.map(str::to_string)));
        }
    }

    /// Build a registry with one worker linked to `task_id`, returning the
    /// registry, the spy output, and the constructed sink.
    async fn fixture(task_id: &str) -> (Arc<TeamRegistry>, Arc<SpyOutput>, CoordinatorStatusSink) {
        let team = Arc::new(TeamRegistry::new(AgentId::new()));
        let agent_id = team
            .spawn_worker("explorer".into(), "alpha".into(), String::new())
            .await
            .unwrap();
        team.set_task_id(&agent_id, task_id.into()).await;
        let output = Arc::new(SpyOutput::default());
        let sink = CoordinatorStatusSink::new(team.clone(), output.clone());
        (team, output, sink)
    }

    fn status_of(workers: &[crate::team_registry::WorkerAgent]) -> &WorkerStatus {
        &workers[0].status
    }

    #[tokio::test]
    async fn running_maps_to_working() {
        let (team, _out, sink) = fixture("task-1").await;

        sink.set_status("task-1", TaskStatus::Running).await;

        let workers = team.list().await;
        assert_eq!(
            status_of(&workers),
            &WorkerStatus::Working {
                activity: "running".into()
            }
        );
    }

    #[tokio::test]
    async fn completed_does_not_transition() {
        let (team, _out, sink) = fixture("task-1").await;

        // First, transition to Working via Running.
        sink.set_status("task-1", TaskStatus::Running).await;
        // A persistent teammate emits Completed per turn-set but keeps running:
        // it must NOT flip the worker to a terminal status.
        sink.set_status("task-1", TaskStatus::Completed).await;

        let workers = team.list().await;
        assert_eq!(
            status_of(&workers),
            &WorkerStatus::Working {
                activity: "running".into()
            },
            "Completed must leave the still-running worker in Working"
        );
    }

    #[tokio::test]
    async fn failed_and_killed_map_through() {
        // Failed.
        let (team, _out, sink) = fixture("task-f").await;
        sink.set_status("task-f", TaskStatus::Failed).await;
        assert_eq!(
            status_of(&team.list().await),
            &WorkerStatus::Failed {
                error: FAILED_ERROR.into()
            }
        );

        // Killed.
        let (team, _out, sink) = fixture("task-k").await;
        sink.set_status("task-k", TaskStatus::Killed).await;
        assert_eq!(status_of(&team.list().await), &WorkerStatus::Killed);
    }

    #[tokio::test]
    async fn unknown_task_id_is_noop() {
        let (team, out, sink) = fixture("task-1").await;

        // No worker is linked to "ghost": must not panic, must not transition,
        // and must not push a status event.
        sink.set_status("ghost", TaskStatus::Running).await;

        let workers = team.list().await;
        assert_eq!(status_of(&workers), &WorkerStatus::Idle);
        assert_eq!(out.calls(), 0, "no emit for an unknown task id");
    }

    #[tokio::test]
    async fn emit_fires_with_active_count() {
        let (_team, out, sink) = fixture("task-1").await;

        sink.set_status("task-1", TaskStatus::Running).await;

        // The single worker is now Working (non-terminal) → active count 1,
        // and the team name is unset in this fixture.
        assert_eq!(out.last(), Some((1, None)));
        assert_eq!(out.calls(), 1);
    }

    #[tokio::test]
    async fn emit_carries_team_name() {
        let (team, out, sink) = fixture("task-1").await;
        team.set_team_name(Some("alpha".into())).await;

        sink.set_status("task-1", TaskStatus::Running).await;

        assert_eq!(out.last(), Some((1, Some("alpha".to_string()))));
    }
}

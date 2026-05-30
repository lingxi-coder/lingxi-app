//! `apply_multiagent_event` — the single mutation seam for multi-agent state.
//! Mirrors `crate::streaming::apply_event`: a pure mutator whose only side
//! effects are `state` mutation and `notify.notify_one()`. (M9-01)

use crate::multiagent::event::MultiAgentEvent;
use crate::state::AppState;
use tokio::sync::Notify;

/// Apply one [`MultiAgentEvent`] to `state` and signal the renderer.
///
/// - `TasksRefreshed(v)` → replace `state.multiagent.tasks` with `v`.
/// - `WorkersRefreshed(v)` → replace `state.multiagent.workers` with `v`.
///
/// After mutation, calls `notify.notify_one()` (the render loop debounces).
pub fn apply_multiagent_event(state: &mut AppState, ev: MultiAgentEvent, notify: &Notify) {
    match ev {
        MultiAgentEvent::TasksRefreshed(tasks) => state.multiagent.tasks = tasks,
        MultiAgentEvent::WorkersRefreshed(workers) => state.multiagent.workers = workers,
    }
    notify.notify_one();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multiagent::state::TaskRow;
    use crate::state::AppState;

    fn row(id: &str) -> TaskRow {
        TaskRow {
            task_id: id.into(),
            task_type: "local_bash".into(),
            status: "running".into(),
            description: "x".into(),
        }
    }

    #[test]
    fn tasks_refreshed_replaces_the_slice() {
        let mut s = AppState::default_for_tests();
        let n = Notify::new();
        apply_multiagent_event(
            &mut s,
            MultiAgentEvent::TasksRefreshed(vec![row("b11111111"), row("b22222222")]),
            &n,
        );
        assert_eq!(s.multiagent.tasks.len(), 2);
        // A second refresh REPLACES (not appends).
        apply_multiagent_event(
            &mut s,
            MultiAgentEvent::TasksRefreshed(vec![row("b33333333")]),
            &n,
        );
        assert_eq!(s.multiagent.tasks.len(), 1);
        assert_eq!(s.multiagent.tasks[0].task_id, "b33333333");
    }

    #[tokio::test]
    async fn apply_calls_notify_one() {
        let mut s = AppState::default_for_tests();
        let n = Notify::new();
        let waiter = n.notified();
        tokio::pin!(waiter);
        apply_multiagent_event(&mut s, MultiAgentEvent::WorkersRefreshed(vec![]), &n);
        let poll = futures::poll!(waiter.as_mut());
        assert!(matches!(poll, std::task::Poll::Ready(())));
    }
}

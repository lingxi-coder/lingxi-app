//! `MultiAgentEvent` — the single output type both feeds produce and the
//! single input `apply_multiagent_event` consumes. (M9-01)

use crate::multiagent::state::{TaskRow, WorkerRow};

/// One multi-agent state update. Feeds emit full-snapshot refresh events
/// (deterministic + idempotent); the mutator replaces the corresponding
/// `MultiAgentState` slice wholesale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MultiAgentEvent {
    /// Replace the task list with this snapshot.
    TasksRefreshed(Vec<TaskRow>),
    /// Replace the worker roster with this snapshot.
    WorkersRefreshed(Vec<WorkerRow>),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variants_construct_and_compare() {
        let a = MultiAgentEvent::TasksRefreshed(vec![TaskRow {
            task_id: "b12345678".into(),
            task_type: "local_bash".into(),
            status: "running".into(),
            description: "build".into(),
        }]);
        let b = a.clone();
        assert_eq!(a, b);
        assert_ne!(a, MultiAgentEvent::WorkersRefreshed(vec![]));
    }
}

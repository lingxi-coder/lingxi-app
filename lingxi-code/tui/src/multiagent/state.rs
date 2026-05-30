//! Multi-agent presentation model. (M9-01)
//!
//! Pure data held on `AppState`, mutated only by
//! [`crate::multiagent::apply::apply_multiagent_event`]. Renderers (M9-03+)
//! are pure functions of this state.

/// One background task as surfaced to the TUI. Mirrors the field shape of
/// `traits::task_registry::TaskRecord` (the live task path) so the poller
/// maps one-to-one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskRow {
    /// 9-char `[bartwmd][0-9a-z]{8}` task id.
    pub task_id: String,
    /// Task type wire string (e.g. `"local_bash"`).
    pub task_type: String,
    /// Status wire string (e.g. `"running"`).
    pub status: String,
    /// Human-readable description.
    pub description: String,
}

/// One teammate/worker row. Populated from the coordinator surface in M9-06;
/// in M9-01 it is filled only by fixtures/tests (no coordinator dependency).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkerRow {
    /// Worker agent id (stringified).
    pub agent_id: String,
    /// Display name.
    pub name: String,
    /// Agent-type string (e.g. `"explorer"`).
    pub agent_type: String,
    /// Simplified status label.
    pub status: String,
}

/// Aggregate multi-agent presentation state owned by `AppState`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MultiAgentState {
    /// Background tasks, newest-first as the feed orders them.
    pub tasks: Vec<TaskRow>,
    /// Teammate/worker roster.
    pub workers: Vec<WorkerRow>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_empty() {
        let s = MultiAgentState::default();
        assert!(s.tasks.is_empty());
        assert!(s.workers.is_empty());
    }
}

//! Multi-agent presentation model. (M9-01)
//!
//! Pure data held on `AppState`, mutated only by
//! `crate::multiagent::apply::apply_multiagent_event`. Renderers (M9-03+)
//! are pure functions of this state.

/// One background task as surfaced to the TUI. Mirrors the field shape of
/// `platform_api::task_registry::TaskRecord` (the live task path) so the poller
/// maps one-to-one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskRow {
    /// Plan approval waits take precedence over the task lifecycle label.
    pub awaiting_plan_approval: bool,
    /// 9-char `[bartwmdks][0-9a-z]{8}` task id.
    pub task_id: String,
    /// Task type wire string (e.g. `"local_bash"`).
    pub task_type: String,
    /// Status wire string (e.g. `"running"`).
    pub status: String,
    /// Human-readable description.
    pub description: String,
    /// (BASH-ROW-USES-DESCRIPTION-NOT-COMMAND) The shell command, for
    /// `local_bash` tasks only — claude-code's `BackgroundTask.tsx` shows
    /// this (not `description`) for non-monitor local-shell rows. `None`
    /// for every other task type.
    pub command: Option<String>,
}

/// One agent within a workflow phase, parsed from the run's output spool
/// (`[workflow_agent] {json}` lines). `state` is the latest lifecycle state seen
/// for the agent (`start`/`done`/`error`/`cached`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkflowAgentRow {
    /// Workflow-global agent invocation index. Stable across `start` → terminal
    /// lifecycle events even when the later event gains an `agent_id`.
    pub index: u64,
    /// The agent's display label (from `agent()`'s label/prompt).
    pub label: String,
    /// Latest lifecycle state: `start` / `done` / `error` / `cached`.
    pub state: String,
    /// First queue timestamp observed for this agent, if the live feed supplied
    /// one. The task spool does not carry this, so rows parsed from history may
    /// leave it unset.
    pub queued_at_ms: Option<u64>,
    /// First "really allocated" timestamp observed for this agent. Used to
    /// distinguish a merely queued agent from one that has actually started.
    pub started_at_ms: Option<u64>,
    /// Latest token count surfaced by the live observer for this agent.
    pub tokens: u64,
    /// Latest tool-call count surfaced by the live observer for this agent.
    pub tool_calls: u64,
}

/// One phase of a workflow run, with the agents that ran under it. Parsed from
/// the spool's `[{index}] === {title} ===` phase markers + `[workflow_agent]`
/// lines carrying a matching `phaseIndex`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkflowPhase {
    /// Phase index as emitted by `phase()` (0-based).
    pub index: usize,
    /// Phase title.
    pub title: String,
    /// Agents that ran under this phase, in first-seen order.
    pub agents: Vec<WorkflowAgentRow>,
}

/// One workflow run as surfaced to the `/workflows` picker. Mirrors
/// `platform_api::task_registry::WorkflowRecord` (identity + timing) and is further
/// enriched by [`crate::multiagent::parse_workflow_spool`] with the agent count
/// and phase/agent tree parsed from the run's output spool. Elapsed is not
/// stored — the picker derives it from `started_at_ms`/`ended_at_ms` against the
/// wall clock at render time.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkflowRow {
    /// 9-char task id (`w…`).
    pub task_id: String,
    /// The effective `wf_…` run id, when known.
    pub run_id: Option<String>,
    /// Display name (workflow `meta.name`).
    pub name: String,
    /// Status wire string (`running`/`completed`/`failed`/…).
    pub status: String,
    /// Launch description (script summary).
    pub description: String,
    /// Current phase step (0-based).
    pub current_step: usize,
    /// Wall-clock start (epoch millis), when known.
    pub started_at_ms: Option<u64>,
    /// Wall-clock end (epoch millis) for a terminal run, when known.
    pub ended_at_ms: Option<u64>,
    /// Persisted script path used by an adopted paused run.
    pub script_path: Option<String>,
    /// Serialized args used by an adopted paused run.
    pub args: Option<String>,
    /// Distinct agents launched by the run (parsed from the spool). `0` until
    /// enriched.
    pub agent_count: usize,
    /// Aggregate workflow tokens when known (terminal outcome or live observer
    /// sum). `0` when the runtime has not reported any yet.
    pub total_tokens: u64,
    /// Phase/agent tree parsed from the run's output spool. Empty until enriched
    /// (or when the run emitted no `phase()`/`agent()` progress).
    pub phases: Vec<WorkflowPhase>,
    /// The run's stored resolved script source. Present ⇒ the `/workflows`
    /// picker offers the `s` "Save dynamic workflow" chord (the oracle gates
    /// `save` on the workflow task's truthy `script`). `None` for adopted paused
    /// records whose stored script is empty.
    pub script: Option<String>,
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

//! `MultiAgentEvent` — the single output type both feeds produce and the
//! single input `apply_multiagent_event` consumes. (M9-01)

use crate::multiagent::state::{TaskRow, WorkerRow, WorkflowAgentRow, WorkflowPhase, WorkflowRow};

/// One structured progress push from a running workflow. This mirrors only the
/// fields the TUI renders; producers may discard logging/usage fields instead
/// of teaching the presentation layer about the task runtime's full payload.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkflowProgressEvent {
    /// Registry task identity (`w…`).
    pub task_id: String,
    /// Workflow runtime identity (`wf_…`).
    pub run_id: String,
    /// Structured progress discriminator (`workflow_phase`/`workflow_agent`).
    pub kind: String,
    /// Phase index or workflow-global agent invocation index, by `kind`.
    pub index: u64,
    /// Phase title on `workflow_phase` events.
    pub title: Option<String>,
    /// Agent display label on `workflow_agent` events.
    pub label: Option<String>,
    /// Phase owning this progress event, when supplied.
    pub phase_index: Option<usize>,
    /// Phase title repeated on an agent lifecycle event, when supplied.
    pub phase_title: Option<String>,
    /// Latest agent lifecycle state (`start`/`done`/`error`/`cached`).
    pub state: Option<String>,
    /// Queue timestamp for an agent that has been scheduled but not yet
    /// allocated, when the runtime supplied one.
    pub queued_at_ms: Option<u64>,
    /// Start timestamp for an agent that has actually allocated, when known.
    pub started_at_ms: Option<u64>,
    /// Latest aggregate token count reported for the agent.
    pub tokens: Option<u64>,
    /// Latest aggregate tool-call count reported for the agent.
    pub tool_calls: Option<u64>,
}

/// One multi-agent state update. Task/worker compatibility feeds retain their
/// idempotent full-refresh events; workflows use pushed upserts and lifecycle
/// deltas so a running view never needs to poll.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MultiAgentEvent {
    /// Replace the task list with this snapshot.
    TasksRefreshed(Vec<TaskRow>),
    /// Replace the worker roster with this snapshot.
    WorkersRefreshed(Vec<WorkerRow>),
    /// Insert or replace one workflow row. Producers emit this when a run is
    /// created/adopted and before forwarding its first progress event.
    WorkflowUpsert(WorkflowRow),
    /// Apply one phase/agent lifecycle update to a workflow already on screen.
    WorkflowProgress(WorkflowProgressEvent),
    /// Apply a workflow status transition without re-reading the registry.
    WorkflowStatusChanged {
        /// Registry task identity (`w…`).
        task_id: String,
        /// Workflow runtime identity used to reject a late prior-run event.
        run_id: Option<String>,
        /// New workflow status wire string.
        status: String,
        /// Terminal wall-clock time, when the producer has one.
        ended_at_ms: Option<u64>,
    },
}

/// Fold one structured workflow progress event into an existing presentation
/// row. Returns `false` when the event targets a different task/run or carries
/// a progress kind the TUI does not render.
pub fn apply_workflow_progress(row: &mut WorkflowRow, progress: &WorkflowProgressEvent) -> bool {
    if row.task_id != progress.task_id {
        return false;
    }
    if let Some(run_id) = row.run_id.as_deref() {
        if run_id != progress.run_id {
            return false;
        }
    } else if !progress.run_id.is_empty() {
        row.run_id = Some(progress.run_id.clone());
    }

    match progress.kind.as_str() {
        "workflow_phase" => {
            let index = progress
                .phase_index
                .unwrap_or_else(|| usize::try_from(progress.index).unwrap_or(usize::MAX));
            let title = progress
                .title
                .as_deref()
                .or(progress.phase_title.as_deref());
            upsert_phase(&mut row.phases, index, title);
            row.current_step = index;
            true
        }
        "workflow_agent" => {
            let phase_index = progress.phase_index.unwrap_or(0);
            let phase = upsert_phase(
                &mut row.phases,
                phase_index,
                progress.phase_title.as_deref(),
            );
            if let Some(agent) = phase
                .agents
                .iter_mut()
                .find(|agent| agent.index == progress.index)
            {
                if let Some(label) = progress.label.as_ref().filter(|label| !label.is_empty()) {
                    agent.label.clone_from(label);
                }
                if let Some(state) = progress.state.as_ref() {
                    agent.state.clone_from(state);
                }
                if let Some(queued_at_ms) = progress.queued_at_ms {
                    agent.queued_at_ms.get_or_insert(queued_at_ms);
                }
                if let Some(started_at_ms) = progress.started_at_ms {
                    agent.started_at_ms.get_or_insert(started_at_ms);
                }
                if let Some(tokens) = progress.tokens {
                    agent.tokens = tokens;
                }
                if let Some(tool_calls) = progress.tool_calls {
                    agent.tool_calls = tool_calls;
                }
            } else {
                phase.agents.push(WorkflowAgentRow {
                    index: progress.index,
                    label: progress.label.clone().unwrap_or_default(),
                    state: progress.state.clone().unwrap_or_default(),
                    queued_at_ms: progress.queued_at_ms,
                    started_at_ms: progress.started_at_ms,
                    tokens: progress.tokens.unwrap_or(0),
                    tool_calls: progress.tool_calls.unwrap_or(0),
                });
            }
            row.agent_count = row.phases.iter().map(|phase| phase.agents.len()).sum();
            row.total_tokens = row
                .phases
                .iter()
                .flat_map(|phase| phase.agents.iter())
                .fold(0, |total, agent| total.saturating_add(agent.tokens));
            true
        }
        _ => false,
    }
}

fn upsert_phase<'a>(
    phases: &'a mut Vec<WorkflowPhase>,
    index: usize,
    title: Option<&str>,
) -> &'a mut WorkflowPhase {
    if let Some(position) = phases.iter().position(|phase| phase.index == index) {
        let phase = &mut phases[position];
        if let Some(title) = title.filter(|title| !title.is_empty()) {
            phase.title = title.to_owned();
        }
        return phase;
    }
    phases.push(WorkflowPhase {
        index,
        title: title.unwrap_or_default().to_string(),
        agents: Vec::new(),
    });
    phases.last_mut().expect("phase was just inserted")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variants_construct_and_compare() {
        let a = MultiAgentEvent::TasksRefreshed(vec![TaskRow {
            unread: false,
            model: None,
            effort: None,
            awaiting_plan_approval: false,
            task_id: "b12345678".into(),
            task_type: "local_bash".into(),
            status: "running".into(),
            description: "build".into(),
            command: None,
            stage: None,
            error: None,
        }]);
        let b = a.clone();
        assert_eq!(a, b);
        assert_ne!(a, MultiAgentEvent::WorkersRefreshed(vec![]));
    }

    #[test]
    fn progress_updates_one_agent_in_place_across_lifecycle_events() {
        let mut row = WorkflowRow {
            task_id: "w123".into(),
            run_id: Some("wf_123".into()),
            ..WorkflowRow::default()
        };
        let mut event = WorkflowProgressEvent {
            task_id: "w123".into(),
            run_id: "wf_123".into(),
            kind: "workflow_agent".into(),
            index: 7,
            label: Some("design".into()),
            phase_index: Some(1),
            phase_title: Some("Design".into()),
            state: Some("start".into()),
            ..WorkflowProgressEvent::default()
        };
        assert!(apply_workflow_progress(&mut row, &event));
        event.state = Some("done".into());
        assert!(apply_workflow_progress(&mut row, &event));

        assert_eq!(row.agent_count, 1);
        assert_eq!(row.phases[0].title, "Design");
        assert_eq!(row.phases[0].agents[0].state, "done");
    }

    #[test]
    fn progress_from_a_different_run_is_ignored() {
        let mut row = WorkflowRow {
            task_id: "w123".into(),
            run_id: Some("wf_current".into()),
            ..WorkflowRow::default()
        };
        let event = WorkflowProgressEvent {
            task_id: "w123".into(),
            run_id: "wf_stale".into(),
            kind: "workflow_phase".into(),
            index: 1,
            title: Some("Stale".into()),
            ..WorkflowProgressEvent::default()
        };
        assert!(!apply_workflow_progress(&mut row, &event));
        assert!(row.phases.is_empty());
    }
}

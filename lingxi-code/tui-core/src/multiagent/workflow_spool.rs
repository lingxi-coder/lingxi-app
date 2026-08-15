//! Parse a workflow run's output spool into a phase/agent tree for the
//! `/workflows` detail view.
//!
//! The workflow runtime writes two structured line shapes to the task spool
//! (see `tasks::handlers::local_workflow::format_progress`):
//!
//! - `[{index}] === {title} ===` — a `phase()` marker.
//! - `[workflow_agent] {json}` — one `agent()` lifecycle event, whose JSON
//!   carries `index`, `label`, `state` (`start`/`done`/`error`/`cached`),
//!   optional `phaseIndex`, and optional `agentId`.
//!
//! Agents emit several lifecycle events (e.g. `start` then `done`); we collapse
//! them to ONE row per agent showing its latest state, grouped under the phase
//! named by `phaseIndex` (defaulting to phase 0 when absent).

use crate::multiagent::state::{WorkflowAgentRow, WorkflowPhase};
use std::collections::HashMap;

/// Parse `spool` into `(distinct_agent_count, phases)`. Phases are returned in
/// first-seen order; each phase's agents are in first-seen order with their
/// latest lifecycle state. Malformed lines are skipped.
#[must_use]
pub fn parse_workflow_spool(spool: &str) -> (usize, Vec<WorkflowPhase>) {
    // Phase index → title (first non-empty title wins).
    let mut titles: HashMap<usize, String> = HashMap::new();
    // Phase index → ordered agents, each with a dedup key.
    let mut agents: HashMap<usize, Vec<(String, WorkflowAgentRow)>> = HashMap::new();
    // First-seen order of phase indices.
    let mut order: Vec<usize> = Vec::new();
    // Distinct agents across the whole run (for the count).
    let mut distinct: HashMap<String, ()> = HashMap::new();

    let see_phase = |index: usize, order: &mut Vec<usize>| {
        if !order.contains(&index) {
            order.push(index);
        }
    };

    for line in spool.lines() {
        // Phase marker: `[{index}] === {title} ===`.
        if let Some(rest) = line.strip_prefix('[') {
            if let Some((idx_str, tail)) = rest.split_once("] === ") {
                if let (Ok(index), Some(title)) =
                    (idx_str.parse::<usize>(), tail.strip_suffix(" ==="))
                {
                    see_phase(index, &mut order);
                    titles.entry(index).or_insert_with(|| title.to_string());
                    continue;
                }
            }
        }
        // Agent lifecycle: `[workflow_agent] {json}`.
        if let Some(json_str) = line.strip_prefix("[workflow_agent] ") {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(json_str) else {
                continue;
            };
            let index = v
                .get("index")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let label = v
                .get("label")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string();
            let state = v
                .get("state")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string();
            let phase_index = usize::try_from(
                v.get("phaseIndex")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0),
            )
            .unwrap_or(0);
            // Key on the workflow-global `index`, which is IDENTICAL across an
            // agent's lifecycle events. `agentId` is NOT usable as the key: the
            // `start` event carries no `agentId` (None) while `done`/`error`
            // carry the spawned id (see `format_progress`), so keying on it would
            // split one agent into two and double the count.
            let key = format!("#{index}");
            distinct.entry(key.clone()).or_insert(());
            see_phase(phase_index, &mut order);
            let bucket = agents.entry(phase_index).or_default();
            if let Some(slot) = bucket.iter_mut().find(|(k, _)| *k == key) {
                // Update to the latest state; keep a non-empty label.
                slot.1.state = state;
                if !label.is_empty() {
                    slot.1.label = label;
                }
            } else {
                bucket.push((
                    key,
                    WorkflowAgentRow {
                        index,
                        label,
                        state,
                    },
                ));
            }
        }
    }

    let phases = order
        .into_iter()
        .map(|index| WorkflowPhase {
            index,
            title: titles.get(&index).cloned().unwrap_or_default(),
            agents: agents
                .remove(&index)
                .unwrap_or_default()
                .into_iter()
                .map(|(_, a)| a)
                .collect(),
        })
        .collect();

    (distinct.len(), phases)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_spool_yields_nothing() {
        let (n, phases) = parse_workflow_spool("");
        assert_eq!(n, 0);
        assert!(phases.is_empty());
    }

    #[test]
    fn phases_and_agents_group_with_latest_state() {
        // Uses the REAL `format_progress` emission shape: the `start` event has
        // NO `agentId`, the `done` event carries the spawned id — both share the
        // stable `index`. The parser must collapse them to one agent (keyed on
        // `index`), not double-count.
        let spool = "\
[1] === Scan ===
[workflow_agent] {\"type\":\"workflow_agent\",\"index\":0,\"label\":\"grep\",\"state\":\"start\",\"phaseIndex\":1}
[workflow_agent] {\"type\":\"workflow_agent\",\"index\":0,\"label\":\"grep\",\"state\":\"done\",\"phaseIndex\":1,\"agentId\":\"a1-uuid\"}
[2] === Fix ===
[workflow_agent] {\"type\":\"workflow_agent\",\"index\":1,\"label\":\"patch\",\"state\":\"start\",\"phaseIndex\":2}
some free-form log line
";
        let (n, phases) = parse_workflow_spool(spool);
        assert_eq!(n, 2, "two distinct agents (start+done of one collapse)");
        assert_eq!(phases.len(), 2);
        assert_eq!(phases[0].title, "Scan");
        assert_eq!(phases[0].agents.len(), 1, "start+done collapsed to one row");
        assert_eq!(phases[0].agents[0].label, "grep");
        assert_eq!(phases[0].agents[0].state, "done", "collapsed to latest");
        assert_eq!(phases[1].title, "Fix");
        assert_eq!(phases[1].agents[0].state, "start");
    }

    #[test]
    fn agents_without_phase_marker_default_to_phase_zero() {
        let spool = "[workflow_agent] {\"index\":0,\"label\":\"x\",\"state\":\"done\"}";
        let (n, phases) = parse_workflow_spool(spool);
        assert_eq!(n, 1);
        assert_eq!(phases.len(), 1);
        assert_eq!(phases[0].index, 0);
        assert_eq!(phases[0].agents[0].label, "x");
    }

    #[test]
    fn malformed_agent_json_is_skipped() {
        let spool = "[workflow_agent] not-json\n[0] === Only ===";
        let (n, phases) = parse_workflow_spool(spool);
        assert_eq!(n, 0);
        assert_eq!(phases.len(), 1);
        assert_eq!(phases[0].title, "Only");
    }
}

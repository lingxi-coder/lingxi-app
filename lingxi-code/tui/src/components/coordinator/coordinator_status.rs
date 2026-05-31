//! `CoordinatorAgentStatus` panel (claude-code `CoordinatorAgentStatus.tsx`):
//! a `main` line + one line per teammate, with a `>`-marked selection. Names
//! are colored by the dialog (this renders the plain text + selection marker).

use crate::multiagent::state::WorkerRow;

/// Render the coordinator panel: a header, the `main` line, and one line per
/// worker. `selected` highlights a row (`0` == the `main` line).
#[must_use]
pub fn render_coordinator_status(workers: &[WorkerRow], selected: usize) -> String {
    let mut out = String::from("Agents\n");
    let marker = |i: usize| if i == selected { "> " } else { "  " };
    out.push_str(marker(0));
    out.push_str("main");
    for (i, w) in workers.iter().enumerate() {
        out.push('\n');
        out.push_str(marker(i + 1));
        out.push_str(&format!("@{}: {}", w.name, w.status));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(name: &str, status: &str) -> WorkerRow {
        WorkerRow {
            agent_id: "a".into(),
            name: name.into(),
            agent_type: "explorer".into(),
            status: status.into(),
        }
    }

    #[test]
    fn main_selected() {
        let out = render_coordinator_status(&[w("alice", "working")], 0);
        assert_eq!(out, "Agents\n> main\n  @alice: working");
    }

    #[test]
    fn worker_selected() {
        let out = render_coordinator_status(&[w("alice", "working"), w("bob", "idle")], 2);
        assert_eq!(out, "Agents\n  main\n  @alice: working\n> @bob: idle");
    }
}

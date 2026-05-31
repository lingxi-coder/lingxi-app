//! M9-06 — coordinator chrome snapshots + a fixture WorkersRefreshed check.

use tui::components::coordinator::agent_progress::{
    render_agent_progress_line, AgentProgressState,
};
use tui::components::coordinator::coordinator_status::render_coordinator_status;
use tui::components::coordinator::team_status::render_team_footer;
use tui::components::coordinator::teammate_view_header::render_teammate_view_header;
use tui::multiagent::state::WorkerRow;

fn workers() -> Vec<WorkerRow> {
    vec![
        WorkerRow {
            agent_id: "a1".into(),
            name: "alice".into(),
            agent_type: "explorer".into(),
            status: "working".into(),
        },
        WorkerRow {
            agent_id: "a2".into(),
            name: "bob".into(),
            agent_type: "writer".into(),
            status: "idle".into(),
        },
    ]
}

#[test]
fn coordinator_panel() {
    insta::assert_snapshot!("coord_panel", render_coordinator_status(&workers(), 1));
}

#[test]
fn team_footer() {
    insta::assert_snapshot!(
        "coord_team_footer",
        render_team_footer(&workers(), true).unwrap()
    );
}

#[test]
fn teammate_header() {
    insta::assert_snapshot!(
        "coord_teammate_header",
        render_teammate_view_header("alice", "Explore the repo")
    );
}

#[test]
fn agent_progress_tree() {
    let a = render_agent_progress_line(
        "alice",
        false,
        2,
        5100,
        &AgentProgressState::Running("Reading".into()),
    );
    let b = render_agent_progress_line("bob", true, 1, 512, &AgentProgressState::Done);
    insta::assert_snapshot!("coord_agent_progress", format!("{a}\n{b}"));
}

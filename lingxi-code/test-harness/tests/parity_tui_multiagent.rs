//! Parity (M9-09): the multi-agent TUI surface's high-value strings asserted
//! through the LIVE pure renderers (no parallel path). Structure, not
//! per-token color (§0 Q3).

use serde_json::Value;
use tui::components::coordinator::agent_progress::{
    render_agent_progress_line, AgentProgressState,
};
use tui::components::coordinator::team_status::render_team_footer;
use tui::components::coordinator::teammate_view_header::render_teammate_view_header;
use tui::components::permissions::worker::{render_worker_badge, render_worker_pending_to_string};
use tui::components::tasks::render_task_row;
use tui::components::tasks::status_footer::render_task_footer;
use tui::multiagent::state::{TaskRow, WorkerRow};
use tui::screens::agents::{
    render_agents_to_string, AgentRow, AgentsDialogMode, AgentsScreenState,
};

const FIXTURE: &str = include_str!("../src/parity/fixtures/parity_tui_multiagent.json");

fn load() -> Value {
    serde_json::from_str(FIXTURE).expect("parity_tui_multiagent.json parses")
}

fn task_row(t: &str, s: &str, d: &str) -> TaskRow {
    TaskRow {
        task_id: "b1".into(),
        task_type: t.into(),
        status: s.into(),
        description: d.into(),
        command: None,
    }
}

#[test]
fn task_footer_strings() {
    let f = load();
    let sc = f["scenarios"][0].clone();
    let tasks: Vec<TaskRow> = sc["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            task_row(
                t["task_type"].as_str().unwrap(),
                t["status"].as_str().unwrap(),
                t["description"].as_str().unwrap(),
            )
        })
        .collect();
    let footer = render_task_footer(&tasks).expect("footer present");
    for needle in sc["expected_contains"].as_array().unwrap() {
        assert!(
            footer.contains(needle.as_str().unwrap()),
            "footer `{footer}` missing `{needle}`"
        );
    }
}

#[test]
fn task_rows_strings() {
    let f = load();
    for row in f["scenarios"][1]["rows"].as_array().unwrap() {
        let r = task_row(
            row["task_type"].as_str().unwrap(),
            row["status"].as_str().unwrap(),
            row["description"].as_str().unwrap(),
        );
        let out = render_task_row(&r);
        let expected = row["expected"].as_str().unwrap();
        assert!(out.contains(expected), "row `{out}` missing `{expected}`");
    }
}

#[test]
fn coordinator_chrome_strings() {
    let f = load();
    let sc = f["scenarios"][2].clone();
    let workers = vec![WorkerRow {
        agent_id: "a".into(),
        name: "alice".into(),
        agent_type: "explorer".into(),
        status: "working".into(),
    }];
    assert!(render_team_footer(&workers, true)
        .unwrap()
        .contains(sc["team_footer_expected"].as_str().unwrap()));
    assert!(render_teammate_view_header("alice", "task")
        .contains(sc["teammate_header_expected"].as_str().unwrap()));
    let prog = render_agent_progress_line("alice", true, 2, 100, &AgentProgressState::Done);
    for needle in sc["agent_progress_expected"].as_array().unwrap() {
        assert!(
            prog.contains(needle.as_str().unwrap()),
            "progress `{prog}` missing `{needle}`"
        );
    }
}

#[test]
fn worker_permission_strings() {
    let f = load();
    let sc = f["scenarios"][3].clone();
    assert!(render_worker_badge("worker").contains(sc["badge_expected"].as_str().unwrap()));
    let pending = render_worker_pending_to_string("Bash", "run ls", Some("worker"), Some("team"));
    for needle in sc["pending_expected"].as_array().unwrap() {
        assert!(
            pending.contains(needle.as_str().unwrap()),
            "pending `{pending}` missing `{needle}`"
        );
    }
}

#[test]
fn agents_screen_strings() {
    let f = load();
    let sc = f["scenarios"][4].clone();
    let rows: Vec<AgentRow> = sc["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| AgentRow {
            name: a["name"].as_str().unwrap().into(),
            description: a["description"].as_str().unwrap().into(),
            tools: vec![],
            ..AgentRow::default()
        })
        .collect();
    let list = render_agents_to_string(&AgentsScreenState {
        rows: rows.clone(),
        selected: 0,
        mode: AgentsDialogMode::List,
    });
    for needle in sc["list_expected"].as_array().unwrap() {
        assert!(
            list.contains(needle.as_str().unwrap()),
            "list `{list}` missing `{needle}`"
        );
    }
    let detail = render_agents_to_string(&AgentsScreenState {
        rows,
        selected: 0,
        mode: AgentsDialogMode::Detail,
    });
    for needle in sc["detail_expected"].as_array().unwrap() {
        assert!(
            detail.contains(needle.as_str().unwrap()),
            "detail `{detail}` missing `{needle}`"
        );
    }
}

//! M9-04 — one snapshot per background-task type (live wire-shape rows) plus a
//! couple of rich-field variants proving the renderers handle full data.

use tui::components::tasks::render_task_row;
use tui::components::tasks::rows::{render_local_workflow_row, render_remote_agent_row};
use tui::multiagent::state::TaskRow;

fn row(task_type: &str, status: &str, description: &str) -> TaskRow {
    TaskRow {
        task_id: "b12345678".into(),
        task_type: task_type.into(),
        status: status.into(),
        description: description.into(),
        command: None,
    }
}

#[test]
fn task_row_local_bash() {
    insta::assert_snapshot!(
        "task_row_local_bash",
        render_task_row(&row("local_bash", "running", "cargo build"), 200)
    );
}

#[test]
fn task_row_local_agent() {
    insta::assert_snapshot!(
        "task_row_local_agent",
        render_task_row(&row("local_agent", "completed", "review"), 200)
    );
}

#[test]
fn task_row_remote_agent() {
    insta::assert_snapshot!(
        "task_row_remote_agent",
        render_task_row(&row("remote_agent", "running", "deploy"), 200)
    );
}

#[test]
fn task_row_in_process_teammate() {
    insta::assert_snapshot!(
        "task_row_in_process_teammate",
        render_task_row(&row("in_process_teammate", "running", "alice"), 200)
    );
}

#[test]
fn task_row_local_workflow() {
    insta::assert_snapshot!(
        "task_row_local_workflow",
        render_task_row(&row("local_workflow", "running", "pipeline"), 200)
    );
}

#[test]
fn task_row_monitor_mcp() {
    insta::assert_snapshot!(
        "task_row_monitor_mcp",
        render_task_row(&row("monitor_mcp", "running", "watch fs"), 200)
    );
}

#[test]
fn task_row_dream() {
    insta::assert_snapshot!(
        "task_row_dream",
        render_task_row(&row("dream", "running", "nightly"), 200)
    );
}

#[test]
fn task_row_rich_variants() {
    insta::assert_snapshot!(
        "task_row_remote_with_counts",
        render_remote_agent_row("deploy", "running", Some((3, 7)))
    );
    insta::assert_snapshot!(
        "task_row_workflow_with_agents",
        render_local_workflow_row("pipeline", "running", Some(4), false)
    );
}

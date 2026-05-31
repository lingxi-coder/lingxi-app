//! M9-05 — BackgroundTasksDialog behavior + snapshots.

use tui::components::tasks::output_tail::OutputTailState;
use tui::multiagent::state::TaskRow;
use tui::screens::background_tasks::{
    handle_background_tasks_key, render_background_tasks_to_string, BackgroundTasksState,
    TaskDialogMode,
};

fn rows() -> Vec<TaskRow> {
    vec![
        TaskRow {
            task_id: "b1".into(),
            task_type: "local_bash".into(),
            status: "running".into(),
            description: "cargo build".into(),
        },
        TaskRow {
            task_id: "b2".into(),
            task_type: "local_agent".into(),
            status: "completed".into(),
            description: "review".into(),
        },
    ]
}

#[test]
fn list_mode_snapshot() {
    let state = BackgroundTasksState::default();
    insta::assert_snapshot!(
        "bg_tasks_list",
        render_background_tasks_to_string(&state, &rows())
    );
}

#[test]
fn empty_list_snapshot() {
    let state = BackgroundTasksState::default();
    insta::assert_snapshot!(
        "bg_tasks_empty",
        render_background_tasks_to_string(&state, &[])
    );
}

#[test]
fn detail_mode_snapshot() {
    let state = BackgroundTasksState {
        selected: 0,
        mode: TaskDialogMode::Detail,
        detail_task_id: Some("b1".into()),
        tail: OutputTailState {
            content: "compiling...\ndone".into(),
            offset: 17,
            total_lines: 2,
            truncated: false,
        },
    };
    insta::assert_snapshot!(
        "bg_tasks_detail",
        render_background_tasks_to_string(&state, &rows())
    );
}

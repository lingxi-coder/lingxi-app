//! `BackgroundTasksDialog` screen (claude-code `BackgroundTasksDialog.tsx`):
//! a list of background tasks with a per-task detail view. Pure reducer over a
//! selected index + a mode, following the `resume.rs` pattern. The task list
//! lives in `AppState.multiagent.tasks` and is passed to the reducer/render.

use crate::components::tasks::output_tail::OutputTailState;

/// List vs. detail.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum TaskDialogMode {
    /// Browsing the task list.
    #[default]
    List,
    /// Viewing one task's detail (output tail).
    Detail,
}

/// Dialog state (selection + mode + the open task's tail buffer).
///
/// (M9-05 Task 5) `PartialEq, Eq` is REQUIRED so the carrying
/// `Screen::BackgroundTasks(..)` variant can keep `Screen: PartialEq`. Every
/// field supports it: `usize`, `TaskDialogMode` (`PartialEq, Eq`),
/// `Option<String>`, and `OutputTailState` (`PartialEq, Eq`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BackgroundTasksState {
    /// Selected row index (clamped to the live task count).
    pub selected: usize,
    /// List or detail.
    pub mode: TaskDialogMode,
    /// Task id whose detail is open (when `mode == Detail`).
    pub detail_task_id: Option<String>,
    /// Accumulated output for the open task (driven by the pump's tail).
    pub tail: OutputTailState,
}

/// What the controller should do after a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskDialogOutcome {
    /// Stay open (selection/mode changed or inert).
    Stay,
    /// Close the dialog.
    Close,
    /// Entered detail for this task id — controller should begin tailing it.
    OpenedDetail(String),
}

/// Reduce a key against the dialog. `task_ids` is the live ordered list of task
/// ids from `AppState.multiagent.tasks`.
#[must_use]
pub fn handle_background_tasks_key(
    state: &mut BackgroundTasksState,
    task_ids: &[String],
    key: crossterm::event::KeyCode,
) -> TaskDialogOutcome {
    use crossterm::event::KeyCode;
    match state.mode {
        TaskDialogMode::List => match key {
            KeyCode::Up | KeyCode::Char('k') => {
                state.selected = state.selected.saturating_sub(1);
                TaskDialogOutcome::Stay
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if !task_ids.is_empty() {
                    state.selected = (state.selected + 1).min(task_ids.len() - 1);
                }
                TaskDialogOutcome::Stay
            }
            KeyCode::Enter => match task_ids.get(state.selected) {
                Some(id) => {
                    state.mode = TaskDialogMode::Detail;
                    state.detail_task_id = Some(id.clone());
                    state.tail = OutputTailState::default();
                    TaskDialogOutcome::OpenedDetail(id.clone())
                }
                None => TaskDialogOutcome::Stay,
            },
            KeyCode::Esc | KeyCode::Char('q') => TaskDialogOutcome::Close,
            _ => TaskDialogOutcome::Stay,
        },
        TaskDialogMode::Detail => match key {
            KeyCode::Esc | KeyCode::Left | KeyCode::Char('q') => {
                state.mode = TaskDialogMode::List;
                state.detail_task_id = None;
                TaskDialogOutcome::Stay
            }
            _ => TaskDialogOutcome::Stay,
        },
    }
}

use crate::components::tasks::detail::render_task_detail;
use crate::components::tasks::render_task_row;
use crate::multiagent::state::TaskRow;

/// Render the dialog body to a string. List mode: a `>`-marked selectable list
/// (claude-code `BackgroundTasksDialog.tsx`); detail mode: the open task's
/// detail. Header line + a trailing key-hint line.
#[must_use]
pub fn render_background_tasks_to_string(
    state: &BackgroundTasksState,
    tasks: &[TaskRow],
) -> String {
    match state.mode {
        TaskDialogMode::List => {
            let mut out = String::from("Background tasks\n");
            if tasks.is_empty() {
                out.push_str("(no background tasks)");
                return out;
            }
            for (i, row) in tasks.iter().enumerate() {
                let marker = if i == state.selected { "> " } else { "  " };
                out.push_str(marker);
                out.push_str(&render_task_row(row));
                out.push('\n');
            }
            out.push_str("\u{2191}\u{2193} move \u{00B7} enter open \u{00B7} esc close");
            out
        }
        TaskDialogMode::Detail => {
            let row = state
                .detail_task_id
                .as_ref()
                .and_then(|id| tasks.iter().find(|t| &t.task_id == id));
            match row {
                Some(r) => format!(
                    "{}\n\u{2190} back \u{00B7} esc close",
                    render_task_detail(r, &state.tail)
                ),
                None => "Background tasks\n(task no longer available)".to_string(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyCode;

    fn ids() -> Vec<String> {
        vec!["b1".into(), "b2".into(), "b3".into()]
    }

    #[test]
    fn down_up_clamp() {
        let mut s = BackgroundTasksState::default();
        let t = ids();
        assert_eq!(
            handle_background_tasks_key(&mut s, &t, KeyCode::Down),
            TaskDialogOutcome::Stay
        );
        assert_eq!(s.selected, 1);
        let _ = handle_background_tasks_key(&mut s, &t, KeyCode::Down);
        let _ = handle_background_tasks_key(&mut s, &t, KeyCode::Down); // clamps at 2
        assert_eq!(s.selected, 2);
        let _ = handle_background_tasks_key(&mut s, &t, KeyCode::Up);
        assert_eq!(s.selected, 1);
    }

    #[test]
    fn enter_opens_detail_then_esc_returns() {
        let mut s = BackgroundTasksState::default();
        let t = ids();
        s.selected = 1;
        assert_eq!(
            handle_background_tasks_key(&mut s, &t, KeyCode::Enter),
            TaskDialogOutcome::OpenedDetail("b2".into())
        );
        assert_eq!(s.mode, TaskDialogMode::Detail);
        assert_eq!(s.detail_task_id.as_deref(), Some("b2"));
        // Esc in detail → back to list (NOT close).
        assert_eq!(
            handle_background_tasks_key(&mut s, &t, KeyCode::Esc),
            TaskDialogOutcome::Stay
        );
        assert_eq!(s.mode, TaskDialogMode::List);
        assert_eq!(s.detail_task_id, None);
    }

    #[test]
    fn esc_in_list_closes() {
        let mut s = BackgroundTasksState::default();
        assert_eq!(
            handle_background_tasks_key(&mut s, &ids(), KeyCode::Esc),
            TaskDialogOutcome::Close
        );
    }
}

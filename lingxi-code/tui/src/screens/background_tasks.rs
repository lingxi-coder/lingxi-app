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
#[derive(Debug, Clone, Default)]
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
        handle_background_tasks_key(&mut s, &t, KeyCode::Down);
        handle_background_tasks_key(&mut s, &t, KeyCode::Down); // clamps at 2
        assert_eq!(s.selected, 2);
        handle_background_tasks_key(&mut s, &t, KeyCode::Up);
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

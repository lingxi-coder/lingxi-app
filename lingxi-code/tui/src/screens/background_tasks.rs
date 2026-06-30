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
    /// (BGTASK-3) The user pressed `x` on a running task — controller should
    /// call the registry's kill for this task id.
    Stop(String),
}

use crate::multiagent::state::TaskRow;

/// Reduce a key against the dialog. `tasks` is the live display-ordered list
/// from `AppState.multiagent.tasks` (via [`display_order`]) — selection
/// indexes into it the same way the renderer does.
#[must_use]
pub fn handle_background_tasks_key(
    state: &mut BackgroundTasksState,
    tasks: &[TaskRow],
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
                if !tasks.is_empty() {
                    state.selected = (state.selected + 1).min(tasks.len() - 1);
                }
                TaskDialogOutcome::Stay
            }
            KeyCode::Enter => match tasks.get(state.selected) {
                Some(t) => {
                    state.mode = TaskDialogMode::Detail;
                    state.detail_task_id = Some(t.task_id.clone());
                    state.tail = OutputTailState::default();
                    TaskDialogOutcome::OpenedDetail(t.task_id.clone())
                }
                None => TaskDialogOutcome::Stay,
            },
            // (BGTASK-3) `x` stops the selected task — only when it's running
            // (claude-code gates the hint/key on `status === 'running'`).
            KeyCode::Char('x') => match tasks.get(state.selected) {
                Some(t) if t.status == "running" => TaskDialogOutcome::Stop(t.task_id.clone()),
                _ => TaskDialogOutcome::Stay,
            },
            // claude-code list hint is `←/Esc close` — both keys close the
            // dialog from the top-level list (TASKS-DIALOG-KEYHINTS).
            KeyCode::Esc | KeyCode::Left | KeyCode::Char('q') => TaskDialogOutcome::Close,
            _ => TaskDialogOutcome::Stay,
        },
        TaskDialogMode::Detail => match key {
            KeyCode::Esc | KeyCode::Left | KeyCode::Char('q') => {
                state.mode = TaskDialogMode::List;
                state.detail_task_id = None;
                TaskDialogOutcome::Stay
            }
            // (BGTASK-3) `x` also stops from the detail view.
            KeyCode::Char('x') => match state
                .detail_task_id
                .as_ref()
                .and_then(|id| tasks.iter().find(|t| &t.task_id == id))
            {
                Some(t) if t.status == "running" => TaskDialogOutcome::Stop(t.task_id.clone()),
                _ => TaskDialogOutcome::Stay,
            },
            _ => TaskDialogOutcome::Stay,
        },
    }
}

use crate::components::tasks::detail::render_task_detail;
use crate::components::tasks::render_task_row;

/// Canonical section order (claude-code `BackgroundTasksDialog.tsx`): the
/// teammate "Agents" group, then Shells/Monitors/Remote agents/Local
/// agents/Workflows, each as its own headed section. `dream` (and any
/// unrecognized type) renders last with no header, matching the TS source
/// (`dreamTasks_0`'s box has no `<Text dimColor>` header).
const SECTION_TYPES: [&str; 6] = [
    "in_process_teammate",
    "local_bash",
    "monitor_mcp",
    "remote_agent",
    "local_agent",
    "local_workflow",
];

/// (TASKS-DIALOG-SORT-ORDER) The list's actual display order: grouped into
/// `SECTION_TYPES` sections (then any other type last, e.g. `dream`),
/// running/pending tasks first within each group. claude-code additionally
/// sorts by start time descending, which needs a wire field `TaskRow`
/// doesn't carry yet — this is the running-first partition the fix
/// explicitly allows as the minimum viable port.
#[must_use]
pub fn display_order(tasks: &[TaskRow]) -> Vec<&TaskRow> {
    let running_first = |group: &mut Vec<&TaskRow>| {
        group.sort_by_key(|t| !matches!(t.status.as_str(), "running" | "pending"));
    };
    let mut out: Vec<&TaskRow> = Vec::with_capacity(tasks.len());
    for ty in SECTION_TYPES {
        let mut group: Vec<&TaskRow> = tasks.iter().filter(|t| t.task_type == ty).collect();
        running_first(&mut group);
        out.extend(group);
    }
    let mut rest: Vec<&TaskRow> = tasks
        .iter()
        .filter(|t| !SECTION_TYPES.contains(&t.task_type.as_str()))
        .collect();
    running_first(&mut rest);
    out.extend(rest);
    out
}

/// (TASKS-DIALOG-FLAT-LIST-NO-SECTIONS) The running-count subtitle line:
/// `N agent[s]` (teammates) · `N active shell[s]` · `N active agent[s]`
/// (remote pending/running + local_agent running), each clause omitted when
/// its count is 0, joined by ` · `. `""` when every clause is empty.
fn running_count_subtitle(tasks: &[TaskRow]) -> String {
    let teammate_running = tasks
        .iter()
        .filter(|t| t.task_type == "in_process_teammate" && t.status == "running")
        .count();
    let bash_running = tasks
        .iter()
        .filter(|t| t.task_type == "local_bash" && t.status == "running")
        .count();
    let agent_running = tasks
        .iter()
        .filter(|t| {
            (t.task_type == "remote_agent" && matches!(t.status.as_str(), "running" | "pending"))
                || (t.task_type == "local_agent" && t.status == "running")
        })
        .count();
    let mut parts = Vec::new();
    if teammate_running > 0 {
        let noun = if teammate_running == 1 {
            "agent"
        } else {
            "agents"
        };
        parts.push(format!("{teammate_running} {noun}"));
    }
    if bash_running > 0 {
        let noun = if bash_running == 1 {
            "active shell"
        } else {
            "active shells"
        };
        parts.push(format!("{bash_running} {noun}"));
    }
    if agent_running > 0 {
        let noun = if agent_running == 1 {
            "active agent"
        } else {
            "active agents"
        };
        parts.push(format!("{agent_running} {noun}"));
    }
    parts.join(" \u{00B7} ")
}

/// Render the dialog body to a string. List mode: a section-grouped
/// selectable list (claude-code `BackgroundTasksDialog.tsx`); detail mode:
/// the open task's detail. Header line + a trailing key-hint line.
#[must_use]
pub fn render_background_tasks_to_string(
    state: &BackgroundTasksState,
    tasks: &[TaskRow],
    max_activity_width: usize,
) -> String {
    match state.mode {
        TaskDialogMode::List => {
            let mut out = String::from("Background tasks\n");
            if tasks.is_empty() {
                // (TASKS-DIALOG-EMPTY-TEXT) claude-code dimmed empty state.
                out.push_str("No tasks currently running");
                return out;
            }
            let subtitle = running_count_subtitle(tasks);
            if !subtitle.is_empty() {
                out.push_str(&subtitle);
                out.push('\n');
            }
            let teammate_n = tasks
                .iter()
                .filter(|t| t.task_type == "in_process_teammate")
                .count();
            let shells_n = tasks.iter().filter(|t| t.task_type == "local_bash").count();
            let monitors_n = tasks
                .iter()
                .filter(|t| t.task_type == "monitor_mcp")
                .count();
            let remote_n = tasks
                .iter()
                .filter(|t| t.task_type == "remote_agent")
                .count();
            let local_agent_n = tasks
                .iter()
                .filter(|t| t.task_type == "local_agent")
                .count();
            let workflow_n = tasks
                .iter()
                .filter(|t| t.task_type == "local_workflow")
                .count();

            let ordered = display_order(tasks);
            let mut last_type: Option<&str> = None;
            let mut any_section_emitted = false;
            for (i, row) in ordered.iter().enumerate() {
                let ty = row.task_type.as_str();
                if last_type != Some(ty) {
                    // (TASKS-DIALOG-FLAT-LIST-NO-SECTIONS) Bold-dim section
                    // header at each section boundary; a blank line before
                    // every section except the first one shown. Only Shells'
                    // header is gated on other groups being present — every
                    // other header is unconditional, and `dream` never gets
                    // one — this asymmetry is literal in the TS source.
                    let header = match ty {
                        "in_process_teammate" => Some(format!("  Agents ({teammate_n})")),
                        "local_bash" if teammate_n > 0 || remote_n > 0 || local_agent_n > 0 => {
                            Some(format!("  Shells ({shells_n})"))
                        }
                        "local_bash" => None,
                        "monitor_mcp" => Some(format!("  Monitors ({monitors_n})")),
                        "remote_agent" => Some(format!("  Remote agents ({remote_n})")),
                        "local_agent" => Some(format!("  Local agents ({local_agent_n})")),
                        "local_workflow" => Some(format!("  Workflows ({workflow_n})")),
                        _ => None,
                    };
                    if any_section_emitted {
                        out.push('\n');
                    }
                    if let Some(h) = header {
                        out.push_str(&h);
                        out.push('\n');
                    }
                    any_section_emitted = true;
                    last_type = Some(ty);
                }
                // (TASKS-DIALOG-SELECTION-MARKER) figures.pointer `❯ ` on the
                // selected row, matching the other LingXi list screens.
                let marker = if i == state.selected {
                    "\u{276F} "
                } else {
                    "  "
                };
                out.push_str(marker);
                out.push_str(&render_task_row(row, max_activity_width));
                out.push('\n');
            }
            // (TASKS-DIALOG-KEYHINTS/BGTASK-3) `↑/↓ select · Enter view ·
            // [x stop] · ←/Esc close` — `x stop` only when the selected row
            // is a running task (claude-code's killable-gate).
            let running = ordered
                .get(state.selected)
                .is_some_and(|t| t.status == "running");
            out.push_str("\u{2191}/\u{2193} select \u{00B7} Enter view \u{00B7} ");
            if running {
                out.push_str("x stop \u{00B7} ");
            }
            out.push_str("\u{2190}/Esc close");
            out
        }
        TaskDialogMode::Detail => {
            let row = state
                .detail_task_id
                .as_ref()
                .and_then(|id| tasks.iter().find(|t| &t.task_id == id));
            match row {
                Some(r) => {
                    let stop_hint = if r.status == "running" {
                        "x stop \u{00B7} "
                    } else {
                        ""
                    };
                    format!(
                        "{}\n{stop_hint}\u{2190} back \u{00B7} esc close",
                        render_task_detail(r, &state.tail)
                    )
                }
                None => "Background tasks\n(task no longer available)".to_string(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyCode;

    fn ids() -> Vec<TaskRow> {
        vec![
            task("b1", "local_bash", "running"),
            task("b2", "local_bash", "completed"),
            task("b3", "local_bash", "completed"),
        ]
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

    fn task(id: &str, ty: &str, status: &str) -> TaskRow {
        TaskRow {
            task_id: id.into(),
            task_type: ty.into(),
            status: status.into(),
            description: format!("{id} desc"),
            command: None,
        }
    }

    #[test]
    fn display_order_groups_by_section_and_running_first() {
        // (TASKS-DIALOG-SORT-ORDER) Feed order is scrambled across types;
        // display_order groups into the canonical section order and puts
        // running/pending tasks first within each group.
        let tasks = vec![
            task("a1", "local_agent", "completed"),
            task("w1", "local_workflow", "running"),
            task("b1", "local_bash", "completed"),
            task("b2", "local_bash", "running"),
            task("a2", "local_agent", "running"),
            task("d1", "dream", "running"),
        ];
        let order: Vec<&str> = display_order(&tasks)
            .into_iter()
            .map(|t| t.task_id.as_str())
            .collect();
        // Shells (b2 running, then b1 completed), Local agents (a2 running,
        // then a1 completed), Workflows (w1), dream last.
        assert_eq!(order, vec!["b2", "b1", "a2", "a1", "w1", "d1"]);
    }

    #[test]
    fn list_render_groups_sections_with_headers_and_subtitle() {
        // (TASKS-DIALOG-FLAT-LIST-NO-SECTIONS)
        let tasks = vec![
            task("b1", "local_bash", "running"),
            task("a1", "local_agent", "completed"),
        ];
        let s = BackgroundTasksState::default();
        let out = render_background_tasks_to_string(&s, &tasks, 200);
        assert!(out.contains("1 active shell"), "{out}");
        assert!(out.contains("  Shells (1)"), "{out}");
        assert!(out.contains("  Local agents (1)"), "{out}");
    }

    #[test]
    fn list_render_hides_shells_header_when_shells_is_the_only_group() {
        // (TASKS-DIALOG-FLAT-LIST-NO-SECTIONS) Literal claude-code quirk:
        // Shells' header is gated on teammates/remote/local-agent being
        // present; with only shells in the list, no header is shown.
        let tasks = vec![
            task("b1", "local_bash", "running"),
            task("b2", "local_bash", "completed"),
        ];
        let s = BackgroundTasksState::default();
        let out = render_background_tasks_to_string(&s, &tasks, 200);
        assert!(!out.contains("Shells"), "{out}");
    }

    #[test]
    fn list_render_no_subtitle_when_nothing_running() {
        let tasks = vec![task("b1", "local_bash", "completed")];
        let s = BackgroundTasksState::default();
        let out = render_background_tasks_to_string(&s, &tasks, 200);
        assert!(!out.contains("active"), "{out}");
    }

    #[test]
    fn x_stops_the_selected_running_task() {
        // (BGTASK-3)
        let tasks = vec![task("b1", "local_bash", "running")];
        let mut s = BackgroundTasksState::default();
        assert_eq!(
            handle_background_tasks_key(&mut s, &tasks, KeyCode::Char('x')),
            TaskDialogOutcome::Stop("b1".into())
        );
    }

    #[test]
    fn x_is_a_no_op_on_a_completed_task() {
        // (BGTASK-3) claude-code gates the kill action on status=='running'.
        let tasks = vec![task("b1", "local_bash", "completed")];
        let mut s = BackgroundTasksState::default();
        assert_eq!(
            handle_background_tasks_key(&mut s, &tasks, KeyCode::Char('x')),
            TaskDialogOutcome::Stay
        );
    }

    #[test]
    fn x_stops_from_detail_view_too() {
        let tasks = vec![task("b1", "local_bash", "running")];
        let mut s = BackgroundTasksState {
            mode: TaskDialogMode::Detail,
            detail_task_id: Some("b1".into()),
            ..Default::default()
        };
        assert_eq!(
            handle_background_tasks_key(&mut s, &tasks, KeyCode::Char('x')),
            TaskDialogOutcome::Stop("b1".into())
        );
    }

    #[test]
    fn list_footer_shows_x_stop_only_for_a_running_selection() {
        // (BGTASK-3/TASKS-DIALOG-KEYHINTS)
        let tasks = vec![
            task("b1", "local_bash", "running"),
            task("b2", "local_agent", "completed"),
        ];
        let running_selected = BackgroundTasksState {
            selected: 0,
            ..Default::default()
        };
        let out = render_background_tasks_to_string(&running_selected, &tasks, 200);
        assert!(out.contains("x stop"), "{out}");

        let completed_selected = BackgroundTasksState {
            selected: 1,
            ..Default::default()
        };
        let out2 = render_background_tasks_to_string(&completed_selected, &tasks, 200);
        assert!(!out2.contains("x stop"), "{out2}");
    }

    #[test]
    fn detail_footer_shows_x_stop_for_a_running_task() {
        let tasks = vec![task("b1", "local_bash", "running")];
        let s = BackgroundTasksState {
            mode: TaskDialogMode::Detail,
            detail_task_id: Some("b1".into()),
            ..Default::default()
        };
        let out = render_background_tasks_to_string(&s, &tasks, 200);
        assert!(out.contains("x stop"), "{out}");
    }
}

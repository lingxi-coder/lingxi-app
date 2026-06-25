//! Background-task rendering (M9-04): per-type row renderers, shell progress,
//! status text, duration/exit formatters, and the live output tail. Pure
//! string renderers — the iocraft components + dialog that display them are
//! M9-05.

pub mod detail;
pub mod format;
pub mod output_tail;
pub mod rows;
pub mod shell_progress;
pub mod status_footer;
pub mod status_text;

use crate::multiagent::state::TaskRow;
use crate::render::truncate_to_width_ellipsis;

/// Render a task list row from the wire-available `TaskRow` (claude-code
/// `BackgroundTask.tsx`). `task_type`/`status`/`description` and (for
/// `local_bash`) `command` are on the wire; rich per-type fields (elapsed,
/// counts, `, unread`, activity, dream phase) are passed as defaults here
/// (spec §2.4 — omit, never invent); the renderers accept them so
/// fixtures/a richer feed light them up unchanged.
///
/// (BASH-ROW-NO-TRUNCATION) The label/command (NOT the status suffix) is
/// truncated to `max_width` with a trailing ellipsis first — claude-code
/// applies `truncate(label, activityLimit, true)` per-type before appending
/// status (`BackgroundTask.tsx`'s several per-type branches all do this).
#[must_use]
pub fn render_task_row(row: &TaskRow, max_width: usize) -> String {
    let d = &truncate_to_width_ellipsis(&row.description, max_width);
    let s = row.status.as_str();
    match row.task_type.as_str() {
        // (BASH-ROW-USES-DESCRIPTION-NOT-COMMAND) claude-code's local_bash
        // row shows the shell command, not the description.
        "local_bash" => {
            let cmd = row.command.as_deref().unwrap_or(d);
            shell_progress::render_shell_progress_to_string(
                &truncate_to_width_ellipsis(cmd, max_width),
                s,
                None,
            )
        }
        "local_agent" => rows::render_local_agent_row(d, s, false),
        "remote_agent" => rows::render_remote_agent_row(d, s, None),
        "in_process_teammate" => rows::render_in_process_teammate_row(d, s, None),
        "local_workflow" => rows::render_local_workflow_row(d, s, None, false),
        "monitor_mcp" => rows::render_monitor_mcp_row(d, s, false),
        "dream" => rows::render_dream_row(d, s, None, None),
        _ => format!("{d} {}", status_text::render_task_status_text(s, None)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_bash_row_prefers_command_over_description() {
        // (BASH-ROW-USES-DESCRIPTION-NOT-COMMAND)
        let row = TaskRow {
            task_id: "b1".into(),
            task_type: "local_bash".into(),
            status: "running".into(),
            description: "Running shell command".into(),
            command: Some("cargo build".into()),
        };
        assert!(render_task_row(&row, 200).starts_with("cargo build"));
    }

    #[test]
    fn local_bash_row_falls_back_to_description_without_command() {
        let row = TaskRow {
            task_id: "b1".into(),
            task_type: "local_bash".into(),
            status: "running".into(),
            description: "Running shell command".into(),
            command: None,
        };
        assert!(render_task_row(&row, 200).starts_with("Running shell command"));
    }

    #[test]
    fn label_is_truncated_with_ellipsis_to_max_width() {
        // (BASH-ROW-NO-TRUNCATION)
        let row = TaskRow {
            task_id: "b1".into(),
            task_type: "local_agent".into(),
            status: "running".into(),
            description: "a very long task description that overflows the column".into(),
            command: None,
        };
        let out = render_task_row(&row, 20);
        assert!(out.starts_with("a very long task de\u{2026}"), "got: {out}");
    }

    #[test]
    fn local_bash_command_is_truncated_with_ellipsis_too() {
        // (BASH-ROW-NO-TRUNCATION) the command, not just the description.
        let row = TaskRow {
            task_id: "b1".into(),
            task_type: "local_bash".into(),
            status: "running".into(),
            description: "short".into(),
            command: Some("a very long shell command that overflows the column width".into()),
        };
        let out = render_task_row(&row, 20);
        assert!(out.starts_with("a very long shell c\u{2026}"), "got: {out}");
    }
}

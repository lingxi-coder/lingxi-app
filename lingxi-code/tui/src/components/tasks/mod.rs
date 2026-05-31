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

/// Render a task list row from the wire-available `TaskRow` (claude-code
/// `BackgroundTask.tsx`). Only `task_type`, `status`, `description` are on the
/// wire, so rich per-type fields (elapsed, counts, `, unread`, activity, dream
/// phase) are passed as defaults here (spec §2.4 — omit, never invent); the
/// renderers accept them so fixtures/a richer feed light them up unchanged.
#[must_use]
pub fn render_task_row(row: &TaskRow) -> String {
    let d = row.description.as_str();
    let s = row.status.as_str();
    match row.task_type.as_str() {
        "local_bash" => shell_progress::render_shell_progress_to_string(d, s, None),
        "local_agent" => rows::render_local_agent_row(d, s, false),
        "remote_agent" => rows::render_remote_agent_row(d, s, None),
        "in_process_teammate" => rows::render_in_process_teammate_row(d, s, None),
        "local_workflow" => rows::render_local_workflow_row(d, s, None, false),
        "monitor_mcp" => rows::render_monitor_mcp_row(d, s, false),
        "dream" => rows::render_dream_row(d, s, None, None),
        _ => format!("{d} {}", status_text::render_task_status_text(s, None)),
    }
}

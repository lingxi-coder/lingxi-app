//! Task detail body (claude-code `*DetailDialog.tsx`): a per-type header plus
//! the live output tail. Remote/dream are placeholders (excluded distributed
//! surfaces, design §1 non-goals).

use crate::components::tasks::output_tail::{render_output_tail, OutputTailState};
use crate::multiagent::state::TaskRow;

/// Lines of output shown in the detail tail.
const DETAIL_TAIL_LINES: usize = 200;

/// Per-type detail header (claude-code `*DetailDialog.tsx`). Distributed types
/// (remote/dream) get a placeholder per the design non-goals.
#[must_use]
pub fn detail_header(row: &TaskRow) -> String {
    match row.task_type.as_str() {
        "local_bash" | "monitor_mcp" => "Shell details".to_string(),
        "in_process_teammate" => format!("@{}", row.description),
        "local_agent" | "local_workflow" => format!("agent \u{203A} {}", row.description),
        _ => "Detail not available in this build".to_string(),
    }
}

/// The detail body: header line, then the tailed output (last N lines).
#[must_use]
pub fn render_task_detail(row: &TaskRow, tail: &OutputTailState) -> String {
    format!(
        "{}\n{}",
        detail_header(row),
        render_output_tail(tail, DETAIL_TAIL_LINES)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::tasks::output_tail::OutputTailState;
    use crate::multiagent::state::TaskRow;

    fn row(task_type: &str, status: &str, desc: &str) -> TaskRow {
        TaskRow {
            task_id: "b1".into(),
            task_type: task_type.into(),
            status: status.into(),
            description: desc.into(),
            command: None,
        }
    }

    #[test]
    fn headers_per_type() {
        assert_eq!(
            detail_header(&row("local_bash", "running", "cargo build")),
            "Shell details"
        );
        assert_eq!(
            detail_header(&row("in_process_teammate", "running", "alice")),
            "@alice"
        );
        assert_eq!(
            detail_header(&row("local_agent", "running", "review")),
            "agent \u{203A} review"
        );
        assert_eq!(
            detail_header(&row("remote_agent", "running", "deploy")),
            "Detail not available in this build"
        );
        assert_eq!(
            detail_header(&row("dream", "running", "nightly")),
            "Detail not available in this build"
        );
    }

    #[test]
    fn body_has_header_and_tail() {
        let tail = OutputTailState {
            content: "line1\nline2".into(),
            offset: 11,
            total_lines: 2,
            truncated: false,
        };
        let out = render_task_detail(&row("local_bash", "running", "cargo build"), &tail);
        assert!(out.starts_with("Shell details\n"));
        assert!(out.ends_with("line1\nline2"));
    }
}

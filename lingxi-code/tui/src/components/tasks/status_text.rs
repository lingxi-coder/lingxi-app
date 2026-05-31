//! Task status label + the `(label)` indicator.
//!
//! Literal lock: claude-code `ShellProgress.tsx` / `TaskStatusText`
//! (`({label}{suffix})`, dim, status-colored) and `taskStatusUtils.tsx`.
//! Color is applied by the M9-05 dialog via `multiagent::style::task_status_color`.

/// Status wire string → display label (claude-code labels). Unknown → `pending`.
#[must_use]
pub fn task_status_label(status: &str) -> &'static str {
    match status {
        "completed" => "done",
        "failed" => "error",
        "killed" => "stopped",
        "running" => "running",
        _ => "pending",
    }
}

/// Wrap an explicit label as `({label}{suffix})`.
#[must_use]
pub fn wrap_status_label(label: &str, suffix: Option<&str>) -> String {
    match suffix {
        Some(s) => format!("({label}{s})"),
        None => format!("({label})"),
    }
}

/// `({label}{suffix})` for a status string (claude-code `TaskStatusText`).
#[must_use]
pub fn render_task_status_text(status: &str, suffix: Option<&str>) -> String {
    wrap_status_label(task_status_label(status), suffix)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        assert_eq!(task_status_label("completed"), "done");
        assert_eq!(task_status_label("failed"), "error");
        assert_eq!(task_status_label("killed"), "stopped");
        assert_eq!(task_status_label("running"), "running");
        assert_eq!(task_status_label("pending"), "pending");
        assert_eq!(task_status_label("weird"), "pending");
    }

    #[test]
    fn status_text_no_suffix() {
        assert_eq!(render_task_status_text("completed", None), "(done)");
        assert_eq!(render_task_status_text("running", None), "(running)");
    }

    #[test]
    fn status_text_with_suffix() {
        assert_eq!(
            render_task_status_text("completed", Some(", unread")),
            "(done, unread)"
        );
    }

    #[test]
    fn wrap_explicit_label() {
        assert_eq!(wrap_status_label("3 agents", None), "(3 agents)");
        assert_eq!(
            wrap_status_label("done", Some(", unread")),
            "(done, unread)"
        );
    }
}

//! `BackgroundTaskStatus` footer (claude-code `BackgroundTaskStatus.tsx`):
//! `{n} background task[s] · ↓ to view`. Hidden when there are no tasks or
//! every task is an in-process teammate (shown in the spinner tree instead).

use crate::multiagent::state::TaskRow;

/// ` · ↓ to view` hint (space + U+00B7 + space + U+2193 + " to view").
const VIEW_HINT: &str = " \u{00B7} \u{2193} to view";

/// The footer pill, or `None` when it should be hidden (no tasks, or every
/// task is an in-process teammate). The count reflects non-teammate tasks.
#[must_use]
pub fn render_task_footer(tasks: &[TaskRow]) -> Option<String> {
    let n = tasks
        .iter()
        .filter(|t| t.task_type != "in_process_teammate")
        .count();
    if n == 0 {
        return None;
    }
    let noun = if n == 1 { "task" } else { "tasks" };
    Some(format!("{n} background {noun}{VIEW_HINT}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multiagent::state::TaskRow;

    fn row(task_type: &str) -> TaskRow {
        TaskRow {
            task_id: "b1".into(),
            task_type: task_type.into(),
            status: "running".into(),
            description: "x".into(),
        }
    }

    #[test]
    fn hidden_when_empty() {
        assert_eq!(render_task_footer(&[]), None);
    }

    #[test]
    fn hidden_when_all_teammates() {
        let tasks = vec![row("in_process_teammate"), row("in_process_teammate")];
        assert_eq!(render_task_footer(&tasks), None);
    }

    #[test]
    fn singular_and_plural() {
        assert_eq!(
            render_task_footer(&[row("local_bash")]).as_deref(),
            Some("1 background task \u{00B7} \u{2193} to view")
        );
        let two = vec![row("local_bash"), row("local_agent")];
        assert_eq!(
            render_task_footer(&two).as_deref(),
            Some("2 background tasks \u{00B7} \u{2193} to view")
        );
    }

    #[test]
    fn counts_nonteammate_only() {
        // a mix: 1 bash + 1 teammate → count reflects the 1 non-teammate.
        let mix = vec![row("local_bash"), row("in_process_teammate")];
        assert_eq!(
            render_task_footer(&mix).as_deref(),
            Some("1 background task \u{00B7} \u{2193} to view")
        );
    }
}

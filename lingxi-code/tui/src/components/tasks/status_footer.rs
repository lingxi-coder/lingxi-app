//! `BackgroundTaskStatus` footer (claude-code `BackgroundTaskStatus.tsx`):
//! the bare `getPillLabel` text. Hidden when there are no tasks or every task
//! is an in-process teammate (shown in the spinner tree instead).
//!
//! (FOOTER-CTA-ALWAYS-ON) claude-code only appends a " · ↓ to view" CTA when
//! `pillNeedsCta` is true, which is exactly the single-ultraplan-remote_agent
//! case — an excluded cloud surface here — so the in-scope pill is always the
//! bare label with no CTA suffix.

use crate::multiagent::state::TaskRow;

/// `◇` (U+25C7) — claude-code `DIAMOND_OPEN`, prefixed on the remote/cloud
/// session pill label.
const DIAMOND_OPEN: &str = "\u{25C7}";

/// The footer pill, or `None` when it should be hidden (no tasks, or every
/// task is an in-process teammate). The label reflects non-teammate tasks.
#[must_use]
pub fn render_task_footer(tasks: &[TaskRow]) -> Option<String> {
    let pill: Vec<&TaskRow> = tasks
        .iter()
        .filter(|t| t.task_type != "in_process_teammate")
        .collect();
    if pill.is_empty() {
        return None;
    }
    Some(pill_label(&pill))
}

/// Type-specific pill label (claude-code `getPillLabel`). When all tasks share
/// a `task_type`, emit the per-type label; otherwise fall back to the generic
/// `{n} background task[s]`. `local_bash` would split into shells vs monitors,
/// but `TaskRow` carries no `kind` field yet, so all `local_bash` are treated
/// as shells (the claude-code shells-only branch). `in_process_teammate` is
/// filtered out before this is called, so its `teams` branch never applies.
fn pill_label(tasks: &[&TaskRow]) -> String {
    let n = tasks.len();
    let all_same = tasks.iter().all(|t| t.task_type == tasks[0].task_type);
    if all_same {
        match tasks[0].task_type.as_str() {
            "local_bash" => plural(n, "1 shell", "shells"),
            "local_agent" => plural(n, "1 local agent", "local agents"),
            "local_workflow" => plural(n, "1 background workflow", "background workflows"),
            "monitor_mcp" => plural(n, "1 monitor", "monitors"),
            "dream" => "dreaming".to_string(),
            "remote_agent" => {
                if n == 1 {
                    format!("{DIAMOND_OPEN} 1 cloud session")
                } else {
                    format!("{DIAMOND_OPEN} {n} cloud sessions")
                }
            }
            _ => generic_label(n),
        }
    } else {
        generic_label(n)
    }
}

/// `1 <one>` when `n == 1`, else `{n} <many>`.
fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        one.to_string()
    } else {
        format!("{n} {many}")
    }
}

/// The mixed-type / unknown-type fallback (`{n} background task[s]`).
fn generic_label(n: usize) -> String {
    let noun = if n == 1 { "task" } else { "tasks" };
    format!("{n} background {noun}")
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
            command: None,
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
        // (FOOTER-PILL-LABEL) All same type → per-type label; here local_bash
        // renders as shells (no `kind` field → all shells).
        assert_eq!(
            render_task_footer(&[row("local_bash")]).as_deref(),
            Some("1 shell")
        );
        // Mixed types → generic `{n} background tasks`.
        let two = vec![row("local_bash"), row("local_agent")];
        assert_eq!(render_task_footer(&two).as_deref(), Some("2 background tasks"));
    }

    #[test]
    fn counts_nonteammate_only() {
        // a mix: 1 bash + 1 teammate → label reflects the 1 non-teammate shell.
        let mix = vec![row("local_bash"), row("in_process_teammate")];
        assert_eq!(render_task_footer(&mix).as_deref(), Some("1 shell"));
    }

    #[test]
    fn per_type_labels() {
        let lbl = |t: &str, n: usize| {
            let rows: Vec<TaskRow> = (0..n).map(|_| row(t)).collect();
            render_task_footer(&rows).unwrap()
        };
        assert_eq!(lbl("local_bash", 3), "3 shells");
        assert_eq!(lbl("local_agent", 1), "1 local agent");
        assert_eq!(lbl("local_agent", 2), "2 local agents");
        assert_eq!(lbl("local_workflow", 2), "2 background workflows");
        assert_eq!(lbl("monitor_mcp", 1), "1 monitor");
        assert_eq!(lbl("dream", 4), "dreaming");
        assert_eq!(lbl("remote_agent", 1), "\u{25C7} 1 cloud session");
        assert_eq!(lbl("remote_agent", 2), "\u{25C7} 2 cloud sessions");
        // Unknown single type → generic fallback.
        assert_eq!(lbl("mystery", 2), "2 background tasks");
    }

    #[test]
    fn no_view_hint_cta() {
        // (FOOTER-CTA-ALWAYS-ON) The " · ↓ to view" CTA only ever applies to
        // the excluded single-ultraplan-remote_agent cloud case, so the
        // in-scope pill never carries it.
        let out = render_task_footer(&[row("local_bash")]).unwrap();
        assert!(!out.contains("to view"), "got: {out}");
    }
}

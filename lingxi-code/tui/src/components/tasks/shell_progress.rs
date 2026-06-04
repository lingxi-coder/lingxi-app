//! `ShellProgress` — the `local_bash` task row (claude-code `ShellProgress.tsx`
//! + `BackgroundTask.tsx` `local_bash` case): `{command} ({label})` with an
//!   optional trailing elapsed (` 12s`) when known.

use crate::components::tasks::format::format_duration;
use crate::components::tasks::status_text::render_task_status_text;

/// `{command} ({label})` + optional ` {elapsed}` (claude-code `local_bash` row).
#[must_use]
pub fn render_shell_progress_to_string(
    command: &str,
    status: &str,
    elapsed_ms: Option<u64>,
) -> String {
    let mut out = format!("{command} {}", render_task_status_text(status, None));
    if let Some(ms) = elapsed_ms {
        out.push(' ');
        out.push_str(&format_duration(ms));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn running_no_elapsed() {
        assert_eq!(
            render_shell_progress_to_string("cargo build", "running", None),
            "cargo build (running)"
        );
    }

    #[test]
    fn completed_with_elapsed() {
        assert_eq!(
            render_shell_progress_to_string("ls", "completed", Some(12_000)),
            "ls (done) 12s"
        );
    }

    #[test]
    fn failed_no_elapsed() {
        assert_eq!(
            render_shell_progress_to_string("false", "failed", None),
            "false (error)"
        );
    }
}

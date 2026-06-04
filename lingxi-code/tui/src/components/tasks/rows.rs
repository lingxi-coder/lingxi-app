//! Per-type background-task row renderers (claude-code `BackgroundTask.tsx`
//! cases). Pure string renderers; rich optionals (`notified`, counts,
//! `activity`, dream `phase`/`detail`) are passed by the caller — the live
//! `super::render_task_row` passes the wire subset (defaults), fixtures pass
//! full data. Per spec §2.4 the live row omits what the wire does not carry.

use crate::components::tasks::status_text::{render_task_status_text, wrap_status_label};

/// `◇` remote running/pending diamond (U+25C7).
pub const DIAMOND_OPEN: char = '\u{25C7}';
/// `◆` remote terminal diamond (U+25C6).
pub const DIAMOND_FILLED: char = '\u{25C6}';
/// ` · ` middot separator (space + U+00B7 + space).
pub const MIDDOT: &str = " \u{00B7} ";

fn unread_suffix(status: &str, notified: bool) -> Option<&'static str> {
    (status == "completed" && !notified).then_some(", unread")
}

/// `local_agent`: `{description} ({label}[, unread])`.
#[must_use]
pub fn render_local_agent_row(description: &str, status: &str, notified: bool) -> String {
    format!(
        "{description} {}",
        render_task_status_text(status, unread_suffix(status, notified))
    )
}

/// `remote_agent`: `◇|◆ {title} · {progress}` where progress is `{done}/{total}`
/// when counts are known, else `done`/`error`/`stopped`/`{status}…`.
#[must_use]
pub fn render_remote_agent_row(title: &str, status: &str, progress: Option<(u64, u64)>) -> String {
    let diamond = if matches!(status, "running" | "pending") {
        DIAMOND_OPEN
    } else {
        DIAMOND_FILLED
    };
    let prog = match progress {
        Some((done, total)) => format!("{done}/{total}"),
        None => match status {
            "completed" => "done".to_string(),
            "failed" => "error".to_string(),
            "killed" => "stopped".to_string(),
            other => format!("{other}…"),
        },
    };
    format!("{diamond} {title}{MIDDOT}{prog}")
}

/// `in_process_teammate`: `@{name} : {activity}` when activity known, else
/// `@{name} ({label})`.
#[must_use]
pub fn render_in_process_teammate_row(name: &str, status: &str, activity: Option<&str>) -> String {
    match activity {
        Some(a) => format!("@{name} : {a}"),
        None => format!("@{name} {}", render_task_status_text(status, None)),
    }
}

/// `local_workflow`: `{name} ({n agents}|{label}[, unread])` — running shows the
/// agent count when known.
#[must_use]
pub fn render_local_workflow_row(
    name: &str,
    status: &str,
    agent_count: Option<u64>,
    notified: bool,
) -> String {
    let suffix = unread_suffix(status, notified);
    let text = match (status, agent_count) {
        ("running", Some(n)) => {
            let noun = if n == 1 { "agent" } else { "agents" };
            wrap_status_label(&format!("{n} {noun}"), suffix)
        }
        _ => render_task_status_text(status, suffix),
    };
    format!("{name} {text}")
}

/// `monitor_mcp`: `{description} ({label}[, unread])`.
#[must_use]
pub fn render_monitor_mcp_row(description: &str, status: &str, notified: bool) -> String {
    format!(
        "{description} {}",
        render_task_status_text(status, unread_suffix(status, notified))
    )
}

/// dream: `{description}[ · {phase}][ · {detail}] ({label})`.
#[must_use]
pub fn render_dream_row(
    description: &str,
    status: &str,
    phase: Option<&str>,
    detail: Option<&str>,
) -> String {
    let mut out = description.to_string();
    if let Some(p) = phase {
        out.push_str(MIDDOT);
        out.push_str(p);
    }
    if let Some(d) = detail {
        out.push_str(MIDDOT);
        out.push_str(d);
    }
    format!("{out} {}", render_task_status_text(status, None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_agent_unread_only_when_completed_and_unnotified() {
        assert_eq!(
            render_local_agent_row("review", "running", false),
            "review (running)"
        );
        assert_eq!(
            render_local_agent_row("review", "completed", false),
            "review (done, unread)"
        );
        assert_eq!(
            render_local_agent_row("review", "completed", true),
            "review (done)"
        );
    }

    #[test]
    fn remote_agent_diamond_and_progress() {
        // running → open diamond + "running…"; with counts → "d/t".
        assert_eq!(
            render_remote_agent_row("deploy", "running", None),
            "\u{25C7} deploy \u{00B7} running…"
        );
        assert_eq!(
            render_remote_agent_row("deploy", "running", Some((3, 7))),
            "\u{25C7} deploy \u{00B7} 3/7"
        );
        // completed → filled diamond + "done".
        assert_eq!(
            render_remote_agent_row("deploy", "completed", None),
            "\u{25C6} deploy \u{00B7} done"
        );
    }

    #[test]
    fn in_process_teammate_name_and_activity() {
        assert_eq!(
            render_in_process_teammate_row("alice", "running", Some("editing")),
            "@alice : editing"
        );
        assert_eq!(
            render_in_process_teammate_row("alice", "running", None),
            "@alice (running)"
        );
    }

    #[test]
    fn local_workflow_agent_count_when_running() {
        assert_eq!(
            render_local_workflow_row("deploy", "running", Some(3), false),
            "deploy (3 agents)"
        );
        assert_eq!(
            render_local_workflow_row("deploy", "running", Some(1), false),
            "deploy (1 agent)"
        );
        assert_eq!(
            render_local_workflow_row("deploy", "completed", None, false),
            "deploy (done, unread)"
        );
    }

    #[test]
    fn monitor_mcp_like_agent() {
        assert_eq!(
            render_monitor_mcp_row("watch fs", "running", false),
            "watch fs (running)"
        );
        assert_eq!(
            render_monitor_mcp_row("watch fs", "completed", true),
            "watch fs (done)"
        );
    }

    #[test]
    fn dream_phase_and_detail() {
        assert_eq!(
            render_dream_row("nightly", "running", Some("updating"), Some("5 files")),
            "nightly \u{00B7} updating \u{00B7} 5 files (running)"
        );
        assert_eq!(
            render_dream_row("nightly", "running", None, None),
            "nightly (running)"
        );
    }
}

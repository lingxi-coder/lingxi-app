//! Compact task-plan and running-agent rows adjacent to the composer.
//!
//! Planned tasks render above the input; live agents render below it. Both
//! surfaces are deliberately bounded so a large plan or agent fan-out cannot
//! consume the whole terminal and hide the composer.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use tui_core::orchestrator_bridge::RunningAgentStatus;
use tui_core::theme::Theme;

/// Maximum task rows shown before a compact overflow line.
const MAX_VISIBLE_TASKS: usize = 5;

/// Presentation state for one item in the model-managed working plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanTaskState {
    /// Not started yet.
    Pending,
    /// Currently being worked on.
    InProgress,
    /// Finished.
    Completed,
}

impl PlanTaskState {
    /// Parse the TodoWrite / Task tool wire status.
    #[must_use]
    pub fn from_wire(status: &str) -> Option<Self> {
        match status {
            "pending" => Some(Self::Pending),
            "in_progress" => Some(Self::InProgress),
            "completed" => Some(Self::Completed),
            _ => None,
        }
    }
}

/// One planned task displayed above the composer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanTask {
    /// Stable V2 task id. TodoWrite V1 items have no id.
    pub id: Option<String>,
    /// Imperative task title.
    pub subject: String,
    /// Present-continuous label used while the task is in progress.
    pub active_form: Option<String>,
    /// Current lifecycle state.
    pub state: PlanTaskState,
}

fn task_row(task: &PlanTask, theme: &Theme) -> Line<'static> {
    // Claude's normal (non-standalone) task renderer always uses `subject` in
    // the list. `activeForm` belongs to the spinner verb above it.
    let (glyph, color) = match task.state {
        PlanTaskState::Pending => ("◻", theme.text),
        PlanTaskState::InProgress => ("◼", theme.suggestion),
        PlanTaskState::Completed => ("✔", theme.success),
    };
    let label_style = match task.state {
        PlanTaskState::Pending => Style::default().fg(crate::style_adapter::to_ratatui(theme.text)),
        PlanTaskState::InProgress => Style::default()
            .fg(crate::style_adapter::to_ratatui(theme.text))
            .add_modifier(Modifier::BOLD),
        PlanTaskState::Completed => Style::default()
            .fg(crate::style_adapter::to_ratatui(theme.dim))
            .add_modifier(Modifier::CROSSED_OUT),
    };
    Line::from(vec![
        Span::raw("  "),
        Span::styled(
            format!("{glyph} "),
            Style::default().fg(crate::style_adapter::to_ratatui(color)),
        ),
        Span::styled(task.subject.clone(), label_style),
    ])
}

/// Render the bounded plan block shown immediately above the composer.
#[must_use]
pub fn task_lines(tasks: &[PlanTask], theme: &Theme) -> Vec<Line<'static>> {
    if tasks.is_empty() {
        return Vec::new();
    }
    let dim = crate::style_adapter::to_ratatui(theme.dim);
    let mut lines = tasks
        .iter()
        .take(MAX_VISIBLE_TASKS)
        .map(|task| task_row(task, theme))
        .collect::<Vec<_>>();
    if tasks.len() > MAX_VISIBLE_TASKS {
        let hidden = &tasks[MAX_VISIBLE_TASKS..];
        let mut counts = Vec::new();
        let in_progress = hidden
            .iter()
            .filter(|task| task.state == PlanTaskState::InProgress)
            .count();
        let pending = hidden
            .iter()
            .filter(|task| task.state == PlanTaskState::Pending)
            .count();
        let completed = hidden
            .iter()
            .filter(|task| task.state == PlanTaskState::Completed)
            .count();
        if in_progress > 0 {
            counts.push(format!("{in_progress} in progress"));
        }
        if pending > 0 {
            counts.push(format!("{pending} pending"));
        }
        if completed > 0 {
            counts.push(format!("{completed} completed"));
        }
        lines.push(Line::from(Span::styled(
            format!("  … +{}", counts.join(", ")),
            Style::default().fg(dim),
        )));
    }
    lines
}

/// Claude's type-aware background-task footer summary (`Ldt` in 2.1.216).
///
/// This surface intentionally summarizes instead of listing descriptions. The
/// detailed rows remain available in `/agents`; next to the composer Claude
/// renders one stable pill such as `2 local agents`.
fn agent_summary(agents: &[RunningAgentStatus]) -> String {
    let count = agents.len();
    let first_type = agents[0].task_type.as_str();
    let homogeneous = agents.iter().all(|agent| agent.task_type == first_type);

    if homogeneous {
        return match first_type {
            "local_agent" => match count {
                1 => "1 local agent".to_string(),
                _ => format!("{count} local agents"),
            },
            // LingXi permits one coordinator team at a time. Claude counts
            // unique team names here, so every homogeneous teammate snapshot
            // from this per-session registry represents one team.
            "in_process_teammate" => "1 team".to_string(),
            "remote_agent" => match count {
                1 => "◇ 1 cloud session".to_string(),
                _ => format!("◇ {count} cloud sessions"),
            },
            _ => match count {
                1 => "1 background task".to_string(),
                _ => format!("{count} background tasks"),
            },
        };
    }

    match count {
        1 => "1 background task".to_string(),
        _ => format!("{count} background tasks"),
    }
}

/// Render the Claude-style live-agent summary immediately below the composer.
#[must_use]
pub fn agent_lines(agents: &[RunningAgentStatus], theme: &Theme) -> Vec<Line<'static>> {
    if agents.is_empty() {
        return Vec::new();
    }
    let accent = crate::style_adapter::to_ratatui(theme.suggestion);
    vec![Line::from(vec![
        Span::raw("  "),
        Span::styled(agent_summary(agents), Style::default().fg(accent)),
    ])]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn task_block_shows_progress_and_each_state() {
        let tasks = vec![
            PlanTask {
                id: None,
                subject: "Inspect layout".into(),
                active_form: None,
                state: PlanTaskState::Completed,
            },
            PlanTask {
                id: None,
                subject: "Implement status".into(),
                active_form: Some("Implementing status".into()),
                state: PlanTaskState::InProgress,
            },
            PlanTask {
                id: None,
                subject: "Build CLI".into(),
                active_form: None,
                state: PlanTaskState::Pending,
            },
        ];
        let lines = plain(&task_lines(&tasks, &Theme::dark()));
        assert_eq!(lines[0], "  ✔ Inspect layout");
        assert_eq!(lines[1], "  ◼ Implement status");
        assert_eq!(lines[2], "  ◻ Build CLI");
    }

    #[test]
    fn task_block_uses_claude_overflow_copy() {
        let tasks = (0..7)
            .map(|index| PlanTask {
                id: Some(index.to_string()),
                subject: format!("Task {index}"),
                active_form: None,
                state: if index == 5 {
                    PlanTaskState::InProgress
                } else {
                    PlanTaskState::Pending
                },
            })
            .collect::<Vec<_>>();
        let lines = plain(&task_lines(&tasks, &Theme::dark()));
        assert_eq!(lines.len(), MAX_VISIBLE_TASKS + 1);
        assert_eq!(lines.last().unwrap(), "  … +1 in progress, 1 pending");
    }

    #[test]
    fn agent_block_matches_claude_type_aware_summary() {
        let agents = (0..2)
            .map(|index| RunningAgentStatus {
                id: format!("a{index}"),
                task_type: "local_agent".into(),
                agent_type: "Explore".into(),
                description: format!("Mapping lane {index}"),
                status: "running".into(),
            })
            .collect::<Vec<_>>();
        let lines = plain(&agent_lines(&agents, &Theme::dark()));
        assert_eq!(lines, vec!["  2 local agents"]);

        let remote = vec![RunningAgentStatus {
            id: "r1".into(),
            task_type: "remote_agent".into(),
            agent_type: "Agent".into(),
            description: "Remote work".into(),
            status: "pending".into(),
        }];
        assert_eq!(
            plain(&agent_lines(&remote, &Theme::dark())),
            vec!["  ◇ 1 cloud session"]
        );

        let teammates = vec![
            RunningAgentStatus {
                id: "t1".into(),
                task_type: "in_process_teammate".into(),
                agent_type: "teammate".into(),
                description: "Review".into(),
                status: "running".into(),
            },
            RunningAgentStatus {
                id: "t2".into(),
                task_type: "in_process_teammate".into(),
                agent_type: "teammate".into(),
                description: "Test".into(),
                status: "running".into(),
            },
        ];
        assert_eq!(
            plain(&agent_lines(&teammates, &Theme::dark())),
            vec!["  1 team"]
        );

        let mixed = vec![agents[0].clone(), remote[0].clone()];
        assert_eq!(
            plain(&agent_lines(&mixed, &Theme::dark())),
            vec!["  2 background tasks"]
        );
    }
}

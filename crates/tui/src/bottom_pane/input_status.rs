//! Compact task-plan and running-agent rows adjacent to the composer.
//!
//! Planned tasks render above the input; live agents render below it. Both
//! surfaces are deliberately bounded so a large plan or agent fan-out cannot
//! consume the whole terminal and hide the composer.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use tui_core::orchestrator_bridge::RunningAgentStatus;
use tui_core::theme::Theme;
use tui_core::tool_display::plan;

// The plan model and its pure string logic now live in
// `tui_core::tool_display::plan` so the terminal, iOS, Android, and the
// Electron desktop all parse TodoWrite the same way and spell the overflow
// line identically. Re-exported here so every existing
// `crate::bottom_pane::input_status::PlanTask` path keeps resolving.
pub use tui_core::tool_display::plan::{
    max_visible_tasks, PlanTask, PlanTaskState, MAX_VISIBLE_TASKS,
};

/// Render transient hook status rows above the composer. These rows are live
/// state only and deliberately have no transcript representation.
#[must_use]
pub fn hook_lines(
    hooks: &std::collections::BTreeMap<String, String>,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let accent = crate::style_adapter::to_ratatui(theme.suggestion);
    hooks
        .values()
        .map(|status| {
            Line::from(vec![
                Span::raw("  "),
                Span::styled("\u{00b7} ", Style::default().fg(accent)),
                Span::styled(status.clone(), Style::default().fg(accent)),
            ])
        })
        .collect()
}

fn task_row(task: &PlanTask, theme: &Theme) -> Line<'static> {
    // Claude's normal (non-standalone) task renderer always uses `subject` in
    // the list. `activeForm` belongs to the spinner verb above it.
    //
    // The glyph is shared data; the color and the BOLD/CROSSED_OUT modifiers
    // are terminal styling and stay here.
    let glyph = task.state.glyph();
    let color = match task.state {
        PlanTaskState::Pending => theme.text,
        PlanTaskState::InProgress => theme.suggestion,
        PlanTaskState::Completed => theme.success,
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
    task_lines_with_limit(tasks, theme, MAX_VISIBLE_TASKS)
}

/// Render the plan block with the caller-supplied Claude task-row cap.
#[must_use]
pub fn task_lines_with_limit(
    tasks: &[PlanTask],
    theme: &Theme,
    max_visible: usize,
) -> Vec<Line<'static>> {
    if tasks.is_empty() || max_visible == 0 {
        return Vec::new();
    }
    let dim = crate::style_adapter::to_ratatui(theme.dim);
    let mut lines = tasks
        .iter()
        .take(max_visible)
        .map(|task| task_row(task, theme))
        .collect::<Vec<_>>();
    if tasks.len() > max_visible {
        if let Some(summary) = plan::overflow_summary(&tasks[max_visible..]) {
            // `overflow_summary` returns the leading `+`, so this stays
            // byte-identical to the previous `format!("  … +{}", …)`.
            lines.push(Line::from(Span::styled(
                format!("  … {summary}"),
                Style::default().fg(dim),
            )));
        }
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
    if agents.iter().any(|agent| agent.custom_content.is_some()) {
        return agents
            .iter()
            .filter_map(|agent| agent.custom_content.as_deref())
            .filter(|content| !content.is_empty())
            .map(|content| {
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled(content.to_string(), Style::default().fg(accent)),
                ])
            })
            .collect();
    }
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
        let rendered = task_lines(&tasks, &Theme::dark());
        let lines = plain(&rendered);
        assert_eq!(lines[0], "  ✔ Inspect layout");
        assert_eq!(lines[1], "  ◼ Implement status");
        assert_eq!(lines[2], "  ◻ Build CLI");
        assert!(rendered[0].spans[2]
            .style
            .add_modifier
            .contains(Modifier::CROSSED_OUT));
        assert!(rendered[1].spans[2]
            .style
            .add_modifier
            .contains(Modifier::BOLD));
        assert!(rendered[2].spans[2].style.add_modifier.is_empty());
    }

    #[test]
    fn hook_status_rows_preserve_configured_copy() {
        let hooks = std::collections::BTreeMap::from([
            ("run-1".to_string(), "Formatting\u{2026}".to_string()),
            ("run-2".to_string(), "Checking policy".to_string()),
        ]);
        assert_eq!(
            plain(&hook_lines(&hooks, &Theme::dark())),
            vec![
                "  \u{00b7} Formatting\u{2026}",
                "  \u{00b7} Checking policy"
            ]
        );
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
    fn task_cap_tracks_claude_terminal_height_thresholds() {
        assert_eq!(max_visible_tasks(10), 0);
        assert_eq!(max_visible_tasks(11), 3);
        assert_eq!(max_visible_tasks(17), 3);
        assert_eq!(max_visible_tasks(18), 4);
        assert_eq!(max_visible_tasks(19), 5);
        assert_eq!(max_visible_tasks(80), 5);

        let task = PlanTask {
            id: None,
            subject: "Hidden on tiny terminals".into(),
            active_form: None,
            state: PlanTaskState::Pending,
        };
        assert!(task_lines_with_limit(&[task], &Theme::dark(), 0).is_empty());
    }

    #[test]
    fn agent_block_matches_claude_type_aware_summary() {
        let agents = (0..2)
            .map(|index| RunningAgentStatus {
                awaiting_plan_approval: false,
                id: format!("a{index}"),
                task_type: "local_agent".into(),
                agent_type: "Explore".into(),
                description: format!("Mapping lane {index}"),
                status: "running".into(),
                custom_content: None,
            })
            .collect::<Vec<_>>();
        let lines = plain(&agent_lines(&agents, &Theme::dark()));
        assert_eq!(lines, vec!["  2 local agents"]);

        let remote = vec![RunningAgentStatus {
            awaiting_plan_approval: false,
            id: "r1".into(),
            task_type: "remote_agent".into(),
            agent_type: "Agent".into(),
            description: "Remote work".into(),
            status: "pending".into(),
            custom_content: None,
        }];
        assert_eq!(
            plain(&agent_lines(&remote, &Theme::dark())),
            vec!["  ◇ 1 cloud session"]
        );

        let teammates = vec![
            RunningAgentStatus {
                awaiting_plan_approval: false,
                id: "t1".into(),
                task_type: "in_process_teammate".into(),
                agent_type: "teammate".into(),
                description: "Review".into(),
                status: "running".into(),
                custom_content: None,
            },
            RunningAgentStatus {
                awaiting_plan_approval: false,
                id: "t2".into(),
                task_type: "in_process_teammate".into(),
                agent_type: "teammate".into(),
                description: "Test".into(),
                status: "running".into(),
                custom_content: None,
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

    #[test]
    fn custom_agent_rows_replace_summary_and_empty_content_hides() {
        let agents = vec![
            RunningAgentStatus {
                awaiting_plan_approval: false,
                id: "a1".into(),
                task_type: "local_agent".into(),
                agent_type: "Explore".into(),
                description: "Map".into(),
                status: "running".into(),
                custom_content: Some("Exploring 42%".into()),
            },
            RunningAgentStatus {
                awaiting_plan_approval: false,
                id: "a2".into(),
                task_type: "local_agent".into(),
                agent_type: "Plan".into(),
                description: "Plan".into(),
                status: "running".into(),
                custom_content: Some(String::new()),
            },
        ];
        assert_eq!(
            plain(&agent_lines(&agents, &Theme::dark())),
            vec!["  Exploring 42%"]
        );
    }
}

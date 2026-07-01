//! `TaskAssignmentMessage` — task assignment notice.
//!
//! Literal lock (claude-code `TaskAssignmentMessage.tsx`): round
//! subagent-cyan border; bold header `Task #{task_id} assigned by
//! {assigned_by}`; bold subject line; optional dim description line.
#![allow(clippy::needless_pass_by_value)]

use iocraft::prelude::*;

use crate::multiagent::style::{agent_color, AgentColor};
use crate::theme::Theme;
use crate::render_iocraft::StyleColorIocraftExt;

/// Props for [`TaskAssignmentMessage`].
#[derive(Debug, Clone, Default, Props)]
pub struct TaskAssignmentProps {
    /// Task id (rendered `#{task_id}`).
    pub task_id: String,
    /// Assigning agent name.
    pub assigned_by: String,
    /// Task subject / title.
    pub subject: String,
    /// Optional description.
    pub description: Option<String>,
    /// Active palette.
    pub theme: Theme,
}

/// Pure-string renderer (border applied by the component). Lines: header,
/// subject, optional description.
#[must_use]
pub fn render_task_assignment_to_string(props: TaskAssignmentProps) -> String {
    let mut out = format!(
        "Task #{} assigned by {}\n{}",
        props.task_id, props.assigned_by, props.subject
    );
    if let Some(desc) = &props.description {
        out.push('\n');
        out.push_str(desc);
    }
    out
}

/// iocraft component. Round cyan border; bold header + subject; dim
/// description.
#[component]
pub fn TaskAssignmentMessage(props: &TaskAssignmentProps) -> impl Into<AnyElement<'static>> {
    let cyan = agent_color(AgentColor::Cyan);
    let header = format!("Task #{} assigned by {}", props.task_id, props.assigned_by);
    let subject = props.subject.clone();
    let description = props.description.clone();
    let dim = props.theme.dim;
    element! {
        View(
            flex_direction: FlexDirection::Column,
            border_style: BorderStyle::Round,
            border_color: cyan,
        ) {
            Text(content: header, color: cyan, weight: Weight::Bold)
            Text(content: subject, weight: Weight::Bold)
            #(description.map(|d| element! {
                Text(content: d, color: dim.to_iocraft())
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_subject_only() {
        let out = render_task_assignment_to_string(TaskAssignmentProps {
            task_id: "123".into(),
            assigned_by: "alice".into(),
            subject: "Set up DB".into(),
            description: None,
            theme: Theme::dark(),
        });
        assert_eq!(out, "Task #123 assigned by alice\nSet up DB");
    }

    #[test]
    fn with_description() {
        let out = render_task_assignment_to_string(TaskAssignmentProps {
            task_id: "7".into(),
            assigned_by: "lead".into(),
            subject: "Migrate".into(),
            description: Some("Move tables".into()),
            theme: Theme::dark(),
        });
        assert_eq!(out, "Task #7 assigned by lead\nMigrate\nMove tables");
    }
}

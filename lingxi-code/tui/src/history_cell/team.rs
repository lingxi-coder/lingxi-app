//! Per-variant history cells for the team/agents surfaces: task assignments,
//! background-agent notifications, channel messages, teammate notes,
//! shutdown notices, hook progress, and plan approvals.
//!
//! Split out of `message.rs` in the second message-cells phase (plan Phase
//! 9). The styled-line renderers here are the single source for these
//! variants: the cells consume them through [`StyledCell`], and the legacy
//! [`crate::message::render_message`] dispatcher delegates to them so its
//! output stays line-identical to the pre-split renderer.

use tui_core::message::{PlanApprovalKind, UserTeammateKind};
use tui_core::render::{agent_color_from_name, SpanStyle, StyledLine, StyledSpan};
use tui_core::theme::Theme;

use super::{colored_lines, plain_lines, StyledCell};

/// Teammate shutdown notice: dim `{from} shut down` + optional `: {reason}`.
pub(crate) fn shutdown_lines(from: &str, reason: Option<&str>, theme: &Theme) -> Vec<StyledLine> {
    let msg = match reason {
        Some(r) => format!("{from} shut down: {r}"),
        None => format!("{from} shut down"),
    };
    colored_lines(&msg, theme.dim)
}

/// Task assignment: plain `Task: {subject}` + optional dim description lines.
pub(crate) fn task_assignment_lines(
    subject: &str,
    description: Option<&str>,
    theme: &Theme,
) -> Vec<StyledLine> {
    let mut out = vec![StyledLine::plain(format!("Task: {subject}"))];
    if let Some(d) = description {
        out.extend(colored_lines(d, theme.dim));
    }
    out
}

/// Background-agent notification: the dim summary line. An empty summary
/// renders nothing (tui-core doc contract — the documented hidden case).
pub(crate) fn agent_notification_lines(summary: &str, theme: &Theme) -> Vec<StyledLine> {
    colored_lines(summary, theme.dim)
}

/// Inbound channel message: `[{server}] {user-or-system}: {content}`.
pub(crate) fn channel_message_lines(
    server: &str,
    user: Option<&str>,
    content: &str,
) -> Vec<StyledLine> {
    let who = user.unwrap_or("system");
    vec![StyledLine::plain(format!("[{server}] {who}: {content}"))]
}

/// A teammate message: an agent-colored `@name` header then the kind-specific
/// body — a completed-task line (success) or a plain note (summary + optional
/// content). `color` is the teammate's claude-code color name.
pub(crate) fn teammate_lines(
    display_name: &str,
    color: Option<&str>,
    kind: &UserTeammateKind,
    theme: &Theme,
) -> Vec<StyledLine> {
    let name_color = agent_color_from_name(color.unwrap_or(""));
    let name_span = |text: String| {
        StyledSpan::styled(
            text,
            SpanStyle {
                fg: name_color,
                ..SpanStyle::default()
            },
        )
    };
    match kind {
        UserTeammateKind::TaskCompleted {
            task_id,
            task_subject,
        } => {
            let subject = task_subject
                .as_deref()
                .map(|s| format!(" ({s})"))
                .unwrap_or_default();
            vec![StyledLine {
                spans: vec![
                    name_span(format!("@{display_name}: ")),
                    StyledSpan::styled(
                        format!("✓ Completed task #{task_id}{subject}"),
                        SpanStyle {
                            fg: theme.success,
                            ..SpanStyle::default()
                        },
                    ),
                ],
            }]
        }
        UserTeammateKind::Note {
            summary,
            content,
            is_transcript_mode,
        } => {
            let mut out = vec![StyledLine {
                spans: vec![name_span(format!("@{display_name}"))],
            }];
            if let Some(s) = summary {
                out.extend(colored_lines(s, theme.dim));
            }
            if *is_transcript_mode {
                if let Some(c) = content {
                    out.extend(colored_lines(c, theme.dim));
                }
            }
            out
        }
    }
}

/// Hook-progress line: dim `hook: {event} (×{count})`.
pub(crate) fn hook_progress_lines(event: &str, count: u32, theme: &Theme) -> Vec<StyledLine> {
    colored_lines(&format!("hook: {event} (×{count})"), theme.dim)
}

/// One nested subagent tool-call line — a dim, indented `⎿` line rendered
/// under the parent `Task` cell so the subagent's inner work is visible.
pub(crate) fn subagent_activity_lines(text: &str, theme: &Theme) -> Vec<StyledLine> {
    colored_lines(&format!("  \u{23BF}  {text}"), theme.dim)
}

/// A plan-approval request/response line.
pub(crate) fn plan_approval_lines(kind: &PlanApprovalKind, theme: &Theme) -> Vec<StyledLine> {
    match kind {
        PlanApprovalKind::Request {
            from, plan_content, ..
        } => {
            let mut out =
                colored_lines(&format!("Plan approval requested by {from}"), theme.warning);
            out.extend(plain_lines(plan_content));
            out
        }
        PlanApprovalKind::Approved { name } => {
            colored_lines(&format!("✓ Plan approved by {name}"), theme.success)
        }
        PlanApprovalKind::Rejected { name, feedback } => {
            let mut out = colored_lines(&format!("✗ Plan rejected by {name}"), theme.error);
            if let Some(f) = feedback {
                out.extend(colored_lines(&format!("  {f}"), theme.dim));
            }
            out
        }
    }
}

/// [`RenderedMessage::Shutdown`](tui_core::message::RenderedMessage::Shutdown)
/// — the dim teammate shutdown notice. The message's `rejected` flag is
/// intentionally not rendered (request and rejected response share the same
/// line — pre-split behavior).
#[derive(Debug)]
pub struct ShutdownCell {
    from: String,
    reason: Option<String>,
}

impl ShutdownCell {
    /// Wrap a shutdown notice.
    #[must_use]
    pub fn new(from: String, reason: Option<String>) -> Self {
        Self { from, reason }
    }
}

impl StyledCell for ShutdownCell {
    fn styled_lines(&self, _width: usize, theme: &Theme, _verbose: bool) -> Vec<StyledLine> {
        shutdown_lines(&self.from, self.reason.as_deref(), theme)
    }
}

/// [`RenderedMessage::TaskAssignment`](tui_core::message::RenderedMessage::TaskAssignment)
/// — `Task: {subject}` + optional dim description. `task_id`/`assigned_by`
/// are intentionally not rendered (pre-split behavior).
#[derive(Debug)]
pub struct TaskAssignmentCell {
    subject: String,
    description: Option<String>,
}

impl TaskAssignmentCell {
    /// Wrap a task assignment notice.
    #[must_use]
    pub fn new(subject: String, description: Option<String>) -> Self {
        Self {
            subject,
            description,
        }
    }
}

impl StyledCell for TaskAssignmentCell {
    fn styled_lines(&self, _width: usize, theme: &Theme, _verbose: bool) -> Vec<StyledLine> {
        task_assignment_lines(&self.subject, self.description.as_deref(), theme)
    }
}

/// [`RenderedMessage::AgentNotification`](tui_core::message::RenderedMessage::AgentNotification)
/// — the dim background-agent summary line. An empty summary is the
/// documented hidden case (renders nothing, [`is_visible`] false). The
/// message's `status` is intentionally not rendered (pre-split behavior).
///
/// [`is_visible`]: super::HistoryCell::is_visible
#[derive(Debug)]
pub struct AgentNotificationCell {
    summary: String,
}

impl AgentNotificationCell {
    /// Wrap a background-agent notification summary.
    #[must_use]
    pub fn new(summary: String) -> Self {
        Self { summary }
    }
}

impl StyledCell for AgentNotificationCell {
    fn styled_lines(&self, _width: usize, theme: &Theme, _verbose: bool) -> Vec<StyledLine> {
        agent_notification_lines(&self.summary, theme)
    }
}

/// [`RenderedMessage::ChannelMessage`](tui_core::message::RenderedMessage::ChannelMessage)
/// — the `[{server}] {who}: {content}` inbound channel line.
#[derive(Debug)]
pub struct ChannelMessageCell {
    server: String,
    user: Option<String>,
    content: String,
}

impl ChannelMessageCell {
    /// Wrap an inbound channel message.
    #[must_use]
    pub fn new(server: String, user: Option<String>, content: String) -> Self {
        Self {
            server,
            user,
            content,
        }
    }
}

impl StyledCell for ChannelMessageCell {
    fn styled_lines(&self, _width: usize, _theme: &Theme, _verbose: bool) -> Vec<StyledLine> {
        channel_message_lines(&self.server, self.user.as_deref(), &self.content)
    }
}

/// [`RenderedMessage::UserTeammate`](tui_core::message::RenderedMessage::UserTeammate)
/// — the agent-colored `@name` header + kind body (completed task or note).
#[derive(Debug)]
pub struct UserTeammateCell {
    display_name: String,
    color: Option<String>,
    kind: UserTeammateKind,
}

impl UserTeammateCell {
    /// Wrap a teammate message.
    #[must_use]
    pub fn new(display_name: String, color: Option<String>, kind: UserTeammateKind) -> Self {
        Self {
            display_name,
            color,
            kind,
        }
    }
}

impl StyledCell for UserTeammateCell {
    fn styled_lines(&self, _width: usize, theme: &Theme, _verbose: bool) -> Vec<StyledLine> {
        teammate_lines(&self.display_name, self.color.as_deref(), &self.kind, theme)
    }
}

/// [`RenderedMessage::HookProgress`](tui_core::message::RenderedMessage::HookProgress)
/// — the dim `hook: {event} (×{count})` line. `transcript_summary` gates
/// commit-safety upstream, not rendering.
#[derive(Debug)]
pub struct HookProgressCell {
    event: String,
    count: u32,
}

impl HookProgressCell {
    /// Wrap a hook-progress line.
    #[must_use]
    pub fn new(event: String, count: u32) -> Self {
        Self { event, count }
    }
}

impl StyledCell for HookProgressCell {
    fn styled_lines(&self, _width: usize, theme: &Theme, _verbose: bool) -> Vec<StyledLine> {
        hook_progress_lines(&self.event, self.count, theme)
    }
}

/// [`RenderedMessage::SubagentActivity`](tui_core::message::RenderedMessage::SubagentActivity)
/// — one nested subagent tool-call line, dim + indented under the Task cell.
#[derive(Debug)]
pub struct SubagentActivityCell {
    text: String,
}

impl SubagentActivityCell {
    /// Wrap a nested subagent activity line.
    #[must_use]
    pub fn new(text: String) -> Self {
        Self { text }
    }
}

impl StyledCell for SubagentActivityCell {
    fn styled_lines(&self, _width: usize, theme: &Theme, _verbose: bool) -> Vec<StyledLine> {
        subagent_activity_lines(&self.text, theme)
    }
}

/// [`RenderedMessage::PlanApproval`](tui_core::message::RenderedMessage::PlanApproval)
/// — plan approval request (warning + plan body), approved (success) or
/// rejected (error + optional dim feedback).
#[derive(Debug)]
pub struct PlanApprovalCell {
    kind: PlanApprovalKind,
}

impl PlanApprovalCell {
    /// Wrap a plan-approval request/response.
    #[must_use]
    pub fn new(kind: PlanApprovalKind) -> Self {
        Self { kind }
    }
}

impl StyledCell for PlanApprovalCell {
    fn styled_lines(&self, _width: usize, theme: &Theme, _verbose: bool) -> Vec<StyledLine> {
        plan_approval_lines(&self.kind, theme)
    }
}

#[cfg(test)]
mod tests {
    use super::super::{HistoryCell, RenderMode};
    use super::*;

    fn plain(cell: &dyn HistoryCell) -> Vec<String> {
        cell.display_lines(80, &Theme::dark(), RenderMode::default())
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    fn first_fg(cell: &dyn HistoryCell) -> Option<ratatui::style::Color> {
        cell.display_lines(80, &Theme::dark(), RenderMode::default())[0].spans[0]
            .style
            .fg
    }

    fn rata(color: tui_core::render::StyleColor) -> ratatui::style::Color {
        crate::style_adapter::to_ratatui(color)
    }

    #[test]
    fn shutdown_cell_renders_dim_notice_with_and_without_reason() {
        let with_reason = ShutdownCell::new("worker-1".to_string(), Some("done".to_string()));
        assert_eq!(
            plain(&with_reason),
            vec!["worker-1 shut down: done".to_string()]
        );
        assert_eq!(first_fg(&with_reason), Some(rata(Theme::dark().dim)));

        let bare = ShutdownCell::new("worker-2".to_string(), None);
        assert_eq!(plain(&bare), vec!["worker-2 shut down".to_string()]);
    }

    #[test]
    fn task_assignment_cell_renders_subject_plus_dim_description() {
        let full = TaskAssignmentCell::new("Fix bug".to_string(), Some("details here".to_string()));
        assert_eq!(
            plain(&full),
            vec!["Task: Fix bug".to_string(), "details here".to_string()]
        );
        let styled = full.display_lines(80, &Theme::dark(), RenderMode::default());
        assert_eq!(styled[0].spans[0].style.fg, None, "subject line is plain");
        assert_eq!(
            styled[1].spans[0].style.fg,
            Some(rata(Theme::dark().dim)),
            "description is dim"
        );

        let bare = TaskAssignmentCell::new("Fix bug".to_string(), None);
        assert_eq!(plain(&bare), vec!["Task: Fix bug".to_string()]);
    }

    #[test]
    fn agent_notification_cell_renders_dim_summary() {
        let cell = AgentNotificationCell::new("agent finished".to_string());
        assert_eq!(plain(&cell), vec!["agent finished".to_string()]);
        assert_eq!(first_fg(&cell), Some(rata(Theme::dark().dim)));
    }

    #[test]
    fn agent_notification_cell_with_empty_summary_is_documented_hidden() {
        // tui-core doc contract: an empty summary renders NOTHING — the one
        // intentionally non-visible state in the team cells.
        let cell = AgentNotificationCell::new(String::new());
        assert!(plain(&cell).is_empty());
        assert!(!cell.is_visible(80));
    }

    #[test]
    fn channel_message_cell_renders_server_user_and_content() {
        let named = ChannelMessageCell::new(
            "slack".to_string(),
            Some("alice".to_string()),
            "hi there".to_string(),
        );
        assert_eq!(plain(&named), vec!["[slack] alice: hi there".to_string()]);

        // Anonymous senders fall back to `system`.
        let anon = ChannelMessageCell::new("slack".to_string(), None, "ping".to_string());
        assert_eq!(plain(&anon), vec!["[slack] system: ping".to_string()]);
    }

    #[test]
    fn teammate_cell_renders_completed_task_with_agent_color() {
        let cell = UserTeammateCell::new(
            "worker-1".to_string(),
            Some("magenta".to_string()),
            UserTeammateKind::TaskCompleted {
                task_id: "42".to_string(),
                task_subject: Some("fix bug".to_string()),
            },
        );
        let lines = plain(&cell);
        assert_eq!(
            lines,
            vec!["@worker-1: ✓ Completed task #42 (fix bug)".to_string()]
        );
        let styled = cell.display_lines(80, &Theme::dark(), RenderMode::default());
        assert_eq!(
            styled[0].spans[0].style.fg,
            Some(rata(agent_color_from_name("magenta"))),
            "`@name` is tinted by the teammate's agent color"
        );
        assert_eq!(
            styled[0].spans[1].style.fg,
            Some(rata(Theme::dark().success)),
            "completion is success-colored"
        );
    }

    #[test]
    fn teammate_cell_note_shows_content_only_in_transcript_mode() {
        let note = |transcript: bool| {
            UserTeammateCell::new(
                "leader".to_string(),
                None,
                UserTeammateKind::Note {
                    summary: Some("looks good".to_string()),
                    content: Some("full body".to_string()),
                    is_transcript_mode: transcript,
                },
            )
        };
        assert_eq!(
            plain(&note(true)),
            vec![
                "@leader".to_string(),
                "looks good".to_string(),
                "full body".to_string()
            ]
        );
        assert_eq!(
            plain(&note(false)),
            vec!["@leader".to_string(), "looks good".to_string()],
            "content is hidden outside transcript mode"
        );
    }

    #[test]
    fn hook_progress_cell_renders_dim_event_count() {
        let cell = HookProgressCell::new("PreToolUse".to_string(), 2);
        assert_eq!(plain(&cell), vec!["hook: PreToolUse (×2)".to_string()]);
        assert_eq!(first_fg(&cell), Some(rata(Theme::dark().dim)));
    }

    #[test]
    fn subagent_activity_cell_renders_dim_indented_line() {
        let cell = SubagentActivityCell::new("Read(/etc/hosts)".to_string());
        // Dim, indented under the parent Task cell with a `⎿` continuation glyph.
        assert_eq!(
            plain(&cell),
            vec!["  \u{23BF}  Read(/etc/hosts)".to_string()]
        );
        assert_eq!(first_fg(&cell), Some(rata(Theme::dark().dim)));
    }

    #[test]
    fn plan_approval_cell_renders_every_kind() {
        let theme = Theme::dark();
        let request = PlanApprovalCell::new(PlanApprovalKind::Request {
            from: "lead".to_string(),
            plan_content: "step one".to_string(),
            plan_file_path: None,
        });
        assert_eq!(
            plain(&request),
            vec![
                "Plan approval requested by lead".to_string(),
                "step one".to_string()
            ]
        );
        assert_eq!(first_fg(&request), Some(rata(theme.warning)));

        let approved = PlanApprovalCell::new(PlanApprovalKind::Approved {
            name: "alice".to_string(),
        });
        assert_eq!(
            plain(&approved),
            vec!["✓ Plan approved by alice".to_string()]
        );
        assert_eq!(first_fg(&approved), Some(rata(theme.success)));

        let rejected = PlanApprovalCell::new(PlanApprovalKind::Rejected {
            name: "bob".to_string(),
            feedback: Some("needs tests".to_string()),
        });
        assert_eq!(
            plain(&rejected),
            vec![
                "✗ Plan rejected by bob".to_string(),
                "  needs tests".to_string()
            ]
        );
        assert_eq!(first_fg(&rejected), Some(rata(theme.error)));
        let styled = rejected.display_lines(80, &theme, RenderMode::default());
        assert_eq!(
            styled[1].spans[0].style.fg,
            Some(rata(theme.dim)),
            "feedback is dim"
        );

        // Rejection without feedback is the single error line.
        let bare = PlanApprovalCell::new(PlanApprovalKind::Rejected {
            name: "bob".to_string(),
            feedback: None,
        });
        assert_eq!(plain(&bare), vec!["✗ Plan rejected by bob".to_string()]);
    }
}

//! Render `RenderedMessage`s into styled lines for the ratatui scrollback.
//!
//! This is the `tui` side of message rendering: it consumes the neutral
//! `tui_core` message model + render primitives and produces `StyledLine`s that
//! [`crate::render::styled_line_to_ratatui`] turns into ratatui text. Every
//! variant renders through the per-variant history cells under
//! [`crate::history_cell`] — [`render_message`] delegates to those same
//! renderers, so this dispatcher stays the line-identity oracle across every
//! variant (the `ported_cells_render_line_identical_to_render_message` test
//! locks it against [`crate::history_cell::cell_for_message`]).

use tui_core::message::RenderedMessage;
use tui_core::render::StyledLine;
use tui_core::theme::Theme;

use crate::history_cell::attachments::{attachment_lines, resource_update_lines, user_image_lines};
use crate::history_cell::message::{
    advisor_lines, assistant_lines, redacted_thinking_lines, thinking_lines, user_bash_input_lines,
    user_command_lines, user_memory_input_lines, user_plan_lines, user_prompt_lines,
    user_text_lines,
};
use crate::history_cell::system::{
    compact_boundary_lines, rate_limit_lines, system_api_error_lines, system_text_lines,
    system_text_rich_lines,
};
use crate::history_cell::team::{
    agent_notification_lines, channel_message_lines, hook_progress_lines, plan_approval_lines,
    shutdown_lines, subagent_activity_lines, task_assignment_lines, teammate_lines,
};
use crate::history_cell::tool::{
    collapsed_read_search_lines, command_output_lines, group_tool_use_lines, tool_result_lines,
    tool_use_lines,
};
use crate::history_cell::DEFAULT_WIDTH;

/// Render one message into styled lines for the scrollback. Every variant has
/// an explicit arm (no wildcard): visible variants produce lines; the only
/// empty renders are documented empty-input cases (empty user text / empty
/// notification summaries). `verbose` expands the collapsible variants
/// (thinking body, tool-use JSON, grouped children).
#[must_use]
pub fn render_message(
    entry: &RenderedMessage,
    width: usize,
    theme: &Theme,
    verbose: bool,
) -> Vec<StyledLine> {
    let width = if width == 0 { DEFAULT_WIDTH } else { width };
    match entry {
        RenderedMessage::UserText { body, .. } => user_text_lines(body, theme),
        RenderedMessage::AssistantText { body, .. } => assistant_lines(body, width, theme),
        RenderedMessage::SystemText { body, is_error, .. } => {
            system_text_lines(body, *is_error, theme)
        }
        RenderedMessage::SystemTextRich { body, level } => {
            system_text_rich_lines(body, *level, theme)
        }
        RenderedMessage::AssistantToolUse { tool, input, .. } => {
            tool_use_lines(tool, input, theme, verbose)
        }
        RenderedMessage::UserToolResult {
            result,
            old_string,
            new_string,
            file_path,
            ..
        } => tool_result_lines(
            result,
            old_string.as_deref(),
            new_string.as_deref(),
            file_path.as_deref(),
            width,
            theme,
        ),
        RenderedMessage::UserBashInput { command } => user_bash_input_lines(command, theme),
        RenderedMessage::UserBashOutput { stdout, stderr }
        | RenderedMessage::UserLocalCommandOutput { stdout, stderr } => {
            command_output_lines(stdout, stderr, theme)
        }
        RenderedMessage::UserCommand {
            command,
            args,
            is_skill,
        } => user_command_lines(command, args, *is_skill),
        RenderedMessage::UserMemoryInput { input } => user_memory_input_lines(input, theme),
        RenderedMessage::UserPlan { plan_content } => user_plan_lines(plan_content),
        RenderedMessage::UserPrompt { text } => user_prompt_lines(text),
        RenderedMessage::AgentNotification { summary, .. } => {
            agent_notification_lines(summary, theme)
        }
        RenderedMessage::CompactBoundary { summary, .. } => {
            compact_boundary_lines(summary, verbose, theme)
        }
        RenderedMessage::AssistantThinking { thinking, .. } => {
            thinking_lines(thinking, width, verbose, theme)
        }
        RenderedMessage::SystemApiError {
            error,
            retry_attempt,
            max_retries,
            ..
        } => system_api_error_lines(error, *retry_attempt, *max_retries, theme),
        RenderedMessage::RateLimit { text, upsell } => {
            rate_limit_lines(text, upsell.as_deref(), theme)
        }
        RenderedMessage::Shutdown { from, reason, .. } => {
            shutdown_lines(from, reason.as_deref(), theme)
        }
        RenderedMessage::TaskAssignment {
            subject,
            description,
            ..
        } => task_assignment_lines(subject, description.as_deref(), theme),
        RenderedMessage::ChannelMessage {
            server,
            user,
            content,
        } => channel_message_lines(server, user.as_deref(), content),
        RenderedMessage::UserTeammate {
            display_name,
            color,
            kind,
        } => teammate_lines(display_name, color.as_deref(), kind, theme),
        RenderedMessage::HookProgress { event, count, .. } => {
            hook_progress_lines(event, *count, theme)
        }
        RenderedMessage::SubagentActivity { text } => subagent_activity_lines(text, theme),
        RenderedMessage::UserResourceUpdate { updates } => resource_update_lines(updates),
        RenderedMessage::UserImage {
            image_id, metadata, ..
        } => user_image_lines(*image_id, metadata.as_deref()),
        RenderedMessage::GroupedToolUse { tool, entries, .. } => {
            group_tool_use_lines(tool, entries, theme, verbose)
        }
        RenderedMessage::CollapsedReadSearch {
            search_count,
            read_count,
            list_count,
            repl_count,
            mcp_call_count,
            mcp_server_names,
            bash_count,
            is_active,
            latest_hint,
            entries,
            mem_write,
            ..
        } => collapsed_read_search_lines(
            *search_count,
            *read_count,
            *list_count,
            *repl_count,
            *mcp_call_count,
            mcp_server_names,
            *bash_count,
            *mem_write,
            *is_active,
            latest_hint.as_deref(),
            entries,
            theme,
            verbose,
        ),
        RenderedMessage::Attachment { attachment } => attachment_lines(attachment, theme),
        RenderedMessage::Advisor { kind, verbose } => advisor_lines(kind, *verbose, theme),
        RenderedMessage::PlanApproval { kind } => plan_approval_lines(kind, theme),
        RenderedMessage::AssistantRedactedThinking => redacted_thinking_lines(theme),
    }
}

#[cfg(test)]
mod tests {
    use tui_core::message::{AdvisorKind, PlanApprovalKind, SystemLevel};

    use super::*;
    use crate::history_cell::message::ASSISTANT_MARKER;

    #[test]
    fn user_text_gets_prompt_prefix() {
        let m = RenderedMessage::UserText {
            body: "hello".to_string(),
            timestamp: 0,
        };
        let lines = render_message(&m, 80, &Theme::dark(), false);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].plain_text(), "> hello");
    }

    #[test]
    fn assistant_text_gets_marker_and_renders_markdown() {
        let m = RenderedMessage::AssistantText {
            body: "**bold** text".to_string(),
            timestamp: 0,
        };
        let lines = render_message(&m, 80, &Theme::dark(), false);
        assert!(!lines.is_empty());
        assert!(lines[0].plain_text().starts_with(ASSISTANT_MARKER));
        assert!(lines[0].plain_text().contains("bold"));
    }

    #[test]
    fn system_text_is_colored() {
        let m = RenderedMessage::SystemText {
            body: "ready".to_string(),
            timestamp: 0,
            is_error: false,
        };
        let lines = render_message(&m, 80, &Theme::dark(), false);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].spans[0].style.fg, Theme::dark().dim);
    }

    #[test]
    fn redacted_thinking_renders_dim_marker() {
        let m = RenderedMessage::AssistantRedactedThinking;
        let lines = render_message(&m, 80, &Theme::dark(), false);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].plain_text().contains("Thinking"));
        assert_eq!(lines[0].spans[0].style.fg, Theme::dark().dim);
    }

    #[test]
    fn teammate_task_completed_and_note_render() {
        let done = RenderedMessage::UserTeammate {
            display_name: "worker-1".to_string(),
            color: Some("magenta".to_string()),
            kind: tui_core::message::UserTeammateKind::TaskCompleted {
                task_id: "42".to_string(),
                task_subject: Some("fix bug".to_string()),
            },
        };
        let lines = render_message(&done, 80, &Theme::dark(), false);
        assert!(lines[0].plain_text().contains("@worker-1"));
        assert!(lines[0].plain_text().contains("Completed task #42"));
        assert!(lines[0].plain_text().contains("fix bug"));
        // The `@name` span is tinted by the teammate's agent color.
        assert_eq!(
            lines[0].spans[0].style.fg,
            tui_core::render::agent_color_from_name("magenta")
        );

        let note = RenderedMessage::UserTeammate {
            display_name: "leader".to_string(),
            color: None,
            kind: tui_core::message::UserTeammateKind::Note {
                summary: Some("looks good".to_string()),
                content: Some("full body".to_string()),
                is_transcript_mode: true,
            },
        };
        let text = render_message(&note, 80, &Theme::dark(), false)
            .iter()
            .map(|l| l.plain_text())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("@leader"));
        assert!(text.contains("looks good"));
        assert!(text.contains("full body"));
    }

    #[test]
    fn advisor_variants_render() {
        let err = RenderedMessage::Advisor {
            kind: tui_core::message::AdvisorKind::Error {
                error_code: "503".to_string(),
            },
            verbose: false,
        };
        let lines = render_message(&err, 80, &Theme::dark(), false);
        assert!(lines[0].plain_text().contains("Advisor unavailable (503)"));
        assert_eq!(lines[0].spans[0].style.fg, Theme::dark().error);

        let result = RenderedMessage::Advisor {
            kind: tui_core::message::AdvisorKind::Result {
                text: "consider caching".to_string(),
            },
            verbose: true,
        };
        let text = render_message(&result, 80, &Theme::dark(), false)
            .iter()
            .map(|l| l.plain_text())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Advisor"));
        assert!(text.contains("consider caching"));
    }

    #[test]
    fn plan_approval_variants_render() {
        let approved = RenderedMessage::PlanApproval {
            kind: tui_core::message::PlanApprovalKind::Approved {
                name: "alice".to_string(),
            },
        };
        let lines = render_message(&approved, 80, &Theme::dark(), false);
        assert!(lines[0].plain_text().contains("Plan approved by alice"));
        assert_eq!(lines[0].spans[0].style.fg, Theme::dark().success);

        let rejected = RenderedMessage::PlanApproval {
            kind: tui_core::message::PlanApprovalKind::Rejected {
                name: "bob".to_string(),
                feedback: Some("needs tests".to_string()),
            },
        };
        let text = render_message(&rejected, 80, &Theme::dark(), false)
            .iter()
            .map(|l| l.plain_text())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Plan rejected by bob"));
        assert!(text.contains("needs tests"));
    }

    #[test]
    fn tool_use_renders_header_and_truncated_args() {
        let m = RenderedMessage::AssistantToolUse {
            id: protocol::ToolUseId::new(),
            tool: "Read".to_string(),
            input: serde_json::json!({"file_path": "src/lib.rs"}),
        };
        let lines = render_message(&m, 80, &Theme::dark(), false);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].plain_text().contains("Read"));
        assert!(lines[1].plain_text().contains("file_path"));
    }

    #[test]
    fn verbose_expands_thinking_tooluse_and_groups() {
        // Thinking: collapsed shows a hint; verbose shows the body.
        let think = RenderedMessage::AssistantThinking {
            thinking: "step one\nstep two".to_string(),
            expanded: false,
        };
        let collapsed = render_message(&think, 80, &Theme::dark(), false);
        assert_eq!(collapsed.len(), 1);
        assert!(collapsed[0].plain_text().contains("ctrl+o"));
        let expanded = render_message(&think, 80, &Theme::dark(), true);
        let text = expanded
            .iter()
            .map(|l| l.plain_text())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("step one") && text.contains("step two"));

        // Tool use: verbose pretty-prints the JSON over multiple lines.
        let tool = RenderedMessage::AssistantToolUse {
            id: protocol::ToolUseId::new(),
            tool: "Read".to_string(),
            input: serde_json::json!({"file_path": "src/lib.rs", "limit": 10}),
        };
        assert!(render_message(&tool, 80, &Theme::dark(), true).len() > 2);

        // Grouped tool use: collapsed is header-only; verbose lists children.
        let group = RenderedMessage::GroupedToolUse {
            tool: "Read".to_string(),
            group_id: protocol::ToolUseId::new(),
            entries: vec![
                (serde_json::json!({"f": "a"}), serde_json::json!("ok")),
                (serde_json::json!({"f": "b"}), serde_json::json!("ok")),
            ],
        };
        assert_eq!(render_message(&group, 80, &Theme::dark(), false).len(), 1);
        assert_eq!(render_message(&group, 80, &Theme::dark(), true).len(), 3);
    }

    #[test]
    fn tool_result_prefers_string_content() {
        let m = RenderedMessage::UserToolResult {
            id: protocol::ToolUseId::new(),
            tool: "Read".to_string(),
            result: serde_json::json!({"content": "hello world"}),
            old_string: None,
            new_string: None,
            file_path: None,
        };
        let lines = render_message(&m, 80, &Theme::dark(), false);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].plain_text().contains("hello world"));
    }

    #[test]
    fn bash_and_command_variants_render() {
        let bash = RenderedMessage::UserBashInput {
            command: "ls -la".to_string(),
        };
        assert!(render_message(&bash, 80, &Theme::dark(), false)[0]
            .plain_text()
            .contains("ls -la"));

        let cmd = RenderedMessage::UserCommand {
            command: "help".to_string(),
            args: String::new(),
            is_skill: false,
        };
        assert_eq!(
            render_message(&cmd, 80, &Theme::dark(), false)[0].plain_text(),
            "/help"
        );
    }

    #[test]
    fn system_error_text_is_error_colored() {
        let m = RenderedMessage::SystemText {
            body: "disk on fire".to_string(),
            timestamp: 0,
            is_error: true,
        };
        let lines = render_message(&m, 80, &Theme::dark(), false);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].plain_text(), "disk on fire");
        assert_eq!(lines[0].spans[0].style.fg, Theme::dark().error);
    }

    #[test]
    fn rate_limit_and_shutdown_render() {
        let rl = RenderedMessage::RateLimit {
            text: "Rate limited".to_string(),
            upsell: Some("Upgrade".to_string()),
        };
        let lines = render_message(&rl, 80, &Theme::dark(), false);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].plain_text().contains("Rate limited"));
        assert!(lines[1].plain_text().contains("Upgrade"));

        let sd = RenderedMessage::Shutdown {
            from: "worker-1".to_string(),
            reason: Some("done".to_string()),
            rejected: false,
        };
        assert!(render_message(&sd, 80, &Theme::dark(), false)[0]
            .plain_text()
            .contains("worker-1"));
    }

    // ===== Phase 0 exhaustive RenderedMessage coverage (codex-ui-structure plan) =====

    // What a fixture's render must satisfy.
    enum Expect {
        // Substrings that must appear in the rendered plain text.
        Visible(&'static [&'static str]),
        // Documented intentional non-visible render (the named reason).
        Hidden(&'static str),
    }

    // (diagnostic label, message, verbose flag, expectation)
    type Fixture = (&'static str, RenderedMessage, bool, Expect);

    // Exhaustive by construction: adding a `RenderedMessage` variant breaks this
    // compile, forcing a new coverage fixture.
    fn variant_name(m: &RenderedMessage) -> &'static str {
        match m {
            RenderedMessage::UserText { .. } => "UserText",
            RenderedMessage::AssistantText { .. } => "AssistantText",
            RenderedMessage::SystemText { .. } => "SystemText",
            RenderedMessage::AssistantToolUse { .. } => "AssistantToolUse",
            RenderedMessage::UserToolResult { .. } => "UserToolResult",
            RenderedMessage::AssistantThinking { .. } => "AssistantThinking",
            RenderedMessage::AssistantRedactedThinking => "AssistantRedactedThinking",
            RenderedMessage::CompactBoundary { .. } => "CompactBoundary",
            RenderedMessage::SystemTextRich { .. } => "SystemTextRich",
            RenderedMessage::SystemApiError { .. } => "SystemApiError",
            RenderedMessage::RateLimit { .. } => "RateLimit",
            RenderedMessage::Shutdown { .. } => "Shutdown",
            RenderedMessage::TaskAssignment { .. } => "TaskAssignment",
            RenderedMessage::AgentNotification { .. } => "AgentNotification",
            RenderedMessage::ChannelMessage { .. } => "ChannelMessage",
            RenderedMessage::UserTeammate { .. } => "UserTeammate",
            RenderedMessage::Advisor { .. } => "Advisor",
            RenderedMessage::HookProgress { .. } => "HookProgress",
            RenderedMessage::SubagentActivity { .. } => "SubagentActivity",
            RenderedMessage::PlanApproval { .. } => "PlanApproval",
            RenderedMessage::UserBashInput { .. } => "UserBashInput",
            RenderedMessage::UserBashOutput { .. } => "UserBashOutput",
            RenderedMessage::UserCommand { .. } => "UserCommand",
            RenderedMessage::UserLocalCommandOutput { .. } => "UserLocalCommandOutput",
            RenderedMessage::UserMemoryInput { .. } => "UserMemoryInput",
            RenderedMessage::UserPlan { .. } => "UserPlan",
            RenderedMessage::UserPrompt { .. } => "UserPrompt",
            RenderedMessage::UserResourceUpdate { .. } => "UserResourceUpdate",
            RenderedMessage::UserImage { .. } => "UserImage",
            RenderedMessage::Attachment { .. } => "Attachment",
            RenderedMessage::GroupedToolUse { .. } => "GroupedToolUse",
            RenderedMessage::CollapsedReadSearch { .. } => "CollapsedReadSearch",
        }
    }

    fn text_fixtures() -> Vec<Fixture> {
        vec![
            (
                "UserText",
                RenderedMessage::UserText {
                    body: "hello".to_string(),
                    timestamp: 0,
                },
                false,
                Expect::Visible(&["> hello"]),
            ),
            (
                "AssistantText",
                RenderedMessage::AssistantText {
                    body: "**bold** text".to_string(),
                    timestamp: 0,
                },
                false,
                Expect::Visible(&["bold"]),
            ),
            (
                "SystemText(error)",
                RenderedMessage::SystemText {
                    body: "disk on fire".to_string(),
                    timestamp: 0,
                    is_error: true,
                },
                false,
                Expect::Visible(&["disk on fire"]),
            ),
            (
                "SystemTextRich(info)",
                RenderedMessage::SystemTextRich {
                    body: "all good".to_string(),
                    level: SystemLevel::Info,
                },
                false,
                Expect::Visible(&["all good"]),
            ),
            (
                "SystemTextRich(warning)",
                RenderedMessage::SystemTextRich {
                    body: "careful".to_string(),
                    level: SystemLevel::Warning,
                },
                false,
                Expect::Visible(&["careful"]),
            ),
            (
                "SystemTextRich(error)",
                RenderedMessage::SystemTextRich {
                    body: "broken".to_string(),
                    level: SystemLevel::Error,
                },
                false,
                Expect::Visible(&["broken"]),
            ),
            (
                "SystemApiError",
                RenderedMessage::SystemApiError {
                    error: "boom".to_string(),
                    retry_attempt: 2,
                    retry_in_seconds: 7,
                    max_retries: 5,
                    truncated: false,
                },
                false,
                Expect::Visible(&["API error", "boom", "2/5"]),
            ),
            (
                "RateLimit",
                RenderedMessage::RateLimit {
                    text: "Rate limited".to_string(),
                    upsell: Some("Upgrade".to_string()),
                },
                false,
                Expect::Visible(&["Rate limited", "Upgrade"]),
            ),
            (
                "CompactBoundary",
                RenderedMessage::CompactBoundary {
                    messages_before: 40,
                    messages_after: 4,
                    summary: "Summary:\nkept context".to_string(),
                },
                false,
                // Counts intentionally omitted (claude-code parity); the marker
                // line itself must be visible.
                Expect::Visible(&["Conversation compacted"]),
            ),
        ]
    }

    fn thinking_and_tool_fixtures() -> Vec<Fixture> {
        vec![
            (
                "AssistantToolUse",
                RenderedMessage::AssistantToolUse {
                    id: protocol::ToolUseId::new(),
                    tool: "Read".to_string(),
                    input: serde_json::json!({"file_path": "src/lib.rs"}),
                },
                false,
                Expect::Visible(&["Read", "file_path"]),
            ),
            (
                "UserToolResult",
                RenderedMessage::UserToolResult {
                    id: protocol::ToolUseId::new(),
                    tool: "Read".to_string(),
                    result: serde_json::json!({"content": "hello world"}),
                    old_string: None,
                    new_string: None,
                    file_path: None,
                },
                false,
                Expect::Visible(&["hello world"]),
            ),
            (
                "AssistantThinking(collapsed)",
                RenderedMessage::AssistantThinking {
                    thinking: "deep thought".to_string(),
                    expanded: false,
                },
                false,
                Expect::Visible(&["ctrl+o"]),
            ),
            (
                "AssistantThinking(verbose)",
                RenderedMessage::AssistantThinking {
                    thinking: "deep thought".to_string(),
                    expanded: false,
                },
                true,
                Expect::Visible(&["deep thought"]),
            ),
            (
                "AssistantRedactedThinking",
                RenderedMessage::AssistantRedactedThinking,
                false,
                Expect::Visible(&["Thinking"]),
            ),
            (
                "GroupedToolUse",
                RenderedMessage::GroupedToolUse {
                    tool: "Read".to_string(),
                    group_id: protocol::ToolUseId::new(),
                    entries: vec![
                        (serde_json::json!({"f": "a"}), serde_json::json!("ok")),
                        (serde_json::json!({"f": "b"}), serde_json::json!("ok")),
                    ],
                },
                false,
                Expect::Visible(&["Read", "×2"]),
            ),
            (
                "CollapsedReadSearch",
                RenderedMessage::CollapsedReadSearch {
                    search_count: 1,
                    read_count: 1,
                    list_count: 0,
                    repl_count: 0,
                    mcp_call_count: 0,
                    mcp_server_names: Vec::new(),
                    bash_count: 0,
                    is_active: false,
                    group_id: protocol::ToolUseId::new(),
                    latest_hint: None,
                    entries: vec!["Read a.rs".to_string(), "Grep foo".to_string()],
                    mem_read: 0,
                    mem_search: 0,
                    mem_write: 0,
                },
                false,
                Expect::Visible(&["Searched for 1 pattern", "read 1 file"]),
            ),
        ]
    }

    fn team_fixtures() -> Vec<Fixture> {
        vec![
            (
                "Shutdown",
                RenderedMessage::Shutdown {
                    from: "worker-1".to_string(),
                    reason: Some("done".to_string()),
                    rejected: false,
                },
                false,
                Expect::Visible(&["worker-1 shut down", "done"]),
            ),
            (
                "TaskAssignment",
                RenderedMessage::TaskAssignment {
                    task_id: "9".to_string(),
                    assigned_by: "lead".to_string(),
                    subject: "Fix bug".to_string(),
                    description: Some("details here".to_string()),
                },
                false,
                Expect::Visible(&["Task: Fix bug", "details here"]),
            ),
            (
                "AgentNotification",
                RenderedMessage::AgentNotification {
                    summary: "agent finished".to_string(),
                    status: Some("completed".to_string()),
                },
                false,
                Expect::Visible(&["agent finished"]),
            ),
            (
                "AgentNotification(empty summary)",
                RenderedMessage::AgentNotification {
                    summary: String::new(),
                    status: None,
                },
                false,
                Expect::Hidden("tui-core doc: empty summary renders nothing"),
            ),
            (
                "ChannelMessage",
                RenderedMessage::ChannelMessage {
                    server: "slack".to_string(),
                    user: Some("alice".to_string()),
                    content: "hi there".to_string(),
                },
                false,
                Expect::Visible(&["slack", "alice", "hi there"]),
            ),
            (
                "UserTeammate(TaskCompleted)",
                RenderedMessage::UserTeammate {
                    display_name: "worker-1".to_string(),
                    color: Some("magenta".to_string()),
                    kind: tui_core::message::UserTeammateKind::TaskCompleted {
                        task_id: "42".to_string(),
                        task_subject: None,
                    },
                },
                false,
                Expect::Visible(&["@worker-1", "Completed task #42"]),
            ),
            (
                "UserTeammate(Note)",
                RenderedMessage::UserTeammate {
                    display_name: "leader".to_string(),
                    color: None,
                    kind: tui_core::message::UserTeammateKind::Note {
                        summary: Some("looks good".to_string()),
                        content: None,
                        is_transcript_mode: false,
                    },
                },
                false,
                Expect::Visible(&["@leader", "looks good"]),
            ),
        ]
    }

    fn advisor_and_plan_fixtures() -> Vec<Fixture> {
        vec![
            (
                "Advisor(ServerToolUse)",
                RenderedMessage::Advisor {
                    kind: AdvisorKind::ServerToolUse {
                        model: Some("gpt-5".to_string()),
                        input: Some("review diff".to_string()),
                    },
                    verbose: false,
                },
                false,
                Expect::Visible(&["Advising", "gpt-5", "review diff"]),
            ),
            (
                "Advisor(Result)",
                RenderedMessage::Advisor {
                    kind: AdvisorKind::Result {
                        text: "consider caching".to_string(),
                    },
                    verbose: false,
                },
                false,
                Expect::Visible(&["Advisor", "consider caching"]),
            ),
            (
                "Advisor(RedactedResult)",
                RenderedMessage::Advisor {
                    kind: AdvisorKind::RedactedResult,
                    verbose: false,
                },
                false,
                Expect::Visible(&["Advisor"]),
            ),
            (
                "Advisor(Error)",
                RenderedMessage::Advisor {
                    kind: AdvisorKind::Error {
                        error_code: "503".to_string(),
                    },
                    verbose: false,
                },
                false,
                Expect::Visible(&["Advisor unavailable (503)"]),
            ),
            (
                "HookProgress",
                RenderedMessage::HookProgress {
                    event: "PreToolUse".to_string(),
                    count: 2,
                    transcript_summary: true,
                },
                false,
                Expect::Visible(&["PreToolUse", "×2"]),
            ),
            (
                "PlanApproval(Request)",
                RenderedMessage::PlanApproval {
                    kind: PlanApprovalKind::Request {
                        from: "lead".to_string(),
                        plan_content: "step one".to_string(),
                        plan_file_path: None,
                    },
                },
                false,
                Expect::Visible(&["Plan approval requested by lead", "step one"]),
            ),
            (
                "PlanApproval(Approved)",
                RenderedMessage::PlanApproval {
                    kind: PlanApprovalKind::Approved {
                        name: "alice".to_string(),
                    },
                },
                false,
                Expect::Visible(&["Plan approved by alice"]),
            ),
            (
                "PlanApproval(Rejected)",
                RenderedMessage::PlanApproval {
                    kind: PlanApprovalKind::Rejected {
                        name: "bob".to_string(),
                        feedback: Some("needs tests".to_string()),
                    },
                },
                false,
                Expect::Visible(&["Plan rejected by bob", "needs tests"]),
            ),
        ]
    }

    fn command_fixtures() -> Vec<Fixture> {
        vec![
            (
                "UserBashInput",
                RenderedMessage::UserBashInput {
                    command: "ls -la".to_string(),
                },
                false,
                Expect::Visible(&["! ls -la"]),
            ),
            (
                "UserBashOutput",
                RenderedMessage::UserBashOutput {
                    stdout: "out line".to_string(),
                    stderr: "err line".to_string(),
                },
                false,
                Expect::Visible(&["out line", "err line"]),
            ),
            (
                "UserCommand(slash)",
                RenderedMessage::UserCommand {
                    command: "model".to_string(),
                    args: "opus".to_string(),
                    is_skill: false,
                },
                false,
                Expect::Visible(&["/model opus"]),
            ),
            (
                "UserCommand(skill)",
                RenderedMessage::UserCommand {
                    command: "deploy".to_string(),
                    args: String::new(),
                    is_skill: true,
                },
                false,
                // tui-core doc: is_skill renders the `Skill(name)` form.
                Expect::Visible(&["Skill(deploy)"]),
            ),
            (
                "UserLocalCommandOutput",
                RenderedMessage::UserLocalCommandOutput {
                    stdout: "local out".to_string(),
                    stderr: "local err".to_string(),
                },
                false,
                Expect::Visible(&["local out", "local err"]),
            ),
            (
                "UserMemoryInput",
                RenderedMessage::UserMemoryInput {
                    input: "remember this".to_string(),
                },
                false,
                Expect::Visible(&["# remember this"]),
            ),
            (
                "UserPlan",
                RenderedMessage::UserPlan {
                    plan_content: "step 1".to_string(),
                },
                false,
                Expect::Visible(&["step 1"]),
            ),
            (
                "UserPrompt",
                RenderedMessage::UserPrompt {
                    text: "echoed prompt".to_string(),
                },
                false,
                Expect::Visible(&["echoed prompt"]),
            ),
        ]
    }

    fn misc_user_fixtures() -> Vec<Fixture> {
        vec![
            (
                "UserResourceUpdate",
                RenderedMessage::UserResourceUpdate {
                    updates: vec![(
                        "grafana".to_string(),
                        "logs".to_string(),
                        Some("changed".to_string()),
                    )],
                },
                false,
                Expect::Visible(&["grafana", "logs"]),
            ),
            (
                "UserImage",
                RenderedMessage::UserImage {
                    image_id: Some(3),
                    metadata: Some("640x480".to_string()),
                    source_path: None,
                },
                false,
                Expect::Visible(&["[Image #3]", "640x480"]),
            ),
        ]
    }

    fn attachment_fixtures() -> Vec<Fixture> {
        use tui_core::message::Attachment;
        let att = |label: &'static str, attachment: Attachment, expect: Expect| -> Fixture {
            (
                label,
                RenderedMessage::Attachment { attachment },
                false,
                expect,
            )
        };
        vec![
            att(
                "Attachment(Directory)",
                Attachment::Directory {
                    display_path: "src".to_string(),
                },
                Expect::Visible(&["src"]),
            ),
            att(
                "Attachment(File)",
                Attachment::File {
                    display_path: "src/lib.rs".to_string(),
                    num_lines: 42,
                    truncated: false,
                },
                Expect::Visible(&["src/lib.rs"]),
            ),
            att(
                "Attachment(CompactFileReference)",
                Attachment::CompactFileReference {
                    display_path: "notes.md".to_string(),
                },
                Expect::Visible(&["notes.md"]),
            ),
            att(
                "Attachment(PdfReference)",
                Attachment::PdfReference {
                    display_path: "spec.pdf".to_string(),
                    page_count: 3,
                },
                Expect::Visible(&["spec.pdf"]),
            ),
            att(
                "Attachment(SelectedLines)",
                Attachment::SelectedLines {
                    count: 4,
                    display_path: "main.rs".to_string(),
                    ide_name: "VS Code".to_string(),
                },
                Expect::Visible(&["main.rs"]),
            ),
            att(
                "Attachment(NestedMemory)",
                Attachment::NestedMemory {
                    display_path: "MEMORY.md".to_string(),
                },
                Expect::Visible(&["MEMORY.md"]),
            ),
            att(
                "Attachment(McpResource)",
                Attachment::McpResource {
                    name: "logs".to_string(),
                    server: "grafana".to_string(),
                },
                Expect::Visible(&["logs"]),
            ),
            att(
                "Attachment(PlanFileReference)",
                Attachment::PlanFileReference {
                    plan_file_path: "plan.md".to_string(),
                },
                Expect::Visible(&["plan.md"]),
            ),
            att(
                "Attachment(InvokedSkills)",
                Attachment::InvokedSkills {
                    skill_names: vec!["deploy".to_string()],
                },
                Expect::Visible(&["deploy"]),
            ),
        ]
    }

    fn coverage_fixtures() -> Vec<Fixture> {
        let mut fixtures = text_fixtures();
        fixtures.extend(thinking_and_tool_fixtures());
        fixtures.extend(team_fixtures());
        fixtures.extend(advisor_and_plan_fixtures());
        fixtures.extend(command_fixtures());
        fixtures.extend(misc_user_fixtures());
        fixtures.extend(attachment_fixtures());
        fixtures
    }

    // NOT ignored: locks the enumeration itself. `variant_name` is an
    // exhaustive match, so a new `RenderedMessage` variant fails compilation
    // here until a coverage fixture exists for it.
    #[test]
    fn coverage_fixture_list_spans_every_variant() {
        let names: std::collections::BTreeSet<&str> = coverage_fixtures()
            .iter()
            .map(|(_, m, _, _)| variant_name(m))
            .collect();
        assert_eq!(
            names.len(),
            31,
            "one fixture per top-level variant at minimum: {names:?}"
        );
    }

    // Failed from Phase 0 (2026-07-02) until the message-cells phases ported
    // every variant: `UserCommand { is_skill: true }` now renders
    // `Skill(name)`, and the `attachment_lines` sub-wildcard that flattened
    // PdfReference/SelectedLines/McpResource/PlanFileReference/InvokedSkills
    // to "[attachment]" was replaced by the explicit per-kind renderer in
    // `history_cell::attachments`.
    #[test]
    fn every_rendered_message_variant_renders_visibly_or_is_documented_hidden() {
        let theme = Theme::dark();
        let mut failures = Vec::new();
        for (name, msg, verbose, expect) in coverage_fixtures() {
            let text = render_message(&msg, 80, &theme, verbose)
                .iter()
                .map(tui_core::render::StyledLine::plain_text)
                .collect::<Vec<_>>()
                .join("\n");
            match expect {
                Expect::Visible(substrings) => {
                    if text.trim().is_empty() {
                        failures.push(format!(
                            "{name}: renders NOTHING (hidden without a documented reason)"
                        ));
                        continue;
                    }
                    for s in substrings {
                        if !text.contains(s) {
                            failures.push(format!("{name}: missing {s:?} in output {text:?}"));
                        }
                    }
                }
                Expect::Hidden(reason) => {
                    if !text.trim().is_empty() {
                        failures.push(format!(
                            "{name}: expected hidden ({reason}) but renders {text:?}"
                        ));
                    }
                }
            }
        }
        assert!(failures.is_empty(), "\n{}", failures.join("\n"));
    }

    // ===== Message-cells phase: the split renderers stay line-identical =====

    /// Every ported (per-variant-cell) fixture must render EXACTLY the same
    /// ratatui lines through its concrete cell as through the legacy
    /// `render_message` dispatcher — the line-identity lock for the split.
    #[test]
    fn ported_cells_render_line_identical_to_render_message() {
        use crate::history_cell::{cell_for_message, RenderMode};
        let theme = Theme::dark();
        for (name, msg, _, _) in coverage_fixtures() {
            for verbose in [false, true] {
                let expected: Vec<ratatui::text::Line<'static>> =
                    render_message(&msg, 80, &theme, verbose)
                        .iter()
                        .map(crate::render::styled_line_to_ratatui)
                        .collect();
                let cell = cell_for_message(msg.clone());
                let got = cell.display_lines(
                    80,
                    &theme,
                    RenderMode {
                        raw: false,
                        verbose,
                    },
                );
                assert_eq!(got, expected, "{name} diverged (verbose={verbose})");
            }
        }
    }
}

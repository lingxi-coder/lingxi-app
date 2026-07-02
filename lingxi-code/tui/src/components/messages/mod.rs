//! Message renderers — one component per `RenderedMessage` variant.
//!
//! M6-02 ships two renderers (`UserTextMessage`, `AssistantTextMessage`).
//! M6-04 adds `AssistantToolUseMessage` + `UserToolResultMessage` plus the
//! `render_entry_to_string` string-form dispatcher used by snapshot tests
//! and the iocraft `Scrollback` component.

pub mod advisor;
pub mod assistant_text;
pub mod assistant_tool_use;
pub mod attachment;
pub mod bash_input;
pub mod bash_output;
pub mod collapsed_read_search;
pub mod command;
pub mod compact_boundary;
pub mod grouped_tool_use;
pub mod hook_progress;
pub mod image;
pub mod local_command_output;
pub mod memory_input;
pub mod plan;
pub mod plan_approval;
pub mod prompt;
pub mod rate_limit;
pub mod redacted_thinking;
pub mod resource_update;
pub mod shutdown;
pub mod system_api_error;
pub mod system_text;
pub mod task_assignment;
pub mod team_mem_saved;
pub mod text_guard;
pub mod thinking;
pub mod user_agent_notification;
pub mod user_channel;
pub mod user_teammate;
pub mod user_text;
pub mod user_tool_result;

use crate::render::StyleColor;
use crate::state::RenderedMessage;
use crate::theme::Theme;
use assistant_tool_use::{render_assistant_tool_use_to_string, AssistantToolUseProps};
use user_tool_result::{render_user_tool_result_to_string, UserToolResultProps};

pub use tui_core::message_render::{
    colored_terminal_lines, encode_terminal_line_ansi, plain_terminal_lines,
    styled_lines_to_terminal_lines, TerminalLine, TerminalSpan,
};

/// String-form dispatcher used by snapshot tests. The iocraft-component
/// dispatcher (returns `AnyElement`) lives in `components::scrollback`;
/// both share the same routing rules.
///
/// `focused` is `true` when this entry's id matches
/// `AppState.focused_tool_id`. `expanded` is `AppState.expanded.get(&id)`
/// (default `false`).
#[must_use]
#[allow(clippy::too_many_lines)] // one arm per RenderedMessage variant (28 variants)
pub fn render_entry_to_string(entry: &RenderedMessage, focused: bool, expanded: bool) -> String {
    render_entry_to_string_at_width(entry, focused, expanded, 0)
}

#[must_use]
#[allow(clippy::too_many_lines)]
pub fn render_entry_to_string_at_width(
    entry: &RenderedMessage,
    focused: bool,
    expanded: bool,
    width: usize,
) -> String {
    match entry {
        // (RRS-08) The interrupt marker renders the InterruptedByUser line.
        RenderedMessage::UserText { body, .. } if body == user_tool_result::INTERRUPT_MESSAGE => {
            format!(
                "{}{}",
                user_tool_result::MARKER,
                user_tool_result::INTERRUPTED_LINE
            )
        }
        // §A4 empty-message guard: a body that is only stripped prompt-XML
        // tags (or `(no content)`) is suppressed entirely — the component
        // returns an empty View, so the string oracle returns "" (no `"> "`
        // prefix row), matching claude-code's `return null`.
        RenderedMessage::UserText { body, .. } if text_guard::is_empty_message_text(body) => {
            String::new()
        }
        RenderedMessage::UserText { body, .. } => format!("> {body}"),
        // Markdown-rendered body, marker-prefixed (oracle == the component's
        // markdown-flattened layout).
        RenderedMessage::AssistantText { body, .. } => {
            // (A2) Default markdown width (0 → 80); this string dispatcher has
            // no terminal width, matching the prior behavior.
            assistant_text::render_assistant_text_to_string(body, width)
        }
        RenderedMessage::SystemText { body, .. } => body.clone(),
        RenderedMessage::AssistantToolUse { id, tool, input } => {
            render_assistant_tool_use_to_string(AssistantToolUseProps {
                id: id.clone(),
                tool: tool.clone(),
                input: input.clone(),
                expanded,
                focused,
                cwd: std::env::current_dir().unwrap_or_default(),
                resolution: None,
            })
        }
        RenderedMessage::UserToolResult {
            id,
            tool,
            result,
            old_string,
            new_string,
            file_path,
        } => render_user_tool_result_to_string(UserToolResultProps {
            id: id.clone(),
            tool: tool.clone(),
            result: result.clone(),
            expanded,
            focused,
            // M7-02: diff inputs populated by `streaming::apply_event` from the
            // paired `ToolUseStart`; thread them so the diff branch can fire.
            old_string: old_string.clone(),
            new_string: new_string.clone(),
            file_path: file_path.clone(),
            // (M7-15) String oracle drops color; the syntect theme is
            // immaterial here. The live component path threads the real theme.
            theme_name: crate::theme::ThemeName::default(),
            // (diff-03) No terminal width in the string oracle — 0 disables
            // padding, matching the prior behavior.
            width,
        }),
        RenderedMessage::AssistantThinking { thinking, expanded } => {
            thinking::render_thinking_to_string(thinking::ThinkingProps {
                thinking: thinking.clone(),
                expanded: *expanded,
            })
        }
        RenderedMessage::AssistantRedactedThinking => {
            redacted_thinking::render_redacted_thinking_to_string()
        }
        RenderedMessage::CompactBoundary { .. } => {
            compact_boundary::render_compact_boundary_to_string()
        }
        RenderedMessage::SystemTextRich { body, level } => {
            system_text::render_system_text_to_string(system_text::SystemTextProps {
                body: body.clone(),
                level: *level,
                ..Default::default()
            })
        }
        RenderedMessage::SystemApiError {
            error,
            retry_attempt,
            retry_in_seconds,
            max_retries,
            truncated,
        } => system_api_error::render_system_api_error_to_string(
            system_api_error::SystemApiErrorProps {
                error: error.clone(),
                retry_attempt: *retry_attempt,
                retry_in_seconds: *retry_in_seconds,
                max_retries: *max_retries,
                truncated: *truncated,
            },
        ),
        RenderedMessage::RateLimit { text, upsell } => {
            rate_limit::render_rate_limit_to_string(rate_limit::RateLimitProps {
                text: text.clone(),
                upsell: upsell.clone(),
            })
        }
        RenderedMessage::Shutdown {
            from,
            reason,
            rejected,
        } => shutdown::render_shutdown_to_string(shutdown::ShutdownProps {
            from: from.clone(),
            reason: reason.clone(),
            rejected: *rejected,
            ..Default::default()
        }),
        RenderedMessage::TaskAssignment {
            task_id,
            assigned_by,
            subject,
            description,
        } => task_assignment::render_task_assignment_to_string(
            task_assignment::TaskAssignmentProps {
                task_id: task_id.clone(),
                assigned_by: assigned_by.clone(),
                subject: subject.clone(),
                description: description.clone(),
                theme: crate::theme::Theme::dark(),
            },
        ),
        RenderedMessage::AgentNotification { summary, status } => {
            user_agent_notification::render_user_agent_notification_to_string(
                user_agent_notification::UserAgentNotificationProps {
                    summary: summary.clone(),
                    status: status.clone(),
                    theme: crate::theme::Theme::dark(),
                },
            )
        }
        RenderedMessage::ChannelMessage {
            server,
            user,
            content,
        } => user_channel::render_user_channel_to_string(user_channel::UserChannelProps {
            server: server.clone(),
            user: user.clone(),
            content: content.clone(),
            theme: crate::theme::Theme::dark(),
        }),
        RenderedMessage::UserTeammate {
            display_name,
            color,
            kind,
        } => user_teammate::render_user_teammate_to_string(user_teammate::UserTeammateProps {
            display_name: display_name.clone(),
            color: color.clone(),
            kind: kind.clone(),
            theme: crate::theme::Theme::dark(),
        }),
        RenderedMessage::Advisor { kind, verbose } => {
            advisor::render_advisor_to_string(advisor::AdvisorProps {
                kind: kind.clone(),
                verbose: *verbose,
                ..Default::default()
            })
        }
        RenderedMessage::HookProgress {
            event,
            count,
            transcript_summary,
        } => hook_progress::render_hook_progress_to_string(hook_progress::HookProgressProps {
            event: event.clone(),
            count: *count,
            transcript_summary: *transcript_summary,
            ..Default::default()
        }),
        RenderedMessage::PlanApproval { kind } => {
            plan_approval::render_plan_approval_to_string(plan_approval::PlanApprovalProps {
                kind: kind.clone(),
                ..Default::default()
            })
        }
        // ---- (M7-05) batch-2 user renderers ----------------------------
        RenderedMessage::UserBashInput { command } => {
            bash_input::render_bash_input_to_string(command)
        }
        RenderedMessage::UserBashOutput { stdout, stderr } => {
            // String form strips ANSI: the parser drops escape codes, so join
            // the span texts.
            bash_output::render_bash_output_spans(stdout, stderr)
                .into_iter()
                .map(|s| s.text)
                .collect::<String>()
        }
        RenderedMessage::UserCommand {
            command,
            args,
            is_skill,
        } => command::render_command_to_string(command, args, *is_skill),
        RenderedMessage::UserLocalCommandOutput { stdout, stderr } => {
            local_command_output::render_local_output_to_string(stdout, stderr)
        }
        RenderedMessage::UserMemoryInput { input } => memory_input::render_memory_to_string(input),
        RenderedMessage::UserPlan { plan_content } => plan::render_plan_to_string(plan_content),
        RenderedMessage::UserPrompt { text } => prompt::render_prompt_to_string(text),
        RenderedMessage::UserResourceUpdate { updates } => {
            let parsed: Vec<resource_update::ResourceUpdate> = updates
                .iter()
                .map(|(s, t, r)| resource_update::ResourceUpdate {
                    server: s.clone(),
                    target: t.clone(),
                    reason: r.clone(),
                })
                .collect();
            resource_update::render_resource_update_to_string(&parsed)
        }
        RenderedMessage::UserImage {
            image_id, metadata, ..
        } => image::render_image_label(*image_id, metadata.as_deref()),
        RenderedMessage::Attachment { attachment } => {
            attachment::render_attachment_to_string(attachment)
        }
        RenderedMessage::GroupedToolUse { tool, entries, .. } => {
            // This dispatcher already receives `expanded: bool` as a parameter
            // (keyed by the group's id upstream) — reuse it.
            grouped_tool_use::render_grouped_to_string(tool, entries, expanded)
        }
        RenderedMessage::CollapsedReadSearch {
            search_count,
            read_count,
            list_count,
            is_active,
            entries,
            mem_read,
            mem_search,
            mem_write,
            ..
        } => {
            let counts = collapsed_read_search::CollapsedCounts {
                search: *search_count,
                read: *read_count,
                list: *list_count,
                is_active: *is_active,
                mem_read: *mem_read,
                mem_search: *mem_search,
                mem_write: *mem_write,
            };
            collapsed_read_search::render_collapsed_to_string(&counts, entries, expanded)
        }
    }
}

/// Styled-line renderer for messages that are safe to commit into native
/// terminal scrollback. Interactive message variants return `None` and must
/// stay in the live iocraft tree so focus, expansion, and theme changes can
/// redraw them.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn render_entry_to_terminal_lines(
    entry: &RenderedMessage,
    width: usize,
    theme: Theme,
) -> Option<Vec<TerminalLine>> {
    let lines = match entry {
        RenderedMessage::UserText { body, .. } if text_guard::is_empty_message_text(body) => {
            Vec::new()
        }
        RenderedMessage::UserText { body, .. } => plain_terminal_lines(&format!("> {body}")),
        RenderedMessage::AssistantText { body, .. } => styled_lines_to_terminal_lines(
            assistant_text::render_assistant_text_to_styled_lines(body, width),
        ),
        RenderedMessage::SystemText { body, is_error, .. } => {
            let color = if *is_error { theme.error } else { theme.dim };
            colored_terminal_lines(body, color)
        }
        RenderedMessage::CompactBoundary { .. } => colored_terminal_lines(
            &compact_boundary::render_compact_boundary_to_string(),
            theme.dim,
        ),
        RenderedMessage::SystemTextRich { body, level } => {
            let text = system_text::render_system_text_to_string(system_text::SystemTextProps {
                body: body.clone(),
                level: *level,
                theme,
            });
            let color = match level {
                crate::state::SystemLevel::Info => theme.dim,
                crate::state::SystemLevel::Warning => theme.warning,
                crate::state::SystemLevel::Error => theme.error,
            };
            colored_terminal_lines(&text, color)
        }
        RenderedMessage::RateLimit { text, upsell } => {
            let mut lines = Vec::new();
            lines.push(vec![TerminalSpan::colored(
                format!("{}{}", user_tool_result::MARKER, text),
                theme.error,
            )]);
            if let Some(upsell) = upsell {
                lines.push(vec![TerminalSpan::colored(
                    format!("{}{}", user_tool_result::INDENT, upsell),
                    theme.dim,
                )]);
            }
            lines
        }
        RenderedMessage::Shutdown {
            from,
            reason,
            rejected,
        } => plain_terminal_lines(&shutdown::render_shutdown_to_string(
            shutdown::ShutdownProps {
                from: from.clone(),
                reason: reason.clone(),
                rejected: *rejected,
                theme,
            },
        )),
        RenderedMessage::TaskAssignment {
            task_id,
            assigned_by,
            subject,
            description,
        } => plain_terminal_lines(&task_assignment::render_task_assignment_to_string(
            task_assignment::TaskAssignmentProps {
                task_id: task_id.clone(),
                assigned_by: assigned_by.clone(),
                subject: subject.clone(),
                description: description.clone(),
                theme,
            },
        )),
        RenderedMessage::AgentNotification { summary, status } => plain_terminal_lines(
            &user_agent_notification::render_user_agent_notification_to_string(
                user_agent_notification::UserAgentNotificationProps {
                    summary: summary.clone(),
                    status: status.clone(),
                    theme,
                },
            ),
        ),
        RenderedMessage::ChannelMessage {
            server,
            user,
            content,
        } => plain_terminal_lines(&user_channel::render_user_channel_to_string(
            user_channel::UserChannelProps {
                server: server.clone(),
                user: user.clone(),
                content: content.clone(),
                theme,
            },
        )),
        RenderedMessage::UserTeammate {
            display_name,
            color,
            kind,
        } => plain_terminal_lines(&user_teammate::render_user_teammate_to_string(
            user_teammate::UserTeammateProps {
                display_name: display_name.clone(),
                color: color.clone(),
                kind: kind.clone(),
                theme,
            },
        )),
        RenderedMessage::HookProgress {
            event,
            count,
            transcript_summary,
        } => plain_terminal_lines(&hook_progress::render_hook_progress_to_string(
            hook_progress::HookProgressProps {
                event: event.clone(),
                count: *count,
                transcript_summary: *transcript_summary,
                theme,
            },
        )),
        RenderedMessage::UserCommand {
            command,
            args,
            is_skill,
        } => plain_terminal_lines(&command::render_command_to_string(command, args, *is_skill)),
        RenderedMessage::UserBashInput { command } => {
            plain_terminal_lines(&bash_input::render_bash_input_to_string(command))
        }
        RenderedMessage::UserBashOutput { stdout, stderr } => {
            let spans = bash_output::render_bash_output_spans(stdout, stderr);
            let rows = crate::render::split_spans_into_line_rows(spans);
            rows.into_iter()
                .map(|row| {
                    row.into_iter()
                        .map(TerminalSpan::from_styled_span)
                        .collect()
                })
                .collect()
        }
        RenderedMessage::UserLocalCommandOutput { stdout, stderr } => plain_terminal_lines(
            &local_command_output::render_local_output_to_string(stdout, stderr),
        ),
        RenderedMessage::UserMemoryInput { input } => {
            plain_terminal_lines(&memory_input::render_memory_to_string(input))
        }
        RenderedMessage::UserPlan { plan_content } => {
            plain_terminal_lines(&plan::render_plan_to_string(plan_content))
        }
        RenderedMessage::UserPrompt { text } => {
            plain_terminal_lines(&prompt::render_prompt_to_string(text))
        }
        RenderedMessage::UserResourceUpdate { updates } => {
            let parsed: Vec<resource_update::ResourceUpdate> = updates
                .iter()
                .map(|(s, t, r)| resource_update::ResourceUpdate {
                    server: s.clone(),
                    target: t.clone(),
                    reason: r.clone(),
                })
                .collect();
            plain_terminal_lines(&resource_update::render_resource_update_to_string(&parsed))
        }
        RenderedMessage::UserImage {
            image_id, metadata, ..
        } => plain_terminal_lines(&image::render_image_label(*image_id, metadata.as_deref())),
        RenderedMessage::Attachment { attachment } => {
            plain_terminal_lines(&attachment::render_attachment_to_string(attachment))
        }
        RenderedMessage::AssistantToolUse { .. }
        | RenderedMessage::UserToolResult { .. }
        | RenderedMessage::AssistantThinking { .. }
        | RenderedMessage::AssistantRedactedThinking
        | RenderedMessage::SystemApiError { .. }
        | RenderedMessage::Advisor { .. }
        | RenderedMessage::PlanApproval { .. }
        | RenderedMessage::GroupedToolUse { .. }
        | RenderedMessage::CollapsedReadSearch { .. } => {
            // Interactive/expandable messages (tool calls + results) commit to
            // native scrollback in their COLLAPSED, line/byte-capped string form
            // (the same shape shown live, `expanded = false`). Keeping them in
            // the live pane let a single huge tool result (e.g. raw JSON) exceed
            // the terminal height and push the input view off-screen.
            plain_terminal_lines(&render_entry_to_string_at_width(entry, false, false, width))
        }
    };

    Some(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain_text(lines: &[TerminalLine]) -> String {
        lines
            .iter()
            .map(|line| {
                line.iter()
                    .map(|span| span.text.as_str())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn terminal_lines_preserve_assistant_markdown_style() {
        let entry = RenderedMessage::AssistantText {
            body: "- **天气**：小雨".to_string(),
            timestamp: 0,
        };
        let lines = render_entry_to_terminal_lines(&entry, 80, Theme::dark()).unwrap();

        assert_eq!(
            plain_text(&lines),
            format!("{}- 天气：小雨", assistant_text::MARKER)
        );
        let encoded = encode_terminal_line_ansi(&lines[0]);
        assert!(
            encoded.contains("\u{1b}[1m天气\u{1b}[0m"),
            "encoded line should preserve bold markdown span: {encoded:?}"
        );
        assert!(
            !encoded.contains('\n'),
            "terminal line encoder must not embed raw newlines: {encoded:?}"
        );
    }

    #[test]
    fn terminal_lines_commit_interactive_tool_messages_collapsed() {
        let entry = RenderedMessage::AssistantToolUse {
            id: protocol::ToolUseId::new(),
            tool: "Read".to_string(),
            input: serde_json::json!({"file_path": "src/lib.rs"}),
        };

        // Tool messages now commit to native scrollback (collapsed form) rather
        // than staying live, so a huge result can't overflow the live pane.
        let lines = render_entry_to_terminal_lines(&entry, 80, Theme::dark());
        assert!(lines.is_some(), "tool messages must render terminal lines");
    }

    #[test]
    fn terminal_lines_rate_limit_uses_active_theme() {
        let mut theme = Theme::dark();
        theme.error = StyleColor::Rgb(1, 2, 3);
        theme.dim = StyleColor::Rgb(4, 5, 6);
        let entry = RenderedMessage::RateLimit {
            text: "limit".to_string(),
            upsell: Some("upgrade".to_string()),
        };

        let lines = render_entry_to_terminal_lines(&entry, 80, theme).unwrap();

        assert_eq!(lines[0][0].fg, Some(theme.error));
        assert_eq!(lines[1][0].fg, Some(theme.dim));
    }
}

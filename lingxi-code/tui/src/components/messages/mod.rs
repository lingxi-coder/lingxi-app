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
pub mod thinking;
pub mod user_agent_notification;
pub mod user_channel;
pub mod user_text;
pub mod user_tool_result;

use crate::state::RenderedMessage;
use assistant_tool_use::{render_assistant_tool_use_to_string, AssistantToolUseProps};
use user_tool_result::{render_user_tool_result_to_string, UserToolResultProps};

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
    match entry {
        RenderedMessage::UserText { body, .. } => format!("> {body}"),
        RenderedMessage::AssistantText { body, .. } => format!("● {body}"),
        RenderedMessage::SystemText { body, .. } => body.clone(),
        RenderedMessage::AssistantToolUse { id, tool, input } => {
            render_assistant_tool_use_to_string(AssistantToolUseProps {
                id: *id,
                tool: tool.clone(),
                input: input.clone(),
                expanded,
                focused,
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
            id: *id,
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
        RenderedMessage::UserImage { image_id, metadata } => {
            image::render_image_label(*image_id, metadata.as_deref())
        }
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
            ..
        } => {
            let counts = collapsed_read_search::CollapsedCounts {
                search: *search_count,
                read: *read_count,
                list: *list_count,
                is_active: *is_active,
            };
            collapsed_read_search::render_collapsed_to_string(&counts, entries, expanded)
        }
    }
}

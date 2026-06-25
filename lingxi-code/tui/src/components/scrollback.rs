//! Per-variant message dispatch for the scrollback.
//!
//! M6-02 shipped a row-based `Scrollback` component here. M7-03 replaced
//! the live scrollback with [`crate::components::virtual_message_list`]
//! (line-based windowing over the full retained log). This module now
//! exists only as the home of [`render_message`] — the per-variant
//! dispatch shared by the windowed renderer.

use std::collections::HashMap;

use iocraft::prelude::*;
use protocol::ToolUseId;

use crate::components::messages::{
    advisor::AdvisorMessage,
    assistant_text::AssistantTextMessage,
    assistant_tool_use::AssistantToolUseMessage,
    attachment::AttachmentMessage,
    bash_input::UserBashInputMessage,
    bash_output::UserBashOutputMessage,
    collapsed_read_search::{CollapsedCounts, CollapsedReadSearchContent},
    command::UserCommandMessage,
    compact_boundary::CompactBoundaryMessage,
    grouped_tool_use::GroupedToolUseContent,
    hook_progress::HookProgressMessage,
    image::UserImageMessage,
    local_command_output::UserLocalCommandOutputMessage,
    memory_input::UserMemoryInputMessage,
    plan::UserPlanMessage,
    plan_approval::PlanApprovalMessage,
    prompt::UserPromptMessage,
    rate_limit::RateLimitMessage,
    redacted_thinking::AssistantRedactedThinkingMessage,
    resource_update::UserResourceUpdateMessage,
    shutdown::ShutdownMessage,
    system_api_error::SystemApiErrorMessage,
    system_text::SystemTextMessage,
    task_assignment::TaskAssignmentMessage,
    thinking::AssistantThinkingMessage,
    user_agent_notification::UserAgentNotificationMessage,
    user_channel::UserChannelMessage,
    user_teammate::UserTeammateMessage,
    user_text::UserTextMessage,
    user_tool_result::UserToolResultMessage,
};
use crate::state::RenderedMessage;
use crate::theme::{Theme, ThemeName};

/// Dispatch one [`RenderedMessage`] to its per-variant renderer, threading
/// the per-id `expanded` flags and `focused_tool_id` into the tool blocks
/// (including M7-02's `StructuredDiff` branch for `UserToolResult`). Shared
/// by the M7-03 `VirtualMessageList` (re-exported as
/// `virtual_message_list::render_message`).
///
/// (M7-15) `theme` is the active render palette and `theme_name` the active
/// theme (for syntect-colored diffs). Both come from `AppState.theme` /
/// `theme_setting.resolve()`; the theme picker's live preview re-renders the
/// whole list, so message colors follow the highlighted theme.
///
/// (A2) `width` is the scrollback viewport width in display columns, threaded
/// into the markdown body renderer so markdown TABLES lay out to fit the
/// terminal (the only block whose layout depends on width). `0` means "use the
/// markdown default" — the height cache passes the same width so measurement
/// stays in lock-step with what is drawn.
#[must_use]
#[allow(clippy::implicit_hasher)] // always called with `AppState.expanded`'s std hasher
#[allow(clippy::too_many_lines)] // one arm per RenderedMessage variant (28 variants)
pub fn render_message(
    m: RenderedMessage,
    expanded: &HashMap<ToolUseId, bool>,
    focused_tool_id: Option<ToolUseId>,
    theme: Theme,
    theme_name: ThemeName,
    width: usize,
    resolved: &HashMap<ToolUseId, bool>,
) -> AnyElement<'static> {
    match m {
        RenderedMessage::UserText { body, .. } => element! {
            UserTextMessage(body: body)
        }
        .into_any(),
        RenderedMessage::AssistantText { body, .. } => element! {
            AssistantTextMessage(body: body, width: width)
        }
        .into_any(),
        RenderedMessage::SystemText { body, is_error, .. } => {
            let color = if is_error { theme.error } else { theme.dim };
            element! {
                Text(content: body, color: color)
            }
            .into_any()
        }
        // M6-04 T11: dispatch the two tool variants through the real
        // components, threading per-id expanded + focused state.
        RenderedMessage::AssistantToolUse { id, tool, input } => {
            let is_expanded = expanded.get(&id).copied().unwrap_or(false);
            let is_focused = focused_tool_id.as_ref() == Some(&id);
            // (ma-02) Resolution state of the paired result (by `id`): `None`
            // when no result has arrived → dim dot; `Some(is_error)` → green /
            // red dot (claude-code `ToolUseLoader`).
            let resolution = resolved.get(&id).copied();
            element! {
                AssistantToolUseMessage(
                    id: id,
                    tool: tool,
                    input: input,
                    expanded: is_expanded,
                    focused: is_focused,
                    // Session cwd drives getDisplayPath path-shortening in the
                    // per-tool preview (claude-code `renderToolUseMessage`).
                    cwd: std::env::current_dir().unwrap_or_default(),
                    resolution: resolution,
                )
            }
            .into_any()
        }
        RenderedMessage::UserToolResult {
            id,
            tool,
            result,
            old_string,
            new_string,
            file_path,
        } => {
            let is_expanded = expanded.get(&id).copied().unwrap_or(false);
            let is_focused = focused_tool_id.as_ref() == Some(&id);
            element! {
                UserToolResultMessage(
                    id: id,
                    tool: tool,
                    result: result,
                    expanded: is_expanded,
                    focused: is_focused,
                    // M7-02: diff inputs populated upstream in
                    // `streaming::apply_event` from the paired `ToolUseStart`.
                    old_string: old_string,
                    new_string: new_string,
                    file_path: file_path,
                    // (M7-15) active theme → diff syntax follows the picker.
                    theme_name: theme_name,
                    // (diff-03) right-edge background padding on changed rows.
                    width: width,
                )
            }
            .into_any()
        }
        RenderedMessage::CompactBoundary { .. } => element! {
            CompactBoundaryMessage()
        }
        .into_any(),
        // (M7-04) batch-1 system/assistant renderers.
        RenderedMessage::AssistantThinking { thinking, expanded } => element! {
            AssistantThinkingMessage(thinking: thinking, expanded: expanded)
        }
        .into_any(),
        RenderedMessage::AssistantRedactedThinking => element! {
            AssistantRedactedThinkingMessage()
        }
        .into_any(),
        RenderedMessage::SystemTextRich { body, level } => element! {
            SystemTextMessage(body: body, level: level, theme: theme)
        }
        .into_any(),
        RenderedMessage::SystemApiError {
            error,
            retry_attempt,
            retry_in_seconds,
            max_retries,
            truncated,
        } => element! {
            SystemApiErrorMessage(
                error: error,
                retry_attempt: retry_attempt,
                retry_in_seconds: retry_in_seconds,
                max_retries: max_retries,
                truncated: truncated,
            )
        }
        .into_any(),
        RenderedMessage::RateLimit { text, upsell } => element! {
            RateLimitMessage(text: text, upsell: upsell)
        }
        .into_any(),
        RenderedMessage::Shutdown {
            from,
            reason,
            rejected,
        } => element! {
            ShutdownMessage(from: from, reason: reason, rejected: rejected, theme: theme)
        }
        .into_any(),
        RenderedMessage::Advisor { kind, verbose } => element! {
            AdvisorMessage(kind: kind, verbose: verbose, theme: theme)
        }
        .into_any(),
        RenderedMessage::HookProgress {
            event,
            count,
            transcript_summary,
        } => element! {
            HookProgressMessage(
                event: event,
                count: count,
                transcript_summary: transcript_summary,
                theme: theme,
            )
        }
        .into_any(),
        RenderedMessage::PlanApproval { kind } => element! {
            PlanApprovalMessage(kind: kind, theme: theme)
        }
        .into_any(),
        // ---- (M7-05) batch-2 user renderers ----------------------------
        RenderedMessage::UserBashInput { command } => element! {
            UserBashInputMessage(command: command)
        }
        .into_any(),
        RenderedMessage::UserBashOutput { stdout, stderr } => element! {
            UserBashOutputMessage(stdout: stdout, stderr: stderr)
        }
        .into_any(),
        RenderedMessage::UserCommand {
            command,
            args,
            is_skill,
        } => element! {
            UserCommandMessage(command: command, args: args, is_skill: is_skill)
        }
        .into_any(),
        RenderedMessage::UserLocalCommandOutput { stdout, stderr } => element! {
            UserLocalCommandOutputMessage(stdout: stdout, stderr: stderr)
        }
        .into_any(),
        RenderedMessage::UserMemoryInput { input } => element! {
            UserMemoryInputMessage(input: input)
        }
        .into_any(),
        RenderedMessage::UserPlan { plan_content } => element! {
            UserPlanMessage(plan_content: plan_content, theme: theme)
        }
        .into_any(),
        RenderedMessage::UserPrompt { text } => element! {
            UserPromptMessage(text: text)
        }
        .into_any(),
        RenderedMessage::UserResourceUpdate { updates } => element! {
            UserResourceUpdateMessage(updates: updates)
        }
        .into_any(),
        RenderedMessage::UserImage { image_id, metadata } => element! {
            UserImageMessage(image_id: image_id, metadata: metadata)
        }
        .into_any(),
        RenderedMessage::Attachment { attachment } => element! {
            AttachmentMessage(attachment: attachment)
        }
        .into_any(),
        RenderedMessage::GroupedToolUse {
            tool,
            group_id,
            entries,
        } => {
            let is_expanded = expanded.get(&group_id).copied().unwrap_or(false);
            element! {
                GroupedToolUseContent(tool: tool, entries: entries, expanded: is_expanded)
            }
            .into_any()
        }
        RenderedMessage::CollapsedReadSearch {
            search_count,
            read_count,
            list_count,
            is_active,
            group_id,
            entries,
            mem_read,
            mem_search,
            mem_write,
        } => {
            let is_expanded = expanded.get(&group_id).copied().unwrap_or(false);
            let counts = CollapsedCounts {
                search: search_count,
                read: read_count,
                list: list_count,
                is_active,
                mem_read,
                mem_search,
                mem_write,
            };
            element! {
                CollapsedReadSearchContent(counts: counts, entries: entries, expanded: is_expanded)
            }
            .into_any()
        }
        RenderedMessage::TaskAssignment {
            task_id,
            assigned_by,
            subject,
            description,
        } => element! {
            TaskAssignmentMessage(
                task_id: task_id,
                assigned_by: assigned_by,
                subject: subject,
                description: description,
                theme: theme,
            )
        }
        .into_any(),
        RenderedMessage::AgentNotification { summary, status } => element! {
            UserAgentNotificationMessage(summary: summary, status: status, theme: theme,)
        }
        .into_any(),
        RenderedMessage::ChannelMessage {
            server,
            user,
            content,
        } => element! {
            UserChannelMessage(server: server, user: user, content: content, theme: theme,)
        }
        .into_any(),
        RenderedMessage::UserTeammate {
            display_name,
            color,
            kind,
        } => element! {
            UserTeammateMessage(display_name: display_name, color: color, kind: kind, theme: theme,)
        }
        .into_any(),
    }
}

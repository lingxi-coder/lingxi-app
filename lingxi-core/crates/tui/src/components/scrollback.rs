//! Per-variant message dispatch for the scrollback.
//!
//! M6-02 shipped a row-based `Scrollback` component here. M7-03 replaced
//! the live scrollback with [`crate::components::virtual_message_list`]
//! (line-based windowing over the full retained log). This module now
//! exists only as the home of [`render_message`] — the per-variant
//! dispatch shared by the windowed renderer.

use std::collections::HashMap;

use iocraft::prelude::*;
use lingxi_protocol::ToolUseId;

use crate::components::messages::{
    advisor::AdvisorMessage, assistant_text::AssistantTextMessage,
    assistant_tool_use::AssistantToolUseMessage, compact_boundary::CompactBoundaryMessage,
    hook_progress::HookProgressMessage, plan_approval::PlanApprovalMessage,
    rate_limit::RateLimitMessage, redacted_thinking::AssistantRedactedThinkingMessage,
    shutdown::ShutdownMessage, system_api_error::SystemApiErrorMessage,
    system_text::SystemTextMessage, thinking::AssistantThinkingMessage,
    user_text::UserTextMessage, user_tool_result::UserToolResultMessage,
};
use crate::state::RenderedMessage;
use crate::theme::TuiTheme;

/// Dispatch one [`RenderedMessage`] to its per-variant renderer, threading
/// the per-id `expanded` flags and `focused_tool_id` into the tool blocks
/// (including M7-02's `StructuredDiff` branch for `UserToolResult`). Shared
/// by the M7-03 `VirtualMessageList` (re-exported as
/// `virtual_message_list::render_message`).
#[must_use]
#[allow(clippy::implicit_hasher)] // always called with `AppState.expanded`'s std hasher
#[allow(clippy::too_many_lines)] // one arm per RenderedMessage variant (15 variants)
pub fn render_message(
    m: RenderedMessage,
    expanded: &HashMap<ToolUseId, bool>,
    focused_tool_id: Option<ToolUseId>,
) -> AnyElement<'static> {
    match m {
        RenderedMessage::UserText { body, .. } => element! {
            UserTextMessage(body: body)
        }
        .into_any(),
        RenderedMessage::AssistantText { body, .. } => element! {
            AssistantTextMessage(body: body)
        }
        .into_any(),
        RenderedMessage::SystemText { body, is_error, .. } => {
            let color = if is_error {
                TuiTheme::ERROR
            } else {
                TuiTheme::DIM
            };
            element! {
                Text(content: body, color: color)
            }
            .into_any()
        }
        // M6-04 T11: dispatch the two tool variants through the real
        // components, threading per-id expanded + focused state.
        RenderedMessage::AssistantToolUse { id, tool, input } => {
            let is_expanded = expanded.get(&id).copied().unwrap_or(false);
            let is_focused = focused_tool_id == Some(id);
            element! {
                AssistantToolUseMessage(
                    id: id,
                    tool: tool,
                    input: input,
                    expanded: is_expanded,
                    focused: is_focused,
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
            let is_focused = focused_tool_id == Some(id);
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
            SystemTextMessage(body: body, level: level)
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
            ShutdownMessage(from: from, reason: reason, rejected: rejected)
        }
        .into_any(),
        RenderedMessage::Advisor { kind, verbose } => element! {
            AdvisorMessage(kind: kind, verbose: verbose)
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
            )
        }
        .into_any(),
        RenderedMessage::PlanApproval { kind } => element! {
            PlanApprovalMessage(kind: kind)
        }
        .into_any(),
    }
}

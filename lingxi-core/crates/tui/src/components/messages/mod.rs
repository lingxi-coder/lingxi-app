//! Message renderers — one component per `RenderedMessage` variant.
//!
//! M6-02 ships two renderers (`UserTextMessage`, `AssistantTextMessage`).
//! M6-04 adds `AssistantToolUseMessage` + `UserToolResultMessage` plus the
//! `render_entry_to_string` string-form dispatcher used by snapshot tests
//! and the iocraft `Scrollback` component.

pub mod advisor;
pub mod assistant_text;
pub mod assistant_tool_use;
pub mod compact_boundary;
pub mod hook_progress;
pub mod plan_approval;
pub mod rate_limit;
pub mod redacted_thinking;
pub mod shutdown;
pub mod system_api_error;
pub mod system_text;
pub mod thinking;
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
        }),
        RenderedMessage::Advisor { kind, verbose } => {
            advisor::render_advisor_to_string(advisor::AdvisorProps {
                kind: kind.clone(),
                verbose: *verbose,
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
        }),
        RenderedMessage::PlanApproval { kind } => {
            plan_approval::render_plan_approval_to_string(plan_approval::PlanApprovalProps {
                kind: kind.clone(),
            })
        }
    }
}

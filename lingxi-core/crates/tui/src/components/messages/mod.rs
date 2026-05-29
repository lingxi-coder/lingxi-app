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
        // (M7-04) TEMPORARY catch-all for the 10 batch-1 variants until each
        // task wires its real arm; the dispatch test in Task 11 makes its
        // removal verifiable (the match becomes exhaustive).
        _ => String::new(),
    }
}

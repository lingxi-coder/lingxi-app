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
    assistant_text::AssistantTextMessage, assistant_tool_use::AssistantToolUseMessage,
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
    }
}

//! `GroupedToolUseContent` — folds consecutive same-tool tool-use blocks.
//!
//! Folding (from claude-code `GroupedToolUseContent.tsx`):
//!   - claude-code delegates each group to the tool's `renderGroupedToolUse`;
//!     M7-05 reproduces the *fold structure* only (the per-tool custom
//!     renderers are the M7-04/06+ tool-renderer surface — out of scope here).
//!   - collapsed: `● {tool} (×N)`  (drops `(×1)` for a single entry)
//!   - expanded: header + each child input line + result line, indented 2sp
//!     (children rendered via the existing `render_assistant_tool_use_to_string`
//!     / `render_user_tool_result_to_string` shapes).
//!   - `group_id` (first child's id) keys `AppState.expanded`.
//!   source: claude-code/src/components/messages/GroupedToolUseContent.tsx
#![allow(clippy::needless_pass_by_value)]
#![allow(clippy::doc_markdown, clippy::doc_lazy_continuation)]

use iocraft::prelude::*;
use protocol::ToolUseId;

use crate::components::messages::assistant_tool_use::{
    render_assistant_tool_use_to_string, AssistantToolUseProps,
};
use crate::components::messages::user_tool_result::{
    render_user_tool_result_to_string, UserToolResultProps,
};
use crate::theme::TuiTheme;

/// Dot marker prefix (matches `assistant_text.rs` `● `).
pub const MARKER: &str = "\u{25CF} ";
/// Child indent (2 spaces).
pub const INDENT: &str = "  ";

/// Pure string renderer.
#[must_use]
pub fn render_grouped_to_string(
    tool: &str,
    entries: &[(serde_json::Value, serde_json::Value)],
    expanded: bool,
) -> String {
    let n = entries.len();
    let header = if n <= 1 {
        format!("{MARKER}{tool}")
    } else {
        format!("{MARKER}{tool} (\u{00D7}{n})")
    };
    if !expanded {
        return header;
    }
    let mut out = header;
    for (input, result) in entries {
        let in_line = render_assistant_tool_use_to_string(AssistantToolUseProps {
            id: ToolUseId::new(),
            tool: tool.to_string(),
            input: input.clone(),
            expanded: false,
            focused: false,
        });
        let res_line = render_user_tool_result_to_string(UserToolResultProps {
            id: ToolUseId::new(),
            tool: tool.to_string(),
            result: result.clone(),
            expanded: false,
            focused: false,
            ..Default::default()
        });
        for line in in_line.lines().chain(res_line.lines()) {
            out.push('\n');
            out.push_str(INDENT);
            out.push_str(line);
        }
    }
    out
}

/// Props for [`GroupedToolUseContent`].
#[derive(Debug, Clone, Default, Props)]
pub struct GroupedToolUseProps {
    /// Shared tool name.
    pub tool: String,
    /// `(input, result)` pairs.
    pub entries: Vec<(serde_json::Value, serde_json::Value)>,
    /// Expanded state (from `AppState.expanded`).
    pub expanded: bool,
}

/// iocraft component.
#[component]
pub fn GroupedToolUseContent(props: &GroupedToolUseProps) -> impl Into<AnyElement<'static>> {
    let body = render_grouped_to_string(&props.tool, &props.entries, props.expanded);
    element! {
        View(flex_direction: FlexDirection::Column) {
            Text(content: body, color: TuiTheme::ASSISTANT)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_bytes() {
        // U+25CF = 0xE2 0x97 0x8F, then ASCII space.
        assert_eq!(MARKER.as_bytes(), &[0xE2, 0x97, 0x8F, 0x20]);
    }

    #[test]
    fn empty_group_no_count() {
        assert_eq!(
            render_grouped_to_string("Bash", &[], false),
            "\u{25CF} Bash"
        );
    }

    #[test]
    fn multi_entry_count() {
        let entries = vec![
            (serde_json::json!({}), serde_json::json!({"content": "a"})),
            (serde_json::json!({}), serde_json::json!({"content": "b"})),
        ];
        assert_eq!(
            render_grouped_to_string("Read", &entries, false),
            "\u{25CF} Read (\u{00D7}2)"
        );
    }
}

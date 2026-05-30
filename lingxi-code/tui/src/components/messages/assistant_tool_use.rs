//! `AssistantToolUseMessage` — header line `● Tool(input_preview)`.
//!
//! Literal locks (byte-for-byte from claude-code):
//!   - marker: `●` (U+25CF, 3-byte UTF-8 `0xE2 0x97 0x8F`)
//!     source: claude-code/src/constants/figures.ts `BLACK_CIRCLE`
//!   - focus prefix: `> ` (ASCII, 2 bytes)
//!     source: claude-code/src/components/MessageSelector.tsx
#![allow(clippy::needless_pass_by_value)]

use iocraft::prelude::*;
use protocol::ToolUseId;

/// Marker glyph. 3-byte UTF-8.
pub const MARKER: &str = "●";
/// Focus prefix prepended when this block is the focused one.
pub const FOCUS_PREFIX: &str = "> ";

/// Props for [`AssistantToolUseMessage`].
#[derive(Debug, Clone, Default, Props)]
pub struct AssistantToolUseProps {
    /// Correlator (model-supplied `tool_use_id`).
    pub id: ToolUseId,
    /// Tool name.
    pub tool: String,
    /// JSON input passed to the tool.
    pub input: serde_json::Value,
    /// `true` → render the pretty-printed JSON body after the header.
    pub expanded: bool,
    /// `true` → render the `> ` focus prefix.
    pub focused: bool,
}

/// Pure-string renderer used by snapshot tests AND the iocraft component
/// (the component delegates to this and wraps the result in `Text`).
///
/// Format:
///   collapsed:        `[> ]● Tool({"k": "v"})`
///   expanded:         `[> ]● Tool({"k": "v"})\n{\n  "k": "v"\n}`
///
/// The single-line preview restores one space after each `:` and `,` so it
/// reads like claude-code's `JSON.stringify(input, null, 0)`-with-spaces.
#[must_use]
pub fn render_assistant_tool_use_to_string(props: AssistantToolUseProps) -> String {
    let prefix = if props.focused { FOCUS_PREFIX } else { "" };
    let preview = single_line_json_preview(&props.input);
    let header = format!("{prefix}{MARKER} {tool}({preview})", tool = props.tool);
    if !props.expanded {
        return header;
    }
    let pretty =
        serde_json::to_string_pretty(&props.input).unwrap_or_else(|_| props.input.to_string());
    format!("{header}\n{pretty}")
}

/// Single-line JSON preview. Renders the input as compact JSON, then
/// inserts one space after each top-level `:` and `,` (string contents
/// are left untouched). No truncation — the caller's iocraft `Text`
/// element handles wrapping.
fn single_line_json_preview(input: &serde_json::Value) -> String {
    let s = input.to_string(); // compact form: {"k":"v"}
    let mut out = String::with_capacity(s.len() + 16);
    let mut in_string = false;
    let mut prev = '\0';
    for ch in s.chars() {
        if ch == '"' && prev != '\\' {
            in_string = !in_string;
        }
        out.push(ch);
        if !in_string && (ch == ':' || ch == ',') {
            out.push(' ');
        }
        prev = ch;
    }
    out
}

/// iocraft component — wraps [`render_assistant_tool_use_to_string`] in a
/// cyan `Text` element (assistant theme).
#[component]
pub fn AssistantToolUseMessage(props: &AssistantToolUseProps) -> impl Into<AnyElement<'static>> {
    let body = render_assistant_tool_use_to_string(props.clone());
    element! {
        View(flex_direction: FlexDirection::Column) {
            Text(content: body, color: Color::Cyan)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_is_three_utf8_bytes() {
        assert_eq!(MARKER.as_bytes(), &[0xE2, 0x97, 0x8F]);
    }

    #[test]
    fn single_line_preview_has_space_after_colon_and_comma() {
        let v = serde_json::json!({"a": 1, "b": "x"});
        let s = single_line_json_preview(&v);
        assert_eq!(s, r#"{"a": 1, "b": "x"}"#);
    }

    #[test]
    fn single_line_preview_leaves_string_internals_alone() {
        let v = serde_json::json!({"k": "a:b,c"});
        let s = single_line_json_preview(&v);
        assert_eq!(s, r#"{"k": "a:b,c"}"#);
    }
}

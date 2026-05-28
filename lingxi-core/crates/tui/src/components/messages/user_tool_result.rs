//! `UserToolResultMessage` — `└ ` indent, dim-colored, line/byte-bounded.
//!
//! Literal locks (byte-for-byte from claude-code):
//!   - indent marker: `└ ` (U+2514 + ASCII space, 4-byte UTF-8 `0xE2 0x94 0x94 0x20`)
//!     source: claude-code/src/components/messages/UserToolResultMessage/UserToolSuccessMessage.tsx
//!   - truncation footer: `[output truncated, {N} more lines]`
//!     source: claude-code/src/utils/messages.ts
//!   - MAX_LINES = 100, MAX_BYTES = 4000
//!     source: claude-code/src/utils/messages.ts
//!     (`MAX_LINES_PRINTED_PER_TOOL_USE_RESULT`,
//!     `MAX_CHARACTERS_PRINTED_PER_TOOL_USE_RESULT`)
//!   - focus prefix: `> ` (ASCII)

use iocraft::prelude::*;
use lingxi_protocol::ToolUseId;

use crate::ansi::{parse_ansi, AnsiColor, AnsiSpan};
use crate::theme::TuiTheme;

/// Indent marker glyph + space. 4-byte UTF-8.
pub const MARKER: &str = "└ ";
/// Per-line indent (2 spaces) — matches `MARKER` display width.
pub const INDENT: &str = "  ";
/// Focus prefix prepended when this block is focused.
pub const FOCUS_PREFIX: &str = "> ";
/// Hard line cap. claude-code parity.
pub const MAX_LINES: usize = 100;
/// Hard byte cap. claude-code parity.
pub const MAX_BYTES: usize = 4000;

/// Props for [`UserToolResultMessage`].
#[derive(Debug, Clone, Default, Props)]
pub struct UserToolResultProps {
    /// Correlator matching the paired `AssistantToolUseMessage.id`.
    pub id: ToolUseId,
    /// Tool name (used to gate Bash → ANSI parser at render time — Task 12).
    pub tool: String,
    /// JSON result payload.
    pub result: serde_json::Value,
    /// `true` → render full body (line/byte-capped) + truncation footer.
    pub expanded: bool,
    /// `true` → render the `> ` focus prefix on the first line.
    pub focused: bool,
}

/// Extract the human-displayable body from a tool result JSON.
///
/// M5-04 turn_loop emits results in three shapes:
///   `{"content": "..."}`                      — string body (Read, Bash, Grep)
///   `{"content": [{"type":"text","text":""}]}` — block-array (some MCP tools)
///   any other shape                            — fall back to pretty-printed JSON
pub fn body_text(result: &serde_json::Value) -> String {
    if let Some(s) = result.get("content").and_then(|c| c.as_str()) {
        return s.to_string();
    }
    if let Some(arr) = result.get("content").and_then(|c| c.as_array()) {
        let mut out = String::new();
        for block in arr {
            if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(t);
            }
        }
        if !out.is_empty() {
            return out;
        }
    }
    serde_json::to_string_pretty(result).unwrap_or_else(|_| result.to_string())
}

/// Apply both the line cap and the byte cap. Returns the truncated body
/// plus a `truncated_lines: usize` count (0 when no truncation bit).
///
/// Order of operations:
///   1. Byte cap (cheaper; bounds the slice we walk for line counting).
///      Slice is walked back to a char boundary to avoid mid-codepoint cuts.
///   2. Line cap (keep first `MAX_LINES` lines of the byte-capped slice).
///
/// `truncated_lines` is the count of lines dropped from the ORIGINAL body
/// (so a 1-line 5000-byte body that gets byte-capped reports 0 dropped
/// lines — the line is shorter, but there's still only 1 line of output).
#[must_use]
pub fn truncate(body: &str) -> (String, usize) {
    // Byte cap first.
    let byte_capped: &str = if body.len() > MAX_BYTES {
        let mut idx = MAX_BYTES;
        while idx > 0 && !body.is_char_boundary(idx) {
            idx -= 1;
        }
        &body[..idx]
    } else {
        body
    };

    let line_count = byte_capped.lines().count();
    if line_count <= MAX_LINES && byte_capped.len() == body.len() {
        return (body.to_string(), 0);
    }
    let lines: Vec<&str> = byte_capped.lines().take(MAX_LINES).collect();
    let kept = lines.join("\n");
    let total_lines = body.lines().count();
    let dropped = total_lines.saturating_sub(lines.len());
    (kept, dropped)
}

/// Pure-string renderer.
///
/// Collapsed: `[> ]└ first_line[ (+N lines)]`
/// Expanded:  `[> ]└ line_1\n  line_2\n  …\n  [output truncated, N more lines]`
#[must_use]
pub fn render_user_tool_result_to_string(props: UserToolResultProps) -> String {
    let prefix = if props.focused { FOCUS_PREFIX } else { "" };
    let body = body_text(&props.result);

    // Collapsed: 1-line summary.
    if !props.expanded {
        let first_line = body.lines().next().unwrap_or("");
        let total = body.lines().count();
        let suffix = if total > 1 {
            format!(" (+{} lines)", total - 1)
        } else {
            String::new()
        };
        return format!("{prefix}{MARKER}{first_line}{suffix}");
    }

    // Expanded: full body, line+byte capped.
    let (truncated, dropped) = truncate(&body);
    let mut out = String::new();
    for (i, line) in truncated.lines().enumerate() {
        if i == 0 {
            out.push_str(prefix);
            out.push_str(MARKER);
        } else {
            out.push('\n');
            out.push_str(INDENT);
        }
        out.push_str(line);
    }
    if dropped > 0 {
        out.push('\n');
        out.push_str(INDENT);
        out.push_str(&format!("[output truncated, {dropped} more lines]"));
    }
    out
}

/// Produce the styled spans for the body. Only Bash output runs through
/// the ANSI parser; everything else is a single default-styled span over
/// the result body text. (M6-04 Task 12.)
///
/// Note: this is the body-only span pipeline. The marker / focus prefix
/// from [`render_user_tool_result_to_string`] is added separately by
/// the iocraft component (so a colored Bash run still gets a dim leading
/// `└ ` marker).
#[must_use]
pub fn render_user_tool_result_body_spans(props: &UserToolResultProps) -> Vec<AnsiSpan> {
    let body = body_text(&props.result);
    let (truncated, _dropped) = truncate(&body);
    if props.tool == "Bash" {
        parse_ansi(&truncated)
    } else {
        vec![AnsiSpan {
            style: AnsiStyle::default(),
            text: truncated,
        }]
    }
}

/// Map an [`AnsiColor`] to an iocraft [`Color`].
///
/// The 8-color palette maps to crossterm's "dark" range (`DarkRed` etc.)
/// to match terminal defaults where SGR 31 is darker than SGR 91. Bright
/// variants map to the non-dark range. `Default` becomes `Color::Reset`.
fn ansi_to_iocraft_color(c: AnsiColor) -> Color {
    match c {
        AnsiColor::Default => Color::Reset,
        AnsiColor::Black => Color::Black,
        AnsiColor::Red => Color::DarkRed,
        AnsiColor::Green => Color::DarkGreen,
        AnsiColor::Yellow => Color::DarkYellow,
        AnsiColor::Blue => Color::DarkBlue,
        AnsiColor::Magenta => Color::DarkMagenta,
        AnsiColor::Cyan => Color::DarkCyan,
        AnsiColor::White => Color::Grey,
        AnsiColor::BrightBlack => Color::DarkGrey,
        AnsiColor::BrightRed => Color::Red,
        AnsiColor::BrightGreen => Color::Green,
        AnsiColor::BrightYellow => Color::Yellow,
        AnsiColor::BrightBlue => Color::Blue,
        AnsiColor::BrightMagenta => Color::Magenta,
        AnsiColor::BrightCyan => Color::Cyan,
        AnsiColor::BrightWhite => Color::White,
    }
}

// Re-export the default style helper for ansi_to_iocraft_color callers.
use crate::ansi::AnsiStyle;

/// iocraft component — wraps [`render_user_tool_result_to_string`] in a
/// dim-grey `Text` element. Bash output passes through the ANSI parser
/// (Task 12): the result body is decomposed into styled spans, each
/// rendered as a child `Text` element with the mapped color.
#[component]
pub fn UserToolResultMessage(props: &UserToolResultProps) -> impl Into<AnyElement<'static>> {
    // For Bash + expanded: render colored spans. Otherwise: fall back to
    // the pure string renderer (faster + already tested).
    if props.tool == "Bash" && props.expanded {
        let header = {
            // Reuse the string renderer to compute the prefix + marker
            // for the first line, then strip the body and replace it with
            // the colored span sequence.
            let prefix = if props.focused { FOCUS_PREFIX } else { "" };
            format!("{prefix}{MARKER}")
        };
        let spans = render_user_tool_result_body_spans(props);
        let span_elements: Vec<AnyElement<'static>> = spans
            .into_iter()
            .map(|s| {
                let color = ansi_to_iocraft_color(s.style.fg);
                let weight = if s.style.bold {
                    Weight::Bold
                } else {
                    Weight::Normal
                };
                element! {
                    Text(content: s.text, color: color, weight: weight)
                }
                .into_any()
            })
            .collect();
        return element! {
            View(flex_direction: FlexDirection::Column) {
                Text(content: header, color: TuiTheme::DIM)
                View(flex_direction: FlexDirection::Row) {
                    #(span_elements)
                }
            }
        };
    }
    let body = render_user_tool_result_to_string(props.clone());
    element! {
        View(flex_direction: FlexDirection::Column) {
            Text(content: body, color: TuiTheme::DIM)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_is_four_utf8_bytes() {
        assert_eq!(MARKER.as_bytes(), &[0xE2, 0x94, 0x94, 0x20]);
    }

    #[test]
    fn truncate_short_body_returns_unchanged() {
        let (s, dropped) = truncate("a\nb\nc");
        assert_eq!(s, "a\nb\nc");
        assert_eq!(dropped, 0);
    }

    #[test]
    fn truncate_120_line_body_caps_at_100() {
        let body = (0..120)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let (s, dropped) = truncate(&body);
        assert_eq!(s.lines().count(), 100);
        assert_eq!(dropped, 20);
    }

    #[test]
    fn truncate_huge_body_respects_byte_cap() {
        let body = "x".repeat(5000);
        let (s, dropped) = truncate(&body);
        assert!(s.len() <= MAX_BYTES);
        assert_eq!(dropped, 0);
    }

    #[test]
    fn body_text_extracts_string_content() {
        let v = serde_json::json!({"content": "hi"});
        assert_eq!(body_text(&v), "hi");
    }

    #[test]
    fn body_text_extracts_block_array() {
        let v = serde_json::json!({"content": [{"type":"text","text":"a"},{"type":"text","text":"b"}]});
        assert_eq!(body_text(&v), "a\nb");
    }
}

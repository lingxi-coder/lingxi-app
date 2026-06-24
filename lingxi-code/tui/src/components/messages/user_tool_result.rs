//! `UserToolResultMessage` — `  ⎿  ` gutter, dim-colored, line/byte-bounded.
//!
//! Literal locks (byte-for-byte from claude-code):
//!   - gutter marker: `  ⎿  ` (2 spaces + U+23BF + 2 spaces, 7-byte UTF-8
//!     `0x20 0x20 0xE2 0x8E 0xBF 0x20 0x20`). Every tool result wraps in
//!     `MessageResponse`, whose fixed `flexShrink=0` left column renders
//!     `<Text dimColor>{"  "}⎿  </Text>`; the result body lives in the adjacent
//!     flex column. Matches the sibling `local_command_output::GUTTER`.
//!     source: claude-code/src/components/MessageResponse.tsx:22
//!   - truncation footer: `[output truncated, {N} more lines]`
//!     source: claude-code/src/utils/messages.ts
//!   - `MAX_LINES` = 100, `MAX_BYTES` = 4000
//!     source: claude-code/src/utils/messages.ts
//!     (`MAX_LINES_PRINTED_PER_TOOL_USE_RESULT`,
//!     `MAX_CHARACTERS_PRINTED_PER_TOOL_USE_RESULT`)
//!   - focus prefix: `> ` (ASCII)
#![allow(clippy::needless_pass_by_value)]

use iocraft::prelude::*;
use protocol::ToolUseId;

use crate::render::ansi::parse_ansi;
use crate::render::{diff, split_spans_into_line_rows, StyledLine, StyledSpan};
use crate::theme::TuiTheme;

/// Dim gutter glyph (2 spaces + U+23BF + 2 spaces, 7-byte UTF-8). Mirrors the
/// `MessageResponse` left column and `local_command_output::GUTTER`.
pub const MARKER: &str = "  \u{23BF}  ";
/// Per-line continuation indent (5 spaces) — matches `MARKER` display width so
/// wrapped/continuation lines align under the content column.
pub const INDENT: &str = "     ";
/// Focus prefix prepended when this block is focused.
pub const FOCUS_PREFIX: &str = "> ";
/// Hard line cap. claude-code parity.
pub const MAX_LINES: usize = 100;
/// Hard byte cap. claude-code parity.
pub const MAX_BYTES: usize = 4000;
/// Max rendered lines for an ERRORED tool result before the
/// `… +N lines (ctrl+o to see all)` footer kicks in. Distinct from the
/// success caps above. claude-code `FallbackToolUseErrorMessage`
/// `MAX_RENDERED_LINES`.
pub const MAX_ERROR_LINES: usize = 10;

/// Extract the content of the first `<tag …>…</tag>` occurrence (claude-code
/// `extractTag`, simplified for our non-nested error inputs). The opening tag
/// may carry an attribute list. Returns `None` when the tag is absent.
fn extract_tag(s: &str, tag: &str) -> Option<String> {
    let needle = format!("<{tag}");
    let mut search = 0;
    while let Some(rel) = s[search..].find(&needle) {
        let at = search + rel;
        let after = at + needle.len();
        // The tag name must be followed by `>` or whitespace so `<tool_use_error>`
        // does not match a hypothetical `<tool_use_error_x>`.
        let next = s[after..].chars().next();
        if matches!(next, Some('>' | ' ' | '\t' | '\n' | '\r')) {
            let gt = s[after..].find('>')? + after;
            let content_start = gt + 1;
            let close = format!("</{tag}>");
            let end = s[content_start..].find(&close)? + content_start;
            return Some(s[content_start..end].to_string());
        }
        search = after;
    }
    None
}

/// Drop `<sandbox_violations>…</sandbox_violations>` blocks from an error body
/// (claude-code `removeSandboxViolationTags`). An unclosed opening tag is kept
/// verbatim (the regex requires the closing tag).
fn remove_sandbox_violations(s: &str) -> String {
    const OPEN: &str = "<sandbox_violations>";
    const CLOSE: &str = "</sandbox_violations>";
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(o) = rest.find(OPEN) {
        out.push_str(&rest[..o]);
        if let Some(c) = rest[o..].find(CLOSE) {
            rest = &rest[o + c + CLOSE.len()..];
        } else {
            out.push_str(&rest[o..]);
            return out;
        }
    }
    out.push_str(rest);
    out
}

/// When `result` is an errored tool result, return the RAW error string.
///
/// claude-code keys the error branch off `param.is_error`; at LingXi's event
/// boundary that bool is dropped, but the signal survives in the payload as
/// either a top-level `"error"` string (every `turn_loop` error path emits
/// `{"error": <bare msg>}`) or a `<tool_use_error>`-wrapped body (the
/// concurrent / pre-execution paths). Both map here.
#[must_use]
pub fn tool_result_error(result: &serde_json::Value) -> Option<String> {
    if let Some(e) = result.get("error").and_then(serde_json::Value::as_str) {
        return Some(e.to_string());
    }
    let body = body_text(result);
    if body.contains("<tool_use_error>") {
        return Some(body);
    }
    None
}

/// Format an errored tool result for display (claude-code
/// `FallbackToolUseErrorMessage`): pull the `<tool_use_error>` content, drop
/// sandbox-violation + `<error>` tags, trim, then normalise the prefix. When
/// not `verbose`, an `InputValidationError:` collapses to a terse summary.
#[must_use]
pub fn format_tool_error(raw: &str, verbose: bool) -> String {
    let extracted = extract_tag(raw, "tool_use_error").unwrap_or_else(|| raw.to_string());
    let without_sandbox = remove_sandbox_violations(&extracted);
    let without_error_tags = without_sandbox.replace("<error>", "").replace("</error>", "");
    let trimmed = without_error_tags.trim();
    if !verbose && trimmed.contains("InputValidationError: ") {
        "Invalid tool parameters".to_string()
    } else if trimmed.starts_with("Error: ") || trimmed.starts_with("Cancelled: ") {
        trimmed.to_string()
    } else {
        format!("Error: {trimmed}")
    }
}

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
    /// M7-02: for Edit/Write diff tools, the pre-edit text (`old_string` for
    /// Edit; `None`/empty for Write). Populated by the dispatcher from the
    /// paired `AssistantToolUse.input`. `None` → no diff rendering.
    pub old_string: Option<String>,
    /// M7-02: for Edit/Write diff tools, the post-edit text (`new_string` for
    /// Edit; `content` for Write). `None` → no diff rendering.
    pub new_string: Option<String>,
    /// M7-02: file path of the edited file (`file_path` input), drives syntax
    /// language detection in the diff.
    pub file_path: Option<String>,
    /// (M7-15) Active theme — drives the syntect `.tmTheme` of the diff's
    /// syntax coloring (the diff recolors with the picker). Threaded from
    /// `scrollback::render_message`. Defaults to dark.
    pub theme_name: crate::theme::ThemeName,
}

/// Extract the human-displayable body from a tool result JSON.
///
/// M5-04 `turn_loop` emits results in three shapes:
///   `{"content": "..."}`                      — string body (Read, Bash, Grep)
///   `{"content": [{"type":"text","text":""}]}` — block-array (some MCP tools)
///   any other shape                            — fall back to pretty-printed JSON
#[must_use]
pub fn body_text(result: &serde_json::Value) -> String {
    // The replay/resume path wraps a persisted tool_result as a bare JSON
    // string (`replay.rs` `Value::String(content)`); return it directly so the
    // content renders WITHOUT the pretty-printer's surrounding quotes (and so
    // the marker/tag detectors below see the raw content).
    if let Some(s) = result.as_str() {
        return s.to_string();
    }
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

/// claude-code `CANCEL_MESSAGE` (utils/messages.ts:210) — the user clicked
/// "No" on a permission prompt. Byte-for-byte.
const CANCEL_MESSAGE: &str = "The user doesn't want to take this action right now. STOP what you are doing and wait for the user to tell you how to proceed.";
/// claude-code `REJECT_MESSAGE` (utils/messages.ts:212) — a tool use was
/// rejected. Byte-for-byte; matches `orchestrator` `synthetic_error_block`.
const REJECT_MESSAGE: &str = "The user doesn't want to proceed with this tool use. The tool use was rejected (eg. if it was a file edit, the new_string was NOT written to the file). STOP what you are doing and wait for the user to tell you how to proceed.";
/// claude-code `INTERRUPT_MESSAGE_FOR_TOOL_USE` (utils/messages.ts:208).
const INTERRUPT_MESSAGE_FOR_TOOL_USE: &str = "[Request interrupted by user for tool use]";
/// The single dim line `InterruptedByUser` renders for a canceled / rejected /
/// interrupted tool result (claude-code `InterruptedByUser.tsx`:
/// `Interrupted ` + `· What should Claude do instead?`).
pub const INTERRUPTED_LINE: &str = "Interrupted \u{00b7} What should Claude do instead?";

/// True when `result` is a canceled / rejected / interrupted tool result
/// (claude-code `UserToolResultMessage` dispatch: `startsWith(CANCEL_MESSAGE)`,
/// `startsWith(REJECT_MESSAGE)`, or `=== INTERRUPT_MESSAGE_FOR_TOOL_USE`). These
/// take precedence over the `is_error` branch, so a persisted REJECT_MESSAGE
/// result renders the terse `Interrupted · …` line instead of the verbose
/// model-facing string (mainly the resume/replay path — the live REPL shows an
/// `Interrupted by user` system message instead).
#[must_use]
pub fn tool_result_interrupted(result: &serde_json::Value) -> bool {
    let body = body_text(result);
    body.starts_with(CANCEL_MESSAGE)
        || body.starts_with(REJECT_MESSAGE)
        || body == INTERRUPT_MESSAGE_FOR_TOOL_USE
}

/// True for tools whose result is shown as a `StructuredDiff` (claude-code
/// `FileEditToolDiff`). Edit/MultiEdit/NotebookEdit show old→new; Write shows
/// a pure-add diff (no prior content).
#[must_use]
pub fn is_diff_tool(tool: &str) -> bool {
    matches!(tool, "Edit" | "Write" | "MultiEdit" | "NotebookEdit")
}

/// Build the `StructuredDiff` lines for an Edit/Write tool. Write = pure add
/// (old = ""); Edit = `old_string` → `new_string`. `path` drives syntax lang.
/// Both sides `None`/empty → empty diff (no lines).
#[must_use]
pub fn render_edit_write_diff_lines(
    _tool: &str,
    old_string: Option<&str>,
    new_string: Option<&str>,
    path: Option<&str>,
    theme: crate::theme::ThemeName,
) -> Vec<StyledLine> {
    let old = old_string.unwrap_or("");
    let new = new_string.unwrap_or("");
    diff::render(old, new, path, theme)
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
/// Collapsed: `[> ]  ⎿  first_line[ (+N lines)]`
/// Expanded:  `[> ]  ⎿  line_1\n     line_2\n     …\n     [output truncated, N more lines]`
#[must_use]
pub fn render_user_tool_result_to_string(props: UserToolResultProps) -> String {
    let prefix = if props.focused { FOCUS_PREFIX } else { "" };

    // (tool-reject-cancel-interrupted) Canceled / rejected / interrupted results
    // collapse to the single dim `Interrupted · What should Claude do instead?`
    // line (claude-code `InterruptedByUser`). Checked BEFORE the error branch
    // (claude-code dispatch order: CANCEL/REJECT/INTERRUPT precede `is_error`).
    if tool_result_interrupted(&props.result) {
        return format!("{prefix}{MARKER}{INTERRUPTED_LINE}");
    }

    // (tool-error) Errored results render as red `Error: …` with
    // `<tool_use_error>` / `<error>` / sandbox-violation tags stripped, capped
    // at `MAX_ERROR_LINES` with a `… +N lines (ctrl+o to see all)` footer
    // (claude-code `FallbackToolUseErrorMessage`). `expanded` ≈ `verbose`:
    // expanded shows the full body with no cap/footer. Color is applied by the
    // component; this string carries the gutter + body layout.
    if let Some(raw) = tool_result_error(&props.result) {
        let error = format_tool_error(&raw, props.expanded);
        let lines: Vec<&str> = error.lines().collect();
        let show = if props.expanded {
            lines.len()
        } else {
            lines.len().min(MAX_ERROR_LINES)
        };
        let mut out = String::new();
        for (i, line) in lines.iter().take(show).enumerate() {
            if i == 0 {
                out.push_str(prefix);
                out.push_str(MARKER);
            } else {
                out.push('\n');
                out.push_str(INDENT);
            }
            out.push_str(line);
        }
        let plus = lines.len().saturating_sub(MAX_ERROR_LINES);
        if !props.expanded && plus > 0 {
            let unit = if plus == 1 { "line" } else { "lines" };
            out.push('\n');
            out.push_str(INDENT);
            out.push_str(&format!("\u{2026} +{plus} {unit} (ctrl+o to see all)"));
        }
        return out;
    }

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

/// Produce the styled spans for the body. Only Bash output runs through the
/// ANSI parser; everything else is a single default-styled span over the
/// result body text. (Migrated to `render::ansi` in M7-01.)
///
/// The ANSI parser returns one `StyledLine` per visual line; this flattens
/// them into a single span vector, re-inserting `\n` between lines so the
/// existing single-`Row` renderer reproduces M6 behavior for one-line Bash
/// output (the only case M6 exercised).
#[must_use]
pub fn render_user_tool_result_body_spans(props: &UserToolResultProps) -> Vec<StyledSpan> {
    let body = body_text(&props.result);
    let (truncated, _dropped) = truncate(&body);
    if props.tool == "Bash" {
        let lines = parse_ansi(&truncated);
        let mut spans: Vec<StyledSpan> = Vec::new();
        for (li, line) in lines.into_iter().enumerate() {
            if li > 0 {
                spans.push(StyledSpan::plain("\n"));
            }
            spans.extend(line.spans);
        }
        spans
    } else {
        vec![StyledSpan::plain(truncated)]
    }
}

/// iocraft component — wraps [`render_user_tool_result_to_string`] in a
/// dim-grey `Text` element. Bash output passes through the ANSI parser
/// (Task 12): the result body is decomposed into styled spans, each
/// rendered as a child `Text` element with the mapped color.
#[component]
pub fn UserToolResultMessage(props: &UserToolResultProps) -> impl Into<AnyElement<'static>> {
    // (tool-reject-cancel-interrupted) A canceled / rejected / interrupted
    // result renders the single dim `Interrupted · …` line (claude-code
    // `InterruptedByUser`), ahead of every other branch.
    if tool_result_interrupted(&props.result) {
        let prefix = if props.focused { FOCUS_PREFIX } else { "" };
        return element! {
            View(flex_direction: FlexDirection::Row) {
                Text(content: format!("{prefix}{MARKER}"), color: TuiTheme::DIM)
                Text(content: INTERRUPTED_LINE.to_string(), color: TuiTheme::DIM)
            }
        };
    }

    // (tool-error) Errored results take precedence over the diff/Bash render
    // paths (claude-code checks `param.is_error` before the success branch):
    // the dim gutter sits beside a RED error body, stripped + prefixed by
    // `format_tool_error`, line-capped with a dim `… +N lines` footer.
    if let Some(raw) = tool_result_error(&props.result) {
        let error = format_tool_error(&raw, props.expanded);
        let lines: Vec<String> = error.lines().map(str::to_string).collect();
        let show = if props.expanded {
            lines.len()
        } else {
            lines.len().min(MAX_ERROR_LINES)
        };
        let plus = lines.len().saturating_sub(MAX_ERROR_LINES);
        let prefix = if props.focused { FOCUS_PREFIX } else { "" };
        let body_rows: Vec<AnyElement<'static>> = lines
            .into_iter()
            .take(show)
            .enumerate()
            .map(|(i, line)| {
                let lead = if i == 0 {
                    format!("{prefix}{MARKER}")
                } else {
                    INDENT.to_string()
                };
                element! {
                    View(flex_direction: FlexDirection::Row) {
                        Text(content: lead, color: TuiTheme::DIM)
                        Text(content: line, color: TuiTheme::ERROR)
                    }
                }
                .into_any()
            })
            .collect();
        let footer = (!props.expanded && plus > 0).then(|| {
            let unit = if plus == 1 { "line" } else { "lines" };
            element! {
                View(flex_direction: FlexDirection::Row) {
                    Text(content: INDENT.to_string(), color: TuiTheme::DIM)
                    Text(
                        content: format!("\u{2026} +{plus} {unit} (ctrl+o to see all)"),
                        color: TuiTheme::DIM,
                    )
                }
            }
        });
        return element! {
            View(flex_direction: FlexDirection::Column) {
                #(body_rows)
                #(footer)
            }
        };
    }

    // M7-02: Edit/Write diff tools render a StructuredDiff when the paired
    // call inputs are present. Each StyledLine becomes a Row; each span a
    // Text wrapped in a View carrying its diff background.
    if is_diff_tool(&props.tool) && (props.old_string.is_some() || props.new_string.is_some()) {
        let lines = render_edit_write_diff_lines(
            &props.tool,
            props.old_string.as_deref(),
            props.new_string.as_deref(),
            props.file_path.as_deref(),
            props.theme_name,
        );
        let prefix = if props.focused { FOCUS_PREFIX } else { "" };
        let header = format!("{prefix}{MARKER}");
        let row_elements: Vec<AnyElement<'static>> = lines
            .into_iter()
            .map(|line| {
                let span_elements: Vec<AnyElement<'static>> = line
                    .spans
                    .into_iter()
                    .map(|s| {
                        let color = s.style.fg.to_iocraft();
                        let bg = s.style.bg.to_iocraft();
                        let weight = if s.style.bold {
                            Weight::Bold
                        } else {
                            Weight::Normal
                        };
                        element! {
                            View(background_color: bg) {
                                Text(content: s.text, color: color, weight: weight)
                            }
                        }
                        .into_any()
                    })
                    .collect();
                element! {
                    View(flex_direction: FlexDirection::Row) {
                        #(span_elements)
                    }
                }
                .into_any()
            })
            .collect();
        return element! {
            View(flex_direction: FlexDirection::Column) {
                Text(content: header, color: TuiTheme::DIM)
                #(row_elements)
            }
        };
    }

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
        // One row per parsed line. The flat span stream rejoins lines with
        // `\n` delimiter spans; `split_spans_into_line_rows` recovers the
        // per-line groups so multi-line Bash output renders on multiple visual
        // rows (a single flex Row of every span — including the delimiters —
        // collapsed them onto one). Color/weight preserved per span.
        let spans = render_user_tool_result_body_spans(props);
        let body_rows: Vec<AnyElement<'static>> = split_spans_into_line_rows(spans)
            .into_iter()
            .map(|line_spans| {
                let span_elements: Vec<AnyElement<'static>> = line_spans
                    .into_iter()
                    .map(|s| {
                        let color = s.style.fg.to_iocraft();
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
                element! {
                    View(flex_direction: FlexDirection::Row) {
                        #(span_elements)
                    }
                }
                .into_any()
            })
            .collect();
        return element! {
            View(flex_direction: FlexDirection::Column) {
                Text(content: header, color: TuiTheme::DIM)
                #(body_rows)
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
    fn marker_is_arc_gutter_bytes() {
        // "  " + U+23BF (0xE2 0x8E 0xBF) + "  " — matches MessageResponse.tsx.
        assert_eq!(MARKER.as_bytes(), &[0x20, 0x20, 0xE2, 0x8E, 0xBF, 0x20, 0x20]);
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
        let v =
            serde_json::json!({"content": [{"type":"text","text":"a"},{"type":"text","text":"b"}]});
        assert_eq!(body_text(&v), "a\nb");
    }

    #[test]
    fn tool_result_error_detects_error_key_and_wrapped_body() {
        // Top-level `"error"` string (turn_loop error paths).
        let v = serde_json::json!({"error": "boom"});
        assert_eq!(tool_result_error(&v).as_deref(), Some("boom"));
        // `<tool_use_error>`-wrapped content body (concurrent / pre-exec paths).
        let v = serde_json::json!({"content": "<tool_use_error>kaboom</tool_use_error>"});
        assert_eq!(
            tool_result_error(&v).as_deref(),
            Some("<tool_use_error>kaboom</tool_use_error>")
        );
        // Success result → None.
        let v = serde_json::json!({"content": "all good"});
        assert!(tool_result_error(&v).is_none());
    }

    #[test]
    fn format_tool_error_strips_tags_and_prefixes() {
        // Bare message → `Error: ` prefix added.
        assert_eq!(format_tool_error("boom", false), "Error: boom");
        // `<tool_use_error>` content is extracted.
        assert_eq!(
            format_tool_error("<tool_use_error>kaboom</tool_use_error>", false),
            "Error: kaboom"
        );
        // Already-prefixed → no double prefix.
        assert_eq!(format_tool_error("Error: already", false), "Error: already");
        assert_eq!(format_tool_error("Cancelled: x", false), "Cancelled: x");
        // `<error>` tags stripped, content kept.
        assert_eq!(format_tool_error("<error>oops</error>", false), "Error: oops");
        // Sandbox-violation block removed.
        assert_eq!(
            format_tool_error("nope<sandbox_violations>secret</sandbox_violations>", false),
            "Error: nope"
        );
        // InputValidationError collapses when non-verbose, full when verbose.
        assert_eq!(
            format_tool_error("InputValidationError: bad field", false),
            "Invalid tool parameters"
        );
        assert_eq!(
            format_tool_error("InputValidationError: bad field", true),
            "Error: InputValidationError: bad field"
        );
    }

    #[test]
    fn render_error_result_is_red_prefixed_and_capped() {
        // 12-line error, collapsed → first 10 lines + `… +2 lines` footer.
        let body = (0..12).map(|i| format!("L{i}")).collect::<Vec<_>>().join("\n");
        let props = UserToolResultProps {
            id: ToolUseId::from("t"),
            tool: "Bash".into(),
            result: serde_json::json!({ "error": body }),
            expanded: false,
            ..Default::default()
        };
        let s = render_user_tool_result_to_string(props);
        // `Error: L0` is the first body line behind the gutter.
        assert!(s.contains("Error: L0"), "got: {s}");
        // Capped at MAX_ERROR_LINES; L10/L11 dropped, footer reports +2.
        assert!(!s.contains("L11"), "got: {s}");
        assert!(s.contains("\u{2026} +2 lines (ctrl+o to see all)"), "got: {s}");
        assert!(s.starts_with(MARKER), "got: {s}");
    }

    #[test]
    fn extract_tag_requires_exact_tag_name() {
        // A longer tag name must not match.
        assert!(extract_tag("<tool_use_error_x>y</tool_use_error_x>", "tool_use_error").is_none());
        assert_eq!(
            extract_tag("<tool_use_error>y</tool_use_error>", "tool_use_error").as_deref(),
            Some("y")
        );
    }

    #[test]
    fn body_text_unwraps_bare_string_value() {
        // Replay path wraps content as a bare JSON string — no surrounding quotes.
        let v = serde_json::Value::String("hello\nworld".to_string());
        assert_eq!(body_text(&v), "hello\nworld");
    }

    #[test]
    fn tool_result_interrupted_detects_all_three_markers() {
        // REJECT_MESSAGE (bare string, the replay shape).
        let v = serde_json::Value::String(REJECT_MESSAGE.to_string());
        assert!(tool_result_interrupted(&v));
        // CANCEL_MESSAGE via the live `{"content": …}` shape.
        let v = serde_json::json!({ "content": CANCEL_MESSAGE });
        assert!(tool_result_interrupted(&v));
        // INTERRUPT_MESSAGE_FOR_TOOL_USE exact match.
        let v = serde_json::Value::String(INTERRUPT_MESSAGE_FOR_TOOL_USE.to_string());
        assert!(tool_result_interrupted(&v));
        // A normal result is not interrupted.
        let v = serde_json::json!({ "content": "ok" });
        assert!(!tool_result_interrupted(&v));
    }

    #[test]
    fn render_interrupted_is_dim_one_liner() {
        let props = UserToolResultProps {
            id: ToolUseId::from("t"),
            tool: "Bash".into(),
            result: serde_json::Value::String(REJECT_MESSAGE.to_string()),
            expanded: true, // even expanded, it stays the one-line interrupt notice
            ..Default::default()
        };
        let s = render_user_tool_result_to_string(props);
        assert_eq!(s, format!("{MARKER}Interrupted \u{00b7} What should Claude do instead?"));
        // The verbose REJECT_MESSAGE body must NOT leak through.
        assert!(!s.contains("STOP what you are doing"), "got: {s}");
    }
}

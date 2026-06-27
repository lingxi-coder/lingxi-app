//! `AssistantToolUseMessage` — header line `● Tool(input_preview)`.
//!
//! Literal locks (byte-for-byte from claude-code):
//!   - marker: `●` (U+25CF, 3-byte UTF-8 `0xE2 0x97 0x8F`)
//!     source: claude-code/src/constants/figures.ts `BLACK_CIRCLE`
//!   - focus prefix: `> ` (ASCII, 2 bytes)
//!     source: claude-code/src/components/MessageSelector.tsx
#![allow(clippy::needless_pass_by_value)]

use std::path::{Path, PathBuf};

use iocraft::prelude::*;
use protocol::ToolUseId;

use crate::theme::TuiTheme;

/// Marker glyph (ma-03 `figures.BLACK_CIRCLE`): `⏺` (U+23FA) on macOS, `●`
/// (U+25CF) elsewhere. Platform-conditional at compile time.
pub const MARKER: &str = if cfg!(target_os = "macos") {
    "\u{23FA}"
} else {
    "\u{25CF}"
};
/// Focus prefix prepended when this block is the focused one.
pub const FOCUS_PREFIX: &str = "> ";

/// Bash command preview caps (claude-code `BashTool/UI.tsx`).
const BASH_MAX_LINES: usize = 2;
const BASH_MAX_CHARS: usize = 160;

/// claude-code `userFacingName()` overrides — the label shown before `(…)`.
/// Most tools use their own name; `Glob` displays as `Search`.
#[must_use]
pub fn user_facing_name(tool: &str) -> &str {
    match tool {
        "Glob" => "Search",
        other => other,
    }
}

/// Shorten a path for display (claude-code `getDisplayPath`): relative to `cwd`
/// when the file is under it, else `~`-prefixed when under `$HOME`, else the
/// absolute path.
#[must_use]
pub fn get_display_path(path: &str, cwd: &Path) -> String {
    let p = Path::new(path);
    if !cwd.as_os_str().is_empty() {
        if let Ok(rel) = p.strip_prefix(cwd) {
            let s = rel.to_string_lossy();
            if !s.is_empty() {
                return s.into_owned();
            }
        }
    }
    if let Some(home) = dirs::home_dir() {
        if let Ok(rest) = p.strip_prefix(&home) {
            return format!("~/{}", rest.to_string_lossy());
        }
    }
    path.to_string()
}

/// Non-verbose Bash command preview: the command, truncated to
/// [`BASH_MAX_LINES`] lines then [`BASH_MAX_CHARS`] chars with a trailing `…`.
fn bash_preview(command: &str) -> String {
    let lines: Vec<&str> = command.split('\n').collect();
    let needs_line = lines.len() > BASH_MAX_LINES;
    let needs_char = command.chars().count() > BASH_MAX_CHARS;
    if !needs_line && !needs_char {
        return command.to_string();
    }
    let mut truncated = if needs_line {
        lines[..BASH_MAX_LINES].join("\n")
    } else {
        command.to_string()
    };
    if truncated.chars().count() > BASH_MAX_CHARS {
        truncated = truncated.chars().take(BASH_MAX_CHARS).collect();
    }
    format!("{}\u{2026}", truncated.trim())
}

/// Per-tool human preview shown inside `(…)` (claude-code's per-tool
/// `renderToolUseMessage`, non-verbose). `Some("")` → render the bare name with
/// no parentheses; `None` → the tool has no custom preview (caller falls back
/// to the compact-JSON preview).
#[must_use]
pub fn render_tool_use_message(
    tool: &str,
    input: &serde_json::Value,
    cwd: &Path,
) -> Option<String> {
    let s = |k: &str| input.get(k).and_then(serde_json::Value::as_str);
    match tool {
        "Read" => {
            let fp = s("file_path")?;
            let mut out = get_display_path(fp, cwd);
            if let Some(pages) = s("pages") {
                out.push_str(&format!(" \u{00b7} pages {pages}"));
            }
            Some(out)
        }
        "Edit" | "Write" | "MultiEdit" => Some(get_display_path(s("file_path")?, cwd)),
        "NotebookEdit" => Some(get_display_path(s("notebook_path")?, cwd)),
        "Bash" => Some(bash_preview(s("command")?)),
        "Grep" | "Glob" => {
            let pattern = s("pattern")?;
            Some(match s("path") {
                Some(path) => {
                    format!("pattern: \"{pattern}\", path: \"{}\"", get_display_path(path, cwd))
                }
                None => format!("pattern: \"{pattern}\""),
            })
        }
        "WebFetch" => Some(s("url")?.to_string()),
        "WebSearch" => Some(s("query")?.to_string()),
        _ => None,
    }
}

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
    /// Session cwd, used by [`get_display_path`] to shorten file-path previews
    /// (claude-code `getDisplayPath`). Empty (the default) renders absolute /
    /// `~`-relative paths only.
    pub cwd: PathBuf,
    /// (ma-02) Resolution state of the paired tool result, driving the `●` dot
    /// color exactly like claude-code's `ToolUseLoader`:
    ///   `None`        → unresolved (no result yet) → dim dot.
    ///   `Some(false)` → resolved success           → green dot.
    ///   `Some(true)`  → resolved error             → red dot.
    /// The dispatcher fills this from the paired `UserToolResult` (looked up by
    /// `id`) — `None` when no result has arrived for this `tool_use_id`.
    pub resolution: Option<bool>,
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
    let name = user_facing_name(&props.tool);
    // Per-tool human preview (claude-code `renderToolUseMessage`); fall back to
    // the compact-JSON preview for tools without a custom formatter. An empty
    // preview renders the bare name (no parentheses).
    let header = match render_tool_use_message(&props.tool, &props.input, &props.cwd) {
        Some(s) if s.is_empty() => format!("{prefix}{MARKER} {name}"),
        Some(s) => format!("{prefix}{MARKER} {name}({s})"),
        None => format!(
            "{prefix}{MARKER} {name}({})",
            single_line_json_preview(&props.input)
        ),
    };
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
pub(crate) fn single_line_json_preview(input: &serde_json::Value) -> String {
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

/// `●` dot color for a tool-use header, given the paired result's resolution
/// state (claude-code `ToolUseLoader`): dim while unresolved, green on success,
/// red on error. `None` = no result yet, `Some(is_error)` = resolved.
#[must_use]
pub fn resolution_dot_color(resolution: Option<bool>) -> Color {
    match resolution {
        None => TuiTheme::DIM,
        Some(false) => TuiTheme::SUCCESS,
        Some(true) => TuiTheme::ERROR,
    }
}

/// iocraft component. (ma-02) The header is split into per-segment styling
/// rather than one blanket cyan `Text`: the `●` dot follows the paired result's
/// resolution state (claude-code `ToolUseLoader`: dim while unresolved, green on
/// success, red on error), the tool NAME is bold in the default text color, and
/// the `(preview)` is the default color too.
#[component]
pub fn AssistantToolUseMessage(props: &AssistantToolUseProps) -> impl Into<AnyElement<'static>> {
    let prefix = if props.focused { FOCUS_PREFIX } else { "" };
    // `ToolUseLoader` dot color: dim when unresolved (`dimColor`), else the
    // success/error theme token with no dimming.
    let dot_color = resolution_dot_color(props.resolution);
    let name = user_facing_name(&props.tool).to_string();
    let preview = match render_tool_use_message(&props.tool, &props.input, &props.cwd) {
        Some(s) if s.is_empty() => None,
        Some(s) => Some(s),
        None => Some(single_line_json_preview(&props.input)),
    };
    let pretty = props
        .expanded
        .then(|| serde_json::to_string_pretty(&props.input).unwrap_or_else(|_| props.input.to_string()));
    element! {
        View(flex_direction: FlexDirection::Column) {
            View(flex_direction: FlexDirection::Row) {
                #((!prefix.is_empty()).then(|| element! {
                    Text(content: prefix.to_string(), color: TuiTheme::DIM)
                }))
                Text(content: MARKER.to_string(), color: dot_color)
                Text(content: format!(" {name}"), weight: Weight::Bold)
                #(preview.map(|p| element! { Text(content: format!("({p})")) }))
            }
            #(pretty.map(|p| element! { Text(content: p, color: TuiTheme::DIM) }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_is_the_platform_black_circle() {
        // (ma-03) `⏺` (U+23FA) on macOS, `●` (U+25CF) elsewhere.
        let glyph = if cfg!(target_os = "macos") {
            "\u{23FA}"
        } else {
            "\u{25CF}"
        };
        assert_eq!(MARKER, glyph);
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

    #[test]
    fn get_display_path_relative_under_cwd() {
        let cwd = Path::new("/home/u/proj");
        assert_eq!(get_display_path("/home/u/proj/src/x.rs", cwd), "src/x.rs");
        // Not under cwd, not under home → absolute.
        assert_eq!(get_display_path("/etc/hosts", cwd), "/etc/hosts");
    }

    #[test]
    fn per_tool_previews_match_claude_code() {
        let cwd = Path::new("/p");
        let m = |t: &str, v: serde_json::Value| render_tool_use_message(t, &v, cwd);
        assert_eq!(m("Read", serde_json::json!({"file_path": "/p/a.rs"})).unwrap(), "a.rs");
        assert_eq!(m("Edit", serde_json::json!({"file_path": "/p/b.rs"})).unwrap(), "b.rs");
        assert_eq!(m("Bash", serde_json::json!({"command": "ls -la"})).unwrap(), "ls -la");
        assert_eq!(
            m("Grep", serde_json::json!({"pattern": "foo"})).unwrap(),
            "pattern: \"foo\""
        );
        assert_eq!(
            m("Glob", serde_json::json!({"pattern": "*.rs", "path": "/p/src"})).unwrap(),
            "pattern: \"*.rs\", path: \"src\""
        );
        assert_eq!(m("WebFetch", serde_json::json!({"url": "https://x.y"})).unwrap(), "https://x.y");
        // Unknown tool → None (caller uses JSON fallback).
        assert!(m("SomeMcpTool", serde_json::json!({"a": 1})).is_none());
    }

    #[test]
    fn glob_user_facing_name_is_search() {
        assert_eq!(user_facing_name("Glob"), "Search");
        assert_eq!(user_facing_name("Read"), "Read");
    }

    #[test]
    fn bash_preview_truncates_long_command() {
        let cmd = "x".repeat(200);
        let out = bash_preview(&cmd);
        assert!(out.ends_with('\u{2026}'));
        assert!(out.chars().count() <= BASH_MAX_CHARS + 1);
    }

    #[test]
    fn header_uses_per_tool_preview_and_name() {
        let s = render_assistant_tool_use_to_string(AssistantToolUseProps {
            id: ToolUseId::from("t"),
            tool: "Glob".into(),
            input: serde_json::json!({"pattern": "*.rs"}),
            expanded: false,
            focused: false,
            cwd: PathBuf::from("/p"),
            resolution: None,
        });
        assert_eq!(s, format!("{MARKER} Search(pattern: \"*.rs\")"));
    }

    #[test]
    fn resolution_maps_to_dot_color() {
        // (ma-02) claude-code `ToolUseLoader`: dim unresolved / green success /
        // red error. The three states map to three distinct theme colors.
        assert_eq!(resolution_dot_color(None), TuiTheme::DIM);
        assert_eq!(resolution_dot_color(Some(false)), TuiTheme::SUCCESS);
        assert_eq!(resolution_dot_color(Some(true)), TuiTheme::ERROR);
    }
}

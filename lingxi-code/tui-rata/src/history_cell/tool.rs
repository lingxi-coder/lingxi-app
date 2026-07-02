//! Per-variant history cells for the tool-side messages: assistant tool
//! calls, user tool results, bash/local command output, and the grouped /
//! collapsed read-search folds.
//!
//! Split out of `message.rs` in the message-cells phase (plan Phase 9). The
//! styled-line renderers here are the single source for these variants: the
//! cells consume them through [`StyledCell`], and the legacy
//! [`crate::message::render_message`] dispatcher delegates to them so its
//! output stays line-identical to the pre-split renderer.

use tui_core::render::{SpanStyle, StyledLine, StyledSpan};
use tui_core::theme::Theme;

use super::{colored_lines, dim_span, plain_lines, truncate, StyledCell};

/// A tool call: `● {tool}` header + arguments. Collapsed → a dim, truncated
/// one-line summary; verbose → the pretty-printed JSON input, dim-indented.
pub(crate) fn tool_use_lines(
    tool: &str,
    input: &serde_json::Value,
    theme: &Theme,
    verbose: bool,
) -> Vec<StyledLine> {
    let header = StyledLine {
        spans: vec![
            StyledSpan::styled(
                "● ".to_string(),
                SpanStyle {
                    fg: theme.success,
                    ..SpanStyle::default()
                },
            ),
            StyledSpan::plain(tool.to_string()),
        ],
    };
    let mut out = vec![header];
    if verbose {
        let pretty = serde_json::to_string_pretty(input).unwrap_or_else(|_| input.to_string());
        for line in pretty.split('\n') {
            out.push(StyledLine {
                spans: vec![dim_span(format!("  {line}"), theme)],
            });
        }
    } else {
        out.push(StyledLine {
            spans: vec![dim_span(
                format!("  {}", truncate(&input.to_string(), 100)),
                theme,
            )],
        });
    }
    out
}

/// A tool result: `⎿ {summary}` — the string content when present, else
/// compact JSON.
pub(crate) fn tool_result_lines(result: &serde_json::Value, theme: &Theme) -> Vec<StyledLine> {
    let summary = if let Some(s) = result.as_str() {
        s.to_string()
    } else if let Some(s) = result.get("content").and_then(serde_json::Value::as_str) {
        s.to_string()
    } else {
        result.to_string()
    };
    vec![StyledLine {
        spans: vec![dim_span(format!("  ⎿ {}", truncate(&summary, 100)), theme)],
    }]
}

/// Bash / local-command output: stdout lines, then error-colored stderr.
pub(crate) fn command_output_lines(stdout: &str, stderr: &str, theme: &Theme) -> Vec<StyledLine> {
    let mut out = plain_lines(stdout);
    if !stderr.is_empty() {
        out.extend(colored_lines(stderr, theme.error));
    }
    out
}

/// A grouped tool-use block: `● {tool} (×{count})` header. Collapsed → header
/// only; verbose → header + each child's truncated input → result line.
pub(crate) fn group_tool_use_lines(
    tool: &str,
    entries: &[(serde_json::Value, serde_json::Value)],
    theme: &Theme,
    verbose: bool,
) -> Vec<StyledLine> {
    let mut out = vec![StyledLine {
        spans: vec![
            StyledSpan::styled(
                "● ".to_string(),
                SpanStyle {
                    fg: theme.success,
                    ..SpanStyle::default()
                },
            ),
            StyledSpan::plain(format!("{tool} (×{})", entries.len())),
        ],
    }];
    if verbose {
        for (input, result) in entries {
            out.push(StyledLine {
                spans: vec![dim_span(
                    format!(
                        "  ⎿ {} → {}",
                        truncate(&input.to_string(), 60),
                        truncate(&result.to_string(), 60)
                    ),
                    theme,
                )],
            });
        }
    }
    out
}

/// The collapsed Read/Search fold: a dim `Read/Search (N results)` count line.
pub(crate) fn collapsed_read_search_lines(entries: &[String], theme: &Theme) -> Vec<StyledLine> {
    colored_lines(
        &format!("Read/Search ({} results)", entries.len()),
        theme.dim,
    )
}

/// [`RenderedMessage::AssistantToolUse`](tui_core::message::RenderedMessage::AssistantToolUse)
/// — the `● {tool}` call header + collapsed/verbose input.
#[derive(Debug)]
pub struct ToolUseCell {
    tool: String,
    input: serde_json::Value,
}

impl ToolUseCell {
    /// Wrap one tool call.
    #[must_use]
    pub fn new(tool: String, input: serde_json::Value) -> Self {
        Self { tool, input }
    }

    /// The tool name.
    #[must_use]
    pub fn tool(&self) -> &str {
        &self.tool
    }
}

impl StyledCell for ToolUseCell {
    fn styled_lines(&self, _width: usize, theme: &Theme, verbose: bool) -> Vec<StyledLine> {
        tool_use_lines(&self.tool, &self.input, theme, verbose)
    }
}

/// [`RenderedMessage::UserToolResult`](tui_core::message::RenderedMessage::UserToolResult)
/// — the `  ⎿ {summary}` result line.
#[derive(Debug)]
pub struct ToolResultCell {
    result: serde_json::Value,
    old_string: Option<String>,
    new_string: Option<String>,
    file_path: Option<String>,
}

impl ToolResultCell {
    /// Wrap one tool result (`old_string`/`new_string`/`file_path` carry the
    /// paired Edit/Write inputs for diff rendering when present).
    #[must_use]
    pub fn new(
        result: serde_json::Value,
        old_string: Option<String>,
        new_string: Option<String>,
        file_path: Option<String>,
    ) -> Self {
        Self {
            result,
            old_string,
            new_string,
            file_path,
        }
    }
}

impl StyledCell for ToolResultCell {
    fn styled_lines(&self, _width: usize, theme: &Theme, _verbose: bool) -> Vec<StyledLine> {
        let _ = (&self.old_string, &self.new_string, &self.file_path);
        tool_result_lines(&self.result, theme)
    }
}

/// [`RenderedMessage::UserBashOutput`](tui_core::message::RenderedMessage::UserBashOutput)
/// and
/// [`RenderedMessage::UserLocalCommandOutput`](tui_core::message::RenderedMessage::UserLocalCommandOutput)
/// — stdout + error-colored stderr.
#[derive(Debug)]
pub struct CommandOutputCell {
    stdout: String,
    stderr: String,
}

impl CommandOutputCell {
    /// Wrap captured command output.
    #[must_use]
    pub fn new(stdout: String, stderr: String) -> Self {
        Self { stdout, stderr }
    }
}

impl StyledCell for CommandOutputCell {
    fn styled_lines(&self, _width: usize, theme: &Theme, _verbose: bool) -> Vec<StyledLine> {
        command_output_lines(&self.stdout, &self.stderr, theme)
    }
}

/// [`RenderedMessage::GroupedToolUse`](tui_core::message::RenderedMessage::GroupedToolUse)
/// — the `● {tool} (×N)` fold of consecutive same-tool calls.
#[derive(Debug)]
pub struct GroupedToolUseCell {
    tool: String,
    entries: Vec<(serde_json::Value, serde_json::Value)>,
}

impl GroupedToolUseCell {
    /// Wrap a fold of `(input, result)` pairs sharing one tool.
    #[must_use]
    pub fn new(tool: String, entries: Vec<(serde_json::Value, serde_json::Value)>) -> Self {
        Self { tool, entries }
    }
}

impl StyledCell for GroupedToolUseCell {
    fn styled_lines(&self, _width: usize, theme: &Theme, verbose: bool) -> Vec<StyledLine> {
        group_tool_use_lines(&self.tool, &self.entries, theme, verbose)
    }
}

/// [`RenderedMessage::CollapsedReadSearch`](tui_core::message::RenderedMessage::CollapsedReadSearch)
/// — the `Read/Search (N results)` fold of read/search/list runs.
#[derive(Debug)]
pub struct CollapsedReadSearchCell {
    entries: Vec<String>,
}

impl CollapsedReadSearchCell {
    /// Wrap the fold's per-entry display lines.
    #[must_use]
    pub fn new(entries: Vec<String>) -> Self {
        Self { entries }
    }
}

impl StyledCell for CollapsedReadSearchCell {
    fn styled_lines(&self, _width: usize, theme: &Theme, _verbose: bool) -> Vec<StyledLine> {
        collapsed_read_search_lines(&self.entries, theme)
    }
}

#[cfg(test)]
mod tests {
    use super::super::{HistoryCell, RenderMode};
    use super::*;

    fn plain(cell: &dyn HistoryCell, verbose: bool) -> Vec<String> {
        cell.display_lines(
            80,
            &Theme::dark(),
            RenderMode {
                raw: false,
                verbose,
            },
        )
        .iter()
        .map(ToString::to_string)
        .collect()
    }

    fn rata(color: tui_core::render::StyleColor) -> ratatui::style::Color {
        crate::style_adapter::to_ratatui(color)
    }

    #[test]
    fn tool_use_cell_collapsed_renders_header_and_truncated_input() {
        let cell = ToolUseCell::new(
            "Read".to_string(),
            serde_json::json!({"file_path": "src/lib.rs"}),
        );
        let lines = plain(&cell, false);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "● Read");
        assert_eq!(lines[1], "  {\"file_path\":\"src/lib.rs\"}");
        assert_eq!(cell.tool(), "Read");
        // Header marker is success-colored; the tool name is unstyled.
        let styled = cell.display_lines(80, &Theme::dark(), RenderMode::default());
        assert_eq!(
            styled[0].spans[0].style.fg,
            Some(rata(Theme::dark().success))
        );
        assert_eq!(styled[0].spans[1].style.fg, None);
        // The input summary is dim.
        assert_eq!(styled[1].spans[0].style.fg, Some(rata(Theme::dark().dim)));
    }

    #[test]
    fn tool_use_cell_truncates_long_input_at_100_chars() {
        let cell = ToolUseCell::new(
            "Bash".to_string(),
            serde_json::json!({"command": "x".repeat(200)}),
        );
        let lines = plain(&cell, false);
        let summary = lines[1].trim_start();
        assert_eq!(summary.chars().count(), 100, "99 chars + ellipsis");
        assert!(summary.ends_with('…'), "got: {summary}");
    }

    #[test]
    fn tool_use_cell_verbose_pretty_prints_json_input() {
        let cell = ToolUseCell::new(
            "Read".to_string(),
            serde_json::json!({"file_path": "src/lib.rs", "limit": 10}),
        );
        let lines = plain(&cell, true);
        assert!(
            lines.len() > 2,
            "pretty JSON spans multiple lines: {lines:?}"
        );
        assert_eq!(lines[0], "● Read");
        assert!(lines[1].starts_with("  {"), "indented JSON: {lines:?}");
        assert!(
            lines.iter().any(|l| l.contains("\"limit\": 10")),
            "pretty-printed field: {lines:?}"
        );
    }

    #[test]
    fn tool_result_cell_prefers_string_content_field() {
        let cell = ToolResultCell::new(
            serde_json::json!({"content": "hello world"}),
            None,
            None,
            None,
        );
        assert_eq!(plain(&cell, false), vec!["  ⎿ hello world".to_string()]);
        let styled = cell.display_lines(80, &Theme::dark(), RenderMode::default());
        assert_eq!(styled[0].spans[0].style.fg, Some(rata(Theme::dark().dim)));
    }

    #[test]
    fn tool_result_cell_unwraps_bare_string_and_flattens_newlines() {
        // A bare-string result (replay path) renders directly, newlines
        // flattened into the one-line summary.
        let cell = ToolResultCell::new(
            serde_json::Value::String("line one\nline two".to_string()),
            None,
            None,
            None,
        );
        assert_eq!(
            plain(&cell, false),
            vec!["  ⎿ line one line two".to_string()]
        );
    }

    #[test]
    fn tool_result_cell_falls_back_to_compact_json() {
        let cell = ToolResultCell::new(serde_json::json!({"status": "ok"}), None, None, None);
        assert_eq!(
            plain(&cell, false),
            vec!["  ⎿ {\"status\":\"ok\"}".to_string()]
        );
    }

    #[test]
    fn command_output_cell_renders_stdout_then_error_colored_stderr() {
        let cell = CommandOutputCell::new("out line".to_string(), "err line".to_string());
        assert_eq!(
            plain(&cell, false),
            vec!["out line".to_string(), "err line".to_string()]
        );
        let styled = cell.display_lines(80, &Theme::dark(), RenderMode::default());
        assert_eq!(styled[0].spans[0].style.fg, None, "stdout keeps default fg");
        assert_eq!(styled[1].spans[0].style.fg, Some(rata(Theme::dark().error)));

        // Empty stderr adds nothing.
        let out_only = CommandOutputCell::new("just out".to_string(), String::new());
        assert_eq!(plain(&out_only, false), vec!["just out".to_string()]);
    }

    #[test]
    fn grouped_tool_use_cell_collapsed_is_header_only() {
        let cell = GroupedToolUseCell::new(
            "Read".to_string(),
            vec![
                (serde_json::json!({"f": "a"}), serde_json::json!("ok")),
                (serde_json::json!({"f": "b"}), serde_json::json!("ok")),
            ],
        );
        assert_eq!(plain(&cell, false), vec!["● Read (×2)".to_string()]);
    }

    #[test]
    fn grouped_tool_use_cell_verbose_lists_child_input_result_pairs() {
        let cell = GroupedToolUseCell::new(
            "Read".to_string(),
            vec![
                (serde_json::json!({"f": "a"}), serde_json::json!("ok")),
                (serde_json::json!({"f": "b"}), serde_json::json!("ok")),
            ],
        );
        let lines = plain(&cell, true);
        assert_eq!(
            lines,
            vec![
                "● Read (×2)".to_string(),
                "  ⎿ {\"f\":\"a\"} → \"ok\"".to_string(),
                "  ⎿ {\"f\":\"b\"} → \"ok\"".to_string(),
            ]
        );
    }

    #[test]
    fn collapsed_read_search_cell_renders_dim_count_line() {
        let cell =
            CollapsedReadSearchCell::new(vec!["Read a.rs".to_string(), "Grep foo".to_string()]);
        assert_eq!(
            plain(&cell, false),
            vec!["Read/Search (2 results)".to_string()]
        );
        let styled = cell.display_lines(80, &Theme::dark(), RenderMode::default());
        assert_eq!(styled[0].spans[0].style.fg, Some(rata(Theme::dark().dim)));
    }
}

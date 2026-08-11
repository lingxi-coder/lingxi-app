//! Per-variant history cells for the tool-side messages: assistant tool
//! calls, user tool results, bash/local command output, and the grouped /
//! collapsed read-search folds.
//!
//! Split out of `message.rs` in the message-cells phase (plan Phase 9). The
//! styled-line renderers here are the single source for these variants: the
//! cells consume them through [`StyledCell`], and the legacy
//! [`crate::message::render_message`] dispatcher delegates to them so its
//! output stays line-identical to the pre-split renderer.

use tui_core::render::ansi::parse_ansi;
use tui_core::render::{diff, SpanStyle, StyleColor, StyledLine, StyledSpan};
use tui_core::theme::{Theme, ThemeName};
use tui_core::tool_display;

use super::{colored_lines, dim_span, truncate, StyledCell};

/// Tool-call header marker (claude-code `figures.BLACK_CIRCLE`): `⏺` (U+23FA)
/// on macOS, `●` (U+25CF) elsewhere — platform-conditional at compile time,
/// mirroring `message::ASSISTANT_MARKER` and the iocraft
/// `assistant_tool_use::MARKER`. (The grouped-fold header stays an
/// unconditional `●` to match the byte-locked iocraft `grouped_tool_use`.)
const TOOL_MARKER: &str = if cfg!(target_os = "macos") {
    "\u{23FA} "
} else {
    "\u{25CF} "
};

/// A tool call: `⏺ {verb}({arg})` header + arguments.
///
/// The header is the shared, parameterized one from
/// `tui_core::tool_display::header` — `Update(src/host.rs)` rather than the
/// bare `Edit` this used to print. Collapsed → the header plus the tool's
/// sub-line (a Bash `$ command`) when it has one, or — when the header names
/// no argument AND there is no sub-line — the truncated input JSON, so a call
/// is never printed without its arguments; verbose → the pretty-printed JSON
/// input, dim-indented.
pub(crate) fn tool_use_lines(
    tool: &str,
    input: &serde_json::Value,
    theme: &Theme,
    verbose: bool,
) -> Vec<StyledLine> {
    let derived = tool_display::tool_header(tool, input);
    let header_carries_an_argument = derived.primary.is_some();
    let header = StyledLine {
        spans: vec![
            StyledSpan::styled(
                TOOL_MARKER.to_string(),
                SpanStyle {
                    fg: theme.success,
                    ..SpanStyle::default()
                },
            ),
            StyledSpan::plain(derived.title()),
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
    } else if let Some(sub) = derived.sub_line {
        out.push(StyledLine {
            spans: vec![dim_span(
                format!("  {} {}", sub.prefix, truncate(&sub.text, 100)),
                theme,
            )],
        });
    } else if !header_carries_an_argument {
        // The pre-shared-header fallback, restored for the calls that lost it.
        // `sub_line` is set only for the shell family and `primary` only from
        // the header table / `GENERIC_PRIMARY_KEYS`, so a tool with NEITHER —
        // `TodoWrite`, or an MCP tool whose input carries none of the generic
        // keys — rendered with no argument information at all. Tools whose
        // header already names their argument (`Read(src/lib.rs)`) keep the
        // one-line form the shared header bought.
        out.push(StyledLine {
            spans: vec![dim_span(
                format!("  {}", truncate(&input.to_string(), 100)),
                theme,
            )],
        });
    }
    out
}

/// A tool result. Default: a dim `⎿  {summary}` one-liner — the string content
/// when present, else compact JSON. An Edit/Write result carrying the paired
/// diff inputs (`old_string`/`new_string`) renders a structured diff instead
/// (plan Phase 9 step 5): a dim added/removed summary header + the diff rows.
pub(crate) fn tool_result_lines(
    tool: &str,
    input: Option<&serde_json::Value>,
    result: &serde_json::Value,
    old_string: Option<&str>,
    new_string: Option<&str>,
    file_path: Option<&str>,
    width: usize,
    theme: &Theme,
) -> Vec<StyledLine> {
    if old_string.is_some() || new_string.is_some() {
        return edit_write_diff_lines(old_string, new_string, file_path, width, theme);
    }
    // The shared headline (`Read 12 lines`, `Found 3 files`, the first line of
    // a command's output) — the same string the clients render. Falls back to
    // the raw payload for a tool with no headline rule.
    let is_error = tool_display::result_is_error(result);
    let summary = tool_display::result_headline(tool, input, result, is_error).unwrap_or_else(
        || {
            if let Some(s) = result.as_str() {
                s.to_string()
            } else if let Some(s) = result.get("content").and_then(serde_json::Value::as_str) {
                s.to_string()
            } else {
                result.to_string()
            }
        },
    );
    vec![StyledLine {
        spans: vec![dim_span(format!("  ⎿  {}", truncate(&summary, 100)), theme)],
    }]
}

/// The Edit/Write structured diff: a dim `  ⎿  Added N line(s)[, removed M
/// line(s)]` gutter header (iocraft `added_removed_header` parity), then the
/// diff rows from `tui_core::render::diff` — Write is a pure add (`old` =
/// empty), `file_path` drives syntax detection, and changed rows are
/// background-padded to `width`. The diff's syntax scheme is the renderer
/// default (dark), matching the markdown code-theme choice.
fn edit_write_diff_lines(
    old_string: Option<&str>,
    new_string: Option<&str>,
    file_path: Option<&str>,
    width: usize,
    theme: &Theme,
) -> Vec<StyledLine> {
    let old = old_string.unwrap_or("");
    let new = new_string.unwrap_or("");
    let (additions, removals) = diff::diff_stats(old, new);
    let summary = added_removed_header(additions, removals).unwrap_or_default();
    let mut out = vec![StyledLine {
        spans: vec![dim_span(format!("  ⎿  {summary}"), theme)],
    }];
    out.extend(diff::render_with_width(
        old,
        new,
        file_path,
        ThemeName::default(),
        width,
    ));
    out
}

// Moved to `tui_core::tool_display::result` so the terminal and the clients
// spell the edit summary identically. Re-exported under the old path so
// `super::added_removed_header` and its byte-lock test keep resolving.
pub(crate) use tui_core::tool_display::result::added_removed_header;

/// Bash / local-command output: ANSI-parsed stdout lines (SGR colors and
/// attributes preserved — the bodies carry raw escape codes), then stderr,
/// also ANSI-parsed, with spans that carry no explicit ANSI foreground tinted
/// the error color (preserving the plain-text stderr coloring).
pub(crate) fn command_output_lines(stdout: &str, stderr: &str, theme: &Theme) -> Vec<StyledLine> {
    let mut out = parse_ansi(stdout);
    if !stderr.is_empty() {
        out.extend(error_tinted(parse_ansi(stderr), theme.error));
    }
    out
}

/// Tint every span without an explicit foreground color with `error`.
fn error_tinted(lines: Vec<StyledLine>, error: StyleColor) -> Vec<StyledLine> {
    lines
        .into_iter()
        .map(|mut line| {
            for span in &mut line.spans {
                if span.style.fg == StyleColor::Default {
                    span.style.fg = error;
                }
            }
            line
        })
        .collect()
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
                        "  ⎿  {} → {}",
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

/// The collapsed Read/Search fold: a dim `Read/Search (N results)` count
/// line; verbose expands the per-entry display lines under a `⎿  ` gutter
/// (tui-core `CollapsedReadSearch.entries` contract: "shown when expanded").
#[allow(clippy::too_many_arguments)]
pub(crate) fn collapsed_read_search_lines(
    search_count: u64,
    read_count: u64,
    list_count: u64,
    repl_count: u64,
    mcp_call_count: u64,
    mcp_server_names: &[String],
    bash_count: u64,
    memory_write_count: u64,
    is_active: bool,
    latest_hint: Option<&str>,
    entries: &[String],
    theme: &Theme,
    verbose: bool,
) -> Vec<StyledLine> {
    let summary = tui_core::collapse::search_read_summary_text_full(
        search_count,
        read_count,
        list_count,
        repl_count,
        mcp_call_count,
        mcp_server_names,
        bash_count,
        memory_write_count,
        is_active,
    );
    let mut out = if summary.is_empty() {
        Vec::new()
    } else {
        colored_lines(&summary, theme.dim)
    };
    // The dim `⎿ <latest read>` hint renders ONLY while the group is active
    // (CollapsedReadSearchContent.tsx parity).
    if is_active {
        if let Some(hint) = latest_hint {
            out.push(StyledLine {
                spans: vec![dim_span(format!("  ⎿  {hint}"), theme)],
            });
        }
    }
    if verbose {
        for entry in entries {
            out.push(StyledLine {
                spans: vec![dim_span(format!("  ⎿  {entry}"), theme)],
            });
        }
    }
    out
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
/// — the `  ⎿  {summary}` result line, or the Edit/Write structured diff when
/// the paired `old_string`/`new_string` inputs are present.
#[derive(Debug)]
pub struct ToolResultCell {
    tool: String,
    result: serde_json::Value,
    old_string: Option<String>,
    new_string: Option<String>,
    file_path: Option<String>,
    input: Option<serde_json::Value>,
}

impl ToolResultCell {
    /// Wrap one tool result (`old_string`/`new_string`/`file_path` carry the
    /// paired Edit/Write inputs for diff rendering when present; `tool` and
    /// `input` drive the shared result headline).
    #[must_use]
    pub fn new(
        tool: String,
        result: serde_json::Value,
        old_string: Option<String>,
        new_string: Option<String>,
        file_path: Option<String>,
        input: Option<serde_json::Value>,
    ) -> Self {
        Self {
            tool,
            result,
            old_string,
            new_string,
            file_path,
            input,
        }
    }
}

impl StyledCell for ToolResultCell {
    fn styled_lines(&self, width: usize, theme: &Theme, _verbose: bool) -> Vec<StyledLine> {
        tool_result_lines(
            &self.tool,
            self.input.as_ref(),
            &self.result,
            self.old_string.as_deref(),
            self.new_string.as_deref(),
            self.file_path.as_deref(),
            width,
            theme,
        )
    }
}

/// [`RenderedMessage::UserBashOutput`](tui_core::message::RenderedMessage::UserBashOutput)
/// and
/// [`RenderedMessage::UserLocalCommandOutput`](tui_core::message::RenderedMessage::UserLocalCommandOutput)
/// — ANSI-parsed stdout + error-tinted ANSI-parsed stderr.
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
    search_count: u64,
    read_count: u64,
    list_count: u64,
    repl_count: u64,
    mcp_call_count: u64,
    mcp_server_names: Vec<String>,
    bash_count: u64,
    memory_write_count: u64,
    is_active: bool,
    latest_hint: Option<String>,
    entries: Vec<String>,
}

impl CollapsedReadSearchCell {
    /// Wrap the fold's counts + verbose entries.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        search_count: u64,
        read_count: u64,
        list_count: u64,
        repl_count: u64,
        mcp_call_count: u64,
        mcp_server_names: Vec<String>,
        bash_count: u64,
        memory_write_count: u64,
        is_active: bool,
        latest_hint: Option<String>,
        entries: Vec<String>,
    ) -> Self {
        Self {
            search_count,
            read_count,
            list_count,
            repl_count,
            mcp_call_count,
            mcp_server_names,
            bash_count,
            memory_write_count,
            is_active,
            latest_hint,
            entries,
        }
    }
}

impl StyledCell for CollapsedReadSearchCell {
    fn styled_lines(&self, _width: usize, theme: &Theme, verbose: bool) -> Vec<StyledLine> {
        collapsed_read_search_lines(
            self.search_count,
            self.read_count,
            self.list_count,
            self.repl_count,
            self.mcp_call_count,
            &self.mcp_server_names,
            self.bash_count,
            self.memory_write_count,
            self.is_active,
            self.latest_hint.as_deref(),
            &self.entries,
            theme,
            verbose,
        )
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
        // The header is now parameterized (`Read(src/lib.rs)`), so a tool with
        // no sub-line renders as a single line instead of a header plus a
        // truncated raw-JSON line.
        assert_eq!(lines, vec![format!("{TOOL_MARKER}Read(src/lib.rs)")]);
        assert_eq!(cell.tool(), "Read");
        // Header marker is success-colored; the title is unstyled.
        let styled = cell.display_lines(80, &Theme::dark(), RenderMode::default());
        assert_eq!(
            styled[0].spans[0].style.fg,
            Some(rata(Theme::dark().success))
        );
        assert_eq!(styled[0].spans[1].style.fg, None);
    }

    /// REGRESSION: the shared-header extraction replaced the unconditional
    /// truncated-input line with `else if let Some(sub) = derived.sub_line`.
    /// `sub_line` is set only for the shell family and `primary` only from the
    /// header table / `GENERIC_PRIMARY_KEYS`, so a tool with NEITHER —
    /// `TodoWrite`, or an MCP tool whose input carries none of the generic
    /// keys — rendered its call with no argument information at all.
    #[test]
    fn a_tool_with_no_header_argument_still_shows_its_input() {
        let cell = ToolUseCell::new(
            "TodoWrite".to_string(),
            serde_json::json!({"todos": [{"content": "Ship it"}]}),
        );
        let lines = plain(&cell, false);
        assert_eq!(lines.len(), 2, "header + arguments, got {lines:?}");
        assert!(
            lines[1].contains("Ship it"),
            "the call's arguments must be visible: {lines:?}"
        );

        // Same for an MCP tool whose input has none of the generic keys.
        let mcp = ToolUseCell::new(
            "mcp__srv__thing".to_string(),
            serde_json::json!({"widget_id": 42}),
        );
        let lines = plain(&mcp, false);
        assert_eq!(lines.len(), 2, "header + arguments, got {lines:?}");
        assert!(lines[1].contains("widget_id"), "{lines:?}");
    }

    #[test]
    fn tool_use_cell_renders_a_bash_command_sub_line_truncated_at_100_chars() {
        let cell = ToolUseCell::new(
            "Bash".to_string(),
            serde_json::json!({"command": "x".repeat(200)}),
        );
        let lines = plain(&cell, false);
        assert_eq!(lines[0], format!("{TOOL_MARKER}Running 1 shell command…"));
        // Bash is the one tool with a sub-line: `$ {command}`, truncated.
        let sub = lines[1].trim_start();
        assert!(sub.starts_with("$ "), "got: {sub}");
        let command = sub.trim_start_matches("$ ");
        assert_eq!(command.chars().count(), 100, "99 chars + ellipsis");
        assert!(command.ends_with('…'), "got: {command}");
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
        // `limit: 10` makes it a partial read, which the header qualifies.
        assert_eq!(
            lines[0],
            format!("{TOOL_MARKER}Read(src/lib.rs) (lines 1-10)")
        );
        assert!(lines[1].starts_with("  {"), "indented JSON: {lines:?}");
        assert!(
            lines.iter().any(|l| l.contains("\"limit\": 10")),
            "pretty-printed field: {lines:?}"
        );
    }

    #[test]
    fn tool_result_cell_prefers_string_content_field() {
        let cell = ToolResultCell::new(
            "Whatever".to_string(),
            serde_json::json!({"content": "hello world"}),
            None,
            None,
            None,
            None,
        );
        assert_eq!(plain(&cell, false), vec!["  ⎿  hello world".to_string()]);
        let styled = cell.display_lines(80, &Theme::dark(), RenderMode::default());
        assert_eq!(styled[0].spans[0].style.fg, Some(rata(Theme::dark().dim)));
    }

    #[test]
    fn tool_result_cell_unwraps_bare_string_and_flattens_newlines() {
        // A bare-string result (replay path) renders directly, newlines
        // flattened into the one-line summary.
        let cell = ToolResultCell::new(
            "Whatever".to_string(),
            serde_json::Value::String("line one\nline two".to_string()),
            None,
            None,
            None,
            None,
        );
        assert_eq!(
            plain(&cell, false),
            vec!["  ⎿  line one line two".to_string()]
        );
    }

    #[test]
    fn tool_result_cell_falls_back_to_compact_json() {
        let cell = ToolResultCell::new(
            "Whatever".to_string(),
            serde_json::json!({"status": "ok"}),
            None,
            None,
            None,
            None,
        );
        assert_eq!(
            plain(&cell, false),
            vec!["  ⎿  {\"status\":\"ok\"}".to_string()]
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
    fn command_output_cell_preserves_ansi_colors_in_stdout() {
        // The bash-output bodies carry raw SGR codes (tui-core contract);
        // they must render as colored spans, not literal escape bytes.
        let cell = CommandOutputCell::new(
            "\u{1b}[31mred\u{1b}[0m plain\nsecond".to_string(),
            String::new(),
        );
        let lines = plain(&cell, false);
        assert_eq!(lines, vec!["red plain".to_string(), "second".to_string()]);
        let styled = cell.display_lines(80, &Theme::dark(), RenderMode::default());
        let red = &styled[0].spans[0];
        assert_eq!(red.content.as_ref(), "red");
        assert_eq!(red.style.fg, Some(ratatui::style::Color::Red));
        let tail = &styled[0].spans[1];
        assert_eq!(tail.content.as_ref(), " plain");
        assert_eq!(tail.style.fg, None, "unstyled stdout keeps default fg");
    }

    #[test]
    fn command_output_cell_stderr_keeps_ansi_colors_and_tints_plain_spans() {
        let cell =
            CommandOutputCell::new(String::new(), "\u{1b}[32mgreen\u{1b}[0m tail".to_string());
        let styled = cell.display_lines(80, &Theme::dark(), RenderMode::default());
        assert_eq!(styled.len(), 1);
        let green = &styled[0].spans[0];
        assert_eq!(green.content.as_ref(), "green");
        assert_eq!(
            green.style.fg,
            Some(ratatui::style::Color::Green),
            "explicit ANSI color wins over the error tint"
        );
        let tail = &styled[0].spans[1];
        assert_eq!(tail.content.as_ref(), " tail");
        assert_eq!(
            tail.style.fg,
            Some(rata(Theme::dark().error)),
            "plain stderr spans keep the error tint"
        );
    }

    #[test]
    fn added_removed_header_variants() {
        // Ported from the iocraft `user_tool_result` renderer, byte-for-byte.
        assert_eq!(added_removed_header(0, 0), None);
        assert_eq!(added_removed_header(1, 0).as_deref(), Some("Added 1 line"));
        assert_eq!(added_removed_header(3, 0).as_deref(), Some("Added 3 lines"));
        // Sole removal clause -> capitalized "Removed".
        assert_eq!(
            added_removed_header(0, 1).as_deref(),
            Some("Removed 1 line")
        );
        assert_eq!(
            added_removed_header(0, 2).as_deref(),
            Some("Removed 2 lines")
        );
        // Both present -> lowercase "removed" joined with ", ".
        assert_eq!(
            added_removed_header(2, 3).as_deref(),
            Some("Added 2 lines, removed 3 lines")
        );
    }

    #[test]
    fn tool_result_cell_renders_edit_diff_when_old_and_new_present() {
        let cell = ToolResultCell::new(
            "Edit".to_string(),
            serde_json::json!({"content": "ok"}),
            Some("alpha\nbeta".to_string()),
            Some("alpha\ngamma".to_string()),
            Some("x.rs".to_string()),
            None,
        );
        let lines = plain(&cell, false);
        assert_eq!(
            lines[0], "  ⎿  Added 1 line, removed 1 line",
            "summary header inside the gutter: {lines:?}"
        );
        let all = lines.join("\n");
        assert!(all.contains("beta"), "removed line shown: {all}");
        assert!(all.contains("gamma"), "added line shown: {all}");
        // Diff rows carry the add/remove line backgrounds.
        let styled = cell.display_lines(80, &Theme::dark(), RenderMode::default());
        assert!(
            styled
                .iter()
                .flat_map(|l| l.spans.iter())
                .any(|s| s.style.bg.is_some()),
            "diff rows carry background colors"
        );
        // The header line is dim, like every other result gutter.
        assert_eq!(styled[0].spans[0].style.fg, Some(rata(Theme::dark().dim)));
    }

    #[test]
    fn tool_result_cell_renders_write_as_pure_add_diff() {
        let cell = ToolResultCell::new(
            "Write".to_string(),
            serde_json::json!({"content": "ok"}),
            None,
            Some("line one\nline two".to_string()),
            Some("x.txt".to_string()),
            None,
        );
        let lines = plain(&cell, false);
        assert_eq!(
            lines[0], "  ⎿  Added 2 lines",
            "pure-add summary: {lines:?}"
        );
        let all = lines.join("\n");
        assert!(all.contains("line one"), "{all}");
        assert!(all.contains("line two"), "{all}");
        // The plain `⎿  ok` summary is replaced by the diff.
        assert!(!all.contains("⎿  ok"), "{all}");
    }

    #[test]
    fn collapsed_read_search_cell_verbose_expands_entries() {
        let cell = CollapsedReadSearchCell::new(
            1,
            1,
            0,
            0,
            0,
            Vec::new(),
            0,
            0,
            false,
            None,
            vec!["Read a.rs".to_string(), "Grep foo".to_string()],
        );
        assert_eq!(
            plain(&cell, true),
            vec![
                "Searched for 1 pattern, read 1 file".to_string(),
                "  ⎿  Read a.rs".to_string(),
                "  ⎿  Grep foo".to_string(),
            ]
        );
        let styled = cell.display_lines(
            80,
            &Theme::dark(),
            RenderMode {
                raw: false,
                verbose: true,
            },
        );
        assert!(
            styled
                .iter()
                .all(|l| l.spans[0].style.fg == Some(rata(Theme::dark().dim))),
            "entries render dim like the summary"
        );
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
                "  ⎿  {\"f\":\"a\"} → \"ok\"".to_string(),
                "  ⎿  {\"f\":\"b\"} → \"ok\"".to_string(),
            ]
        );
    }

    #[test]
    fn collapsed_read_search_cell_renders_dim_count_line() {
        let cell = CollapsedReadSearchCell::new(
            1,
            1,
            0,
            0,
            0,
            Vec::new(),
            0,
            0,
            false,
            None,
            vec!["Read a.rs".to_string(), "Grep foo".to_string()],
        );
        assert_eq!(
            plain(&cell, false),
            vec!["Searched for 1 pattern, read 1 file".to_string()]
        );
        let styled = cell.display_lines(80, &Theme::dark(), RenderMode::default());
        assert_eq!(styled[0].spans[0].style.fg, Some(rata(Theme::dark().dim)));
    }

    #[test]
    fn fullscreen_categories_render_in_summary() {
        let cell = CollapsedReadSearchCell::new(
            0,
            0,
            0,
            0,
            2,
            vec!["github".to_string()],
            1,
            1,
            false,
            None,
            Vec::new(),
        );
        assert_eq!(
            plain(&cell, false),
            vec!["Queried github 2 times, ran 1 bash command, wrote 1 memory".to_string()]
        );
    }
}

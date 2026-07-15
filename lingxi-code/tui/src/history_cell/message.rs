//! Per-variant history cells for the plain conversation messages: user text,
//! assistant text (markdown), thinking blocks, advisor blocks, plan/memory
//! echoes, and the user prompt/command/bash-input echoes.
//!
//! Split out of `message.rs` in the message-cells phases (plan Phase 9). The
//! styled-line renderers here are the single source for these variants: the
//! cells consume them through [`StyledCell`], and the legacy
//! [`crate::message::render_message`] dispatcher delegates to them so its
//! output stays line-identical to the pre-split renderer.

use tui_core::message::AdvisorKind;
use tui_core::render::markdown::{render_with_width, MarkdownTheme};
use tui_core::render::{SpanStyle, StyleColor, StyledLine, StyledSpan};
use tui_core::theme::{Theme, ThemeName};

use super::{colored_lines, plain_lines, truncate, StyledCell};

/// Assistant dot marker (iocraft `assistant_text::MARKER` parity): `⏺ `
/// (U+23FA) on macOS — renders as the reddish record glyph — `● ` (U+25CF)
/// elsewhere, each + a trailing space.
pub(crate) const ASSISTANT_MARKER: &str = if cfg!(target_os = "macos") {
    "\u{23FA} "
} else {
    "\u{25CF} "
};
/// Continuation indent aligning wrapped assistant lines under the marker.
pub(crate) const CONT_INDENT: &str = "  ";

fn markdown_theme() -> MarkdownTheme {
    MarkdownTheme {
        inline_code: StyleColor::Rgb(177, 185, 249),
        code_theme: ThemeName::Dark,
    }
}

/// claude-code `INTERRUPT_MESSAGE` (utils/messages.ts:207) — the `UserText`
/// turn content pushed when a streaming turn is interrupted. Rendered as the
/// dim `InterruptedByUser` line, not the `> ` echo.
pub(crate) const INTERRUPT_MESSAGE: &str = "[Request interrupted by user]";

/// The line body claude-code's `InterruptedByUser.tsx` shows for an
/// interrupted turn (iocraft `user_tool_result::INTERRUPTED_LINE`).
const INTERRUPTED_LINE: &str = "Interrupted \u{00b7} What should Claude do instead?";

/// `> {body}` user echo (empty body → nothing). A body equal to
/// [`INTERRUPT_MESSAGE`] renders the dim `  ⎿  Interrupted · …` line instead
/// (1:1 with the iocraft `UserTextMessage` special-case).
pub(crate) fn user_text_lines(body: &str, theme: &Theme) -> Vec<StyledLine> {
    if body == INTERRUPT_MESSAGE {
        return colored_lines(&format!("  \u{23BF}  {INTERRUPTED_LINE}"), theme.dim);
    }
    if body.is_empty() {
        Vec::new()
    } else {
        vec![StyledLine::plain(format!("> {body}"))]
    }
}

/// Markdown-render the assistant body, prefixing the first line with the
/// `● ` marker and indenting continuation lines (mirrors the iocraft renderer).
///
/// The marker takes the theme `text` color (claude-code `AssistantTextMessage`
/// renders the dot with `color="text"`: black on light themes, white on dark),
/// NOT the terminal default — so it stays visible/consistent across themes.
pub(crate) fn assistant_lines(body: &str, width: usize, theme: &Theme) -> Vec<StyledLine> {
    let mut out = Vec::new();
    let mut rendered = render_with_width(body, &markdown_theme(), width)
        .into_iter()
        .filter(|line| !line.spans.is_empty());

    let marker = || {
        StyledSpan::styled(
            ASSISTANT_MARKER,
            SpanStyle {
                fg: theme.text,
                ..SpanStyle::default()
            },
        )
    };
    if let Some(mut first) = rendered.next() {
        first.spans.insert(0, marker());
        out.push(first);
    }
    for mut line in rendered {
        line.spans.insert(0, StyledSpan::plain(CONT_INDENT));
        out.push(line);
    }
    if out.is_empty() {
        out.push(StyledLine {
            spans: vec![marker()],
        });
    }
    out
}

/// A user prompt echoed into scrollback, one plain line per body line.
pub(crate) fn user_prompt_lines(text: &str) -> Vec<StyledLine> {
    plain_lines(text)
}

/// Slash-command echo: `/{command} {args}`, or the `Skill(name)` form when
/// `is_skill` (tui-core `UserCommand` doc contract).
pub(crate) fn user_command_lines(command: &str, args: &str, is_skill: bool) -> Vec<StyledLine> {
    let text = if is_skill {
        format!("Skill({command})")
    } else if args.is_empty() {
        format!("/{command}")
    } else {
        format!("/{command} {args}")
    };
    vec![StyledLine::plain(text)]
}

/// Bash-mode command echo: dim `! {command}`.
pub(crate) fn user_bash_input_lines(command: &str, theme: &Theme) -> Vec<StyledLine> {
    colored_lines(&format!("! {command}"), theme.dim)
}

/// Assistant thinking block, 1:1 with the iocraft `AssistantThinkingMessage`
/// (a documented literal-lock of claude-code `AssistantThinkingMessage.tsx`):
/// the `∴ ` marker (U+2234), dim+italic header. Collapsed → `∴ Thinking
/// (ctrl+o to expand)`; expanded → `∴ Thinking…`, a gap=1 blank row, then the
/// markdown body indented 2 spaces and dim-colored. `✻` is reserved for
/// REDACTED thinking (see [`redacted_thinking_lines`]).
pub(crate) fn thinking_lines(
    thinking: &str,
    width: usize,
    verbose: bool,
    theme: &Theme,
) -> Vec<StyledLine> {
    let header = |text: &str| StyledLine {
        spans: vec![StyledSpan::styled(
            text.to_string(),
            SpanStyle {
                fg: theme.dim,
                italic: true,
                ..SpanStyle::default()
            },
        )],
    };
    if !verbose {
        return vec![header("\u{2234} Thinking (ctrl+o to expand)")];
    }
    let mut out = vec![header("\u{2234} Thinking\u{2026}")];
    // Expanded: a gap=1 blank row, then the markdown body indented 2 and
    // dim-colored (claude-code `<Box paddingLeft={2}><Markdown dimColor>`).
    let body: Vec<StyledLine> = render_with_width(thinking, &markdown_theme(), width)
        .into_iter()
        .filter(|line| !line.spans.is_empty())
        .collect();
    if !body.is_empty() {
        out.push(StyledLine { spans: Vec::new() });
        for mut line in body {
            for span in &mut line.spans {
                span.style.fg = theme.dim;
            }
            line.spans.insert(0, StyledSpan::plain("  "));
            out.push(line);
        }
    }
    out
}

/// Redacted thinking: the bare dim `✻ Thinking…` marker (no expandable body).
pub(crate) fn redacted_thinking_lines(theme: &Theme) -> Vec<StyledLine> {
    colored_lines("✻ Thinking…", theme.dim)
}

/// An advisor block: a `✻ Advisor…` marker line whose text depends on the
/// kind. `verbose` is the MESSAGE-level flag baked at construction (claude
/// -code transcript mode), not the Ctrl-O render toggle.
pub(crate) fn advisor_lines(kind: &AdvisorKind, verbose: bool, theme: &Theme) -> Vec<StyledLine> {
    match kind {
        AdvisorKind::ServerToolUse { model, input } => {
            let mut header = "✻ Advising".to_string();
            if let Some(m) = model {
                header.push_str(&format!(" ({m})"));
            }
            let mut out = colored_lines(&header, theme.dim);
            if let Some(i) = input {
                out.extend(colored_lines(&format!("  {}", truncate(i, 100)), theme.dim));
            }
            out
        }
        AdvisorKind::Result { text } => {
            let mut out = colored_lines("✻ Advisor", theme.dim);
            let body = if verbose {
                text.clone()
            } else {
                truncate(text, 100)
            };
            out.extend(colored_lines(&format!("  {body}"), theme.dim));
            out
        }
        AdvisorKind::RedactedResult => colored_lines("✻ Advisor", theme.dim),
        AdvisorKind::Error { error_code } => colored_lines(
            &format!("✻ Advisor unavailable ({error_code})"),
            theme.error,
        ),
    }
}

/// Plan-mode plan body, echoed as plain lines.
pub(crate) fn user_plan_lines(plan_content: &str) -> Vec<StyledLine> {
    plain_lines(plan_content)
}

/// Memory write echo: dim `# {input}`.
pub(crate) fn user_memory_input_lines(input: &str, theme: &Theme) -> Vec<StyledLine> {
    colored_lines(&format!("# {input}"), theme.dim)
}

/// [`RenderedMessage::UserText`](tui_core::message::RenderedMessage::UserText)
/// — the `> `-prefixed user prompt echo.
#[derive(Debug)]
pub struct UserTextCell {
    body: String,
}

impl UserTextCell {
    /// Wrap the submitted prompt body.
    #[must_use]
    pub fn new(body: String) -> Self {
        Self { body }
    }

    /// The prompt body text.
    #[must_use]
    pub fn body(&self) -> &str {
        &self.body
    }
}

impl StyledCell for UserTextCell {
    fn styled_lines(&self, _width: usize, theme: &Theme, _verbose: bool) -> Vec<StyledLine> {
        user_text_lines(&self.body, theme)
    }
}

/// [`RenderedMessage::AssistantText`](tui_core::message::RenderedMessage::AssistantText)
/// — the marker-prefixed, markdown-rendered assistant reply. Also the active
/// streaming cell: deltas [`Self::append`] to the body in place.
#[derive(Debug)]
pub struct AssistantTextCell {
    body: String,
}

impl AssistantTextCell {
    /// Wrap the assistant body (may be empty at `TurnStarted`; an empty body
    /// still renders the bare 1-row marker).
    #[must_use]
    pub fn new(body: String) -> Self {
        Self { body }
    }

    /// The assistant body text (markdown source).
    #[must_use]
    pub fn body(&self) -> &str {
        &self.body
    }

    /// Append a streaming text delta to the body in place.
    pub fn append(&mut self, delta: &str) {
        self.body.push_str(delta);
    }
}

impl StyledCell for AssistantTextCell {
    fn styled_lines(&self, width: usize, theme: &Theme, _verbose: bool) -> Vec<StyledLine> {
        // The just-opened active cell (before any delta) has an empty body: render
        // NOTHING. The running spinner is the "thinking" indicator — a bare `●`
        // tail row here is the stray marker `flush_or_discard_active` guards
        // against, and it prematurely grows the viewport by a row. Once a delta
        // arrives the body is non-empty and the `● …` reply renders normally.
        if self.body.is_empty() {
            return Vec::new();
        }
        assistant_lines(&self.body, width, theme)
    }
}

/// [`RenderedMessage::UserPrompt`](tui_core::message::RenderedMessage::UserPrompt)
/// — a prompt body echoed as plain lines.
#[derive(Debug)]
pub struct UserPromptCell {
    text: String,
}

impl UserPromptCell {
    /// Wrap the echoed prompt text.
    #[must_use]
    pub fn new(text: String) -> Self {
        Self { text }
    }

    /// The echoed prompt text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }
}

impl StyledCell for UserPromptCell {
    fn styled_lines(&self, _width: usize, _theme: &Theme, _verbose: bool) -> Vec<StyledLine> {
        user_prompt_lines(&self.text)
    }
}

/// [`RenderedMessage::UserCommand`](tui_core::message::RenderedMessage::UserCommand)
/// — the `/{command} {args}` (or `Skill(name)`) echo.
#[derive(Debug)]
pub struct UserCommandCell {
    command: String,
    args: String,
    is_skill: bool,
}

impl UserCommandCell {
    /// Wrap a slash-command echo.
    #[must_use]
    pub fn new(command: String, args: String, is_skill: bool) -> Self {
        Self {
            command,
            args,
            is_skill,
        }
    }

    /// The command name (without leading slash).
    #[must_use]
    pub fn command(&self) -> &str {
        &self.command
    }
}

impl StyledCell for UserCommandCell {
    fn styled_lines(&self, _width: usize, _theme: &Theme, _verbose: bool) -> Vec<StyledLine> {
        user_command_lines(&self.command, &self.args, self.is_skill)
    }
}

/// [`RenderedMessage::UserBashInput`](tui_core::message::RenderedMessage::UserBashInput)
/// — the dim `! {command}` bash-mode echo.
#[derive(Debug)]
pub struct UserBashInputCell {
    command: String,
}

impl UserBashInputCell {
    /// Wrap the bash-mode command line.
    #[must_use]
    pub fn new(command: String) -> Self {
        Self { command }
    }

    /// The command line the user typed.
    #[must_use]
    pub fn command(&self) -> &str {
        &self.command
    }
}

impl StyledCell for UserBashInputCell {
    fn styled_lines(&self, _width: usize, theme: &Theme, _verbose: bool) -> Vec<StyledLine> {
        user_bash_input_lines(&self.command, theme)
    }
}

/// [`RenderedMessage::AssistantThinking`](tui_core::message::RenderedMessage::AssistantThinking)
/// — collapsed expand hint / verbose body, driven by the render mode's
/// verbose toggle (Ctrl-O), not the message's own `expanded` flag (pre-split
/// behavior: the dispatcher ignores that flag).
#[derive(Debug)]
pub struct ThinkingCell {
    thinking: String,
}

impl ThinkingCell {
    /// Wrap the thinking text.
    #[must_use]
    pub fn new(thinking: String) -> Self {
        Self { thinking }
    }

    /// The thinking body text.
    #[must_use]
    pub fn thinking(&self) -> &str {
        &self.thinking
    }

    /// Append a streaming thinking delta (M5 cc2.1.198 thinking streaming —
    /// mirrors [`AssistantTextCell::append`] so `TurnEvent::ThinkingDelta`
    /// mutates the active thinking cell in place).
    pub fn append(&mut self, delta: &str) {
        self.thinking.push_str(delta);
    }
}

impl StyledCell for ThinkingCell {
    fn styled_lines(&self, width: usize, theme: &Theme, verbose: bool) -> Vec<StyledLine> {
        thinking_lines(&self.thinking, width, verbose, theme)
    }
}

/// [`RenderedMessage::AssistantRedactedThinking`](tui_core::message::RenderedMessage::AssistantRedactedThinking)
/// — the bare dim `✻ Thinking…` marker.
#[derive(Debug)]
pub struct RedactedThinkingCell;

impl StyledCell for RedactedThinkingCell {
    fn styled_lines(&self, _width: usize, theme: &Theme, _verbose: bool) -> Vec<StyledLine> {
        redacted_thinking_lines(theme)
    }
}

/// [`RenderedMessage::Advisor`](tui_core::message::RenderedMessage::Advisor)
/// — the `✻ Advisor…` block. Expansion follows the message's own baked
/// `verbose` flag, not the Ctrl-O render toggle (pre-split behavior).
#[derive(Debug)]
pub struct AdvisorCell {
    kind: AdvisorKind,
    verbose: bool,
}

impl AdvisorCell {
    /// Wrap an advisor block (`verbose` is the message-level flag).
    #[must_use]
    pub fn new(kind: AdvisorKind, verbose: bool) -> Self {
        Self { kind, verbose }
    }
}

impl StyledCell for AdvisorCell {
    fn styled_lines(&self, _width: usize, theme: &Theme, _verbose: bool) -> Vec<StyledLine> {
        advisor_lines(&self.kind, self.verbose, theme)
    }
}

/// [`RenderedMessage::UserPlan`](tui_core::message::RenderedMessage::UserPlan)
/// — the plan-mode plan body echoed as plain lines.
#[derive(Debug)]
pub struct UserPlanCell {
    plan_content: String,
}

impl UserPlanCell {
    /// Wrap the plan content.
    #[must_use]
    pub fn new(plan_content: String) -> Self {
        Self { plan_content }
    }
}

impl StyledCell for UserPlanCell {
    fn styled_lines(&self, _width: usize, _theme: &Theme, _verbose: bool) -> Vec<StyledLine> {
        user_plan_lines(&self.plan_content)
    }
}

/// [`RenderedMessage::UserMemoryInput`](tui_core::message::RenderedMessage::UserMemoryInput)
/// — the dim `# {input}` memory-write echo.
#[derive(Debug)]
pub struct UserMemoryInputCell {
    input: String,
}

impl UserMemoryInputCell {
    /// Wrap the memory input text.
    #[must_use]
    pub fn new(input: String) -> Self {
        Self { input }
    }
}

impl StyledCell for UserMemoryInputCell {
    fn styled_lines(&self, _width: usize, theme: &Theme, _verbose: bool) -> Vec<StyledLine> {
        user_memory_input_lines(&self.input, theme)
    }
}

#[cfg(test)]
mod tests {
    use super::super::{HistoryCell, RenderMode};
    use super::*;

    fn plain(cell: &dyn HistoryCell) -> Vec<String> {
        cell.display_lines(80, &Theme::dark(), RenderMode::default())
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    #[test]
    fn user_text_cell_renders_prompt_prefix_line() {
        let cell = UserTextCell::new("hello".to_string());
        assert_eq!(plain(&cell), vec!["> hello".to_string()]);
        assert_eq!(cell.body(), "hello");
    }

    #[test]
    fn empty_user_text_cell_is_invisible() {
        let cell = UserTextCell::new(String::new());
        assert!(plain(&cell).is_empty());
        assert!(!cell.is_visible(80));
    }

    #[test]
    fn assistant_cell_renders_marker_markdown_and_continuation_indent() {
        let cell = AssistantTextCell::new("**bold** text\n\nsecond para".to_string());
        let lines = plain(&cell);
        assert!(
            lines[0].starts_with(ASSISTANT_MARKER),
            "marker glyph on first line: {lines:?}"
        );
        assert!(lines[0].contains("bold"), "markdown body: {lines:?}");
        // Markdown styling survives: the `**bold**` span carries bold.
        let styled = cell.display_lines(80, &Theme::dark(), RenderMode::default());
        assert!(
            styled[0].spans.iter().any(|s| s.content.contains("bold")
                && s.style
                    .add_modifier
                    .contains(ratatui::style::Modifier::BOLD)),
            "bold modifier preserved: {styled:?}"
        );
        // Continuation lines are indented under the marker.
        assert!(
            lines[1..].iter().all(|l| l.starts_with(CONT_INDENT)),
            "continuation indent: {lines:?}"
        );
    }

    #[test]
    fn empty_assistant_cell_renders_nothing_until_content() {
        // The just-opened active cell (empty body, before any delta) renders NO
        // rows — the running spinner is the deliberation indicator; a bare `●`
        // tail here is the stray marker we avoid (and it prematurely grew the
        // viewport). The marker appears with the first streamed content.
        let empty = AssistantTextCell::new(String::new());
        assert!(
            plain(&empty).is_empty(),
            "empty active cell renders no rows"
        );
        assert_eq!(empty.desired_height(80, RenderMode::default()), 0);
        // Once content arrives, the `● …` reply renders normally.
        let filled = AssistantTextCell::new("hi".to_string());
        assert!(plain(&filled)[0].contains("hi"));
        assert!(plain(&filled)[0].starts_with(ASSISTANT_MARKER));
    }

    #[test]
    fn assistant_cell_append_grows_body_in_place() {
        let mut cell = AssistantTextCell::new("Hel".to_string());
        cell.append("lo");
        assert_eq!(cell.body(), "Hello");
        assert!(plain(&cell)[0].contains("Hello"));
    }

    #[test]
    fn user_prompt_cell_splits_lines_plain() {
        let cell = UserPromptCell::new("echoed prompt\nsecond line".to_string());
        assert_eq!(
            plain(&cell),
            vec!["echoed prompt".to_string(), "second line".to_string()]
        );
        assert_eq!(cell.text(), "echoed prompt\nsecond line");
    }

    #[test]
    fn user_command_cell_renders_slash_forms() {
        let bare = UserCommandCell::new("help".to_string(), String::new(), false);
        assert_eq!(plain(&bare), vec!["/help".to_string()]);
        assert_eq!(bare.command(), "help");
        let with_args = UserCommandCell::new("model".to_string(), "opus".to_string(), false);
        assert_eq!(plain(&with_args), vec!["/model opus".to_string()]);
    }

    #[test]
    fn user_command_cell_renders_skill_form() {
        // tui-core doc contract: is_skill renders `Skill(name)`, not `/name`.
        let skill = UserCommandCell::new("deploy".to_string(), String::new(), true);
        assert_eq!(plain(&skill), vec!["Skill(deploy)".to_string()]);
    }

    #[test]
    fn user_bash_input_cell_renders_dim_bang_line() {
        let cell = UserBashInputCell::new("ls -la".to_string());
        assert_eq!(plain(&cell), vec!["! ls -la".to_string()]);
        assert_eq!(cell.command(), "ls -la");
        let styled = cell.display_lines(80, &Theme::dark(), RenderMode::default());
        assert_eq!(
            styled[0].spans[0].style.fg,
            Some(crate::style_adapter::to_ratatui(Theme::dark().dim)),
            "dim color preserved"
        );
    }

    #[test]
    fn thinking_cell_collapses_by_default_and_expands_with_render_verbose() {
        let cell = ThinkingCell::new("step one\nstep two".to_string());
        assert_eq!(cell.thinking(), "step one\nstep two");
        assert_eq!(
            plain(&cell),
            vec!["\u{2234} Thinking (ctrl+o to expand)".to_string()],
            "collapsed shows only the hint"
        );
        let expanded: Vec<String> = cell
            .display_lines(
                80,
                &Theme::dark(),
                RenderMode {
                    raw: false,
                    verbose: true,
                },
            )
            .iter()
            .map(ToString::to_string)
            .collect();
        // Expanded: ∴ header, a gap=1 blank row, then the markdown body
        // indented 2 spaces (1:1 with the iocraft AssistantThinkingMessage).
        assert_eq!(expanded[0], "\u{2234} Thinking\u{2026}");
        assert_eq!(expanded[1], "", "gap=1 blank row after the header");
        assert!(
            expanded[2].starts_with("  "),
            "body indented 2: {:?}",
            expanded[2]
        );
        assert!(
            expanded.iter().any(|l| l.contains("step one")),
            "body present: {expanded:?}"
        );
    }

    #[test]
    fn redacted_thinking_cell_renders_the_bare_dim_marker() {
        let cell = RedactedThinkingCell;
        assert_eq!(plain(&cell), vec!["✻ Thinking…".to_string()]);
        let styled = cell.display_lines(80, &Theme::dark(), RenderMode::default());
        assert_eq!(
            styled[0].spans[0].style.fg,
            Some(crate::style_adapter::to_ratatui(Theme::dark().dim))
        );
        // No expandable body: verbose renders the same single line.
        let verbose = cell.display_lines(
            80,
            &Theme::dark(),
            RenderMode {
                raw: false,
                verbose: true,
            },
        );
        assert_eq!(verbose.len(), 1);
    }

    #[test]
    fn advisor_cell_renders_every_kind() {
        let server = AdvisorCell::new(
            AdvisorKind::ServerToolUse {
                model: Some("gpt-5".to_string()),
                input: Some("review diff".to_string()),
            },
            false,
        );
        assert_eq!(
            plain(&server),
            vec![
                "✻ Advising (gpt-5)".to_string(),
                "  review diff".to_string()
            ]
        );

        let redacted = AdvisorCell::new(AdvisorKind::RedactedResult, false);
        assert_eq!(plain(&redacted), vec!["✻ Advisor".to_string()]);

        let error = AdvisorCell::new(
            AdvisorKind::Error {
                error_code: "503".to_string(),
            },
            false,
        );
        assert_eq!(
            plain(&error),
            vec!["✻ Advisor unavailable (503)".to_string()]
        );
        let styled = error.display_lines(80, &Theme::dark(), RenderMode::default());
        assert_eq!(
            styled[0].spans[0].style.fg,
            Some(crate::style_adapter::to_ratatui(Theme::dark().error))
        );
    }

    #[test]
    fn advisor_cell_expansion_follows_the_message_flag_not_render_verbose() {
        let long = "x".repeat(150);
        let kind = AdvisorKind::Result { text: long.clone() };

        // Message flag off: truncated to 100 chars even when the render mode
        // is verbose (pre-split dispatcher behavior).
        let collapsed = AdvisorCell::new(kind.clone(), false);
        let render_verbose = RenderMode {
            raw: false,
            verbose: true,
        };
        let body = collapsed.display_lines(80, &Theme::dark(), render_verbose)[1].to_string();
        assert!(body.ends_with('…'), "truncated: {body:?}");
        assert!(body.len() < long.len());

        // Message flag on: the full text, even in a non-verbose render.
        let expanded = AdvisorCell::new(kind, true);
        let body = expanded.display_lines(80, &Theme::dark(), RenderMode::default())[1].to_string();
        assert_eq!(body, format!("  {long}"));
    }

    #[test]
    fn user_plan_cell_renders_plain_plan_lines() {
        let cell = UserPlanCell::new("step 1\nstep 2".to_string());
        assert_eq!(
            plain(&cell),
            vec!["step 1".to_string(), "step 2".to_string()]
        );
    }

    #[test]
    fn user_memory_input_cell_renders_dim_hash_line() {
        let cell = UserMemoryInputCell::new("remember this".to_string());
        assert_eq!(plain(&cell), vec!["# remember this".to_string()]);
        let styled = cell.display_lines(80, &Theme::dark(), RenderMode::default());
        assert_eq!(
            styled[0].spans[0].style.fg,
            Some(crate::style_adapter::to_ratatui(Theme::dark().dim))
        );
    }
}

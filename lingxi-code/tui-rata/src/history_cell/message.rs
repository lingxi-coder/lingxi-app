//! Per-variant history cells for the plain conversation messages: user text,
//! assistant text (markdown), and the user prompt/command/bash-input echoes.
//!
//! Split out of `message.rs` in the message-cells phase (plan Phase 9). The
//! styled-line renderers here are the single source for these variants: the
//! cells consume them through [`StyledCell`], and the legacy
//! [`crate::message::render_message`] dispatcher delegates to them so its
//! output stays line-identical to the pre-split renderer.

use tui_core::render::markdown::{render_with_width, MarkdownTheme};
use tui_core::render::{StyleColor, StyledLine, StyledSpan};
use tui_core::theme::{Theme, ThemeName};

use super::{colored_lines, plain_lines, StyledCell};

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

/// `> {body}` user echo (empty body → nothing).
pub(crate) fn user_text_lines(body: &str) -> Vec<StyledLine> {
    if body.is_empty() {
        Vec::new()
    } else {
        vec![StyledLine::plain(format!("> {body}"))]
    }
}

/// Markdown-render the assistant body, prefixing the first line with the
/// `● ` marker and indenting continuation lines (mirrors the iocraft renderer).
pub(crate) fn assistant_lines(body: &str, width: usize) -> Vec<StyledLine> {
    let mut out = Vec::new();
    let mut rendered = render_with_width(body, &markdown_theme(), width)
        .into_iter()
        .filter(|line| !line.spans.is_empty());

    if let Some(mut first) = rendered.next() {
        first.spans.insert(0, StyledSpan::plain(ASSISTANT_MARKER));
        out.push(first);
    }
    for mut line in rendered {
        line.spans.insert(0, StyledSpan::plain(CONT_INDENT));
        out.push(line);
    }
    if out.is_empty() {
        out.push(StyledLine::plain(ASSISTANT_MARKER));
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
    fn styled_lines(&self, _width: usize, _theme: &Theme, _verbose: bool) -> Vec<StyledLine> {
        user_text_lines(&self.body)
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
    fn styled_lines(&self, width: usize, _theme: &Theme, _verbose: bool) -> Vec<StyledLine> {
        assistant_lines(&self.body, width)
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
    fn empty_assistant_cell_renders_one_bare_marker_row() {
        // The TurnStarted gotcha: an empty active assistant cell must still
        // occupy exactly one marker row (layout tests depend on it).
        let cell = AssistantTextCell::new(String::new());
        assert_eq!(plain(&cell), vec![ASSISTANT_MARKER.to_string()]);
        assert_eq!(cell.desired_height(80, RenderMode::default()), 1);
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
}

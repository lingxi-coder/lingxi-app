//! Transcript history cells (plan Phase 3).
//!
//! Modeled on codex-rs `tui/src/history_cell/mod.rs` — the UI architecture
//! pattern only, over `LingXi`'s own data model: a [`HistoryCell`] is the unit
//! of conversation history. Committed cells are inserted into the terminal's
//! native scrollback exactly once; the transient active (in-flight) cell can
//! mutate in place while streaming and is rendered as the live tail until it
//! is finalized (see [`crate::transcript::Transcript`]).
//!
//! The core `RenderedMessage` variants render through concrete per-variant
//! cells (plan Phase 9, "message cells"): [`message`] (user/assistant text,
//! prompt/command/bash input), [`system`] (system text/rich/api-error/rate
//! -limit), and [`tool`] (tool use/result, bash/local command output,
//! grouped/collapsed folds). [`cell_for_message`] picks the concrete cell;
//! variants not yet split keep flowing through the adapter cell
//! [`MessageHistoryCell`], which wraps a whole
//! [`tui_core::message::RenderedMessage`] and delegates to
//! [`crate::message::render_message`] so their output is byte-identical to
//! the pre-transcript flush path (the second message-cells phase ports them).

pub mod message;
pub mod system;
pub mod tool;

use std::any::Any;
use std::time::Instant;

use ratatui::text::{Line, Text};
use ratatui::widgets::{Paragraph, Wrap};
use tui_core::message::RenderedMessage;
use tui_core::render::{SpanStyle, StyleColor, StyledLine, StyledSpan};
use tui_core::theme::Theme;

/// How a [`HistoryCell`] renders its lines.
///
/// Folds codex's `HistoryRenderMode` (rich vs raw) together with `LingXi`'s
/// verbose/expanded toggle (Ctrl-O), since both select *how* the same cell
/// renders. The default is rich + collapsed — the pre-transcript behavior.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderMode {
    /// `true` → copy-friendly plain output ([`HistoryCell::raw_lines`])
    /// instead of rich styled output (codex "raw scrollback" mode).
    pub raw: bool,
    /// `true` → expand collapsible content (thinking bodies, tool-use JSON,
    /// grouped children). Toggled by Ctrl-O.
    pub verbose: bool,
}

/// A single renderable unit of conversation history.
///
/// Each cell produces logical [`Line`]s and reports how many viewport rows
/// those lines occupy at a given terminal width. The default height
/// implementation uses `Paragraph::line_count` with `Wrap { trim: false }`
/// (the codex height contract), which accounts for lines wider than the
/// viewport; concrete cells only override it when they apply layout logic
/// beyond what `Paragraph` captures.
pub trait HistoryCell: std::fmt::Debug + Send + Sync + Any {
    /// The styled lines for the chat surface at `width` columns, colored by
    /// `theme`, rendered per `mode` (rich/raw + verbose expansion).
    fn display_lines(&self, width: u16, theme: &Theme, mode: RenderMode) -> Vec<Line<'static>>;

    /// Copy-friendly plain (unstyled) logical lines for raw scrollback mode.
    fn raw_lines(&self) -> Vec<Line<'static>>;

    /// The number of viewport rows needed to render this cell at `width`.
    ///
    /// Measured with a fixed theme: themes only recolor spans — they never
    /// change text content or wrapping — so the row count is theme-invariant.
    fn desired_height(&self, width: u16, mode: RenderMode) -> u16 {
        Paragraph::new(Text::from(self.display_lines(width, &Theme::dark(), mode)))
            .wrap(Wrap { trim: false })
            .line_count(width)
            .try_into()
            .unwrap_or(u16::MAX)
    }

    /// Whether this cell has any visible output at `width` columns (cells that
    /// render to nothing are consumed by the commit cursor without inserting).
    fn is_visible(&self, width: u16) -> bool {
        self.desired_height(width, RenderMode::default()) > 0
    }

    /// Cache key for active cells whose rendering changes with elapsed time
    /// (spinners/animations): the live tail is re-rendered when the key
    /// changes. `None` (the default) means the cell is time-invariant.
    fn animation_key(&self, _now: Instant) -> Option<u64> {
        None
    }

    /// Upcast for downcasting to the concrete cell type.
    ///
    /// Explicit instead of relying on `dyn HistoryCell` → `dyn Any` trait
    /// upcasting, which needs rustc ≥ 1.86 (> workspace MSRV 1.82).
    fn as_any(&self) -> &dyn Any;

    /// Mutable upcast for downcasting (in-place mutation of the active cell).
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

/// Renderer default width when a caller passes 0 columns (pre-split
/// `render_message` behavior, kept by every cell).
pub(crate) const DEFAULT_WIDTH: usize = 80;

/// Internal single-source render contract for the concrete cells: produce the
/// neutral [`StyledLine`]s for one message. The blanket [`HistoryCell`] impl
/// below derives rich/raw display lines, width defaulting, and downcasting
/// uniformly from it, so each cell only owns its variant's rendering.
pub(crate) trait StyledCell: std::fmt::Debug + Send + Sync + Any {
    /// The styled lines at `width` columns (never 0), colored by `theme`,
    /// with collapsible content expanded per `verbose`.
    fn styled_lines(&self, width: usize, theme: &Theme, verbose: bool) -> Vec<StyledLine>;
}

impl<T: StyledCell> HistoryCell for T {
    fn display_lines(&self, width: u16, theme: &Theme, mode: RenderMode) -> Vec<Line<'static>> {
        if mode.raw {
            return self.raw_lines();
        }
        let width = if width == 0 {
            DEFAULT_WIDTH
        } else {
            usize::from(width)
        };
        self.styled_lines(width, theme, mode.verbose)
            .iter()
            .map(crate::render::styled_line_to_ratatui)
            .collect()
    }

    fn raw_lines(&self) -> Vec<Line<'static>> {
        // Plain text of the rich render at the renderer's default width; the
        // theme is irrelevant once styles are stripped.
        self.styled_lines(DEFAULT_WIDTH, &Theme::dark(), false)
            .iter()
            .map(|line| {
                Line::from(
                    line.spans
                        .iter()
                        .map(|span| span.text.as_str())
                        .collect::<String>(),
                )
            })
            .collect()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// Convert one [`RenderedMessage`] into its concrete [`HistoryCell`]: the
/// core variants map to the per-variant cells in [`message`]/[`system`]/
/// [`tool`]; every not-yet-ported variant falls back to the
/// [`MessageHistoryCell`] adapter (removed once the second message-cells
/// phase covers them all).
#[must_use]
pub fn cell_for_message(message: RenderedMessage) -> Box<dyn HistoryCell> {
    match message {
        RenderedMessage::UserText { body, .. } => Box::new(message::UserTextCell::new(body)),
        RenderedMessage::AssistantText { body, .. } => {
            Box::new(message::AssistantTextCell::new(body))
        }
        RenderedMessage::UserPrompt { text } => Box::new(message::UserPromptCell::new(text)),
        RenderedMessage::UserCommand {
            command,
            args,
            is_skill,
        } => Box::new(message::UserCommandCell::new(command, args, is_skill)),
        RenderedMessage::UserBashInput { command } => {
            Box::new(message::UserBashInputCell::new(command))
        }
        RenderedMessage::SystemText { body, is_error, .. } => {
            Box::new(system::SystemTextCell::new(body, is_error))
        }
        RenderedMessage::SystemTextRich { body, level } => {
            Box::new(system::SystemTextRichCell::new(body, level))
        }
        RenderedMessage::SystemApiError {
            error,
            retry_attempt,
            max_retries,
            ..
        } => Box::new(system::SystemApiErrorCell::new(
            error,
            retry_attempt,
            max_retries,
        )),
        RenderedMessage::RateLimit { text, upsell } => {
            Box::new(system::RateLimitCell::new(text, upsell))
        }
        RenderedMessage::AssistantToolUse { tool, input, .. } => {
            Box::new(tool::ToolUseCell::new(tool, input))
        }
        RenderedMessage::UserToolResult {
            result,
            old_string,
            new_string,
            file_path,
            ..
        } => Box::new(tool::ToolResultCell::new(
            result, old_string, new_string, file_path,
        )),
        RenderedMessage::UserBashOutput { stdout, stderr }
        | RenderedMessage::UserLocalCommandOutput { stdout, stderr } => {
            Box::new(tool::CommandOutputCell::new(stdout, stderr))
        }
        RenderedMessage::GroupedToolUse { tool, entries, .. } => {
            Box::new(tool::GroupedToolUseCell::new(tool, entries))
        }
        RenderedMessage::CollapsedReadSearch { entries, .. } => {
            Box::new(tool::CollapsedReadSearchCell::new(entries))
        }
        other => Box::new(MessageHistoryCell::new(other)),
    }
}

// ===== Shared styled-line primitives (moved from `message.rs` in the
// message-cells split; used by the cell modules AND the legacy renderer). =====

/// One [`StyledLine`] per `\n`-separated line, single-span colored `color`
/// (empty text → no lines).
pub(crate) fn colored_lines(text: &str, color: StyleColor) -> Vec<StyledLine> {
    if text.is_empty() {
        return Vec::new();
    }
    text.split('\n')
        .map(|line| StyledLine {
            spans: vec![StyledSpan::styled(
                line.to_string(),
                SpanStyle {
                    fg: color,
                    ..SpanStyle::default()
                },
            )],
        })
        .collect()
}

/// One unstyled [`StyledLine`] per `\n`-separated line (empty text → none).
pub(crate) fn plain_lines(text: &str) -> Vec<StyledLine> {
    if text.is_empty() {
        return Vec::new();
    }
    text.split('\n').map(StyledLine::plain).collect()
}

/// One-line truncation to `max` chars (newlines flattened to spaces).
pub(crate) fn truncate(s: &str, max: usize) -> String {
    let flat = s.replace('\n', " ");
    if flat.chars().count() <= max {
        flat
    } else {
        let cut: String = flat.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

/// A dim-colored span in the active theme.
pub(crate) fn dim_span(text: String, theme: &Theme) -> StyledSpan {
    StyledSpan::styled(
        text,
        SpanStyle {
            fg: theme.dim,
            ..SpanStyle::default()
        },
    )
}

/// The adapter [`HistoryCell`] over a whole [`RenderedMessage`], delegating
/// to [`crate::message::render_message`], so rendering of the variants not
/// yet split into per-variant cells is byte-identical to the pre-transcript
/// renderer (plan Phase 3 step 5; the second message-cells phase retires it).
#[derive(Debug)]
pub struct MessageHistoryCell {
    message: RenderedMessage,
}

impl MessageHistoryCell {
    /// Wrap one rendered message.
    #[must_use]
    pub fn new(message: RenderedMessage) -> Self {
        Self { message }
    }

    /// The wrapped message.
    #[must_use]
    pub fn message(&self) -> &RenderedMessage {
        &self.message
    }

    /// Mutable access to the wrapped message (streaming deltas append to the
    /// active cell's body in place).
    pub fn message_mut(&mut self) -> &mut RenderedMessage {
        &mut self.message
    }
}

impl StyledCell for MessageHistoryCell {
    fn styled_lines(&self, width: usize, theme: &Theme, verbose: bool) -> Vec<StyledLine> {
        crate::message::render_message(&self.message, width, theme, verbose)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assistant(body: &str) -> MessageHistoryCell {
        MessageHistoryCell::new(RenderedMessage::AssistantText {
            body: body.to_string(),
            timestamp: 0,
        })
    }

    /// The adapter's rich lines must be byte-identical to the pre-transcript
    /// flush path: `render_message` → `styled_line_to_ratatui`.
    #[test]
    fn display_lines_match_legacy_render_message_pipeline() {
        let theme = Theme::dark();
        let fixtures = [
            RenderedMessage::AssistantText {
                body: "Hello **world**\n\n- a\n- b".to_string(),
                timestamp: 0,
            },
            RenderedMessage::UserText {
                body: "hi there".to_string(),
                timestamp: 0,
            },
            RenderedMessage::SystemText {
                body: "boom".to_string(),
                timestamp: 0,
                is_error: true,
            },
            RenderedMessage::AssistantThinking {
                thinking: "pondering deeply".to_string(),
                expanded: false,
            },
        ];
        for message in fixtures {
            for verbose in [false, true] {
                let expected: Vec<Line<'static>> =
                    crate::message::render_message(&message, 80, &theme, verbose)
                        .iter()
                        .map(crate::render::styled_line_to_ratatui)
                        .collect();
                let cell = MessageHistoryCell::new(message.clone());
                let got = cell.display_lines(
                    80,
                    &theme,
                    RenderMode {
                        raw: false,
                        verbose,
                    },
                );
                assert_eq!(got, expected, "diverged for {message:?} verbose={verbose}");
            }
        }
    }

    #[test]
    fn raw_mode_display_lines_are_plain_raw_lines() {
        let cell = assistant("Hello **bold**");
        let raw = cell.display_lines(
            80,
            &Theme::dark(),
            RenderMode {
                raw: true,
                verbose: false,
            },
        );
        assert_eq!(raw, cell.raw_lines());
        for line in &raw {
            for span in &line.spans {
                assert_eq!(
                    span.style,
                    ratatui::style::Style::default(),
                    "raw lines carry no styling: {span:?}"
                );
            }
        }
        // The text itself is preserved (markdown-rendered, styles dropped).
        let all: String = raw
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        assert!(all.contains("Hello"), "got: {all}");
        assert!(all.contains("bold"), "got: {all}");
    }

    #[test]
    fn verbose_mode_expands_thinking_body() {
        let cell = MessageHistoryCell::new(RenderedMessage::AssistantThinking {
            thinking: "secret reasoning".to_string(),
            expanded: false,
        });
        let theme = Theme::dark();
        let collapsed: String = cell
            .display_lines(80, &theme, RenderMode::default())
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(collapsed.contains("ctrl+o to expand"), "got: {collapsed}");
        assert!(!collapsed.contains("secret reasoning"), "got: {collapsed}");
        let expanded: String = cell
            .display_lines(
                80,
                &theme,
                RenderMode {
                    raw: false,
                    verbose: true,
                },
            )
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(expanded.contains("secret reasoning"), "got: {expanded}");
    }

    #[test]
    fn desired_height_counts_wrapped_rows_not_logical_lines() {
        // One 100-char logical line: 1 row at width 100+, 10 rows at width 10.
        let cell = MessageHistoryCell::new(RenderedMessage::SystemText {
            body: "a".repeat(100),
            timestamp: 0,
            is_error: false,
        });
        // Width passes through to the renderer, but this variant does not
        // wrap by itself — the Paragraph measurement must account for it.
        assert_eq!(cell.desired_height(120, RenderMode::default()), 1);
        assert_eq!(cell.desired_height(10, RenderMode::default()), 10);
    }

    #[test]
    fn is_visible_reflects_empty_render() {
        let empty = MessageHistoryCell::new(RenderedMessage::UserText {
            body: String::new(),
            timestamp: 0,
        });
        assert!(!empty.is_visible(80), "empty user text renders nothing");
        assert!(assistant("hello").is_visible(80));
    }

    /// `true` when the factory maps `message` to the concrete cell type `T`.
    fn maps_to<T: 'static>(message: RenderedMessage) -> bool {
        cell_for_message(message)
            .as_any()
            .downcast_ref::<T>()
            .is_some()
    }

    #[test]
    fn factory_maps_text_and_system_variants_to_concrete_cells() {
        assert!(maps_to::<message::UserTextCell>(
            RenderedMessage::UserText {
                body: "hi".into(),
                timestamp: 0,
            }
        ));
        assert!(maps_to::<message::AssistantTextCell>(
            RenderedMessage::AssistantText {
                body: "hi".into(),
                timestamp: 0,
            }
        ));
        assert!(maps_to::<message::UserPromptCell>(
            RenderedMessage::UserPrompt { text: "p".into() }
        ));
        assert!(maps_to::<message::UserCommandCell>(
            RenderedMessage::UserCommand {
                command: "help".into(),
                args: String::new(),
                is_skill: false,
            }
        ));
        assert!(maps_to::<message::UserBashInputCell>(
            RenderedMessage::UserBashInput {
                command: "ls".into(),
            }
        ));
        assert!(maps_to::<system::SystemTextCell>(
            RenderedMessage::SystemText {
                body: "s".into(),
                timestamp: 0,
                is_error: false,
            }
        ));
        assert!(maps_to::<system::SystemTextRichCell>(
            RenderedMessage::SystemTextRich {
                body: "s".into(),
                level: tui_core::message::SystemLevel::Info,
            }
        ));
        assert!(maps_to::<system::SystemApiErrorCell>(
            RenderedMessage::SystemApiError {
                error: "e".into(),
                retry_attempt: 1,
                retry_in_seconds: 1,
                max_retries: 3,
                truncated: false,
            }
        ));
        assert!(maps_to::<system::RateLimitCell>(
            RenderedMessage::RateLimit {
                text: "r".into(),
                upsell: None,
            }
        ));
        // Not yet ported: falls back to the adapter cell.
        assert!(maps_to::<MessageHistoryCell>(
            RenderedMessage::AssistantThinking {
                thinking: "t".into(),
                expanded: false,
            }
        ));
    }

    #[test]
    fn factory_maps_tool_variants_to_concrete_cells() {
        assert!(maps_to::<tool::ToolUseCell>(
            RenderedMessage::AssistantToolUse {
                id: protocol::ToolUseId::new(),
                tool: "Read".into(),
                input: serde_json::json!({}),
            }
        ));
        assert!(maps_to::<tool::ToolResultCell>(
            RenderedMessage::UserToolResult {
                id: protocol::ToolUseId::new(),
                tool: "Read".into(),
                result: serde_json::json!("ok"),
                old_string: None,
                new_string: None,
                file_path: None,
            }
        ));
        assert!(maps_to::<tool::CommandOutputCell>(
            RenderedMessage::UserBashOutput {
                stdout: "o".into(),
                stderr: String::new(),
            }
        ));
        assert!(maps_to::<tool::CommandOutputCell>(
            RenderedMessage::UserLocalCommandOutput {
                stdout: "o".into(),
                stderr: String::new(),
            }
        ));
        assert!(maps_to::<tool::GroupedToolUseCell>(
            RenderedMessage::GroupedToolUse {
                tool: "Read".into(),
                group_id: protocol::ToolUseId::new(),
                entries: Vec::new(),
            }
        ));
        assert!(maps_to::<tool::CollapsedReadSearchCell>(
            RenderedMessage::CollapsedReadSearch {
                search_count: 0,
                read_count: 0,
                list_count: 0,
                is_active: false,
                group_id: protocol::ToolUseId::new(),
                entries: Vec::new(),
                mem_read: 0,
                mem_search: 0,
                mem_write: 0,
            }
        ));
    }

    #[test]
    fn animation_key_defaults_to_none_and_downcast_roundtrips() {
        let mut cell = assistant("hi");
        assert_eq!(
            HistoryCell::animation_key(&cell, Instant::now()),
            None,
            "static cells are time-invariant"
        );
        // as_any / as_any_mut expose the concrete cell for in-place mutation.
        let concrete = cell
            .as_any_mut()
            .downcast_mut::<MessageHistoryCell>()
            .expect("downcast");
        if let RenderedMessage::AssistantText { body, .. } = concrete.message_mut() {
            body.push_str(" there");
        }
        assert!(matches!(
            cell.as_any().downcast_ref::<MessageHistoryCell>().map(MessageHistoryCell::message),
            Some(RenderedMessage::AssistantText { body, .. }) if body == "hi there"
        ));
    }
}

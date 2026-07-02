//! Transcript history cells (plan Phase 3).
//!
//! Modeled on codex-rs `tui/src/history_cell/mod.rs` — the UI architecture
//! pattern only, over `LingXi`'s own data model: a [`HistoryCell`] is the unit
//! of conversation history. Committed cells are inserted into the terminal's
//! native scrollback exactly once; the transient active (in-flight) cell can
//! mutate in place while streaming and is rendered as the live tail until it
//! is finalized (see [`crate::transcript::Transcript`]).
//!
//! This phase introduces the trait plus one adapter cell,
//! [`MessageHistoryCell`], which wraps a whole
//! [`tui_core::message::RenderedMessage`] and delegates to the existing
//! [`crate::message::render_message`] renderer so output is byte-identical to
//! the pre-transcript flush path. Later plan phases split it into
//! per-variant cells.

use std::any::Any;
use std::time::Instant;

use ratatui::text::{Line, Text};
use ratatui::widgets::{Paragraph, Wrap};
use tui_core::message::RenderedMessage;
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

/// The first [`HistoryCell`]: an adapter over a whole [`RenderedMessage`]
/// delegating to [`crate::message::render_message`], so transcript rendering
/// is byte-identical to the pre-transcript renderer while the per-variant
/// cell split proceeds (plan Phase 3 step 5).
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

impl HistoryCell for MessageHistoryCell {
    fn display_lines(&self, width: u16, theme: &Theme, mode: RenderMode) -> Vec<Line<'static>> {
        if mode.raw {
            return self.raw_lines();
        }
        crate::message::render_message(&self.message, usize::from(width), theme, mode.verbose)
            .iter()
            .map(crate::render::styled_line_to_ratatui)
            .collect()
    }

    fn raw_lines(&self) -> Vec<Line<'static>> {
        // Plain text of the rich render at the renderer's default width; the
        // theme is irrelevant once styles are stripped.
        crate::message::render_message(&self.message, 0, &Theme::dark(), false)
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

//! The conversation transcript: committed history cells, the active
//! (in-flight) cell, and the native-scrollback commit cursor (plan Phase 3).
//!
//! Ports the codex `ChatWidget` transcript-state split (committed cells vs a
//! mutable active cell) onto `LingXi`'s data model. Committed cells are
//! inserted into the terminal's native scrollback exactly once, tracked by
//! [`Transcript::committed_to_terminal`]; the active cell is never committed
//! while streaming — it renders as the live tail
//! ([`Transcript::visible_live_tail`]) and only enters `committed` when
//! [`Transcript::flush_active`] finalizes it, so active text is never
//! double-rendered.

use std::io;
use std::io::Write;

use ratatui::backend::Backend;
use ratatui::text::Line;
use tui_core::message::RenderedMessage;
use tui_core::theme::Theme;

use crate::history_cell::{cell_for_message, HistoryCell, RenderMode};

/// Committed history + active in-flight cell + native-scrollback commit
/// cursor + render mode (rich/raw and verbose/expanded state).
#[derive(Default)]
pub struct Transcript {
    /// Finalized cells, in commit order.
    committed: Vec<Box<dyn HistoryCell>>,
    /// The in-flight (actively streaming) cell, if any. Held out of
    /// `committed` so native-scrollback flushes never commit it early.
    active: Option<Box<dyn HistoryCell>>,
    /// How many of `committed` are already inserted into the terminal's
    /// native scrollback (the commit cursor — cells before it are immutable
    /// terminal history).
    committed_to_terminal: usize,
    /// Rich-vs-raw + verbose/expanded state applied when rendering cells.
    render_mode: RenderMode,
}

impl Transcript {
    /// An empty transcript.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Seed a transcript from an initial conversation: every message is
    /// committed (nothing is streaming yet).
    #[must_use]
    pub fn from_messages(messages: Vec<RenderedMessage>) -> Self {
        Self {
            committed: messages.into_iter().map(cell_for_message).collect(),
            ..Self::default()
        }
    }

    /// Append a finalized cell to the committed history.
    pub fn push_committed(&mut self, cell: Box<dyn HistoryCell>) {
        self.committed.push(cell);
    }

    /// Commit a [`RenderedMessage`] via [`cell_for_message`] — the transcript
    /// owns the `RenderedMessage` → [`HistoryCell`] conversion (per-variant
    /// cells for the ported core variants, the adapter cell otherwise).
    pub fn push_message(&mut self, message: RenderedMessage) {
        self.push_committed(cell_for_message(message));
    }

    /// Replace the active in-flight cell. Callers that must not lose a
    /// still-streaming predecessor call [`Self::flush_active`] first.
    pub fn set_active(&mut self, cell: Box<dyn HistoryCell>) {
        self.active = Some(cell);
    }

    /// Mutate the active cell in place (streaming deltas). Returns the
    /// closure's result, or `None` when no cell is active.
    pub fn mutate_active<R>(&mut self, f: impl FnOnce(&mut dyn HistoryCell) -> R) -> Option<R> {
        self.active.as_deref_mut().map(f)
    }

    /// Finalize the active cell: move it to the end of the committed history
    /// (no-op when idle). It becomes eligible for the next native-scrollback
    /// flush.
    pub fn flush_active(&mut self) {
        if let Some(cell) = self.active.take() {
            self.committed.push(cell);
        }
    }

    /// Insert every not-yet-committed finalized cell into the terminal's
    /// native scrollback (above the bottom viewport), advancing the commit
    /// cursor. Cells that render to no lines are consumed by the cursor
    /// without inserting. A cell contributing a raw escape block (an inline
    /// image) gets it emitted below its lines — rich mode only; raw mode is
    /// copy-friendly text. The active cell is never flushed here — it stays
    /// the live tail until [`Self::flush_active`].
    ///
    /// # Errors
    /// Propagates the first terminal IO error from the history insertion.
    pub fn flush_to_native_scrollback<B: Backend + Write>(
        &mut self,
        terminal: &mut crate::terminal::Terminal<B>,
        width: u16,
        theme: &Theme,
    ) -> io::Result<()> {
        while self.committed_to_terminal < self.committed.len() {
            let cell = &self.committed[self.committed_to_terminal];
            let lines = cell.display_lines(width.max(1), theme, self.render_mode);
            let escape = if self.render_mode.raw {
                None
            } else {
                cell.scrollback_escape()
            };
            self.committed_to_terminal += 1;
            if !lines.is_empty() {
                terminal.insert_history_lines(&lines)?;
            }
            if let Some(escape) = escape {
                terminal.insert_history_image(escape.rows, &escape.escape)?;
            }
        }
        Ok(())
    }

    /// The live tail: what the active cell currently renders (empty when
    /// idle). This — not the committed flush — is how in-flight content
    /// becomes visible, so active text is never double-rendered.
    #[must_use]
    pub fn visible_live_tail(&self, width: u16, theme: &Theme) -> Vec<Line<'static>> {
        self.active.as_ref().map_or_else(Vec::new, |cell| {
            cell.display_lines(width.max(1), theme, self.render_mode)
        })
    }

    /// Drop all transcript state: committed cells, the active cell, and the
    /// native-scrollback commit cursor (`/clear`).
    pub fn clear(&mut self) {
        self.committed.clear();
        self.active = None;
        self.committed_to_terminal = 0;
    }

    /// The committed cells, in commit order (the active cell is excluded).
    #[must_use]
    pub fn committed_cells(&self) -> &[Box<dyn HistoryCell>] {
        &self.committed
    }

    /// The active in-flight cell, if any.
    #[must_use]
    pub fn active_cell(&self) -> Option<&dyn HistoryCell> {
        self.active.as_deref()
    }

    /// The native-scrollback commit cursor: how many committed cells are
    /// already inserted into the terminal.
    #[must_use]
    pub fn committed_to_terminal(&self) -> usize {
        self.committed_to_terminal
    }

    /// `true` when the transcript holds nothing (no committed cells and no
    /// active cell).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.committed.is_empty() && self.active.is_none()
    }

    /// The render mode applied to cell rendering.
    #[must_use]
    pub fn render_mode(&self) -> RenderMode {
        self.render_mode
    }

    /// Replace the render mode (rich/raw + verbose).
    pub fn set_render_mode(&mut self, mode: RenderMode) {
        self.render_mode = mode;
    }

    /// Whether collapsible content renders expanded (Ctrl-O state).
    #[must_use]
    pub fn verbose(&self) -> bool {
        self.render_mode.verbose
    }

    /// Set the verbose/expanded state.
    pub fn set_verbose(&mut self, verbose: bool) {
        self.render_mode.verbose = verbose;
    }

    /// Flip the verbose/expanded state; returns the new value.
    pub fn toggle_verbose(&mut self) -> bool {
        self.render_mode.verbose = !self.render_mode.verbose;
        self.render_mode.verbose
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;
    use crate::history_cell::message::AssistantTextCell;
    use crate::terminal::test_support::TestWriteBackend;
    use crate::terminal::Terminal;

    /// An 80x24 bottom-anchored test terminal with a 4-row viewport plus the
    /// raw escape-byte capture handle (history inserts are raw writes).
    fn test_terminal() -> (Terminal<TestWriteBackend>, Rc<RefCell<Vec<u8>>>) {
        let backend = TestWriteBackend::new(80, 24);
        let raw = backend.raw_handle();
        let mut terminal = Terminal::with_options(backend).expect("test terminal");
        terminal.set_bottom_viewport_height(4).expect("viewport");
        // Drop the setup escapes so tests only see flush output.
        raw.borrow_mut().clear();
        (terminal, raw)
    }

    fn raw_string(raw: &Rc<RefCell<Vec<u8>>>) -> String {
        String::from_utf8_lossy(&raw.borrow()).into_owned()
    }

    fn system(body: &str) -> RenderedMessage {
        RenderedMessage::SystemText {
            body: body.to_string(),
            timestamp: 0,
            is_error: false,
        }
    }

    fn assistant_cell(body: &str) -> Box<dyn HistoryCell> {
        Box::new(AssistantTextCell::new(body.to_string()))
    }

    /// Append `delta` to the active assistant cell; `false` when the active
    /// cell is missing or not assistant text (mirrors the app's delta path).
    fn append_delta(transcript: &mut Transcript, delta: &str) -> bool {
        transcript
            .mutate_active(|cell| {
                if let Some(assistant) = cell.as_any_mut().downcast_mut::<AssistantTextCell>() {
                    assistant.append(delta);
                    true
                } else {
                    false
                }
            })
            .unwrap_or(false)
    }

    #[test]
    fn flush_commits_cells_in_push_order() {
        let mut transcript = Transcript::new();
        transcript.push_message(system("first-line-alpha"));
        transcript.push_message(system("second-line-beta"));
        let (mut terminal, raw) = test_terminal();
        transcript
            .flush_to_native_scrollback(&mut terminal, 80, &Theme::dark())
            .unwrap();
        assert_eq!(transcript.committed_to_terminal(), 2);
        let out = raw_string(&raw);
        let first = out.find("first-line-alpha").expect("first committed");
        let second = out.find("second-line-beta").expect("second committed");
        assert!(first < second, "commit order preserved:\n{out}");
    }

    #[test]
    fn commit_cursor_only_flushes_new_cells_and_never_reflushes() {
        let mut transcript = Transcript::new();
        transcript.push_message(system("early-cell"));
        let (mut terminal, raw) = test_terminal();
        transcript
            .flush_to_native_scrollback(&mut terminal, 80, &Theme::dark())
            .unwrap();
        assert_eq!(transcript.committed_to_terminal(), 1);
        transcript.push_message(system("late-cell"));
        transcript
            .flush_to_native_scrollback(&mut terminal, 80, &Theme::dark())
            .unwrap();
        assert_eq!(transcript.committed_to_terminal(), 2);
        let out = raw_string(&raw);
        assert_eq!(
            out.matches("early-cell").count(),
            1,
            "already-committed cells are never re-inserted:\n{out}"
        );
        assert_eq!(out.matches("late-cell").count(), 1);
    }

    #[test]
    fn invisible_cells_advance_the_cursor_without_inserting() {
        let mut transcript = Transcript::new();
        // Empty user text renders to zero lines.
        transcript.push_message(RenderedMessage::UserText {
            body: String::new(),
            timestamp: 0,
        });
        transcript.push_message(system("visible"));
        let (mut terminal, _raw) = test_terminal();
        transcript
            .flush_to_native_scrollback(&mut terminal, 80, &Theme::dark())
            .unwrap();
        assert_eq!(transcript.committed_to_terminal(), 2);
    }

    #[test]
    fn active_cell_mutates_in_place_and_shows_in_live_tail() {
        let mut transcript = Transcript::new();
        transcript.set_active(assistant_cell("Hel"));
        assert!(append_delta(&mut transcript, "lo"));
        let tail = transcript.visible_live_tail(80, &Theme::dark());
        let text: String = tail.iter().map(ToString::to_string).collect();
        assert!(text.contains("Hello"), "live tail shows the delta: {text}");
        assert!(
            transcript.committed_cells().is_empty(),
            "mutation never commits"
        );
    }

    #[test]
    fn mutate_active_returns_none_when_idle() {
        let mut transcript = Transcript::new();
        assert_eq!(transcript.mutate_active(|_| 42), None);
        assert!(!append_delta(&mut transcript, "x"));
        assert!(transcript.visible_live_tail(80, &Theme::dark()).is_empty());
    }

    #[test]
    fn flush_active_moves_active_to_committed_end() {
        let mut transcript = Transcript::new();
        transcript.push_message(system("older"));
        transcript.set_active(assistant_cell("streamed"));
        transcript.flush_active();
        assert!(transcript.active_cell().is_none());
        assert_eq!(transcript.committed_cells().len(), 2);
        let last = transcript.committed_cells()[1]
            .as_any()
            .downcast_ref::<AssistantTextCell>()
            .expect("assistant cell");
        assert_eq!(last.body(), "streamed");
        // Flushing again is a no-op.
        transcript.flush_active();
        assert_eq!(transcript.committed_cells().len(), 2);
    }

    #[test]
    fn live_tail_and_committed_flush_never_double_render_active_text() {
        let mut transcript = Transcript::new();
        transcript.push_message(system("finalized-text"));
        transcript.set_active(assistant_cell("streaming-tail-text"));
        let (mut terminal, raw) = test_terminal();
        let theme = Theme::dark();
        transcript
            .flush_to_native_scrollback(&mut terminal, 80, &theme)
            .unwrap();
        let out = raw_string(&raw);
        assert!(out.contains("finalized-text"), "committed flushed:\n{out}");
        assert!(
            !out.contains("streaming-tail-text"),
            "active text must NOT be committed while streaming:\n{out}"
        );
        let tail: String = transcript
            .visible_live_tail(80, &theme)
            .iter()
            .map(ToString::to_string)
            .collect();
        assert!(tail.contains("streaming-tail-text"), "got: {tail}");

        // Once finalized, the text commits exactly once and leaves the tail.
        transcript.flush_active();
        transcript
            .flush_to_native_scrollback(&mut terminal, 80, &theme)
            .unwrap();
        let out = raw_string(&raw);
        assert_eq!(out.matches("streaming-tail-text").count(), 1);
        assert!(transcript.visible_live_tail(80, &theme).is_empty());
    }

    #[test]
    fn clear_resets_cells_active_and_commit_cursor() {
        let mut transcript = Transcript::from_messages(vec![system("a"), system("b")]);
        let (mut terminal, _raw) = test_terminal();
        transcript
            .flush_to_native_scrollback(&mut terminal, 80, &Theme::dark())
            .unwrap();
        transcript.set_active(assistant_cell("mid-stream"));
        assert_eq!(transcript.committed_to_terminal(), 2);
        transcript.clear();
        assert!(transcript.is_empty());
        assert!(transcript.committed_cells().is_empty());
        assert!(transcript.active_cell().is_none());
        assert_eq!(transcript.committed_to_terminal(), 0);
    }

    #[test]
    fn verbose_state_expands_collapsible_cells_at_flush_time() {
        let thinking = RenderedMessage::AssistantThinking {
            thinking: "hidden reasoning body".to_string(),
            expanded: false,
        };
        // Collapsed (default): the placeholder commits, not the body.
        let mut collapsed = Transcript::new();
        collapsed.push_message(thinking.clone());
        let (mut terminal, raw) = test_terminal();
        collapsed
            .flush_to_native_scrollback(&mut terminal, 80, &Theme::dark())
            .unwrap();
        let out = raw_string(&raw);
        assert!(out.contains("ctrl+o to expand"), "collapsed:\n{out}");
        assert!(!out.contains("hidden reasoning body"), "collapsed:\n{out}");

        // Verbose: the body commits.
        let mut expanded = Transcript::new();
        assert!(expanded.toggle_verbose(), "toggle returns the new state");
        assert!(expanded.verbose());
        expanded.push_message(thinking);
        let (mut terminal, raw) = test_terminal();
        expanded
            .flush_to_native_scrollback(&mut terminal, 80, &Theme::dark())
            .unwrap();
        let out = raw_string(&raw);
        assert!(out.contains("hidden reasoning body"), "expanded:\n{out}");
    }

    #[test]
    fn render_mode_raw_flushes_plain_lines() {
        let mut transcript = Transcript::new();
        transcript.set_render_mode(RenderMode {
            raw: true,
            verbose: false,
        });
        assert!(transcript.render_mode().raw);
        transcript.push_message(RenderedMessage::AssistantText {
            body: "plain-raw-body".to_string(),
            timestamp: 0,
        });
        let (mut terminal, raw) = test_terminal();
        transcript
            .flush_to_native_scrollback(&mut terminal, 80, &Theme::dark())
            .unwrap();
        assert_eq!(transcript.committed_to_terminal(), 1);
        assert!(raw_string(&raw).contains("plain-raw-body"));
    }

    #[test]
    fn from_messages_seeds_committed_only() {
        let transcript = Transcript::from_messages(vec![system("seeded")]);
        assert_eq!(transcript.committed_cells().len(), 1);
        assert!(transcript.active_cell().is_none());
        assert_eq!(transcript.committed_to_terminal(), 0, "nothing flushed yet");
        assert!(!transcript.is_empty());
    }

    /// An image cell on a graphics-capable terminal: the text fallback line
    /// commits first, the inline-image escape follows below it. Raw render
    /// mode (copy-friendly) suppresses the escape but keeps the fallback.
    #[test]
    fn image_cell_flush_emits_text_fallback_then_inline_escape() {
        use crate::history_cell::attachments::UserImageCell;
        use crate::term_image::ImageProtocol;

        let png = std::env::temp_dir().join(format!(
            "tui-rata-transcript-img-{}.png",
            std::process::id()
        ));
        image::RgbaImage::new(4, 20).save(&png).expect("test png");

        let cell = |protocol| {
            Box::new(UserImageCell::with_protocol(
                Some(7),
                None,
                Some(png.display().to_string()),
                protocol,
            ))
        };

        // Rich mode + kitty: fallback line then the escape.
        let mut transcript = Transcript::new();
        transcript.push_committed(cell(ImageProtocol::Kitty));
        let (mut terminal, raw) = test_terminal();
        transcript
            .flush_to_native_scrollback(&mut terminal, 80, &Theme::dark())
            .unwrap();
        let out = raw_string(&raw);
        let fallback = out.find("[Image #7]").expect("text fallback committed");
        let escape = out.find("\x1b_Ga=T,f=100,r=2,").expect("kitty escape");
        assert!(fallback < escape, "fallback precedes the image:\n{out:?}");
        assert_eq!(transcript.committed_to_terminal(), 1);

        // Raw mode: fallback only, no escape.
        let mut transcript = Transcript::new();
        transcript.set_render_mode(RenderMode {
            raw: true,
            verbose: false,
        });
        transcript.push_committed(cell(ImageProtocol::Kitty));
        let (mut terminal, raw) = test_terminal();
        transcript
            .flush_to_native_scrollback(&mut terminal, 80, &Theme::dark())
            .unwrap();
        let out = raw_string(&raw);
        assert!(out.contains("[Image #7]"), "fallback stays in raw mode");
        assert!(!out.contains("\x1b_G"), "no escape in raw mode:\n{out:?}");

        // No graphics support: fallback only.
        let mut transcript = Transcript::new();
        transcript.push_committed(cell(ImageProtocol::None));
        let (mut terminal, raw) = test_terminal();
        transcript
            .flush_to_native_scrollback(&mut terminal, 80, &Theme::dark())
            .unwrap();
        let out = raw_string(&raw);
        assert!(out.contains("[Image #7]"));
        assert!(!out.contains("\x1b_G"));

        std::fs::remove_file(&png).ok();
    }
}

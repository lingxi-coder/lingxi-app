//! The production terminal substrate: a bottom-anchored inline terminal,
//! derived from codex's `custom_terminal.rs` (itself derived from
//! `ratatui::Terminal`, MIT-licensed).
//!
//! Unlike `ratatui::Terminal` with `Viewport::Inline`, this keeps the viewport
//! as an ABSOLUTE `Rect` on screen ([`Terminal::viewport_area`]) that the caller
//! repositions via [`Terminal::set_bottom_viewport_height`]. Finalized history
//! is written ABOVE the viewport with scroll-region escapes
//! ([`Terminal::insert_history_lines`]), so the terminal's own native
//! scrollback owns the transcript. This is the model that fixes the
//! ghost-stacking + out-of-buffer panics of the built-in inline path.
//!
//! Raw terminal modes are owned by the [`TerminalSession`] guard (raw mode +
//! bracketed paste on construction, restore on drop — panic-safe), so the
//! runtime cannot leave the user's shell in raw mode.
//!
//! The core invariant (the one whose violation caused the earlier panic):
//! [`Frame::area`] equals the buffer's absolute viewport rect, so every child
//! rect a widget draws into is guaranteed to live inside the allocated buffer.

use std::io;
use std::io::Write;

use crossterm::cursor::MoveDown;
use crossterm::cursor::MoveTo;
use crossterm::cursor::MoveToColumn;
use crossterm::cursor::RestorePosition;
use crossterm::cursor::SavePosition;
use crossterm::cursor::SetCursorStyle;
use crossterm::execute;
use crossterm::queue;
use crossterm::style::Colors;
use crossterm::style::Print;
use crossterm::style::SetAttribute;
use crossterm::style::SetBackgroundColor;
use crossterm::style::SetColors;
use crossterm::style::SetForegroundColor;
use crossterm::terminal::disable_raw_mode;
use crossterm::terminal::enable_raw_mode;
use crossterm::terminal::Clear as CtClear;
use crossterm::terminal::ClearType as CtClearType;
use ratatui::backend::Backend;
use ratatui::backend::ClearType;
use ratatui::buffer::Buffer;
use ratatui::buffer::Cell;
use ratatui::layout::Position;
use ratatui::layout::Rect;
use ratatui::layout::Size;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Widget;
use unicode_width::UnicodeWidthStr;

/// RAII guard for the process-global terminal modes the TUI needs: raw mode +
/// bracketed paste are enabled on construction and restored on drop (including
/// unwind on panic), along with a visible default-style cursor. Construct ONE
/// per interactive run, before building the [`Terminal`], and keep it alive
/// until the terminal has been dropped.
pub struct TerminalSession {
    _private: (),
}

impl TerminalSession {
    /// Enable raw mode + bracketed paste on stdout.
    ///
    /// # Errors
    /// Returns any terminal IO error from enabling raw mode or writing the
    /// bracketed-paste escape.
    pub fn new() -> io::Result<Self> {
        enable_raw_mode()?;
        execute!(io::stdout(), crossterm::event::EnableBracketedPaste)?;
        Ok(Self { _private: () })
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        // Best-effort restore; a failing write must not double-panic.
        let _ = execute!(
            io::stdout(),
            crossterm::event::DisableBracketedPaste,
            SetCursorStyle::DefaultUserShape,
            crossterm::cursor::Show,
        );
        let _ = disable_raw_mode();
    }
}

/// Display width of a cell symbol, ignoring OSC escape sequences (which consume
/// no display columns). Wide glyphs contribute their full width.
fn display_width(s: &str) -> usize {
    if !s.contains('\x1B') {
        return s.width();
    }
    let mut visible = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(ch) = chars.next() {
        if ch == '\x1B' && chars.clone().next() == Some(']') {
            chars.next();
            for c in chars.by_ref() {
                if c == '\x07' {
                    break;
                }
            }
            continue;
        }
        visible.push(ch);
    }
    visible.width()
}

/// A consistent view into the terminal state for one render pass. [`Self::area`]
/// returns the absolute viewport rect; the buffer is sized to exactly that rect.
pub struct Frame<'a> {
    cursor_position: Option<Position>,
    cursor_style: SetCursorStyle,
    viewport_area: Rect,
    buffer: &'a mut Buffer,
}

impl Frame<'_> {
    /// The absolute viewport rect this frame draws into (stable during a render).
    #[must_use]
    pub const fn area(&self) -> Rect {
        self.viewport_area
    }

    /// Render a [`Widget`] into `area` (a sub-rect of [`Self::area`]).
    pub fn render_widget<W: Widget>(&mut self, widget: W, area: Rect) {
        widget.render(area, self.buffer);
    }

    /// Show the cursor at `position` after this frame is flushed.
    pub fn set_cursor_position<P: Into<Position>>(&mut self, position: P) {
        self.cursor_position = Some(position.into());
    }

    /// Set the visible cursor style applied after this frame is flushed.
    pub fn set_cursor_style(&mut self, style: SetCursorStyle) {
        self.cursor_style = style;
    }

    /// The frame's draw buffer, for widgets that need direct cell access.
    pub fn buffer_mut(&mut self) -> &mut Buffer {
        self.buffer
    }
}

/// A double-buffered terminal whose viewport is an absolute on-screen rect.
pub struct Terminal<B>
where
    B: Backend + Write,
{
    backend: B,
    buffers: [Buffer; 2],
    current: usize,
    hidden_cursor: bool,
    /// Absolute on-screen rect of the bottom viewport.
    pub viewport_area: Rect,
    /// Last observed terminal size, used to detect resizes.
    pub last_known_screen_size: Size,
    /// Last cursor position written, used to reposition the inline viewport on
    /// terminal resize.
    pub last_known_cursor_pos: Position,
}

impl<B> Drop for Terminal<B>
where
    B: Backend + Write,
{
    fn drop(&mut self) {
        let _ = self.reset_cursor_style();
        if self.hidden_cursor {
            let _ = self.show_cursor();
        }
        // `reset_cursor_style` only queues; make sure it reaches the terminal
        // even when the cursor was already visible (no flushing call after it).
        let _ = Write::flush(&mut self.backend);
    }
}

impl<B> Terminal<B>
where
    B: Backend + Write,
{
    /// Build a terminal, probing the backend for its size + current cursor
    /// position (the initial viewport anchor).
    pub fn with_options(mut backend: B) -> io::Result<Self> {
        let screen_size = backend.size()?;
        let cursor_pos = backend
            .get_cursor_position()
            .unwrap_or(Position { x: 0, y: 0 });
        Ok(Self {
            backend,
            buffers: [Buffer::empty(Rect::ZERO), Buffer::empty(Rect::ZERO)],
            current: 0,
            hidden_cursor: false,
            viewport_area: Rect::new(0, cursor_pos.y, 0, 0),
            last_known_screen_size: screen_size,
            last_known_cursor_pos: cursor_pos,
        })
    }

    /// A [`Frame`] over the current buffer for one render pass.
    pub fn get_frame(&mut self) -> Frame<'_> {
        Frame {
            cursor_position: None,
            cursor_style: SetCursorStyle::DefaultUserShape,
            viewport_area: self.viewport_area,
            buffer: &mut self.buffers[self.current],
        }
    }

    fn previous_buffer(&self) -> &Buffer {
        &self.buffers[1 - self.current]
    }

    fn previous_buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffers[1 - self.current]
    }

    /// The underlying backend.
    pub fn backend(&self) -> &B {
        &self.backend
    }

    /// The underlying backend, mutably (for raw escape writes).
    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    /// Diff the previous/current buffers and write only the changes.
    pub fn flush(&mut self) -> io::Result<()> {
        let updates = diff_buffers(self.previous_buffer(), &self.buffers[self.current]);
        if let Some(DrawCommand::Put { x, y, .. }) = updates
            .iter()
            .rev()
            .find(|c| matches!(c, DrawCommand::Put { .. }))
        {
            self.last_known_cursor_pos = Position { x: *x, y: *y };
        }
        draw(&mut self.backend, updates.into_iter())
    }

    /// Record a new terminal size (buffers are resized on the next viewport set).
    pub fn resize(&mut self, screen_size: Size) {
        self.last_known_screen_size = screen_size;
    }

    /// Move/resize the viewport to an absolute rect, resizing both buffers to it.
    pub fn set_viewport_area(&mut self, area: Rect) {
        self.buffers[self.current].resize(area);
        self.buffers[1 - self.current].resize(area);
        self.viewport_area = area;
    }

    /// Compute and apply the absolute BOTTOM viewport rect for `height` rows:
    /// full terminal width, anchored at the viewport's current top (initially
    /// the cursor row probed at construction). When growing past the bottom of
    /// the screen, everything above is scrolled up to make room and the
    /// viewport is pinned to the bottom edge (codex `Tui::draw` viewport
    /// sizing). A changed rect clears stale screen content and forces a full
    /// repaint on the next [`Self::draw`].
    ///
    /// # Errors
    /// Returns any backend IO error from probing the size, scrolling, or
    /// clearing.
    pub fn set_bottom_viewport_height(&mut self, height: u16) -> io::Result<()> {
        let size = self.size()?;
        let mut area = self.viewport_area;
        area.height = height.min(size.height);
        area.width = size.width;
        // If the viewport has expanded past the bottom, scroll everything else
        // up to make room.
        if area.bottom() > size.height {
            let scroll_by = area.bottom() - size.height;
            self.backend.scroll_region_up(0..area.top(), scroll_by)?;
            area.y = size.height - area.height;
        }
        if area != self.viewport_area {
            let old = self.viewport_area;
            // The clear position matters on iTerm2: `clear_after_position` wipes
            // to END of display, and the reverse-index history scroll that
            // follows leaks any cleared-but-still-wanted composer row into
            // scrollback as an orphaned frame (codex's bottom-anchored viewport
            // sidesteps this by growing UP into scrollback; ours grows down).
            // So clear only rows that actually become stale:
            //   • pure growth (same top, bottom ≥ old bottom): nothing goes
            //     stale — the composer redraw paints the new rows. No clear.
            //   • shrink at a fixed top (bottom < old bottom): only the rows
            //     BELOW the new viewport are exposed — clear from there down.
            //   • anything else (move, startup-from-empty): clear from the
            //     higher of the two tops, the conservative original behavior.
            let same_top = !old.is_empty() && area.x == old.x && area.top() == old.top();
            if same_top && area.bottom() >= old.bottom() {
                // Pure growth: no destructive clear, just force a full repaint.
                self.set_viewport_area(area);
                self.invalidate_viewport();
            } else {
                let clear_from = if old.is_empty() {
                    area.as_position()
                } else if same_top {
                    // Shrink: the composer above `area.bottom()` stays valid;
                    // only the vacated rows below it need erasing.
                    Position::new(0, area.bottom())
                } else {
                    old.as_position()
                };
                self.clear_after_position(clear_from)?;
                self.set_viewport_area(area);
            }
        }
        Ok(())
    }

    /// Insert `lines` into the terminal's native scrollback ABOVE the viewport
    /// (codex `insert_history_lines`, standard mode): when the viewport is not
    /// yet at the bottom of the screen it is first scrolled down to make room;
    /// then a scroll region confined to the rows above the viewport receives
    /// the new lines, so the viewport itself never moves or repaints. The
    /// cursor position is restored afterwards. Lines wider than the viewport
    /// wrap in the terminal and are accounted row-exactly.
    ///
    /// # Errors
    /// Returns any backend IO error from writing the escape sequences.
    pub fn insert_history_lines(&mut self, lines: &[Line<'_>]) -> io::Result<()> {
        if lines.is_empty() {
            return Ok(());
        }
        let screen_size = self.size().unwrap_or(Size::new(0, 0));
        let mut area = self.viewport_area;
        let mut should_update_area = false;
        let last_cursor_pos = self.last_known_cursor_pos;

        let wrap_width = usize::from(area.width.max(1));
        let wrapped_rows: usize = lines
            .iter()
            .map(|line| line.width().max(1).div_ceil(wrap_width))
            .sum();
        let wrapped_lines = u16::try_from(wrapped_rows).unwrap_or(u16::MAX);

        let writer = &mut self.backend;
        let cursor_top = if area.bottom() < screen_size.height {
            // The viewport is not at the bottom of the screen: scroll it down
            // to make room. Don't scroll it past the bottom of the screen.
            let scroll_amount = wrapped_lines.min(screen_size.height - area.bottom());

            let top_1based = area.top() + 1;
            queue!(writer, SetScrollRegion(top_1based..screen_size.height))?;
            queue!(writer, MoveTo(0, area.top()))?;
            for _ in 0..scroll_amount {
                // Reverse Index: scroll the region down one row.
                queue!(writer, Print("\x1bM"))?;
            }
            queue!(writer, ResetScrollRegion)?;

            let cursor_top = area.top().saturating_sub(1);
            area.y += scroll_amount;
            should_update_area = true;
            cursor_top
        } else {
            area.top().saturating_sub(1)
        };

        // Confine the scroll region to the rows ABOVE the viewport, put the
        // cursor at its bottom, and write the lines there: only history rows
        // scroll; the viewport stays put.
        queue!(writer, SetScrollRegion(1..area.top()))?;
        queue!(writer, MoveTo(0, cursor_top))?;
        for line in lines {
            queue!(writer, Print("\r\n"))?;
            write_history_line(writer, line, wrap_width)?;
        }
        queue!(writer, ResetScrollRegion)?;
        // NB: MoveTo instead of set_cursor_position, so the terminal's
        // last-known cursor position is untouched — inserting history is
        // cursor-position-neutral.
        queue!(writer, MoveTo(last_cursor_pos.x, last_cursor_pos.y))?;

        if should_update_area {
            self.set_viewport_area(area);
        }
        Ok(())
    }

    /// Insert a raw escape block (an inline image) into the native scrollback
    /// ABOVE the viewport: reserve `rows` blank rows via
    /// [`Self::insert_history_lines`] (which owns all the room-making/scroll
    /// arithmetic), then anchor the escape at the reserved block's top-left —
    /// kitty/iTerm2 draw down-and-right from the cursor — and restore the
    /// cursor. Callers size the escape to exactly `rows` terminal rows (see
    /// `history_cell::ScrollbackEscape`). Zero rows or an empty escape is a
    /// no-op.
    ///
    /// # Errors
    /// Returns any backend IO error from writing the escape sequences.
    pub fn insert_history_image(&mut self, rows: u16, escape: &str) -> io::Result<()> {
        if rows == 0 || escape.is_empty() {
            return Ok(());
        }
        let blank = vec![Line::default(); usize::from(rows)];
        self.insert_history_lines(&blank)?;
        // The reserved rows sit directly above the (possibly just-moved)
        // viewport. On a terminal too short to fit them all, saturate to the
        // top row — the escape clips instead of erroring.
        let top = self.viewport_area.top().saturating_sub(rows);
        let last_cursor_pos = self.last_known_cursor_pos;
        queue!(
            self.backend,
            MoveTo(0, top),
            Print(escape),
            // Cursor-position-neutral, matching insert_history_lines.
            MoveTo(last_cursor_pos.x, last_cursor_pos.y)
        )?;
        Ok(())
    }

    /// The last fully drawn frame buffer (test introspection: the custom
    /// terminal flushes escape diffs to the backend writer, so a cell-grid
    /// snapshot only exists in the double buffer).
    #[cfg(test)]
    pub(crate) fn last_frame_buffer(&self) -> &Buffer {
        self.previous_buffer()
    }

    /// Resize internal state if the backend size changed since last known.
    pub fn autoresize(&mut self) -> io::Result<()> {
        let screen_size = self.size()?;
        if screen_size != self.last_known_screen_size {
            self.resize(screen_size);
        }
        Ok(())
    }

    /// Draw one frame: autoresize, run `render_callback`, flush the diff, place
    /// (or hide) the cursor, and swap buffers.
    pub fn draw<F>(&mut self, render_callback: F) -> io::Result<()>
    where
        F: FnOnce(&mut Frame),
    {
        self.autoresize()?;
        let mut frame = self.get_frame();
        render_callback(&mut frame);
        let cursor_position = frame.cursor_position;
        let cursor_style = frame.cursor_style;
        self.flush()?;
        match cursor_position {
            None => self.hide_cursor()?,
            Some(position) => {
                self.set_cursor_style(cursor_style)?;
                self.show_cursor()?;
                self.set_cursor_position(position)?;
            }
        }
        self.swap_buffers();
        Backend::flush(&mut self.backend)?;
        Ok(())
    }

    /// Hide the cursor.
    pub fn hide_cursor(&mut self) -> io::Result<()> {
        self.backend.hide_cursor()?;
        self.hidden_cursor = true;
        Ok(())
    }

    /// Show the cursor.
    pub fn show_cursor(&mut self) -> io::Result<()> {
        self.backend.show_cursor()?;
        self.hidden_cursor = false;
        Ok(())
    }

    /// Whether the cursor is currently hidden (a draw that claims no cursor
    /// position hides it). Test-only introspection.
    #[cfg(test)]
    pub(crate) fn cursor_hidden(&self) -> bool {
        self.hidden_cursor
    }

    /// Apply a visible cursor style.
    pub fn set_cursor_style(&mut self, style: SetCursorStyle) -> io::Result<()> {
        queue!(self.backend, style)
    }

    /// Restore the user's default cursor style.
    pub fn reset_cursor_style(&mut self) -> io::Result<()> {
        self.set_cursor_style(SetCursorStyle::DefaultUserShape)
    }

    /// Query the backend's current cursor position.
    pub fn get_cursor_position(&mut self) -> io::Result<Position> {
        self.backend.get_cursor_position()
    }

    /// Move the cursor and record the position.
    pub fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        let position = position.into();
        self.backend.set_cursor_position(position)?;
        self.last_known_cursor_pos = position;
        Ok(())
    }

    /// Clear from the top of the viewport through the end of screen.
    pub fn clear(&mut self) -> io::Result<()> {
        if self.viewport_area.is_empty() {
            return Ok(());
        }
        self.clear_after_position(self.viewport_area.as_position())
    }

    /// Clear from `position` through the end of screen and force a full redraw.
    pub fn clear_after_position(&mut self, position: Position) -> io::Result<()> {
        self.backend.set_cursor_position(position)?;
        self.backend.clear_region(ClearType::AfterCursor)?;
        self.previous_buffer_mut().reset();
        Ok(())
    }

    /// Force a full repaint on the next draw (after raw escape operations, e.g.
    /// a viewport grow that exposed rows still holding pre-TUI terminal
    /// content).
    ///
    /// `diff_buffers` only emits a cell when it differs from the previous
    /// buffer and only clears *trailing* blanks per row, so a blank (space)
    /// cell in a freshly exposed viewport row equals the reset previous buffer
    /// and is NOT emitted — leaving stale scrollback showing through the
    /// viewport's blank columns (the footer's 2-column indent, the composer
    /// gutter: the `┌`/`川` left-edge artifacts). Marking every previous cell
    /// `skip` makes it unequal to any real (`skip == false`) current cell, so
    /// blanks are repainted as spaces. This is row-bounded (no
    /// clear-to-end-of-display), so it does NOT reintroduce the
    /// scrollback-orphan leak that motivated the no-clear growth path.
    pub fn invalidate_viewport(&mut self) {
        let buf = self.previous_buffer_mut();
        buf.reset();
        for cell in &mut buf.content {
            cell.set_skip(true);
        }
    }

    /// Reset the inactive buffer and swap it in as current.
    pub fn swap_buffers(&mut self) {
        self.previous_buffer_mut().reset();
        self.current = 1 - self.current;
    }

    /// The backend's real size.
    pub fn size(&self) -> io::Result<Size> {
        self.backend.size()
    }

    /// Open a synchronized-update bracket (`CSI ?2026h`): the terminal
    /// buffers everything until the matching [`Self::end_sync_update`], so a
    /// frame's viewport move + history insert + diff repaint apply atomically
    /// (codex `Tui::draw`'s `stdout().sync_update`; the bytes go through the
    /// backend so tests can observe them).
    pub fn begin_sync_update(&mut self) -> io::Result<()> {
        queue!(self.backend, crossterm::terminal::BeginSynchronizedUpdate)
    }

    /// Close the synchronized-update bracket (`CSI ?2026l`) and flush.
    pub fn end_sync_update(&mut self) -> io::Result<()> {
        queue!(self.backend, crossterm::terminal::EndSynchronizedUpdate)?;
        Write::flush(&mut self.backend)
    }
}

/// CSI DECSTBM: set the terminal scroll region to the 1-based row range
/// (`start..end` inclusive of `start`, exclusive semantics follow codex's
/// usage where `end` is the last row inside the region).
#[derive(Debug, Clone, PartialEq, Eq)]
struct SetScrollRegion(std::ops::Range<u16>);

impl crossterm::Command for SetScrollRegion {
    fn write_ansi(&self, f: &mut impl std::fmt::Write) -> std::fmt::Result {
        write!(f, "\x1b[{};{}r", self.0.start, self.0.end)
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> std::io::Result<()> {
        panic!("tried to execute SetScrollRegion command using WinAPI, use ANSI instead");
    }

    #[cfg(windows)]
    fn is_ansi_code_supported(&self) -> bool {
        true
    }
}

/// CSI DECSTBM reset: restore the scroll region to the full screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ResetScrollRegion;

impl crossterm::Command for ResetScrollRegion {
    fn write_ansi(&self, f: &mut impl std::fmt::Write) -> std::fmt::Result {
        write!(f, "\x1b[r")
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> std::io::Result<()> {
        panic!("tried to execute ResetScrollRegion command using WinAPI, use ANSI instead");
    }

    #[cfg(windows)]
    fn is_ansi_code_supported(&self) -> bool {
        true
    }
}

/// Render a single history line into the scrollback region: clear continuation
/// rows for lines that will wrap, set the line-level colors, and write styled
/// spans. The caller is responsible for cursor positioning and the leading
/// `\r\n`.
fn write_history_line<W: Write>(
    writer: &mut W,
    line: &Line<'_>,
    wrap_width: usize,
) -> io::Result<()> {
    let physical_rows = u16::try_from(line.width().max(1).div_ceil(wrap_width)).unwrap_or(u16::MAX);
    if physical_rows > 1 {
        queue!(writer, SavePosition)?;
        for _ in 1..physical_rows {
            queue!(writer, MoveDown(1), MoveToColumn(0))?;
            queue!(writer, CtClear(CtClearType::UntilNewLine))?;
        }
        queue!(writer, RestorePosition)?;
    }
    queue!(
        writer,
        SetColors(Colors::new(
            line.style
                .fg
                .map_or(crossterm::style::Color::Reset, Into::into),
            line.style
                .bg
                .map_or(crossterm::style::Color::Reset, Into::into),
        ))
    )?;
    queue!(writer, CtClear(CtClearType::UntilNewLine))?;
    // Merge the line-level style into each span so the emitted ANSI reflects
    // line styles (e.g. dim system rows).
    let merged_spans: Vec<Span> = line
        .spans
        .iter()
        .map(|s| Span {
            style: s.style.patch(line.style),
            content: s.content.clone(),
        })
        .collect();
    write_spans(writer, merged_spans.iter())
}

/// Write styled spans as ANSI, diffing modifiers/colors between spans and
/// resetting everything at the end (codex `write_spans`).
fn write_spans<'a, I>(writer: &mut impl Write, content: I) -> io::Result<()>
where
    I: IntoIterator<Item = &'a Span<'a>>,
{
    let mut fg = Color::Reset;
    let mut bg = Color::Reset;
    let mut last_modifier = Modifier::empty();
    for span in content {
        let mut modifier = Modifier::empty();
        modifier.insert(span.style.add_modifier);
        modifier.remove(span.style.sub_modifier);
        if modifier != last_modifier {
            ModifierDiff {
                from: last_modifier,
                to: modifier,
            }
            .queue(writer)?;
            last_modifier = modifier;
        }
        let next = (
            span.style.fg.unwrap_or(Color::Reset),
            span.style.bg.unwrap_or(Color::Reset),
        );
        if next != (fg, bg) {
            queue!(writer, SetColors(Colors::new(next.0.into(), next.1.into())))?;
            (fg, bg) = next;
        }
        queue!(writer, Print(span.content.clone()))?;
    }
    queue!(
        writer,
        SetForegroundColor(crossterm::style::Color::Reset),
        SetBackgroundColor(crossterm::style::Color::Reset),
        SetAttribute(crossterm::style::Attribute::Reset),
    )
}

#[derive(Debug)]
enum DrawCommand {
    Put { x: u16, y: u16, cell: Cell },
    ClearToEnd { x: u16, y: u16, bg: Color },
}

/// Compute the minimal set of draw commands between two buffers (verbatim from
/// codex: wide-glyph-aware trailing `ClearToEnd`, per-cell skip for the tail of
/// multi-width glyphs).
fn diff_buffers(a: &Buffer, b: &Buffer) -> Vec<DrawCommand> {
    let previous_buffer = &a.content;
    let next_buffer = &b.content;

    let mut updates = vec![];
    let mut last_nonblank_columns = vec![0u16; a.area.height as usize];
    for y in 0..a.area.height {
        let row_start = y as usize * a.area.width as usize;
        let row_end = row_start + a.area.width as usize;
        let row = &next_buffer[row_start..row_end];
        let bg = row.last().map_or(Color::Reset, |cell| cell.bg);

        let mut last_nonblank_column = 0usize;
        let mut column = 0usize;
        while column < row.len() {
            let cell = &row[column];
            let width = display_width(cell.symbol());
            if cell.symbol() != " " || cell.bg != bg || cell.modifier != Modifier::empty() {
                last_nonblank_column = column + width.saturating_sub(1);
            }
            column += width.max(1);
        }

        if last_nonblank_column + 1 < row.len() {
            let (x, y) = a.pos_of(row_start + last_nonblank_column + 1);
            updates.push(DrawCommand::ClearToEnd { x, y, bg });
        }

        last_nonblank_columns[y as usize] = u16::try_from(last_nonblank_column).unwrap_or(u16::MAX);
    }

    let mut invalidated: usize = 0;
    let mut to_skip: usize = 0;
    for (i, (current, previous)) in next_buffer.iter().zip(previous_buffer.iter()).enumerate() {
        if !current.skip && (current != previous || invalidated > 0) && to_skip == 0 {
            let (x, y) = a.pos_of(i);
            let row = i / a.area.width as usize;
            if x <= last_nonblank_columns[row] {
                updates.push(DrawCommand::Put {
                    x,
                    y,
                    cell: next_buffer[i].clone(),
                });
            }
        }
        to_skip = display_width(current.symbol()).saturating_sub(1);
        let affected_width = display_width(current.symbol()).max(display_width(previous.symbol()));
        invalidated = affected_width.max(invalidated).saturating_sub(1);
    }
    updates
}

fn draw<I>(writer: &mut impl Write, commands: I) -> io::Result<()>
where
    I: Iterator<Item = DrawCommand>,
{
    let mut fg = Color::Reset;
    let mut bg = Color::Reset;
    let mut modifier = Modifier::empty();
    let mut last_pos: Option<Position> = None;
    for command in commands {
        let (x, y) = match command {
            DrawCommand::Put { x, y, .. } | DrawCommand::ClearToEnd { x, y, .. } => (x, y),
        };
        if !matches!(last_pos, Some(p) if x == p.x + 1 && y == p.y) {
            queue!(writer, MoveTo(x, y))?;
        }
        last_pos = Some(Position { x, y });
        match command {
            DrawCommand::Put { cell, .. } => {
                if cell.modifier != modifier {
                    ModifierDiff {
                        from: modifier,
                        to: cell.modifier,
                    }
                    .queue(writer)?;
                    modifier = cell.modifier;
                }
                if cell.fg != fg || cell.bg != bg {
                    queue!(
                        writer,
                        SetColors(Colors::new(cell.fg.into(), cell.bg.into()))
                    )?;
                    fg = cell.fg;
                    bg = cell.bg;
                }
                queue!(writer, Print(cell.symbol()))?;
            }
            DrawCommand::ClearToEnd { bg: clear_bg, .. } => {
                queue!(writer, SetAttribute(crossterm::style::Attribute::Reset))?;
                modifier = Modifier::empty();
                queue!(writer, SetBackgroundColor(clear_bg.into()))?;
                bg = clear_bg;
                queue!(writer, CtClear(CtClearType::UntilNewLine))?;
            }
        }
    }
    queue!(
        writer,
        SetForegroundColor(crossterm::style::Color::Reset),
        SetBackgroundColor(crossterm::style::Color::Reset),
        SetAttribute(crossterm::style::Attribute::Reset),
    )?;
    Ok(())
}

struct ModifierDiff {
    from: Modifier,
    to: Modifier,
}

impl ModifierDiff {
    fn queue<W: io::Write>(self, w: &mut W) -> io::Result<()> {
        use crossterm::style::Attribute as CAttribute;
        let removed = self.from - self.to;
        if removed.contains(Modifier::REVERSED) {
            queue!(w, SetAttribute(CAttribute::NoReverse))?;
        }
        if removed.contains(Modifier::BOLD) {
            queue!(w, SetAttribute(CAttribute::NormalIntensity))?;
            if self.to.contains(Modifier::DIM) {
                queue!(w, SetAttribute(CAttribute::Dim))?;
            }
        }
        if removed.contains(Modifier::ITALIC) {
            queue!(w, SetAttribute(CAttribute::NoItalic))?;
        }
        if removed.contains(Modifier::UNDERLINED) {
            queue!(w, SetAttribute(CAttribute::NoUnderline))?;
        }
        if removed.contains(Modifier::DIM) {
            queue!(w, SetAttribute(CAttribute::NormalIntensity))?;
        }
        if removed.contains(Modifier::CROSSED_OUT) {
            queue!(w, SetAttribute(CAttribute::NotCrossedOut))?;
        }
        if removed.contains(Modifier::SLOW_BLINK) || removed.contains(Modifier::RAPID_BLINK) {
            queue!(w, SetAttribute(CAttribute::NoBlink))?;
        }

        let added = self.to - self.from;
        if added.contains(Modifier::REVERSED) {
            queue!(w, SetAttribute(CAttribute::Reverse))?;
        }
        if added.contains(Modifier::BOLD) {
            queue!(w, SetAttribute(CAttribute::Bold))?;
        }
        if added.contains(Modifier::ITALIC) {
            queue!(w, SetAttribute(CAttribute::Italic))?;
        }
        if added.contains(Modifier::UNDERLINED) {
            queue!(w, SetAttribute(CAttribute::Underlined))?;
        }
        if added.contains(Modifier::DIM) {
            queue!(w, SetAttribute(CAttribute::Dim))?;
        }
        if added.contains(Modifier::CROSSED_OUT) {
            queue!(w, SetAttribute(CAttribute::CrossedOut))?;
        }
        if added.contains(Modifier::SLOW_BLINK) {
            queue!(w, SetAttribute(CAttribute::SlowBlink))?;
        }
        if added.contains(Modifier::RAPID_BLINK) {
            queue!(w, SetAttribute(CAttribute::RapidBlink))?;
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    //! A `ratatui::backend::TestBackend` that also implements [`Write`]: the
    //! custom terminal diff-flushes and inserts history through raw escape
    //! writes, so tests need both the cell grid (via [`Backend`]) and the raw
    //! byte stream (via [`Write`]) to observe it.

    use std::cell::RefCell;
    use std::rc::Rc;

    use ratatui::backend::TestBackend;
    use ratatui::backend::WindowSize;

    use super::{io, Backend, Cell, ClearType, Position, Size, Write};

    /// A [`TestBackend`] wrapper capturing every raw escape byte written.
    pub(crate) struct TestWriteBackend {
        inner: TestBackend,
        raw: Rc<RefCell<Vec<u8>>>,
    }

    impl TestWriteBackend {
        pub(crate) fn new(width: u16, height: u16) -> Self {
            Self {
                inner: TestBackend::new(width, height),
                raw: Rc::new(RefCell::new(Vec::new())),
            }
        }

        /// Shared handle onto the captured raw bytes; stays readable after the
        /// terminal (and backend) have been dropped.
        pub(crate) fn raw_handle(&self) -> Rc<RefCell<Vec<u8>>> {
            Rc::clone(&self.raw)
        }
    }

    impl Write for TestWriteBackend {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.raw.borrow_mut().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Backend for TestWriteBackend {
        fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
        where
            I: Iterator<Item = (u16, u16, &'a Cell)>,
        {
            self.inner.draw(content)
        }

        fn hide_cursor(&mut self) -> io::Result<()> {
            self.inner.hide_cursor()
        }

        fn show_cursor(&mut self) -> io::Result<()> {
            self.inner.show_cursor()
        }

        fn get_cursor_position(&mut self) -> io::Result<Position> {
            self.inner.get_cursor_position()
        }

        fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
            self.inner.set_cursor_position(position)
        }

        fn clear(&mut self) -> io::Result<()> {
            self.inner.clear()
        }

        fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
            self.inner.clear_region(clear_type)
        }

        fn append_lines(&mut self, n: u16) -> io::Result<()> {
            self.inner.append_lines(n)
        }

        fn size(&self) -> io::Result<Size> {
            self.inner.size()
        }

        fn window_size(&mut self) -> io::Result<WindowSize> {
            self.inner.window_size()
        }

        fn flush(&mut self) -> io::Result<()> {
            Backend::flush(&mut self.inner)
        }

        fn scroll_region_up(
            &mut self,
            region: std::ops::Range<u16>,
            line_count: u16,
        ) -> io::Result<()> {
            self.inner.scroll_region_up(region, line_count)
        }

        fn scroll_region_down(
            &mut self,
            region: std::ops::Range<u16>,
            line_count: u16,
        ) -> io::Result<()> {
            self.inner.scroll_region_down(region, line_count)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::TestWriteBackend;
    use super::*;

    fn test_terminal(width: u16, height: u16) -> Terminal<TestWriteBackend> {
        Terminal::with_options(TestWriteBackend::new(width, height)).expect("terminal")
    }

    fn raw_string(raw: &std::rc::Rc<std::cell::RefCell<Vec<u8>>>) -> String {
        String::from_utf8_lossy(&raw.borrow()).into_owned()
    }

    // ===== Plan Phase 1 step 6 tests =====

    #[test]
    fn absolute_viewport_rect_equals_frame_area() {
        let mut terminal = test_terminal(80, 24);
        terminal.set_bottom_viewport_height(4).unwrap();
        // Anchored at the construction-time cursor row (0 for TestBackend).
        assert_eq!(terminal.viewport_area, Rect::new(0, 0, 80, 4));
        assert_eq!(terminal.get_frame().area(), Rect::new(0, 0, 80, 4));

        // The invariant holds for an arbitrary absolute rect too: the frame
        // (and thus its buffer) is exactly the viewport rect.
        let bottom = Rect::new(0, 19, 80, 5);
        terminal.set_viewport_area(bottom);
        let mut drawn_area = Rect::ZERO;
        terminal
            .draw(|frame| {
                drawn_area = frame.area();
                assert_eq!(frame.buffer_mut().area, bottom);
            })
            .unwrap();
        assert_eq!(drawn_area, bottom);
    }

    #[test]
    fn viewport_height_changes_do_not_panic_and_stay_on_screen() {
        let mut terminal = test_terminal(80, 24);
        // Grow, shrink, exceed the screen height, and go minimal — every shape
        // must draw cleanly and stay within the screen.
        for height in [4u16, 9, 12, 6, 20, 24, 40, 1, 4] {
            terminal.set_bottom_viewport_height(height).unwrap();
            let area = terminal.viewport_area;
            assert_eq!(area.width, 80);
            assert_eq!(
                area.height,
                height.min(24),
                "height {height} clamps to screen"
            );
            assert!(
                area.bottom() <= 24,
                "viewport must stay on screen: {area:?}"
            );
            terminal
                .draw(|frame| {
                    let a = frame.area();
                    frame.render_widget(
                        ratatui::widgets::Paragraph::new("x".repeat(usize::from(a.width))),
                        a,
                    );
                })
                .unwrap();
        }
    }

    #[test]
    fn cursor_is_hidden_when_no_cursor_position_is_set() {
        let mut terminal = test_terminal(80, 24);
        terminal.set_bottom_viewport_height(4).unwrap();
        terminal.draw(|_frame| {}).unwrap();
        assert!(terminal.hidden_cursor, "no cursor position set → hidden");

        terminal
            .draw(|frame| frame.set_cursor_position(Position::new(3, 2)))
            .unwrap();
        assert!(!terminal.hidden_cursor, "cursor position set → visible");
        assert_eq!(terminal.get_cursor_position().unwrap(), Position::new(3, 2));
    }

    #[test]
    fn cursor_style_resets_on_drop() {
        let backend = TestWriteBackend::new(80, 24);
        let raw = backend.raw_handle();
        let mut terminal = Terminal::with_options(backend).unwrap();
        terminal.set_bottom_viewport_height(4).unwrap();
        terminal
            .draw(|frame| {
                frame.set_cursor_position(Position::new(0, 0));
                frame.set_cursor_style(SetCursorStyle::SteadyBar);
            })
            .unwrap();
        raw.borrow_mut().clear();
        drop(terminal);
        assert!(
            raw_string(&raw).contains("\x1b[0 q"),
            "drop must emit the DefaultUserShape cursor-style reset; got: {:?}",
            raw_string(&raw)
        );
    }

    #[test]
    fn insert_history_writes_above_bottom_pinned_viewport() {
        let mut terminal = test_terminal(80, 24);
        // Viewport pinned to the bottom 4 rows (20..24).
        terminal.set_viewport_area(Rect::new(0, 20, 80, 4));
        let raw = terminal.backend().raw_handle();
        raw.borrow_mut().clear();

        terminal
            .insert_history_lines(&[Line::from("history-one"), Line::from("history-two")])
            .unwrap();

        let out = raw_string(&raw);
        // The scroll region is confined to the rows ABOVE the viewport
        // (1-based rows 1..=20 exclusive of the viewport top at row 21)…
        let region = out
            .find("\x1b[1;20r")
            .expect("scroll region above viewport");
        // …the cursor is parked on the region's bottom row (0-based 19 →
        // 1-based row 20, column 1), still above the viewport…
        let move_above = out.find("\x1b[20;1H").expect("cursor above viewport");
        // …and both history lines are written inside that region.
        let one = out.find("history-one").expect("first line written");
        let two = out.find("history-two").expect("second line written");
        let reset = out.find("\x1b[r").expect("scroll region reset");
        assert!(region < move_above && move_above < one && one < two && two < reset);
        // Cursor-position-neutral: the write ends by restoring the last known
        // cursor position.
        assert!(out.ends_with("\x1b[1;1H"), "restores cursor: {out:?}");
        // The viewport itself never moved.
        assert_eq!(terminal.viewport_area, Rect::new(0, 20, 80, 4));
        // No room-making reverse-index scroll was needed at the bottom.
        assert!(!out.contains("\x1bM"));
    }

    #[test]
    fn insert_history_scrolls_viewport_down_when_not_at_bottom() {
        let mut terminal = test_terminal(80, 24);
        terminal.set_bottom_viewport_height(4).unwrap();
        assert_eq!(terminal.viewport_area, Rect::new(0, 0, 80, 4));
        let raw = terminal.backend().raw_handle();
        raw.borrow_mut().clear();

        terminal
            .insert_history_lines(&[Line::from("aaa"), Line::from("bbb")])
            .unwrap();

        let out = raw_string(&raw);
        // Reverse-index scrolls make room by pushing the viewport down…
        assert_eq!(out.matches("\x1bM").count(), 2, "two rows of room: {out:?}");
        // …so the viewport slides down by exactly the inserted row count and
        // history occupies the rows above it.
        assert_eq!(terminal.viewport_area, Rect::new(0, 2, 80, 4));
        assert!(out.contains("aaa") && out.contains("bbb"));
    }

    #[test]
    fn osc_escape_width_does_not_shift_diff_output() {
        // An OSC 8 hyperlink wrapping a single visible char occupies ONE cell.
        let osc_link = "\x1b]8;;http://example.com\x07a\x1b]8;;\x07";
        assert_eq!(display_width(osc_link), 1, "OSC bytes are zero-width");
        assert_eq!(display_width("你"), 2, "wide glyphs keep their width");
        assert_eq!(display_width("plain"), 5);

        // The buffer diff must not treat the escape bytes as display columns:
        // cells after an OSC-wrapped symbol still get their own Put commands
        // at consecutive x positions.
        let area = Rect::new(0, 0, 10, 1);
        let prev = Buffer::empty(area);
        let mut next = Buffer::empty(area);
        next.cell_mut(Position::new(0, 0))
            .unwrap()
            .set_symbol(osc_link);
        next.cell_mut(Position::new(1, 0)).unwrap().set_symbol("b");
        next.cell_mut(Position::new(2, 0)).unwrap().set_symbol("c");

        let puts: Vec<(u16, u16, String)> = diff_buffers(&prev, &next)
            .into_iter()
            .filter_map(|cmd| match cmd {
                DrawCommand::Put { x, y, cell } => Some((x, y, cell.symbol().to_string())),
                DrawCommand::ClearToEnd { .. } => None,
            })
            .collect();
        assert_eq!(
            puts,
            vec![
                (0, 0, osc_link.to_string()),
                (1, 0, "b".to_string()),
                (2, 0, "c".to_string()),
            ],
            "OSC escapes must not skip or shift subsequent cells"
        );
    }

    #[test]
    fn invalidate_viewport_repaints_leading_blank_columns() {
        // Regression (footer `┌C` / composer `川` left-edge artifact): an inline
        // viewport drawn over pre-existing terminal content must repaint its
        // blank columns (the footer's 2-col indent, the composer gutter).
        // `diff_buffers` skips a cell equal to the previous buffer and only
        // clears TRAILING blanks per row, so leading/interior blanks that match
        // a reset previous buffer were never emitted — leaving stale scrollback
        // showing through. `invalidate_viewport` marks every previous cell
        // `skip`, so every real (skip=false) cell — blanks included — differs
        // and is repainted.
        let mut term = test_terminal(10, 2);
        term.set_bottom_viewport_height(2).unwrap();
        term.invalidate_viewport();
        assert!(
            term.previous_buffer().content.iter().all(|c| c.skip),
            "invalidate must mark every previous cell skip"
        );

        // A frame with two leading blank columns then text (like "  Enter").
        let area = term.viewport_area;
        let mut next = Buffer::empty(area);
        for (i, ch) in "  Enter".chars().enumerate() {
            next.cell_mut(Position::new(u16::try_from(i).unwrap(), area.y))
                .unwrap()
                .set_symbol(&ch.to_string());
        }
        let leading: Vec<(u16, String)> = diff_buffers(term.previous_buffer(), &next)
            .into_iter()
            .filter_map(|c| match c {
                DrawCommand::Put { x, y, cell } if x < 2 && y == area.y => {
                    Some((x, cell.symbol().to_string()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            leading,
            vec![(0, " ".to_string()), (1, " ".to_string())],
            "leading blank columns must be repainted as spaces after invalidate"
        );
    }

    #[test]
    fn insert_history_with_no_lines_is_a_no_op() {
        let mut terminal = test_terminal(80, 24);
        terminal.set_viewport_area(Rect::new(0, 20, 80, 4));
        let raw = terminal.backend().raw_handle();
        raw.borrow_mut().clear();
        terminal.insert_history_lines(&[]).unwrap();
        assert!(raw.borrow().is_empty());
        assert_eq!(terminal.viewport_area, Rect::new(0, 20, 80, 4));
    }

    #[test]
    fn insert_history_image_reserves_rows_and_anchors_escape_above_viewport() {
        let mut terminal = test_terminal(80, 24);
        // Viewport pinned to the bottom 4 rows (20..24).
        terminal.set_viewport_area(Rect::new(0, 20, 80, 4));
        let raw = terminal.backend().raw_handle();
        raw.borrow_mut().clear();

        terminal
            .insert_history_image(3, "\x1b_Gfake-image\x1b\\")
            .unwrap();

        let out = raw_string(&raw);
        // Room is made through the normal history path (scroll region above
        // the viewport)…
        assert!(
            out.contains("\x1b[1;20r"),
            "room-making scroll region: {out:?}"
        );
        // …then the escape is anchored at the reserved block's top-left:
        // 3 rows above the viewport top (0-based row 17 → 1-based 18, col 1).
        let anchor = out.find("\x1b[18;1H").expect("anchor above viewport");
        let escape = out.find("\x1b_Gfake-image").expect("escape emitted");
        assert!(anchor < escape, "anchor precedes the escape:\n{out:?}");
        // Cursor-position-neutral, like text history inserts.
        assert!(out.ends_with("\x1b[1;1H"), "restores cursor: {out:?}");
        // The viewport itself never moved.
        assert_eq!(terminal.viewport_area, Rect::new(0, 20, 80, 4));
    }

    #[test]
    fn insert_history_image_with_no_rows_or_empty_escape_is_a_no_op() {
        let mut terminal = test_terminal(80, 24);
        terminal.set_viewport_area(Rect::new(0, 20, 80, 4));
        let raw = terminal.backend().raw_handle();
        raw.borrow_mut().clear();
        terminal.insert_history_image(0, "\x1b_Gx\x1b\\").unwrap();
        terminal.insert_history_image(3, "").unwrap();
        assert!(raw.borrow().is_empty());
    }
}

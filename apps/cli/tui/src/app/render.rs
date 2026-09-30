use super::RataApp;
use ratatui::backend::Backend;
use ratatui::layout::Rect;
use std::io;
use std::io::Write;

impl<'cb> RataApp<'cb> {
    /// Seed the terminal surface from startup settings/environment.
    pub fn configure_fullscreen(&mut self, enabled: bool, copy_on_select: bool) {
        self.fullscreen = enabled;
        self.chat_widget.set_collapse_fullscreen(enabled);
        self.copy_on_select = copy_on_select;
        self.selection.clear();
    }
    /// Desired inline-viewport height at `width` columns: the widget reports
    /// its own height ([`crate::chat_widget::ChatWidget::desired_height`]); the 4/20 clamp is
    /// deliberately app-side viewport policy.
    pub(super) fn viewport_height(&self, width: u16) -> u16 {
        self.chat_widget.desired_height(width).clamp(4, 20)
    }
    /// Write the staged hook-returned terminal escape sequences
    /// (`TurnEvent::TerminalSequence`) straight to the terminal's writer —
    /// the host that owns the controlling tty (claude-code `BEo`; the old
    /// backend's async `pump_terminal_sequence` writing to stdout). The
    /// sequences are allowlisted OSC/BEL escapes that never move the cursor,
    /// so writing them between frames cannot corrupt the viewport diff.
    pub(super) fn write_terminal_sequences<B: Backend<Error = std::io::Error> + Write>(
        &mut self,
        terminal: &mut crate::terminal::Terminal<B>,
    ) -> io::Result<()> {
        let sequences = self.chat_widget.take_terminal_sequences();
        if sequences.is_empty() {
            return Ok(());
        }
        let backend = terminal.backend_mut();
        for seq in sequences {
            backend.write_all(seq.as_bytes())?;
        }
        // Disambiguated: the raw writer flush (`io::Write`), not
        // `ratatui::backend::Backend::flush`.
        Write::flush(backend)
    }
    /// Commit finalized transcript cells into the terminal's native
    /// scrollback (see [`crate::chat_widget::ChatWidget::flush_scrollback`]).
    pub(super) fn flush_scrollback<B: Backend<Error = std::io::Error> + Write>(
        &mut self,
        terminal: &mut crate::terminal::Terminal<B>,
    ) -> io::Result<()> {
        self.chat_widget.flush_scrollback(terminal)
    }
    /// Draw one frame through the widget's render contract
    /// ([`crate::chat_widget::ChatWidget::render_frame`] is the frame adapter).
    pub(super) fn draw<B: Backend<Error = std::io::Error> + Write>(
        &mut self,
        terminal: &mut crate::terminal::Terminal<B>,
    ) -> io::Result<()> {
        let chat_widget = &mut self.chat_widget;
        if self.fullscreen {
            terminal.draw(|frame| chat_widget.render_fullscreen_frame(frame))
        } else {
            terminal.draw(|frame| chat_widget.render_frame(frame))
        }
    }
    /// One frame: viewport sizing, history flush, and the widget draw, all
    /// inside a synchronized-update bracket so the terminal applies the frame
    /// atomically (codex `Tui::draw`). The bracket must close even when a
    /// step fails — a dangling `?2026h` freezes the terminal.
    pub(super) fn render_tick<B: Backend<Error = std::io::Error> + Write>(
        &mut self,
        terminal: &mut crate::terminal::Terminal<B>,
    ) -> io::Result<()> {
        terminal.begin_sync_update()?;
        let result = (|| {
            if self.chat_widget.take_terminal_replay_required() {
                terminal.reset_for_replay()?;
                self.selection.clear();
            }
            let size = terminal.size()?;
            self.chat_widget.set_terminal_rows(size.height);
            if self.fullscreen {
                terminal.resize(size);
                let full = Rect::new(0, 0, size.width, size.height);
                if terminal.viewport_area != full {
                    terminal.set_viewport_area(full);
                    terminal.invalidate_viewport();
                    self.selection.clear();
                }
            } else {
                terminal.set_bottom_viewport_height(self.viewport_height(size.width))?;
                self.flush_scrollback(terminal)?;
            }
            self.draw(terminal)
        })();
        let end = terminal.end_sync_update();
        result.and(end)
    }
}

//! The composer: the multi-line input buffer with a cursor + submitted-prompt
//! history recall.
//!
//! Backend-neutral editing state (no ratatui/crossterm types on the model): the
//! app routes key semantics into these methods and reads [`Composer::lines`] +
//! [`Composer::cursor_row_col`] to render. The buffer is a `Vec<char>` so cursor
//! indexing is O(1) and free of byte/char-boundary bugs.
//!
//! Multi-line: `Enter` submits, a modified `Enter` (Alt/Shift) inserts a
//! newline. `Up`/`Down` move the cursor between lines, and recall history only
//! when already on the first/last line — the standard CLI composer feel.
//!
//! Rendering lives in the separate [`ComposerView`] (the model above stays
//! backend-neutral): a [`Renderable`] that draws codex's borderless shape (a
//! background-styled block with a bold `›` gutter prompt) into
//! `(Rect, &mut Buffer)` and reports the CJK-aware cursor position.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use crate::renderable::Renderable;

/// Maximum composer content lines shown before the box stops growing and the
/// content scrolls to follow the cursor.
pub const MAX_VISIBLE_LINES: usize = 6;

/// The input buffer + cursor + submitted-prompt history.
#[derive(Debug, Default)]
pub struct Composer {
    chars: Vec<char>,
    /// Cursor position as a char index in `0..=chars.len()`.
    cursor: usize,
    /// Submitted prompts, oldest first.
    history: Vec<String>,
    /// `None` while editing the live buffer; `Some(i)` while browsing
    /// `history[i]`.
    browse: Option<usize>,
    /// The live buffer saved while browsing history (restored on return).
    stash: Vec<char>,
    /// Visual-selection anchor (char index); `Some` while a selection is active.
    anchor: Option<usize>,
}

impl Composer {
    /// The current buffer text (may contain `\n`).
    #[must_use]
    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    /// Whether the buffer is empty ignoring surrounding whitespace.
    #[must_use]
    pub fn is_blank(&self) -> bool {
        self.text().trim().is_empty()
    }

    /// The buffer split into visual lines (always at least one line).
    #[must_use]
    pub fn lines(&self) -> Vec<String> {
        self.text().split('\n').map(str::to_string).collect()
    }

    /// The cursor's `(row, col)` in the line grid (both 0-based, in `char`s).
    #[must_use]
    pub fn cursor_row_col(&self) -> (usize, usize) {
        let mut row = 0;
        let mut col = 0;
        for &c in &self.chars[..self.cursor] {
            if c == '\n' {
                row += 1;
                col = 0;
            } else {
                col += 1;
            }
        }
        (row, col)
    }

    /// Insert a character at the cursor (adopts the browsed history line as the
    /// live buffer first).
    pub fn insert(&mut self, c: char) {
        self.detach_history();
        self.chars.insert(self.cursor, c);
        self.cursor += 1;
    }

    /// Insert a newline at the cursor (modified-Enter).
    pub fn insert_newline(&mut self) {
        self.insert('\n');
    }

    /// Replace the whole buffer with `text`, cursor at the end (used by command
    /// completion). Detaches any history browsing.
    pub fn replace_all(&mut self, text: &str) {
        self.detach_history();
        self.chars = text.chars().collect();
        self.cursor = self.chars.len();
    }

    /// If the whitespace-delimited token ending at the cursor starts with `@`,
    /// return `(at_index, fragment)` — the char index of the `@` and the text
    /// between it and the cursor (used to drive `@file` completion).
    #[must_use]
    pub fn at_fragment(&self) -> Option<(usize, String)> {
        let mut start = self.cursor;
        while start > 0 && !self.chars[start - 1].is_whitespace() {
            start -= 1;
        }
        if start < self.cursor && self.chars[start] == '@' {
            let fragment = self.chars[start + 1..self.cursor].iter().collect();
            Some((start, fragment))
        } else {
            None
        }
    }

    /// Replace the current `@`-token's fragment (from just after the `@` at
    /// `at_index` up to the cursor) with `insert`, leaving the cursor after it.
    pub fn complete_at(&mut self, at_index: usize, insert: &str) {
        self.detach_history();
        let start = at_index + 1;
        let end = self.cursor.min(self.chars.len());
        if start <= end {
            self.chars.splice(start..end, insert.chars());
            self.cursor = start + insert.chars().count();
        }
    }

    /// Delete the character before the cursor (`Backspace`).
    pub fn backspace(&mut self) {
        self.detach_history();
        if self.cursor > 0 {
            self.cursor -= 1;
            self.chars.remove(self.cursor);
        }
    }

    /// Delete the character at the cursor (`Delete`).
    pub fn delete(&mut self) {
        self.detach_history();
        if self.cursor < self.chars.len() {
            self.chars.remove(self.cursor);
        }
    }

    /// Move the cursor one char left.
    pub fn move_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    /// Move the cursor one char right.
    pub fn move_right(&mut self) {
        if self.cursor < self.chars.len() {
            self.cursor += 1;
        }
    }

    /// Move the cursor to the start of the current line.
    pub fn home(&mut self) {
        self.cursor = self.line_start(self.cursor);
    }

    /// Move the cursor to the end of the current line.
    pub fn end(&mut self) {
        self.cursor = self.line_end(self.cursor);
    }

    /// Move the cursor to the start of the previous word.
    pub fn move_word_left(&mut self) {
        self.cursor = self.word_left_from(self.cursor);
    }

    /// Move the cursor to the start of the next word.
    pub fn move_word_right(&mut self) {
        let mut i = self.cursor;
        while i < self.chars.len() && self.chars[i].is_whitespace() {
            i += 1;
        }
        while i < self.chars.len() && !self.chars[i].is_whitespace() {
            i += 1;
        }
        self.cursor = i;
    }

    /// Vim `w`: move to the START of the next word (skip the rest of the current
    /// word, then any whitespace).
    pub fn next_word_start(&mut self) {
        let mut i = self.cursor;
        while i < self.chars.len() && !self.chars[i].is_whitespace() {
            i += 1;
        }
        while i < self.chars.len() && self.chars[i].is_whitespace() {
            i += 1;
        }
        self.cursor = i;
    }

    /// Vim `e`: move to the END of the current/next word (advance at least one,
    /// skip whitespace, then land on the last char of the word).
    pub fn word_end(&mut self) {
        let mut i = self.cursor + 1;
        while i < self.chars.len() && self.chars[i].is_whitespace() {
            i += 1;
        }
        while i + 1 < self.chars.len() && !self.chars[i + 1].is_whitespace() {
            i += 1;
        }
        self.cursor = i.min(self.chars.len().saturating_sub(1));
    }

    /// Begin a visual selection anchored at the cursor.
    pub fn start_selection(&mut self) {
        self.anchor = Some(self.cursor);
    }

    /// Drop any active selection.
    pub fn clear_selection(&mut self) {
        self.anchor = None;
    }

    /// Whether a selection is active.
    #[must_use]
    pub fn has_selection(&self) -> bool {
        self.anchor.is_some()
    }

    /// The inclusive-of-cursor selection span `[start, end)` in char indices
    /// (anchor..=cursor, normalized), or `None` when no selection is active.
    #[must_use]
    pub fn selection_range(&self) -> Option<(usize, usize)> {
        self.anchor.map(|a| {
            let lo = a.min(self.cursor);
            let hi = a.max(self.cursor);
            // Vim visual is inclusive of the cursor cell.
            (lo, (hi + 1).min(self.chars.len()))
        })
    }

    /// The selected text, or `None` when no selection is active.
    #[must_use]
    pub fn selected_text(&self) -> Option<String> {
        self.selection_range()
            .map(|(lo, hi)| self.chars[lo..hi].iter().collect())
    }

    /// Delete the active selection, returning the removed text; clears the
    /// selection and places the cursor at its start. `None` when inactive.
    pub fn delete_selection(&mut self) -> Option<String> {
        let (lo, hi) = self.selection_range()?;
        self.detach_history();
        let removed: String = self.chars.drain(lo..hi).collect();
        self.cursor = lo;
        self.anchor = None;
        Some(removed)
    }

    /// Insert a string at the cursor (used by vim paste), cursor after it.
    pub fn insert_str(&mut self, s: &str) {
        self.detach_history();
        let chars: Vec<char> = s.chars().collect();
        let n = chars.len();
        self.chars.splice(self.cursor..self.cursor, chars);
        self.cursor += n;
    }

    /// Delete from the cursor back to the previous word boundary.
    pub fn delete_word(&mut self) {
        self.detach_history();
        let target = self.word_left_from(self.cursor);
        self.chars.drain(target..self.cursor);
        self.cursor = target;
    }

    /// Delete from the start of the current line up to the cursor.
    pub fn kill_to_line_start(&mut self) {
        self.detach_history();
        let start = self.line_start(self.cursor);
        self.chars.drain(start..self.cursor);
        self.cursor = start;
    }

    /// Delete from the cursor to the end of the current line (vim `D`).
    pub fn kill_to_line_end(&mut self) {
        self.detach_history();
        let end = self.line_end(self.cursor);
        self.chars.drain(self.cursor..end);
    }

    /// Delete the whole current line, including its trailing newline (vim `dd`).
    pub fn delete_line(&mut self) {
        self.detach_history();
        let start = self.line_start(self.cursor);
        let mut end = self.line_end(self.cursor);
        if end < self.chars.len() && self.chars[end] == '\n' {
            end += 1;
        }
        self.chars.drain(start..end);
        self.cursor = start.min(self.chars.len());
    }

    /// Move the cursor up one line preserving the column (vim `k`); unlike
    /// [`Self::up`], never recalls history.
    pub fn cursor_line_up(&mut self) {
        let (row, col) = self.cursor_row_col();
        if row > 0 {
            self.set_cursor_row_col(row - 1, col);
        }
    }

    /// Move the cursor down one line preserving the column (vim `j`); unlike
    /// [`Self::down`], never recalls history.
    pub fn cursor_line_down(&mut self) {
        let (row, col) = self.cursor_row_col();
        if row + 1 < self.lines().len() {
            self.set_cursor_row_col(row + 1, col);
        }
    }

    fn line_start(&self, from: usize) -> usize {
        let mut i = from;
        while i > 0 && self.chars[i - 1] != '\n' {
            i -= 1;
        }
        i
    }

    fn line_end(&self, from: usize) -> usize {
        let mut i = from;
        while i < self.chars.len() && self.chars[i] != '\n' {
            i += 1;
        }
        i
    }

    fn word_left_from(&self, from: usize) -> usize {
        let mut i = from;
        while i > 0 && self.chars[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !self.chars[i - 1].is_whitespace() {
            i -= 1;
        }
        i
    }

    /// `Up`: move the cursor to the previous line (preserving the column), or
    /// recall the previous history entry when already on the first line.
    pub fn up(&mut self) {
        let (row, col) = self.cursor_row_col();
        if row == 0 {
            self.history_prev();
        } else {
            self.set_cursor_row_col(row - 1, col);
        }
    }

    /// `Down`: move the cursor to the next line (preserving the column), or
    /// recall the next history entry when already on the last line.
    pub fn down(&mut self) {
        let (row, col) = self.cursor_row_col();
        if row + 1 >= self.lines().len() {
            self.history_next();
        } else {
            self.set_cursor_row_col(row + 1, col);
        }
    }

    /// Take the current text, pushing it to history (when non-blank) and
    /// resetting the buffer. Returns the raw (untrimmed) text.
    pub fn take(&mut self) -> String {
        let text = self.text();
        if !text.trim().is_empty() {
            self.history.push(text.clone());
        }
        self.chars.clear();
        self.cursor = 0;
        self.browse = None;
        self.stash.clear();
        text
    }

    /// Recall the previous (older) history entry.
    fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let next = match self.browse {
            None => {
                self.stash = self.chars.clone();
                self.history.len() - 1
            }
            Some(0) => return,
            Some(i) => i - 1,
        };
        self.load_history(next);
    }

    /// Recall the next (newer) history entry, or restore the live buffer.
    fn history_next(&mut self) {
        match self.browse {
            None => {}
            Some(i) if i + 1 < self.history.len() => self.load_history(i + 1),
            Some(_) => {
                self.chars = std::mem::take(&mut self.stash);
                self.cursor = self.chars.len();
                self.browse = None;
            }
        }
    }

    fn load_history(&mut self, i: usize) {
        self.chars = self.history[i].chars().collect();
        self.cursor = self.chars.len();
        self.browse = Some(i);
    }

    /// Editing a browsed history entry adopts it as the live buffer.
    fn detach_history(&mut self) {
        self.browse = None;
        self.stash.clear();
    }

    fn set_cursor_row_col(&mut self, row: usize, col: usize) {
        let mut r = 0;
        let mut idx = 0;
        let mut line_start = 0;
        while idx < self.chars.len() && r < row {
            if self.chars[idx] == '\n' {
                r += 1;
                line_start = idx + 1;
            }
            idx += 1;
        }
        // Clamp the column to the target line's length.
        let mut end = line_start;
        while end < self.chars.len() && self.chars[end] != '\n' {
            end += 1;
        }
        self.cursor = (line_start + col).min(end);
    }
}

/// The composer's [`Renderable`] view: codex's borderless shape (a
/// background-styled block, no box glyphs) with a bold `›` prompt rendered
/// into a 2-column left gutter on the textarea's first visible row, scrolled
/// so the cursor row stays visible. The cursor position is reported in
/// DISPLAY columns (CJK/wide chars are 2 columns), clamped inside the inset
/// textarea.
///
/// Extracted from the former `RataApp::render_viewport`'s composer zone (plan
/// Phase 2): the view renders into `(Rect, &mut Buffer)`; only the terminal
/// draw boundary ([`crate::chat_widget::ChatWidget::render_frame`]) adapts a
/// [`crate::terminal::Frame`].
pub struct ComposerView<'a> {
    composer: &'a Composer,
    /// Session accent tint (`/color`): there is no border to tint since the
    /// borderless-composer rework (Task 3) — it now tints the `›` prompt.
    accent: Option<ratatui::style::Color>,
}

impl<'a> ComposerView<'a> {
    /// A view over `composer`.
    #[must_use]
    pub fn new(composer: &'a Composer) -> Self {
        Self {
            composer,
            accent: None,
        }
    }

    /// Tint the `›` gutter prompt with the session accent color (`/color`).
    /// Previously tinted the box border; the border is gone (Task 3).
    #[must_use]
    pub fn with_accent(mut self, accent: Option<ratatui::style::Color>) -> Self {
        self.accent = accent;
        self
    }

    /// The first content row shown when only `visible_rows` rows fit: scrolls
    /// just enough to keep the cursor row inside the window.
    fn first_visible_row(&self, visible_rows: usize) -> usize {
        let (crow, _) = self.composer.cursor_row_col();
        crow.saturating_sub(visible_rows.saturating_sub(1))
    }
}

/// Codex-shape composer inner rect: 1-row top/bottom padding, 2-column left
/// gutter (the `›` prompt), 1-column right margin (codex `layout_areas`
/// insets `tlbr(1, LIVE_PREFIX_COLS, 1, 1)` with `LIVE_PREFIX_COLS` = 2).
fn inner_rect(area: Rect) -> Rect {
    Rect {
        x: area.x.saturating_add(2),
        y: area.y.saturating_add(1),
        width: area.width.saturating_sub(3),
        height: area.height.saturating_sub(2),
    }
}

impl Renderable for ComposerView<'_> {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        // Codex chat_composer render: a borderless background block…
        let style = crate::style::user_message_style();
        ratatui::widgets::Block::default().style(style).render(area, buf);
        let inner = inner_rect(area);
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        // …with a bold `›` in the 2-column gutter on the first textarea row.
        // The `/color` accent used to tint the (now removed) border; it tints
        // the prompt instead.
        let prompt_style = match self.accent {
            Some(accent) => ratatui::style::Style::default()
                .fg(accent)
                .add_modifier(ratatui::style::Modifier::BOLD),
            None => ratatui::style::Style::default().add_modifier(ratatui::style::Modifier::BOLD),
        };
        buf.set_span(area.x, inner.y, &Span::styled("›", prompt_style), 2);
        let body: Vec<Line> = self
            .composer
            .lines()
            .iter()
            .map(|l| Line::from(l.clone()))
            .collect();
        let first_row = self.first_visible_row(usize::from(inner.height));
        Paragraph::new(body)
            .style(style)
            .scroll((u16::try_from(first_row).unwrap_or(0), 0))
            .render(inner, buf);
    }

    /// Content lines clamped to [`MAX_VISIBLE_LINES`] plus the 2 padding rows
    /// (same arithmetic as the old border rows — pane height is unchanged).
    fn desired_height(&self, _width: u16) -> u16 {
        let content = self.composer.lines().len().clamp(1, MAX_VISIBLE_LINES);
        u16::try_from(content + 2).unwrap_or(u16::MAX)
    }

    fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
        let inner = inner_rect(area);
        // A degenerate inner rect (terminal too small for even one content
        // row) has nowhere the cursor could sit INSIDE the composer: claim
        // none so the terminal hides it instead of parking it outside the
        // box (plan Phase 13 graceful-clipping fix).
        if inner.width == 0 || inner.height == 0 {
            return None;
        }
        let (crow, ccol) = self.composer.cursor_row_col();
        let first_row = self.first_visible_row(usize::from(inner.height));
        let cursor_y = inner.y + u16::try_from(crow - first_row).unwrap_or(0);
        // Cursor X in DISPLAY columns (CJK/wide chars are 2 cols), not chars.
        let before_cursor: String = self
            .composer
            .lines()
            .get(crow)
            .map(|l| l.chars().take(ccol).collect())
            .unwrap_or_default();
        let disp_w = unicode_width::UnicodeWidthStr::width(before_cursor.as_str());
        let cursor_x = inner.x + u16::try_from(disp_w).unwrap_or(0);
        Some((
            cursor_x.min(inner.x + inner.width.saturating_sub(1)),
            cursor_y.min(inner.y + inner.height.saturating_sub(1)),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(s: &str) -> Composer {
        let mut c = Composer::default();
        for ch in s.chars() {
            c.insert(ch);
        }
        c
    }

    #[test]
    fn insert_and_backspace_track_cursor() {
        let mut c = typed("abc");
        assert_eq!(c.text(), "abc");
        assert_eq!(c.cursor_row_col(), (0, 3));
        c.backspace();
        assert_eq!(c.text(), "ab");
        c.move_left();
        c.insert('X');
        assert_eq!(c.text(), "aXb");
    }

    #[test]
    fn left_right_move_and_delete_at_cursor() {
        let mut c = typed("abc");
        c.move_left();
        c.move_left(); // between a|bc
        c.delete(); // removes 'b'
        assert_eq!(c.text(), "ac");
        c.move_left(); // |ac
        c.move_left(); // clamps at 0
        assert_eq!(c.cursor_row_col(), (0, 0));
    }

    #[test]
    fn newline_makes_multiline_and_rowcol_tracks() {
        let mut c = typed("ab");
        c.insert_newline();
        c.insert('c');
        assert_eq!(c.text(), "ab\nc");
        assert_eq!(c.lines(), vec!["ab".to_string(), "c".to_string()]);
        assert_eq!(c.cursor_row_col(), (1, 1));
    }

    #[test]
    fn up_down_move_between_lines_preserving_column() {
        let mut c = typed("abcd");
        c.insert_newline();
        c.insert('e'); // "abcd\ne", cursor at (1,1)
        c.up(); // to row 0, col 1
        assert_eq!(c.cursor_row_col(), (0, 1));
        c.down(); // back to row 1, col clamped to 1
        assert_eq!(c.cursor_row_col(), (1, 1));
    }

    #[test]
    fn up_on_first_line_recalls_history() {
        let mut c = Composer::default();
        for ch in "first".chars() {
            c.insert(ch);
        }
        assert_eq!(c.take(), "first");
        for ch in "second".chars() {
            c.insert(ch);
        }
        assert_eq!(c.take(), "second");
        // Live buffer is empty; Up walks back newest → oldest.
        c.up();
        assert_eq!(c.text(), "second");
        c.up();
        assert_eq!(c.text(), "first");
        c.up(); // clamps at oldest
        assert_eq!(c.text(), "first");
    }

    #[test]
    fn down_returns_to_live_stash() {
        let mut c = Composer::default();
        for ch in "old".chars() {
            c.insert(ch);
        }
        c.take();
        for ch in "draft".chars() {
            c.insert(ch);
        }
        c.up(); // browse "old", stashing "draft"
        assert_eq!(c.text(), "old");
        c.down(); // restore the live draft
        assert_eq!(c.text(), "draft");
    }

    #[test]
    fn editing_a_recalled_entry_detaches_history() {
        let mut c = Composer::default();
        for ch in "abc".chars() {
            c.insert(ch);
        }
        c.take();
        c.up(); // recall "abc"
        c.insert('!'); // edit → becomes live
        assert_eq!(c.text(), "abc!");
        c.down(); // no live stash to restore; stays put
        assert_eq!(c.text(), "abc!");
    }

    #[test]
    fn take_pushes_history_and_clears() {
        let mut c = typed("hi");
        assert_eq!(c.take(), "hi");
        assert!(c.text().is_empty());
        assert_eq!(c.cursor_row_col(), (0, 0));
        // Blank input is not pushed to history.
        let mut blank = typed("   ");
        blank.take();
        blank.up();
        assert_eq!(blank.text(), "");
    }

    #[test]
    fn home_and_end_move_within_current_line() {
        let mut c = typed("ab");
        c.insert_newline();
        for ch in "cdef".chars() {
            c.insert(ch);
        }
        // Cursor is at (1,4); Home goes to line start, End to line end.
        c.home();
        assert_eq!(c.cursor_row_col(), (1, 0));
        c.end();
        assert_eq!(c.cursor_row_col(), (1, 4));
    }

    #[test]
    fn word_motion_skips_words_and_whitespace() {
        let mut c = typed("foo bar baz");
        c.move_word_left(); // to start of "baz"
        assert_eq!(c.cursor_row_col(), (0, 8));
        c.move_word_left(); // to start of "bar"
        assert_eq!(c.cursor_row_col(), (0, 4));
        c.move_word_right(); // back to end of "bar"
        assert_eq!(c.cursor_row_col(), (0, 7));
    }

    #[test]
    fn delete_word_removes_previous_word() {
        let mut c = typed("foo bar");
        c.delete_word();
        assert_eq!(c.text(), "foo ");
        c.delete_word();
        assert_eq!(c.text(), "");
    }

    #[test]
    fn kill_to_line_start_clears_line_prefix() {
        let mut c = typed("keep");
        c.insert_newline();
        for ch in "drop this".chars() {
            c.insert(ch);
        }
        c.kill_to_line_start();
        assert_eq!(c.text(), "keep\n");
    }

    #[test]
    fn at_fragment_detects_at_token_before_cursor() {
        let mut c = typed("see @src");
        assert_eq!(c.at_fragment(), Some((4, "src".to_string())));
        // No @ token → None.
        let plain = typed("hello");
        assert_eq!(plain.at_fragment(), None);
        // A bare @ yields an empty fragment.
        let bare = typed("@");
        assert_eq!(bare.at_fragment(), Some((0, String::new())));
    }

    #[test]
    fn complete_at_replaces_the_at_token_fragment() {
        let mut c = typed("open @sr");
        let (at, _) = c.at_fragment().unwrap();
        c.complete_at(at, "src/main.rs");
        assert_eq!(c.text(), "open @src/main.rs");
        assert_eq!(c.cursor_row_col(), (0, 17));
    }

    #[test]
    fn kill_to_line_end_and_delete_line() {
        let mut c = typed("keep");
        c.insert_newline();
        for ch in "drop rest".chars() {
            c.insert(ch);
        }
        // Cursor at end of line 2; move to after "drop " then kill to end.
        c.home();
        c.move_right();
        c.move_right();
        c.move_right();
        c.move_right();
        c.move_right(); // after "drop "
        c.kill_to_line_end();
        assert_eq!(c.text(), "keep\ndrop ");
        // dd removes the whole current line + its structure.
        c.delete_line();
        assert_eq!(c.text(), "keep\n");
    }

    #[test]
    fn cursor_line_up_down_stay_in_text_without_history() {
        let mut c = Composer::default();
        for ch in "old".chars() {
            c.insert(ch);
        }
        c.take(); // history has "old"
        for ch in "a".chars() {
            c.insert(ch);
        }
        c.insert_newline();
        c.insert('b'); // "a\nb", cursor (1,1)
        c.cursor_line_up();
        assert_eq!(c.cursor_row_col(), (0, 1));
        // At the top line, cursor_line_up does NOT pull history (unlike `up`).
        c.cursor_line_up();
        assert_eq!(c.text(), "a\nb");
        assert_eq!(c.cursor_row_col(), (0, 1));
    }

    #[test]
    fn next_word_start_and_word_end_vim_motions() {
        let mut c = typed("foo bar baz");
        c.home();
        c.next_word_start(); // → start of "bar"
        assert_eq!(c.cursor_row_col(), (0, 4));
        c.word_end(); // → end of "bar"
        assert_eq!(c.cursor_row_col(), (0, 6));
        c.next_word_start(); // → start of "baz"
        assert_eq!(c.cursor_row_col(), (0, 8));
    }

    #[test]
    fn selection_range_delete_and_paste() {
        let mut c = typed("hello world");
        c.home();
        c.start_selection();
        for _ in 0..5 {
            c.move_right();
        } // select "hello" (anchor 0, cursor 5)
        assert_eq!(c.selected_text().as_deref(), Some("hello ")); // inclusive of cursor cell
        let removed = c.delete_selection().unwrap();
        assert_eq!(removed, "hello ");
        assert_eq!(c.text(), "world");
        assert!(!c.has_selection());
        // Paste the removed text back at the cursor.
        c.home();
        c.insert_str(&removed);
        assert_eq!(c.text(), "hello world");
    }

    // ===== ComposerView (Renderable contract, plan Phase 2) =====

    use ratatui::layout::Position;

    fn buffer_row(buf: &Buffer, y: u16) -> String {
        (buf.area.left()..buf.area.right())
            .map(|x| {
                buf.cell(Position::new(x, y))
                    .map_or(" ", ratatui::buffer::Cell::symbol)
            })
            .collect()
    }

    #[test]
    fn composer_renders_codex_shape_gutter_prompt_no_borders() {
        let c = typed("hi");
        let area = Rect::new(0, 0, 40, 3);
        let mut buf = Buffer::empty(area);
        ComposerView::new(&c).render(area, &mut buf);
        let row = |y: u16| -> String {
            (0..40)
                .map(|x| {
                    buf.cell(ratatui::layout::Position::new(x, y))
                        .unwrap()
                        .symbol()
                        .to_string()
                })
                .collect()
        };
        // Row 0 and row 2 are padding (no border glyphs anywhere).
        assert!(!row(0).contains('┌') && !row(2).contains('└'));
        // Row 1: gutter prompt + text at column 2.
        assert!(row(1).starts_with("› hi"), "row1: {:?}", row(1));
    }

    #[test]
    fn composer_cursor_sits_in_the_inset_textarea() {
        let c = typed("ab");
        let area = Rect::new(0, 0, 40, 3);
        let pos = ComposerView::new(&c).cursor_pos(area);
        // inner.x = 2 (gutter), + display width 2 = 4; row = 1 (below top padding).
        assert_eq!(pos, Some((4, 1)));
    }

    #[test]
    fn view_renders_codex_shape_with_gutter_prompt_pinned_to_the_top_row() {
        let mut c = typed("first");
        c.insert_newline();
        c.insert_str("second");
        let view = ComposerView::new(&c);
        let area = Rect::new(0, 0, 12, 4);
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf);
        // Row 0 and row 3 are padding: no border glyphs anywhere.
        assert!(!buffer_row(&buf, 0).contains('┌'));
        assert!(!buffer_row(&buf, 3).contains('└'));
        // The `›` gutter prompt always sits on the top visible textarea row;
        // wrapped/continuation lines no longer carry a "  " prefix — they
        // start at the same `inner.x` gutter column as the prompted line.
        assert!(buffer_row(&buf, 1).starts_with("› first"));
        assert!(buffer_row(&buf, 2).starts_with("  second"));
    }

    #[test]
    fn view_desired_height_is_content_plus_padding_clamped_to_cap() {
        let mut c = typed("one");
        assert_eq!(
            ComposerView::new(&c).desired_height(80),
            3,
            "1 line + top/bottom padding"
        );
        for _ in 0..9 {
            c.insert_newline();
        }
        // 10 content lines clamp at MAX_VISIBLE_LINES (6): 6 + 2 = 8.
        assert_eq!(ComposerView::new(&c).desired_height(80), 8);
    }

    #[test]
    fn view_cursor_pos_uses_display_columns_for_cjk() {
        let c = typed("你好");
        let view = ComposerView::new(&c);
        // x = gutter inner.x(2) + two wide chars × 2 columns = 6; y = row 1.
        assert_eq!(view.cursor_pos(Rect::new(0, 0, 20, 3)), Some((6, 1)));
        // An offset area shifts the reported cursor with it.
        assert_eq!(view.cursor_pos(Rect::new(0, 5, 20, 3)), Some((6, 6)));
    }

    #[test]
    fn view_scrolls_to_keep_cursor_row_visible_and_clamps_cursor_inside() {
        let mut c = typed("l0");
        for i in 1..8 {
            c.insert_newline();
            c.insert_str(&format!("l{i}"));
        }
        let view = ComposerView::new(&c);
        // 3 visible content rows for 8 lines with the cursor on the last one:
        // the window scrolls so l7 is the bottom visible row.
        let area = Rect::new(0, 0, 10, 5);
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf);
        // The `›` gutter prompt is pinned to the top visible row (l5 here),
        // regardless of which logical line scrolled into view.
        assert!(buffer_row(&buf, 1).starts_with("› l5"));
        assert!(buffer_row(&buf, 3).starts_with("  l7"));
        let (x, y) = view.cursor_pos(area).expect("cursor");
        assert_eq!((x, y), (4, 3), "cursor on the bottom visible row");
        assert!(y < area.bottom() - 1, "cursor stays inside the padding");
    }
}

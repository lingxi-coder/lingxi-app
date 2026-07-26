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

use std::ops::Range;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Span;
use ratatui::widgets::Widget;

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
    /// The inner width the view last wrapped at (0 = never rendered). Written
    /// by [`ComposerView`] during render so `Up`/`Down` can move across
    /// VISUAL (soft-wrapped) rows — codex's textarea wrap-cache seam.
    last_wrap_width: std::cell::Cell<u16>,
    /// Sticky target display column for consecutive `Up`/`Down` moves (codex
    /// `preferred_col`): set on the first vertical move, kept while moving
    /// through shorter lines, cleared by any other edit or motion.
    preferred_col: Option<usize>,
    /// Lazily computed visual rows for `(width, revision)` — recomputed by
    /// [`Self::rows_for`] only when the width or the text changes, so the
    /// per-frame render/height/cursor passes and Up/Down share one layout
    /// instead of rescanning the buffer each call (codex `WrapCache`).
    wrap_cache: std::cell::RefCell<Option<WrapCache>>,
    /// Bumped on every text mutation; part of the wrap-cache key.
    revision: u64,
}

/// Cached [`wrap_layout`] output plus the key it was computed for.
#[derive(Debug)]
struct WrapCache {
    width: u16,
    revision: u64,
    rows: Vec<Range<usize>>,
}

/// One display column per char, except wide (CJK/emoji) chars.
fn char_width(c: char) -> usize {
    unicode_width::UnicodeWidthChar::width(c).unwrap_or(0)
}

/// Word-wrap `chars` at `width` display columns into visual rows of char
/// ranges — the single source of truth shared by rendering, cursor
/// positioning, scrolling and `Up`/`Down` movement (codex `wrap_ranges`).
///
/// Greedy first-fit: whole words move to the next row when they don't fit;
/// words wider than a full row break at display-column boundaries; trailing
/// spaces stay on the row they follow (and may overflow — rendering clips).
///
/// Each returned range carries a +1 sentinel slot past its drawable content
/// (`chars[start..end - 1]`): the terminating `\n`, the first char of the
/// next row (soft wrap), or one-past-the-end on the final row. The sentinel
/// makes ranges overlap so a cursor ON a soft boundary belongs to the LATER
/// row (`row_index` picks the last row whose start ≤ cursor), exactly like
/// codex's `wrapped_line_index_by_start`.
fn wrap_layout(chars: &[char], width: usize) -> Vec<Range<usize>> {
    let mut rows = Vec::new();
    let mut line_start = 0usize;
    loop {
        let line_end = chars[line_start..]
            .iter()
            .position(|&c| c == '\n')
            .map_or(chars.len(), |i| line_start + i);
        wrap_logical_line(chars, line_start, line_end, width, &mut rows);
        if line_end >= chars.len() {
            break;
        }
        line_start = line_end + 1;
    }
    rows
}

/// Wrap one logical line `[start, end)` (no `\n` inside) into `rows`.
fn wrap_logical_line(
    chars: &[char],
    start: usize,
    end: usize,
    width: usize,
    rows: &mut Vec<Range<usize>>,
) {
    if width == 0 {
        rows.push(start..end + 1);
        return;
    }
    let mut row_start = start;
    let mut col = 0usize;
    let mut i = start;
    while i < end {
        if chars[i] == ' ' {
            // Trailing spaces stay on the current row (may overflow the
            // width; rendering clips them — textwrap/codex behavior).
            col += 1;
            i += 1;
            continue;
        }
        // Measure the word [i, j).
        let mut j = i;
        let mut w = 0usize;
        while j < end && chars[j] != ' ' {
            w += char_width(chars[j]);
            j += 1;
        }
        if col > 0 && col + w > width {
            // The whole word moves to a fresh row (+1 sentinel overlap).
            rows.push(row_start..i + 1);
            row_start = i;
            col = 0;
        }
        if w > width {
            // A word wider than a full row breaks at column boundaries.
            while i < j {
                let cw = char_width(chars[i]);
                if col > 0 && col + cw > width {
                    rows.push(row_start..i + 1);
                    row_start = i;
                    col = 0;
                }
                col += cw;
                i += 1;
            }
        } else {
            col += w;
            i = j;
        }
    }
    rows.push(row_start..end + 1);
}

/// The visual row a cursor char-index belongs to: the LAST row whose start is
/// ≤ `pos` (sentinel overlap assigns soft-boundary positions to the later
/// row — codex `wrapped_line_index_by_start`).
fn row_index(rows: &[Range<usize>], pos: usize) -> usize {
    rows.partition_point(|r| r.start <= pos).saturating_sub(1)
}

impl Composer {
    /// The current buffer text (may contain `\n`).
    #[must_use]
    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    /// Whether the buffer holds no characters at all.
    ///
    /// O(1) and allocation-free, unlike [`Self::text`], which collects the
    /// `Vec<char>` into a fresh `String`. The key path asks this on EVERY
    /// keystroke (the ←-on-empty gesture needs to know emptiness before and
    /// after each key), so it must not allocate.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
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

    /// If the whitespace-delimited token ending at the cursor is an emoji
    /// shortcode (`:hea` / `:heart:`), return its start and filter fragment.
    /// Tokens containing a second non-terminal `:` are not shortcode
    /// candidates (for example URLs and `key:value` text).
    #[must_use]
    pub fn emoji_fragment(&self) -> Option<(usize, String)> {
        let mut start = self.cursor;
        while start > 0 && !self.chars[start - 1].is_whitespace() {
            start -= 1;
        }
        if start >= self.cursor || self.chars[start] != ':' {
            return None;
        }
        let fragment: String = self.chars[start + 1..self.cursor].iter().collect();
        let core = fragment.strip_suffix(':').unwrap_or(&fragment);
        if core.is_empty()
            || core
                .chars()
                .any(|ch| !(ch.is_ascii_alphanumeric() || matches!(ch, '_' | '+' | '-')))
        {
            return None;
        }
        Some((start, fragment))
    }

    /// Replace the complete shortcode token (including an optional trailing
    /// `:`) with the selected emoji glyph.
    pub fn complete_emoji(&mut self, start: usize, insert: &str) {
        self.detach_history();
        let end = self.cursor.min(self.chars.len());
        if start <= end {
            self.chars.splice(start..end, insert.chars());
            self.cursor = start + insert.chars().count();
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
        self.preferred_col = None;
    }

    /// Move the cursor one char right.
    pub fn move_right(&mut self) {
        if self.cursor < self.chars.len() {
            self.cursor += 1;
        }
        self.preferred_col = None;
    }

    /// Move the cursor to the start of the current line.
    pub fn home(&mut self) {
        self.cursor = self.line_start(self.cursor);
        self.preferred_col = None;
    }

    /// Move the cursor to the end of the current line.
    pub fn end(&mut self) {
        self.cursor = self.line_end(self.cursor);
        self.preferred_col = None;
    }

    /// Move the cursor to the start of the previous word.
    pub fn move_word_left(&mut self) {
        self.cursor = self.word_left_from(self.cursor);
        self.preferred_col = None;
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
        self.preferred_col = None;
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
        self.preferred_col = None;
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
        self.preferred_col = None;
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

    /// The last `n` chars before the cursor (paste-burst retro-capture
    /// window) — bounded so the hot input path never copies the whole
    /// buffer prefix.
    #[must_use]
    pub fn chars_before_cursor(&self, n: usize) -> String {
        let start = self.cursor.saturating_sub(n);
        self.chars[start..self.cursor].iter().collect()
    }

    /// Remove the `n` chars immediately before the cursor (paste-burst
    /// retro-capture: they were typed, then reclassified as pasted text and
    /// moved into the burst buffer).
    pub fn remove_chars_before_cursor(&mut self, n: usize) {
        self.detach_history();
        let start = self.cursor.saturating_sub(n);
        self.chars.drain(start..self.cursor);
        self.cursor = start;
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

    /// The visual rows wrapped at `width`, through the `(width, revision)`
    /// cache — recomputed only when the width or the text changed.
    fn rows_for(&self, width: u16) -> std::cell::Ref<'_, Vec<Range<usize>>> {
        {
            let mut cache = self.wrap_cache.borrow_mut();
            let stale = cache
                .as_ref()
                .is_none_or(|c| c.width != width || c.revision != self.revision);
            if stale {
                *cache = Some(WrapCache {
                    width,
                    revision: self.revision,
                    rows: wrap_layout(&self.chars, usize::from(width)),
                });
            }
        }
        std::cell::Ref::map(self.wrap_cache.borrow(), |c| &c.as_ref().unwrap().rows)
    }

    /// The current visual-row layout at the width the view last rendered at,
    /// or `None` before the first render (logical-line fallback).
    fn visual_rows(&self) -> Option<Vec<Range<usize>>> {
        let width = self.last_wrap_width.get();
        (width > 0).then(|| self.rows_for(width).clone())
    }

    /// The cursor's display column within visual row `row` (CJK-aware).
    fn display_col_in(&self, row: &Range<usize>) -> usize {
        self.chars[row.start..self.cursor]
            .iter()
            .map(|&c| char_width(c))
            .sum()
    }

    /// Place the cursor at display column `target` on the visual row `row`
    /// (clamped to the row's drawable content — codex
    /// `move_to_display_col_on_line`).
    fn set_display_col_in(&mut self, row: &Range<usize>, target: usize) {
        let content_end = (row.end - 1).min(self.chars.len());
        let mut col = 0usize;
        for i in row.start..content_end {
            let cw = char_width(self.chars[i]);
            if col + cw > target {
                self.cursor = i;
                return;
            }
            col += cw;
        }
        // Landing past the row's content: on a SOFT-wrapped row the boundary
        // index is the NEXT row's start (sentinel overlap), so parking there
        // would bounce `row_index` forward again and Up/Down could never
        // leave a full-width row — stop on the row's last char instead. Hard
        // (newline-terminated) and final rows keep the true end-of-content.
        let soft = content_end < self.chars.len() && self.chars[content_end] != '\n';
        self.cursor = if soft {
            content_end.saturating_sub(1).max(row.start)
        } else {
            content_end
        };
    }

    /// `Up`: move the cursor to the previous VISUAL row preserving the display
    /// column (sticky across shorter lines), or recall the previous history
    /// entry when already on the first row. Falls back to logical-line
    /// movement before the first render (codex `move_cursor_up`).
    pub fn up(&mut self) {
        if let Some(rows) = self.visual_rows() {
            let idx = row_index(&rows, self.cursor);
            if idx == 0 {
                self.history_prev();
                return;
            }
            let col = self
                .preferred_col
                .unwrap_or_else(|| self.display_col_in(&rows[idx]));
            self.preferred_col = Some(col);
            self.set_display_col_in(&rows[idx - 1], col);
            return;
        }
        let (row, col) = self.cursor_row_col();
        if row == 0 {
            self.history_prev();
        } else {
            self.set_cursor_row_col(row - 1, col);
        }
    }

    /// `Down`: move the cursor to the next VISUAL row preserving the display
    /// column (sticky across shorter lines), or recall the next history entry
    /// when already on the last row. Falls back to logical-line movement
    /// before the first render (codex `move_cursor_down`).
    pub fn down(&mut self) {
        if let Some(rows) = self.visual_rows() {
            let idx = row_index(&rows, self.cursor);
            if idx + 1 >= rows.len() {
                self.history_next();
                return;
            }
            let col = self
                .preferred_col
                .unwrap_or_else(|| self.display_col_in(&rows[idx]));
            self.preferred_col = Some(col);
            self.set_display_col_in(&rows[idx + 1], col);
            return;
        }
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
        self.preferred_col = None;
        self.revision = self.revision.wrapping_add(1);
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
                self.preferred_col = None;
                self.revision = self.revision.wrapping_add(1);
            }
        }
    }

    fn load_history(&mut self, i: usize) {
        self.chars = self.history[i].chars().collect();
        self.cursor = self.chars.len();
        self.browse = Some(i);
        self.preferred_col = None;
        self.revision = self.revision.wrapping_add(1);
    }

    /// Editing a browsed history entry adopts it as the live buffer. Every
    /// edit funnels through here, so it also drops the sticky `Up`/`Down`
    /// column.
    fn detach_history(&mut self) {
        self.browse = None;
        self.stash.clear();
        self.preferred_col = None;
        self.revision = self.revision.wrapping_add(1);
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
/// When [`Self::attached_images`] is non-empty, dim `📎 label` indicator rows
/// are drawn above the prompt — claude-code input attachment parity. The
/// composer's height grows by one row per attached image.
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
    /// Attached image labels shown as dim indicators above the text input.
    /// Empty when no images are attached (the default). claude-code parity.
    attached_images: &'a [String],
}

impl<'a> ComposerView<'a> {
    /// A view over `composer`.
    #[must_use]
    pub fn new(composer: &'a Composer) -> Self {
        Self {
            composer,
            accent: None,
            attached_images: &[],
        }
    }

    /// Tint the `›` gutter prompt with the session accent color (`/color`).
    /// Previously tinted the box border; the border is gone (Task 3).
    #[must_use]
    pub fn with_accent(mut self, accent: Option<ratatui::style::Color>) -> Self {
        self.accent = accent;
        self
    }

    /// Attach image labels shown as dim lines above the text input — one per
    /// image, each prefixed with `📎` and the label. Empty clears the row.
    #[must_use]
    pub fn with_attached_images(mut self, images: &'a [String]) -> Self {
        self.attached_images = images;
        self
    }

    /// The first visible visual row when only `visible_rows` rows fit: scrolls
    /// just enough to keep the cursor's visual row inside the window.
    fn first_visible_row(cursor_row: usize, visible_rows: usize) -> usize {
        cursor_row.saturating_sub(visible_rows.saturating_sub(1))
    }

    /// Render attachment indicator lines above the text input. Each line is a
    /// dim `📎 label`. The attachment rows consume the TOP of the area, and the
    /// text prompt + gutter shift down by the row count. Returns the number of
    /// rows consumed.
    fn render_attachments(&self, area: Rect, buf: &mut Buffer) -> u16 {
        if self.attached_images.is_empty() {
            return 0;
        }
        let dim = ratatui::style::Style::default().fg(ratatui::style::Color::Rgb(0x88, 0x88, 0x88));
        let style = crate::style::user_message_style();
        for (i, label) in self.attached_images.iter().enumerate() {
            let y = area.y + u16::try_from(i).unwrap_or(0);
            // Background-fill the row with user-message style so it sits inside
            // the composer block visually.
            ratatui::widgets::Block::default()
                .style(style)
                .render(Rect::new(area.x, y, area.width, 1), buf);
            let line = format!("📎 {}", label);
            let truncated: String = line
                .chars()
                .take(usize::from(area.width.saturating_sub(4)))
                .collect();
            let span = Span::styled(truncated, dim);
            buf.set_span(area.x + 2, y, &span, area.width.saturating_sub(3));
        }
        u16::try_from(self.attached_images.len()).unwrap_or(0)
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
        ratatui::widgets::Block::default()
            .style(style)
            .render(area, buf);
        // Attachment indicators consume the top rows; shift the text area down.
        let attach_rows = self.render_attachments(area, buf);
        // Remaining area for the text input.
        let text_area = Rect {
            x: area.x,
            y: area.y + attach_rows,
            width: area.width,
            height: area.height.saturating_sub(attach_rows),
        };
        let inner = inner_rect(text_area);
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
        // Seed the model's wrap width so `Up`/`Down` move across the same
        // visual rows the user sees (codex textarea wrap-cache seam).
        self.composer.last_wrap_width.set(inner.width);
        let rows = self.composer.rows_for(inner.width);
        let cursor_row = row_index(&rows, self.composer.cursor);
        let first_row = Self::first_visible_row(cursor_row, usize::from(inner.height));
        for (dy, row) in rows
            .iter()
            .skip(first_row)
            .take(usize::from(inner.height))
            .enumerate()
        {
            let content_end = (row.end - 1).min(self.composer.chars.len());
            let content: String = self.composer.chars[row.start..content_end].iter().collect();
            buf.set_stringn(
                inner.x,
                inner.y + u16::try_from(dy).unwrap_or(0),
                content,
                usize::from(inner.width),
                style,
            );
        }
    }

    /// Visual (soft-wrapped) lines at `width`, clamped to
    /// [`MAX_VISIBLE_LINES`], plus the 2 padding rows, plus attachment
    /// indicator rows.
    fn desired_height(&self, width: u16) -> u16 {
        // Derive the inner width from `inner_rect` so the gutter/margin inset
        // has a single source of truth (the height fed in is irrelevant).
        let inner_w = inner_rect(Rect::new(0, 0, width, 3)).width;
        let content = self
            .composer
            .rows_for(inner_w)
            .len()
            .clamp(1, MAX_VISIBLE_LINES);
        let attach_rows = u16::try_from(self.attached_images.len()).unwrap_or(0);
        u16::try_from(content + 2 + attach_rows as usize).unwrap_or(u16::MAX)
    }

    fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
        let attach_rows = self
            .attached_images
            .is_empty()
            .then_some(0u16)
            .map(|_| 0)
            .unwrap_or(u16::try_from(self.attached_images.len()).unwrap_or(0));
        let text_area = Rect {
            x: area.x,
            y: area.y + attach_rows,
            width: area.width,
            height: area.height.saturating_sub(attach_rows),
        };
        let inner = inner_rect(text_area);
        if inner.width == 0 || inner.height == 0 {
            return None;
        }
        let rows = self.composer.rows_for(inner.width);
        let cursor_row = row_index(&rows, self.composer.cursor);
        let disp_col = self.composer.display_col_in(&rows[cursor_row]);
        let first_row = Self::first_visible_row(cursor_row, usize::from(inner.height));
        let cursor_y = inner.y + u16::try_from(cursor_row - first_row).unwrap_or(0);
        let cursor_x = inner.x + u16::try_from(disp_col).unwrap_or(0);
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
        let c = typed("see @src");
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

    fn labels(s: &[&str]) -> Vec<String> {
        s.iter().map(|s| (*s).to_string()).collect()
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
    #[test]
    fn view_wraps_long_line_and_reports_wrapped_height() {
        let c = typed("hello world this wraps");
        assert_eq!(
            ComposerView::new(&c).desired_height(12),
            6,
            "4 wrapped lines + 2 padding"
        );
        let area = Rect::new(0, 0, 12, 4);
        let mut buf = Buffer::empty(area);
        ComposerView::new(&c).render(area, &mut buf);
        let row2 = buffer_row(&buf, 2).trim().to_string();
        assert!(
            !row2.is_empty(),
            "row2 should have wrapped text, got {:?}",
            buffer_row(&buf, 2)
        );
    }

    #[test]
    fn view_cursor_tracks_into_wrapped_line() {
        let c = typed("abcdefghijkl");
        let view = ComposerView::new(&c);
        assert_eq!(view.desired_height(8), 5, "3 wrapped + 2 padding = 5");
        let area = Rect::new(0, 0, 8, 5);
        let pos = view.cursor_pos(area).expect("cursor");
        assert_eq!((pos.0, pos.1), (4, 3), "cursor at (4,3) in wrapped line");
    }

    #[test]
    fn wrapped_multiword_line_cursor_matches_rendered_row() {
        // Word wrap puts "world" whole on the second visual row; the cursor
        // must land after it (display col 5), matching the RENDERED text —
        // not a char-level rewrap remainder.
        let c = typed("hello world");
        let view = ComposerView::new(&c);
        let area = Rect::new(0, 0, 12, 4); // inner width 9: "hello " | "world"
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf);
        assert!(
            buffer_row(&buf, 2).starts_with("  world"),
            "row2: {:?}",
            buffer_row(&buf, 2)
        );
        let (x, y) = view.cursor_pos(area).expect("cursor");
        assert_eq!((x, y), (7, 2), "cursor after 'world' on the wrapped row");
    }

    #[test]
    fn cjk_wrapped_cursor_uses_display_cols() {
        let c = typed("你好世界"); // 2 display cols each
        let view = ComposerView::new(&c);
        let area = Rect::new(0, 0, 8, 4); // inner width 5: "你好" | "世界"
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf);
        // NOTE: buffer_row reads per-CELL symbols — a wide char's trailing
        // cell reads as a space, so "世界" comes back as "世 界".
        assert!(
            buffer_row(&buf, 2).starts_with("  世 界"),
            "row2: {:?}",
            buffer_row(&buf, 2)
        );
        let (x, y) = view.cursor_pos(area).expect("cursor");
        assert_eq!((x, y), (6, 2), "cursor after 世界 at display col 4");
    }

    #[test]
    fn up_moves_within_a_wrapped_logical_line_not_into_history() {
        let mut c = typed("aaaa bbbb cccc");
        let area = Rect::new(0, 0, 12, 5); // inner width 9: "aaaa bbbb " | "cccc"
        let mut buf = Buffer::empty(area);
        ComposerView::new(&c).render(area, &mut buf); // seeds the wrap width
        c.up(); // from the 2nd visual row back into the 1st — NOT history
        assert_eq!(
            c.cursor_row_col(),
            (0, 4),
            "cursor moves to display col 4 of the first visual row"
        );
    }

    #[test]
    fn up_traverses_full_width_soft_rows_without_sticking() {
        // Regression (review #1): an unbroken word wraps into FULL-width rows
        // whose content end equals the next row's start; landing there bounced
        // the cursor back down and Up could never leave the last row.
        let mut c = typed("aaaaaaaaaaaa"); // 12 chars, inner width 4 → 3 rows
        let area = Rect::new(0, 0, 7, 5); // inner width 4
        let mut buf = Buffer::empty(area);
        ComposerView::new(&c).render(area, &mut buf);
        assert_eq!(c.cursor_row_col(), (0, 12));
        c.up(); // row 2 → row 1 (lands on its last char, col 3)
        assert_eq!(c.cursor_row_col(), (0, 7), "one visual row up");
        c.up(); // row 1 → row 0
        assert_eq!(c.cursor_row_col(), (0, 3), "two visual rows up");
        c.up(); // row 0 → history (empty) → no-op
        assert_eq!(c.cursor_row_col(), (0, 3), "first row recalls history");
    }

    #[test]
    fn word_motions_clear_the_sticky_column() {
        // Regression (review #5): move_word_right/next_word_start/word_end
        // must drop preferred_col like every other horizontal motion.
        let mut c = typed("aaaaaaaa\nbb cc\ndd");
        let area = Rect::new(0, 0, 24, 6);
        let mut buf = Buffer::empty(area);
        ComposerView::new(&c).render(area, &mut buf);
        c.up(); // (2,2) → (1,2)? cursor ends at (1,2); seeds preferred_col 2
        c.up(); // → (0,2)
        assert_eq!(c.cursor_row_col(), (0, 2));
        c.up(); // history (empty) no-op; preferred_col still armed
        c.move_word_right(); // horizontal word motion must clear stickiness
        let here = c.cursor_row_col().1;
        c.down();
        assert_eq!(
            c.cursor_row_col(),
            (1, here.min(5)),
            "Down uses the CURRENT column, not the stale sticky one"
        );
    }

    #[test]
    fn up_down_keep_preferred_display_col_across_short_lines() {
        let mut c = typed("abcdef\nab\nabcdef");
        let area = Rect::new(0, 0, 24, 6); // wide: no soft wrap
        let mut buf = Buffer::empty(area);
        ComposerView::new(&c).render(area, &mut buf);
        // Cursor at end (2,6): Up clamps onto the short line, Up again
        // restores the ORIGINAL column on the long line (sticky column).
        c.up();
        assert_eq!(c.cursor_row_col(), (1, 2), "clamped to the short line");
        c.up();
        assert_eq!(c.cursor_row_col(), (0, 6), "sticky col restored");
        c.down();
        assert_eq!(c.cursor_row_col(), (1, 2));
        c.down();
        assert_eq!(c.cursor_row_col(), (2, 6), "sticky col restored downward");
    }

    #[test]
    fn attached_images_render_as_dim_indicators_and_shift_prompt_down() {
        let c = typed("hi");
        let imgs = labels(&["photo.png", "screenshot.jpg"]);
        let view = ComposerView::new(&c).with_attached_images(&imgs);
        let area = Rect::new(0, 0, 40, 6); // 2 attach + 2 pad + 2 text = 6
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf);
        // Row 0 & 1: attachment indicators with 📎
        let row0 = buffer_row(&buf, 0);
        assert!(
            row0.contains("📎") && row0.contains("photo.png"),
            "row0: {row0:?}"
        );
        let row1 = buffer_row(&buf, 1);
        assert!(
            row1.contains("📎") && row1.contains("screenshot.jpg"),
            "row1: {row1:?}"
        );
        // Row 2: padding, row 3: prompt + text
        let row3 = buffer_row(&buf, 3);
        assert!(row3.contains("› hi"), "row3: {row3:?}");
        // Height includes attachment rows
        assert_eq!(view.desired_height(80), 5); // 1 content + 2 pad + 2 attach
    }

    #[test]
    fn attached_images_cursor_pos_adjusts_for_attachment_offset() {
        let c = typed("ab");
        let imgs = labels(&["img.png"]);
        let view = ComposerView::new(&c).with_attached_images(&imgs);
        let area = Rect::new(0, 0, 40, 5);
        let pos = view.cursor_pos(area);
        // inner.x = 2, disp width of "ab" = 2, y = 1 (top pad) + 1 (attach row offset) = 2
        assert_eq!(pos, Some((4, 2)));
    }
}

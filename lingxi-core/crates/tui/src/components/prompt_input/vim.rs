//! Vim mode for `PromptInput` (M7-08: normal/insert + motions).
//!
//! Pure functional state machine modelled on claude-code `src/vim/`
//! (types.ts / motions.ts / transitions.ts) and `src/utils/Cursor.ts`.
//! No iocraft, no async, no AppState coupling — vim is PromptInput-local
//! state (M7 design §2.3). Operators + visual + registers are M7-09;
//! `pending_operator`/`register`/`Visual` are scaffold here.
//!
//! Simplifications vs claude-code (M7-08 locked): logical lines (not
//! display-wrapped), char boundaries (not graphemes), no dot-repeat/undo.
//! Display-wrap-aware `gj`/`gk` (claude-code resolves `j`/`k`/`$` against
//! wrapped visual lines via `MeasuredText`) are DEFERRED — M7-08 `j`/`k`/`$`
//! are logical-line motions. Grapheme-cluster motion (claude-code's
//! `Intl.Segmenter`) is deferred to M8; motions step by `char` here.

/// Vim editing mode. Visual is a scaffold for M7-09 (never constructed here).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VimMode {
    Normal,
    Insert,
    /// Scaffold only — M7-09 implements Visual. M7-08 never enters this.
    Visual,
}

/// Operator scaffold for M7-09. M7-08 never sets a non-None pending operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operator {
    Delete,
    Change,
    Yank,
}

/// f/F/t/T find direction+stop kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindKind {
    F, // forward, land on
    BigF, // backward, land on   (key 'F')
    T, // forward, land before
    BigT, // backward, land before (key 'T')
}

/// NORMAL-mode command-parse sub-state. Mirrors claude-code CommandState,
/// M7-08 subset (no operator*, replace, indent — those are M7-09).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandState {
    Idle,
    /// Accumulating a count prefix, e.g. after "3".
    Count { digits: String },
    /// After 'f'/'F'/'t'/'T' — waiting for the target char.
    Find { kind: FindKind, count: usize },
    /// After 'g' — waiting for the second key (gg, etc.).
    G { count: usize },
}

/// PromptInput-local vim state (parent spec §2.3). Scaffold fields
/// (`pending_operator`, `register`) are reserved for M7-09; M7-08 leaves
/// them None/empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VimState {
    pub mode: VimMode,
    pub command: CommandState,
    pub pending_operator: Option<Operator>,
    pub register: Option<String>,
    /// Last f/F/t/T (kind, char) for ';'/',' — scaffold; M7-08 records it
    /// but does not implement ';'/',' (those are M7-09 polish).
    pub last_find: Option<(FindKind, char)>,
}

impl Default for VimState {
    /// claude-code createInitialVimState(): start in INSERT.
    fn default() -> Self {
        Self {
            mode: VimMode::Insert,
            command: CommandState::Idle,
            pending_operator: None,
            register: None,
            last_find: None,
        }
    }
}

/// Single-step motion keys M7-08 resolves. (W/B/E WORD-motions deferred.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    Left, Right, Down, Up,        // h l j k
    NextWord, PrevWord, EndWord,  // w b e
    LineStart, FirstNonBlank, LineEnd, // 0 ^ $
    FileStart, LastLine,          // gg  G
}

/// What a key did to the buffer/cursor. The caller (root.rs) applies it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VimEffect {
    /// Cursor moved to this byte offset; buffer text unchanged.
    Move(usize),
    /// Buffer replaced and cursor set (used by o/O which insert a newline).
    Edit { text: String, cursor: usize },
    /// No-op (unrecognized key in Normal, or motion that didn't move).
    None,
}

/// Logical-line cursor over a UTF-8 buffer. Offset is a byte index always on
/// a char boundary. Pure; every method returns a new offset.
#[derive(Debug, Clone, Copy)]
pub struct VimCursor<'a> {
    pub text: &'a str,
    pub offset: usize,
}

impl<'a> VimCursor<'a> {
    fn clamp(&self, off: usize) -> usize {
        let off = off.min(self.text.len());
        if self.text.is_char_boundary(off) {
            off
        } else {
            let mut c = off;
            while c > 0 && !self.text.is_char_boundary(c) {
                c -= 1;
            }
            c
        }
    }

    #[must_use]
    pub fn left(&self) -> Self {
        if self.offset == 0 {
            return *self;
        }
        let prev = self.text[..self.offset]
            .char_indices()
            .last()
            .map_or(0, |(i, _)| i);
        Self { text: self.text, offset: prev }
    }

    #[must_use]
    pub fn right(&self) -> Self {
        if self.offset >= self.text.len() {
            return *self;
        }
        let ch_len = self.text[self.offset..]
            .chars()
            .next()
            .map_or(0, char::len_utf8);
        Self { text: self.text, offset: self.offset + ch_len }
    }

    fn logical_line_start(&self, from: usize) -> usize {
        self.text[..from].rfind('\n').map_or(0, |i| i + 1)
    }

    fn logical_line_end(&self, from: usize) -> usize {
        self.text[from..].find('\n').map_or(self.text.len(), |i| from + i)
    }

    #[must_use]
    pub fn start_of_logical_line(&self) -> Self {
        Self { text: self.text, offset: self.logical_line_start(self.offset) }
    }

    #[must_use]
    pub fn end_of_logical_line(&self) -> Self {
        Self { text: self.text, offset: self.logical_line_end(self.offset) }
    }

    #[must_use]
    pub fn first_non_blank(&self) -> Self {
        let start = self.logical_line_start(self.offset);
        let end = self.logical_line_end(self.offset);
        let line = &self.text[start..end];
        let rel = line.find(|c: char| !c.is_whitespace()).unwrap_or(0);
        Self { text: self.text, offset: start + rel }
    }

    /// Column = byte distance from logical-line start, clamped to dest line.
    fn move_to_line(&self, target_start: usize, target_end: usize) -> Self {
        let cur_start = self.logical_line_start(self.offset);
        let col = self.offset - cur_start;
        let line_len = target_end - target_start;
        let raw = target_start + col.min(line_len);
        Self { text: self.text, offset: self.clamp(raw) }
    }

    #[must_use]
    pub fn down_logical_line(&self) -> Self {
        let end = self.logical_line_end(self.offset);
        if end >= self.text.len() {
            return *self; // last line: no-op
        }
        let next_start = end + 1; // skip the '\n'
        let next_end = self.logical_line_end(next_start);
        self.move_to_line(next_start, next_end)
    }

    #[must_use]
    pub fn up_logical_line(&self) -> Self {
        let start = self.logical_line_start(self.offset);
        if start == 0 {
            return *self; // first line: no-op
        }
        let prev_end = start - 1; // the '\n' itself
        let prev_start = self.logical_line_start(prev_end);
        self.move_to_line(prev_start, prev_end)
    }

    #[must_use]
    pub fn start_of_first_line(&self) -> Self {
        Self { text: self.text, offset: 0 }
    }

    #[must_use]
    pub fn start_of_last_line(&self) -> Self {
        let off = self.text.rfind('\n').map_or(0, |i| i + 1);
        Self { text: self.text, offset: off }
    }

    /// 1-indexed logical line, clamped (vim `Ngg` / `G`).
    #[must_use]
    pub fn go_to_line(&self, line_1indexed: usize) -> Self {
        let target = line_1indexed.saturating_sub(1);
        let mut off = 0usize;
        for (i, l) in self.text.split('\n').enumerate() {
            if i == target {
                return Self { text: self.text, offset: off };
            }
            off += l.len() + 1; // +1 for '\n'
        }
        // clamp to last line start
        self.start_of_last_line()
    }

    fn is_at_end(&self) -> bool {
        self.offset >= self.text.len()
    }
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}
fn is_space(c: char) -> bool {
    c.is_whitespace()
}
fn is_punct(c: char) -> bool {
    !is_space(c) && !is_word_char(c)
}

impl<'a> VimCursor<'a> {
    fn char_at(&self, off: usize) -> Option<char> {
        if off >= self.text.len() {
            return None;
        }
        self.text[off..].chars().next()
    }

    /// Byte offset of the char after `off` (clamped to len).
    fn next_off(&self, off: usize) -> usize {
        self.char_at(off).map_or(off, |c| off + c.len_utf8())
    }
    /// Byte offset of the char before `off` (clamped to 0).
    fn prev_off(&self, off: usize) -> usize {
        if off == 0 {
            return 0;
        }
        self.text[..off].char_indices().last().map_or(0, |(i, _)| i)
    }

    #[must_use]
    pub fn next_vim_word(&self) -> Self {
        if self.is_at_end() {
            return *self;
        }
        let mut pos = self.offset;
        let c = self.char_at(pos).unwrap();
        if is_word_char(c) {
            while pos < self.text.len() && self.char_at(pos).is_some_and(is_word_char) {
                pos = self.next_off(pos);
            }
        } else if is_punct(c) {
            while pos < self.text.len() && self.char_at(pos).is_some_and(is_punct) {
                pos = self.next_off(pos);
            }
        }
        while pos < self.text.len() && self.char_at(pos).is_some_and(is_space) {
            pos = self.next_off(pos);
        }
        Self { text: self.text, offset: pos }
    }

    #[must_use]
    pub fn prev_vim_word(&self) -> Self {
        if self.offset == 0 {
            return *self;
        }
        let mut pos = self.prev_off(self.offset);
        while pos > 0 && self.char_at(pos).is_some_and(is_space) {
            pos = self.prev_off(pos);
        }
        if pos == 0 && self.char_at(0).is_some_and(is_space) {
            return Self { text: self.text, offset: 0 };
        }
        let c = self.char_at(pos).unwrap();
        if is_word_char(c) {
            while pos > 0 {
                let p = self.prev_off(pos);
                if !self.char_at(p).is_some_and(is_word_char) {
                    break;
                }
                pos = p;
            }
        } else if is_punct(c) {
            while pos > 0 {
                let p = self.prev_off(pos);
                if !self.char_at(p).is_some_and(is_punct) {
                    break;
                }
                pos = p;
            }
        }
        Self { text: self.text, offset: pos }
    }

    #[must_use]
    pub fn end_vim_word(&self) -> Self {
        if self.is_at_end() {
            return *self;
        }
        let mut pos = self.next_off(self.offset);
        while pos < self.text.len() && self.char_at(pos).is_some_and(is_space) {
            pos = self.next_off(pos);
        }
        if pos >= self.text.len() {
            return Self { text: self.text, offset: self.text.len() };
        }
        let c = self.char_at(pos).unwrap();
        let pred: fn(char) -> bool = if is_word_char(c) { is_word_char } else { is_punct };
        loop {
            let nxt = self.next_off(pos);
            if nxt >= self.text.len() || !self.char_at(nxt).is_some_and(pred) {
                break;
            }
            pos = nxt;
        }
        Self { text: self.text, offset: pos }
    }
}

impl<'a> VimCursor<'a> {
    /// vim f/F/t/T. Returns target byte offset or None if not found.
    #[must_use]
    pub fn find_character(&self, ch: char, kind: FindKind, count: usize) -> Option<usize> {
        let forward = matches!(kind, FindKind::F | FindKind::T);
        let till = matches!(kind, FindKind::T | FindKind::BigT);
        let count = count.max(1);
        let mut found = 0usize;
        if forward {
            let mut pos = self.next_off(self.offset);
            while pos < self.text.len() {
                if self.char_at(pos) == Some(ch) {
                    found += 1;
                    if found == count {
                        return Some(if till { self.prev_off(pos).max(self.offset) } else { pos });
                    }
                }
                pos = self.next_off(pos);
            }
        } else {
            if self.offset == 0 {
                return None;
            }
            let mut pos = self.prev_off(self.offset);
            loop {
                if self.char_at(pos) == Some(ch) {
                    found += 1;
                    if found == count {
                        return Some(if till { self.next_off(pos).min(self.offset) } else { pos });
                    }
                }
                if pos == 0 {
                    break;
                }
                pos = self.prev_off(pos);
            }
        }
        None
    }
}

fn apply_single_motion(m: Motion, c: VimCursor<'_>) -> VimCursor<'_> {
    match m {
        Motion::Left => c.left(),
        Motion::Right => c.right(),
        Motion::Down => c.down_logical_line(),
        Motion::Up => c.up_logical_line(),
        Motion::NextWord => c.next_vim_word(),
        Motion::PrevWord => c.prev_vim_word(),
        Motion::EndWord => c.end_vim_word(),
        Motion::LineStart => c.start_of_logical_line(),
        Motion::FirstNonBlank => c.first_non_blank(),
        Motion::LineEnd => c.end_of_logical_line(),
        Motion::FileStart => c.start_of_first_line(),
        Motion::LastLine => c.start_of_last_line(),
    }
}

/// Apply `m` exactly `count` times, breaking when a step does not move.
#[must_use]
pub fn resolve_motion<'a>(m: Motion, cursor: VimCursor<'a>, count: usize) -> VimCursor<'a> {
    let mut result = cursor;
    for _ in 0..count.max(1) {
        let next = apply_single_motion(m, result);
        if next.offset == result.offset {
            break;
        }
        result = next;
    }
    result
}

/// Footer mode-indicator literal. Matches the well-known vim convention
/// (claude-code surfaces the mode via PromptInputModeIndicator; the literal
/// status-line text is the standard vim `-- MODE --`).
#[must_use]
pub fn mode_indicator(mode: VimMode) -> &'static str {
    match mode {
        VimMode::Normal => "-- NORMAL --",
        VimMode::Insert => "-- INSERT --",
        VimMode::Visual => "-- VISUAL --",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_state_is_insert() {
        let s = VimState::default();
        assert_eq!(s.mode, VimMode::Insert);
        assert_eq!(s.command, CommandState::Idle);
        assert!(s.pending_operator.is_none());
    }

    #[test]
    fn mode_indicator_strings() {
        assert_eq!(mode_indicator(VimMode::Normal), "-- NORMAL --");
        assert_eq!(mode_indicator(VimMode::Insert), "-- INSERT --");
        assert_eq!(mode_indicator(VimMode::Visual), "-- VISUAL --");
    }
}

#[cfg(test)]
mod cursor_tests {
    use super::*;

    fn cur(text: &str, off: usize) -> VimCursor<'_> {
        VimCursor { text, offset: off }
    }

    #[test]
    fn left_right_clamp_and_utf8() {
        // "héllo": 'é' is 2 bytes (offsets 1..3).
        assert_eq!(cur("héllo", 0).left().offset, 0);          // clamp at 0
        assert_eq!(cur("héllo", 0).right().offset, 1);
        assert_eq!(cur("héllo", 1).right().offset, 3);          // skip whole 'é'
        assert_eq!(cur("héllo", 3).left().offset, 1);
        assert_eq!(cur("hi", 2).right().offset, 2);             // clamp at end
    }

    #[test]
    fn logical_line_bounds() {
        let t = "abc\ndefg\nhi";
        // cursor in middle of line 2 (offset 6 = 'f')
        assert_eq!(cur(t, 6).start_of_logical_line().offset, 4); // 'd'
        assert_eq!(cur(t, 6).end_of_logical_line().offset, 8);   // after 'g' (the \n)
        // line 1 has no leading blanks -> first_non_blank == start
        assert_eq!(cur("  xy", 3).first_non_blank().offset, 2);  // 'x'
    }

    #[test]
    fn down_up_preserve_column_clamped() {
        let t = "abcd\nef\nghij";
        // on line0 col3 ('d'), down -> line1 but line1 len 2 -> clamp to end (col2 = after 'f')
        let c = cur(t, 3).down_logical_line();
        assert_eq!(c.offset, 7); // line1 = "ef" at 5..7, end is 7
        // from there, down -> line2 col2 = 'i' (offset 8+2=10)
        let c2 = cur(t, 7).down_logical_line();
        assert_eq!(c2.offset, 10);
        // up from line2 col2 -> line1 clamp end = 7
        assert_eq!(cur(t, 10).up_logical_line().offset, 7);
    }

    #[test]
    fn first_last_line_and_goto() {
        let t = "one\ntwo\nthree";
        assert_eq!(cur(t, 9).start_of_first_line().offset, 0);
        assert_eq!(cur(t, 0).start_of_last_line().offset, 8);  // 'three'
        assert_eq!(cur(t, 0).go_to_line(2).offset, 4);          // 'two' (1-indexed)
        assert_eq!(cur(t, 0).go_to_line(99).offset, 8);         // clamp to last
    }
}

#[cfg(test)]
mod word_tests {
    use super::*;
    fn cur(text: &str, off: usize) -> VimCursor<'_> {
        VimCursor { text, offset: off }
    }

    #[test]
    fn next_word_skips_to_next_word_start() {
        let t = "foo bar baz";
        assert_eq!(cur(t, 0).next_vim_word().offset, 4); // 'b' of bar
        assert_eq!(cur(t, 4).next_vim_word().offset, 8); // 'b' of baz
        assert_eq!(cur(t, 8).next_vim_word().offset, 11); // end (no next)
    }

    #[test]
    fn next_word_treats_punctuation_as_its_own_word() {
        let t = "foo.bar";
        // from 'f': over the word "foo" then land on '.'
        assert_eq!(cur(t, 0).next_vim_word().offset, 3); // '.'
        // from '.': over the punctuation run then land on "bar"
        assert_eq!(cur(t, 3).next_vim_word().offset, 4); // 'b'
    }

    #[test]
    fn prev_word_goes_to_word_start() {
        let t = "foo bar baz";
        assert_eq!(cur(t, 8).prev_vim_word().offset, 4); // start of 'bar'
        assert_eq!(cur(t, 5).prev_vim_word().offset, 4); // inside 'bar' -> its start
        assert_eq!(cur(t, 2).prev_vim_word().offset, 0); // inside 'foo' -> 0
    }

    #[test]
    fn end_word_lands_on_last_char_of_word() {
        let t = "foo bar";
        assert_eq!(cur(t, 0).end_vim_word().offset, 2); // 'o' (last of foo)
        assert_eq!(cur(t, 2).end_vim_word().offset, 6); // 'r' (last of bar)
    }

    #[test]
    fn word_motions_handle_punctuation_boundaries() {
        let t = "a, b";
        assert_eq!(cur(t, 0).end_vim_word().offset, 1); // ',' is end of next "word"
    }
}

#[cfg(test)]
mod find_tests {
    use super::*;
    fn cur(text: &str, off: usize) -> VimCursor<'_> {
        VimCursor { text, offset: off }
    }

    #[test]
    fn f_lands_on_char() {
        let t = "abcdabcd";
        assert_eq!(cur(t, 0).find_character('c', FindKind::F, 1), Some(2));
        assert_eq!(cur(t, 0).find_character('c', FindKind::F, 2), Some(6)); // 2nd c
        assert_eq!(cur(t, 0).find_character('z', FindKind::F, 1), None);
    }

    #[test]
    fn t_lands_before_char() {
        let t = "abcdabcd";
        assert_eq!(cur(t, 0).find_character('c', FindKind::T, 1), Some(1)); // before first c
    }

    #[test]
    fn big_f_searches_backward() {
        let t = "abcdabcd";
        assert_eq!(cur(t, 7).find_character('a', FindKind::BigF, 1), Some(4));
        assert_eq!(cur(t, 7).find_character('a', FindKind::BigF, 2), Some(0));
    }

    #[test]
    fn big_t_lands_after_char_backward() {
        let t = "abcdabcd";
        // from offset 7 ('d'), backward till 'a' (at 4) -> land just after it = 5
        assert_eq!(cur(t, 7).find_character('a', FindKind::BigT, 1), Some(5));
    }
}

#[cfg(test)]
mod resolve_tests {
    use super::*;
    fn cur(text: &str, off: usize) -> VimCursor<'_> {
        VimCursor { text, offset: off }
    }

    #[test]
    fn single_step_motions() {
        assert_eq!(resolve_motion(Motion::Right, cur("hello", 0), 1).offset, 1);
        assert_eq!(resolve_motion(Motion::Left, cur("hello", 3), 1).offset, 2);
        assert_eq!(resolve_motion(Motion::LineEnd, cur("hello", 0), 1).offset, 5);
        assert_eq!(resolve_motion(Motion::LineStart, cur("hello", 3), 1).offset, 0);
    }

    #[test]
    fn count_repeats_motion() {
        assert_eq!(resolve_motion(Motion::Right, cur("hello", 0), 3).offset, 3);
        assert_eq!(resolve_motion(Motion::NextWord, cur("a b c d", 0), 2).offset, 4); // 'c'
    }

    #[test]
    fn count_breaks_early_at_bound() {
        // Right 100 times on "hi" stops at end (offset 2), not panic.
        assert_eq!(resolve_motion(Motion::Right, cur("hi", 0), 100).offset, 2);
        // Up on first line is a no-op; count doesn't matter.
        assert_eq!(resolve_motion(Motion::Up, cur("abc", 1), 5).offset, 1);
    }

    #[test]
    fn down_crosses_logical_lines() {
        let t = "abc\ndef\nghi";
        assert_eq!(resolve_motion(Motion::Down, cur(t, 1), 1).offset, 5); // line2 col1 = 'e'
        assert_eq!(resolve_motion(Motion::Down, cur(t, 1), 2).offset, 9); // line3 col1 = 'h'
    }
}

//! Vim mode for `PromptInput` (M7-08: normal/insert + motions).
//!
//! Pure functional state machine modelled on claude-code `src/vim/`
//! (types.ts / motions.ts / transitions.ts) and `src/utils/Cursor.ts`.
//! No iocraft, no async, no `AppState` coupling — vim is PromptInput-local
//! state (M7 design §2.3). Operators + visual + registers are M7-09;
//! `pending_operator`/`register`/`Visual` are scaffold here.
//!
//! Simplifications vs claude-code (M7-08 locked): logical lines (not
//! display-wrapped), char boundaries (not graphemes), no dot-repeat/undo.
//! Display-wrap-aware `gj`/`gk` (claude-code resolves `j`/`k`/`$` against
//! wrapped visual lines via `MeasuredText`) are DEFERRED — M7-08 `j`/`k`/`$`
//! are logical-line motions. Grapheme-cluster motion (claude-code's
//! `Intl.Segmenter`) is deferred to M8; motions step by `char` here.
//!
//! ## GATE: vim subset (M7-09 — parent spec §4 R1 / hard gate §3.3)
//!
//! Core vim SHIPS and passes the operator×motion matrix. Obscure cases are
//! DEFERRED to M8 with this documented "vim parity subset" line:
//!
//! IN (M7-09): operators d/c/y × motions {w b e $ 0 ^ h l j k f-char t-char
//! G gg}, counts (3dw, 2yy, d3w); doubled ops dd/cc/yy (+counts); cw->ce;
//! x (count); p/P charwise+linewise; the unnamed yank/delete register;
//! Visual (v) + Visual-line (V) with d/c/y on the selection; c enters Insert.
//!
//! DEFERRED to M8 (vim parity subset): `.` dot-repeat; macros q/@; ex-commands `:`;
//! `/` search-as-motion; named/numbered registers (only the unnamed register
//! ships); text objects iw/aw (claude-code has textObjects.ts + operatorTextObj
//! — NOT wired here; text-object keys after an operator are a no-op, never a
//! panic); W/B/E WORD-motions; r replace; ~ toggle-case; J join; indent ops;
//! gj/gk display-wrap motions; find-repeat; bare `NG` motion; Visual block
//! (`Ctrl-v`), o (swap ends), gv (reselect).
//!
//! NOTE: claude-code's vim (src/vim/*) has NO Visual mode — v/V are implemented
//! to standard vim semantics; there is no claude-code literal to match for them.
//!
//! ## Motion-endpoint reconciliation (M7-09 / M7-08 Issue #2)
//! Operator inclusive motions extend exactly one char past the target
//! (`to = next_off(to)`). M7-08's `e` (`end_vim_word`) already lands ON the last
//! char of the word (e.g. `e` on "foo"@0 -> offset 2), matching claude-code, and
//! `$` (`end_of_logical_line`) lands at the trailing `\n`/`len` exactly as
//! claude-code's `findLogicalLineEnd`. `next_off(len)` is a no-op (no overflow),
//! so `de`/`d$`/`dw` at the buffer tail produce correct ranges; delete/`x`/line-op
//! cursors clamp to `len - last_char_len`. No M7-08 endpoint required correction.
//!
//! ## Telemetry (M7-08 + M7-09)
//! M7-08 and M7-09 each emit ZERO telemetry events; baseline stays 326 (M7-16
//! audits the real M7 total). `tengu_tui_vim_mode_entered` (aggregated, NOT
//! per-keystroke) is a CANDIDATE deferred to the M7-16 telemetry audit — see M7
//! design §2.7. Per-keystroke vim telemetry is explicitly NOT done. Do not
//! register a name here without an emit site (M6 discipline).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Vim editing mode. Visual is a scaffold for M7-09 (never constructed here).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VimMode {
    /// Normal mode — keys are motions/commands, not literal text.
    Normal,
    /// Insert mode — keys insert text (the M6 default editing path).
    Insert,
    /// Scaffold only — M7-09 implements Visual. M7-08 never enters this.
    Visual,
}

/// Operator scaffold for M7-09. M7-08 never sets a non-None pending operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operator {
    /// `d` — delete (M7-09).
    Delete,
    /// `c` — change (M7-09).
    Change,
    /// `y` — yank (M7-09).
    Yank,
}

/// The unnamed register: yanked/deleted text + whether it is linewise.
/// Replaces M7-08's `register: Option<String>` scaffold. Empty `text` = nothing
/// to paste. claude-code models linewise as "string ends with '\n'"; we make it
/// explicit (`operators.ts::executePaste` detects `register.endsWith('\n')`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Register {
    /// The yanked/deleted content (linewise content always ends with `\n`).
    pub text: String,
    /// Whether the content is linewise (paste opens new lines) vs charwise.
    pub linewise: bool,
}

/// Visual-mode selection. `anchor` is the byte offset where `v`/`V` was pressed;
/// the live end is the current cursor. `linewise` distinguishes `V` from `v`.
/// claude-code has NO visual mode — this is standard vim semantics (GATE note).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisualState {
    /// Byte offset where `v`/`V` was pressed (one end of the selection).
    pub anchor: usize,
    /// `true` for `V` (line-visual), `false` for `v` (char-visual).
    pub linewise: bool,
}

/// Byte length of the last `char` of `text`, or 1 if empty. The char-level
/// analogue of claude-code's `lastGrapheme(text).length || 1`, used to clamp a
/// Normal-mode cursor so it never rests past the last char of the buffer.
#[must_use]
fn last_char_len(text: &str) -> usize {
    text.chars().next_back().map_or(1, char::len_utf8)
}

/// f/F/t/T find direction+stop kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindKind {
    /// `f` — forward, land on the target char.
    F,
    /// `F` — backward, land on the target char.
    BigF,
    /// `t` — forward, land one char before the target.
    T,
    /// `T` — backward, land one char after the target.
    BigT,
}

/// NORMAL-mode command-parse sub-state. Mirrors claude-code `CommandState`,
/// M7-08 subset (no operator*, replace, indent — those are M7-09).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandState {
    /// No pending prefix.
    Idle,
    /// Accumulating a count prefix, e.g. after "3".
    Count {
        /// The digits typed so far (e.g. `"10"` for `10j`).
        digits: String,
    },
    /// After 'f'/'F'/'t'/'T' — waiting for the target char.
    Find {
        /// Which find variant (forward/backward, land-on/land-before).
        kind: FindKind,
        /// The resolved count prefix (Nth occurrence).
        count: usize,
    },
    /// After 'g' — waiting for the second key (gg, etc.).
    G {
        /// The resolved count prefix (`Ngg` → line N).
        count: usize,
    },
    // ---- M7-09 additions (port of claude-code `types.ts::CommandState`) ----
    /// After 'd'/'c'/'y' — waiting for a motion / doubled key / count / find / g.
    Operator {
        /// The chosen operator.
        op: Operator,
        /// The resolved (leading) count prefix.
        count: usize,
    },
    /// After an operator then a digit, e.g. `d3w` — accumulating the inner count.
    OperatorCount {
        /// The chosen operator.
        op: Operator,
        /// The resolved leading count (multiplies the inner count).
        count: usize,
        /// The inner-count digits typed so far.
        digits: String,
    },
    /// After an operator then f/F/t/T, e.g. `df` — waiting for the target char.
    OperatorFind {
        /// The chosen operator.
        op: Operator,
        /// The resolved count prefix (Nth occurrence).
        count: usize,
        /// Which find variant.
        kind: FindKind,
    },
    /// After an operator then 'g', e.g. `dg` — waiting for the second 'g' (dgg).
    OperatorG {
        /// The chosen operator.
        op: Operator,
        /// The resolved count prefix (`Ndgg` → line N).
        count: usize,
    },
}

/// PromptInput-local vim state (parent spec §2.3). Scaffold fields
/// (`pending_operator`, `register`) are reserved for M7-09; M7-08 leaves
/// them None/empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VimState {
    /// Current editing mode.
    pub mode: VimMode,
    /// NORMAL-mode command-parse sub-state.
    pub command: CommandState,
    /// (M7-08 scaffold; M7-09 superseded by `CommandState::Operator`.)
    /// Retained for struct stability; always `None`. Remove in a future cleanup.
    pub pending_operator: Option<Operator>,
    /// The unnamed yank/delete register (M7-09). `Register::default()` = empty.
    pub register: Register,
    /// Last f/F/t/T (kind, char) for ';'/',' — scaffold; M7-08 records it
    /// but does not implement ';'/',' (those are M7-09 polish).
    pub last_find: Option<(FindKind, char)>,
    /// Visual-mode selection (M7-09). `Some(..)` iff `mode == VimMode::Visual`.
    pub visual: Option<VisualState>,
}

impl Default for VimState {
    /// claude-code `createInitialVimState()`: start in INSERT.
    fn default() -> Self {
        Self {
            mode: VimMode::Insert,
            command: CommandState::Idle,
            pending_operator: None,
            register: Register::default(),
            last_find: None,
            visual: None,
        }
    }
}

/// Single-step motion keys M7-08 resolves. (W/B/E WORD-motions deferred.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    /// `h` — one char left.
    Left,
    /// `l` — one char right.
    Right,
    /// `j` — one logical line down (column-preserving).
    Down,
    /// `k` — one logical line up (column-preserving).
    Up,
    /// `w` — start of the next vim word.
    NextWord,
    /// `b` — start of the previous vim word.
    PrevWord,
    /// `e` — end of the current/next vim word.
    EndWord,
    /// `0` — start of the logical line.
    LineStart,
    /// `^` — first non-blank of the logical line.
    FirstNonBlank,
    /// `$` — end of the logical line.
    LineEnd,
    /// `gg` (bare) — start of the first line.
    FileStart,
    /// `G` (bare) — start of the last line.
    LastLine,
}

/// What a key did to the buffer/cursor. The caller (root.rs) applies it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VimEffect {
    /// Cursor moved to this byte offset; buffer text unchanged.
    Move(usize),
    /// Buffer replaced and cursor set (used by o/O which insert a newline).
    Edit {
        /// The new buffer contents.
        text: String,
        /// The new byte cursor (on a char boundary).
        cursor: usize,
    },
    /// No-op (unrecognized key in Normal, or motion that didn't move).
    None,
}

/// Logical-line cursor over a UTF-8 buffer. Offset is a byte index always on
/// a char boundary. Pure; every method returns a new offset.
#[derive(Debug, Clone, Copy)]
pub struct VimCursor<'a> {
    /// The buffer being navigated.
    pub text: &'a str,
    /// Byte offset into `text`, always on a char boundary.
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

    /// One char left (saturating at byte 0).
    #[must_use]
    pub fn left(&self) -> Self {
        if self.offset == 0 {
            return *self;
        }
        let prev = self.text[..self.offset]
            .char_indices()
            .last()
            .map_or(0, |(i, _)| i);
        Self {
            text: self.text,
            offset: prev,
        }
    }

    /// One char right (saturating at end of buffer).
    #[must_use]
    pub fn right(&self) -> Self {
        if self.offset >= self.text.len() {
            return *self;
        }
        let ch_len = self.text[self.offset..]
            .chars()
            .next()
            .map_or(0, char::len_utf8);
        Self {
            text: self.text,
            offset: self.offset + ch_len,
        }
    }

    fn logical_line_start(&self, from: usize) -> usize {
        self.text[..from].rfind('\n').map_or(0, |i| i + 1)
    }

    fn logical_line_end(&self, from: usize) -> usize {
        self.text[from..]
            .find('\n')
            .map_or(self.text.len(), |i| from + i)
    }

    /// Start (byte offset) of the logical line containing the cursor.
    #[must_use]
    pub fn start_of_logical_line(&self) -> Self {
        Self {
            text: self.text,
            offset: self.logical_line_start(self.offset),
        }
    }

    /// End (byte offset of the trailing `\n` or buffer end) of the logical line.
    #[must_use]
    pub fn end_of_logical_line(&self) -> Self {
        Self {
            text: self.text,
            offset: self.logical_line_end(self.offset),
        }
    }

    /// First non-blank char of the logical line (`^`), or line start if blank.
    #[must_use]
    pub fn first_non_blank(&self) -> Self {
        let start = self.logical_line_start(self.offset);
        let end = self.logical_line_end(self.offset);
        let line = &self.text[start..end];
        let rel = line.find(|c: char| !c.is_whitespace()).unwrap_or(0);
        Self {
            text: self.text,
            offset: start + rel,
        }
    }

    /// Move to the destination logical line preserving the cursor's CHARACTER
    /// column (claude-code `Cursor.up/downLogicalLine` semantics — column in
    /// string-index/char units, clamped to the dest line). CJK display-width
    /// (2-cell) columns stay deferred to M8; this is a char-count column.
    fn move_to_line(&self, target_start: usize, target_end: usize) -> Self {
        let cur_start = self.logical_line_start(self.offset);
        // Current column = number of chars from this line's start to the cursor.
        let col = self.text[cur_start..self.offset].chars().count();
        // Walk `col` chars into the dest line; a shorter dest line clamps to its
        // end (consistent with the existing end-of-line column behavior).
        let dest = &self.text[target_start..target_end];
        let raw = dest
            .char_indices()
            .nth(col)
            .map_or(target_end, |(i, _)| target_start + i);
        Self {
            text: self.text,
            offset: self.clamp(raw),
        }
    }

    /// One logical line down (`j`), preserving the char column clamped to the
    /// destination line. No-op on the last line.
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

    /// One logical line up (`k`), preserving the char column clamped to the
    /// destination line. No-op on the first line.
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

    /// Start of the first logical line (`gg` bare → offset 0).
    #[must_use]
    pub fn start_of_first_line(&self) -> Self {
        Self {
            text: self.text,
            offset: 0,
        }
    }

    /// Start of the last logical line (`G` bare).
    #[must_use]
    pub fn start_of_last_line(&self) -> Self {
        let off = self.text.rfind('\n').map_or(0, |i| i + 1);
        Self {
            text: self.text,
            offset: off,
        }
    }

    /// 1-indexed logical line, clamped (vim `Ngg` / `G`).
    #[must_use]
    pub fn go_to_line(&self, line_1indexed: usize) -> Self {
        let target = line_1indexed.saturating_sub(1);
        let mut off = 0usize;
        for (i, l) in self.text.split('\n').enumerate() {
            if i == target {
                return Self {
                    text: self.text,
                    offset: off,
                };
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

    /// Start of the next vim word (`w`): skip the current word/punct run, then
    /// skip whitespace.
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
        Self {
            text: self.text,
            offset: pos,
        }
    }

    /// Start of the previous vim word (`b`): skip whitespace backward, then to
    /// the start of the word/punct run under the resulting position.
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
            return Self {
                text: self.text,
                offset: 0,
            };
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
        Self {
            text: self.text,
            offset: pos,
        }
    }

    /// End of the current/next vim word (`e`): the last char of the word/punct
    /// run at/after the next position.
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
            return Self {
                text: self.text,
                offset: self.text.len(),
            };
        }
        let c = self.char_at(pos).unwrap();
        let pred: fn(char) -> bool = if is_word_char(c) {
            is_word_char
        } else {
            is_punct
        };
        loop {
            let nxt = self.next_off(pos);
            if nxt >= self.text.len() || !self.char_at(nxt).is_some_and(pred) {
                break;
            }
            pos = nxt;
        }
        Self {
            text: self.text,
            offset: pos,
        }
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
                        return Some(if till {
                            self.prev_off(pos).max(self.offset)
                        } else {
                            pos
                        });
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
                        return Some(if till {
                            self.next_off(pos).min(self.offset)
                        } else {
                            pos
                        });
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

/// claude-code `motions.ts::isInclusiveMotion` — `e E $` include the dest char.
/// (`E` is a WORD-motion, deferred; kept in the set for parity but never reached
/// because `motion_for_operator_key` does not map `E`.)
#[must_use]
fn is_inclusive_motion(key: char) -> bool {
    matches!(key, 'e' | 'E' | '$')
}

/// claude-code `motions.ts::isLinewiseMotion` — `j k G` and the digraph `gg`.
#[must_use]
fn is_linewise_motion(key: &str) -> bool {
    matches!(key, "j" | "k" | "G" | "gg")
}

/// Map an operator-pending motion key to its `Motion`. Returns `None` for keys
/// that are not operator-eligible motions (those are handled elsewhere: 'f'/'g'
/// start sub-states; the doubled op key is a line op).
#[must_use]
fn motion_for_operator_key(ch: char) -> Option<Motion> {
    match ch {
        'h' => Some(Motion::Left),
        'l' => Some(Motion::Right),
        'j' => Some(Motion::Down),
        'k' => Some(Motion::Up),
        'w' => Some(Motion::NextWord),
        'b' => Some(Motion::PrevWord),
        'e' => Some(Motion::EndWord),
        '0' => Some(Motion::LineStart),
        '^' => Some(Motion::FirstNonBlank),
        '$' => Some(Motion::LineEnd),
        _ => None,
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
pub fn resolve_motion(m: Motion, cursor: VimCursor<'_>, count: usize) -> VimCursor<'_> {
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

/// Resolved operator byte range over the buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperatorRange {
    /// Inclusive lower bound (byte offset).
    pub from: usize,
    /// Exclusive upper bound (byte offset).
    pub to: usize,
    /// Whether the range covers whole lines (linewise) vs chars (charwise).
    pub linewise: bool,
}

/// claude-code `operators.ts::getOperatorRange`. `cursor` is the start; `target`
/// is where the motion landed; `motion_key` classifies inclusivity/linewiseness;
/// `count` feeds the `cw`->`ce` special case.
#[must_use]
fn operator_range(
    cursor: VimCursor<'_>,
    target: usize,
    motion_key: char,
    op: Operator,
    count: usize,
) -> OperatorRange {
    let text = cursor.text;
    let mut from = cursor.offset.min(target);
    let mut to = cursor.offset.max(target);
    let mut linewise = false;

    if op == Operator::Change && motion_key == 'w' {
        // cw -> ce: change to end of (count-th) word, not start of next word.
        let mut wc = cursor;
        for _ in 0..count.saturating_sub(1) {
            wc = wc.next_vim_word();
        }
        let word_end = wc.end_vim_word();
        to = word_end.next_off(word_end.offset); // inclusive: through the last char
    } else if is_linewise_motion(motion_key.encode_utf8(&mut [0u8; 4])) {
        linewise = true;
        match text[to..].find('\n') {
            None => {
                to = text.len();
                if from > 0 && text.as_bytes()[from - 1] == b'\n' {
                    from -= 1;
                }
            }
            Some(rel) => {
                to += rel + 1; // include the newline
            }
        }
    } else if is_inclusive_motion(motion_key) && cursor.offset <= target {
        let c = VimCursor { text, offset: to };
        to = c.next_off(to);
    }

    OperatorRange { from, to, linewise }
}

/// claude-code `operators.ts::applyOperator`. Returns the effect to apply, the
/// register to store, and whether the caller should enter Insert (change only).
#[must_use]
fn apply_operator(
    op: Operator,
    text: &str,
    from: usize,
    to: usize,
    linewise: bool,
) -> (VimEffect, Register, bool) {
    let mut content = text[from..to].to_string();
    if linewise && !content.ends_with('\n') {
        content.push('\n');
    }
    let register = Register {
        text: content,
        linewise,
    };

    match op {
        Operator::Yank => (VimEffect::Move(from), register, false),
        Operator::Delete => {
            let new_text = format!("{}{}", &text[..from], &text[to..]);
            let max_off = new_text.len().saturating_sub(last_char_len(&new_text));
            let cursor = from.min(max_off);
            (
                VimEffect::Edit {
                    text: new_text,
                    cursor,
                },
                register,
                false,
            )
        }
        Operator::Change => {
            let new_text = format!("{}{}", &text[..from], &text[to..]);
            (
                VimEffect::Edit {
                    text: new_text,
                    cursor: from,
                },
                register,
                true,
            )
        }
    }
}

/// claude-code `operators.ts::executeLineOp` — dd/cc/yy over `count` logical lines.
#[must_use]
fn line_op(op: Operator, cursor: VimCursor<'_>, count: usize) -> (VimEffect, Register, bool) {
    let text = cursor.text;
    let count = count.max(1);
    let lines: Vec<&str> = text.split('\n').collect();
    // Logical line index = number of '\n' before the cursor offset.
    let current_line = text[..cursor.offset].matches('\n').count();
    let lines_to_affect = count.min(lines.len().saturating_sub(current_line));

    let line_start = cursor.start_of_logical_line().offset;
    let mut line_end = line_start;
    for _ in 0..lines_to_affect {
        match text[line_end..].find('\n') {
            None => {
                line_end = text.len();
                break;
            }
            Some(rel) => line_end += rel + 1, // include the newline
        }
    }

    let mut content = text[line_start..line_end].to_string();
    if !content.ends_with('\n') {
        content.push('\n');
    }
    let register = Register {
        text: content,
        linewise: true,
    };

    match op {
        Operator::Yank => (VimEffect::Move(line_start), register, false),
        Operator::Delete => {
            let mut delete_start = line_start;
            let delete_end = line_end;
            // Deleting to EOF with a preceding newline: consume it (no orphan '\n').
            if delete_end == text.len()
                && delete_start > 0
                && text.as_bytes()[delete_start - 1] == b'\n'
            {
                delete_start -= 1;
            }
            let new_text = format!("{}{}", &text[..delete_start], &text[delete_end..]);
            let max_off = new_text.len().saturating_sub(last_char_len(&new_text));
            let cursor_off = delete_start.min(max_off);
            (
                VimEffect::Edit {
                    text: new_text,
                    cursor: cursor_off,
                },
                register,
                false,
            )
        }
        Operator::Change => {
            if lines.len() == 1 {
                (
                    VimEffect::Edit {
                        text: String::new(),
                        cursor: 0,
                    },
                    register,
                    true,
                )
            } else {
                let before = &lines[..current_line];
                let after = &lines[(current_line + lines_to_affect)..];
                let new_lines: Vec<&str> = before
                    .iter()
                    .chain(std::iter::once(&""))
                    .chain(after.iter())
                    .copied()
                    .collect();
                let new_text = new_lines.join("\n");
                (
                    VimEffect::Edit {
                        text: new_text,
                        cursor: line_start,
                    },
                    register,
                    true,
                )
            }
        }
    }
}

/// claude-code `operators.ts::executeX` — delete `count` chars forward from the
/// cursor. Register charwise. No-op (and register untouched) if at EOF.
#[must_use]
fn delete_char_x(cursor: VimCursor<'_>, count: usize) -> (VimEffect, Register) {
    let text = cursor.text;
    let from = cursor.offset;
    if from >= text.len() {
        return (VimEffect::None, Register::default());
    }
    let mut end = cursor;
    for _ in 0..count.max(1) {
        if end.is_at_end() {
            break;
        }
        end = end.right();
    }
    let to = end.offset;
    let deleted = text[from..to].to_string();
    let new_text = format!("{}{}", &text[..from], &text[to..]);
    let max_off = new_text.len().saturating_sub(last_char_len(&new_text));
    let cursor_off = from.min(max_off);
    (
        VimEffect::Edit {
            text: new_text,
            cursor: cursor_off,
        },
        Register {
            text: deleted,
            linewise: false,
        },
    )
}

/// claude-code `operators.ts::executePaste`. `after`=p, `!after`=P. `count` repeats.
#[must_use]
fn paste(after: bool, count: usize, register: &Register, cursor: VimCursor<'_>) -> VimEffect {
    if register.text.is_empty() {
        return VimEffect::None;
    }
    let count = count.max(1);
    let text = cursor.text;

    if register.linewise {
        // Content sans the single trailing '\n', split into its lines.
        let content = register.text.strip_suffix('\n').unwrap_or(&register.text);
        let lines: Vec<&str> = text.split('\n').collect();
        let current_line = text[..cursor.offset].matches('\n').count();
        let insert_line = if after {
            current_line + 1
        } else {
            current_line
        };

        let content_lines: Vec<&str> = content.split('\n').collect();
        let mut repeated: Vec<&str> = Vec::with_capacity(content_lines.len() * count);
        for _ in 0..count {
            repeated.extend_from_slice(&content_lines);
        }

        let mut new_lines: Vec<&str> = Vec::with_capacity(lines.len() + repeated.len());
        new_lines.extend_from_slice(&lines[..insert_line]);
        new_lines.extend_from_slice(&repeated);
        new_lines.extend_from_slice(&lines[insert_line..]);

        let new_text = new_lines.join("\n");
        let cursor_off = line_start_offset(&new_lines, insert_line);
        VimEffect::Edit {
            text: new_text,
            cursor: cursor_off,
        }
    } else {
        let to_insert = register.text.repeat(count);
        let insert_point = if after && cursor.offset < text.len() {
            cursor.next_off(cursor.offset)
        } else {
            cursor.offset
        };
        let new_text = format!(
            "{}{}{}",
            &text[..insert_point],
            to_insert,
            &text[insert_point..]
        );
        let last_gr = last_char_len(&to_insert);
        let new_off = (insert_point + to_insert.len()).saturating_sub(last_gr);
        VimEffect::Edit {
            text: new_text,
            cursor: new_off.max(insert_point),
        }
    }
}

/// Byte offset of the start of `line_index` within `lines` joined by '\n'.
/// (claude-code `operators.ts::getLineStartOffset`: `lines.slice(0, lineIndex)
/// .join('\n').length + (lineIndex > 0 ? 1 : 0)` = sum(len) + one '\n' per
/// preceding line.)
#[must_use]
fn line_start_offset(lines: &[&str], line_index: usize) -> usize {
    if line_index == 0 {
        return 0;
    }
    let body: usize = lines[..line_index].iter().map(|l| l.len()).sum();
    body + line_index // one '\n' per preceding line
}

/// The i/a/o/I/A/O mode-entry effect. Returns the cursor placement (Move) or
/// the buffer edit (Edit, for o/O). Caller flips mode to Insert.
#[must_use]
pub fn enter_insert_effect(key: char, c: VimCursor<'_>) -> VimEffect {
    match key {
        'i' => VimEffect::Move(c.offset),
        'a' => VimEffect::Move(if c.is_at_end() {
            c.offset
        } else {
            c.right().offset
        }),
        'I' => VimEffect::Move(c.first_non_blank().offset),
        'A' => VimEffect::Move(c.end_of_logical_line().offset),
        'o' => {
            let end = c.end_of_logical_line().offset;
            let mut text = String::with_capacity(c.text.len() + 1);
            text.push_str(&c.text[..end]);
            text.push('\n');
            text.push_str(&c.text[end..]);
            VimEffect::Edit {
                text,
                cursor: end + 1,
            }
        }
        'O' => {
            let start = c.start_of_logical_line().offset;
            let mut text = String::with_capacity(c.text.len() + 1);
            text.push_str(&c.text[..start]);
            text.push('\n');
            text.push_str(&c.text[start..]);
            VimEffect::Edit {
                text,
                cursor: start,
            }
        }
        _ => VimEffect::None,
    }
}

/// vim Normal-mode cursor clamp on Esc: cannot rest one past the last char of
/// a non-empty logical line. Returns the clamped byte offset.
#[must_use]
pub fn esc_clamp(text: &str, offset: usize) -> usize {
    let c = VimCursor { text, offset };
    let start = c.logical_line_start(offset);
    let end = c.logical_line_end(offset);
    if offset > start && offset == end {
        // sitting at end of a non-empty line -> step left one char
        c.left().offset
    } else {
        offset
    }
}

/// What `handle_vim_key` decided. `PassThrough` means "this key is ordinary
/// Insert-mode input — run the M6 default editing pipeline." `Pending` means
/// "consumed, awaiting more keys (count/find/g), no effect yet."
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VimOutcome {
    /// The key produced a buffer/cursor effect the caller must apply.
    Effect(VimEffect),
    /// Consumed; awaiting more keys (count/find/g) — no effect yet.
    Pending,
    /// Ordinary Insert-mode input — run the M6 default editing pipeline.
    PassThrough,
}

/// Consume one key against the vim state. `text`/`offset` are the current
/// buffer + byte cursor. Mutates `state.mode`/`state.command`; returns the
/// outcome. Pure aside from the `&mut VimState`.
///
/// This is the single key dispatcher: it is intentionally one flat sequence of
/// early-return guards (insert / ctrl-alt passthrough / visual / esc) followed
/// by the command-state match (find/g/count + the M7-09 operator-pending arms).
/// Splitting it further would obscure the priority order, so the line-count lint
/// is allowed here (the same posture as root.rs `handle_live_key`).
#[allow(clippy::too_many_lines)]
pub fn handle_vim_key(
    state: &mut VimState,
    text: &str,
    offset: usize,
    key: KeyEvent,
) -> VimOutcome {
    // ----- INSERT mode: Esc -> Normal; everything else passes through. -----
    if state.mode == VimMode::Insert {
        if key.code == KeyCode::Esc {
            state.mode = VimMode::Normal;
            state.command = CommandState::Idle;
            return VimOutcome::Effect(VimEffect::Move(esc_clamp(text, offset)));
        }
        return VimOutcome::PassThrough;
    }

    // ----- NORMAL mode (and Visual scaffold, treated as Normal for M7-08). --
    // (M7-08 review) CONTROL/ALT combos are app-level bindings, NOT vim
    // commands: Ctrl-C (cancel), Ctrl-Alt-V (vim toggle), and any other ctrl
    // binding. Pass them through so `handle_live_key` falls to the
    // `map_iocraft_key` + `dispatch` pipeline. Without this, Normal mode
    // swallowed EVERY ctrl/alt combo — you could not cancel with Ctrl-C nor
    // toggle vim off from Normal. Pending command state is left intact (the
    // combo is not a vim key, so it neither advances nor cancels it). SHIFT is
    // deliberately NOT passed through — `G`/`$`/`^`/`A` are real vim motions.
    if key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
        return VimOutcome::PassThrough;
    }

    // ----- VISUAL mode (M7-09). claude-code has no visual mode; standard vim. --
    // Placed after the ctrl/alt passthrough (so Ctrl-Alt-V / Ctrl-C still escape
    // a Visual selection) and before the Normal Esc-cancel.
    if state.mode == VimMode::Visual {
        return handle_visual_key(state, text, offset, key);
    }

    // Esc in Normal cancels any pending command.
    if key.code == KeyCode::Esc {
        state.command = CommandState::Idle;
        return VimOutcome::Effect(VimEffect::None);
    }

    let cursor = VimCursor { text, offset };

    // Literal-char states (Find target, G second key) consume the raw char.
    match std::mem::replace(&mut state.command, CommandState::Idle) {
        CommandState::Find { kind, count } => {
            if let KeyCode::Char(ch) = key.code {
                state.last_find = Some((kind, ch));
                return match cursor.find_character(ch, kind, count) {
                    Some(off) => VimOutcome::Effect(VimEffect::Move(off)),
                    None => VimOutcome::Effect(VimEffect::None),
                };
            }
            return VimOutcome::Effect(VimEffect::None);
        }
        CommandState::G { count } => {
            if let KeyCode::Char('g') = key.code {
                let dest = if count > 1 {
                    cursor.go_to_line(count)
                } else {
                    cursor.start_of_first_line()
                };
                return VimOutcome::Effect(VimEffect::Move(dest.offset));
            }
            // any other key cancels the g-prefix
            return VimOutcome::Effect(VimEffect::None);
        }
        CommandState::Count { digits } => {
            // Continue count, or execute with the parsed count.
            if let KeyCode::Char(c @ '0'..='9') = key.code {
                let mut d = digits;
                d.push(c);
                state.command = CommandState::Count { digits: d };
                return VimOutcome::Pending;
            }
            let count = digits.parse::<usize>().unwrap_or(1).max(1);
            return dispatch_normal(state, cursor, count, key);
        }
        // ---- M7-09 operator-pending arms (port of claude-code operators.ts). --
        CommandState::Operator { op, count } => {
            return dispatch_operator_pending(state, cursor, op, count, key);
        }
        CommandState::OperatorCount { op, count, digits } => {
            if let KeyCode::Char(c @ '0'..='9') = key.code {
                let mut d = digits;
                d.push(c);
                state.command = CommandState::OperatorCount {
                    op,
                    count,
                    digits: d,
                };
                return VimOutcome::Pending;
            }
            let inner = digits.parse::<usize>().unwrap_or(1).max(1);
            return dispatch_operator_pending(state, cursor, op, count * inner, key);
        }
        CommandState::OperatorFind { op, count, kind } => {
            if let KeyCode::Char(ch) = key.code {
                state.last_find = Some((kind, ch));
                return match cursor.find_character(ch, kind, count) {
                    Some(target) => {
                        // find ranges are inclusive (operators.ts::getOperatorRangeForFind).
                        let from = cursor.offset.min(target);
                        let to_raw = cursor.offset.max(target);
                        let to = VimCursor {
                            text: cursor.text,
                            offset: to_raw,
                        }
                        .next_off(to_raw);
                        finish_operator(state, op, cursor.text, from, to, false)
                    }
                    None => VimOutcome::Effect(VimEffect::None),
                };
            }
            return VimOutcome::Effect(VimEffect::None);
        }
        CommandState::OperatorG { op, count } => {
            if let KeyCode::Char('g') = key.code {
                let target = if count > 1 {
                    cursor.go_to_line(count)
                } else {
                    cursor.start_of_first_line()
                };
                return operator_over_lines(state, op, cursor, target.offset);
            }
            return VimOutcome::Effect(VimEffect::None);
        }
        CommandState::Idle => {}
    }

    // From Idle: a 1-9 starts a count; everything else dispatches with count 1.
    if let KeyCode::Char(c @ '1'..='9') = key.code {
        state.command = CommandState::Count {
            digits: c.to_string(),
        };
        return VimOutcome::Pending;
    }
    dispatch_normal(state, cursor, 1, key)
}

/// Handle one key while in VISUAL mode (M7-09). `v`/`V` set the anchor in
/// `dispatch_normal`; here motions move the live end, `d`/`c`/`y` apply over the
/// selection (then -> Normal/Insert), and `Esc` cancels to Normal. claude-code
/// has no visual mode — this is standard vim semantics (see the GATE note).
fn handle_visual_key(state: &mut VimState, text: &str, offset: usize, key: KeyEvent) -> VimOutcome {
    let visual = state.visual.expect("Visual mode without VisualState");
    let vcursor = VimCursor { text, offset };

    // Esc -> Normal (clamp the cursor as Normal mode requires).
    if key.code == KeyCode::Esc {
        state.mode = VimMode::Normal;
        state.visual = None;
        state.command = CommandState::Idle;
        return VimOutcome::Effect(VimEffect::Move(esc_clamp(text, offset)));
    }

    let KeyCode::Char(ch) = key.code else {
        return VimOutcome::Effect(VimEffect::None);
    };

    // d/c/y operate on the selection, then return to Normal/Insert.
    if matches!(ch, 'd' | 'c' | 'y') {
        let op = match ch {
            'd' => Operator::Delete,
            'c' => Operator::Change,
            _ => Operator::Yank,
        };
        let (from, to, linewise) = visual_range(text, visual, offset);
        state.mode = VimMode::Normal;
        state.visual = None;
        return finish_operator(state, op, text, from, to, linewise);
    }

    // Pending count / g-prefix inside Visual reuse the Count / G sub-states.
    match std::mem::replace(&mut state.command, CommandState::Idle) {
        CommandState::Count { digits } => {
            if let '0'..='9' = ch {
                let mut d = digits;
                d.push(ch);
                state.command = CommandState::Count { digits: d };
                return VimOutcome::Pending;
            }
            let count = digits.parse::<usize>().unwrap_or(1).max(1);
            return visual_motion(state, vcursor, count, ch);
        }
        CommandState::G { count: _ } => {
            if ch == 'g' {
                return VimOutcome::Effect(VimEffect::Move(vcursor.start_of_first_line().offset));
            }
            return VimOutcome::Effect(VimEffect::None);
        }
        CommandState::Idle => {}
        other => {
            state.command = other; // shouldn't happen in Visual; keep
        }
    }
    if let '1'..='9' = ch {
        state.command = CommandState::Count {
            digits: ch.to_string(),
        };
        return VimOutcome::Pending;
    }
    visual_motion(state, vcursor, 1, ch)
}

/// Handle a Normal-mode key that is NOT a count digit, with `count` resolved.
fn dispatch_normal(
    state: &mut VimState,
    cursor: VimCursor<'_>,
    count: usize,
    key: KeyEvent,
) -> VimOutcome {
    let KeyCode::Char(ch) = key.code else {
        return VimOutcome::Effect(VimEffect::None);
    };

    // Mode-entry keys.
    if matches!(ch, 'i' | 'a' | 'I' | 'A' | 'o' | 'O') {
        state.mode = VimMode::Insert;
        return VimOutcome::Effect(enter_insert_effect(ch, cursor));
    }

    // Operators (M7-09): enter operator-pending. A leading count (already
    // resolved into `count`) seeds the operator's count; an inner count after
    // the operator (`d3w`) multiplies via `OperatorCount`.
    let op = match ch {
        'd' => Some(Operator::Delete),
        'c' => Some(Operator::Change),
        'y' => Some(Operator::Yank),
        _ => None,
    };
    if let Some(op) = op {
        state.command = CommandState::Operator { op, count };
        return VimOutcome::Pending;
    }

    // x: delete count chars under/after the cursor.
    if ch == 'x' {
        let (effect, register) = delete_char_x(cursor, count);
        if effect != VimEffect::None {
            state.register = register;
        }
        return VimOutcome::Effect(effect);
    }

    // p / P: paste the unnamed register.
    if ch == 'p' || ch == 'P' {
        let effect = paste(ch == 'p', count, &state.register, cursor);
        return VimOutcome::Effect(effect);
    }

    // v / V: enter Visual / Visual-line (M7-09).
    if ch == 'v' || ch == 'V' {
        state.mode = VimMode::Visual;
        state.visual = Some(VisualState {
            anchor: cursor.offset,
            linewise: ch == 'V',
        });
        return VimOutcome::Effect(VimEffect::None);
    }

    // Simple motions.
    let motion = match ch {
        'h' => Some(Motion::Left),
        'l' => Some(Motion::Right),
        'j' => Some(Motion::Down),
        'k' => Some(Motion::Up),
        'w' => Some(Motion::NextWord),
        'b' => Some(Motion::PrevWord),
        'e' => Some(Motion::EndWord),
        '0' => Some(Motion::LineStart),
        '^' => Some(Motion::FirstNonBlank),
        '$' => Some(Motion::LineEnd),
        'G' => Some(Motion::LastLine),
        _ => None,
    };
    if let Some(m) = motion {
        let dest = resolve_motion(m, cursor, count);
        return VimOutcome::Effect(VimEffect::Move(dest.offset));
    }

    // Find prefixes.
    let find_kind = match ch {
        'f' => Some(FindKind::F),
        'F' => Some(FindKind::BigF),
        't' => Some(FindKind::T),
        'T' => Some(FindKind::BigT),
        _ => None,
    };
    if let Some(kind) = find_kind {
        state.command = CommandState::Find { kind, count };
        return VimOutcome::Pending;
    }

    // g-prefix.
    if ch == 'g' {
        state.command = CommandState::G { count };
        return VimOutcome::Pending;
    }

    VimOutcome::Effect(VimEffect::None)
}

/// One key in operator-pending state (op already chosen, count resolved).
fn dispatch_operator_pending(
    state: &mut VimState,
    cursor: VimCursor<'_>,
    op: Operator,
    count: usize,
    key: KeyEvent,
) -> VimOutcome {
    // Esc cancels.
    if key.code == KeyCode::Esc {
        state.command = CommandState::Idle;
        return VimOutcome::Effect(VimEffect::None);
    }
    let KeyCode::Char(ch) = key.code else {
        state.command = CommandState::Idle;
        return VimOutcome::Effect(VimEffect::None);
    };

    // Inner count: a digit 1-9 seeds OperatorCount. ('0' is the LineStart motion,
    // not a count seed — mirrors M7-08's "`0` is a motion from Idle.")
    if let '1'..='9' = ch {
        state.command = CommandState::OperatorCount {
            op,
            count,
            digits: ch.to_string(),
        };
        return VimOutcome::Pending;
    }

    // Doubled operator key -> line op (dd/cc/yy).
    let op_char = match op {
        Operator::Delete => 'd',
        Operator::Change => 'c',
        Operator::Yank => 'y',
    };
    if ch == op_char {
        let (effect, register, enter_insert) = line_op(op, cursor, count);
        state.register = register;
        state.command = CommandState::Idle;
        if enter_insert {
            state.mode = VimMode::Insert;
        }
        return VimOutcome::Effect(effect);
    }

    // Find prefix -> OperatorFind.
    let find_kind = match ch {
        'f' => Some(FindKind::F),
        'F' => Some(FindKind::BigF),
        't' => Some(FindKind::T),
        'T' => Some(FindKind::BigT),
        _ => None,
    };
    if let Some(kind) = find_kind {
        state.command = CommandState::OperatorFind { op, count, kind };
        return VimOutcome::Pending;
    }

    // g -> OperatorG (dgg).
    if ch == 'g' {
        state.command = CommandState::OperatorG { op, count };
        return VimOutcome::Pending;
    }

    // G -> operator over lines to last/Nth line.
    if ch == 'G' {
        let target = if count > 1 {
            cursor.go_to_line(count)
        } else {
            cursor.start_of_last_line()
        };
        return operator_over_lines(state, op, cursor, target.offset);
    }

    // Simple motion.
    if let Some(motion) = motion_for_operator_key(ch) {
        let target = resolve_motion(motion, cursor, count);
        if target.offset == cursor.offset {
            // motion didn't move -> operator no-op (operators.ts: target.equals(cursor)).
            state.command = CommandState::Idle;
            return VimOutcome::Effect(VimEffect::None);
        }
        let range = operator_range(cursor, target.offset, ch, op, count);
        return finish_operator(state, op, cursor.text, range.from, range.to, range.linewise);
    }

    // Text objects (iw/aw...) and any other key: DEFERRED (GATE) -> cancel, no-op.
    state.command = CommandState::Idle;
    VimOutcome::Effect(VimEffect::None)
}

/// Apply an operator over a resolved byte range; store register; reset command;
/// enter Insert for change. A zero-width range is a no-op (register untouched).
fn finish_operator(
    state: &mut VimState,
    op: Operator,
    text: &str,
    from: usize,
    to: usize,
    linewise: bool,
) -> VimOutcome {
    state.command = CommandState::Idle;
    if from == to {
        return VimOutcome::Effect(VimEffect::None);
    }
    let (effect, register, enter_insert) = apply_operator(op, text, from, to, linewise);
    state.register = register;
    if enter_insert {
        state.mode = VimMode::Insert;
    }
    VimOutcome::Effect(effect)
}

/// Operator over whole lines from cursor's line to `target_offset`'s line
/// (dG / dgg / `NdG`). Always linewise. Mirrors `executeOperatorG`/`Gg` + line range.
fn operator_over_lines(
    state: &mut VimState,
    op: Operator,
    cursor: VimCursor<'_>,
    target_offset: usize,
) -> VimOutcome {
    let text = cursor.text;
    let from_line_start = VimCursor {
        text,
        offset: cursor.offset.min(target_offset),
    }
    .start_of_logical_line()
    .offset;
    let to_line = VimCursor {
        text,
        offset: cursor.offset.max(target_offset),
    };
    let to_line_end = match text[to_line.offset..].find('\n') {
        None => text.len(),
        Some(rel) => to_line.offset + rel + 1,
    };
    finish_operator(state, op, text, from_line_start, to_line_end, true)
}

/// Byte range + linewise flag for a Visual selection from anchor to cursor.
/// Charwise: inclusive of the cursor char (+1 char on the high end).
/// Linewise: whole logical lines spanning anchor..cursor.
#[must_use]
fn visual_range(text: &str, visual: VisualState, cursor_offset: usize) -> (usize, usize, bool) {
    let lo = visual.anchor.min(cursor_offset);
    let hi = visual.anchor.max(cursor_offset);
    if visual.linewise {
        let from = VimCursor { text, offset: lo }
            .start_of_logical_line()
            .offset;
        let to = match text[hi..].find('\n') {
            None => text.len(),
            Some(rel) => hi + rel + 1,
        };
        (from, to, true)
    } else {
        // charwise inclusive: extend one char past the high offset (clamped to len).
        let to = VimCursor { text, offset: hi }.next_off(hi);
        (lo, to, false)
    }
}

/// A motion key inside Visual mode: move the cursor (selection end). Anchor stays.
/// `gg`/`G` are linewise navigation; supported as selection extenders.
fn visual_motion(
    state: &mut VimState,
    cursor: VimCursor<'_>,
    count: usize,
    ch: char,
) -> VimOutcome {
    let dest = match ch {
        'g' => {
            // Need a second 'g'; stash a G-pending. The Visual block's `G` arm
            // resolves `gg` to file-start on the next key.
            state.command = CommandState::G { count };
            return VimOutcome::Pending;
        }
        'G' => return VimOutcome::Effect(VimEffect::Move(cursor.start_of_last_line().offset)),
        _ => match motion_for_operator_key(ch) {
            Some(m) => resolve_motion(m, cursor, count),
            None => return VimOutcome::Effect(VimEffect::None),
        },
    };
    VimOutcome::Effect(VimEffect::Move(dest.offset))
}

/// Footer mode-indicator literal. Matches the well-known vim convention
/// (claude-code surfaces the mode via `PromptInputModeIndicator`; the literal
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

    #[test]
    fn m7_08_adds_no_telemetry_events() {
        // M7-08 ships 0 new telemetry events. The M7-16 audit (which IS the
        // "real M7 total" decision this guard deferred to) locked the registry
        // at 330: vim itself still registers NOTHING —
        // `tengu_tui_vim_mode_entered` stayed DEFERRED (Esc-from-Insert churn,
        // no aggregator). The +4 vs the 326 M6 baseline is M7-16's
        // screen_opened/screen_closed/search_opened + lingxi_core_v0_8_0_released
        // (none from vim). This guard fails if vim accidentally mints an event.
        // Baseline 330 → 336 after the CronDelete/CronList +6 tool events (LSP.7b).
        assert_eq!(telemetry::tengu::ALL_EVENT_NAMES.len(), 336);
    }

    #[test]
    fn m7_09_adds_no_telemetry_events() {
        // M7-09 ships 0 new telemetry events. Registry locked at 330 by the
        // M7-16 audit; vim operators/visual emit nothing (per-keystroke
        // telemetry is explicitly NOT done; `vim_mode_entered` stayed deferred).
        // The +4 vs the 326 baseline is all M7-16 (screen/search + release
        // marker). This guard fails if vim registers a new event.
        // Baseline 330 → 336 after the CronDelete/CronList +6 tool events (LSP.7b).
        assert_eq!(telemetry::tengu::ALL_EVENT_NAMES.len(), 336);
    }

    #[test]
    fn deferred_text_object_after_operator_is_noop_not_panic() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut s = VimState {
            mode: VimMode::Normal,
            ..VimState::default()
        };
        // 'd' then 'i' (would be `diw` text-object in full vim) -> deferred -> no-op.
        handle_vim_key(
            &mut s,
            "foo bar",
            1,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
        );
        let out = handle_vim_key(
            &mut s,
            "foo bar",
            1,
            KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE),
        );
        assert_eq!(out, VimOutcome::Effect(VimEffect::None));
        assert_eq!(s.command, CommandState::Idle);
        assert_eq!(s.register, Register::default()); // nothing deleted/yanked
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
        assert_eq!(cur("héllo", 0).left().offset, 0); // clamp at 0
        assert_eq!(cur("héllo", 0).right().offset, 1);
        assert_eq!(cur("héllo", 1).right().offset, 3); // skip whole 'é'
        assert_eq!(cur("héllo", 3).left().offset, 1);
        assert_eq!(cur("hi", 2).right().offset, 2); // clamp at end
    }

    #[test]
    fn logical_line_bounds() {
        let t = "abc\ndefg\nhi";
        // cursor in middle of line 2 (offset 6 = 'f')
        assert_eq!(cur(t, 6).start_of_logical_line().offset, 4); // 'd'
        assert_eq!(cur(t, 6).end_of_logical_line().offset, 8); // after 'g' (the \n)
                                                               // line 1 has no leading blanks -> first_non_blank == start
        assert_eq!(cur("  xy", 3).first_non_blank().offset, 2); // 'x'
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
    fn down_up_preserve_char_column_multibyte() {
        // 'é' is 2 bytes. "éé\nabcd":
        //   line0 "éé" = bytes 0..4 (é@0..2, é@2..4), '\n'@4, line1 "abcd" = 5..9.
        // Cursor at char-col 2 of line0 = byte 4 (end of "éé").
        // j must preserve the CHAR column (2), landing on 'c' (byte 7) — NOT the
        // byte column (4) which would land past 'd' at byte 9.
        let t = "éé\nabcd";
        assert_eq!(cur(t, 4).down_logical_line().offset, 7); // 'c'

        // k reverse: from char-col 2 of "abcd" (byte 7 = 'c') back up to "éé".
        // char-col 2 of "éé" is byte 4 (end-of-line position for a 2-char line).
        assert_eq!(cur(t, 7).up_logical_line().offset, 4);
    }

    #[test]
    fn down_clamps_to_shorter_multibyte_dest_line() {
        // "ééé\nx": line0 "ééé" = 0..6, '\n'@6, line1 "x" = 7..8.
        // Cursor at char-col 3 of line0 = byte 6 (end of "ééé").
        // j into "x" (only 1 char) clamps to the dest line end = byte 8.
        let t = "ééé\nx";
        assert_eq!(cur(t, 6).down_logical_line().offset, 8);
    }

    #[test]
    fn down_char_column_mixed_multibyte_then_ascii() {
        // "éabc\nxyzw": é@0..2, a@2, b@3, c@4, '\n'@5, x@6, y@7, z@8, w@9.
        // Cursor on line0 at char-col 2 = 'b' (byte 3).
        // j must land on char-col 2 of "xyzw" = 'z' (byte 8), NOT byte-col 3 = 'w'(9).
        let t = "éabc\nxyzw";
        assert_eq!(cur(t, 3).down_logical_line().offset, 8); // 'z'
    }

    #[test]
    fn first_last_line_and_goto() {
        let t = "one\ntwo\nthree";
        assert_eq!(cur(t, 9).start_of_first_line().offset, 0);
        assert_eq!(cur(t, 0).start_of_last_line().offset, 8); // 'three'
        assert_eq!(cur(t, 0).go_to_line(2).offset, 4); // 'two' (1-indexed)
        assert_eq!(cur(t, 0).go_to_line(99).offset, 8); // clamp to last
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
        assert_eq!(
            resolve_motion(Motion::LineEnd, cur("hello", 0), 1).offset,
            5
        );
        assert_eq!(
            resolve_motion(Motion::LineStart, cur("hello", 3), 1).offset,
            0
        );
    }

    #[test]
    fn count_repeats_motion() {
        assert_eq!(resolve_motion(Motion::Right, cur("hello", 0), 3).offset, 3);
        assert_eq!(
            resolve_motion(Motion::NextWord, cur("a b c d", 0), 2).offset,
            4
        ); // 'c'
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

#[cfg(test)]
mod transition_tests {
    use super::*;
    fn cur(text: &str, off: usize) -> VimCursor<'_> {
        VimCursor { text, offset: off }
    }

    #[test]
    fn i_inserts_at_cursor() {
        assert_eq!(
            enter_insert_effect('i', cur("hello", 2)),
            VimEffect::Move(2)
        );
    }

    #[test]
    fn a_inserts_after_cursor() {
        assert_eq!(
            enter_insert_effect('a', cur("hello", 2)),
            VimEffect::Move(3)
        );
        // at end: stays
        assert_eq!(
            enter_insert_effect('a', cur("hello", 5)),
            VimEffect::Move(5)
        );
    }

    #[test]
    fn cap_i_first_non_blank() {
        assert_eq!(enter_insert_effect('I', cur("  hi", 3)), VimEffect::Move(2));
    }

    #[test]
    fn cap_a_end_of_line() {
        assert_eq!(
            enter_insert_effect('A', cur("ab\ncd", 0)),
            VimEffect::Move(2)
        ); // end of line0
    }

    #[test]
    fn o_opens_line_below() {
        // "ab\ncd", cursor on line0 -> newline after line0, cursor at its start (offset 3)
        assert_eq!(
            enter_insert_effect('o', cur("ab\ncd", 1)),
            VimEffect::Edit {
                text: "ab\n\ncd".to_string(),
                cursor: 3
            }
        );
    }

    #[test]
    fn cap_o_opens_line_above() {
        // "ab\ncd", cursor on line1 ('c' @3) -> newline before line1, cursor at its start (offset 3)
        assert_eq!(
            enter_insert_effect('O', cur("ab\ncd", 3)),
            VimEffect::Edit {
                text: "ab\n\ncd".to_string(),
                cursor: 3
            }
        );
    }

    #[test]
    fn esc_clamps_past_end_of_line() {
        // In vim, Normal-mode cursor cannot sit on the trailing position of a
        // non-empty line; clamp left by one char.
        assert_eq!(esc_clamp("hello", 5), 4);
        assert_eq!(esc_clamp("hello", 3), 3); // already valid
        assert_eq!(esc_clamp("", 0), 0); // empty line: stay
        assert_eq!(esc_clamp("ab\ncd", 2), 1); // end of line0 -> clamp to 'b'
    }
}

#[cfg(test)]
mod dispatch_tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }
    fn esc() -> KeyEvent {
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
    }

    #[test]
    fn esc_from_insert_enters_normal_and_clamps() {
        let mut s = VimState::default(); // Insert
        let out = handle_vim_key(&mut s, "hello", 5, esc());
        assert_eq!(s.mode, VimMode::Normal);
        assert_eq!(out, VimOutcome::Effect(VimEffect::Move(4)));
    }

    #[test]
    fn insert_mode_passes_through_chars() {
        let mut s = VimState::default(); // Insert
        let out = handle_vim_key(&mut s, "hi", 2, key('x'));
        assert_eq!(s.mode, VimMode::Insert);
        assert_eq!(out, VimOutcome::PassThrough); // default editing inserts 'x'
    }

    #[test]
    fn normal_h_l_move() {
        let mut s = VimState {
            mode: VimMode::Normal,
            ..VimState::default()
        };
        assert_eq!(
            handle_vim_key(&mut s, "hello", 2, key('l')),
            VimOutcome::Effect(VimEffect::Move(3))
        );
        assert_eq!(
            handle_vim_key(&mut s, "hello", 2, key('h')),
            VimOutcome::Effect(VimEffect::Move(1))
        );
    }

    #[test]
    fn normal_i_enters_insert() {
        let mut s = VimState {
            mode: VimMode::Normal,
            ..VimState::default()
        };
        let out = handle_vim_key(&mut s, "hello", 2, key('i'));
        assert_eq!(s.mode, VimMode::Insert);
        assert_eq!(out, VimOutcome::Effect(VimEffect::Move(2)));
    }

    #[test]
    fn count_then_motion() {
        let mut s = VimState {
            mode: VimMode::Normal,
            ..VimState::default()
        };
        // "3w" on "a b c d e" -> 4th word
        assert_eq!(
            handle_vim_key(&mut s, "a b c d e", 0, key('3')),
            VimOutcome::Pending
        );
        assert_eq!(s.command, CommandState::Count { digits: "3".into() });
        let out = handle_vim_key(&mut s, "a b c d e", 0, key('w'));
        assert_eq!(out, VimOutcome::Effect(VimEffect::Move(6))); // 'd'
        assert_eq!(s.command, CommandState::Idle); // reset after execute
    }

    #[test]
    fn zero_is_line_start_not_count() {
        let mut s = VimState {
            mode: VimMode::Normal,
            ..VimState::default()
        };
        let out = handle_vim_key(&mut s, "  hello", 4, key('0'));
        assert_eq!(out, VimOutcome::Effect(VimEffect::Move(0)));
    }

    #[test]
    fn caret_first_non_blank() {
        let mut s = VimState {
            mode: VimMode::Normal,
            ..VimState::default()
        };
        let out = handle_vim_key(&mut s, "  hello", 4, key('^'));
        assert_eq!(out, VimOutcome::Effect(VimEffect::Move(2)));
    }

    #[test]
    fn gg_goes_to_first_line() {
        let mut s = VimState {
            mode: VimMode::Normal,
            ..VimState::default()
        };
        assert_eq!(
            handle_vim_key(&mut s, "a\nb\nc", 4, key('g')),
            VimOutcome::Pending
        );
        assert_eq!(s.command, CommandState::G { count: 1 });
        let out = handle_vim_key(&mut s, "a\nb\nc", 4, key('g'));
        assert_eq!(out, VimOutcome::Effect(VimEffect::Move(0)));
    }

    #[test]
    fn count_gg_goes_to_line_n() {
        let mut s = VimState {
            mode: VimMode::Normal,
            ..VimState::default()
        };
        handle_vim_key(&mut s, "a\nb\nc", 0, key('2'));
        handle_vim_key(&mut s, "a\nb\nc", 0, key('g'));
        let out = handle_vim_key(&mut s, "a\nb\nc", 0, key('g'));
        assert_eq!(out, VimOutcome::Effect(VimEffect::Move(2))); // line 2 = 'b'
    }

    #[test]
    fn cap_g_goes_to_last_line() {
        let mut s = VimState {
            mode: VimMode::Normal,
            ..VimState::default()
        };
        let out = handle_vim_key(
            &mut s,
            "a\nb\nc",
            0,
            KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT),
        );
        assert_eq!(out, VimOutcome::Effect(VimEffect::Move(4))); // 'c'
    }

    #[test]
    fn f_char_finds() {
        let mut s = VimState {
            mode: VimMode::Normal,
            ..VimState::default()
        };
        assert_eq!(
            handle_vim_key(&mut s, "abcdc", 0, key('f')),
            VimOutcome::Pending
        );
        assert_eq!(
            handle_vim_key(&mut s, "abcdc", 0, key('c')),
            VimOutcome::Effect(VimEffect::Move(2))
        );
    }

    #[test]
    fn count_f_finds_nth() {
        let mut s = VimState {
            mode: VimMode::Normal,
            ..VimState::default()
        };
        handle_vim_key(&mut s, "abcdc", 0, key('2'));
        handle_vim_key(&mut s, "abcdc", 0, key('f'));
        let out = handle_vim_key(&mut s, "abcdc", 0, key('c'));
        assert_eq!(out, VimOutcome::Effect(VimEffect::Move(4))); // 2nd 'c'
    }

    #[test]
    fn find_not_found_is_noop() {
        let mut s = VimState {
            mode: VimMode::Normal,
            ..VimState::default()
        };
        handle_vim_key(&mut s, "abc", 0, key('f'));
        assert_eq!(
            handle_vim_key(&mut s, "abc", 0, key('z')),
            VimOutcome::Effect(VimEffect::None)
        );
        assert_eq!(s.command, CommandState::Idle);
    }

    #[test]
    fn unknown_normal_key_is_noop() {
        let mut s = VimState {
            mode: VimMode::Normal,
            ..VimState::default()
        };
        assert_eq!(
            handle_vim_key(&mut s, "abc", 0, key('q')),
            VimOutcome::Effect(VimEffect::None)
        );
    }

    // (M7-08 review) Normal mode must NOT swallow CONTROL/ALT key combos: they
    // are app-level bindings (Ctrl-C cancel, Ctrl-Alt-V vim toggle, …) that the
    // dispatcher owns. Returning `PassThrough` lets `handle_live_key` fall
    // through to `map_iocraft_key` + `dispatch`. Plain (NONE/SHIFT) keys still
    // route to vim so motions like `h`/`G`/`$` keep working.

    #[test]
    fn normal_ctrl_combo_passes_through() {
        let mut s = VimState {
            mode: VimMode::Normal,
            ..VimState::default()
        };
        // Ctrl-C in Normal mode must pass through to the cancel binding.
        let out = handle_vim_key(
            &mut s,
            "abc",
            0,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        );
        assert_eq!(out, VimOutcome::PassThrough);
        assert_eq!(s.mode, VimMode::Normal, "mode must be untouched");
    }

    #[test]
    fn normal_ctrl_alt_v_passes_through() {
        let mut s = VimState {
            mode: VimMode::Normal,
            ..VimState::default()
        };
        // Ctrl-Alt-V (the vim toggle) must pass through, not be swallowed.
        let out = handle_vim_key(
            &mut s,
            "abc",
            0,
            KeyEvent::new(
                KeyCode::Char('v'),
                KeyModifiers::CONTROL | KeyModifiers::ALT,
            ),
        );
        assert_eq!(out, VimOutcome::PassThrough);
    }

    #[test]
    fn normal_alt_combo_passes_through() {
        let mut s = VimState {
            mode: VimMode::Normal,
            ..VimState::default()
        };
        let out = handle_vim_key(
            &mut s,
            "abc",
            0,
            KeyEvent::new(KeyCode::Char('b'), KeyModifiers::ALT),
        );
        assert_eq!(out, VimOutcome::PassThrough);
    }

    #[test]
    fn normal_plain_and_shift_keys_still_route_to_vim() {
        let mut s = VimState {
            mode: VimMode::Normal,
            ..VimState::default()
        };
        // Plain 'l' is a vim motion, NOT a pass-through.
        assert_eq!(
            handle_vim_key(&mut s, "hello", 0, key('l')),
            VimOutcome::Effect(VimEffect::Move(1))
        );
        // SHIFT 'G' (last line) is a vim motion, NOT a pass-through.
        assert_eq!(
            handle_vim_key(
                &mut s,
                "a\nb",
                0,
                KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT)
            ),
            VimOutcome::Effect(VimEffect::Move(2))
        );
    }

    #[test]
    fn ctrl_combo_passes_through_even_in_pending_count() {
        // A pending count must not trap a ctrl combo either — Ctrl-C should
        // still reach the cancel binding mid-count.
        let mut s = VimState {
            mode: VimMode::Normal,
            ..VimState::default()
        };
        assert_eq!(
            handle_vim_key(&mut s, "abc", 0, key('2')),
            VimOutcome::Pending
        );
        let out = handle_vim_key(
            &mut s,
            "abc",
            0,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        );
        assert_eq!(out, VimOutcome::PassThrough);
    }
}

#[cfg(test)]
mod m7_09_types_tests {
    use super::*;

    #[test]
    fn default_register_is_empty_charwise() {
        let s = VimState::default();
        assert_eq!(s.register, Register::default());
        assert!(s.register.text.is_empty());
        assert!(!s.register.linewise);
        assert!(s.visual.is_none());
    }

    #[test]
    fn operator_command_states_constructible() {
        let a = CommandState::Operator {
            op: Operator::Delete,
            count: 1,
        };
        let b = CommandState::OperatorCount {
            op: Operator::Change,
            count: 1,
            digits: "3".into(),
        };
        let c = CommandState::OperatorFind {
            op: Operator::Yank,
            count: 2,
            kind: FindKind::F,
        };
        let d = CommandState::OperatorG {
            op: Operator::Delete,
            count: 1,
        };
        assert_ne!(a, b);
        assert_ne!(c, d);
    }

    #[test]
    fn visual_state_records_anchor_and_kind() {
        let v = VisualState {
            anchor: 3,
            linewise: true,
        };
        assert_eq!(v.anchor, 3);
        assert!(v.linewise);
    }

    #[test]
    fn last_char_len_handles_utf8_and_empty() {
        assert_eq!(last_char_len("hi"), 1);
        assert_eq!(last_char_len("hé"), 2); // 'é' is 2 bytes
        assert_eq!(last_char_len(""), 1); // empty -> 1
    }
}

#[cfg(test)]
mod motion_class_tests {
    use super::*;

    #[test]
    fn inclusive_motions_are_e_and_dollar() {
        assert!(is_inclusive_motion('e'));
        assert!(is_inclusive_motion('$'));
        assert!(!is_inclusive_motion('w'));
        assert!(!is_inclusive_motion('0'));
        assert!(!is_inclusive_motion('h'));
    }

    #[test]
    fn linewise_motions_are_jk_and_g() {
        assert!(is_linewise_motion("j"));
        assert!(is_linewise_motion("k"));
        assert!(is_linewise_motion("G"));
        assert!(is_linewise_motion("gg"));
        assert!(!is_linewise_motion("w"));
        assert!(!is_linewise_motion("$"));
    }

    #[test]
    fn operator_motion_map_covers_required_keys() {
        assert_eq!(motion_for_operator_key('w'), Some(Motion::NextWord));
        assert_eq!(motion_for_operator_key('b'), Some(Motion::PrevWord));
        assert_eq!(motion_for_operator_key('e'), Some(Motion::EndWord));
        assert_eq!(motion_for_operator_key('$'), Some(Motion::LineEnd));
        assert_eq!(motion_for_operator_key('0'), Some(Motion::LineStart));
        assert_eq!(motion_for_operator_key('^'), Some(Motion::FirstNonBlank));
        assert_eq!(motion_for_operator_key('h'), Some(Motion::Left));
        assert_eq!(motion_for_operator_key('l'), Some(Motion::Right));
        assert_eq!(motion_for_operator_key('j'), Some(Motion::Down));
        assert_eq!(motion_for_operator_key('k'), Some(Motion::Up));
        assert_eq!(motion_for_operator_key('z'), None);
    }
}

#[cfg(test)]
mod apply_op_tests {
    use super::*;
    fn cur(text: &str, off: usize) -> VimCursor<'_> {
        VimCursor { text, offset: off }
    }

    #[test]
    fn range_exclusive_for_w() {
        // dw on "foo bar": w moves 0->4, range [0,4) exclusive (not inclusive).
        let r = operator_range(cur("foo bar", 0), 4, 'w', Operator::Delete, 1);
        assert_eq!((r.from, r.to, r.linewise), (0, 4, false));
    }

    #[test]
    fn range_inclusive_for_e_and_dollar() {
        // de on "foo bar": e moves 0->2 ('o'), inclusive -> to = 3.
        let r = operator_range(cur("foo bar", 0), 2, 'e', Operator::Delete, 1);
        assert_eq!((r.from, r.to, r.linewise), (0, 3, false));
        // d$ on "foo": $ moves 0->3 (== len), inclusive but already at end -> to stays 3.
        let r2 = operator_range(cur("foo", 0), 3, '$', Operator::Delete, 1);
        assert_eq!((r2.from, r2.to, r2.linewise), (0, 3, false));
    }

    #[test]
    fn range_cw_changes_to_end_of_word_like_ce() {
        // cw on "foo bar" from 0: special-cased to end-of-word -> through 'o' (to=3),
        // NOT to start of next word (4). This is the claude-code cw->ce rule.
        let r = operator_range(cur("foo bar", 0), 4, 'w', Operator::Change, 1);
        assert_eq!((r.from, r.to, r.linewise), (0, 3, false));
    }

    #[test]
    fn range_linewise_for_j() {
        // dj on "a\nb\nc" from offset 0: j is linewise, deletes lines 0..1 incl
        // trailing newline of line1 -> [0, 4) ("a\nb\n").
        let r = operator_range(cur("a\nb\nc", 0), 2, 'j', Operator::Delete, 1);
        assert!(r.linewise);
        assert_eq!((r.from, r.to), (0, 4));
    }

    #[test]
    fn apply_delete_sets_register_and_edits() {
        let (effect, reg, enter_insert) = apply_operator(Operator::Delete, "foo bar", 0, 4, false);
        assert_eq!(
            reg,
            Register {
                text: "foo ".into(),
                linewise: false
            }
        );
        assert_eq!(
            effect,
            VimEffect::Edit {
                text: "bar".into(),
                cursor: 0
            }
        );
        assert!(!enter_insert);
    }

    #[test]
    fn apply_yank_keeps_text_moves_cursor_to_from() {
        let (effect, reg, enter_insert) = apply_operator(Operator::Yank, "foo bar", 4, 7, false);
        assert_eq!(
            reg,
            Register {
                text: "bar".into(),
                linewise: false
            }
        );
        assert_eq!(effect, VimEffect::Move(4)); // buffer unchanged, cursor to range start
        assert!(!enter_insert);
    }

    #[test]
    fn apply_change_edits_and_requests_insert() {
        let (effect, reg, enter_insert) = apply_operator(Operator::Change, "foo bar", 0, 3, false);
        assert_eq!(
            reg,
            Register {
                text: "foo".into(),
                linewise: false
            }
        );
        assert_eq!(
            effect,
            VimEffect::Edit {
                text: " bar".into(),
                cursor: 0
            }
        );
        assert!(enter_insert);
    }

    #[test]
    fn apply_linewise_delete_tags_register_linewise() {
        // delete "a\n" (line 0) from "a\nb": register linewise, text "a\nb" -> "b".
        let (effect, reg, _) = apply_operator(Operator::Delete, "a\nb", 0, 2, true);
        assert!(reg.linewise);
        assert_eq!(reg.text, "a\n");
        assert_eq!(
            effect,
            VimEffect::Edit {
                text: "b".into(),
                cursor: 0
            }
        );
    }
}

#[cfg(test)]
mod line_op_tests {
    use super::*;
    fn cur(text: &str, off: usize) -> VimCursor<'_> {
        VimCursor { text, offset: off }
    }

    #[test]
    fn dd_deletes_current_line_register_linewise() {
        // dd on line 1 ('b') of "a\nb\nc": delete "b\n", register linewise.
        let (effect, reg, enter_insert) = line_op(Operator::Delete, cur("a\nb\nc", 2), 1);
        assert!(reg.linewise);
        assert_eq!(reg.text, "b\n");
        assert_eq!(
            effect,
            VimEffect::Edit {
                text: "a\nc".into(),
                cursor: 2
            }
        );
        assert!(!enter_insert);
    }

    #[test]
    fn dd_last_line_consumes_preceding_newline() {
        // dd on last line ('c') of "a\nb\nc": delete to EOF; preceding '\n' consumed
        // so no orphan trailing newline. Result "a\nb".
        let (effect, _reg, _) = line_op(Operator::Delete, cur("a\nb\nc", 4), 1);
        match effect {
            VimEffect::Edit { text, .. } => assert_eq!(text, "a\nb"),
            other => panic!("expected Edit, got {other:?}"),
        }
    }

    #[test]
    fn count_dd_deletes_n_lines() {
        // 2dd on "a\nb\nc\nd" from line0: delete "a\nb\n" -> "c\nd".
        let (effect, reg, _) = line_op(Operator::Delete, cur("a\nb\nc\nd", 0), 2);
        assert_eq!(reg.text, "a\nb\n");
        match effect {
            VimEffect::Edit { text, cursor } => {
                assert_eq!(text, "c\nd");
                assert_eq!(cursor, 0);
            }
            other => panic!("expected Edit, got {other:?}"),
        }
    }

    #[test]
    fn yy_yanks_keeps_buffer_cursor_to_line_start() {
        // yy on line1 of "a\nb\nc": register "b\n" linewise, buffer unchanged, cursor->line start (2).
        let (effect, reg, enter_insert) = line_op(Operator::Yank, cur("a\nb\nc", 3), 1);
        assert_eq!(
            reg,
            Register {
                text: "b\n".into(),
                linewise: true
            }
        );
        assert_eq!(effect, VimEffect::Move(2));
        assert!(!enter_insert);
    }

    #[test]
    fn cc_clears_line_enters_insert_at_line_start() {
        // cc on line1 of "ab\ncd\nef": clear "cd" -> "ab\n\nef", enter insert at line start (3).
        let (effect, _reg, enter_insert) = line_op(Operator::Change, cur("ab\ncd\nef", 4), 1);
        assert_eq!(
            effect,
            VimEffect::Edit {
                text: "ab\n\nef".into(),
                cursor: 3
            }
        );
        assert!(enter_insert);
    }

    #[test]
    fn cc_single_line_buffer_clears_to_empty() {
        let (effect, _reg, enter_insert) = line_op(Operator::Change, cur("hello", 2), 1);
        assert_eq!(
            effect,
            VimEffect::Edit {
                text: String::new(),
                cursor: 0
            }
        );
        assert!(enter_insert);
    }
}

#[cfg(test)]
mod x_tests {
    use super::*;
    fn cur(text: &str, off: usize) -> VimCursor<'_> {
        VimCursor { text, offset: off }
    }

    #[test]
    fn x_deletes_char_under_cursor() {
        // x on "hello" at 0: delete 'h' -> "ello", register "h", cursor 0.
        let (effect, reg) = delete_char_x(cur("hello", 0), 1);
        assert_eq!(
            reg,
            Register {
                text: "h".into(),
                linewise: false
            }
        );
        assert_eq!(
            effect,
            VimEffect::Edit {
                text: "ello".into(),
                cursor: 0
            }
        );
    }

    #[test]
    fn count_x_deletes_n_chars() {
        // 3x on "hello" at 0: delete "hel" -> "lo", cursor 0.
        let (effect, reg) = delete_char_x(cur("hello", 0), 3);
        assert_eq!(reg.text, "hel");
        assert_eq!(
            effect,
            VimEffect::Edit {
                text: "lo".into(),
                cursor: 0
            }
        );
    }

    #[test]
    fn x_clamps_cursor_to_last_char() {
        // x on last char of "ab" at 1: delete 'b' -> "a"; cursor clamps to 0.
        let (effect, _reg) = delete_char_x(cur("ab", 1), 1);
        assert_eq!(
            effect,
            VimEffect::Edit {
                text: "a".into(),
                cursor: 0
            }
        );
    }

    #[test]
    fn x_at_eof_is_noop() {
        let (effect, reg) = delete_char_x(cur("ab", 2), 1);
        assert_eq!(effect, VimEffect::None);
        assert_eq!(reg, Register::default()); // unchanged
    }

    #[test]
    fn count_x_overshoot_clamps_at_eof() {
        // 9x on "ab" at 0: delete both -> "", cursor 0.
        let (effect, reg) = delete_char_x(cur("ab", 0), 9);
        assert_eq!(reg.text, "ab");
        assert_eq!(
            effect,
            VimEffect::Edit {
                text: String::new(),
                cursor: 0
            }
        );
    }
}

#[cfg(test)]
mod paste_tests {
    use super::*;
    fn cur(text: &str, off: usize) -> VimCursor<'_> {
        VimCursor { text, offset: off }
    }
    fn reg_char(s: &str) -> Register {
        Register {
            text: s.into(),
            linewise: false,
        }
    }
    fn reg_line(s: &str) -> Register {
        Register {
            text: s.into(),
            linewise: true,
        }
    }

    #[test]
    fn charwise_p_inserts_after_cursor() {
        // p with register "X" on "ab" at 0: insert after 'a' -> "aXb", cursor on 'X' (1).
        let effect = paste(true, 1, &reg_char("X"), cur("ab", 0));
        assert_eq!(
            effect,
            VimEffect::Edit {
                text: "aXb".into(),
                cursor: 1
            }
        );
    }

    #[test]
    fn charwise_cap_p_inserts_before_cursor() {
        // P with register "X" on "ab" at 1: insert at cursor -> "aXb", cursor on 'X' (1).
        let effect = paste(false, 1, &reg_char("X"), cur("ab", 1));
        assert_eq!(
            effect,
            VimEffect::Edit {
                text: "aXb".into(),
                cursor: 1
            }
        );
    }

    #[test]
    fn charwise_p_repeats_count_times() {
        // 3p with register "X" on "ab" at 0: "aXXXb", cursor on last 'X' (3).
        let effect = paste(true, 3, &reg_char("X"), cur("ab", 0));
        assert_eq!(
            effect,
            VimEffect::Edit {
                text: "aXXXb".into(),
                cursor: 3
            }
        );
    }

    #[test]
    fn charwise_p_at_eof_inserts_at_cursor() {
        // p with register "X" on "ab" at 2 (EOF): insert at cursor -> "abX", cursor on 'X' (2).
        let effect = paste(true, 1, &reg_char("X"), cur("ab", 2));
        assert_eq!(
            effect,
            VimEffect::Edit {
                text: "abX".into(),
                cursor: 2
            }
        );
    }

    #[test]
    fn linewise_p_opens_line_below() {
        // p with linewise register "x\n" on "a\nb" at 0 (line0): new line below -> "a\nx\nb",
        // cursor at start of pasted line (2).
        let effect = paste(true, 1, &reg_line("x\n"), cur("a\nb", 0));
        assert_eq!(
            effect,
            VimEffect::Edit {
                text: "a\nx\nb".into(),
                cursor: 2
            }
        );
    }

    #[test]
    fn linewise_cap_p_opens_line_above() {
        // P with linewise register "x\n" on "a\nb" at 2 (line1): new line above -> "a\nx\nb",
        // cursor at start of pasted line (2).
        let effect = paste(false, 1, &reg_line("x\n"), cur("a\nb", 2));
        assert_eq!(
            effect,
            VimEffect::Edit {
                text: "a\nx\nb".into(),
                cursor: 2
            }
        );
    }

    #[test]
    fn empty_register_is_noop() {
        let effect = paste(true, 1, &Register::default(), cur("ab", 0));
        assert_eq!(effect, VimEffect::None);
    }
}

#[cfg(test)]
mod op_dispatch_tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    fn key(c: char) -> KeyEvent {
        let m = if c.is_uppercase() {
            KeyModifiers::SHIFT
        } else {
            KeyModifiers::NONE
        };
        KeyEvent::new(KeyCode::Char(c), m)
    }
    fn esc() -> KeyEvent {
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
    }
    fn normal() -> VimState {
        VimState {
            mode: VimMode::Normal,
            ..VimState::default()
        }
    }

    #[test]
    fn d_enters_operator_pending() {
        let mut s = normal();
        assert_eq!(
            handle_vim_key(&mut s, "foo bar", 0, key('d')),
            VimOutcome::Pending
        );
        assert_eq!(
            s.command,
            CommandState::Operator {
                op: Operator::Delete,
                count: 1
            }
        );
    }

    #[test]
    fn dw_deletes_word() {
        let mut s = normal();
        handle_vim_key(&mut s, "foo bar", 0, key('d'));
        let out = handle_vim_key(&mut s, "foo bar", 0, key('w'));
        assert_eq!(
            out,
            VimOutcome::Effect(VimEffect::Edit {
                text: "bar".into(),
                cursor: 0
            })
        );
        assert_eq!(s.command, CommandState::Idle);
        assert_eq!(s.register.text, "foo ");
    }

    #[test]
    fn de_deletes_to_end_of_word_inclusive() {
        let mut s = normal();
        handle_vim_key(&mut s, "foo bar", 0, key('d'));
        let out = handle_vim_key(&mut s, "foo bar", 0, key('e'));
        assert_eq!(
            out,
            VimOutcome::Effect(VimEffect::Edit {
                text: " bar".into(),
                cursor: 0
            })
        );
    }

    #[test]
    fn d_dollar_deletes_to_eol() {
        let mut s = normal();
        handle_vim_key(&mut s, "foo bar", 4, key('d'));
        let out = handle_vim_key(&mut s, "foo bar", 4, key('$'));
        assert_eq!(
            out,
            VimOutcome::Effect(VimEffect::Edit {
                text: "foo ".into(),
                cursor: 3
            })
        );
    }

    #[test]
    fn dd_deletes_line() {
        let mut s = normal();
        handle_vim_key(&mut s, "a\nb\nc", 2, key('d'));
        let out = handle_vim_key(&mut s, "a\nb\nc", 2, key('d'));
        assert_eq!(
            out,
            VimOutcome::Effect(VimEffect::Edit {
                text: "a\nc".into(),
                cursor: 2
            })
        );
        assert!(s.register.linewise);
    }

    #[test]
    fn cc_clears_line_and_enters_insert() {
        let mut s = normal();
        handle_vim_key(&mut s, "ab\ncd", 0, key('c'));
        let out = handle_vim_key(&mut s, "ab\ncd", 0, key('c'));
        assert_eq!(
            out,
            VimOutcome::Effect(VimEffect::Edit {
                text: "\ncd".into(),
                cursor: 0
            })
        );
        assert_eq!(s.mode, VimMode::Insert);
    }

    #[test]
    fn cw_changes_to_end_of_word_and_enters_insert() {
        let mut s = normal();
        handle_vim_key(&mut s, "foo bar", 0, key('c'));
        let out = handle_vim_key(&mut s, "foo bar", 0, key('w'));
        assert_eq!(
            out,
            VimOutcome::Effect(VimEffect::Edit {
                text: " bar".into(),
                cursor: 0
            })
        );
        assert_eq!(s.mode, VimMode::Insert);
    }

    #[test]
    fn yy_yanks_line_keeps_buffer() {
        let mut s = normal();
        handle_vim_key(&mut s, "a\nb\nc", 2, key('y'));
        let out = handle_vim_key(&mut s, "a\nb\nc", 2, key('y'));
        assert_eq!(out, VimOutcome::Effect(VimEffect::Move(2)));
        assert_eq!(
            s.register,
            Register {
                text: "b\n".into(),
                linewise: true
            }
        );
    }

    #[test]
    fn count_dw_multiplies() {
        // 3dw on "a b c d e" from 0: delete 3 words -> "d e".
        let mut s = normal();
        handle_vim_key(&mut s, "a b c d e", 0, key('3'));
        handle_vim_key(&mut s, "a b c d e", 0, key('d'));
        let out = handle_vim_key(&mut s, "a b c d e", 0, key('w'));
        assert_eq!(
            out,
            VimOutcome::Effect(VimEffect::Edit {
                text: "d e".into(),
                cursor: 0
            })
        );
    }

    #[test]
    fn d_count_w_inner_count_multiplies() {
        // d3w on "a b c d e" from 0: same as 3dw -> "d e".
        let mut s = normal();
        handle_vim_key(&mut s, "a b c d e", 0, key('d'));
        handle_vim_key(&mut s, "a b c d e", 0, key('3'));
        let out = handle_vim_key(&mut s, "a b c d e", 0, key('w'));
        assert_eq!(
            out,
            VimOutcome::Effect(VimEffect::Edit {
                text: "d e".into(),
                cursor: 0
            })
        );
    }

    #[test]
    fn count_yy_yanks_n_lines() {
        // 2yy on "a\nb\nc" from 0: register "a\nb\n".
        let mut s = normal();
        handle_vim_key(&mut s, "a\nb\nc", 0, key('2'));
        handle_vim_key(&mut s, "a\nb\nc", 0, key('y'));
        let out = handle_vim_key(&mut s, "a\nb\nc", 0, key('y'));
        assert_eq!(out, VimOutcome::Effect(VimEffect::Move(0)));
        assert_eq!(s.register.text, "a\nb\n");
    }

    #[test]
    fn df_char_deletes_through_find() {
        // df_c on "abcde" from 0: find 'c' at 2, inclusive -> delete "abc" -> "de".
        let mut s = normal();
        handle_vim_key(&mut s, "abcde", 0, key('d'));
        handle_vim_key(&mut s, "abcde", 0, key('f'));
        assert_eq!(
            s.command,
            CommandState::OperatorFind {
                op: Operator::Delete,
                count: 1,
                kind: FindKind::F
            }
        );
        let out = handle_vim_key(&mut s, "abcde", 0, key('c'));
        assert_eq!(
            out,
            VimOutcome::Effect(VimEffect::Edit {
                text: "de".into(),
                cursor: 0
            })
        );
    }

    #[test]
    fn d_cap_g_deletes_to_last_line() {
        // dG on "a\nb\nc" from 0: linewise delete all -> "".
        let mut s = normal();
        handle_vim_key(&mut s, "a\nb\nc", 0, key('d'));
        let out = handle_vim_key(&mut s, "a\nb\nc", 0, key('G'));
        match out {
            VimOutcome::Effect(VimEffect::Edit { text, .. }) => assert_eq!(text, ""),
            other => panic!("expected Edit, got {other:?}"),
        }
    }

    #[test]
    fn dgg_deletes_to_first_line() {
        // dgg on "a\nb\nc" from offset 4 (line2 'c'): linewise delete lines 0..2 -> "".
        let mut s = normal();
        handle_vim_key(&mut s, "a\nb\nc", 4, key('d'));
        assert_eq!(
            handle_vim_key(&mut s, "a\nb\nc", 4, key('g')),
            VimOutcome::Pending
        );
        assert_eq!(
            s.command,
            CommandState::OperatorG {
                op: Operator::Delete,
                count: 1
            }
        );
        let out = handle_vim_key(&mut s, "a\nb\nc", 4, key('g'));
        match out {
            VimOutcome::Effect(VimEffect::Edit { text, .. }) => assert_eq!(text, ""),
            other => panic!("expected Edit, got {other:?}"),
        }
    }

    #[test]
    fn x_deletes_char() {
        let mut s = normal();
        let out = handle_vim_key(&mut s, "hello", 0, key('x'));
        assert_eq!(
            out,
            VimOutcome::Effect(VimEffect::Edit {
                text: "ello".into(),
                cursor: 0
            })
        );
        assert_eq!(s.register.text, "h");
    }

    #[test]
    fn p_pastes_after() {
        let mut s = normal();
        s.register = Register {
            text: "X".into(),
            linewise: false,
        };
        let out = handle_vim_key(&mut s, "ab", 0, key('p'));
        assert_eq!(
            out,
            VimOutcome::Effect(VimEffect::Edit {
                text: "aXb".into(),
                cursor: 1
            })
        );
    }

    #[test]
    fn esc_cancels_operator_pending() {
        let mut s = normal();
        handle_vim_key(&mut s, "foo", 0, key('d'));
        let out = handle_vim_key(&mut s, "foo", 0, esc());
        assert_eq!(out, VimOutcome::Effect(VimEffect::None));
        assert_eq!(s.command, CommandState::Idle);
    }

    #[test]
    fn operator_motion_noop_when_motion_does_not_move() {
        // dh at offset 0: h is a no-op -> operator no-op, register untouched.
        let mut s = normal();
        handle_vim_key(&mut s, "abc", 0, key('d'));
        let out = handle_vim_key(&mut s, "abc", 0, key('h'));
        assert_eq!(out, VimOutcome::Effect(VimEffect::None));
        assert_eq!(s.register, Register::default());
        assert_eq!(s.command, CommandState::Idle);
    }
}

#[cfg(test)]
mod visual_tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    fn key(c: char) -> KeyEvent {
        let m = if c.is_uppercase() {
            KeyModifiers::SHIFT
        } else {
            KeyModifiers::NONE
        };
        KeyEvent::new(KeyCode::Char(c), m)
    }
    fn esc() -> KeyEvent {
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
    }
    fn normal() -> VimState {
        VimState {
            mode: VimMode::Normal,
            ..VimState::default()
        }
    }

    #[test]
    fn v_enters_visual_sets_anchor() {
        let mut s = normal();
        let out = handle_vim_key(&mut s, "hello", 2, key('v'));
        assert_eq!(s.mode, VimMode::Visual);
        assert_eq!(
            s.visual,
            Some(VisualState {
                anchor: 2,
                linewise: false
            })
        );
        assert_eq!(out, VimOutcome::Effect(VimEffect::None));
    }

    #[test]
    fn cap_v_enters_visual_linewise() {
        let mut s = normal();
        handle_vim_key(&mut s, "a\nb", 0, key('V'));
        assert_eq!(s.mode, VimMode::Visual);
        assert_eq!(
            s.visual,
            Some(VisualState {
                anchor: 0,
                linewise: true
            })
        );
    }

    #[test]
    fn visual_motion_moves_selection_end() {
        // v then l l on "hello" from 0: cursor moves to 2 (anchor stays 0).
        let mut s = normal();
        handle_vim_key(&mut s, "hello", 0, key('v'));
        let out = handle_vim_key(&mut s, "hello", 0, key('l'));
        assert_eq!(out, VimOutcome::Effect(VimEffect::Move(1)));
        let out2 = handle_vim_key(&mut s, "hello", 1, key('l'));
        assert_eq!(out2, VimOutcome::Effect(VimEffect::Move(2)));
        assert_eq!(
            s.visual,
            Some(VisualState {
                anchor: 0,
                linewise: false
            })
        );
    }

    #[test]
    fn visual_d_deletes_inclusive_selection() {
        // v (anchor 0) then d on "hello" with cursor at 2: charwise inclusive ->
        // delete [0,3) "hel" -> "lo", back to Normal.
        let mut s = normal();
        handle_vim_key(&mut s, "hello", 0, key('v')); // anchor 0
        let out = handle_vim_key(&mut s, "hello", 2, key('d'));
        assert_eq!(
            out,
            VimOutcome::Effect(VimEffect::Edit {
                text: "lo".into(),
                cursor: 0
            })
        );
        assert_eq!(s.mode, VimMode::Normal);
        assert!(s.visual.is_none());
        assert_eq!(s.register.text, "hel");
    }

    #[test]
    fn visual_y_yanks_selection_keeps_buffer() {
        let mut s = normal();
        handle_vim_key(&mut s, "hello", 0, key('v'));
        let out = handle_vim_key(&mut s, "hello", 2, key('y'));
        // yank moves cursor to range start, buffer unchanged.
        assert_eq!(out, VimOutcome::Effect(VimEffect::Move(0)));
        assert_eq!(
            s.register,
            Register {
                text: "hel".into(),
                linewise: false
            }
        );
        assert_eq!(s.mode, VimMode::Normal);
    }

    #[test]
    fn visual_c_deletes_and_enters_insert() {
        let mut s = normal();
        handle_vim_key(&mut s, "hello", 0, key('v'));
        let out = handle_vim_key(&mut s, "hello", 2, key('c'));
        assert_eq!(
            out,
            VimOutcome::Effect(VimEffect::Edit {
                text: "lo".into(),
                cursor: 0
            })
        );
        assert_eq!(s.mode, VimMode::Insert);
        assert!(s.visual.is_none());
    }

    #[test]
    fn visual_line_d_deletes_whole_lines() {
        // V then cursor on line1 ('b' @2) then d on "a\nb\nc":
        // linewise delete lines 0..1 -> "c".
        let mut s = normal();
        handle_vim_key(&mut s, "a\nb\nc", 0, key('V')); // anchor 0, linewise
        let out = handle_vim_key(&mut s, "a\nb\nc", 2, key('d')); // cursor on line1
        assert_eq!(
            out,
            VimOutcome::Effect(VimEffect::Edit {
                text: "c".into(),
                cursor: 0
            })
        );
        assert!(s.register.linewise);
        assert_eq!(s.register.text, "a\nb\n");
        assert_eq!(s.mode, VimMode::Normal);
    }

    #[test]
    fn visual_esc_returns_to_normal() {
        let mut s = normal();
        handle_vim_key(&mut s, "hello", 2, key('v'));
        let out = handle_vim_key(&mut s, "hello", 4, esc());
        assert_eq!(out, VimOutcome::Effect(VimEffect::Move(4))); // 4 < len 5 -> no clamp
        assert_eq!(s.mode, VimMode::Normal);
        assert!(s.visual.is_none());
    }

    #[test]
    fn visual_count_motion_extends() {
        // v then 2l on "hello" from 0: cursor -> 2.
        let mut s = normal();
        handle_vim_key(&mut s, "hello", 0, key('v'));
        assert_eq!(
            handle_vim_key(&mut s, "hello", 0, key('2')),
            VimOutcome::Pending
        );
        let out = handle_vim_key(&mut s, "hello", 0, key('l'));
        assert_eq!(out, VimOutcome::Effect(VimEffect::Move(2)));
    }
}

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
//! ## Telemetry (M7-08)
//! M7-08 emits ZERO telemetry events. `tengu_tui_vim_mode_entered`
//! (aggregated, NOT per-keystroke) is a CANDIDATE deferred to the M7-16
//! telemetry audit — see M7 design §2.7. Do not register it here without an
//! emit site (M6 discipline: every registered name has a real emit site).

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
    /// Scaffold for M7-09 operators; always `None` in M7-08.
    pub pending_operator: Option<Operator>,
    /// Scaffold for M7-09 registers; never written in M7-08.
    pub register: Option<String>,
    /// Last f/F/t/T (kind, char) for ';'/',' — scaffold; M7-08 records it
    /// but does not implement ';'/',' (those are M7-09 polish).
    pub last_find: Option<(FindKind, char)>,
}

impl Default for VimState {
    /// claude-code `createInitialVimState()`: start in INSERT.
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
        // M7-08 ships 0 new telemetry events; baseline locked at 326 (M7-16
        // audits the real M7 total). tengu_tui_vim_mode_entered is DEFERRED to
        // M7-16. This guard fails if M7-08 accidentally registers a new event.
        assert_eq!(lingxi_telemetry::tengu::ALL_EVENT_NAMES.len(), 326);
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

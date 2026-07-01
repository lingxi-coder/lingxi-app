//! Vim editing layer for the composer: a `Normal`/`Insert`/`Visual` mode state
//! machine that maps command-mode keys onto the [`Composer`]'s editing
//! primitives.
//!
//! A documented parity SUBSET (like the iocraft composer): motions
//! (`h/j/k/l/w/e/b/0/$`), edits (`x/D/dd`), insert-entry (`i/a/A/I/o`), count
//! prefixes (`3w`, `5x`), a `Visual` mode (`v` + motions + `d/x/y`), and
//! `y`/`p` yank/paste through a single register. Registers-by-name, `.`-repeat,
//! and text objects are out of scope. `Insert` mode is a thin pass-through —
//! the app's normal composer handling does the typing; only `Esc` is
//! intercepted.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::composer::Composer;

/// The vim editing mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VimMode {
    /// Command mode: keys are motions/edits, not text.
    Normal,
    /// Insert mode: keys type text (handled by the app's composer path).
    Insert,
    /// Visual mode: motions extend a selection; `d/x/y` operate on it.
    Visual,
}

/// Vim editing state: mode + a one-key pending operator (`d` of `dd`) + an
/// accumulating count prefix + the yank/delete register.
#[derive(Debug, Clone, Default)]
pub struct VimState {
    /// Current mode.
    pub mode: Mode,
    /// A pending operator char (currently only `'d'` for `dd`).
    pending: Option<char>,
    /// Accumulating count prefix digits (e.g. `"3"` for `3w`).
    count: String,
    /// The single yank/delete register.
    register: String,
}

/// Alias so `VimState { mode: ... }` reads naturally; the real enum is
/// [`VimMode`].
pub type Mode = VimMode;

impl Default for VimMode {
    fn default() -> Self {
        Self::Insert
    }
}

impl VimState {
    /// Enable vim starting in `Insert` mode (so behavior is unchanged until the
    /// user presses `Esc`).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A short mode label for the status line.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self.mode {
            VimMode::Normal => "NORMAL",
            VimMode::Insert => "INSERT",
            VimMode::Visual => "VISUAL",
        }
    }

    /// Take the pending count (default 1), clearing it.
    fn take_count(&mut self) -> usize {
        let n = self.count.parse::<usize>().unwrap_or(1).max(1);
        self.count.clear();
        n
    }
}

/// What the app should do after the vim layer sees a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VimOutcome {
    /// The vim layer fully handled the key; the app does nothing else.
    Consumed,
    /// The app should run its normal composer handling (Insert-mode typing,
    /// Ctrl-chords, etc.).
    Passthrough,
    /// The user submitted (Normal-mode `Enter`); the app should send the buffer.
    Submit,
}

/// Route `key` through the vim state machine, mutating `vim` + `composer`.
#[must_use]
pub fn handle_key(vim: &mut VimState, composer: &mut Composer, key: KeyEvent) -> VimOutcome {
    // Ctrl-chords are never vim motions — let the app handle Ctrl-C, etc.
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return VimOutcome::Passthrough;
    }
    match vim.mode {
        VimMode::Insert => match key.code {
            KeyCode::Esc => {
                vim.mode = VimMode::Normal;
                composer.move_left();
                VimOutcome::Consumed
            }
            _ => VimOutcome::Passthrough,
        },
        VimMode::Normal => handle_normal(vim, composer, key.code),
        VimMode::Visual => handle_visual(vim, composer, key.code),
    }
}

/// Accumulate a count digit; returns `true` when `code` was consumed as a digit.
fn take_digit(vim: &mut VimState, code: KeyCode) -> bool {
    if let KeyCode::Char(c) = code {
        if c.is_ascii_digit() && !(c == '0' && vim.count.is_empty()) {
            vim.count.push(c);
            return true;
        }
    }
    false
}

/// Apply a motion `code` to `composer`; returns `true` when it was a motion.
fn motion(composer: &mut Composer, code: KeyCode) -> bool {
    match code {
        KeyCode::Char('h') | KeyCode::Left => composer.move_left(),
        KeyCode::Char('l') | KeyCode::Right => composer.move_right(),
        KeyCode::Char('k') | KeyCode::Up => composer.cursor_line_up(),
        KeyCode::Char('j') | KeyCode::Down => composer.cursor_line_down(),
        KeyCode::Char('w') => composer.next_word_start(),
        KeyCode::Char('e') => composer.word_end(),
        KeyCode::Char('b') => composer.move_word_left(),
        KeyCode::Char('0') | KeyCode::Home => composer.home(),
        KeyCode::Char('$') | KeyCode::End => composer.end(),
        _ => return false,
    }
    true
}

fn handle_normal(vim: &mut VimState, composer: &mut Composer, code: KeyCode) -> VimOutcome {
    if take_digit(vim, code) {
        return VimOutcome::Consumed;
    }
    // Resolve a pending `d` operator: `dd` deletes count lines.
    if vim.pending == Some('d') {
        vim.pending = None;
        if code == KeyCode::Char('d') {
            for _ in 0..vim.take_count() {
                composer.delete_line();
            }
            return VimOutcome::Consumed;
        }
    }
    let count = vim.take_count();
    // Count-repeatable motions.
    if is_motion(code) {
        for _ in 0..count {
            motion(composer, code);
        }
        return VimOutcome::Consumed;
    }
    match code {
        KeyCode::Char('x') => {
            for _ in 0..count {
                composer.delete();
            }
        }
        KeyCode::Char('D') => composer.kill_to_line_end(),
        KeyCode::Char('d') => vim.pending = Some('d'),
        KeyCode::Char('p') => {
            let reg = vim.register.clone();
            if !reg.is_empty() {
                composer.move_right();
                composer.insert_str(&reg);
            }
        }
        KeyCode::Char('v') => {
            composer.start_selection();
            vim.mode = VimMode::Visual;
        }
        KeyCode::Char('i') => vim.mode = VimMode::Insert,
        KeyCode::Char('a') => {
            composer.move_right();
            vim.mode = VimMode::Insert;
        }
        KeyCode::Char('A') => {
            composer.end();
            vim.mode = VimMode::Insert;
        }
        KeyCode::Char('I') => {
            composer.home();
            vim.mode = VimMode::Insert;
        }
        KeyCode::Char('o') => {
            composer.end();
            composer.insert_newline();
            vim.mode = VimMode::Insert;
        }
        KeyCode::Enter => return VimOutcome::Submit,
        // Normal mode swallows everything else (including Esc → stays Normal).
        _ => {}
    }
    VimOutcome::Consumed
}

fn handle_visual(vim: &mut VimState, composer: &mut Composer, code: KeyCode) -> VimOutcome {
    if take_digit(vim, code) {
        return VimOutcome::Consumed;
    }
    let count = vim.take_count();
    if is_motion(code) {
        for _ in 0..count {
            motion(composer, code);
        }
        return VimOutcome::Consumed;
    }
    match code {
        KeyCode::Char('d') | KeyCode::Char('x') => {
            if let Some(removed) = composer.delete_selection() {
                vim.register = removed;
            }
            vim.mode = VimMode::Normal;
        }
        KeyCode::Char('y') => {
            if let Some(sel) = composer.selected_text() {
                vim.register = sel;
            }
            composer.clear_selection();
            vim.mode = VimMode::Normal;
        }
        KeyCode::Esc => {
            composer.clear_selection();
            vim.mode = VimMode::Normal;
        }
        _ => {}
    }
    VimOutcome::Consumed
}

fn is_motion(code: KeyCode) -> bool {
    matches!(
        code,
        KeyCode::Char('h' | 'l' | 'k' | 'j' | 'w' | 'e' | 'b' | '0' | '$')
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::Up
            | KeyCode::Down
            | KeyCode::Home
            | KeyCode::End
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn typed(s: &str) -> Composer {
        let mut c = Composer::default();
        for ch in s.chars() {
            c.insert(ch);
        }
        c
    }

    fn normal() -> VimState {
        let mut v = VimState::new();
        v.mode = VimMode::Normal;
        v
    }

    /// Send a key, discarding the outcome (for setup/motion steps that don't
    /// assert on the returned [`VimOutcome`]).
    fn send(vim: &mut VimState, c: &mut Composer, code: KeyCode) {
        let _ = handle_key(vim, c, press(code));
    }

    #[test]
    fn esc_enters_normal_and_i_returns_to_insert() {
        let mut vim = VimState::new();
        let mut c = typed("abc");
        assert_eq!(vim.mode, VimMode::Insert);
        assert_eq!(handle_key(&mut vim, &mut c, press(KeyCode::Esc)), VimOutcome::Consumed);
        assert_eq!(vim.mode, VimMode::Normal);
        assert_eq!(handle_key(&mut vim, &mut c, press(KeyCode::Char('i'))), VimOutcome::Consumed);
        assert_eq!(vim.mode, VimMode::Insert);
    }

    #[test]
    fn insert_mode_passes_typing_through() {
        let mut vim = VimState::new();
        let mut c = typed("");
        assert_eq!(
            handle_key(&mut vim, &mut c, press(KeyCode::Char('x'))),
            VimOutcome::Passthrough
        );
    }

    #[test]
    fn word_motions_use_vim_semantics() {
        let mut vim = normal();
        let mut c = typed("hello world");
        c.home();
        // `w` → start of "world" (vim semantics, not end-of-word).
        send(&mut vim, &mut c, KeyCode::Char('w'));
        assert_eq!(c.cursor_row_col(), (0, 6));
    }

    #[test]
    fn count_prefix_repeats_motion_and_delete() {
        let mut vim = normal();
        let mut c = typed("abcdef");
        c.home();
        // `3l` moves right 3.
        send(&mut vim, &mut c, KeyCode::Char('3'));
        send(&mut vim, &mut c, KeyCode::Char('l'));
        assert_eq!(c.cursor_row_col(), (0, 3));
        // `2x` deletes 2 chars.
        c.home();
        send(&mut vim, &mut c, KeyCode::Char('2'));
        send(&mut vim, &mut c, KeyCode::Char('x'));
        assert_eq!(c.text(), "cdef");
    }

    #[test]
    fn dd_deletes_line() {
        let mut c = typed("one");
        c.insert_newline();
        for ch in "two".chars() {
            c.insert(ch);
        }
        let mut vim = normal();
        send(&mut vim, &mut c, KeyCode::Char('d'));
        send(&mut vim, &mut c, KeyCode::Char('d'));
        assert_eq!(c.text(), "one\n");
    }

    #[test]
    fn visual_select_delete_and_paste() {
        let mut vim = normal();
        let mut c = typed("hello world");
        c.home();
        // v, then 4× l selects "hello" (inclusive of the cursor cell).
        send(&mut vim, &mut c, KeyCode::Char('v'));
        assert_eq!(vim.mode, VimMode::Visual);
        for _ in 0..4 {
            send(&mut vim, &mut c, KeyCode::Char('l'));
        }
        send(&mut vim, &mut c, KeyCode::Char('d'));
        assert_eq!(vim.mode, VimMode::Normal);
        assert_eq!(c.text(), " world");
        // Paste the deleted "hello" back.
        c.home();
        send(&mut vim, &mut c, KeyCode::Char('p'));
        assert!(c.text().contains("hello"));
    }

    #[test]
    fn visual_yank_keeps_text_and_sets_register() {
        let mut vim = normal();
        let mut c = typed("abcde");
        c.home();
        send(&mut vim, &mut c, KeyCode::Char('v'));
        send(&mut vim, &mut c, KeyCode::Char('l'));
        send(&mut vim, &mut c, KeyCode::Char('y'));
        assert_eq!(vim.mode, VimMode::Normal);
        assert_eq!(c.text(), "abcde"); // yank does not delete
        assert!(!c.has_selection());
    }

    #[test]
    fn normal_enter_signals_submit() {
        let mut vim = normal();
        let mut c = typed("hi");
        assert_eq!(handle_key(&mut vim, &mut c, press(KeyCode::Enter)), VimOutcome::Submit);
    }

    #[test]
    fn ctrl_chords_pass_through_in_both_modes() {
        let mut vim = VimState::new();
        let mut c = typed("");
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(handle_key(&mut vim, &mut c, ctrl_c), VimOutcome::Passthrough);
        vim.mode = VimMode::Normal;
        assert_eq!(handle_key(&mut vim, &mut c, ctrl_c), VimOutcome::Passthrough);
    }
}

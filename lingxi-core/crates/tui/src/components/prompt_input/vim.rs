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

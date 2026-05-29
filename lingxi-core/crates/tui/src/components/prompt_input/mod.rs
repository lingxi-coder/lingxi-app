//! `PromptInput` — the content-driven multi-line bottom zone.
//!
//! M6-02 shipped a single-line editor; M7-06 refactors this file into the
//! `prompt_input/` submodule (`mod.rs` = editor core, `footer.rs` = footer
//! surface) and adds multi-line editing. The single-line behaviour is
//! preserved exactly when the buffer has no `\n`.
//!
//! M6-02 single-line editor supported:
//! - `InsertChar(char)` — any printable Unicode char
//! - `Backspace`
//! - `MoveCursor(CursorMove)` — `Left | Right | Home | End`
//! - `HistoryStep(i8)` — `-1` older, `+1` newer (wired in `app::dispatch`)
//! - `Submit` via parent (Enter)
//!
//! UTF-8 boundary safety: the cursor is a *byte* index. Inserts/deletes
//! and moves always land at a `char` boundary; out-of-bounds requests
//! saturate.

use iocraft::prelude::*;

/// Cursor movement primitive for the line editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorMove {
    /// Move one char left (saturating at 0).
    Left,
    /// Move one char right (saturating at end).
    Right,
    /// Jump to position 0.
    Home,
    /// Jump to end (`text.len()`).
    End,
}

/// Insert `ch` at byte index `cursor`, returning `(new_text, new_cursor)`.
#[must_use]
pub fn apply_insert(text: &str, cursor: usize, ch: char) -> (String, usize) {
    let cursor = clamp_to_char_boundary(text, cursor);
    let mut out = String::with_capacity(text.len() + ch.len_utf8());
    out.push_str(&text[..cursor]);
    out.push(ch);
    out.push_str(&text[cursor..]);
    let new_cursor = cursor + ch.len_utf8();
    (out, new_cursor)
}

/// Delete the char immediately before `cursor` (saturating at 0).
#[must_use]
pub fn apply_backspace(text: &str, cursor: usize) -> (String, usize) {
    let cursor = clamp_to_char_boundary(text, cursor);
    if cursor == 0 {
        return (text.to_string(), 0);
    }
    let prev = text[..cursor].char_indices().last().map_or(0, |(i, _)| i);
    let mut out = String::with_capacity(text.len());
    out.push_str(&text[..prev]);
    out.push_str(&text[cursor..]);
    (out, prev)
}

/// Compute the new cursor byte index for a `CursorMove`.
#[must_use]
pub fn apply_move(text: &str, cursor: usize, m: CursorMove) -> usize {
    match m {
        CursorMove::Home => 0,
        CursorMove::End => text.len(),
        CursorMove::Left => {
            if cursor == 0 {
                0
            } else {
                text[..cursor].char_indices().last().map_or(0, |(i, _)| i)
            }
        }
        CursorMove::Right => {
            if cursor >= text.len() {
                text.len()
            } else {
                let rest = &text[cursor..];
                let ch_len = rest.chars().next().map_or(0, char::len_utf8);
                cursor + ch_len
            }
        }
    }
}

fn clamp_to_char_boundary(text: &str, cursor: usize) -> usize {
    if cursor > text.len() {
        return text.len();
    }
    if text.is_char_boundary(cursor) {
        cursor
    } else {
        let mut c = cursor;
        while c > 0 && !text.is_char_boundary(c) {
            c -= 1;
        }
        c
    }
}

/// Byte indices at which each logical line begins. Always non-empty:
/// `line_starts("")` is `[0]`. A trailing `\n` opens an empty final line.
#[must_use]
pub fn line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0usize];
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i + 1);
        }
    }
    starts
}

/// Props for `PromptInput`.
#[derive(Default, Props)]
pub struct PromptInputProps {
    /// Current text in the prompt buffer.
    pub text: String,
    /// Byte-index cursor position (unused visually in M6-02 — wraps in M7).
    pub cursor: usize,
}

/// Render the prompt line. M6-02 emits a single row with `"> "` marker;
/// multi-line wrapping arrives in M7.
#[component]
pub fn PromptInput(props: &PromptInputProps) -> impl Into<AnyElement<'static>> {
    let _ = props.cursor; // cursor visualisation is a M7 enhancement
    let display = format!("> {}", props.text);
    element! {
        View(flex_direction: FlexDirection::Row, height: 1) {
            Text(content: display)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_starts_single_line() {
        assert_eq!(line_starts("hello"), vec![0]);
        assert_eq!(line_starts(""), vec![0]);
    }

    #[test]
    fn line_starts_multi_line() {
        // "ab\ncd\ne" — line starts at byte 0, 3, 6.
        assert_eq!(line_starts("ab\ncd\ne"), vec![0, 3, 6]);
    }

    #[test]
    fn line_starts_trailing_newline() {
        // A trailing "\n" opens an empty final line.
        assert_eq!(line_starts("ab\n"), vec![0, 3]);
    }

    #[test]
    fn insert_at_end() {
        let (t, c) = apply_insert("hi", 2, '!');
        assert_eq!(t, "hi!");
        assert_eq!(c, 3);
    }

    #[test]
    fn insert_at_middle() {
        let (t, c) = apply_insert("ac", 1, 'b');
        assert_eq!(t, "abc");
        assert_eq!(c, 2);
    }

    #[test]
    fn backspace_at_zero_is_noop() {
        let (t, c) = apply_backspace("hi", 0);
        assert_eq!(t, "hi");
        assert_eq!(c, 0);
    }

    #[test]
    fn backspace_removes_prev_char() {
        let (t, c) = apply_backspace("hi", 2);
        assert_eq!(t, "h");
        assert_eq!(c, 1);
    }

    #[test]
    fn move_home_end() {
        assert_eq!(apply_move("hello", 3, CursorMove::Home), 0);
        assert_eq!(apply_move("hello", 3, CursorMove::End), 5);
    }

    #[test]
    fn move_left_right_utf8() {
        // "héllo" — 'é' is 2 bytes.
        let text = "héllo";
        let c = apply_move(text, 0, CursorMove::Right);
        assert_eq!(c, 1);
        let c = apply_move(text, 1, CursorMove::Right);
        assert_eq!(c, 3); // skipped 'é' as a whole.
        let c = apply_move(text, 3, CursorMove::Left);
        assert_eq!(c, 1);
    }
}

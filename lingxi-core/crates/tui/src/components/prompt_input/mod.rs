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
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub mod footer;
pub use footer::{FooterMode, PromptInputFooter, PromptInputFooterProps};

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

/// Map a byte `cursor` into `(line_index, grapheme_column)`.
/// Column counts grapheme clusters from the line start (not bytes).
#[must_use]
pub fn cursor_line_col(text: &str, cursor: usize) -> (usize, usize) {
    let cursor = clamp_to_char_boundary(text, cursor);
    let starts = line_starts(text);
    // Largest line start <= cursor.
    let line = starts.iter().rposition(|&s| s <= cursor).unwrap_or(0);
    let line_start = starts[line];
    let col = text[line_start..cursor].graphemes(true).count();
    (line, col)
}

/// Insert a `\n` at byte index `cursor`, returning `(new_text, new_cursor)`.
/// Mirrors `apply_insert` but for the newline char.
#[must_use]
pub fn apply_newline(text: &str, cursor: usize) -> (String, usize) {
    apply_insert(text, cursor, '\n')
}

/// Number of terminal rows the buffer occupies at the given total column
/// `width`, accounting for the 2-column `"> "` marker and soft-wrapping each
/// logical line. Always at least 1 (an empty buffer shows one row).
#[must_use]
pub fn visual_row_count(text: &str, width: usize) -> usize {
    let usable = width.saturating_sub(2).max(1);
    let starts = line_starts(text);
    let mut rows = 0usize;
    for (i, &start) in starts.iter().enumerate() {
        let end = starts.get(i + 1).map_or(text.len(), |&s| s - 1); // drop the '\n'
        let line = &text[start..end];
        let w = UnicodeWidthStr::width(line);
        // ceil(w / usable), but an empty line is still 1 row.
        rows += if w == 0 { 1 } else { w.div_ceil(usable) };
    }
    rows.max(1)
}

/// Move the cursor `delta` logical lines (`-1` up, `+1` down), preserving the
/// grapheme column (clamped to the destination line). Returns the new byte
/// cursor. At the top/bottom edge the cursor stays on its current line.
#[must_use]
#[allow(
    clippy::cast_possible_wrap,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
pub fn apply_move_vertical(text: &str, cursor: usize, delta: i32) -> usize {
    let starts = line_starts(text);
    let (line, col) = cursor_line_col(text, cursor);
    let target_line = (line as i32 + delta).clamp(0, starts.len() as i32 - 1) as usize;
    let line_start = starts[target_line];
    // Destination line end drops the trailing '\n'.
    let line_end = starts.get(target_line + 1).map_or(text.len(), |&s| s - 1);
    // Walk `col` graphemes into the destination line, clamping at its end.
    let mut byte = line_start;
    let dest = &text[line_start..line_end];
    for (i, (off, g)) in dest.grapheme_indices(true).enumerate() {
        if i == col {
            byte = line_start + off;
            break;
        }
        byte = line_start + off + g.len();
    }
    byte
}

/// Props for `PromptInput`.
#[derive(Default, Props)]
pub struct PromptInputProps {
    /// Current text in the prompt buffer (may contain `\n`).
    pub text: String,
    /// Byte-index cursor position (always at a char boundary).
    pub cursor: usize,
    /// Total terminal column width (drives wrap + height). 0 → treat as 80.
    pub width: usize,
}

/// Render the prompt zone. M7-06 emits one `Text` row per logical line: the
/// first carries the `"> "` marker, the rest are indented 2 columns to align
/// under it. The outer `View`'s height grows with [`visual_row_count`].
#[component]
#[allow(clippy::cast_possible_truncation)]
pub fn PromptInput(props: &PromptInputProps) -> impl Into<AnyElement<'static>> {
    let _ = props.cursor; // cursor glyph rendering remains an M7-08 (vim) enhancement
    let width = if props.width == 0 { 80 } else { props.width };
    let starts = line_starts(&props.text);
    let height = visual_row_count(&props.text, width);
    // One Text per logical line; first line carries the "> " marker, the rest
    // are indented by 2 columns to align under it.
    let lines: Vec<(usize, String)> = starts
        .iter()
        .enumerate()
        .map(|(i, &start)| {
            let end = starts.get(i + 1).map_or(props.text.len(), |&s| s - 1);
            (i, props.text[start..end].to_string())
        })
        .collect();
    element! {
        View(flex_direction: FlexDirection::Column, height: height as u16) {
            #(lines.into_iter().map(|(i, content)| {
                let display = if i == 0 {
                    format!("> {content}")
                } else {
                    format!("  {content}")
                };
                element! {
                    View(flex_direction: FlexDirection::Row) {
                        Text(content: display)
                    }
                }
            }))
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
    fn cursor_line_col_basics() {
        // "ab\ncd" — byte 0..2 on line 0; byte 3.. on line 1.
        assert_eq!(cursor_line_col("ab\ncd", 0), (0, 0));
        assert_eq!(cursor_line_col("ab\ncd", 2), (0, 2)); // end of line 0
        assert_eq!(cursor_line_col("ab\ncd", 3), (1, 0)); // start of line 1
        assert_eq!(cursor_line_col("ab\ncd", 5), (1, 2)); // end of line 1
    }

    #[test]
    fn cursor_line_col_grapheme_column() {
        // "é" is 2 bytes but column 1. "éb\nx": byte 3 is after "éb" → col 2.
        assert_eq!(cursor_line_col("éb\nx", 3), (0, 2));
    }

    #[test]
    fn newline_at_end() {
        let (t, c) = apply_newline("hi", 2);
        assert_eq!(t, "hi\n");
        assert_eq!(c, 3);
    }

    #[test]
    fn newline_in_middle_splits_line() {
        let (t, c) = apply_newline("abcd", 2);
        assert_eq!(t, "ab\ncd");
        assert_eq!(c, 3); // cursor after the inserted '\n'
    }

    #[test]
    fn newline_respects_char_boundary() {
        // "é" is 2 bytes; a mid-codepoint cursor saturates left to byte 0.
        let (t, c) = apply_newline("é", 1);
        assert_eq!(t, "\né");
        assert_eq!(c, 1);
    }

    #[test]
    fn visual_rows_single_short_line() {
        // "hi" with the "> " marker, width 80 → 1 row.
        assert_eq!(visual_row_count("hi", 80), 1);
        assert_eq!(visual_row_count("", 80), 1); // empty buffer still 1 row
    }

    #[test]
    fn visual_rows_three_logical_lines() {
        assert_eq!(visual_row_count("a\nb\nc", 80), 3);
    }

    #[test]
    fn visual_rows_wraps_long_line() {
        // 10 graphemes, usable width 4 (after the 2-col "> " marker) → ceil(10/4)=3.
        // width arg is the TOTAL column count; usable = width - 2.
        assert_eq!(visual_row_count("0123456789", 6), 3);
    }

    #[test]
    fn visual_rows_wrap_plus_newline() {
        // "0123456789" wraps to 3 (width 6 → usable 4), then "x" is 1 → 4 total.
        assert_eq!(visual_row_count("0123456789\nx", 6), 4);
    }

    #[test]
    fn move_down_preserves_column() {
        // "abc\ndef", cursor at line0 col2 (byte 2) → down → line1 col2 (byte 6).
        assert_eq!(apply_move_vertical("abc\ndef", 2, 1), 6);
    }

    #[test]
    fn move_up_preserves_column() {
        // line1 col1 (byte 5) → up → line0 col1 (byte 1).
        assert_eq!(apply_move_vertical("abc\ndef", 5, -1), 1);
    }

    #[test]
    fn move_down_clamps_to_shorter_line() {
        // line0 col3 (byte 3, end of "abc") → down → "de" only has col 0..2 → byte 6 (col2).
        assert_eq!(apply_move_vertical("abc\nde", 3, 1), 6);
    }

    #[test]
    fn move_up_at_top_is_noop_to_target_col_on_line0() {
        // Already on line 0 → up clamps to line 0 (same line), column preserved.
        assert_eq!(apply_move_vertical("abc\ndef", 1, -1), 1);
    }

    #[test]
    fn move_down_at_bottom_is_noop() {
        // Already on last line → down stays (column preserved on same line).
        assert_eq!(apply_move_vertical("abc\ndef", 5, 1), 5);
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

    #[test]
    fn prompt_input_renders_three_lines() {
        let mut el =
            element! { PromptInput(text: "a\nb\nc".to_string(), cursor: 0usize, width: 80usize) };
        let out = el.to_string();
        assert!(out.contains("> a"), "got: {out}");
        assert!(out.contains("  b"), "got: {out}");
        assert!(out.contains("  c"), "got: {out}");
    }

    #[test]
    fn prompt_input_single_line_unchanged() {
        let mut el =
            element! { PromptInput(text: "hi".to_string(), cursor: 0usize, width: 80usize) };
        let out = el.to_string();
        assert!(out.contains("> hi"), "got: {out}");
    }
}

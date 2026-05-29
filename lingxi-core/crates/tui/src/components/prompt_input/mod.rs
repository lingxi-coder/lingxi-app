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

pub mod fuzzy;

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

/// The COLUMN CONTRACT for the prompt editor (M7-06, foundation for the
/// M7-08/09 vim vertical-motion `j`/`k`/`gg`/`G`):
///
/// > **A "column" is a DISPLAY width** — the sum of
/// > [`unicode_width`](UnicodeWidthStr) cell widths of the graphemes from the
/// > line start up to the cursor.
///
/// This is the SAME unit the soft-wrap/height path ([`visual_row_count`]) and
/// the terminal renderer use, so the height metric and the motion metric agree.
/// Vertical motion preserves the *visual* column the user sees, not a
/// grapheme index — on lines with wide (CJK, 2-cell) characters the two
/// diverge, and a grapheme-index metric would drift the caret horizontally.
/// All offsets remain grapheme/UTF-8 safe: we walk graphemes, accumulate
/// display width, and only ever land on a grapheme boundary.
///
/// Map a byte `cursor` into `(line_index, display_column)`.
#[must_use]
pub fn cursor_line_col(text: &str, cursor: usize) -> (usize, usize) {
    let cursor = clamp_to_char_boundary(text, cursor);
    let starts = line_starts(text);
    // Largest line start <= cursor.
    let line = starts.iter().rposition(|&s| s <= cursor).unwrap_or(0);
    let line_start = starts[line];
    // Column == cumulative DISPLAY width (not grapheme count) of the text
    // between the line start and the cursor. See the column contract above.
    let col = UnicodeWidthStr::width(&text[line_start..cursor]);
    (line, col)
}

/// Insert a `\n` at byte index `cursor`, returning `(new_text, new_cursor)`.
/// Mirrors `apply_insert` but for the newline char.
#[must_use]
pub fn apply_newline(text: &str, cursor: usize) -> (String, usize) {
    apply_insert(text, cursor, '\n')
}

/// Number of terminal rows the buffer occupies at the given total column
/// `width`, accounting for the 2-column prompt marker (`"❯ "` on the first
/// logical line, a matching 2-space indent on the rest) and soft-wrapping each
/// logical line. Always at least 1 (an empty buffer shows one row).
///
/// Column width is measured with [`UnicodeWidthStr`] — the same DISPLAY-width
/// unit as the column contract on [`cursor_line_col`] and the terminal
/// renderer, so the height metric and the motion metric agree.
///
/// NOTE (M7-06 budget-vs-render skew — within a single LOGICAL line): this
/// function budgets `usable = width - 2` cells per row for EVERY logical line,
/// modelling a 2-col continuation indent on wrapped rows. For the
/// multi-LOGICAL-line case this is exact: [`PromptInput`] emits one `Text` per
/// logical line and indents continuation logical lines 2 cols, so
/// budget == render (pinned by `budget_equals_rendered_rows_for_logical_lines`).
/// But when a SINGLE logical line is long enough to soft-wrap, iocraft wraps it
/// at the FULL `width` (no continuation indent on the wrapped rows), while this
/// budget assumed `width - 2`. So for within-logical-line wrap the budget can
/// over-count rows by up to one per wrap. This is a deliberate M7-06
/// simplification (a slightly conservative height is safe — it never clips
/// text); a future pass (M7-16) can render true continuation-indented wraps to
/// close the skew. The same NOTE lives at the [`PromptInput`] render site.
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
/// DISPLAY column (clamped to the destination line). Returns the new byte
/// cursor. At the top/bottom edge the cursor stays on its current line.
///
/// "Column" here is the display-width unit defined by the column contract on
/// [`cursor_line_col`] — the same unit [`visual_row_count`] budgets with, so
/// `j`/`k` land where the user *visually* sees the caret even on lines with
/// wide (CJK, 2-cell) characters. We walk the destination line's graphemes
/// accumulating cell width and land on the first grapheme boundary whose
/// cumulative width is `>=` the target column (i.e. `==` it, or — when the
/// target falls inside a wide char — just past it). When the target exceeds
/// the line's width we clamp to the line end. The result is always a grapheme
/// boundary, never a mid-codepoint byte index.
#[must_use]
#[allow(
    clippy::cast_possible_wrap,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
pub fn apply_move_vertical(text: &str, cursor: usize, delta: i32) -> usize {
    let starts = line_starts(text);
    let (line, target_col) = cursor_line_col(text, cursor);
    let target_line = (line as i32 + delta).clamp(0, starts.len() as i32 - 1) as usize;
    let line_start = starts[target_line];
    // Destination line end drops the trailing '\n'.
    let line_end = starts.get(target_line + 1).map_or(text.len(), |&s| s - 1);
    let dest = &text[line_start..line_end];
    // Walk graphemes accumulating display width; stop at the first boundary
    // whose cumulative width reaches the target column. Each boundary's column
    // is the width of everything BEFORE it, so we test before adding `g`'s
    // width. Falls through to the line end when the target is past the line.
    let mut acc_width = 0usize;
    let mut byte = line_end;
    for (off, g) in dest.grapheme_indices(true) {
        if acc_width >= target_col {
            byte = line_start + off;
            break;
        }
        acc_width += UnicodeWidthStr::width(g);
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
/// first carries the `"❯ "` marker (claude-code `PromptInputModeIndicator`'s
/// `figures.pointer` + space), the rest are indented 2 columns to align under
/// it. The outer `View`'s height grows with [`visual_row_count`].
///
/// NOTE (M7-06 budget-vs-render skew): [`visual_row_count`] budgets one row per
/// `width - 2` cells for every logical line (modelling a 2-col continuation
/// indent). That is exact for the multi-LOGICAL-line case below — each logical
/// line is one `Text` row, first prefixed `"❯ "`, the rest indented 2 cols. But
/// each `Text` is left to iocraft to soft-wrap WITHIN a logical line, and
/// iocraft wraps at the FULL `width` with no continuation indent, so a long
/// single logical line renders slightly differently from the `width - 2`
/// budget. See the matching NOTE on [`visual_row_count`]; deferred to M7-16.
#[component]
#[allow(clippy::cast_possible_truncation)]
pub fn PromptInput(props: &PromptInputProps) -> impl Into<AnyElement<'static>> {
    let _ = props.cursor; // cursor glyph rendering remains an M7-08 (vim) enhancement
    let width = if props.width == 0 { 80 } else { props.width };
    let starts = line_starts(&props.text);
    let height = visual_row_count(&props.text, width);
    // One Text per logical line; first line carries the "❯ " marker, the rest
    // are indented by 2 columns to align under it. "❯ " is display-width 2, the
    // same as the indent, so the `usable = width - 2` budget is unchanged.
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
                    format!("❯ {content}")
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
    fn cursor_line_col_wide_char_display_width() {
        // Column == DISPLAY width (not grapheme count). "中" is 3 bytes, 1
        // grapheme, but display width 2. "中b\nxyz": byte 4 is after "中b" →
        // display col 2 + 1 = 3 (NOT grapheme count 2). This pins the
        // display-width column contract that apply_move_vertical relies on.
        assert_eq!(cursor_line_col("中b\nxyz", 4), (0, 3));
        // After just "中" (byte 3) → display col 2.
        assert_eq!(cursor_line_col("中b\nxyz", 3), (0, 2));
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
        // "hi" with the "❯ " marker, width 80 → 1 row.
        assert_eq!(visual_row_count("hi", 80), 1);
        assert_eq!(visual_row_count("", 80), 1); // empty buffer still 1 row
    }

    #[test]
    fn visual_rows_three_logical_lines() {
        assert_eq!(visual_row_count("a\nb\nc", 80), 3);
    }

    #[test]
    fn visual_rows_wraps_long_line() {
        // 10 graphemes, usable width 4 (after the 2-col "❯ " marker) → ceil(10/4)=3.
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
    fn move_down_preserves_display_column_over_wide_char() {
        // "中b\nxyz": "中" is display-width 2, "b" is 1. Cursor after "中b"
        // (byte 4) is at DISPLAY column 3. Down → line 1 "xyz": the boundary
        // whose cumulative display width is >= 3 is the end (after "z"),
        // grapheme offset 3 within the line. Line 1 starts at byte 5 (中=3 +
        // b=1 + \n=1), so byte = 5 + 3 = 8 = text.len(). A grapheme-index
        // metric (col 2) would have landed at byte 7 instead — the drift this
        // fix removes.
        assert_eq!(apply_move_vertical("中b\nxyz", 4, 1), 8);
    }

    #[test]
    fn move_down_lands_just_past_wide_char_boundary() {
        // "ab\n中d": cursor after "a" (byte 1) is DISPLAY column 1. Down →
        // line 1 "中d": boundary display columns are 0 (before 中), 2 (after
        // 中), 3 (after d). The first boundary >= 1 is the one after "中"
        // (display col 2), at grapheme offset 3 within the line. Line 1
        // starts at byte 3 (ab=2 + \n=1), so byte = 3 + 3 = 6. The caret
        // lands just past the wide char rather than splitting it.
        assert_eq!(apply_move_vertical("ab\n中d", 1, 1), 6);
    }

    #[test]
    fn move_up_preserves_display_column_over_wide_char() {
        // Inverse of the down case. "中b\nxyz": cursor on line 1 after "xyz"
        // (byte 8) is DISPLAY column 3. Up → line 0 "中b": boundary display
        // columns 0, 2 (after 中), 3 (after b). First >= 3 is after "b"
        // (offset 4), byte 0 + 4 = 4.
        assert_eq!(apply_move_vertical("中b\nxyz", 8, -1), 4);
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
        assert!(out.contains("❯ a"), "got: {out}");
        assert!(out.contains("  b"), "got: {out}");
        assert!(out.contains("  c"), "got: {out}");
    }

    #[test]
    fn budget_equals_rendered_rows_for_logical_lines() {
        // For a multi-LOGICAL-line buffer where no single logical line is long
        // enough to soft-wrap, `visual_row_count` (the height budget the cache
        // relies on) MUST equal the number of rows the component renders. The
        // component emits exactly one Text row per logical line, so we count
        // the non-empty rendered lines and assert equality with the budget.
        let text = "line one\nline two\nline three";
        let width = 80usize;
        let budget = visual_row_count(text, width);
        let mut el = element! { PromptInput(text: text.to_string(), cursor: 0usize, width: width) };
        let out = el.to_string();
        let rendered_rows = out.lines().filter(|l| !l.trim().is_empty()).count();
        assert_eq!(
            budget, rendered_rows,
            "budget {budget} must equal rendered rows {rendered_rows} for logical-line buffer; got:\n{out}"
        );
        assert_eq!(budget, 3, "three logical lines → 3 rows");
    }

    #[test]
    fn prompt_input_single_line_unchanged() {
        let mut el =
            element! { PromptInput(text: "hi".to_string(), cursor: 0usize, width: 80usize) };
        let out = el.to_string();
        assert!(out.contains("❯ hi"), "got: {out}");
    }
}

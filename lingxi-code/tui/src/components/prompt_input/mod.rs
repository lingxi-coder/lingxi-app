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
use crate::render_iocraft::StyleColorIocraftExt;

pub mod banner;
pub use banner::{render_session_color_banner, SessionColorBanner, SessionColorBannerProps};

pub mod footer;
pub use footer::{FooterMode, PromptInputFooter, PromptInputFooterProps};

pub mod completion;
pub mod fuzzy;
pub mod palette;

pub mod history_search;
pub use history_search::{
    handle_history_search_key, hs_accept, hs_backspace, hs_cycle, hs_open, hs_push_char,
    recompute_match, HistorySearchOverlay, HistorySearchOverlayProps, HistorySearchState,
    HsKeyOutcome,
};

pub mod image_paste;
pub use image_paste::{
    apply_paste_block, expand_pasted_text_refs, format_image_ref, format_pasted_text_ref,
    is_image_path, paste_text_ref_num_lines, process_paste, Attachment, AttachmentKind, PasteApply,
    PasteCoalescer, PasteOutcome, PasteState, BURST_WINDOW, MAX_PASTE_LINES, PASTE_THRESHOLD,
};

pub mod vim;
pub use vim::{mode_indicator, VimMode, VimState};

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

/// Split one logical line into `(segment, is_cursor)` chunks for the cursor
/// glyph render, mirroring claude-code's `Cursor.render` before/at/after split
/// (`utils/Cursor.ts:244-296`).
///
/// `cursor_col_in_line` is the DISPLAY column (see the column contract on
/// [`cursor_line_col`]) where the caret sits on this line, or `None` if the
/// caret is not on this line (then the whole line is one non-cursor chunk).
///
/// The single grapheme whose cumulative display width first *exceeds* the
/// cursor column is the "at-cursor" chunk (`is_cursor = true`) — this matches
/// the TS `nextWidth > column` test, so a caret on a wide (2-cell) cluster
/// marks the WHOLE cluster. When the caret is at end-of-line (no grapheme
/// crosses the column) a synthetic `" "` cursor chunk is appended, faithful to
/// claude-code's `atCursor = cursorChar` default (`cursorChar` is `" "` when
/// the input shows its cursor — `TextInput.tsx:105`). An empty line therefore
/// yields a single highlighted space.
///
/// Returned chunks concatenate back to the original line (plus the trailing
/// synthetic space at EOL); adjacent chunks of the same flag are NOT merged
/// (callers style per-chunk), but the cursor chunk is always exactly one
/// grapheme (or the synthetic space).
#[must_use]
pub fn render_line_with_cursor(
    line: &str,
    cursor_col_in_line: Option<usize>,
) -> Vec<(String, bool)> {
    let Some(column) = cursor_col_in_line else {
        // Caret is not on this line: one plain chunk (skip empty so the line
        // contributes nothing rather than an empty Text).
        if line.is_empty() {
            return Vec::new();
        }
        return vec![(line.to_string(), false)];
    };

    let mut before = String::new();
    let mut at_cursor: Option<String> = None;
    let mut after = String::new();
    let mut current_width = 0usize;

    for g in line.graphemes(true) {
        if at_cursor.is_some() {
            after.push_str(g);
            continue;
        }
        let next_width = current_width + UnicodeWidthStr::width(g);
        if next_width > column {
            at_cursor = Some(g.to_string());
        } else {
            current_width = next_width;
            before.push_str(g);
        }
    }

    // EOL: no grapheme crossed the column → synthetic single-space cursor,
    // faithful to claude-code's `atCursor = cursorChar` (" ") default.
    let cursor_chunk = at_cursor.unwrap_or_else(|| " ".to_string());

    let mut chunks = Vec::with_capacity(3);
    if !before.is_empty() {
        chunks.push((before, false));
    }
    chunks.push((cursor_chunk, true));
    if !after.is_empty() {
        chunks.push((after, false));
    }
    chunks
}

/// Compute the inline progressive argument-hint shown as dimmed ghost text
/// after a fully-typed slash command in the `commandWithoutArgs` state.
///
/// Faithful port of the claude-code path that produces this hint:
/// `useTypeahead.tsx:756-759` (build the hint from `exactMatch.argNames` ONLY
/// when the user has typed the command name followed by a trailing space) fed
/// into `BaseTextInput.tsx:92,105,111` (render it inline). The combined gate:
///
/// * the buffer starts with `/` (a slash command),
/// * it ends with a single trailing space — TS `value.endsWith(' ')`, the
///   `commandWithoutArgs` ready-for-arguments state,
/// * the text before the first space exactly names a known command, and
/// * that command declares a non-empty `argNames` list.
///
/// `arg_names_for` looks up a command name (without the leading `/`) → its
/// declared argument names; it returns `None`/empty for every command that
/// declares none (all built-ins), so this function returns `None` and nothing
/// new renders — byte-identical to today. The remaining-names string itself is
/// produced by the shared `command_api::generate_progressive_argument_hint`
/// helper, so the bracketed `"[arg2] [arg3]"` shape matches TS exactly.
#[must_use]
pub fn progressive_argument_hint<'a>(
    buffer: &str,
    arg_names_for: impl Fn(&str) -> Option<&'a [String]>,
) -> Option<String> {
    // Slash command + ready-for-arguments trailing space (TS `value.endsWith(' ')`).
    if !buffer.starts_with('/') || !buffer.ends_with(' ') {
        return None;
    }
    // The command name is the run between the leading `/` and the first space.
    let after_slash = &buffer[1..];
    let space_index = after_slash.find(' ')?;
    let command_name = &after_slash[..space_index];
    if command_name.is_empty() {
        return None;
    }
    let arg_names = arg_names_for(command_name)?;
    if arg_names.is_empty() {
        return None;
    }
    // Everything after the command name's space is the typed-args region; TS
    // parses it with the shell-quote-faithful `parseArguments`.
    let args_text = &after_slash[space_index + 1..];
    let typed_args = command_api::parse_arguments(args_text);
    command_api::generate_progressive_argument_hint(arg_names, &typed_args)
}

/// Props for `PromptInput`.
#[derive(Props)]
pub struct PromptInputProps {
    /// Current text in the prompt buffer (may contain `\n`).
    pub text: String,
    /// Byte-index cursor position (always at a char boundary).
    pub cursor: usize,
    /// Total terminal column width (drives wrap + height). 0 → treat as 80.
    pub width: usize,
    /// Draw the inverse-video cursor block. Defaults to `true` (cursor shown);
    /// tests force `false`. Mirrors claude-code gating the cursor on
    /// `focus && showCursor && terminalFocus` (`BaseTextInput.tsx:63`) — when
    /// the prompt isn't the active/focused input, no caret is drawn.
    pub show_cursor: bool,
    /// Inline progressive argument-hint (e.g. `"[arg2] [arg3]"`) to render as
    /// dimmed ghost text after the typed command, in the `commandWithoutArgs`
    /// state. `None` (the default for every existing call site and every command
    /// without declared `argNames`) renders nothing extra, keeping the output
    /// byte-identical. Mirrors claude-code `BaseTextInput.tsx:105,111` rendering
    /// `props.argumentHint` as `<Text dimColor>` after the input value.
    pub argument_hint: Option<String>,
}

impl Default for PromptInputProps {
    fn default() -> Self {
        PromptInputProps {
            text: String::new(),
            cursor: 0,
            width: 0,
            show_cursor: true,
            argument_hint: None,
        }
    }
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
    let width = if props.width == 0 { 80 } else { props.width };
    let starts = line_starts(&props.text);
    let height = visual_row_count(&props.text, width);
    // Map the byte cursor → (line_index, display_col) once, so only the line
    // containing the caret gets a highlighted chunk. Gated on `show_cursor`
    // (claude-code's `focus && showCursor && terminalFocus`): when off, no line
    // is the cursor line.
    let (cursor_line, cursor_col) = if props.show_cursor {
        let (l, c) = cursor_line_col(&props.text, props.cursor);
        (Some(l), c)
    } else {
        (None, 0)
    };
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
    let last_line = lines.len().saturating_sub(1);
    // Inline progressive argument-hint rendered after the LAST line. By
    // construction the hint is only ever set in the `commandWithoutArgs` state
    // (a single-line slash command ending in a space), so the last line is the
    // command line. `None` ⇒ no extra element ⇒ output byte-identical to today.
    let argument_hint = props.argument_hint.clone();
    element! {
        View(flex_direction: FlexDirection::Column, height: height as u16) {
            #(lines.into_iter().map(|(i, content)| {
                let prefix = if i == 0 { "❯ " } else { "  " };
                let col_in_line = if cursor_line == Some(i) { Some(cursor_col) } else { None };
                let chunks = render_line_with_cursor(&content, col_in_line);
                // The hint trails the final line only; the gate already requires a
                // trailing space, so (mirroring `BaseTextInput.tsx:105`'s
                // `value.endsWith(" ") ? "" : " "`) no extra separator is added.
                let line_hint = if i == last_line { argument_hint.clone() } else { None };
                // The caret is inverse-video: claude-code renders the cursor as
                // `invert(atCursor)` (utils/Cursor.ts) — chalk.inverse swaps the
                // glyph's fg/bg against the terminal defaults, i.e. a solid block
                // the color of the default foreground (white) with the glyph
                // punched out in the background color (black). We render that as a
                // White-background box with a Black glyph rather than
                // `Text(invert: true)`: iocraft TRIMS inverted TRAILING whitespace,
                // so the synthetic end-of-line / empty-prompt caret (a single
                // cursor space) would vanish under SGR-reverse — the MOST common
                // caret position. A `View(background_color)` box is an explicit
                // styled cell that is never trimmed, so the block stays visible at
                // end-of-line. For the prompt's uncolored text a white/black block
                // is pixel-identical to true inverse. (The earlier Cyan block had
                // the right idea but the wrong color — claude-code has no cyan
                // cursor.)
                element! {
                    View(flex_direction: FlexDirection::Row) {
                        Text(content: prefix.to_string())
                        #(chunks.into_iter().map(|(seg, is_cursor)| {
                            if is_cursor {
                                element! {
                                    View(background_color: Color::White) {
                                        Text(content: seg, color: Color::Black)
                                    }
                                }.into_any()
                            } else {
                                element! { Text(content: seg) }.into_any()
                            }
                        }))
                        #(line_hint.map(|hint| element! {
                            Text(content: hint, color: crate::theme::TuiTheme::DIM.to_iocraft())
                        }))
                    }
                }
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ----- progressive_argument_hint (ARGS.3 inline ghost hint) -----

    /// Build a name→argNames lookup for the hint tests.
    fn lookup<'a>(name: &str, argv: &'a [String]) -> impl Fn(&str) -> Option<&'a [String]> {
        let key = name.to_string();
        move |q: &str| if q == key { Some(argv) } else { None }
    }

    #[test]
    fn hint_shows_remaining_args_after_command_and_space() {
        let args = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        // "/deploy " → all three remain.
        assert_eq!(
            progressive_argument_hint("/deploy ", lookup("deploy", &args)),
            Some("[a] [b] [c]".to_string())
        );
        // "/deploy prod " → one consumed → two remain.
        assert_eq!(
            progressive_argument_hint("/deploy prod ", lookup("deploy", &args)),
            Some("[b] [c]".to_string())
        );
        // "/deploy prod us-east " → two consumed → one remains.
        assert_eq!(
            progressive_argument_hint("/deploy prod us-east ", lookup("deploy", &args)),
            Some("[c]".to_string())
        );
    }

    #[test]
    fn hint_none_when_all_args_filled() {
        let args = vec!["a".to_string(), "b".to_string()];
        assert_eq!(
            progressive_argument_hint("/deploy x y ", lookup("deploy", &args)),
            None
        );
    }

    #[test]
    fn hint_none_without_trailing_space() {
        let args = vec!["a".to_string()];
        // Still typing the command name (palette state, not commandWithoutArgs).
        assert_eq!(
            progressive_argument_hint("/deploy", lookup("deploy", &args)),
            None
        );
        // Mid-typing an argument (no trailing space) → no progressive hint.
        assert_eq!(
            progressive_argument_hint("/deploy pro", lookup("deploy", &args)),
            None
        );
    }

    #[test]
    fn hint_none_for_command_without_argnames() {
        // Empty argNames (every built-in) → nothing renders.
        let empty: Vec<String> = Vec::new();
        assert_eq!(
            progressive_argument_hint("/help ", lookup("help", &empty)),
            None
        );
        // Unknown command (not in the lookup) → nothing renders.
        let args = vec!["a".to_string()];
        assert_eq!(
            progressive_argument_hint("/unknown ", lookup("deploy", &args)),
            None
        );
    }

    #[test]
    fn hint_none_for_non_slash_or_bare_slash() {
        let args = vec!["a".to_string()];
        // Not a slash command.
        assert_eq!(
            progressive_argument_hint("deploy ", lookup("deploy", &args)),
            None
        );
        // Bare "/" + space → empty command name → no hint.
        assert_eq!(
            progressive_argument_hint("/ ", lookup("deploy", &args)),
            None
        );
    }

    #[test]
    fn hint_parses_quoted_typed_args_like_shell() {
        let args = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        // A quoted run counts as ONE typed arg (shell-quote-faithful parsing).
        assert_eq!(
            progressive_argument_hint("/deploy \"hello world\" ", lookup("deploy", &args)),
            Some("[b] [c]".to_string())
        );
    }

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
    fn cursor_none_when_not_on_line() {
        // No caret on this line → one plain chunk, no highlight.
        assert_eq!(
            render_line_with_cursor("hello", None),
            vec![("hello".to_string(), false)]
        );
    }

    #[test]
    fn cursor_none_empty_line_yields_nothing() {
        // A non-cursor empty line contributes no chunk (no empty Text).
        assert_eq!(
            render_line_with_cursor("", None),
            Vec::<(String, bool)>::new()
        );
    }

    #[test]
    fn cursor_mid_line_marks_exactly_one_char() {
        // "hello", caret at display col 2 → before "he", at "l", after "lo".
        assert_eq!(
            render_line_with_cursor("hello", Some(2)),
            vec![
                ("he".to_string(), false),
                ("l".to_string(), true),
                ("lo".to_string(), false),
            ]
        );
        // Exactly one chunk is the cursor, and it is exactly one grapheme.
        let chunks = render_line_with_cursor("hello", Some(2));
        let cursor_chunks: Vec<_> = chunks.iter().filter(|(_, c)| *c).collect();
        assert_eq!(cursor_chunks.len(), 1);
        assert_eq!(cursor_chunks[0].0.chars().count(), 1);
    }

    #[test]
    fn cursor_at_line_start_marks_first_char() {
        // col 0 → no "before" chunk, first char is the cursor.
        assert_eq!(
            render_line_with_cursor("hello", Some(0)),
            vec![("h".to_string(), true), ("ello".to_string(), false)]
        );
    }

    #[test]
    fn cursor_at_eol_appends_synthetic_space() {
        // caret past the last grapheme (col == line width) → before "hi",
        // synthetic highlighted " ", no after. Mirrors atCursor = cursorChar.
        assert_eq!(
            render_line_with_cursor("hi", Some(2)),
            vec![("hi".to_string(), false), (" ".to_string(), true)]
        );
    }

    #[test]
    fn cursor_empty_line_single_highlighted_space() {
        // Empty cursor line → a single highlighted space (the caret block).
        assert_eq!(
            render_line_with_cursor("", Some(0)),
            vec![(" ".to_string(), true)]
        );
    }

    #[test]
    fn cursor_on_wide_char_marks_whole_cluster() {
        // "中b": "中" is display-width 2. Caret at display col 0 lands on the
        // wide cluster — the WHOLE "中" is the cursor chunk (nextWidth 2 > 0).
        assert_eq!(
            render_line_with_cursor("中b", Some(0)),
            vec![("中".to_string(), true), ("b".to_string(), false)]
        );
        // Caret at display col 1 (mid the 2-cell char) still resolves to the
        // whole "中" cluster, never splitting the codepoint.
        assert_eq!(
            render_line_with_cursor("中b", Some(1)),
            vec![("中".to_string(), true), ("b".to_string(), false)]
        );
        // Caret at display col 2 (just past "中") lands on "b".
        assert_eq!(
            render_line_with_cursor("中b", Some(2)),
            vec![("中".to_string(), false), ("b".to_string(), true)]
        );
    }

    #[test]
    fn cursor_on_combining_grapheme_marks_whole_cluster() {
        // "é" written as e + combining acute (U+0301) is one grapheme, width 1.
        // Caret at col 0 marks the whole cluster (both codepoints) as cursor.
        let line = "e\u{0301}x";
        assert_eq!(
            render_line_with_cursor(line, Some(0)),
            vec![("e\u{0301}".to_string(), true), ("x".to_string(), false)]
        );
    }

    #[test]
    fn prompt_input_cursor_line_preserves_text() {
        // Render with show_cursor: the cursor block (color-swapped View/Text) is
        // emitted as a separate span, but iocraft's plain `to_string()` strips
        // styling, so all the visible characters still appear in order. The
        // per-chunk color swap itself is pinned by `render_line_with_cursor`
        // unit tests; here we assert the component splits without dropping text.
        let mut el = element! {
            PromptInput(text: "hi".to_string(), cursor: 0usize, width: 80usize, show_cursor: true)
        };
        let out = el.to_string();
        assert!(out.contains("❯ hi"), "got: {out}");
    }

    #[test]
    fn prompt_input_cursor_at_eol_renders_marker() {
        // Empty buffer, caret at EOL (byte 0) with the cursor shown → the "❯ "
        // marker row is emitted with a synthetic-space cursor block, so the
        // marker is still present and nothing panics on an empty line.
        let mut el = element! {
            PromptInput(text: String::new(), cursor: 0usize, width: 80usize, show_cursor: true)
        };
        let out = el.to_string();
        assert!(out.contains('❯'), "got: {out:?}");
    }

    #[test]
    fn prompt_input_hidden_cursor_plain_text() {
        // show_cursor: false → no cursor line, plain text only.
        let mut el = element! {
            PromptInput(text: "hi".to_string(), cursor: 0usize, width: 80usize, show_cursor: false)
        };
        let out = el.to_string();
        assert!(out.contains("❯ hi"), "got: {out}");
    }

    #[test]
    fn prompt_input_renders_three_lines() {
        let mut el = element! {
            PromptInput(text: "a\nb\nc".to_string(), cursor: 0usize, width: 80usize, show_cursor: false)
        };
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
        let mut el = element! {
            PromptInput(text: text.to_string(), cursor: 0usize, width: width, show_cursor: false)
        };
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
        let mut el = element! {
            PromptInput(text: "hi".to_string(), cursor: 0usize, width: 80usize, show_cursor: false)
        };
        let out = el.to_string();
        assert!(out.contains("❯ hi"), "got: {out}");
    }
}

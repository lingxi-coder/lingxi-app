//! StructuredDiff viewer (M7-02) — similar line+word diff, syntect-colored.
//!
//! Layout parity: claude-code/src/components/StructuredDiff/Fallback.tsx.
//!
//! Locked structural facts (from `Fallback.tsx` — `transformLinesToObjects`,
//! `processAdjacentLines`, `formatDiff`):
//!   - Each diff line is `add` / `remove` / `nochange` (context).
//!   - Gutter = right-aligned line number (`padStart(maxWidth)`) + one space;
//!     then the sigil column (`+` add / `-` remove / ` ` context) + one space;
//!     then the syntax-colored content.
//!   - `maxWidth` = width of the largest line number.
//!   - Added lines get a green line background (`diffAdded`), removed lines red
//!     (`diffRemoved`); content keeps its syntax fg layered over the line bg.
//!   - Adjacent remove→add runs are paired for word-level diffing;
//!     `CHANGE_THRESHOLD = 0.4` decides word-diff vs. plain full-line coloring
//!     when the two lines are too dissimilar.
//!
//! M7-01 type note: `StyledSpan` is `{ text, style: SpanStyle { fg, bg, bold,
//! .. }, kind }`. Diff backgrounds are `StyleColor::Rgb` values set on
//! `style.bg`; the dim gutter uses `StyleColor::Named(BrightBlack)`.

// `claude-code`, `Fallback.tsx`, `diffAdded` etc. read better unquoted in the
// module prose; suppress the doc-markdown nudge crate-wide for this file
// (matches `render/markdown.rs`).
#![allow(clippy::doc_markdown)]

use similar::{ChangeTag, TextDiff};

use crate::render::syntax;
use crate::render::{NamedColor, SpanStyle, StyleColor, StyledLine, StyledSpan};
use crate::theme::TuiTheme;

/// Classification of one rendered diff line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    /// Inserted line (new file only).
    Add,
    /// Deleted line (old file only).
    Remove,
    /// Unchanged context line.
    Context,
}

/// One row in the structured diff before layout.
#[derive(Debug, Clone)]
pub struct DiffRow {
    /// Add / Remove / Context classification.
    pub kind: LineKind,
    /// Line text with the trailing newline stripped.
    pub text: String,
    /// New-file line number for Add/Context; old-file line number for Remove.
    pub line_no: usize,
}

/// Green background for added lines (claude-code `diffAdded`). Dark + low
/// saturation so syntax fg stays readable. M7-15 will source these from the
/// active Theme.
#[must_use]
pub fn add_bg(_theme: &TuiTheme) -> StyleColor {
    StyleColor::Rgb(0x00, 0x40, 0x00)
}

/// Red background for removed lines (claude-code `diffRemoved`).
#[must_use]
pub fn remove_bg(_theme: &TuiTheme) -> StyleColor {
    StyleColor::Rgb(0x40, 0x00, 0x00)
}

/// claude-code CHANGE_THRESHOLD: above this changed-fraction, word diffing is
/// abandoned for whole-line coloring (lines too dissimilar to align words).
const CHANGE_THRESHOLD: f64 = 0.4;

/// Brighter emphasis background for the changed *words* of a paired add line
/// (claude-code `diffAddedWord`).
#[must_use]
pub fn add_word_bg(_theme: &TuiTheme) -> StyleColor {
    StyleColor::Rgb(0x00, 0x80, 0x00)
}

/// Brighter emphasis background for the changed *words* of a paired remove
/// line (claude-code `diffRemovedWord`).
#[must_use]
pub fn remove_word_bg(_theme: &TuiTheme) -> StyleColor {
    StyleColor::Rgb(0x80, 0x00, 0x00)
}

/// Run a line-level diff and classify each change. Line numbers follow
/// claude-code: removed lines number against the old file, added/context
/// against the new file.
#[must_use]
pub fn diff_rows(old: &str, new: &str) -> Vec<DiffRow> {
    let diff = TextDiff::from_lines(old, new);
    let mut rows = Vec::new();
    let mut old_no = 1usize;
    let mut new_no = 1usize;
    for change in diff.iter_all_changes() {
        let text = change.value().trim_end_matches('\n').to_string();
        match change.tag() {
            ChangeTag::Delete => {
                rows.push(DiffRow {
                    kind: LineKind::Remove,
                    text,
                    line_no: old_no,
                });
                old_no += 1;
            }
            ChangeTag::Insert => {
                rows.push(DiffRow {
                    kind: LineKind::Add,
                    text,
                    line_no: new_no,
                });
                new_no += 1;
            }
            ChangeTag::Equal => {
                rows.push(DiffRow {
                    kind: LineKind::Context,
                    text,
                    line_no: new_no,
                });
                old_no += 1;
                new_no += 1;
            }
        }
    }
    rows
}

/// Sigil character for a line kind: '+' / '-' / ' '.
fn sigil(kind: LineKind) -> char {
    match kind {
        LineKind::Add => '+',
        LineKind::Remove => '-',
        LineKind::Context => ' ',
    }
}

/// Build the gutter span ("  12 + ") for a row: right-aligned line number +
/// space + sigil + space, dim-colored, carrying the line background.
fn gutter_span(row: &DiffRow, gutter_w: usize, bg: StyleColor) -> StyledSpan {
    let text = format!(
        "{:>w$} {} ",
        row.line_no,
        sigil(row.kind),
        w = gutter_w
    );
    StyledSpan::styled(
        text,
        SpanStyle {
            fg: StyleColor::Named(NamedColor::BrightBlack),
            bg,
            ..SpanStyle::default()
        },
    )
}

/// Syntax-highlight `text` as a single line and overlay `bg` onto every
/// content span (keep the syntect fg, force the diff background).
fn content_spans(text: &str, lang: Option<&str>, bg: StyleColor, theme: &TuiTheme) -> Vec<StyledSpan> {
    let highlighted = syntax::highlight(text, lang, theme);
    if let Some(first) = highlighted.into_iter().next() {
        first
            .spans
            .into_iter()
            .map(|mut s| {
                s.style.bg = bg;
                s
            })
            .collect()
    } else {
        // Empty line — still carry the bg so the row colors fully.
        vec![StyledSpan::styled(
            String::new(),
            SpanStyle {
                bg,
                ..SpanStyle::default()
            },
        )]
    }
}

/// Build the content spans for one line of a paired word-diff. `is_add`
/// selects which side's changes to emphasize. Returns `None` if the two lines
/// are too dissimilar (caller falls back to whole-line coloring).
fn word_diff_spans(
    remove_text: &str,
    add_text: &str,
    is_add: bool,
    theme: &TuiTheme,
    line_bg: StyleColor,
) -> Option<Vec<StyledSpan>> {
    let wd = TextDiff::from_words(remove_text, add_text);
    // Changed fraction = changed words / total words on this side.
    let (mut changed, mut total) = (0usize, 0usize);
    for ch in wd.iter_all_changes() {
        match ch.tag() {
            ChangeTag::Equal => total += 1,
            ChangeTag::Delete => {
                if !is_add {
                    total += 1;
                    changed += 1;
                }
            }
            ChangeTag::Insert => {
                if is_add {
                    total += 1;
                    changed += 1;
                }
            }
        }
    }
    #[allow(clippy::cast_precision_loss)]
    let too_dissimilar = total == 0 || (changed as f64 / total as f64) > CHANGE_THRESHOLD;
    if too_dissimilar {
        return None;
    }
    let emph_bg = if is_add {
        add_word_bg(theme)
    } else {
        remove_word_bg(theme)
    };
    let mut spans = Vec::new();
    for ch in wd.iter_all_changes() {
        let show = matches!(ch.tag(), ChangeTag::Equal)
            || (is_add && ch.tag() == ChangeTag::Insert)
            || (!is_add && ch.tag() == ChangeTag::Delete);
        if !show {
            continue;
        }
        let emphasized = !matches!(ch.tag(), ChangeTag::Equal);
        spans.push(StyledSpan::styled(
            ch.value(),
            SpanStyle {
                bg: if emphasized { emph_bg } else { line_bg },
                ..SpanStyle::default()
            },
        ));
    }
    Some(spans)
}

/// Layout a single non-word-diffed row (context, or unpaired/too-dissimilar
/// add/remove): gutter + whole-line syntax-colored content over the line bg.
fn plain_row(row: &DiffRow, gutter_w: usize, lang: Option<&str>, theme: &TuiTheme) -> StyledLine {
    let bg = match row.kind {
        LineKind::Add => add_bg(theme),
        LineKind::Remove => remove_bg(theme),
        LineKind::Context => StyleColor::Default,
    };
    let mut spans = vec![gutter_span(row, gutter_w, bg)];
    spans.extend(content_spans(&row.text, lang, bg, theme));
    StyledLine { spans }
}

/// Layout a word-diffed row: gutter + per-word emphasis spans.
fn word_row(row: &DiffRow, gutter_w: usize, content: Vec<StyledSpan>, theme: &TuiTheme) -> StyledLine {
    let bg = match row.kind {
        LineKind::Add => add_bg(theme),
        LineKind::Remove => remove_bg(theme),
        LineKind::Context => StyleColor::Default,
    };
    let mut spans = vec![gutter_span(row, gutter_w, bg)];
    spans.extend(content);
    StyledLine { spans }
}

/// Render a structured diff of `old` → `new`. `path` drives syntax language
/// detection (claude-code's `filePath` prop). Never panics.
#[must_use]
pub fn render(old: &str, new: &str, path: Option<&str>, theme: &TuiTheme) -> Vec<StyledLine> {
    let rows = diff_rows(old, new);
    if rows.is_empty() {
        return Vec::new();
    }
    // Gutter width = widest line number, right-aligned.
    let max_no = rows.iter().map(|r| r.line_no).max().unwrap_or(1);
    let gutter_w = max_no.to_string().len();
    let lang = syntax::detect_language(None, path);

    layout_rows(&rows, gutter_w, lang.as_deref(), theme)
}

/// Lay out a slice of diff rows into styled lines, pairing adjacent
/// remove→add runs for word-level diffing (claude-code `processAdjacentLines`).
fn layout_rows(
    rows: &[DiffRow],
    gutter_w: usize,
    lang: Option<&str>,
    theme: &TuiTheme,
) -> Vec<StyledLine> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < rows.len() {
        if rows[i].kind == LineKind::Remove {
            // Collect the contiguous remove run, then the contiguous add run.
            let rem_start = i;
            while i < rows.len() && rows[i].kind == LineKind::Remove {
                i += 1;
            }
            let rem_end = i;
            let add_start = i;
            while i < rows.len() && rows[i].kind == LineKind::Add {
                i += 1;
            }
            let add_end = i;
            let removes = &rows[rem_start..rem_end];
            let adds = &rows[add_start..add_end];
            // Pair the k-th remove with the k-th add for word diffing; any
            // unpaired surplus on either side falls back to whole-line.
            let pairs = removes.len().min(adds.len());
            for k in 0..pairs {
                let rem = &removes[k];
                let add = &adds[k];
                let rb = remove_bg(theme);
                let ab = add_bg(theme);
                match (
                    word_diff_spans(&rem.text, &add.text, false, theme, rb),
                    word_diff_spans(&rem.text, &add.text, true, theme, ab),
                ) {
                    (Some(rem_spans), Some(add_spans)) => {
                        out.push(word_row(rem, gutter_w, rem_spans, theme));
                        out.push(word_row(add, gutter_w, add_spans, theme));
                    }
                    _ => {
                        // Too dissimilar — whole-line coloring for both.
                        out.push(plain_row(rem, gutter_w, lang, theme));
                        out.push(plain_row(add, gutter_w, lang, theme));
                    }
                }
            }
            for rem in &removes[pairs..] {
                out.push(plain_row(rem, gutter_w, lang, theme));
            }
            for add in &adds[pairs..] {
                out.push(plain_row(add, gutter_w, lang, theme));
            }
        } else {
            out.push(plain_row(&rows[i], gutter_w, lang, theme));
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Render a row to its concatenated text for structural assertion.
    fn rowline(l: &StyledLine) -> String {
        l.spans.iter().map(|s| s.text.clone()).collect()
    }

    #[test]
    fn classify_pure_add() {
        let rows = diff_rows("a\n", "a\nb\n");
        // a = context, b = add
        assert_eq!(
            rows.iter().map(|r| r.kind).collect::<Vec<_>>(),
            vec![LineKind::Context, LineKind::Add]
        );
    }

    #[test]
    fn classify_pure_remove() {
        let rows = diff_rows("a\nb\n", "a\n");
        assert_eq!(
            rows.iter().map(|r| r.kind).collect::<Vec<_>>(),
            vec![LineKind::Context, LineKind::Remove]
        );
    }

    #[test]
    fn classify_modify_is_remove_then_add() {
        let rows = diff_rows("foo\n", "bar\n");
        assert_eq!(
            rows.iter().map(|r| r.kind).collect::<Vec<_>>(),
            vec![LineKind::Remove, LineKind::Add]
        );
    }

    #[test]
    fn classify_empty_diff_is_all_context() {
        let rows = diff_rows("a\nb\n", "a\nb\n");
        assert!(rows.iter().all(|r| r.kind == LineKind::Context));
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn render_pure_add_has_plus_sigil_and_green_bg() {
        let lines = render("a\n", "a\nb\n", Some("x.txt"), &TuiTheme);
        assert_eq!(lines.len(), 2);
        // The add line's rendered text contains "+" sigil and the content "b".
        let add = &lines[1];
        let joined = rowline(add);
        assert!(joined.contains('+'), "add line carries + sigil: {joined:?}");
        assert!(joined.contains('b'));
        // At least one span on the add line has the green add background.
        assert!(
            add.spans.iter().any(|s| s.style.bg == add_bg(&TuiTheme)),
            "add line has green background"
        );
    }

    #[test]
    fn render_pure_remove_has_minus_sigil_and_red_bg() {
        let lines = render("a\nb\n", "a\n", Some("x.txt"), &TuiTheme);
        let rem = lines
            .iter()
            .find(|l| rowline(l).contains('-'))
            .expect("a - line");
        assert!(rowline(rem).contains('b'));
        assert!(
            rem.spans.iter().any(|s| s.style.bg == remove_bg(&TuiTheme)),
            "remove line has red background"
        );
    }

    #[test]
    fn render_gutter_has_line_numbers() {
        let lines = render("a\n", "a\nb\n", Some("x.txt"), &TuiTheme);
        // Context line "a" is line 1, add line "b" is line 2.
        assert!(rowline(&lines[0]).contains('1'));
        assert!(rowline(&lines[1]).contains('2'));
    }

    #[test]
    fn render_empty_diff_all_context_no_sigils() {
        let lines = render("a\nb\n", "a\nb\n", Some("x.txt"), &TuiTheme);
        assert_eq!(lines.len(), 2);
        for l in &lines {
            let j = rowline(l);
            assert!(
                !j.contains('+') && !j.contains('-'),
                "context lines carry neither + nor - sigil: {j:?}"
            );
        }
    }

    #[test]
    fn word_diff_highlights_only_changed_words() {
        // "function oldName(param)" -> "function newName(param)": only the
        // word "oldName"/"newName" should carry the intra-line emphasis bg.
        let lines = render(
            "function oldName(param)\n",
            "function newName(param)\n",
            Some("x.js"),
            &TuiTheme,
        );
        // One remove row + one add row.
        let rem = lines.iter().find(|l| rowline(l).contains('-')).unwrap();
        let add = lines.iter().find(|l| rowline(l).contains('+')).unwrap();
        // The changed-word span ("oldName"/"newName") carries the EMPHASIS bg,
        // while the unchanged "function "/"(param)" spans carry the line bg.
        assert!(rem
            .spans
            .iter()
            .any(|s| s.text.contains("oldName") && s.style.bg == remove_word_bg(&TuiTheme)));
        assert!(add
            .spans
            .iter()
            .any(|s| s.text.contains("newName") && s.style.bg == add_word_bg(&TuiTheme)));
        // The shared word "function" is NOT emphasized.
        assert!(add
            .spans
            .iter()
            .any(|s| s.text.contains("function") && s.style.bg != add_word_bg(&TuiTheme)));
    }

    #[test]
    fn word_diff_skipped_when_lines_too_dissimilar() {
        // Wildly different lines (> CHANGE_THRESHOLD changed) fall back to
        // whole-line coloring: no word-emphasis spans.
        let lines = render("aaaaaaaa\n", "zzzzzzzz\n", Some("x.txt"), &TuiTheme);
        let add = lines.iter().find(|l| rowline(l).contains('+')).unwrap();
        assert!(
            add.spans
                .iter()
                .all(|s| s.style.bg != add_word_bg(&TuiTheme)),
            "dissimilar lines use whole-line coloring, not word emphasis"
        );
    }
}

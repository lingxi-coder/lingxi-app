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

    rows.into_iter()
        .map(|row| {
            let bg = match row.kind {
                LineKind::Add => add_bg(theme),
                LineKind::Remove => remove_bg(theme),
                LineKind::Context => StyleColor::Default,
            };
            let mut spans = vec![gutter_span(&row, gutter_w, bg)];
            spans.extend(content_spans(&row.text, lang.as_deref(), bg, theme));
            StyledLine { spans }
        })
        .collect()
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
}

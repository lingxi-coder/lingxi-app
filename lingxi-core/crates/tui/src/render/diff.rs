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

/// Hard cap on rendered diff body lines (claude-code shows a "… N more lines"
/// footer past a budget). Mirrors the M6 UserToolResult MAX_LINES intent.
pub const MAX_DIFF_LINES: usize = 100;

/// Context radius for grouped (unified-diff) hunks — 3 lines, matching the
/// unified-diff default.
const CONTEXT_RADIUS: usize = 3;

/// Build a "@@ -oldStart,oldLen +newStart,newLen @@" header line. claude-code
/// renders hunk headers dim/cyan.
fn hunk_header(old_start: usize, old_len: usize, new_start: usize, new_len: usize) -> StyledLine {
    StyledLine {
        spans: vec![StyledSpan::styled(
            format!("@@ -{old_start},{old_len} +{new_start},{new_len} @@"),
            SpanStyle {
                fg: StyleColor::Named(NamedColor::BrightBlack),
                ..SpanStyle::default()
            },
        )],
    }
}

/// A "… {n} more lines" truncation footer (dim).
fn truncation_footer(n: usize) -> StyledLine {
    StyledLine {
        spans: vec![StyledSpan::styled(
            format!("… {n} more lines"),
            SpanStyle {
                fg: StyleColor::Named(NamedColor::BrightBlack),
                ..SpanStyle::default()
            },
        )],
    }
}

/// A unified-diff hunk header quadruple: `(old_start, old_len, new_start,
/// new_len)` with 1-based starts.
type HunkHeader = (usize, usize, usize, usize);

/// Group the diff into unified-diff hunks. Returns, per hunk, the header
/// quadruple and the rows of that hunk.
fn grouped_hunks(old: &str, new: &str) -> Vec<(HunkHeader, Vec<DiffRow>)> {
    let diff = TextDiff::from_lines(old, new);
    let mut hunks = Vec::new();
    for group in diff.grouped_ops(CONTEXT_RADIUS) {
        if group.is_empty() {
            continue;
        }
        let old_start = group.first().map_or(0, |op| op.old_range().start);
        let old_end = group.last().map_or(0, |op| op.old_range().end);
        let new_start = group.first().map_or(0, |op| op.new_range().start);
        let new_end = group.last().map_or(0, |op| op.new_range().end);
        let mut rows = Vec::new();
        for op in &group {
            for change in diff.iter_changes(op) {
                let text = change.value().trim_end_matches('\n').to_string();
                let (kind, line_no) = match change.tag() {
                    ChangeTag::Delete => (
                        LineKind::Remove,
                        change.old_index().map_or(0, |i| i + 1),
                    ),
                    ChangeTag::Insert => {
                        (LineKind::Add, change.new_index().map_or(0, |i| i + 1))
                    }
                    ChangeTag::Equal => (
                        LineKind::Context,
                        change.new_index().map_or(0, |i| i + 1),
                    ),
                };
                rows.push(DiffRow {
                    kind,
                    text,
                    line_no,
                });
            }
        }
        hunks.push((
            (
                old_start + 1,
                old_end - old_start,
                new_start + 1,
                new_end - new_start,
            ),
            rows,
        ));
    }
    hunks
}

/// Render a structured diff of `old` → `new`. `path` drives syntax language
/// detection (claude-code's `filePath` prop). Hunks are separated by unified
/// `@@` headers; output past `MAX_DIFF_LINES` body rows is truncated with a
/// "… N more lines" footer. Never panics.
#[must_use]
pub fn render(old: &str, new: &str, path: Option<&str>, theme: &TuiTheme) -> Vec<StyledLine> {
    let hunks = grouped_hunks(old, new);
    if hunks.is_empty() {
        return Vec::new();
    }
    // Gutter width = widest line number across all hunks, right-aligned.
    let max_no = hunks
        .iter()
        .flat_map(|(_, rows)| rows.iter().map(|r| r.line_no))
        .max()
        .unwrap_or(1);
    let gutter_w = max_no.to_string().len();
    let lang = syntax::detect_language(None, path);

    // Total body rows across all hunks (excludes headers) — used for the
    // truncation footer count.
    let total_body: usize = hunks.iter().map(|(_, rows)| rows.len()).sum();

    let mut out: Vec<StyledLine> = Vec::new();
    let mut body_emitted = 0usize;
    let mut truncated = false;
    'hunks: for (header, rows) in hunks {
        let (os, ol, ns, nl) = header;
        out.push(hunk_header(os, ol, ns, nl));
        let laid = layout_rows(&rows, gutter_w, lang.as_deref(), theme);
        // `laid` has one line per row (word-diff pairs are 1:1 with rows), so
        // body_emitted tracks row count directly.
        for line in laid {
            if body_emitted >= MAX_DIFF_LINES {
                truncated = true;
                break 'hunks;
            }
            out.push(line);
            body_emitted += 1;
        }
    }
    if truncated {
        out.push(truncation_footer(total_body - body_emitted));
    }
    out
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
                if let (Some(rem_spans), Some(add_spans)) = (
                    word_diff_spans(&rem.text, &add.text, false, theme, rb),
                    word_diff_spans(&rem.text, &add.text, true, theme, ab),
                ) {
                    out.push(word_row(rem, gutter_w, rem_spans, theme));
                    out.push(word_row(add, gutter_w, add_spans, theme));
                } else {
                    // Too dissimilar — whole-line coloring for both.
                    out.push(plain_row(rem, gutter_w, lang, theme));
                    out.push(plain_row(add, gutter_w, lang, theme));
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

    /// Is this a `@@ ... @@` hunk header line?
    fn is_header(l: &StyledLine) -> bool {
        rowline(l).contains("@@")
    }

    /// Body (non-header) lines only.
    fn body(lines: &[StyledLine]) -> Vec<&StyledLine> {
        lines.iter().filter(|l| !is_header(l)).collect()
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
        // The add line carries a "+" sigil and the content "b", over green bg.
        let add = lines
            .iter()
            .find(|l| !is_header(l) && rowline(l).contains('+'))
            .expect("an add line");
        let joined = rowline(add);
        assert!(joined.contains('b'), "add line content: {joined:?}");
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
            .find(|l| !is_header(l) && rowline(l).contains('-'))
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
        let rows = body(&lines);
        // Context line "a" is line 1, add line "b" is line 2.
        assert!(rows.iter().any(|l| rowline(l).contains('1')));
        assert!(rows.iter().any(|l| rowline(l).contains('2')));
    }

    #[test]
    fn render_empty_diff_is_empty() {
        // No changes -> no hunks -> empty output (claude-code renders nothing).
        let lines = render("a\nb\n", "a\nb\n", Some("x.txt"), &TuiTheme);
        assert!(lines.is_empty());
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
        // One remove row + one add row (skip the @@ header).
        let rem = lines
            .iter()
            .find(|l| !is_header(l) && rowline(l).contains('-'))
            .unwrap();
        let add = lines
            .iter()
            .find(|l| !is_header(l) && rowline(l).contains('+'))
            .unwrap();
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
        let add = lines
            .iter()
            .find(|l| !is_header(l) && rowline(l).contains('+'))
            .unwrap();
        assert!(
            add.spans
                .iter()
                .all(|s| s.style.bg != add_word_bg(&TuiTheme)),
            "dissimilar lines use whole-line coloring, not word emphasis"
        );
    }

    #[test]
    fn render_with_separated_changes_emits_hunk_header() {
        // Two change clusters separated by a long unchanged run -> the second
        // cluster is preceded by a hunk header "@@ ... @@".
        let old = (1..=40).map(|n| format!("line{n}")).collect::<Vec<_>>().join("\n") + "\n";
        let mut new_lines: Vec<String> = (1..=40).map(|n| format!("line{n}")).collect();
        new_lines[2] = "CHANGED_TOP".into();
        new_lines[37] = "CHANGED_BOTTOM".into();
        let new = new_lines.join("\n") + "\n";
        let lines = render(&old, &new, Some("x.txt"), &TuiTheme);
        let headers = lines.iter().filter(|l| rowline(l).contains("@@")).count();
        assert!(headers >= 1, "separated change clusters produce hunk header(s)");
    }

    #[test]
    fn render_truncates_large_diff_with_footer() {
        // A diff with > MAX_DIFF_LINES changed lines is truncated with a footer.
        let old = String::new();
        let new = (0..(MAX_DIFF_LINES + 50))
            .map(|n| format!("add{n}"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        let lines = render(&old, &new, Some("x.txt"), &TuiTheme);
        // Body capped at MAX_DIFF_LINES; total = 1 header + cap + 1 footer.
        assert!(
            lines.len() <= MAX_DIFF_LINES + 2,
            "truncated to header + cap + footer, got {}",
            lines.len()
        );
        let last = rowline(lines.last().unwrap());
        assert!(last.contains("more lines"), "truncation footer present: {last:?}");
    }
}

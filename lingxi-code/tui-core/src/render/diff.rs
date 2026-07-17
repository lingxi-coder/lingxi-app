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
use crate::theme::ThemeName;

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

/// Green background for added lines — claude-code's SYNTAX-HIGHLIGHTED diff
/// color scheme (the `scopes`-carrying renderer LingXi mirrors, NOT the plain
/// `theme.diffAdded` token): dark-theme truecolor `addLine = rgb(2,40,0)`. A
/// deep, low-luminance green so the syntect fg layered on top stays readable.
/// Locked to the dark-theme value like the gutter decorations.
#[must_use]
pub fn add_bg(_theme: ThemeName) -> StyleColor {
    StyleColor::Rgb(2, 40, 0)
}

/// Red background for removed lines — claude-code syntax-diff `deleteLine`
/// (`rgb(61,1,0)`, dark-theme truecolor value).
#[must_use]
pub fn remove_bg(_theme: ThemeName) -> StyleColor {
    StyleColor::Rgb(61, 1, 0)
}

/// (diff-02) Green decoration color for the Add gutter (sigil + line number)
/// — claude-code `theme.addDecoration` (`rgb(80, 200, 80)`, dark-theme
/// value), locked independent of the active theme like `add_bg`/`remove_bg`.
#[must_use]
pub fn add_decoration(_theme: ThemeName) -> StyleColor {
    StyleColor::Rgb(80, 200, 80)
}

/// (diff-02) Red decoration color for the Remove gutter — claude-code
/// `theme.deleteDecoration` (`rgb(220, 90, 90)`).
#[must_use]
pub fn remove_decoration(_theme: ThemeName) -> StyleColor {
    StyleColor::Rgb(220, 90, 90)
}

/// Foreground color for removed-line content text — claude-code's syntax-diff
/// `foreground` (`rgb(248,248,242)`, dark-theme), the near-white default fg the
/// diff renderer layers over the removed-line background. Locked to the
/// dark-theme value like `remove_bg`.
#[must_use]
pub fn remove_text_fg(_theme: ThemeName) -> StyleColor {
    StyleColor::Rgb(248, 248, 242)
}

/// claude-code CHANGE_THRESHOLD: above this changed-fraction, word diffing is
/// abandoned for whole-line coloring (lines too dissimilar to align words).
const CHANGE_THRESHOLD: f64 = 0.4;

/// Brighter emphasis background for the changed *words* of a paired add line —
/// claude-code syntax-diff `addWord` (`rgb(4,71,0)`, dark-theme truecolor).
#[must_use]
pub fn add_word_bg(_theme: ThemeName) -> StyleColor {
    StyleColor::Rgb(4, 71, 0)
}

/// Brighter emphasis background for the changed *words* of a paired remove
/// line — claude-code syntax-diff `deleteWord` (`rgb(92,2,0)`, dark-theme).
#[must_use]
pub fn remove_word_bg(_theme: ThemeName) -> StyleColor {
    StyleColor::Rgb(92, 2, 0)
}

/// Classify a single `similar` line change into a [`DiffRow`] — the ONE source
/// of truth for diff line classification. Maps the change tag to a [`LineKind`],
/// strips the trailing newline, and derives the line number from the change's
/// file index (claude-code numbering: removed lines against the old file,
/// added/context against the new file). The `grouped_hunks` production path is
/// the sole caller; keeping classification here means there is exactly one
/// place that decides add/remove/context + line number.
fn classify_change(change: &similar::Change<&str>) -> DiffRow {
    let text = change.value().trim_end_matches('\n').to_string();
    let (kind, line_no) = match change.tag() {
        ChangeTag::Delete => (LineKind::Remove, change.old_index().map_or(0, |i| i + 1)),
        ChangeTag::Insert => (LineKind::Add, change.new_index().map_or(0, |i| i + 1)),
        ChangeTag::Equal => (LineKind::Context, change.new_index().map_or(0, |i| i + 1)),
    };
    DiffRow {
        kind,
        text,
        line_no,
    }
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
/// space + sigil + space, carrying the line background. (diff-02) Colored
/// green/red for Add/Remove (claude-code `decorationColor`); dim BrightBlack
/// only for Context.
fn gutter_span(row: &DiffRow, gutter_w: usize, bg: StyleColor, theme: ThemeName) -> StyledSpan {
    let text = format!("{:>w$} {} ", row.line_no, sigil(row.kind), w = gutter_w);
    let fg = match row.kind {
        LineKind::Add => add_decoration(theme),
        LineKind::Remove => remove_decoration(theme),
        LineKind::Context => StyleColor::Named(NamedColor::BrightBlack),
    };
    StyledSpan::styled(
        text,
        SpanStyle {
            fg,
            bg,
            ..SpanStyle::default()
        },
    )
}

/// Syntax-highlight `text` as a single line and overlay `bg` onto every
/// content span (keep the syntect fg, force the diff background).
fn content_spans(
    text: &str,
    lang: Option<&str>,
    bg: StyleColor,
    theme: ThemeName,
) -> Vec<StyledSpan> {
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
    theme: ThemeName,
    line_bg: StyleColor,
) -> Option<Vec<StyledSpan>> {
    let wd = TextDiff::from_words(remove_text, add_text);
    // Changed fraction = changed words / total words on this side.
    //
    // (diff-04) claude-code's `wordDiffStrings` uses a symmetric CHAR-LENGTH
    // ratio (changedLen / (oldLen+newLen) > 0.4) rather than this per-side
    // word-count. Porting the ratio alone REGRESSES, because it is coupled to
    // the TOKENIZER: `similar`'s `from_words` produces coarser tokens than
    // TS's punctuation-splitting `tokenize` (e.g. `oldName(param)` is not
    // split on `(`), so the char-length ratio comes out above 0.4 and skips
    // word-diff on cases TS keeps. Matching TS requires porting its tokenizer
    // too — the larger work tracked separately; the word-count heuristic
    // stays until then.
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
    let text_fg = if !is_add { Some(remove_text_fg(theme)) } else { None };
    let mut spans = Vec::new();
    for ch in wd.iter_all_changes() {
        let show = matches!(ch.tag(), ChangeTag::Equal)
            || (is_add && ch.tag() == ChangeTag::Insert)
            || (!is_add && ch.tag() == ChangeTag::Delete);
        if !show {
            continue;
        }
        let emphasized = !matches!(ch.tag(), ChangeTag::Equal);
        let fg = text_fg.unwrap_or(StyleColor::Default);
        spans.push(StyledSpan::styled(
            ch.value(),
            SpanStyle {
                bg: if emphasized { emph_bg } else { line_bg },
                fg,
                ..SpanStyle::default()
            },
        ));
    }
    Some(spans)
}

/// Layout a single non-word-diffed row (context, or unpaired/too-dissimilar
/// add/remove): gutter + whole-line syntax-colored content over the line bg.
fn plain_row(
    row: &DiffRow,
    gutter_w: usize,
    lang: Option<&str>,
    theme: ThemeName,
    width: usize,
) -> StyledLine {
    let bg = match row.kind {
        LineKind::Add => add_bg(theme),
        LineKind::Remove => remove_bg(theme),
        LineKind::Context => StyleColor::Default,
    };
    let mut spans = vec![gutter_span(row, gutter_w, bg, theme)];
    // (diff-01) Removed lines render as PLAINTEXT in claude-code (no syntax
    // highlight) — only the red bg decoration marks them. Added/context lines
    // keep syntax coloring.
    if row.kind == LineKind::Remove {
        spans.push(StyledSpan::styled(
            row.text.clone(),
            SpanStyle {
                bg,
                fg: remove_text_fg(theme),
                ..SpanStyle::default()
            },
        ));
    } else {
        spans.extend(content_spans(&row.text, lang, bg, theme));
    }
    let mut line = StyledLine { spans };
    // (diff-03) Pad changed (Add/Remove) rows so the line bg reaches the
    // right edge; Context keeps the terminal-default bg, so no pad.
    if row.kind != LineKind::Context {
        pad_line_to_width(&mut line, width, bg);
    }
    line
}

/// Layout a word-diffed row: gutter + per-word emphasis spans.
fn word_row(
    row: &DiffRow,
    gutter_w: usize,
    content: Vec<StyledSpan>,
    theme: ThemeName,
    width: usize,
) -> StyledLine {
    let bg = match row.kind {
        LineKind::Add => add_bg(theme),
        LineKind::Remove => remove_bg(theme),
        LineKind::Context => StyleColor::Default,
    };
    let mut spans = vec![gutter_span(row, gutter_w, bg, theme)];
    spans.extend(content);
    let mut line = StyledLine { spans };
    if row.kind != LineKind::Context {
        pad_line_to_width(&mut line, width, bg);
    }
    line
}

/// Hard cap on rendered diff body lines (claude-code shows a "… N more lines"
/// footer past a budget). Mirrors the M6 UserToolResult MAX_LINES intent.
pub const MAX_DIFF_LINES: usize = 100;

/// Context radius for grouped (unified-diff) hunks — 3 lines, matching the
/// unified-diff default.
const CONTEXT_RADIUS: usize = 3;

/// Dim `...` separator drawn BETWEEN hunks (claude-code `StructuredDiffList`
/// "ellipsis separators between them"). Replaces unified `@@` headers, which
/// claude-code never renders.
fn hunk_separator() -> StyledLine {
    StyledLine {
        spans: vec![StyledSpan::styled(
            "...".to_string(),
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
                rows.push(classify_change(&change));
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

/// (fileedit-result-added-removed-header) Count of added/removed lines
/// between `old` and `new` — claude-code's `structuredPatch.reduce(...)` line
/// counts, for the `Added N line(s)[, removed M line(s)]` summary header.
#[must_use]
pub fn diff_stats(old: &str, new: &str) -> (usize, usize) {
    let diff = TextDiff::from_lines(old, new);
    let mut additions = 0usize;
    let mut removals = 0usize;
    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Insert => additions += 1,
            ChangeTag::Delete => removals += 1,
            ChangeTag::Equal => {}
        }
    }
    (additions, removals)
}

/// Render a structured diff of `old` → `new`. `path` drives syntax language
/// detection (claude-code's `filePath` prop). Hunks are separated by unified
/// `@@` headers; output past `MAX_DIFF_LINES` body rows is truncated with a
/// "… N more lines" footer. Never panics. Equivalent to
/// [`render_with_width`] with `width = 0` (no right-edge padding — see
/// diff-03 there).
#[must_use]
pub fn render(old: &str, new: &str, path: Option<&str>, theme: ThemeName) -> Vec<StyledLine> {
    render_with_width(old, new, path, theme, 0)
}

/// (diff-03) [`render`], plus padding changed (Add/Remove) rows with spaces
/// carrying the line background out to `width` total columns (claude-code
/// `wrapText`'s "pad changed lines so background extends to edge"). Context
/// rows are never padded (terminal-default background). `width = 0` (or any
/// value at/below the gutter width) disables padding — every row already
/// reaches or exceeds that, so the pad-to-width never has anything to add.
#[must_use]
pub fn render_with_width(
    old: &str,
    new: &str,
    path: Option<&str>,
    theme: ThemeName,
    width: usize,
) -> Vec<StyledLine> {
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
    // (syntax-01) `new` wins for the first-line/shebang heuristic (it's the
    // post-edit content); a pure-delete diff (`new` empty) falls back to `old`.
    let first_line = new.lines().next().or_else(|| old.lines().next());
    let lang = syntax::detect_language_with_first_line(None, path, first_line);

    // Total body rows across all hunks (excludes headers) — used for the
    // truncation footer count.
    let total_body: usize = hunks.iter().map(|(_, rows)| rows.len()).sum();

    let mut out: Vec<StyledLine> = Vec::new();
    let mut body_emitted = 0usize;
    let mut truncated = false;
    let mut first_hunk = true;
    'hunks: for (_header, rows) in hunks {
        // (diff-05) claude-code's StructuredDiffList separates hunks with a dim
        // `...` line between them — it never renders unified `@@ -a,b +c,d @@`
        // headers. Emit the separator before every hunk except the first.
        if !first_hunk {
            out.push(hunk_separator());
        }
        first_hunk = false;
        let laid = layout_rows(&rows, gutter_w, lang.as_deref(), theme, width);
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

/// (diff-03) Append a space-pad span carrying `bg` so `line`'s total display
/// width reaches `width`. No-op if `line` already reaches/exceeds `width`.
fn pad_line_to_width(line: &mut StyledLine, width: usize, bg: StyleColor) {
    let cur_w: usize = line
        .spans
        .iter()
        .map(|s| unicode_width::UnicodeWidthStr::width(s.text.as_str()))
        .sum();
    if cur_w < width {
        line.spans.push(StyledSpan::styled(
            " ".repeat(width - cur_w),
            SpanStyle {
                bg,
                ..SpanStyle::default()
            },
        ));
    }
}

/// Lay out a slice of diff rows into styled lines, pairing adjacent
/// remove→add runs for word-level diffing (claude-code `processAdjacentLines`).
fn layout_rows(
    rows: &[DiffRow],
    gutter_w: usize,
    lang: Option<&str>,
    theme: ThemeName,
    width: usize,
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
                    out.push(word_row(rem, gutter_w, rem_spans, theme, width));
                    out.push(word_row(add, gutter_w, add_spans, theme, width));
                } else {
                    // Too dissimilar — whole-line coloring for both.
                    out.push(plain_row(rem, gutter_w, lang, theme, width));
                    out.push(plain_row(add, gutter_w, lang, theme, width));
                }
            }
            for rem in &removes[pairs..] {
                out.push(plain_row(rem, gutter_w, lang, theme, width));
            }
            for add in &adds[pairs..] {
                out.push(plain_row(add, gutter_w, lang, theme, width));
            }
        } else {
            out.push(plain_row(&rows[i], gutter_w, lang, theme, width));
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

    /// Classified rows from the PRODUCTION path (`grouped_hunks`) flattened
    /// across hunks. The `classify_*` tests assert against this so they
    /// validate the same classification the renderer uses — there is now a
    /// single classification code path (`classify_change`).
    fn classified_rows(old: &str, new: &str) -> Vec<DiffRow> {
        grouped_hunks(old, new)
            .into_iter()
            .flat_map(|(_, rows)| rows)
            .collect()
    }

    #[test]
    fn diff_colors_match_claude_code_syntax_diff_dark() {
        // Byte-exact claude-code 2.1.211 dark-theme SYNTAX-HIGHLIGHTED diff scheme
        // (the `scopes`-carrying renderer LingXi mirrors, not the plain
        // `theme.diff*` tokens).
        assert_eq!(add_bg(ThemeName::Dark), StyleColor::Rgb(2, 40, 0)); // addLine
        assert_eq!(remove_bg(ThemeName::Dark), StyleColor::Rgb(61, 1, 0)); // deleteLine
        assert_eq!(add_word_bg(ThemeName::Dark), StyleColor::Rgb(4, 71, 0)); // addWord
        assert_eq!(remove_word_bg(ThemeName::Dark), StyleColor::Rgb(92, 2, 0)); // deleteWord
        assert_eq!(add_decoration(ThemeName::Dark), StyleColor::Rgb(80, 200, 80)); // addDecoration
        assert_eq!(remove_decoration(ThemeName::Dark), StyleColor::Rgb(220, 90, 90)); // deleteDecoration
        assert_eq!(remove_text_fg(ThemeName::Dark), StyleColor::Rgb(248, 248, 242)); // foreground
    }

    #[test]
    fn classify_pure_add() {
        let rows = classified_rows("a\n", "a\nb\n");
        // a = context, b = add
        assert_eq!(
            rows.iter().map(|r| r.kind).collect::<Vec<_>>(),
            vec![LineKind::Context, LineKind::Add]
        );
    }

    #[test]
    fn classify_pure_remove() {
        let rows = classified_rows("a\nb\n", "a\n");
        assert_eq!(
            rows.iter().map(|r| r.kind).collect::<Vec<_>>(),
            vec![LineKind::Context, LineKind::Remove]
        );
    }

    #[test]
    fn classify_modify_is_remove_then_add() {
        let rows = classified_rows("foo\n", "bar\n");
        assert_eq!(
            rows.iter().map(|r| r.kind).collect::<Vec<_>>(),
            vec![LineKind::Remove, LineKind::Add]
        );
    }

    #[test]
    fn classify_empty_diff_yields_no_hunks() {
        // No changes -> `grouped_ops` produces no groups -> no rows. (The
        // matching `render` behavior — empty output — is covered by
        // `render_empty_diff_is_empty`.)
        let rows = classified_rows("a\nb\n", "a\nb\n");
        assert!(rows.is_empty());
    }

    #[test]
    fn render_with_width_pads_changed_rows_to_full_width() {
        // (diff-03) A short added line, padded to 30 columns total, carries
        // the green bg on the trailing pad span too.
        let lines = render_with_width("a\n", "a\nb\n", Some("x.txt"), ThemeName::Dark, 30);
        let add = lines
            .iter()
            .find(|l| !is_header(l) && rowline(l).contains('+'))
            .expect("an add line");
        let total_w: usize = add
            .spans
            .iter()
            .map(|s| unicode_width::UnicodeWidthStr::width(s.text.as_str()))
            .sum();
        assert_eq!(total_w, 30, "row should be padded to the full width");
        let last = add.spans.last().expect("at least one span");
        assert!(
            last.text.chars().all(|c| c == ' '),
            "pad span is spaces: {last:?}"
        );
        assert_eq!(last.style.bg, add_bg(ThemeName::Dark));
    }

    #[test]
    fn render_with_width_does_not_pad_context_rows() {
        // (diff-03) Context rows keep the terminal-default bg — no pad span.
        let lines = render_with_width("a\n", "a\nb\n", Some("x.txt"), ThemeName::Dark, 30);
        let ctx = lines
            .iter()
            .find(|l| !is_header(l) && !rowline(l).contains('+') && !rowline(l).contains('-'))
            .expect("a context line");
        let total_w: usize = ctx
            .spans
            .iter()
            .map(|s| unicode_width::UnicodeWidthStr::width(s.text.as_str()))
            .sum();
        assert!(
            total_w < 30,
            "context row must not be padded to width: {total_w}"
        );
    }

    #[test]
    fn render_with_width_zero_matches_render() {
        // width=0 is render()'s exact behavior (no padding ever fires).
        let to_lines = |v: &[StyledLine]| v.iter().map(rowline).collect::<Vec<_>>();
        assert_eq!(
            to_lines(&render_with_width(
                "a\n",
                "a\nb\n",
                Some("x.txt"),
                ThemeName::Dark,
                0
            )),
            to_lines(&render("a\n", "a\nb\n", Some("x.txt"), ThemeName::Dark)),
        );
    }

    #[test]
    fn render_pure_add_has_plus_sigil_and_green_bg() {
        let lines = render("a\n", "a\nb\n", Some("x.txt"), ThemeName::Dark);
        // The add line carries a "+" sigil and the content "b", over green bg.
        let add = lines
            .iter()
            .find(|l| !is_header(l) && rowline(l).contains('+'))
            .expect("an add line");
        let joined = rowline(add);
        assert!(joined.contains('b'), "add line content: {joined:?}");
        // At least one span on the add line has the green add background.
        assert!(
            add.spans
                .iter()
                .any(|s| s.style.bg == add_bg(ThemeName::Dark)),
            "add line has green background"
        );
    }

    #[test]
    fn render_pure_remove_has_minus_sigil_and_red_bg() {
        let lines = render("a\nb\n", "a\n", Some("x.txt"), ThemeName::Dark);
        let rem = lines
            .iter()
            .find(|l| !is_header(l) && rowline(l).contains('-'))
            .expect("a - line");
        assert!(rowline(rem).contains('b'));
        assert!(
            rem.spans
                .iter()
                .any(|s| s.style.bg == remove_bg(ThemeName::Dark)),
            "remove line has red background"
        );
    }

    #[test]
    fn render_add_gutter_uses_green_decoration_not_dim() {
        // (diff-02) The Add gutter's sigil+number span is colored green, not
        // the dim BrightBlack used for Context lines.
        let lines = render("a\n", "a\nb\n", Some("x.txt"), ThemeName::Dark);
        let add = lines
            .iter()
            .find(|l| !is_header(l) && rowline(l).contains('+'))
            .expect("an add line");
        let gutter = &add.spans[0];
        assert_eq!(gutter.style.fg, add_decoration(ThemeName::Dark));
    }

    #[test]
    fn render_remove_gutter_uses_red_decoration_not_dim() {
        // (diff-02)
        let lines = render("a\nb\n", "a\n", Some("x.txt"), ThemeName::Dark);
        let rem = lines
            .iter()
            .find(|l| !is_header(l) && rowline(l).contains('-'))
            .expect("a - line");
        let gutter = &rem.spans[0];
        assert_eq!(gutter.style.fg, remove_decoration(ThemeName::Dark));
    }

    #[test]
    fn render_context_gutter_stays_dim() {
        // (diff-02) Context lines keep the BrightBlack dim gutter fg.
        let lines = render("a\n", "a\nb\n", Some("x.txt"), ThemeName::Dark);
        let ctx = lines
            .iter()
            .find(|l| !is_header(l) && !rowline(l).contains('+') && !rowline(l).contains('-'))
            .expect("a context line");
        let gutter = &ctx.spans[0];
        assert_eq!(gutter.style.fg, StyleColor::Named(NamedColor::BrightBlack));
    }

    #[test]
    fn render_gutter_has_line_numbers() {
        let lines = render("a\n", "a\nb\n", Some("x.txt"), ThemeName::Dark);
        let rows = body(&lines);
        // Context line "a" is line 1, add line "b" is line 2.
        assert!(rows.iter().any(|l| rowline(l).contains('1')));
        assert!(rows.iter().any(|l| rowline(l).contains('2')));
    }

    #[test]
    fn diff_stats_counts_additions_and_removals() {
        // (fileedit-result-added-removed-header)
        assert_eq!(diff_stats("a\nb\n", "a\nb\n"), (0, 0));
        assert_eq!(diff_stats("a\n", "a\nb\nc\n"), (2, 0));
        assert_eq!(diff_stats("a\nb\nc\n", "a\n"), (0, 2));
        assert_eq!(diff_stats("a\nb\n", "a\nc\nd\n"), (2, 1));
    }

    #[test]
    fn render_empty_diff_is_empty() {
        // No changes -> no hunks -> empty output (claude-code renders nothing).
        let lines = render("a\nb\n", "a\nb\n", Some("x.txt"), ThemeName::Dark);
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
            ThemeName::Dark,
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
            .any(|s| s.text.contains("oldName") && s.style.bg == remove_word_bg(ThemeName::Dark)));
        assert!(add
            .spans
            .iter()
            .any(|s| s.text.contains("newName") && s.style.bg == add_word_bg(ThemeName::Dark)));
        // The shared word "function" is NOT emphasized.
        assert!(add
            .spans
            .iter()
            .any(|s| s.text.contains("function") && s.style.bg != add_word_bg(ThemeName::Dark)));
    }

    #[test]
    fn word_diff_skipped_when_lines_too_dissimilar() {
        // Wildly different lines (> CHANGE_THRESHOLD changed) fall back to
        // whole-line coloring: no word-emphasis spans.
        let lines = render("aaaaaaaa\n", "zzzzzzzz\n", Some("x.txt"), ThemeName::Dark);
        let add = lines
            .iter()
            .find(|l| !is_header(l) && rowline(l).contains('+'))
            .unwrap();
        assert!(
            add.spans
                .iter()
                .all(|s| s.style.bg != add_word_bg(ThemeName::Dark)),
            "dissimilar lines use whole-line coloring, not word emphasis"
        );
    }

    #[test]
    fn render_with_separated_changes_emits_hunk_separator() {
        // Two change clusters separated by a long unchanged run -> the second
        // cluster is preceded by a dim `...` separator. claude-code's
        // StructuredDiffList never renders unified `@@ -a,b +c,d @@` headers.
        let old = (1..=40)
            .map(|n| format!("line{n}"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        let mut new_lines: Vec<String> = (1..=40).map(|n| format!("line{n}")).collect();
        new_lines[2] = "CHANGED_TOP".into();
        new_lines[37] = "CHANGED_BOTTOM".into();
        let new = new_lines.join("\n") + "\n";
        let lines = render(&old, &new, Some("x.txt"), ThemeName::Dark);
        let separators = lines.iter().filter(|l| rowline(l).trim() == "...").count();
        assert!(
            separators >= 1,
            "separated change clusters are divided by a `...` separator"
        );
        assert!(
            !lines.iter().any(|l| rowline(l).contains("@@")),
            "claude-code never renders unified @@ hunk headers"
        );
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
        let lines = render(&old, &new, Some("x.txt"), ThemeName::Dark);
        // Body capped at MAX_DIFF_LINES; total = 1 header + cap + 1 footer.
        assert!(
            lines.len() <= MAX_DIFF_LINES + 2,
            "truncated to header + cap + footer, got {}",
            lines.len()
        );
        let last = rowline(lines.last().unwrap());
        assert!(
            last.contains("more lines"),
            "truncation footer present: {last:?}"
        );
    }
}

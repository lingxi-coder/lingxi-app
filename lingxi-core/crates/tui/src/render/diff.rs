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
use similar::{ChangeTag, TextDiff};

use crate::render::StyledLine;
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

/// Render a structured diff of `old` → `new`. `path` drives syntax language
/// detection (claude-code's `filePath` prop). Never panics.
#[must_use]
pub fn render(_old: &str, _new: &str, _path: Option<&str>, _theme: &TuiTheme) -> Vec<StyledLine> {
    Vec::new() // implemented in Tasks 8-10
}

#[cfg(test)]
mod tests {
    use super::*;

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
}

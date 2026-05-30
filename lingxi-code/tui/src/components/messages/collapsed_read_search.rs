//! `CollapsedReadSearchContent` — folds Read/Search/List runs into a count line.
//!
//! Folding (from claude-code `CollapsedReadSearchContent.tsx`, non-verbose):
//!   - gutter `  ⎿  ` + comma-joined parts; first part capitalized
//!     (`verb[0].toUpperCase() + verb.slice(1)`).
//!   - verbs: Searching for/Searched for, Reading/Read, Listing/Listed
//!   - nouns: pattern(s), file(s), directory/directories
//!   - all counts 0 → render nothing (claude-code returns null).
//!   - order (within the M7-05 subset): search, then read, then list.
//!   - SCOPE: read/search/list only; git/commit/PR/push/branch,
//!     bash-command, MCP-query, auto-memory, team-memory parts → M8; the
//!     live `⤿` progress hint + min-display-time debounce → M8.
//!   source: claude-code/src/components/messages/CollapsedReadSearchContent.tsx
#![allow(clippy::doc_markdown, clippy::doc_lazy_continuation)]

use iocraft::prelude::*;

use crate::theme::TuiTheme;

/// Dim gutter for the summary + each expanded entry row (2 spaces + U+23BF +
/// 2 spaces).
pub const GUTTER: &str = "  \u{23BF}  ";

/// The three M7-05 counts + active flag.
#[derive(Debug, Clone, Default)]
pub struct CollapsedCounts {
    /// Search (Grep/Glob) tool uses.
    pub search: u64,
    /// File reads.
    pub read: u64,
    /// Directory listings.
    pub list: u64,
    /// `true` → present-tense verbs.
    pub is_active: bool,
}

fn cap_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Build the comma-joined summary (without the gutter). Empty when all 0.
#[must_use]
pub fn render_summary(c: &CollapsedCounts) -> String {
    let mut parts: Vec<String> = Vec::new();
    if c.search > 0 {
        let verb = if c.is_active {
            "searching for"
        } else {
            "searched for"
        };
        let noun = if c.search == 1 { "pattern" } else { "patterns" };
        parts.push(format!("{verb} {} {noun}", c.search));
    }
    if c.read > 0 {
        let verb = if c.is_active { "reading" } else { "read" };
        let noun = if c.read == 1 { "file" } else { "files" };
        parts.push(format!("{verb} {} {noun}", c.read));
    }
    if c.list > 0 {
        let verb = if c.is_active { "listing" } else { "listed" };
        let noun = if c.list == 1 {
            "directory"
        } else {
            "directories"
        };
        parts.push(format!("{verb} {} {noun}", c.list));
    }
    if parts.is_empty() {
        return String::new();
    }
    // Capitalize the first part only.
    let mut joined = cap_first(&parts[0]);
    for p in &parts[1..] {
        joined.push_str(", ");
        joined.push_str(p);
    }
    joined
}

/// Pure string renderer: gutter + summary; expanded → + indented entry rows.
/// All counts 0 → empty string.
#[must_use]
pub fn render_collapsed_to_string(
    c: &CollapsedCounts,
    entries: &[String],
    expanded: bool,
) -> String {
    let summary = render_summary(c);
    if summary.is_empty() {
        return String::new();
    }
    let mut out = format!("{GUTTER}{summary}");
    if expanded {
        for e in entries {
            out.push('\n');
            out.push_str(GUTTER);
            out.push_str(e);
        }
    }
    out
}

/// Props for [`CollapsedReadSearchContent`].
#[derive(Debug, Clone, Default, Props)]
pub struct CollapsedReadSearchProps {
    /// search/read/list counts + active flag.
    pub counts: CollapsedCounts,
    /// Per-entry display lines (expanded mode).
    pub entries: Vec<String>,
    /// Expanded state (from `AppState.expanded`).
    pub expanded: bool,
}

/// iocraft component.
#[component]
pub fn CollapsedReadSearchContent(
    props: &CollapsedReadSearchProps,
) -> impl Into<AnyElement<'static>> {
    let body = render_collapsed_to_string(&props.counts, &props.entries, props.expanded);
    element! {
        View(flex_direction: FlexDirection::Column) {
            Text(content: body, color: TuiTheme::DIM)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gutter_bytes() {
        assert_eq!(
            GUTTER.as_bytes(),
            &[0x20, 0x20, 0xE2, 0x8E, 0xBF, 0x20, 0x20]
        );
    }

    #[test]
    fn finalized_summary() {
        let c = CollapsedCounts {
            search: 2,
            read: 1,
            list: 0,
            is_active: false,
        };
        assert_eq!(
            render_collapsed_to_string(&c, &[], false),
            "  \u{23BF}  Searched for 2 patterns, read 1 file"
        );
    }

    #[test]
    fn active_present_tense() {
        let c = CollapsedCounts {
            search: 0,
            read: 3,
            list: 0,
            is_active: true,
        };
        assert_eq!(
            render_collapsed_to_string(&c, &[], false),
            "  \u{23BF}  Reading 3 files"
        );
    }

    #[test]
    fn list_directories_plural() {
        let c = CollapsedCounts {
            search: 0,
            read: 0,
            list: 2,
            is_active: false,
        };
        assert_eq!(
            render_collapsed_to_string(&c, &[], false),
            "  \u{23BF}  Listed 2 directories"
        );
    }

    #[test]
    fn single_pattern_singular() {
        let c = CollapsedCounts {
            search: 1,
            read: 0,
            list: 0,
            is_active: false,
        };
        assert_eq!(
            render_collapsed_to_string(&c, &[], false),
            "  \u{23BF}  Searched for 1 pattern"
        );
    }

    #[test]
    fn zero_counts_empty() {
        let c = CollapsedCounts::default();
        assert_eq!(render_collapsed_to_string(&c, &[], false), "");
    }
}

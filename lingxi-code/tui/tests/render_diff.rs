//! Structure snapshots for `render::diff` (M7-02).
//! PARITY (design §0 Q3): snapshot STRUCTURE — sigils (+/-/space), gutter line
//! numbers, hunk headers, and per span Colored/Plain/Emphasized — NOT exact
//! per-token colors. Concrete colors are normalized to C/P/E flags.
use tui::render::diff::{add_word_bg, remove_word_bg, render};
use tui::render::{StyleColor, StyledLine};
use tui::theme::ThemeName;

/// Flag each span: 'E' if it carries a word-emphasis bg, 'C' if fg colored,
/// else 'P'. Prefix each span with the flag + its literal text so the snapshot
/// is color-stable but structure-revealing.
fn structure(lines: &[StyledLine]) -> String {
    let mut out = String::new();
    for (i, l) in lines.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        for s in &l.spans {
            let emph = s.style.bg == add_word_bg(ThemeName::Dark)
                || s.style.bg == remove_word_bg(ThemeName::Dark);
            let flag = if emph {
                'E'
            } else if s.style.fg == StyleColor::Default {
                'P'
            } else {
                'C'
            };
            out.push('[');
            out.push(flag);
            out.push(']');
            out.push_str(&s.text);
        }
    }
    out
}

#[test]
fn snapshot_pure_add() {
    insta::assert_snapshot!(structure(&render(
        "a\n",
        "a\nb\n",
        Some("x.rs"),
        ThemeName::Dark
    )));
}

#[test]
fn snapshot_pure_remove() {
    insta::assert_snapshot!(structure(&render(
        "a\nb\n",
        "a\n",
        Some("x.rs"),
        ThemeName::Dark
    )));
}

#[test]
fn snapshot_modify_mixed() {
    insta::assert_snapshot!(structure(&render(
        "foo\nkeep\n",
        "bar\nkeep\n",
        Some("x.rs"),
        ThemeName::Dark
    )));
}

#[test]
fn snapshot_word_level_intraline() {
    insta::assert_snapshot!(structure(&render(
        "function oldName(p)\n",
        "function newName(p)\n",
        Some("x.js"),
        ThemeName::Dark
    )));
}

#[test]
fn snapshot_empty_diff() {
    let s = structure(&render("a\nb\n", "a\nb\n", Some("x.rs"), ThemeName::Dark));
    assert!(!s.contains('+') && !s.contains('-'));
    insta::assert_snapshot!(s);
}

#[test]
fn snapshot_large_diff_truncation() {
    let new = (0..160)
        .map(|n| format!("L{n}"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    let s = structure(&render("", &new, Some("x.txt"), ThemeName::Dark));
    assert!(s.contains("more lines"), "truncation footer in snapshot");
    insta::assert_snapshot!(s);
}

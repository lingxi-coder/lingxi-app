//! Structure snapshots for `render::syntax` (M7-02).
//! PARITY (design §0 Q3): we snapshot STRUCTURE — line count, and per span
//! whether it is "colored" (fg != default) or "plain" — NOT exact colors.
//! Concrete colors are normalized so a syntect theme bump never breaks us.
use lingxi_tui::render::syntax::highlight;
use lingxi_tui::render::{StyleColor, StyledLine};
use lingxi_tui::theme::ThemeName;

/// Render each line as "C"/"P" per span (Colored / Plain) + the text, so the
/// snapshot captures structure without baking in concrete ANSI colors.
fn structure(lines: &[StyledLine]) -> String {
    let mut out = String::new();
    for (i, l) in lines.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        for s in &l.spans {
            let flag = if s.style.fg == StyleColor::Default {
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
fn snapshot_rust() {
    let s = structure(&highlight(
        "fn main() {\n    let x = 1;\n}\n",
        Some("rust"),
        ThemeName::Dark,
    ));
    insta::assert_snapshot!(s);
}

#[test]
fn snapshot_python() {
    let s = structure(&highlight(
        "def f(x):\n    return x + 1\n",
        Some("python"),
        ThemeName::Dark,
    ));
    insta::assert_snapshot!(s);
}

#[test]
fn snapshot_js() {
    let s = structure(&highlight(
        "const a = () => 42;\n",
        Some("js"),
        ThemeName::Dark,
    ));
    insta::assert_snapshot!(s);
}

#[test]
fn snapshot_json() {
    let s = structure(&highlight(
        "{\n  \"k\": 1\n}\n",
        Some("json"),
        ThemeName::Dark,
    ));
    insta::assert_snapshot!(s);
}

#[test]
fn snapshot_unknown_lang_is_all_plain() {
    let s = structure(&highlight(
        "alpha\nbeta\n",
        Some("klingon"),
        ThemeName::Dark,
    ));
    // Every span flagged [P]; assert structurally AND snapshot.
    assert!(!s.contains("[C]"), "unknown lang must be all-plain");
    insta::assert_snapshot!(s);
}

#[test]
fn snapshot_empty_code_block() {
    let s = structure(&highlight("", Some("rust"), ThemeName::Dark));
    assert!(s.is_empty());
    insta::assert_snapshot!(s);
}

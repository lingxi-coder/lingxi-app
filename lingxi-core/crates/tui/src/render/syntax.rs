//! syntect syntax highlighting wrapper (M7-02).
//!
//! Parity (design §0 Q3): equivalent-look highlighting, NOT byte-identical to
//! highlight.js. Tests assert which spans are colored, not exact colors.
//!
//! M7-01 type note: `StyledSpan` is the nested shape
//! `{ text, style: SpanStyle { fg, bg, bold, .. }, kind }`, NOT a flat
//! `{ text, fg, bg, bold }`. A syntect `Color { r, g, b, a }` maps to
//! `StyleColor::Rgb(r, g, b)` (alpha dropped); `FontStyle::BOLD` → `style.bold`.

// `highlight.js`, `syntect`, `base16-ocean.dark` etc. read better unquoted in
// the module prose; suppress the doc-markdown nudge crate-wide for this file
// (matches `render/markdown.rs`).
#![allow(clippy::doc_markdown)]

use std::sync::OnceLock;

use syntect::easy::HighlightLines;
use syntect::highlighting::{Color as SynColor, FontStyle, Style as SynStyle, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

use crate::render::{SpanStyle, StyleColor, StyledLine, StyledSpan};
use crate::theme::TuiTheme;

fn syntax_set() -> &'static SyntaxSet {
    static SS: OnceLock<SyntaxSet> = OnceLock::new();
    SS.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn theme_set() -> &'static ThemeSet {
    static TS: OnceLock<ThemeSet> = OnceLock::new();
    TS.get_or_init(ThemeSet::load_defaults)
}

/// Map the active TUI theme to a bundled syntect `.tmTheme` name. Today
/// `TuiTheme` is the M6 unit struct (one fixed palette), so this returns the
/// single dark default. M7-15 (theme picker) expands `TuiTheme` into a
/// registry and generalizes this to a per-theme lookup (light themes map to
/// "InspiredGitHub", etc.). The returned name MUST exist in
/// `ThemeSet::load_defaults().themes` (default set ships base16-ocean.dark,
/// base16-ocean.light, base16-eighties.dark, base16-mocha.dark,
/// InspiredGitHub, Solarized (dark), Solarized (light)).
#[must_use]
pub fn tm_theme_for(_theme: &TuiTheme) -> &'static str {
    "base16-ocean.dark"
}

/// Resolve a language token from a fence info-string (preferred) or a file
/// path extension. Returns a token suitable for syntect's
/// `find_syntax_by_token` / `find_syntax_by_extension`. `None` → plain.
#[must_use]
pub fn detect_language(info_string: Option<&str>, path: Option<&str>) -> Option<String> {
    // Fence info-string wins. CommonMark info-strings may carry metadata
    // after the language (e.g. "rust,ignore" or "ts {1,3}"); take the first
    // whitespace/comma-delimited token.
    if let Some(info) = info_string {
        let token = info
            .split(|c: char| c.is_whitespace() || c == ',')
            .next()
            .unwrap_or("")
            .trim();
        if !token.is_empty() {
            return Some(token.to_string());
        }
    }
    // Fall back to the file extension.
    if let Some(p) = path {
        if let Some(ext) = std::path::Path::new(p).extension().and_then(|e| e.to_str()) {
            if !ext.is_empty() {
                return Some(ext.to_string());
            }
        }
    }
    None
}

/// One plain (uncolored) StyledLine per input line — the fallback when the
/// language is unknown or absent.
fn plain_lines(code: &str) -> Vec<StyledLine> {
    code.lines()
        .map(|line| StyledLine {
            spans: vec![StyledSpan::plain(line)],
        })
        .collect()
}

/// Highlight `code` for `lang` (a fence info-string token or detected
/// language), themed by `theme`. Unknown/None lang → one plain StyledLine
/// per input line. Never panics.
#[must_use]
pub fn highlight(code: &str, lang: Option<&str>, theme: &TuiTheme) -> Vec<StyledLine> {
    if code.is_empty() {
        return Vec::new();
    }
    let ss = syntax_set();
    // Resolve the syntax: by token first (info-string), then by extension.
    let syntax = lang.and_then(|l| {
        ss.find_syntax_by_token(l)
            .or_else(|| ss.find_syntax_by_extension(l))
    });
    let Some(syntax) = syntax else {
        return plain_lines(code);
    };
    let tm = &theme_set().themes[tm_theme_for(theme)];
    let mut hl = HighlightLines::new(syntax, tm);
    let mut out = Vec::new();
    for line in LinesWithEndings::from(code) {
        // highlight_line never panics on the bundled syntaxes; on the rare
        // regex error, degrade that line to plain rather than unwrap-panic.
        let ranges = hl.highlight_line(line, ss).unwrap_or_default();
        let spans = if ranges.is_empty() {
            vec![StyledSpan::plain(line.trim_end_matches('\n'))]
        } else {
            ranges
                .into_iter()
                .map(|(st, text)| syn_span_to_styled(st, text))
                .collect()
        };
        out.push(StyledLine { spans });
    }
    out
}

/// Convert a syntect (Style, &str) range into a StyledSpan. Strips the
/// trailing newline so StyledLine spans never carry "\n". Code blocks keep
/// the terminal background; only the foreground is themed (bg = Default).
fn syn_span_to_styled(st: SynStyle, text: &str) -> StyledSpan {
    StyledSpan::styled(
        text.trim_end_matches('\n'),
        SpanStyle {
            fg: syn_color_to_style_color(st.foreground),
            bg: StyleColor::Default,
            bold: st.font_style.contains(FontStyle::BOLD),
            italic: st.font_style.contains(FontStyle::ITALIC),
            underline: st.font_style.contains(FontStyle::UNDERLINE),
        },
    )
}

/// Map a syntect 24-bit `Color` to the M7-01 `StyleColor::Rgb` (alpha dropped).
fn syn_color_to_style_color(c: SynColor) -> StyleColor {
    StyleColor::Rgb(c.r, c.g, c.b)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A span is "colored" if its fg differs from the plain (terminal-default)
    /// foreground. Parity = equivalent look, not exact colors (§0 Q3).
    fn is_colored(s: &StyledSpan) -> bool {
        s.style.fg != StyleColor::Default
    }

    #[test]
    fn detects_from_fence_info_string() {
        assert_eq!(detect_language(Some("rust"), None).as_deref(), Some("rust"));
        // info-string may carry extra metadata: "```rust,ignore" -> first token
        assert_eq!(
            detect_language(Some("rust,ignore"), None).as_deref(),
            Some("rust")
        );
        assert_eq!(
            detect_language(Some("python"), None).as_deref(),
            Some("python")
        );
    }

    #[test]
    fn detects_from_path_extension() {
        assert_eq!(
            detect_language(None, Some("src/main.rs")).as_deref(),
            Some("rs")
        );
        assert_eq!(
            detect_language(None, Some("a/b/app.py")).as_deref(),
            Some("py")
        );
        assert_eq!(
            detect_language(None, Some("data.json")).as_deref(),
            Some("json")
        );
    }

    #[test]
    fn fence_info_string_wins_over_path() {
        assert_eq!(
            detect_language(Some("js"), Some("file.py")).as_deref(),
            Some("js")
        );
    }

    #[test]
    fn no_lang_returns_none() {
        assert_eq!(detect_language(None, None), None);
        assert_eq!(detect_language(Some(""), None), None);
        assert_eq!(detect_language(None, Some("Makefile")), None); // no extension
    }

    #[test]
    fn highlight_rust_keeps_line_count_and_colors_some_spans() {
        let theme = TuiTheme;
        let code = "fn main() {\n    let x = 1;\n}\n";
        let lines = highlight(code, Some("rust"), &theme);
        // STRUCTURE assertion (parity = equivalent look, not exact colors):
        assert_eq!(lines.len(), 3, "one StyledLine per source line");
        // At least one span on the keyword line is non-default colored.
        let any_colored = lines.iter().flat_map(|l| &l.spans).any(is_colored);
        assert!(
            any_colored,
            "rust highlight produced at least one colored span"
        );
    }

    #[test]
    fn highlight_unknown_lang_is_plain_one_span_per_line() {
        let theme = TuiTheme;
        let lines = highlight("alpha\nbeta\n", Some("not-a-language"), &theme);
        assert_eq!(lines.len(), 2);
        for l in &lines {
            assert_eq!(l.spans.len(), 1, "plain fallback = single span per line");
            assert!(!is_colored(&l.spans[0]), "plain fallback span is uncolored");
        }
    }

    #[test]
    fn highlight_none_lang_is_plain() {
        let theme = TuiTheme;
        let lines = highlight("just text\n", None, &theme);
        assert_eq!(lines.len(), 1);
        assert!(!is_colored(&lines[0].spans[0]));
    }

    #[test]
    fn highlight_empty_is_empty() {
        let theme = TuiTheme;
        assert!(highlight("", Some("rust"), &theme).is_empty());
    }

    #[test]
    fn tm_theme_for_returns_a_bundled_theme_name() {
        let name = tm_theme_for(&TuiTheme);
        // The default syntect ThemeSet must contain whatever name we map to,
        // or HighlightLines::new would panic on the index in `highlight`.
        assert!(
            theme_set().themes.contains_key(name),
            "{name} is a bundled theme"
        );
    }
}

//! syntect syntax highlighting wrapper (M7-02).
//!
//! Parity (design §0 Q3): equivalent-look highlighting, NOT byte-identical to
//! highlight.js. Tests assert which spans are colored, not exact colors.
use crate::render::StyledLine;
use crate::theme::TuiTheme;

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

/// Highlight `code` for `lang` (a fence info-string token or detected
/// language), themed by `theme`. Unknown/None lang → one plain StyledLine
/// per input line. Never panics.
#[must_use]
pub fn highlight(_code: &str, _lang: Option<&str>, _theme: &TuiTheme) -> Vec<StyledLine> {
    Vec::new() // implemented in Task 4
}

#[cfg(test)]
mod tests {
    use super::*;

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
}

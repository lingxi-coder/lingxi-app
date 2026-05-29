//! syntect syntax highlighting wrapper (M7-02).
//!
//! Parity (design §0 Q3): equivalent-look highlighting, NOT byte-identical to
//! highlight.js. Tests assert which spans are colored, not exact colors.
use crate::render::StyledLine;
use crate::theme::TuiTheme;

/// Highlight `code` for `lang` (a fence info-string token or detected
/// language), themed by `theme`. Unknown/None lang → one plain StyledLine
/// per input line. Never panics.
#[must_use]
pub fn highlight(_code: &str, _lang: Option<&str>, _theme: &TuiTheme) -> Vec<StyledLine> {
    Vec::new() // implemented in Task 4
}

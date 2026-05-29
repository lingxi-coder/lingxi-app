//! StructuredDiff viewer (M7-02) — similar line+word diff, syntect-colored.
//!
//! Layout parity: claude-code/src/components/StructuredDiff/Fallback.tsx.
use crate::render::StyledLine;
use crate::theme::TuiTheme;

/// Render a structured diff of `old` → `new`. `path` drives syntax language
/// detection (claude-code's `filePath` prop). Never panics.
#[must_use]
pub fn render(_old: &str, _new: &str, _path: Option<&str>, _theme: &TuiTheme) -> Vec<StyledLine> {
    Vec::new() // implemented in Tasks 7-11
}

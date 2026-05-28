//! Shared text formatter used by `/mcp`, `/hooks`, and `/agents`.
//!
//! Locked layout (M5-11 T0 step 2 L6/L7/L8):
//!   `"<Label> ({count}):\n  <row1>\n  <row2>\n…"`
//! with each row pre-formatted by the caller as a single line (no trailing
//! newline). The render adds the two leading spaces + trailing newline per
//! row, and the trailing newline at end of the buffer.

/// Render a labelled list with the locked layout.
#[must_use]
pub fn render_list(label: &str, rows: Vec<String>) -> String {
    let count = rows.len();
    let mut out = format!("{label} ({count}):\n");
    for row in rows {
        out.push_str("  ");
        out.push_str(&row);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_list_renders_header_only() {
        let s = render_list("MCP servers", vec![]);
        assert_eq!(s, "MCP servers (0):\n");
    }

    #[test]
    fn multi_row_renders_with_leading_two_spaces() {
        let s = render_list(
            "Hooks",
            vec!["fmt PostToolUse 60000ms".into(), "lint Stop 30000ms".into()],
        );
        assert_eq!(
            s,
            "Hooks (2):\n  fmt PostToolUse 60000ms\n  lint Stop 30000ms\n"
        );
    }
}

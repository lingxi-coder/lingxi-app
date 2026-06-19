//! Stateless renderer for `/help` output. Produces a byte-locked string
//! formatted per plan M5-10 Task 0 step 2.
//!
//! Format:
//!
//! ```text
//! Commands:\n
//!   /<name padded to longest+2>  <description>\n
//!   ... (99 lines, sorted ASCII-ascending) ...
//! ```
//!
//! Where `<description>` is `core_description(name)` for the 18 core
//! commands and the literal `"(unimplemented in v0.6.0)"` for the other
//! 81 non-core entries. Total = 1 header + 99 commands = 100 lines.

use crate::builtin_support::names::{core_description, BUILTIN_COMMAND_NAMES, BUILTIN_CORE_NAMES};

/// Render the locked `/help` output as a single `String`.
///
/// The output is **byte-locked** — drift indicates the canonical surface
/// changed and must be re-locked against the parity fixture
/// `parity_help_screen.txt`.
#[must_use]
pub fn render_help_screen() -> String {
    let col1_width = BUILTIN_COMMAND_NAMES
        .iter()
        .map(|n| n.len())
        .max()
        .unwrap_or(0)
        + 2;

    // Capacity hint: header + 99 lines.
    let mut out = String::with_capacity(10 + 99 * (col1_width + 40));
    out.push_str("Commands:\n");

    for name in BUILTIN_COMMAND_NAMES {
        // Column 1: `/<name>` left-padded to col1_width characters.
        out.push_str("  /");
        out.push_str(name);
        let prefix_used = 1 + name.len(); // `/` + name
        if prefix_used < col1_width {
            for _ in prefix_used..col1_width {
                out.push(' ');
            }
        }
        out.push_str("  ");

        // Column 2: core description if known, else unimplemented marker.
        if is_core(name) {
            out.push_str(core_description(name));
        } else {
            out.push_str("(unimplemented in v0.6.0)");
        }
        out.push('\n');
    }

    out
}

fn is_core(name: &str) -> bool {
    BUILTIN_CORE_NAMES.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_starts_with_locked_header() {
        let s = render_help_screen();
        assert!(s.starts_with("Commands:\n"));
    }

    #[test]
    fn output_has_exactly_100_lines() {
        // 1 header + 99 commands = 100 lines (each terminated by '\n').
        let s = render_help_screen();
        let n = s.matches('\n').count();
        assert_eq!(
            n, 100,
            "expected 100 newlines (1 header + 99 commands), got {n}"
        );
    }

    #[test]
    fn first_command_line_is_add_dir_unimplemented() {
        let s = render_help_screen();
        let line2 = s.lines().nth(1).unwrap();
        // Col-1 width = longest_name (rate-limit-options = 18) + 2 = 20.
        // "/add-dir" (8 chars: `/`+`add-dir`) gets 13 spaces of right-padding
        // to reach col-1=20, then 2 spaces of separator before description.
        let expected = format!(
            "  /add-dir{}  (unimplemented in v0.6.0)",
            " ".repeat(20 - 8)
        );
        assert_eq!(line2, expected);
    }

    #[test]
    fn agents_line_uses_core_description() {
        let s = render_help_screen();
        let line = s.lines().find(|l| l.starts_with("  /agents ")).unwrap();
        // "/agents" = 7 chars; pad 13 spaces to col1=20; then 2 separator
        // spaces; then "Manage agent configurations".
        let expected = format!("  /agents{}  Manage agent configurations", " ".repeat(20 - 7));
        assert_eq!(line, expected);
    }

    #[test]
    fn every_command_appears_once() {
        let s = render_help_screen();
        for name in BUILTIN_COMMAND_NAMES {
            // Match the exact line-start pattern `  /<name> ` (with trailing
            // space) to avoid prefix collisions like `/commit` matching
            // inside `/git-commit`.
            let needle = format!("  /{name} ");
            let count = s.matches(&needle).count();
            assert_eq!(count, 1, "/{name} should appear exactly once, got {count}");
        }
    }

    #[test]
    fn output_ends_with_newline() {
        let s = render_help_screen();
        assert!(s.ends_with('\n'));
    }
}

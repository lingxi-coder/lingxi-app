//! Stateless renderer for `/help` output. Produces a byte-locked string
//! formatted per plan M5-10 Task 0 step 2.
//!
//! Format:
//!
//! ```text
//! Commands:\n
//!   /<name padded to longest+2>  <description>\n
//!   ... (73 lines, sorted ASCII-ascending) ...
//! ```
//!
//! Where `<description>` is `core_description(name)` for the 18 core
//! commands and the literal `"(unimplemented in v0.6.0)"` for the other
//! non-core entries. The 26 hidden/disabled commands
//! ([`is_palette_hidden`]) are filtered out to match claude-code's
//! `commands.filter(c => !c.isHidden && !$te(c))` help/palette filter, so the
//! total is 1 header + 73 visible commands = 74 lines.

use crate::builtin_support::names::{
    core_description, is_palette_hidden, BUILTIN_COMMAND_NAMES, BUILTIN_CORE_NAMES,
};

/// Render the locked `/help` output as a single `String`.
///
/// Hidden / disabled commands (per [`is_palette_hidden`]) are omitted, exactly
/// as claude-code's `/help` and slash palette drop any command where
/// `isHidden || isEnabled()===off`.
///
/// The output is **byte-locked** — drift indicates the canonical surface
/// changed and must be re-locked against the parity fixture
/// `parity_help_screen.txt`.
#[must_use]
pub fn render_help_screen() -> String {
    // Column width is computed over the VISIBLE commands only (claude-code
    // never pads to a hidden command's width since the hidden ones never
    // reach the renderer).
    let visible = || BUILTIN_COMMAND_NAMES.iter().filter(|n| !is_palette_hidden(n));

    let col1_width = visible().map(|n| n.len()).max().unwrap_or(0) + 2;

    // Capacity hint: header + visible lines.
    let mut out = String::with_capacity(10 + 80 * (col1_width + 40));
    out.push_str("Commands:\n");

    for name in visible() {
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
    fn output_has_exactly_74_lines() {
        // 1 header + 73 visible commands = 74 lines (each terminated by '\n').
        // The 26 hidden/disabled commands (is_palette_hidden) are filtered out,
        // matching claude-code's `!isHidden && !$te` help/palette filter.
        let s = render_help_screen();
        let n = s.matches('\n').count();
        assert_eq!(
            n, 74,
            "expected 74 newlines (1 header + 73 visible commands), got {n}"
        );
    }

    #[test]
    fn hidden_and_disabled_commands_are_omitted() {
        use crate::builtin_support::names::{CORRECT_BY_DESIGN_STUBS, HIDDEN_PALETTE_COMMANDS};
        let s = render_help_screen();
        for name in HIDDEN_PALETTE_COMMANDS
            .iter()
            .copied()
            .chain(CORRECT_BY_DESIGN_STUBS.iter().map(|(n, _)| *n))
        {
            let needle = format!("  /{name} ");
            assert!(
                !s.contains(&needle),
                "/{name} is hidden/disabled and must not appear in /help"
            );
        }
    }

    #[test]
    fn visible_host_bound_commands_still_appear() {
        // claude-code SHOWS these (no isHidden/isEnabled gate), so /help must too.
        let s = render_help_screen();
        for name in ["btw", "x402", "reload-plugins", "install-slack-app", "mobile", "desktop"] {
            assert!(
                s.contains(&format!("  /{name} ")),
                "/{name} is visible in claude-code and must appear in /help"
            );
        }
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
    fn every_visible_command_appears_once() {
        use crate::builtin_support::names::is_palette_hidden;
        let s = render_help_screen();
        for name in BUILTIN_COMMAND_NAMES {
            // Hidden/disabled commands are filtered out (see is_palette_hidden);
            // only the 73 visible commands appear.
            if is_palette_hidden(name) {
                continue;
            }
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

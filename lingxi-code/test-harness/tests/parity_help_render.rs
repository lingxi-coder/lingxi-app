//! Parity: `/help` byte-locked rendering. Compares the renderer output to
//! the golden `parity_help_screen.txt`.
//!
//! M5-10 Task 10.

use command_api::builtin_support::help_render::render_help_screen;

const GOLDEN: &str = include_str!("../src/parity/fixtures/parity_help_screen.txt");

#[test]
fn render_help_screen_matches_golden_byte_for_byte() {
    let actual = render_help_screen();
    assert_eq!(actual, GOLDEN, "/help output drifted from golden fixture");
}

#[test]
fn golden_starts_with_locked_header() {
    assert!(GOLDEN.starts_with("Commands:\n"));
}

#[test]
fn golden_has_76_lines() {
    // 1 header + 75 VISIBLE commands = 76 lines (each `\n`-terminated).
    // The 26 hidden/disabled commands (is_palette_hidden) are filtered out,
    // matching claude-code's `commands.filter(c => !c.isHidden && !$te(c))`.
    // (101 builtins − 26 hidden = 75, no DISABLE_*_COMMAND env set.)
    assert_eq!(GOLDEN.matches('\n').count(), 76);
}

#[test]
fn golden_omits_hidden_and_disabled_commands() {
    use command_api::builtin_support::names::{CORRECT_BY_DESIGN_STUBS, HIDDEN_PALETTE_COMMANDS};
    for name in HIDDEN_PALETTE_COMMANDS
        .iter()
        .copied()
        .chain(CORRECT_BY_DESIGN_STUBS.iter().map(|(n, _)| *n))
    {
        assert!(
            !GOLDEN.contains(&format!("  /{name} ")),
            "/{name} is hidden/disabled and must not appear in the /help golden"
        );
    }
}

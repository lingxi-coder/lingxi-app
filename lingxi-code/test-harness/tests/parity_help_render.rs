//! Parity: `/help` byte-locked rendering. Compares the renderer output to
//! the golden `parity_help_screen.txt`.
//!
//! M5-10 Task 10.

use lingxi_commands::builtin::help_render::render_help_screen;

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
fn golden_has_100_lines() {
    // 1 header + 99 commands = 100 lines (each `\n`-terminated).
    assert_eq!(GOLDEN.matches('\n').count(), 100);
}

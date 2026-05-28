//! Snapshot: the M6-01 placeholder root renders a single line containing
//! `"lingxi-tui v"` + the crate version. We assert on the string-form
//! version line (sourced from `env!("CARGO_PKG_VERSION")`) rather than
//! the full ANSI buffer — full buffer snapshots are introduced in M6-02
//! once the StatusLine/PromptInput surfaces stabilise.

#[test]
fn placeholder_line_matches_snapshot() {
    // Sourced inline from CARGO_PKG_VERSION (build-time const, stable
    // across `cargo test` runs). Bump-safe: M6-09 bumps the crate to
    // 0.7.0 and the snapshot will need a one-line update.
    let line = format!("lingxi-tui v{}", env!("CARGO_PKG_VERSION"));
    insta::assert_snapshot!("placeholder_line", line);
}

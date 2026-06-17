//! Integration smoke test for M6-01 TUI routing.
//!
//! When `lingxi-cli` is invoked with stdin redirected from `/dev/null`
//! (i.e. non-TTY) and no prompt, it must fall back to `StdioRepl`, NOT
//! enter the TUI (which would deadlock on `crossterm::event::EventStream`
//! in a non-TTY context). The PTY smoke proves the routing decision —
//! it does NOT exercise the iocraft render loop (that's M6-09).

use assert_cmd::Command;
use std::time::Duration;

/// Stdin from `/dev/null` (immediate EOF) → REPL prints "> " then exits 0
/// on EOF. This matches v0.6.0 REPL behavior.
#[test]
fn non_tty_stdin_routes_to_stdio_repl_and_exits() {
    let mut cmd = Command::cargo_bin("lingxi-cli").unwrap();
    // Empty stdin → EOF → REPL exits 0 (v0.6.0 contract).
    cmd.write_stdin("");
    cmd.timeout(Duration::from_secs(5));
    let output = cmd.assert().success().get_output().clone();
    let stderr = String::from_utf8_lossy(&output.stderr);
    // The v0.6.0 REPL prints "> " to stderr (idle prompt). Asserting that
    // proves we did NOT take the TUI path (which would print iocraft
    // ANSI escape sequences to stdout instead).
    assert!(
        stderr.contains("> "),
        "expected stdio REPL prompt; got stderr={stderr:?}"
    );
}

/// `--no-tui` with an empty prompt also routes to `StdioRepl` even if a
/// TTY were detected. We can't easily fake a TTY in `assert_cmd`, but
/// the `--no-tui` flag short-circuits TTY detection.
#[test]
fn no_tui_flag_takes_stdio_path() {
    let mut cmd = Command::cargo_bin("lingxi-cli").unwrap();
    cmd.arg("--no-tui");
    cmd.write_stdin("");
    cmd.timeout(Duration::from_secs(5));
    cmd.assert().success();
}

/// `-p "hello"` always prints one-shot regardless of TTY state. The CLI's
/// `posix-minimal` HTTP transport is a stub that fails every request with a
/// (retryable) "connection failed" — so the single turn fails and the binary
/// exits. We only care that it exits without the TUI hijacking the terminal.
///
/// `CLAUDE_CODE_MAX_RETRIES=0` is REQUIRED for determinism: with the default of
/// 10 retries the retryable stub error is retried with exponential backoff,
/// whose accumulated sleeps exceed any short test timeout (the binary would
/// eventually exit, but only after the full backoff sequence). Capping retries
/// makes the failure-then-exit immediate, independent of the retry-backoff
/// schedule and of any network.
#[test]
fn print_mode_unaffected_by_tui_routing() {
    let mut cmd = Command::cargo_bin("lingxi-cli").unwrap();
    cmd.arg("-p").arg("hello");
    cmd.env("CLAUDE_CODE_MAX_RETRIES", "0");
    cmd.timeout(Duration::from_secs(10));
    // Exit code may be 0 or 1 depending on the env. We assert only that the
    // process terminates within the timeout (no TUI takeover).
    let output = cmd.output().expect("binary ran");
    assert!(output.status.code().is_some(), "process did not terminate");
}

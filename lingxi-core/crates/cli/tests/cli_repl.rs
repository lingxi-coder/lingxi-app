//! End-to-end REPL tests driven by `assert_cmd` with stdin scripted via
//! `write_stdin`.  Tests the full binary in REPL mode (no positional prompt
//! argument).
//!
//! See plan `docs/superpowers/plans/2026-05-25-m5-13-repl-mode.md` Task 6.

use assert_cmd::Command;
use predicates::prelude::*;

/// Convenience: build a `Command` that runs the binary in REPL mode
/// (no positional prompt, fake API key so runtime construction succeeds).
fn repl_cmd() -> Command {
    let mut c = Command::cargo_bin("lingxi-cli").unwrap();
    c.env("ANTHROPIC_API_KEY", "sk-ant-test-repl-fake");
    c
}

#[test]
fn exit_command_terminates_repl_with_code_0() {
    repl_cmd()
        .write_stdin("/exit\n")
        .assert()
        .code(0)
        .stdout(predicate::str::contains("Exiting."));
}

#[test]
fn eof_terminates_repl_with_code_0() {
    // Empty stdin → immediate EOF → REPL exits 0.
    repl_cmd().write_stdin("").assert().code(0);
}

#[test]
fn version_command_in_repl_prints_version_then_eof_exits_0() {
    repl_cmd()
        .write_stdin("/version\n")
        .assert()
        .code(0)
        .stdout(predicate::str::contains("lingxi-cli "));
}

#[test]
fn unknown_slash_command_prints_unknown_then_exit_works() {
    repl_cmd()
        .write_stdin("/zzz-not-a-command\n/exit\n")
        .assert()
        .code(0)
        .stdout(predicate::str::contains(
            "Unknown command: /zzz-not-a-command",
        ))
        .stdout(predicate::str::contains("Exiting."));
}

#[test]
fn empty_lines_just_loop_then_exit() {
    repl_cmd().write_stdin("\n\n/exit\n").assert().code(0);
}

#[test]
fn prompt_appears_on_stderr_not_stdout() {
    use std::io::Write;
    use std::process::Stdio;

    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_lingxi-cli"))
        .env("ANTHROPIC_API_KEY", "sk-ant-test-repl-fake")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn failed");

    child.stdin.as_mut().unwrap().write_all(b"/exit\n").unwrap();

    let output = child.wait_with_output().expect("wait failed");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("> "),
        "expected '> ' on stderr, got: {stderr:?}"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("> "),
        "prompt leaked to stdout: {stdout:?}"
    );
}

#[test]
fn multiple_slash_commands_then_exit() {
    let output = repl_cmd()
        .write_stdin("/version\n/version\n/exit\n")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let count = stdout.matches("lingxi-cli ").count();
    assert!(
        count >= 2,
        "expected /version to appear >=2 times in stdout, got {count}: {stdout:?}"
    );
}

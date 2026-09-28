//! End-to-end REPL tests driven by `assert_cmd` with stdin scripted via
//! `write_stdin`.  Tests the full binary in REPL mode (no positional prompt
//! argument).
//!
//! See plan `docs/superpowers/plans/2026-05-25-m5-13-repl-mode.md` Task 6.

use assert_cmd::Command;
use predicates::prelude::*;

/// Convenience: build a `Command` that runs the binary in REPL mode
/// (no positional prompt, fake API key so runtime construction succeeds).
///
/// The returned `TempDir` is the run's `$LINGXI_CONFIG_DIR` and must stay bound
/// for as long as the command runs. Rooting the config home in the OS temp
/// directory is what keeps this spawned binary off the machine's native
/// credential store: the engine reads the root and picks the plaintext store
/// for a throwaway home (`credential_root_is_ephemeral`). Without it the REPL
/// boots against the real login keychain, which on macOS means the credential
/// broker refuses an unpackaged process and every test here dies at startup
/// with "secure storage init failed" before it can script a single line.
fn repl_cmd() -> (Command, tempfile::TempDir) {
    let home = tempfile::tempdir().unwrap();
    let mut c = Command::cargo_bin("lingxi-cli").unwrap();
    c.env("ANTHROPIC_API_KEY", "sk-ant-test-repl-fake")
        .env("LINGXI_CONFIG_DIR", home.path());
    (c, home)
}

#[test]
fn exit_command_terminates_repl_with_code_0() {
    let (mut cmd, _home) = repl_cmd();
    cmd.write_stdin("/exit\n")
        .assert()
        .code(0)
        .stdout(predicate::str::contains("Exiting."));
}

#[test]
fn eof_terminates_repl_with_code_0() {
    // Empty stdin → immediate EOF → REPL exits 0.
    let (mut cmd, _home) = repl_cmd();
    cmd.write_stdin("").assert().code(0);
}

#[test]
fn version_command_in_repl_prints_version_then_eof_exits_0() {
    let (mut cmd, _home) = repl_cmd();
    cmd.write_stdin("/version\n")
        .assert()
        .code(0)
        .stdout(predicate::str::contains("lingxi-cli "));
}

#[test]
fn unknown_slash_command_prints_unknown_then_exit_works() {
    let (mut cmd, _home) = repl_cmd();
    cmd.write_stdin("/zzz-not-a-command\n/exit\n")
        .assert()
        .code(0)
        .stdout(predicate::str::contains(
            "Unknown command: /zzz-not-a-command",
        ))
        .stdout(predicate::str::contains("Exiting."));
}

#[test]
fn empty_lines_just_loop_then_exit() {
    let (mut cmd, _home) = repl_cmd();
    cmd.write_stdin("\n\n/exit\n").assert().code(0);
}

#[test]
fn prompt_appears_on_stderr_not_stdout() {
    use std::io::Write;
    use std::process::Stdio;

    let home = tempfile::tempdir().unwrap();
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_lingxi-cli"))
        .env("ANTHROPIC_API_KEY", "sk-ant-test-repl-fake")
        .env("LINGXI_CONFIG_DIR", home.path())
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
    let (mut cmd, _home) = repl_cmd();
    let output = cmd
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

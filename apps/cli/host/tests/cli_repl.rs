//! End-to-end REPL tests driven by `assert_cmd` with stdin scripted via
//! `write_stdin`.  Tests the full binary in REPL mode (no positional prompt
//! argument).
//!
//! See plan `docs/superpowers/plans/2026-05-25-m5-13-repl-mode.md` Task 6.

use assert_cmd::Command;
use predicates::prelude::*;
use std::time::Duration;

#[path = "common/native_fallback.rs"]
mod native_fallback;

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
fn unknown_slash_command_runs_fallback_then_repl_exit_works() {
    const PROMPT: &str = "/zzz-not-a-command keep this prompt";
    const ANSWER: &str = "loopback fallback answer";

    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let stub = native_fallback::LocalMessagesStub::start();
    let mut command = Command::cargo_bin("lingxi-cli").unwrap();
    stub.configure_isolated_command(
        &mut command,
        home.path(),
        cwd.path(),
        "cli-repl-unknown-fallback-loopback-test",
    );
    command.args([
        "--setting-sources",
        "project",
        "--strict-mcp-config",
        "--model",
        "claude-sonnet-4-5",
        "--tools",
        "",
    ]);

    let input = format!("{PROMPT}\n/exit\n");
    let output = command
        .timeout(Duration::from_secs(20))
        .write_stdin(input.into_bytes())
        .output()
        .expect("CLI REPL fallback should finish against the local provider");
    let requests = stub.finish();

    assert_eq!(
        output.status.code(),
        Some(0),
        "the model fallback and following /exit should succeed; stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(ANSWER),
        "local answer missing from REPL: {stdout}"
    );
    assert!(
        stdout.contains("Exiting."),
        "REPL should process /exit: {stdout}"
    );
    native_fallback::assert_unknown_fallback_request(&requests, PROMPT);
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

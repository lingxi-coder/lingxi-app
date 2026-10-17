//! End-to-end slash-command dispatch over the real binary.
//!
//! Local commands stay offline. Unknown slash commands use Native's model
//! fallback, which is exercised against a loopback-only Messages stub.
//!
//! Every test here gives the child a temp `$LINGXI_CONFIG_DIR`. That is what
//! keeps the spawned binary off the machine's native credential store — the
//! engine reads the credential root and picks the plaintext store for a
//! throwaway home. Without it, macOS's credential broker refuses the
//! unpackaged test binary and every case here dies at startup with
//! "secure storage init failed" instead of exercising what it names.

use assert_cmd::Command;
use predicates::prelude::*;
use std::time::Duration;

#[path = "common/native_fallback.rs"]
mod native_fallback;

#[test]
fn slash_version_dispatch_works_without_api() {
    let home = tempfile::tempdir().unwrap();
    Command::cargo_bin("lingxi-cli")
        .unwrap()
        .env("ANTHROPIC_API_KEY", "sk-test-fake")
        .env("LINGXI_CONFIG_DIR", home.path())
        .args(["--model", "claude-sonnet-4-5", "/version"])
        .assert()
        .success()
        .stdout(predicate::str::starts_with("lingxi-cli "));
}

#[test]
fn slash_help_dispatch_works_without_api() {
    let home = tempfile::tempdir().unwrap();
    Command::cargo_bin("lingxi-cli")
        .unwrap()
        .env("ANTHROPIC_API_KEY", "sk-test-fake")
        .env("LINGXI_CONFIG_DIR", home.path())
        .args(["--model", "claude-sonnet-4-5", "/help"])
        .assert()
        .success();
}

#[test]
fn unknown_slash_command_runs_fallback_through_loopback_provider() {
    const PROMPT: &str = "/zzz-not-a-real-command keep this prompt";
    const ANSWER: &str = "loopback fallback answer";

    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let stub = native_fallback::LocalMessagesStub::start();
    let mut command = Command::cargo_bin("lingxi-cli").unwrap();
    stub.configure_isolated_command(
        &mut command,
        home.path(),
        cwd.path(),
        "cli-unknown-fallback-loopback-test",
    );
    command.args([
        "--setting-sources",
        "project",
        "--strict-mcp-config",
        "--print",
        "--model",
        "claude-sonnet-4-5",
        "--tools",
        "",
        "--max-budget-usd",
        "1",
        PROMPT,
    ]);

    let output = command
        .timeout(Duration::from_secs(20))
        .output()
        .expect("CLI fallback should finish against the local provider");
    let requests = stub.finish();

    assert!(
        output.status.success(),
        "unknown slash fallback should complete via the local provider; status={:?}, stdout={}, stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains(ANSWER),
        "the local model response should reach CLI output: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    native_fallback::assert_unknown_fallback_request(&requests, PROMPT);
}

#[test]
fn json_mode_emits_command_output_event() {
    // /version dispatches synchronously; under --json the result lands as
    // a `{"event":"command_output", …}` line.
    let home = tempfile::tempdir().unwrap();
    Command::cargo_bin("lingxi-cli")
        .unwrap()
        .env("ANTHROPIC_API_KEY", "sk-test-fake")
        .env("LINGXI_CONFIG_DIR", home.path())
        .args(["--model", "claude-sonnet-4-5", "--json", "/version"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"event\":\"command_output\""))
        .stdout(predicate::str::contains("\"display\":\"lingxi-cli "));
}

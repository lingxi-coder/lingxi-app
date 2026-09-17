//! End-to-end slash-command dispatch over the real binary.
//!
//! These tests exercise the runtime path that does NOT require a live
//! Anthropic API endpoint — slash commands bypass `run_turn` entirely
//! and resolve against the in-process command registry.
//!
//! Every test here gives the child a temp `$LINGXI_CONFIG_DIR`. That is what
//! keeps the spawned binary off the machine's native credential store — the
//! engine reads the credential root and picks the plaintext store for a
//! throwaway home. Without it, macOS's credential broker refuses the
//! unpackaged test binary and every case here dies at startup with
//! "secure storage init failed" instead of exercising what it names.

use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn slash_version_dispatch_works_without_api() {
    let home = tempfile::tempdir().unwrap();
    Command::cargo_bin("lingxi-cli")
        .unwrap()
        .env("ANTHROPIC_API_KEY", "sk-test-fake")
        .env("LINGXI_CONFIG_DIR", home.path())
        .arg("/version")
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
        .arg("/help")
        .assert()
        .success();
}

#[test]
fn unknown_slash_command_exits_1() {
    let home = tempfile::tempdir().unwrap();
    Command::cargo_bin("lingxi-cli")
        .unwrap()
        .env("ANTHROPIC_API_KEY", "sk-test-fake")
        .env("LINGXI_CONFIG_DIR", home.path())
        .arg("/zzz-not-a-real-command")
        .assert()
        .code(1)
        .stdout(predicate::str::contains(
            "Unknown command: /zzz-not-a-real-command",
        ));
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
        .args(["--json", "/version"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"event\":\"command_output\""))
        .stdout(predicate::str::contains("\"display\":\"lingxi-cli "));
}

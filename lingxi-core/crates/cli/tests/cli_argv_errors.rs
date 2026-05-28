//! Exit-code coverage for argv parse errors + cwd validation + REPL stub.
//!
//! Locks per plan M5-12 Task 0 step 2.

use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn unknown_flag_exits_2() {
    Command::cargo_bin("lingxi-cli")
        .unwrap()
        .arg("--nonexistent-flag")
        .assert()
        .code(2);
}

#[test]
fn cwd_to_nonexistent_path_exits_1() {
    Command::cargo_bin("lingxi-cli")
        .unwrap()
        .args(["--cwd", "/this/does/not/exist/at/all/zzz"])
        .arg("hello")
        .assert()
        .code(1)
        .stderr(predicate::str::contains("--cwd path does not exist"));
}

#[test]
fn empty_prompt_with_no_resume_enters_repl_then_exits_64() {
    Command::cargo_bin("lingxi-cli")
        .unwrap()
        .assert()
        .code(64)
        .stderr(predicate::str::contains("REPL mode not yet wired"));
}

#[test]
fn resume_with_invalid_uuid_exits_1() {
    Command::cargo_bin("lingxi-cli")
        .unwrap()
        .env("ANTHROPIC_API_KEY", "sk-test-fake")
        .args(["--resume", "not-a-uuid"])
        .assert()
        .code(1);
}

#[test]
fn resume_without_value_exits_64() {
    Command::cargo_bin("lingxi-cli")
        .unwrap()
        .env("ANTHROPIC_API_KEY", "sk-test-fake")
        .arg("--resume")
        .assert()
        .code(64)
        .stderr(predicate::str::contains(
            "interactive resume picker not yet wired",
        ));
}

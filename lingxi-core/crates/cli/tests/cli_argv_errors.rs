//! Exit-code coverage for argv parse errors + cwd validation + REPL entry.
//!
//! Locks per plan M5-12 Task 0 step 2.  M5-13 updated the REPL stub to real
//! behaviour: an empty stdin now exits 0 (EOF) instead of 64 (NOT_IMPLEMENTED).

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

/// M5-13: the REPL is real now.  Empty stdin → EOF → exits 0.
/// The `"> "` prompt appears on stderr; stdout gets the EOF newline.
#[test]
fn empty_prompt_with_no_resume_enters_repl_and_exits_0_on_eof() {
    Command::cargo_bin("lingxi-cli")
        .unwrap()
        .env("ANTHROPIC_API_KEY", "sk-ant-test-fake")
        .write_stdin("") // immediate EOF
        .assert()
        .code(0)
        .stderr(predicate::str::contains("> "));
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
